//! Capture de l'écran et encodage en tuiles JPEG.
//!
//! L'écran est découpé en tuiles de 128×128 px ; seules les tuiles modifiées
//! depuis l'image précédente sont encodées et envoyées, regroupées en bandes
//! quand elles se touchent. Le visualiseur accuse réception de chaque image :
//! le [`Pacer`] en déduit la latence et le débit de la liaison, donc combien
//! d'images peuvent être en route et combien chacune peut peser.
//!
//! Quand la liaison ne suit pas, les zones qui bougent partent en qualité
//! réduite (voir [`ladder`]) : l'image reste fluide. Dès qu'une zone ne bouge
//! plus, elle est renvoyée en pleine qualité.

use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc::{Receiver, RecvTimeoutError, TryRecvError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use distant_proto::session::{
    encode_frame, Control, MonitorInfo, Tile, QUALITY_BALANCED, QUALITY_FAST, QUALITY_HIGH,
};
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use rayon::prelude::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xcap::Monitor;

use super::pacing::Pacer;

const TILE: u32 = 128;
/// Cadence maximale : environ 30 images par seconde.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Intervalle de vérification quand l'écran ne change pas.
const IDLE_INTERVAL: Duration = Duration::from_millis(50);
/// Le même, juste après un clic, une frappe ou un déplacement de souris :
/// c'est là que l'écran va changer, et que l'attente se remarque.
const ACTIVE_INTERVAL: Duration = Duration::from_millis(16);
const ACTIVE_FOR: Duration = Duration::from_millis(400);
/// Temps laissé au poste pour réagir à un événement avant de capturer.
const INPUT_SETTLE: Duration = Duration::from_millis(8);
/// Captures sans changement avant qu'une tuile envoyée en qualité réduite
/// soit renvoyée en pleine qualité.
const CALM_CAPTURES: u8 = 3;
/// Poids approximatif des en-têtes d'un JPEG (tables de quantification et de Huffman).
const JPEG_OVERHEAD: usize = 620;

/// Position et taille de l'écran partagé, dans le repère utilisé pour
/// l'injection de la souris (pixels physiques sous Windows et Linux,
/// points sous macOS : c'est ce que renvoie `xcap`).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Réglages partagés entre la session (réseau), la capture et l'injection.
pub struct CaptureSettings {
    monitor: AtomicUsize,
    quality: AtomicU8,
    dirty: AtomicBool,
    /// Le visualiseur a demandé les mesures de l'hôte.
    stats: AtomicBool,
    /// Le visualiseur sait agrandir une tuile envoyée en demi-résolution.
    scaled_tiles: AtomicBool,
    geometry: Mutex<Geometry>,
}

impl CaptureSettings {
    pub fn new() -> Self {
        let monitors = Monitor::all().unwrap_or_default();
        let primary = monitors.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
        let geometry = monitors
            .get(primary)
            .map(geometry_of)
            .unwrap_or(Geometry { x: 0, y: 0, width: 1, height: 1 });
        Self {
            monitor: AtomicUsize::new(primary),
            quality: AtomicU8::new(QUALITY_BALANCED),
            dirty: AtomicBool::new(true),
            stats: AtomicBool::new(false),
            scaled_tiles: AtomicBool::new(false),
            geometry: Mutex::new(geometry),
        }
    }

    pub fn set_monitor(&self, index: usize) {
        self.monitor.store(index, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_quality(&self, quality: u8) {
        if self.quality.swap(quality, Ordering::Relaxed) != quality {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn set_stats(&self, enabled: bool) {
        self.stats.store(enabled, Ordering::Relaxed);
    }

    pub fn set_scaled_tiles(&self, supported: bool) {
        self.scaled_tiles.store(supported, Ordering::Relaxed);
    }

    pub fn geometry(&self) -> Geometry {
        *self.geometry.lock().unwrap()
    }
}

fn geometry_of(m: &Monitor) -> Geometry {
    Geometry {
        x: m.x().unwrap_or(0),
        y: m.y().unwrap_or(0),
        width: m.width().unwrap_or(1).max(1),
        height: m.height().unwrap_or(1).max(1),
    }
}

pub fn list_monitors() -> Vec<MonitorInfo> {
    Monitor::all()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            name: m
                .friendly_name()
                .ok()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("Écran {}", i + 1)),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            primary: m.is_primary().unwrap_or(i == 0),
        })
        .collect()
}

/// Ce que la session signale à la capture.
pub enum Signal {
    /// Le visualiseur a affiché l'image `seq` ; `at` est l'heure d'arrivée de
    /// son accusé de réception.
    Ack { seq: u32, at: Instant },
    /// Un événement clavier ou souris vient d'être reçu : l'écran va changer.
    Input,
}

/// Démarre le thread de capture. Il s'arrête avec la session (`cancel`).
pub fn spawn(
    settings: Arc<CaptureSettings>,
    out: mpsc::Sender<Vec<u8>>,
    signals: Receiver<Signal>,
    cancel: CancellationToken,
) {
    let result = std::thread::Builder::new().name("distant-capture".into()).spawn(move || {
        if let Err(e) = capture_loop(&settings, &out, &signals, &cancel) {
            tracing::error!("capture arrêtée : {e:#}");
            cancel.cancel();
        }
    });
    if let Err(e) = result {
        tracing::error!("impossible de lancer la capture : {e}");
    }
}

/// Image RGBA.
struct Frame {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
}

struct Preset {
    jpeg_quality: u8,
    /// Couleur en pleine résolution (4:4:4) plutôt qu'en demi-résolution (4:2:0).
    full_chroma: bool,
    /// Au-delà de cette largeur, l'image est réduite de moitié.
    downscale_above: u32,
}

fn preset(quality: u8) -> Preset {
    match quality {
        QUALITY_FAST => Preset { jpeg_quality: 50, full_chroma: false, downscale_above: 1600 },
        QUALITY_HIGH => Preset { jpeg_quality: 85, full_chroma: true, downscale_above: u32::MAX },
        _ => Preset { jpeg_quality: 70, full_chroma: false, downscale_above: 2560 },
    }
}

/// Un niveau d'encodage : du plus fidèle (niveau 0, celui du préréglage) au
/// plus léger.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    jpeg_quality: u8,
    full_chroma: bool,
    /// Réduction de la zone avant encodage (1 : aucune ; 2 : moitié ;
    /// 4 : quart). Le visualiseur l'agrandit.
    scale: u8,
}

impl Level {
    /// Poids relatif d'une zone encodée à ce niveau, mesuré sur des écrans de
    /// bureau (texte, fenêtres, quelques images). C'est un ordre de grandeur :
    /// [`Cost`] corrige le reste d'après les images réellement encodées.
    fn weight(&self) -> f32 {
        let q = self.jpeg_quality as f32;
        // 1 à la qualité 70 ; environ 0,5 à la qualité 28 et 1,45 à la qualité 85.
        let quality = match q {
            q if q >= 70.0 => 1.0 + (q - 70.0) * 0.03,
            q if q >= 52.0 => 0.76 + (q - 52.0) * 0.0133,
            q => 0.76 - (52.0 - q) * 0.01,
        };
        let chroma = if self.full_chroma { 1.25 } else { 1.0 };
        // Une zone réduite garde plus de détail par pixel : moitié moins de
        // pixels par côté ne divise le poids que par trois.
        let scale = match self.scale {
            1 => 1.0,
            2 => 0.34,
            _ => 0.13,
        };
        quality * chroma * scale
    }
}

/// Niveaux d'encodage disponibles pour un préréglage, du plus fidèle au plus
/// léger. `scaled` : le visualiseur accepte les zones en résolution réduite.
fn ladder(preset: &Preset, scaled: bool) -> Vec<Level> {
    let q = preset.jpeg_quality;
    let reduced = |jpeg_quality, scale| Level { jpeg_quality, full_chroma: false, scale };
    let mut levels = vec![
        Level { jpeg_quality: q, full_chroma: preset.full_chroma, scale: 1 },
        reduced((q as u16 * 3 / 4) as u8, 1),
        reduced((q / 2).max(28), 1),
    ];
    if scaled {
        levels.extend([reduced(55, 2), reduced(30, 2), reduced(45, 4)]);
    } else {
        levels.push(reduced(18, 1));
    }
    levels
}

/// Prévision du poids d'une image avant de l'encoder : octets par pixel à
/// chaque niveau, appris des images réellement encodées.
struct Cost {
    bytes_per_pixel: Vec<f32>,
    /// Niveaux déjà mesurés. Les autres sont déduits du dernier niveau
    /// mesuré, à proportion de leur poids relatif (voir [`Level::weight`]).
    measured: Vec<bool>,
    weights: Vec<f32>,
    /// Pixels en dessous desquels une image encodée n'apprend rien : les
    /// en-têtes dominent.
    floor: u64,
}

impl Cost {
    fn new(ladder: &[Level], top: f32, floor: u64) -> Self {
        let weights: Vec<f32> = ladder.iter().map(Level::weight).collect();
        Self {
            bytes_per_pixel: weights.iter().map(|weight| top * weight / weights[0]).collect(),
            measured: vec![false; ladder.len()],
            weights,
            floor,
        }
    }

    /// Prévision pour ce qui bouge : de grandes zones, mêlant texte et aplats.
    fn moving(ladder: &[Level]) -> Self {
        // Un écran de bureau en qualité 70 pèse environ 0,075 octet par pixel.
        Self::new(ladder, 0.075 * ladder[0].weight(), (4 * TILE * TILE) as u64)
    }

    /// Prévision pour l'affinage : quelques tuiles à la fois, souvent du
    /// texte, bien plus dense que la moyenne de l'écran.
    fn still(ladder: &[Level]) -> Self {
        Self::new(ladder, 0.3 * ladder[0].weight(), (TILE * TILE / 2) as u64)
    }

    fn predict(&self, level: usize, pixels: u64, rects: usize) -> usize {
        (pixels as f32 * self.bytes_per_pixel[level]) as usize + rects * JPEG_OVERHEAD
    }

    /// Niveau le plus fidèle dont le poids prévu tient dans `budget`.
    fn pick(&self, pixels: u64, rects: usize, budget: usize) -> usize {
        let levels = self.bytes_per_pixel.len();
        (0..levels).find(|&level| self.predict(level, pixels, rects) <= budget).unwrap_or(levels - 1)
    }

    /// Corrige la prévision : `bytes` octets obtenus pour `pixels` pixels
    /// encodés en `rects` zones au niveau `level`.
    fn observe(&mut self, level: usize, bytes: usize, rects: usize, pixels: u64) {
        if pixels < self.floor {
            return;
        }
        let payload = bytes.saturating_sub(rects * JPEG_OVERHEAD) as f32;
        let observed = (payload / pixels as f32).clamp(0.001, 2.0);
        if self.measured[level] {
            self.bytes_per_pixel[level] += (observed - self.bytes_per_pixel[level]) * 0.3;
        } else {
            self.bytes_per_pixel[level] = observed;
            self.measured[level] = true;
        }
        let per_weight = self.bytes_per_pixel[level] / self.weights[level];
        for other in 0..self.weights.len() {
            if !self.measured[other] {
                self.bytes_per_pixel[other] = per_weight * self.weights[other];
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct TileState {
    /// Niveau auquel la tuile a été envoyée la dernière fois.
    level: u8,
    /// Captures consécutives sans changement.
    calm: u8,
}

/// État de chaque tuile chez le visualiseur.
struct Grid {
    width: u32,
    height: u32,
    cols: u32,
    tiles: Vec<TileState>,
}

impl Grid {
    fn new(width: u32, height: u32) -> Self {
        let cols = width.div_ceil(TILE);
        let rows = height.div_ceil(TILE);
        Self { width, height, cols, tiles: vec![TileState::default(); (cols * rows) as usize] }
    }

    fn matches(&self, frame: &Frame) -> bool {
        self.width == frame.width && self.height == frame.height
    }

    fn rect(&self, index: u32) -> Rect {
        let x = (index % self.cols) * TILE;
        let y = (index / self.cols) * TILE;
        Rect { x, y, w: TILE.min(self.width - x), h: TILE.min(self.height - y) }
    }

    /// Tuiles qui diffèrent entre `prev` et `cur`, dans l'ordre ; toutes si
    /// `prev` manque.
    fn changed(&self, prev: Option<&Frame>, cur: &Frame) -> Vec<u32> {
        let count = self.tiles.len() as u32;
        let Some(prev) = prev.filter(|p| p.width == cur.width && p.height == cur.height) else {
            return (0..count).collect();
        };
        (0..count).into_par_iter().filter(|&i| differs(prev, cur, &self.rect(i))).collect()
    }

    /// Enregistre une capture : les tuiles `changed` (triées) ont bougé, les
    /// autres sont restées calmes une fois de plus.
    fn note(&mut self, changed: &[u32]) {
        let mut changed = changed.iter().peekable();
        for (index, tile) in self.tiles.iter_mut().enumerate() {
            if changed.next_if(|&&c| c as usize == index).is_some() {
                tile.calm = 0;
            } else {
                tile.calm = tile.calm.saturating_add(1);
            }
        }
    }

    fn mark(&mut self, tiles: &[u32], level: usize) {
        for &index in tiles {
            self.tiles[index as usize].level = level as u8;
        }
    }

    /// Tuiles calmes envoyées en qualité réduite, à renvoyer en mieux, tant
    /// que leur poids prévu tient dans `spare` octets ; avec `at_least_one`,
    /// la première est prise même si elle dépasse. Renvoie des couples
    /// (niveau visé, tuiles).
    ///
    /// L'affinage se fait en deux temps : les zones les plus réduites (au
    /// quart) passent d'abord en demi-résolution, peu coûteuse, pour que le
    /// texte redevienne lisible partout ; la pleine qualité vient ensuite.
    fn refinable(&self, cost: &Cost, ladder: &[Level], spare: usize, at_least_one: bool) -> Vec<(usize, Vec<u32>)> {
        let half = ladder.iter().position(|level| level.scale == 2);
        let coarse = |tile: &TileState| half.is_some() && ladder[tile.level as usize].scale > 2;
        let mut planned = 0;
        let mut any = false;
        let mut passes = Vec::new();
        for (target, first) in [(half.unwrap_or(0), true), (0, false)] {
            let mut picked = Vec::new();
            for (index, tile) in self.tiles.iter().enumerate() {
                if tile.level == 0 || tile.calm < CALM_CAPTURES || coarse(tile) != first {
                    continue;
                }
                planned += cost.predict(target, self.rect(index as u32).area(), 1);
                if planned > spare && (any || !at_least_one) {
                    break;
                }
                any = true;
                picked.push(index as u32);
            }
            if !picked.is_empty() {
                passes.push((target, picked));
            }
            if planned > spare {
                break;
            }
        }
        passes
    }

    /// Regroupe des tuiles (triées) en bandes : celles qui se suivent sur une
    /// même rangée ne font qu'un rectangle, donc qu'un seul JPEG.
    fn runs(&self, tiles: &[u32]) -> Vec<Rect> {
        let mut rects: Vec<Rect> = Vec::new();
        let mut last = None;
        for &index in tiles {
            let rect = self.rect(index);
            match rects.last_mut() {
                Some(run) if last == Some(index.wrapping_sub(1)) && run.y == rect.y => run.w += rect.w,
                _ => rects.push(rect),
            }
            last = Some(index);
        }
        rects
    }
}

/// Une zone à encoder, à un niveau donné.
struct Job {
    rect: Rect,
    level: usize,
}

/// Où part le temps, cumulé sur une seconde puis envoyé au visualiseur.
#[derive(Default)]
struct Timings {
    /// Images envoyées.
    frames: u32,
    /// Captures faites, y compris celles où rien n'avait changé à l'écran.
    captures: u32,
    capture: Duration,
    encode: Duration,
    wait: Duration,
    /// Niveau le plus léger utilisé (0 : pleine qualité).
    level: usize,
}

impl Timings {
    /// Moyennes en millisecondes : par capture pour la capture, par image
    /// envoyée pour le reste.
    fn message(&self, pacer: &Pacer, levels: usize) -> Control {
        let average = |total: Duration, count: u32| (total.as_millis() / count.max(1) as u128).min(u16::MAX as u128) as u16;
        Control::Stats {
            frames: self.frames.min(u16::MAX as u32) as u16,
            capture: average(self.capture, self.captures),
            encode: average(self.encode, self.frames),
            wait: average(self.wait, self.frames),
            rtt: pacer.base_rtt().map(|rtt| rtt.as_millis().min(u16::MAX as u128) as u16),
            rate: pacer.rate().map(|rate| (rate / 1000.0).min(u32::MAX as f64) as u32),
            level: Some(self.level as u8),
            levels: Some(levels as u8),
        }
    }
}

fn capture_loop(
    settings: &CaptureSettings,
    out: &mpsc::Sender<Vec<u8>>,
    signals: &Receiver<Signal>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut seq: u32 = 0;
    let mut pacer = Pacer::new();
    let mut last_input: Option<Instant> = None;
    let mut monitors: Vec<Monitor> = Vec::new();
    let mut current: Option<usize> = None;
    let mut prev: Option<Frame> = None;
    let mut grid = Grid::new(0, 0);
    // Prévisions du poids des images (ce qui bouge, ce qui est affiné),
    // propres au préréglage en cours.
    let mut forecast: Option<(u8, Cost, Cost)> = None;
    let mut levels = 1;
    let mut failures: u32 = 0;
    let mut timings = Timings::default();
    let mut window = Instant::now();

    while !cancel.is_cancelled() {
        if window.elapsed() >= Duration::from_secs(1) {
            if settings.stats.load(Ordering::Relaxed) && timings.frames > 0 {
                // Jamais bloquant : une mesure perdue n'a aucune importance.
                let _ = out.try_send(timings.message(&pacer, levels).encode());
            }
            timings = Timings::default();
            window = Instant::now();
        }

        loop {
            match signals.try_recv() {
                Ok(signal) => apply(signal, &mut pacer, &mut last_input),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        pacer.forget_stale(Instant::now());
        // Latence de la liaison pas encore mesurée : une fois la première
        // image arrivée, une image vide part seule, et rien d'autre avant son
        // accusé de réception.
        let probing = pacer.needs_probe() && prev.is_some();
        if let (true, true, Some(frame)) = (probing, pacer.idle(), prev.as_ref()) {
            seq = seq.wrapping_add(1);
            let payload = encode_frame(seq, frame.width as u16, frame.height as u16, &[]);
            let bytes = payload.len();
            if out.blocking_send(payload).is_err() {
                break;
            }
            pacer.on_sent(seq, bytes, Instant::now());
        }
        if probing || !pacer.can_send() {
            let waiting = Instant::now();
            let signal = signals.recv_timeout(Duration::from_millis(250));
            timings.wait += waiting.elapsed();
            match signal {
                Ok(signal) => apply(signal, &mut pacer, &mut last_input),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            continue;
        }

        let started = Instant::now();
        if settings.dirty.swap(false, Ordering::Relaxed) || current.is_none() {
            monitors = Monitor::all().context("liste des écrans")?;
            if monitors.is_empty() {
                bail!("aucun écran détecté");
            }
            let index = settings.monitor.load(Ordering::Relaxed).min(monitors.len() - 1);
            *settings.geometry.lock().unwrap() = geometry_of(&monitors[index]);
            current = Some(index);
            prev = None; // image complète au prochain envoi
        }
        let Some(monitor) = current.and_then(|i| monitors.get(i)) else { continue };

        let capturing = Instant::now();
        let image = match monitor.capture_image() {
            Ok(image) => {
                failures = 0;
                timings.capture += capturing.elapsed();
                timings.captures += 1;
                image
            }
            Err(e) => {
                // Écran verrouillé, invite UAC, changement de résolution… on réessaie.
                failures += 1;
                if failures == 1 || failures.is_multiple_of(40) {
                    tracing::warn!("capture impossible : {e}");
                }
                if failures.is_multiple_of(8) {
                    current = None;
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        };

        let encoding = Instant::now();
        let quality = settings.quality.load(Ordering::Relaxed);
        let preset = preset(quality);
        let ladder = ladder(&preset, settings.scaled_tiles.load(Ordering::Relaxed));
        levels = ladder.len();
        if forecast.as_ref().is_none_or(|(q, ..)| *q != quality) {
            forecast = Some((quality, Cost::moving(&ladder), Cost::still(&ladder)));
        }
        let Some((_, moving, still)) = forecast.as_mut() else { continue };
        let frame = prepare(image, &preset);
        if !grid.matches(&frame) {
            grid = Grid::new(frame.width, frame.height);
            prev = None;
        }

        // Ce qui a bougé part au niveau que la liaison peut écouler…
        let budget = pacer.frame_budget();
        let changed = grid.changed(prev.as_ref(), &frame);
        grid.note(&changed);
        let mut jobs: Vec<Job> = Vec::new();
        let mut planned = 0;
        if !changed.is_empty() {
            let rects = grid.runs(&changed);
            let pixels = rects.iter().map(Rect::area).sum();
            let level = moving.pick(pixels, rects.len(), budget);
            planned = moving.predict(level, pixels, rects.len());
            grid.mark(&changed, level);
            timings.level = timings.level.max(level);
            jobs.extend(rects.into_iter().map(|rect| Job { rect, level }));
        }
        // … et ce qui s'est calmé est affiné avec la place qui reste : toute
        // la liaison si rien ne bouge (le régulateur borne la file d'attente).
        let moved = jobs.len();
        let spare = budget.saturating_sub(planned);
        for (level, tiles) in grid.refinable(still, &ladder, spare, jobs.is_empty()) {
            grid.mark(&tiles, level);
            jobs.extend(grid.runs(&tiles).into_iter().map(|rect| Job { rect, level }));
        }

        if jobs.is_empty() {
            // Écran figé et net.
            prev = Some(frame);
            let active = last_input.is_some_and(|at| at.elapsed() < ACTIVE_FOR);
            let interval = if active { ACTIVE_INTERVAL } else { IDLE_INTERVAL };
            match signals.recv_timeout(interval.saturating_sub(started.elapsed())) {
                Ok(signal) => {
                    let input = matches!(signal, Signal::Input);
                    apply(signal, &mut pacer, &mut last_input);
                    if input {
                        std::thread::sleep(INPUT_SETTLE);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            continue;
        }

        let tiles = jobs
            .par_iter()
            .map(|job| encode_rect(&frame, &job.rect, &ladder[job.level]))
            .collect::<anyhow::Result<Vec<_>>>()?;
        // Les prévisions se corrigent d'après ce qui vient d'être encodé,
        // niveau par niveau.
        for (cost, jobs, tiles) in [(moving, &jobs[..moved], &tiles[..moved]), (still, &jobs[moved..], &tiles[moved..])] {
            for level in 0..ladder.len() {
                let encoded = jobs.iter().zip(tiles).filter(|(job, _)| job.level == level);
                let (bytes, rects, pixels) = encoded.fold((0, 0, 0), |(bytes, rects, pixels), (job, tile)| {
                    (bytes + tile.jpeg.len(), rects + 1, pixels + job.rect.area())
                });
                cost.observe(level, bytes, rects, pixels);
            }
        }

        seq = seq.wrapping_add(1);
        let payload = encode_frame(seq, frame.width as u16, frame.height as u16, &tiles);
        let bytes = payload.len();
        timings.encode += encoding.elapsed();
        timings.frames += 1;
        if out.blocking_send(payload).is_err() {
            break; // session terminée
        }
        pacer.on_sent(seq, bytes, Instant::now());
        prev = Some(frame);
        sleep_remaining(started, MIN_FRAME_INTERVAL);
    }
    Ok(())
}

fn apply(signal: Signal, pacer: &mut Pacer, last_input: &mut Option<Instant>) {
    match signal {
        Signal::Ack { seq, at } => pacer.on_ack(seq, at),
        Signal::Input => *last_input = Some(Instant::now()),
    }
}

fn sleep_remaining(started: Instant, interval: Duration) {
    if let Some(rest) = interval.checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
}

fn prepare(image: xcap::image::RgbaImage, preset: &Preset) -> Frame {
    let (width, height) = image.dimensions();
    // Les dimensions voyagent sur 16 bits : on réduit les écrans géants.
    let too_big = width > u16::MAX as u32 || height > u16::MAX as u32;
    let data = image.into_raw();
    if (width > preset.downscale_above || too_big) && width >= 2 && height >= 2 {
        downscale_half(&data, width, height)
    } else {
        Frame { width, height, data }
    }
}

/// Réduction de moitié (moyenne de 2×2 pixels).
fn downscale_half(src: &[u8], width: u32, height: u32) -> Frame {
    let (nw, nh) = (width / 2, height / 2);
    let src_stride = width as usize * 4;
    let dst_stride = nw as usize * 4;
    let mut data = vec![0u8; dst_stride * nh as usize];
    data.par_chunks_mut(dst_stride).enumerate().for_each(|(y, dst)| {
        let r0 = &src[2 * y * src_stride..][..src_stride];
        let r1 = &src[(2 * y + 1) * src_stride..][..src_stride];
        for x in 0..nw as usize {
            let i = x * 8;
            for c in 0..4 {
                let sum = r0[i + c] as u16 + r0[i + 4 + c] as u16 + r1[i + c] as u16 + r1[i + 4 + c] as u16;
                dst[x * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    });
    Frame { width: nw, height: nh, data }
}

fn differs(a: &Frame, b: &Frame, r: &Rect) -> bool {
    let stride = a.width as usize * 4;
    let len = r.w as usize * 4;
    (r.y..r.y + r.h).any(|row| {
        let off = row as usize * stride + r.x as usize * 4;
        a.data[off..off + len] != b.data[off..off + len]
    })
}

/// Pixels de la zone `r`, sans copie quand elle fait toute la largeur.
fn region<'a>(frame: &'a Frame, r: &Rect) -> Cow<'a, [u8]> {
    let stride = frame.width as usize * 4;
    let row_len = r.w as usize * 4;
    if r.x == 0 && r.w == frame.width {
        let start = r.y as usize * stride;
        return Cow::Borrowed(&frame.data[start..start + r.h as usize * stride]);
    }
    let mut buf = Vec::with_capacity(row_len * r.h as usize);
    for row in r.y..r.y + r.h {
        let off = row as usize * stride + r.x as usize * 4;
        buf.extend_from_slice(&frame.data[off..off + row_len]);
    }
    Cow::Owned(buf)
}

/// Pixels de la zone `r` réduite d'un facteur `scale` (moyenne de blocs de
/// `scale`×`scale` pixels ; au bord d'une zone qui n'est pas un multiple, la
/// dernière colonne ou rangée est répétée). Renvoie aussi la largeur et la
/// hauteur obtenues.
fn reduced_region(frame: &Frame, r: &Rect, scale: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = (r.w.div_ceil(scale), r.h.div_ceil(scale));
    let stride = frame.width as usize * 4;
    let half = scale * scale / 2;
    let mut data = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            for dy in 0..scale {
                let row = (r.y + (y * scale + dy).min(r.h - 1)) as usize * stride;
                for dx in 0..scale {
                    let at = row + (r.x + (x * scale + dx).min(r.w - 1)) as usize * 4;
                    for (total, value) in sum.iter_mut().zip(&frame.data[at..at + 4]) {
                        *total += *value as u32;
                    }
                }
            }
            data.extend(sum.map(|total| ((total + half) / (scale * scale)) as u8));
        }
    }
    (data, nw, nh)
}

/// Encode la zone `r` en JPEG. La tuile garde la position et la taille de la
/// zone à l'écran, même quand le JPEG est en résolution réduite.
fn encode_rect(frame: &Frame, r: &Rect, level: &Level) -> anyhow::Result<Tile> {
    let (pixels, w, h) = if level.scale > 1 {
        let (data, w, h) = reduced_region(frame, r, level.scale as u32);
        (Cow::Owned(data), w, h)
    } else {
        (region(frame, r), r.w, r.h)
    };
    let mut jpeg = Vec::with_capacity(pixels.len() / 8);
    let mut encoder = Encoder::new(&mut jpeg, level.jpeg_quality);
    encoder.set_sampling_factor(if level.full_chroma { SamplingFactor::R_4_4_4 } else { SamplingFactor::R_4_2_0 });
    encoder.encode(&pixels, w as u16, h as u16, ColorType::Rgba)?;
    Ok(Tile { x: r.x as u16, y: r.y as u16, w: r.w as u16, h: r.h as u16, jpeg })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, fill: u8) -> Frame {
        Frame { width, height, data: vec![fill; (width * height * 4) as usize] }
    }

    fn changed_rects(prev: Option<&Frame>, cur: &Frame) -> Vec<Rect> {
        let grid = Grid::new(cur.width, cur.height);
        grid.runs(&grid.changed(prev, cur))
    }

    #[test]
    fn first_frame_is_full() {
        let f = frame(300, 200, 0);
        let rects = changed_rects(None, &f);
        let area: u64 = rects.iter().map(Rect::area).sum();
        assert_eq!(area, 300 * 200);
        // Une bande par rangée de tuiles.
        assert_eq!(rects, vec![Rect { x: 0, y: 0, w: 300, h: 128 }, Rect { x: 0, y: 128, w: 300, h: 72 }]);
    }

    #[test]
    fn identical_frames_send_nothing() {
        let a = frame(300, 200, 7);
        let b = frame(300, 200, 7);
        assert!(changed_rects(Some(&a), &b).is_empty());
    }

    #[test]
    fn small_change_sends_one_tile() {
        let a = frame(1000, 600, 0);
        let mut b = frame(1000, 600, 0);
        // Pixel (130, 5) -> tuile (128, 0).
        let off = ((5 * 1000 + 130) * 4) as usize;
        b.data[off] = 255;
        let rects = changed_rects(Some(&a), &b);
        assert_eq!(rects, vec![Rect { x: 128, y: 0, w: 128, h: 128 }]);
    }

    #[test]
    fn edge_tiles_are_clipped() {
        let a = frame(130, 130, 0);
        let mut b = frame(130, 130, 0);
        let last = b.data.len() - 1;
        b.data[last] = 1;
        let rects = changed_rects(Some(&a), &b);
        assert_eq!(rects, vec![Rect { x: 128, y: 128, w: 2, h: 2 }]);
    }

    #[test]
    fn neighbouring_tiles_share_a_jpeg() {
        // 5 colonnes × 2 rangées.
        let grid = Grid::new(600, 200);
        // Deux tuiles voisines, une isolée, puis la fin d'une rangée et le
        // début de la suivante, qui ne se touchent pas à l'écran.
        let rects = grid.runs(&[0, 1, 3, 4, 5]);
        assert_eq!(
            rects,
            vec![
                Rect { x: 0, y: 0, w: 256, h: 128 },
                Rect { x: 384, y: 0, w: 216, h: 128 },
                Rect { x: 0, y: 128, w: 128, h: 72 },
            ]
        );
    }

    #[test]
    fn calm_tiles_are_refined_within_the_budget() {
        let mut grid = Grid::new(512, 128);
        let levels = ladder(&preset(QUALITY_BALANCED), false);
        let cost = Cost::still(&levels);
        // Tout part en qualité réduite ; la tuile 2 bouge encore.
        grid.note(&[0, 1, 2, 3]);
        grid.mark(&[0, 1, 2, 3], 3);
        assert!(grid.refinable(&cost, &levels, usize::MAX, true).is_empty(), "rien n'est encore calme");
        for _ in 0..CALM_CAPTURES {
            grid.note(&[2]);
        }
        assert_eq!(grid.refinable(&cost, &levels, usize::MAX, false), vec![(0, vec![0, 1, 3])]);
        // Budget serré : une tuile à la fois, mais au moins une si on y tient.
        let one = cost.predict(0, (TILE * TILE) as u64, 1);
        assert_eq!(grid.refinable(&cost, &levels, one, false), vec![(0, vec![0])]);
        assert!(grid.refinable(&cost, &levels, 0, false).is_empty());
        assert_eq!(grid.refinable(&cost, &levels, 0, true), vec![(0, vec![0])]);
        // Une fois renvoyées en pleine qualité, il n'y a plus rien à affiner.
        grid.mark(&[0, 1, 3], 0);
        assert!(grid.refinable(&cost, &levels, usize::MAX, true).is_empty());
    }

    #[test]
    fn the_coarsest_tiles_are_refined_in_two_steps() {
        let mut grid = Grid::new(512, 128);
        let levels = ladder(&preset(QUALITY_BALANCED), true);
        let half = levels.iter().position(|level| level.scale == 2).unwrap();
        let quarter = levels.iter().position(|level| level.scale == 4).unwrap();
        let cost = Cost::still(&levels);
        // Tuiles 0 et 2 au quart, tuile 1 en demi-résolution, tuile 3 nette.
        grid.mark(&[0, 2], quarter);
        grid.mark(&[1], half);
        for _ in 0..CALM_CAPTURES {
            grid.note(&[]);
        }
        // D'abord le quart vers la demi-résolution, puis le reste au net.
        let passes = grid.refinable(&cost, &levels, usize::MAX, false);
        assert_eq!(passes, vec![(half, vec![0, 2]), (0, vec![1])]);
        // Budget serré : la première étape passe avant tout le reste.
        let one = cost.predict(half, (TILE * TILE) as u64, 1);
        assert_eq!(grid.refinable(&cost, &levels, one, false), vec![(half, vec![0])]);
        // Étape suivante : les deux tuiles restées en demi-résolution passent au net.
        for (level, tiles) in passes {
            grid.mark(&tiles, level);
        }
        assert_eq!(grid.refinable(&cost, &levels, usize::MAX, false), vec![(0, vec![0, 2])]);
    }

    #[test]
    fn the_level_follows_the_budget() {
        let preset = preset(QUALITY_BALANCED);
        let levels = ladder(&preset, true);
        assert_eq!(levels[0], BALANCED_TOP);
        assert!(levels.windows(2).all(|pair| pair[1].weight() < pair[0].weight()), "du plus lourd au plus léger");
        assert!(ladder(&preset, false).iter().all(|level| level.scale == 1), "sans l'accord du visualiseur, pas de réduction");

        let cost = Cost::moving(&levels);
        let screen = 1920 * 1080;
        let full = cost.predict(0, screen, 9);
        assert_eq!(cost.pick(screen, 9, full), 0);
        assert_eq!(cost.pick(screen, 9, full - 1), 1);
        assert_eq!(cost.pick(screen, 9, 1), levels.len() - 1, "au pire, le niveau le plus léger");
        // Une petite zone tient toujours en pleine qualité.
        assert_eq!(cost.pick((TILE * TILE) as u64, 1, 10_000), 0);
    }

    #[test]
    fn the_cost_learns_from_encoded_frames() {
        let levels = ladder(&preset(QUALITY_BALANCED), true);
        let mut cost = Cost::moving(&levels);
        let pixels = 1_000_000;
        let before: Vec<usize> = (0..levels.len()).map(|level| cost.predict(level, pixels, 0)).collect();
        // Des images deux fois plus lourdes que prévu au niveau 2 : tous les
        // niveaux, jamais mesurés, suivent à proportion.
        cost.observe(2, before[2] * 2, 0, pixels);
        for (level, &before) in before.iter().enumerate() {
            let ratio = cost.predict(level, pixels, 0) as f32 / before as f32;
            assert!((ratio - 2.0).abs() < 0.05, "niveau {level} : ×{ratio}");
        }
        // Le niveau 0 se révèle plus lourd encore : lui seul est corrigé.
        cost.observe(0, before[0] * 6, 0, pixels);
        assert!((cost.predict(0, pixels, 0) as f32 / before[0] as f32 - 6.0).abs() < 0.05);
        assert!((cost.predict(2, pixels, 0) as f32 / before[2] as f32 - 2.0).abs() < 0.05);
        // Une zone minuscule n'apprend rien.
        cost.observe(0, 50_000, 1, 100);
        assert!((cost.predict(0, pixels, 0) as f32 / before[0] as f32 - 6.0).abs() < 0.05);
    }

    #[test]
    fn downscale_averages() {
        let mut f = frame(4, 2, 0);
        f.data[0] = 200; // pixel (0,0), rouge
        f.data[4] = 100; // pixel (1,0), rouge
        let small = downscale_half(&f.data, 4, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.data[0], 75); // (200 + 100 + 0 + 0) / 4
    }

    #[test]
    fn reduced_regions_handle_odd_sizes() {
        let mut f = frame(5, 3, 0);
        // Dernière colonne (x = 4) à 80 partout.
        for y in 0..3 {
            f.data[(y * 5 + 4) * 4] = 80;
        }
        let (data, w, h) = reduced_region(&f, &Rect { x: 2, y: 0, w: 3, h: 3 }, 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(data.len(), 2 * 2 * 4);
        assert_eq!(data[0], 0); // colonnes 2 et 3
        assert_eq!(data[4], 80); // colonne 4, répétée
        // Au quart : un seul pixel, moyenne de 4×4 dont 8 valent 80 (la
        // colonne 4, répétée deux fois, sur quatre rangées).
        let (data, w, h) = reduced_region(&f, &Rect { x: 2, y: 0, w: 3, h: 3 }, 4);
        assert_eq!((w, h), (1, 1));
        assert_eq!(data[0], 40);
    }

    const BALANCED_TOP: Level = Level { jpeg_quality: 70, full_chroma: false, scale: 1 };

    #[test]
    fn tiles_encode_to_jpeg() {
        let f = frame(200, 100, 128);
        let tile = encode_rect(&f, &Rect { x: 128, y: 0, w: 72, h: 100 }, &BALANCED_TOP).unwrap();
        assert_eq!(&tile.jpeg[..2], &[0xFF, 0xD8]); // marqueur SOI
    }

    /// Dimensions annoncées dans l'en-tête SOF0 d'un JPEG.
    fn jpeg_size(jpeg: &[u8]) -> (u16, u16) {
        let sof = jpeg.windows(2).position(|m| m == [0xFF, 0xC0]).unwrap();
        let be = |i: usize| u16::from_be_bytes([jpeg[sof + i], jpeg[sof + i + 1]]);
        (be(7), be(5))
    }

    #[test]
    fn half_tiles_keep_their_place_on_screen() {
        let f = frame(200, 100, 128);
        for (scale, size) in [(2, (36, 50)), (4, (18, 25))] {
            let level = Level { scale, ..BALANCED_TOP };
            let tile = encode_rect(&f, &Rect { x: 128, y: 0, w: 72, h: 100 }, &level).unwrap();
            assert_eq!((tile.x, tile.y, tile.w, tile.h), (128, 0, 72, 100));
            assert_eq!(jpeg_size(&tile.jpeg), size);
        }
    }

    /// Mesure du pipeline sur ce poste, et du poids de chaque niveau :
    /// `cargo test -p distant mesure -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn mesure_du_pipeline() {
        let monitors = Monitor::all().unwrap();
        let monitor = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).unwrap();
        for quality in [QUALITY_FAST, QUALITY_BALANCED, QUALITY_HIGH] {
            let preset = preset(quality);
            let levels = ladder(&preset, true);
            let (mut cap, mut prep, mut diff) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
            let mut encode = vec![(Duration::ZERO, 0usize); levels.len()];
            let mut prev: Option<Frame> = None;
            let n = 10;
            for _ in 0..n {
                let t = Instant::now();
                let image = monitor.capture_image().unwrap();
                cap += t.elapsed();
                let t = Instant::now();
                let frame = prepare(image, &preset);
                prep += t.elapsed();
                let grid = Grid::new(frame.width, frame.height);
                let t = Instant::now();
                let _ = grid.changed(prev.as_ref(), &frame);
                diff += t.elapsed();
                let rects = grid.runs(&grid.changed(None, &frame));
                for (level, total) in levels.iter().zip(&mut encode) {
                    let t = Instant::now();
                    let tiles: Vec<Tile> = rects.par_iter().map(|r| encode_rect(&frame, r, level).unwrap()).collect();
                    total.0 += t.elapsed();
                    total.1 += tiles.iter().map(|t| t.jpeg.len()).sum::<usize>();
                }
                prev = Some(frame);
            }
            println!("préréglage {quality} : capture {:?} · préparation {:?} · comparaison {:?}", cap / n, prep / n, diff / n);
            let top = encode[0].1 as f32;
            for (level, (time, bytes)) in levels.iter().zip(&encode) {
                println!(
                    "  qualité {:>2}{}{} : {:?}, {} Ko par écran complet, poids {:.2} (prévu {:.2})",
                    level.jpeg_quality,
                    if level.full_chroma { " 4:4:4" } else { "" },
                    match level.scale {
                        1 => "",
                        2 => " demi",
                        _ => " quart",
                    },
                    *time / n,
                    bytes / n as usize / 1000,
                    *bytes as f32 / top,
                    level.weight() / levels[0].weight(),
                );
            }
        }
    }
}
