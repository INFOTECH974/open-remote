//! Injection des événements clavier/souris reçus du visualiseur.
//!
//! Le clavier est transmis en « touches physiques » (`KeyboardEvent.code`) :
//! le poste contrôlé applique sa propre disposition clavier, exactement comme
//! un clavier branché dessus. Les raccourcis, AltGr et touches mortes
//! fonctionnent quand les deux postes ont la même disposition (AZERTY ↔ AZERTY).

mod keymap;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows::Backend;

#[cfg(not(windows))]
mod enigo_backend;
#[cfg(not(windows))]
use enigo_backend::Backend;

use std::{
    collections::HashSet,
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
};

use distant_proto::session::{InputEvent, MouseButton};

use super::capture::CaptureSettings;

enum Cmd {
    Event(InputEvent),
    Stop,
}

/// Thread dédié à l'injection (certaines API système n'aiment pas changer de thread).
pub struct InputWorker {
    tx: Sender<Cmd>,
}

impl InputWorker {
    pub fn spawn(settings: Arc<CaptureSettings>) -> Self {
        let (tx, rx) = channel();
        let result = std::thread::Builder::new()
            .name("distant-input".into())
            .spawn(move || run(rx, settings));
        if let Err(e) = result {
            tracing::error!("impossible de lancer l'injection clavier/souris : {e}");
        }
        Self { tx }
    }

    pub fn send(&self, event: InputEvent) {
        let _ = self.tx.send(Cmd::Event(event));
    }
}

impl Drop for InputWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

fn run(rx: Receiver<Cmd>, settings: Arc<CaptureSettings>) {
    let mut backend = match Backend::new() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("contrôle clavier/souris indisponible : {e:#}");
            return;
        }
    };
    let mut keys: HashSet<&'static str> = HashSet::new();
    let mut buttons: HashSet<MouseButton> = HashSet::new();

    while let Ok(Cmd::Event(event)) = rx.recv() {
        match event {
            InputEvent::MouseMove { x, y } => {
                let g = settings.geometry();
                let px = g.x + (x.clamp(0.0, 1.0) * (g.width - 1) as f32).round() as i32;
                let py = g.y + (y.clamp(0.0, 1.0) * (g.height - 1) as f32).round() as i32;
                backend.move_to(px, py);
            }
            InputEvent::MouseButton { button, down } => {
                if down {
                    buttons.insert(button);
                } else {
                    buttons.remove(&button);
                }
                backend.button(button, down);
            }
            InputEvent::Wheel { dx, dy } => backend.wheel(dx, dy),
            InputEvent::Key { code, down, .. } => {
                let Some(key) = keymap::lookup(&code) else { continue };
                if down {
                    keys.insert(key.code);
                } else {
                    keys.remove(key.code);
                }
                backend.key(key, down);
            }
            InputEvent::ReleaseAll => release_all(&mut backend, &mut keys, &mut buttons),
        }
    }
    // Fin de session : rien ne doit rester enfoncé sur le poste contrôlé.
    release_all(&mut backend, &mut keys, &mut buttons);
}

fn release_all(backend: &mut Backend, keys: &mut HashSet<&'static str>, buttons: &mut HashSet<MouseButton>) {
    for code in keys.drain() {
        if let Some(key) = keymap::lookup(code) {
            backend.key(key, false);
        }
    }
    for button in buttons.drain() {
        backend.button(button, false);
    }
}
