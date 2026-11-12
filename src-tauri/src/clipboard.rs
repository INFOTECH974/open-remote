//! Presse-papiers partagé entre le poste qui contrôle et le poste contrôlé.
//!
//! Chaque côté surveille son presse-papiers pendant la session ; quand le
//! texte change, il l'envoie à l'autre, qui le place dans le sien. Copier
//! d'un côté, coller de l'autre : dans les deux sens.
//!
//! - Seul le texte est partagé (pas les images ni les fichiers).
//! - Ce qui était dans le presse-papiers avant la session n'est pas envoyé :
//!   seulement ce qui est copié ensuite.
//! - Côté visualiseur, rien ne part tant que la fenêtre de session n'est pas
//!   au premier plan : un mot de passe copié pour une autre application ne
//!   voyage que si l'on revient ensuite dans la session.
//! - Le presse-papiers emprunte le canal chiffré de la session, comme le reste.

use std::{
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

use tokio_util::sync::CancellationToken;

/// Intervalle de relecture du presse-papiers.
const POLL: Duration = Duration::from_millis(400);
/// Au-delà, le texte n'est pas partagé.
pub const MAX_TEXT: usize = 1024 * 1024;

enum Command {
    /// Texte reçu de l'autre poste, à placer dans le presse-papiers.
    Set(String),
    /// Autorise ou suspend l'envoi des changements locaux.
    Active(bool),
}

/// Poignée vers le fil qui surveille le presse-papiers ; il s'arrête avec la session.
#[derive(Clone)]
pub struct ClipboardSync {
    commands: mpsc::Sender<Command>,
}

impl ClipboardSync {
    /// `on_change` reçoit chaque nouveau texte copié sur ce poste, tant que
    /// la surveillance est active.
    pub fn spawn(active: bool, cancel: CancellationToken, on_change: impl Fn(String) + Send + 'static) -> Self {
        let (commands, rx) = mpsc::channel();
        let thread = std::thread::Builder::new().name("distant-clipboard".into()).spawn(move || {
            let mut clipboard = match arboard::Clipboard::new() {
                Ok(clipboard) => clipboard,
                Err(e) => {
                    tracing::warn!("presse-papiers inaccessible : {e}");
                    return;
                }
            };
            let mut tracker = Tracker::new(clipboard.get_text().ok());
            let mut active = active;
            while !cancel.is_cancelled() {
                match rx.recv_timeout(POLL) {
                    Ok(Command::Set(text)) => {
                        if text.len() <= MAX_TEXT && clipboard.set_text(text.as_str()).is_ok() {
                            tracker.remember(&text);
                        }
                        continue;
                    }
                    // Redevenu actif : on regarde tout de suite ce qui a été copié entre-temps.
                    Ok(Command::Active(now)) => active = now,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if !active {
                    continue;
                }
                // Une erreur ici veut dire « pas de texte » (image, presse-papiers vide ou occupé).
                if let Some(text) = clipboard.get_text().ok().and_then(|text| tracker.changed(text)) {
                    on_change(text);
                }
            }
        });
        if let Err(e) = thread {
            tracing::warn!("surveillance du presse-papiers impossible : {e}");
        }
        Self { commands }
    }

    /// Place dans le presse-papiers le texte reçu de l'autre poste.
    pub fn set(&self, text: String) {
        let _ = self.commands.send(Command::Set(text));
    }

    pub fn set_active(&self, active: bool) {
        let _ = self.commands.send(Command::Active(active));
    }
}

/// Retient le dernier texte connu des deux postes, pour n'envoyer que les
/// vrais changements et ne jamais renvoyer à l'autre ce qu'il vient d'envoyer.
struct Tracker {
    last: Option<String>,
}

/// Windows écrit les fins de ligne en CRLF, macOS et Linux en LF : le même
/// texte ne doit pas passer pour un changement en faisant l'aller-retour.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

impl Tracker {
    fn new(initial: Option<String>) -> Self {
        Self { last: initial.map(|text| normalize(&text)) }
    }

    fn remember(&mut self, text: &str) {
        self.last = Some(normalize(text));
    }

    /// Le texte à envoyer s'il diffère du dernier connu.
    fn changed(&mut self, text: String) -> Option<String> {
        if text.is_empty() || text.len() > MAX_TEXT {
            return None;
        }
        let normalized = normalize(&text);
        if self.last.as_deref() == Some(normalized.as_str()) {
            return None;
        }
        self.last = Some(normalized);
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_text_is_sent() {
        let mut tracker = Tracker::new(Some("déjà là".into()));
        // Ce qui était copié avant la session ne part pas.
        assert_eq!(tracker.changed("déjà là".into()), None);
        assert_eq!(tracker.changed("nouveau".into()), Some("nouveau".into()));
        assert_eq!(tracker.changed("nouveau".into()), None);
    }

    #[test]
    fn received_text_is_not_sent_back() {
        let mut tracker = Tracker::new(None);
        tracker.remember("ligne 1\nligne 2");
        // Windows relit ce même texte avec des fins de ligne CRLF.
        assert_eq!(tracker.changed("ligne 1\r\nligne 2".into()), None);
        assert_eq!(tracker.changed("autre chose".into()), Some("autre chose".into()));
    }

    #[test]
    fn empty_and_huge_texts_are_ignored() {
        let mut tracker = Tracker::new(None);
        assert_eq!(tracker.changed(String::new()), None);
        assert_eq!(tracker.changed("x".repeat(MAX_TEXT + 1)), None);
        assert_eq!(tracker.changed("x".repeat(MAX_TEXT)).map(|t| t.len()), Some(MAX_TEXT));
    }
}
