//! Commandes appelées par l'interface (JavaScript → Rust).

use distant_proto::{
    format_id, normalize_id,
    secure::{derive_secret, normalize_password},
    session::{Control, InputEvent, QUALITY_FAST, QUALITY_HIGH},
};
use std::path::PathBuf;

use serde::Serialize;
use tauri::{ipc::Channel, AppHandle, State, WebviewWindow};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::{
    config::{new_password, to_hex},
    state::{AppState, HostStatus, IncomingInfo},
    update::UpdateStatus,
    viewer::{self, Credential, QueuedFile, SessionInfo, ViewerEvent},
};

type CmdResult<T = ()> = Result<T, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    id: String,
    password: String,
    hostname: String,
    status: HostStatus,
    accept_incoming: bool,
    relay: String,
    relay_key: String,
    unattended: bool,
    autostart: bool,
    auto_update: bool,
    update: UpdateStatus,
    machines: Vec<MachineView>,
    incoming: Option<IncomingInfo>,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    id: String,
    display_id: String,
    name: String,
    os: String,
    last: u64,
    saved: bool,
}

#[tauri::command]
pub fn get_overview(app: AppHandle, state: State<'_, AppState>) -> Overview {
    tracing::debug!("interface chargée : get_overview");
    let config = state.config();
    let incoming = state.incoming.lock().unwrap().as_ref().map(|s| IncomingInfo {
        active: true,
        name: s.peer_name.clone(),
        os: s.peer_os.clone(),
    });
    Overview {
        id: format_id(&config.id),
        password: state.password.lock().unwrap().clone(),
        hostname: crate::hostname(),
        status: state.host_status.lock().unwrap().clone(),
        accept_incoming: config.accept_incoming,
        relay: config.relay,
        relay_key: config.relay_key,
        unattended: config.unattended_secret.is_some(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        auto_update: config.auto_update,
        update: state.update.lock().unwrap().clone(),
        machines: config
            .machines
            .iter()
            .map(|m| MachineView {
                id: m.id.clone(),
                display_id: format_id(&m.id),
                name: m.name.clone(),
                os: m.os.clone(),
                last: m.last,
                saved: m.secret.is_some(),
            })
            .collect(),
        incoming,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
pub fn regenerate_password(state: State<'_, AppState>) -> String {
    let password = new_password();
    *state.password.lock().unwrap() = password.clone();
    password
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, relay: String, relay_key: String) -> CmdResult {
    let relay = relay.trim().to_string();
    if relay.is_empty() {
        return Err("Indiquez l'adresse du serveur relais.".into());
    }
    state
        .update_config(|c| {
            c.relay = relay;
            c.relay_key = relay_key.trim().to_string();
        })
        .map_err(|e| e.to_string())?;
    state.restart_host();
    Ok(())
}

#[tauri::command]
pub fn set_accept_incoming(state: State<'_, AppState>, enabled: bool) -> CmdResult {
    state.update_config(|c| c.accept_incoming = enabled).map_err(|e| e.to_string())?;
    if !enabled {
        // Couper aussi une éventuelle session en cours.
        if let Some(session) = state.incoming.lock().unwrap().as_ref() {
            session.cancel.cancel();
        }
    }
    state.restart_host();
    Ok(())
}

/// Définit (ou retire, avec `None`) le mot de passe permanent.
#[tauri::command]
pub async fn set_unattended_password(state: State<'_, AppState>, password: Option<String>) -> CmdResult {
    let secret = match password {
        Some(password) => {
            if normalize_password(&password).chars().count() < 8 {
                return Err("Le mot de passe permanent doit faire au moins 8 caractères.".into());
            }
            let id = state.config().id;
            let secret = tauri::async_runtime::spawn_blocking(move || derive_secret(&password, &id))
                .await
                .map_err(|e| e.to_string())?;
            Some(to_hex(&secret))
        }
        None => None,
    };
    state.update_config(|c| c.unattended_secret = secret).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult {
    let autolaunch = app.autolaunch();
    let result = if enabled { autolaunch.enable() } else { autolaunch.disable() };
    result.map_err(|e| format!("Impossible de modifier le lancement au démarrage : {e}"))
}

#[tauri::command]
pub fn forget_machine(state: State<'_, AppState>, id: String) -> CmdResult {
    let id = normalize_id(&id);
    state.update_config(|c| c.machines.retain(|m| m.id != id)).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn end_incoming(state: State<'_, AppState>) {
    if let Some(session) = state.incoming.lock().unwrap().as_ref() {
        session.cancel.cancel();
    }
}

#[tauri::command]
pub async fn connect(app: AppHandle, id: String, password: String, remember: bool) -> CmdResult<u32> {
    viewer::connect(&app, &id, Credential::Password { password, remember }).await
}

/// Connexion à une machine dont le mot de passe a été mémorisé.
#[tauri::command]
pub async fn connect_saved(app: AppHandle, id: String) -> CmdResult<u32> {
    viewer::connect(&app, &id, Credential::Saved).await
}

#[tauri::command]
pub async fn viewer_attach(
    app: AppHandle,
    sid: u32,
    on_frame: Channel,
    on_event: Channel<ViewerEvent>,
) -> CmdResult<SessionInfo> {
    viewer::attach(&app, sid, on_frame, on_event)
}

// Les commandes suivantes sont appelées très souvent (souris, accusés de
// réception) : elles sont `async` pour ne pas occuper le thread de l'interface.

#[tauri::command]
pub async fn viewer_ack(app: AppHandle, sid: u32, seq: u32) -> CmdResult {
    viewer::get(&app, sid)?.send(Control::FrameAck { seq });
    Ok(())
}

#[tauri::command]
pub async fn viewer_input(app: AppHandle, sid: u32, event: InputEvent) -> CmdResult {
    viewer::get(&app, sid)?.send(Control::Input { event });
    Ok(())
}

#[tauri::command]
pub async fn viewer_set_monitor(app: AppHandle, sid: u32, index: usize) -> CmdResult {
    viewer::get(&app, sid)?.send(Control::SetMonitor { index });
    Ok(())
}

#[tauri::command]
pub async fn viewer_set_quality(app: AppHandle, state: State<'_, AppState>, sid: u32, quality: u8) -> CmdResult {
    let quality = quality.clamp(QUALITY_FAST, QUALITY_HIGH);
    viewer::get(&app, sid)?.send(Control::SetQuality { quality });
    state.update_config(|c| c.quality = quality).map_err(|e| e.to_string())?;
    Ok(())
}

/// Ouvre le sélecteur de fichiers du système ; liste vide si l'utilisateur annule.
#[tauri::command]
pub async fn viewer_pick_files(window: WebviewWindow) -> CmdResult<Vec<PathBuf>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("Envoyer des fichiers au poste distant")
        .pick_files(move |paths| {
            let _ = tx.send(paths);
        });
    let paths = rx.await.map_err(|e| e.to_string())?.unwrap_or_default();
    Ok(paths.into_iter().filter_map(|p| p.into_path().ok()).collect())
}

/// Met des fichiers en file d'envoi vers le poste distant.
#[tauri::command]
pub async fn viewer_send_files(app: AppHandle, sid: u32, paths: Vec<PathBuf>) -> CmdResult<Vec<QueuedFile>> {
    Ok(viewer::get(&app, sid)?.send_files(paths).await)
}

#[tauri::command]
pub async fn viewer_cancel_file(app: AppHandle, sid: u32, id: u32) -> CmdResult {
    viewer::get(&app, sid)?.cancel_file(id);
    Ok(())
}

#[tauri::command]
pub async fn viewer_disconnect(app: AppHandle, sid: u32) -> CmdResult {
    viewer::disconnect(&app, sid);
    Ok(())
}
