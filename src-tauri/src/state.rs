//! État global de l'application, partagé entre les commandes et les tâches réseau.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{new_password, Config},
    update::UpdateStatus,
    viewer::ViewerSession,
};

pub struct AppState {
    pub config_path: PathBuf,
    pub config: Mutex<Config>,
    /// Mot de passe de session, régénéré à chaque lancement.
    pub password: Mutex<String>,
    pub host_status: Mutex<HostStatus>,
    /// Incrémenté pour relancer la connexion au relais (réglages modifiés).
    pub host_restart: watch::Sender<u64>,
    /// Vrai pendant qu'une connexion entrante s'authentifie ou est active :
    /// une seule à la fois, ce qui empêche aussi les essais de mot de passe en parallèle.
    pub host_busy: AtomicBool,
    /// Session entrante active (quelqu'un contrôle ce poste).
    pub incoming: Mutex<Option<IncomingSession>>,
    pub lockout: Mutex<Lockout>,
    /// Sessions sortantes (ce poste contrôle d'autres postes), par numéro.
    pub viewers: Mutex<HashMap<u32, Arc<ViewerSession>>>,
    pub next_sid: AtomicU32,
    /// Mise à jour de l'application : ce que la fenêtre affiche.
    pub update: Mutex<UpdateStatus>,
    /// Vrai pendant un téléchargement ou une installation : un seul à la fois.
    pub update_busy: AtomicBool,
    /// L'utilisateur demande d'installer sans attendre la fin des sessions.
    pub update_force: AtomicBool,
    pub update_now: Notify,
}

impl AppState {
    pub fn new(config_path: PathBuf, config: Config) -> Self {
        Self {
            config_path,
            config: Mutex::new(config),
            password: Mutex::new(new_password()),
            host_status: Mutex::new(HostStatus::new("connecting", "Démarrage…")),
            host_restart: watch::channel(0).0,
            host_busy: AtomicBool::new(false),
            incoming: Mutex::new(None),
            lockout: Mutex::new(Lockout::default()),
            viewers: Mutex::new(HashMap::new()),
            next_sid: AtomicU32::new(1),
            update: Mutex::new(UpdateStatus::default()),
            update_busy: AtomicBool::new(false),
            update_force: AtomicBool::new(false),
            update_now: Notify::new(),
        }
    }

    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }

    /// Modifie et enregistre les réglages.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) -> anyhow::Result<Config> {
        let mut config = self.config.lock().unwrap();
        f(&mut config);
        config.save(&self.config_path)?;
        Ok(config.clone())
    }

    pub fn restart_host(&self) {
        self.host_restart.send_modify(|n| *n += 1);
    }

    /// Réserve le poste pour une connexion entrante. `None` s'il est déjà pris.
    pub fn try_reserve_host(&self) -> Option<BusyGuard<'_>> {
        self.host_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| BusyGuard(self))
    }
}

pub struct BusyGuard<'a>(&'a AppState);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.host_busy.store(false, Ordering::Release);
    }
}

/// État de la connexion au relais, affiché dans la fenêtre principale.
#[derive(Serialize, Clone, Debug)]
pub struct HostStatus {
    /// `disabled`, `connecting`, `online` ou `error`.
    pub state: &'static str,
    pub message: String,
}

impl HostStatus {
    pub fn new(state: &'static str, message: impl Into<String>) -> Self {
        Self { state, message: message.into() }
    }
}

pub struct IncomingSession {
    pub peer_name: String,
    pub peer_os: String,
    pub cancel: CancellationToken,
}

#[derive(Serialize, Clone, Debug)]
pub struct IncomingInfo {
    pub active: bool,
    pub name: String,
    pub os: String,
}

/// Verrouillage après plusieurs mots de passe erronés : 30 s après le 3e échec,
/// puis le double à chaque nouvel échec, jusqu'à 10 minutes.
#[derive(Default)]
pub struct Lockout {
    failures: u32,
    until: Option<Instant>,
}

impl Lockout {
    pub fn is_locked(&self) -> bool {
        self.until.is_some_and(|t| Instant::now() < t)
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
        if self.failures >= 3 {
            let secs = (30u64 << (self.failures - 3).min(5)).min(600);
            self.until = Some(Instant::now() + Duration::from_secs(secs));
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Raccourcis pour notifier la fenêtre principale.
pub fn set_host_status(app: &AppHandle, state: &'static str, message: impl Into<String>) {
    let status = HostStatus::new(state, message);
    *app.state::<AppState>().host_status.lock().unwrap() = status.clone();
    let _ = app.emit("host-status", status);
}

pub fn emit_incoming(app: &AppHandle, info: IncomingInfo) {
    let _ = app.emit("incoming-session", info);
}

#[derive(Serialize, Clone)]
struct Notice {
    /// `alert` (tentative refusée) ou `info` (fichier reçu).
    kind: &'static str,
    message: String,
}

/// Alerte à lire : reste affichée dans la fenêtre principale.
pub fn emit_notice(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("host-notice", Notice { kind: "alert", message: message.into() });
}

/// Simple information (fichier reçu), sans caractère d'alerte.
pub fn emit_info(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("host-notice", Notice { kind: "info", message: message.into() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_after_three_failures() {
        let mut l = Lockout::default();
        l.record_failure();
        l.record_failure();
        assert!(!l.is_locked());
        l.record_failure();
        assert!(l.is_locked());
        l.reset();
        assert!(!l.is_locked());
    }
}
