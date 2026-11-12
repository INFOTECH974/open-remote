//! Mise à jour de l'application.
//!
//! Une copie installée lit `latest.json` dans la dernière version publiée,
//! télécharge l'installateur prévu pour elle, vérifie sa signature avec la clé
//! publique de `tauri.conf.json`, l'installe et redémarre.
//!
//! Un poste sans surveillance doit pouvoir recevoir un correctif sans personne
//! devant : quand le réglage « mises à jour automatiques » est actif, la
//! nouvelle version est téléchargée en arrière-plan puis installée dès que le
//! poste n'est plus en session, et l'application revient comme elle était
//! (réduite dans la zone de notification si sa fenêtre était fermée).

use std::{sync::atomic::Ordering, time::Duration};

use anyhow::{anyhow, Context};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::state::AppState;

/// Première recherche après le lancement : laisse le temps au réseau d'arriver.
const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// Poste en session : nouvel essai d'installation après ce délai.
const BUSY_RETRY: Duration = Duration::from_secs(60);

/// État affiché dans la fenêtre principale.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// Version publiée, plus récente que celle qui tourne.
    pub available: Option<String>,
    /// Vide, `downloading`, `waiting` (téléchargée, le poste est en session) ou `installing`.
    pub phase: &'static str,
    /// Avancement du téléchargement, quand le serveur annonce sa taille.
    pub percent: Option<u8>,
    pub error: Option<String>,
}

fn set_status(app: &AppHandle, change: impl FnOnce(&mut UpdateStatus)) {
    let state = app.state::<AppState>();
    let status = {
        let mut status = state.update.lock().unwrap();
        change(&mut status);
        status.clone()
    };
    let _ = app.emit("update-status", status);
}

/// Cherche une version plus récente et met l'état à jour.
async fn check(app: &AppHandle) -> anyhow::Result<Option<Update>> {
    let mut builder = app.updater_builder();
    if cfg!(debug_assertions) {
        // En développement, on peut viser un autre `latest.json` (serveur local).
        if let Ok(endpoint) = std::env::var("DISTANT_UPDATE_ENDPOINT") {
            builder = builder.endpoints(vec![endpoint.parse().context("DISTANT_UPDATE_ENDPOINT")?])?;
        }
        // Essai à blanc : la version publiée compte comme nouvelle, même si
        // c'est celle qui tourne, pour rejouer son téléchargement.
        if dry_run() {
            builder = builder.version_comparator(|_, _| true);
        }
    }
    let update = builder.build()?.check().await?;
    set_status(app, |s| {
        s.available = update.as_ref().map(|u| u.version.clone());
        s.error = None;
    });
    Ok(update)
}

/// Télécharge la mise à jour ; sa signature est vérifiée avant de rendre la main.
async fn download(app: &AppHandle, update: &Update) -> anyhow::Result<Vec<u8>> {
    set_status(app, |s| {
        s.phase = "downloading";
        s.percent = None;
        s.error = None;
    });
    let mut received = 0u64;
    let mut reported = None;
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                let percent = total
                    .filter(|total| *total > 0)
                    .map(|total| (received.saturating_mul(100) / total).min(100) as u8);
                // Un événement par pour cent, pas un par morceau reçu.
                if percent != reported {
                    reported = percent;
                    set_status(app, |s| s.percent = percent);
                }
            },
            || {},
        )
        .await?;
    Ok(bytes)
}

/// Installe la mise à jour téléchargée et redémarre : ne rend la main qu'en cas d'échec.
///
/// `hidden` : personne n'a demandé cette installation et la fenêtre est
/// fermée, la nouvelle version démarre donc réduite elle aussi.
fn install(app: &AppHandle, update: &Update, bytes: Vec<u8>, hidden: bool) -> anyhow::Result<()> {
    if dry_run() {
        return Err(anyhow!(
            "Essai : Distant {} téléchargé et vérifié ({} octets), non installé.",
            update.version,
            bytes.len()
        ));
    }
    set_status(app, |s| s.phase = "installing");
    let state = app.state::<AppState>();
    if hidden {
        state.update_config(|c| c.start_hidden_once = true)?;
    }
    tracing::info!("installation de Distant {}", update.version);
    // Sous Windows, l'installateur ferme l'application puis la relance lui-même.
    if let Err(e) = update.install(bytes) {
        if hidden {
            let _ = state.update_config(|c| c.start_hidden_once = false);
        }
        return Err(e.into());
    }
    app.restart()
}

/// Essai à blanc (`DISTANT_UPDATE_DRY_RUN`, builds de développement) : tout
/// se passe comme pour une vraie mise à jour, sauf l'installation.
fn dry_run() -> bool {
    cfg!(debug_assertions) && std::env::var_os("DISTANT_UPDATE_DRY_RUN").is_some()
}

/// Aucune session dans un sens ni dans l'autre : redémarrer ne coupe personne.
fn is_idle(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    !state.host_busy.load(Ordering::Acquire)
        && state.incoming.lock().unwrap().is_none()
        && state.viewers.lock().unwrap().is_empty()
}

fn main_window_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main").and_then(|w| w.is_visible().ok()).unwrap_or(false)
}

fn failed(app: &AppHandle, context: &str, error: &anyhow::Error) -> String {
    let message = format!("{context} : {error:#}");
    tracing::warn!("{message}");
    set_status(app, |s| {
        s.phase = "";
        s.percent = None;
        s.error = Some(message.clone());
    });
    message
}

// ── En arrière-plan ─────────────────────────────────────────────────────

pub fn spawn_service(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            match check(&app).await {
                Ok(Some(update)) => {
                    tracing::info!("Distant {} est disponible", update.version);
                    if app.state::<AppState>().config().auto_update {
                        install_when_idle(&app, &update).await;
                    }
                }
                Ok(None) => {}
                // Hors ligne ou serveur injoignable : rien à signaler, on réessaiera.
                Err(e) => tracing::debug!("recherche de mise à jour : {e:#}"),
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Télécharge tout de suite, installe quand le poste n'est plus en session.
async fn install_when_idle(app: &AppHandle, update: &Update) {
    let state = app.state::<AppState>();
    if state.update_busy.swap(true, Ordering::AcqRel) {
        return; // une installation demandée à la main est déjà en cours
    }
    let result = async {
        let bytes = download(app, update).await?;
        set_status(app, |s| s.phase = "waiting");
        let forced = loop {
            if state.update_force.load(Ordering::Acquire) {
                break true;
            }
            if is_idle(app) {
                break false;
            }
            tokio::select! {
                _ = tokio::time::sleep(BUSY_RETRY) => {}
                _ = state.update_now.notified() => {}
            }
        };
        // Réglage coupé pendant l'attente : la décision revient à l'utilisateur.
        if !forced && !state.config().auto_update {
            set_status(app, |s| s.phase = "");
            return Ok(());
        }
        install(app, update, bytes, !forced && !main_window_visible(app))
    }
    .await;
    state.update_force.store(false, Ordering::Release);
    state.update_busy.store(false, Ordering::Release);
    if let Err(e) = result {
        failed(app, "Mise à jour impossible", &e);
    }
}

// ── À la demande ────────────────────────────────────────────────────────

/// Recherche demandée depuis la fenêtre : contrairement à celle de l'arrière-plan, elle dit ses erreurs.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<UpdateStatus, String> {
    match check(&app).await {
        Ok(_) => Ok(app.state::<AppState>().update.lock().unwrap().clone()),
        Err(e) => Err(format!("Recherche de mise à jour impossible : {e:#}")),
    }
}

/// Installe la dernière version maintenant, même en session, puis redémarre.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.update_busy.swap(true, Ordering::AcqRel) {
        // Déjà téléchargée par l'arrière-plan, qui attendait que le poste soit libre.
        state.update_force.store(true, Ordering::Release);
        state.update_now.notify_one();
        return Ok(());
    }
    let result = async {
        let update = check(&app).await?.ok_or_else(|| anyhow!("Distant est déjà à jour."))?;
        let bytes = download(&app, &update).await?;
        install(&app, &update, bytes, false)
    }
    .await;
    state.update_busy.store(false, Ordering::Release);
    result.map_err(|e| failed(&app, "Mise à jour impossible", &e))
}

/// Active ou coupe l'installation automatique.
#[tauri::command]
pub fn set_auto_update(app: AppHandle, enabled: bool) -> Result<(), String> {
    app.state::<AppState>().update_config(|c| c.auto_update = enabled).map_err(|e| e.to_string())?;
    Ok(())
}
