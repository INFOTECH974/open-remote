//! Cadence d'envoi réglée sur ce que la liaison écoule réellement.
//!
//! Le visualiseur accuse réception de chaque image une fois affichée. L'hôte
//! en tire deux mesures :
//!
//! - le **délai de base** : l'aller-retour d'une petite image, donc la latence
//!   pure de la liaison (relais compris) ;
//! - le **débit** : ce que la liaison et l'affichage écoulent par seconde,
//!   mesuré sur les images assez grosses pour que leur transport se voie.
//!
//! Elles décident de deux choses. Combien d'images peuvent être en route à la
//! fois : sur une liaison lente mais large, plusieurs petites images se
//! suivent sans attendre l'accusé de la première, alors que de grosses images
//! sur une liaison étroite sont retenues avant de s'empiler (la latence ne
//! s'accumule pas). Et le poids visé pour une image, d'où la capture déduit
//! la qualité d'encodage : mieux vaut une image un peu floue tout de suite,
//! affinée dès que l'écran se calme, qu'une image nette en retard.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Images en route au plus : de quoi tenir 30 images par seconde jusqu'à
/// environ 250 ms d'aller-retour.
const MAX_IN_FLIGHT: usize = 8;
/// File d'attente tolérée, en secondes de débit : c'est la latence que l'on
/// accepte d'ajouter quand la liaison sature. La fenêtre d'octets en route
/// vaut ce que la liaison contient déjà en temps normal (débit × délai de
/// base) plus cette file.
const QUEUE_SECONDS: f64 = 0.08;
/// Plancher de la fenêtre d'octets : deux petites images se suivent toujours.
const MIN_WINDOW: usize = 24 * 1024;
/// Cadence visée quand tout l'écran bouge : le poids d'une image en découle.
const TARGET_FPS: f64 = 20.0;
/// Poids visé tant que le débit n'est pas connu (première image).
const FIRST_BUDGET: usize = 64 * 1024;
const MIN_BUDGET: usize = 10 * 1024;
/// En dessous de ce poids, une image ne dit rien du débit : elle mesure la latence.
const MEASURABLE: usize = 8 * 1024;
/// Durée de transport minimale retenue pour une mesure de débit (gigue).
const MIN_SERVICE: Duration = Duration::from_millis(5);
/// Le délai de base est le plus petit aller-retour des deux dernières
/// tranches de 15 secondes où il a pu être mesuré.
const BASE_WINDOW: Duration = Duration::from_secs(15);
/// Une image sans accusé de réception depuis ce délai n'est plus attendue.
const STALE: Duration = Duration::from_secs(10);

struct Sent {
    seq: u32,
    at: Instant,
    bytes: usize,
}

/// Minimum glissant de l'aller-retour, sur deux tranches de [`BASE_WINDOW`].
#[derive(Default)]
struct BaseRtt {
    current: Option<Duration>,
    previous: Option<Duration>,
    rotated: Option<Instant>,
}

impl BaseRtt {
    fn observe(&mut self, rtt: Duration, now: Instant) {
        // Une tranche sans mesure ne chasse pas la précédente : pendant un
        // long mouvement (que des grosses images), la dernière valeur reste.
        let rotated = *self.rotated.get_or_insert(now);
        if now.saturating_duration_since(rotated) >= BASE_WINDOW && self.current.is_some() {
            self.previous = self.current.take();
            self.rotated = Some(now);
        }
        self.current = Some(self.current.map_or(rtt, |c| c.min(rtt)));
    }

    fn get(&self) -> Option<Duration> {
        match (self.current, self.previous) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[derive(Default)]
pub struct Pacer {
    in_flight: VecDeque<Sent>,
    bytes_in_flight: usize,
    base: BaseRtt,
    /// Débit estimé, en octets par seconde.
    rate: Option<f64>,
    last_ack: Option<Instant>,
    /// Première grosse image, accusée avant que le délai de base soit connu :
    /// son poids et son aller-retour, pour corriger l'estimation ensuite.
    blind: Option<(usize, Duration)>,
}

impl Pacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// L'image `seq`, de `bytes` octets, vient de partir.
    pub fn on_sent(&mut self, seq: u32, bytes: usize, now: Instant) {
        self.in_flight.push_back(Sent { seq, at: now, bytes });
        self.bytes_in_flight += bytes;
    }

    /// Le visualiseur a affiché l'image `seq` (et donc toutes les précédentes).
    pub fn on_ack(&mut self, seq: u32, at: Instant) {
        let mut bytes = 0;
        let mut first_sent = None;
        let mut last_sent = None;
        while let Some(front) = self.in_flight.front() {
            // Comparaison modulo 2³² : `front.seq` est avant `seq`, ou égal.
            if seq.wrapping_sub(front.seq) >= 1 << 31 {
                break;
            }
            bytes += front.bytes;
            first_sent.get_or_insert(front.at);
            last_sent = Some(front.at);
            self.in_flight.pop_front();
        }
        let (Some(first_sent), Some(last_sent)) = (first_sent, last_sent) else { return };
        self.bytes_in_flight -= bytes;

        if bytes < MEASURABLE {
            let first = self.base.get().is_none();
            self.base.observe(at.saturating_duration_since(last_sent), at);
            // Le débit avait été estimé à l'aveugle, tout l'aller-retour
            // compté comme du transport : on retire la latence.
            if let (true, Some((bytes, rtt)), Some(base)) = (first, self.blind.take(), self.base.get()) {
                let service = rtt.saturating_sub(base).max(MIN_SERVICE);
                self.rate = Some(bytes as f64 / service.as_secs_f64());
            }
        } else {
            // Le transport de ces octets a commencé quand les précédents sont
            // arrivés, ou, si la liaison était libre, au moment où une image
            // vide serait arrivée. Tant que le délai de base est inconnu,
            // tout l'aller-retour compte comme du transport : l'estimation
            // pèche par prudence.
            let mut start = first_sent + self.base.get().unwrap_or_default();
            if let Some(previous) = self.last_ack {
                start = start.max(previous);
            }
            let service = at.saturating_duration_since(start).max(MIN_SERVICE);
            let sample = bytes as f64 / service.as_secs_f64();
            if self.rate.is_none() && self.base.get().is_none() {
                self.blind = Some((bytes, service));
            }
            self.rate = Some(match self.rate {
                None => sample,
                // Une baisse est prise au sérieux plus vite qu'une hausse.
                Some(rate) if sample < rate => rate + (sample - rate) * 0.4,
                Some(rate) => rate + (sample - rate) * 0.3,
            });
        }
        self.last_ack = Some(at);
    }

    /// Vrai si une image de plus peut partir sans allonger la file d'attente.
    pub fn can_send(&self) -> bool {
        if self.in_flight.is_empty() {
            return true;
        }
        if self.in_flight.len() >= MAX_IN_FLIGHT {
            return false;
        }
        let pipe = self.base.get().unwrap_or_default().as_secs_f64() + QUEUE_SECONDS;
        let window = self.rate.map_or(0, |rate| (rate * pipe) as usize).max(MIN_WINDOW);
        self.bytes_in_flight < window
    }

    /// Poids visé pour la prochaine image, en octets.
    pub fn frame_budget(&self) -> usize {
        match self.rate {
            None => FIRST_BUDGET,
            Some(rate) => ((rate / TARGET_FPS) as usize).max(MIN_BUDGET),
        }
    }

    /// Vrai tant que la latence de la liaison n'a pas été mesurée. La
    /// capture laisse alors la liaison se vider puis envoie une image vide,
    /// dont l'aller-retour ne doit rien au débit.
    pub fn needs_probe(&self) -> bool {
        self.base.get().is_none()
    }

    /// Vrai si rien n'est en route.
    pub fn idle(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// Oublie les images dont l'accusé de réception n'arrivera plus (le
    /// visualiseur en laisse tomber quand son propre envoi sature) : sans
    /// cela, une seule perte suffirait à bloquer l'envoi pour de bon.
    pub fn forget_stale(&mut self, now: Instant) {
        while self.in_flight.front().is_some_and(|sent| now.saturating_duration_since(sent.at) > STALE) {
            if let Some(sent) = self.in_flight.pop_front() {
                self.bytes_in_flight -= sent.bytes;
            }
        }
    }

    /// Aller-retour d'une petite image (latence de la liaison), s'il est connu.
    pub fn base_rtt(&self) -> Option<Duration> {
        self.base.get()
    }

    /// Débit estimé de la liaison, en octets par seconde.
    pub fn rate(&self) -> Option<f64> {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Liaison simulée : `latency` dans chaque sens, `rate` octets par seconde,
    /// les images arrivent l'une après l'autre.
    struct Link {
        latency: Duration,
        rate: f64,
        free_at: Instant,
        /// Accusés en route : (arrivée chez l'hôte, numéro).
        acks: VecDeque<(Instant, u32)>,
    }

    impl Link {
        fn new(latency_ms: u64, rate: f64, now: Instant) -> Self {
            Self { latency: Duration::from_millis(latency_ms), rate, free_at: now, acks: VecDeque::new() }
        }

        fn send(&mut self, seq: u32, bytes: usize, now: Instant) {
            let start = self.free_at.max(now + self.latency);
            self.free_at = start + Duration::from_secs_f64(bytes as f64 / self.rate);
            self.acks.push_back((self.free_at + self.latency, seq));
        }

        fn deliver(&mut self, pacer: &mut Pacer, now: Instant) {
            while self.acks.front().is_some_and(|(at, _)| *at <= now) {
                let (at, seq) = self.acks.pop_front().unwrap();
                pacer.on_ack(seq, at);
            }
        }
    }

    /// Hôte simulé pendant `seconds` : une image toutes les 33 ms quand le
    /// régulateur le permet, de poids `size(budget)`. Renvoie le nombre
    /// d'images envoyées et le pire retard d'un accusé sur la latence pure.
    fn run(link: &mut Link, pacer: &mut Pacer, start: Instant, seconds: u64, size: impl Fn(usize) -> usize) -> (u32, Duration) {
        let step = Duration::from_millis(1);
        let mut now = start;
        let mut next_frame = start;
        let mut seq = 0u32;
        let mut worst = Duration::ZERO;
        let mut sent_at = std::collections::HashMap::new();
        while now < start + Duration::from_secs(seconds) {
            for (at, acked) in link.acks.iter().filter(|(at, _)| *at <= now) {
                let delay: Duration = at.duration_since(sent_at[acked]);
                worst = worst.max(delay.saturating_sub(link.latency * 2));
            }
            link.deliver(pacer, now);
            if now >= next_frame && pacer.can_send() {
                seq += 1;
                let bytes = size(pacer.frame_budget());
                pacer.on_sent(seq, bytes, now);
                link.send(seq, bytes, now);
                sent_at.insert(seq, now);
                next_frame = now + Duration::from_millis(33);
            }
            now += step;
        }
        (seq, worst)
    }

    #[test]
    fn small_frames_keep_full_rate_over_a_slow_round_trip() {
        // 200 ms d'aller-retour, liaison large, petites images (frappe, survol).
        let start = Instant::now();
        let mut link = Link::new(100, 5_000_000.0, start);
        let mut pacer = Pacer::new();
        let (frames, _) = run(&mut link, &mut pacer, start, 4, |_| 3_000);
        // Avec deux images en route au plus, on plafonnerait à 10 par seconde.
        assert!(frames > 4 * 27, "{frames} images en 4 s");
        let base = pacer.base_rtt().unwrap();
        assert!((195..=215).contains(&base.as_millis()), "délai de base {base:?}");
    }

    #[test]
    fn big_frames_do_not_pile_up_on_a_narrow_link() {
        // 500 Ko/s (4 Mbit/s), 80 ms d'aller-retour, tout l'écran bouge :
        // l'encodeur suit le poids demandé, sans descendre sous 12 Ko.
        let start = Instant::now();
        let mut link = Link::new(40, 500_000.0, start);
        let mut pacer = Pacer::new();
        // Une petite image d'abord, comme la sonde de latence.
        pacer.on_sent(0, 100, start);
        pacer.on_ack(0, start + Duration::from_millis(80));
        let (frames, worst) = run(&mut link, &mut pacer, start + Duration::from_millis(80), 6, |budget| budget.max(12_000));
        let rate = pacer.rate().unwrap();
        assert!((350_000.0..=650_000.0).contains(&rate), "débit estimé {rate}");
        assert!(frames >= 6 * 15, "{frames} images en 6 s");
        // La première image part à l'aveugle ; ensuite la file reste courte.
        assert!(worst < Duration::from_millis(450), "pire retard {worst:?}");
    }

    #[test]
    fn unknown_latency_gives_a_cautious_estimate() {
        let start = Instant::now();
        let mut pacer = Pacer::new();
        assert!(pacer.needs_probe());
        // Première image, grosse, sur une liaison lente à répondre mais large.
        pacer.on_sent(1, 64_000, start);
        pacer.on_ack(1, start + Duration::from_millis(200));
        // Tout l'aller-retour est compté comme du transport.
        assert!(pacer.rate().unwrap() <= 64_000.0 / 0.2 + 1.0);
        // L'image vide mesure la latence, et l'estimation est corrigée :
        // 64 Ko en 200 − 190 ms.
        assert!(pacer.needs_probe() && pacer.idle());
        pacer.on_sent(2, 12, start + Duration::from_millis(200));
        pacer.on_ack(2, start + Duration::from_millis(390));
        assert!(!pacer.needs_probe());
        assert_eq!(pacer.base_rtt(), Some(Duration::from_millis(190)));
        assert!(pacer.rate().unwrap() > 5_000_000.0);
    }

    #[test]
    fn the_window_holds_big_frames_but_not_small_ones() {
        let start = Instant::now();
        let mut pacer = Pacer::new();
        assert!(pacer.can_send());
        // Débit inconnu : une grosse image à la fois.
        pacer.on_sent(1, 100_000, start);
        assert!(!pacer.can_send());
        pacer.on_ack(1, start + Duration::from_millis(100));
        // Petites images : plusieurs en route, jusqu'à la limite.
        for seq in 2..2 + MAX_IN_FLIGHT as u32 {
            assert!(pacer.can_send());
            pacer.on_sent(seq, 2_000, start + Duration::from_millis(100));
        }
        assert!(!pacer.can_send());
        // Un accusé vaut pour toutes les images précédentes.
        pacer.on_ack(5, start + Duration::from_millis(200));
        assert!(pacer.can_send());
        assert_eq!(pacer.in_flight.len(), MAX_IN_FLIGHT - 4);
    }

    #[test]
    fn the_base_delay_survives_a_long_burst_of_big_frames() {
        let start = Instant::now();
        let mut pacer = Pacer::new();
        pacer.on_sent(1, 100, start);
        pacer.on_ack(1, start + Duration::from_millis(90));
        // Une minute de grosses images : aucune mesure de latence possible.
        let later = start + Duration::from_secs(60);
        pacer.on_sent(2, 50_000, later);
        pacer.on_ack(2, later + Duration::from_millis(150));
        assert_eq!(pacer.base_rtt(), Some(Duration::from_millis(90)));
        // Une petite image plus lente : la mesure suit, sans oublier d'un coup.
        pacer.on_sent(3, 100, later + Duration::from_secs(1));
        pacer.on_ack(3, later + Duration::from_millis(1120));
        assert_eq!(pacer.base_rtt(), Some(Duration::from_millis(90)));
        let much_later = later + Duration::from_secs(40);
        pacer.on_sent(4, 100, much_later);
        pacer.on_ack(4, much_later + Duration::from_millis(130));
        assert_eq!(pacer.base_rtt(), Some(Duration::from_millis(120)));
    }

    #[test]
    fn a_lost_ack_does_not_block_forever() {
        let start = Instant::now();
        let mut pacer = Pacer::new();
        pacer.on_sent(1, 100_000, start);
        pacer.on_sent(2, 100_000, start);
        pacer.forget_stale(start + Duration::from_secs(5));
        assert!(!pacer.can_send());
        pacer.forget_stale(start + STALE + Duration::from_millis(1));
        assert!(pacer.can_send());
        assert_eq!(pacer.bytes_in_flight, 0);
    }
}
