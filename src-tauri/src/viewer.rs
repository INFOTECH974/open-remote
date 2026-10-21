//! Côté « visualiseur » : ce poste contrôle un autre poste.
//!
//! `connect` s'authentifie depuis la fenêtre principale, puis ouvre une
//! fenêtre dédiée. Cette fenêtre appelle `attach` pour recevoir les images
//! (en binaire, via un `Channel` Tauri) et envoie clavier/souris par commandes.

use std::{
    sync::{atomic::Ordering, Arc, Mutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Context};
use distant_proto::{
    format_id,
    frame::{read_frame, read_json, write_frame, write_json},
    is_valid_id, normalize_id,
    relay::{ClientMsg, RelayMsg},
    secure::{derive_secret, handshake, normalize_password, Role, SecureReader, SecureWriter},
    session::{
        decode, Control, Incoming, MonitorInfo, AUTH_FAILED, AUTH_RETRY, STATUS_BUSY,
        STATUS_LOCKED, STATUS_OK, TAG_FRAME,
    },
    PROTOCOL_VERSION,
};
use serde::Serialize;
use tauri::{ipc::Channel, ipc::InvokeResponseBody, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::{
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{from_hex, to_hex, Config},
    net::connect_relay,
    state::AppState,
};

pub struct ViewerSession {
    pub host_id: String,
    pub hostname: String,
    pub host_os: String,
    pub monitors: Vec<MonitorInfo>,
    out: mpsc::Sender<Control>,
    cancel: CancellationToken,
    /// Flux entrant, récupéré par la fenêtre du visualiseur à son ouverture.
    reader: Mutex<Option<SecureReader<OwnedReadHalf>>>,
}

impl ViewerSession {
    /// Envoi non bloquant ; un mouvement de souris perdu si le réseau sature n'a pas d'importance.
    pub fn send(&self, control: Control) {
        let _ = self.out.try_send(control);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub host_id: String,
    pub hostname: String,
    pub os: String,
    pub monitors: Vec<MonitorInfo>,
    pub monitor: usize,
    pub quality: u8,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViewerEvent {
    Closed { reason: String },
}

/// Comment s'authentifier auprès du poste distant.
pub enum Credential {
    /// Mot de passe saisi ; `remember` le mémorise (sous forme de secret dérivé).
    Password { password: String, remember: bool },
    /// Secret mémorisé lors d'une connexion précédente.
    Saved,
}

struct Opened {
    reader: SecureReader<OwnedReadHalf>,
    writer: SecureWriter<OwnedWriteHalf>,
    hostname: String,
    os: String,
    monitors: Vec<MonitorInfo>,
}

/// Se connecte à l'hôte `id_input` et ouvre la fenêtre de contrôle.
pub async fn connect(app: &AppHandle, id_input: &str, credential: Credential) -> Result<u32, String> {
    let id = normalize_id(id_input);
    if !is_valid_id(&id) {
        return Err("L'ID doit comporter 9 chiffres.".into());
    }

    let state = app.state::<AppState>();
    let config = state.config();
    let (secret, remember) = match credential {
        Credential::Password { password, remember } => {
            if normalize_password(&password).is_empty() {
                return Err("Saisissez le mot de passe du poste distant.".into());
            }
            let host_id = id.clone();
            let secret = tauri::async_runtime::spawn_blocking(move || derive_secret(&password, &host_id))
                .await
                .map_err(|e| e.to_string())?;
            // Saisie manuelle : on mémorise ou on oublie selon la case cochée.
            (secret, Some(remember.then(|| to_hex(&secret))))
        }
        Credential::Saved => {
            let secret = config
                .machine(&id)
                .and_then(|m| m.secret.as_deref())
                .and_then(from_hex)
                .ok_or("Aucun mot de passe mémorisé pour ce poste.")?;
            (secret, None)
        }
    };

    let opened = timeout(Duration::from_secs(40), open_session(&config, &id, &secret))
        .await
        .map_err(|_| "Délai de connexion dépassé.".to_string())?
        .map_err(|e| e.to_string())?;

    let sid = state.next_sid.fetch_add(1, Ordering::Relaxed);
    let (out_tx, out_rx) = mpsc::channel::<Control>(256);
    tauri::async_runtime::spawn(writer_loop(opened.writer, out_rx));

    let session = Arc::new(ViewerSession {
        host_id: id.clone(),
        hostname: opened.hostname,
        host_os: opened.os,
        monitors: opened.monitors,
        out: out_tx,
        cancel: CancellationToken::new(),
        reader: Mutex::new(Some(opened.reader)),
    });
    state.viewers.lock().unwrap().insert(sid, session.clone());
    let saved = state.update_config(|c| c.record_machine(&id, &session.hostname, &session.host_os, remember));
    if let Err(e) = saved {
        tracing::warn!("réglages non enregistrés : {e:#}");
    }

    let title = format!("{} — {}", session.hostname, format_id(&id));
    let window = WebviewWindowBuilder::new(
        app,
        format!("viewer-{sid}"),
        WebviewUrl::App(format!("viewer.html?sid={sid}").into()),
    )
    .title(title)
    .inner_size(1280.0, 800.0)
    .min_inner_size(640.0, 400.0)
    .center()
    .build();
    if let Err(e) = window {
        disconnect(app, sid);
        return Err(format!("Impossible d'ouvrir la fenêtre : {e}"));
    }
    Ok(sid)
}

async fn open_session(config: &Config, id: &str, secret: &[u8; 32]) -> anyhow::Result<Opened> {
    let mut sock = connect_relay(&config.relay).await?;
    let key = Some(config.relay_key.trim().to_string()).filter(|k| !k.is_empty());
    write_json(&mut sock, &ClientMsg::Connect { id: id.into(), version: PROTOCOL_VERSION, key }).await?;

    // Le relais attend jusqu'à 15 s que l'hôte réponde.
    match read_json::<RelayMsg, _>(&mut sock).await.context("Le relais a fermé la connexion")? {
        RelayMsg::Paired => {}
        RelayMsg::Error { code } => bail!(code.message()),
        _ => bail!("Réponse inattendue du relais."),
    }

    let status = read_frame(&mut sock).await.context("Le poste distant a fermé la connexion")?;
    match status.as_slice() {
        STATUS_OK => {}
        STATUS_BUSY => bail!("Ce poste est déjà contrôlé par quelqu'un d'autre."),
        STATUS_LOCKED => bail!(
            "Trop d'essais de mot de passe : ce poste refuse les connexions pendant quelques minutes."
        ),
        _ => bail!("Réponse inattendue du poste distant."),
    }

    let hello = Control::Hello {
        name: crate::hostname(),
        os: std::env::consts::OS.into(),
        version: PROTOCOL_VERSION,
    }
    .encode();

    // L'hôte essaie son mot de passe temporaire, puis son mot de passe
    // permanent : il répond AUTH_RETRY entre les deux.
    for _ in 0..3 {
        let (mut sealer, mut opener) = handshake(&mut sock, secret, id, Role::Viewer).await?;
        write_frame(&mut sock, &sealer.seal(&hello)).await?;
        let reply = read_frame(&mut sock)
            .await
            .map_err(|_| anyhow!("Le poste distant a fermé la connexion."))?;
        if reply == AUTH_RETRY {
            continue;
        }
        if reply == AUTH_FAILED {
            bail!("Mot de passe incorrect.");
        }
        let plaintext = opener.open(&reply).map_err(|_| anyhow!("Mot de passe incorrect."))?;
        let Incoming::Control(Control::Welcome { hostname, os, monitors }) = decode(&plaintext)? else {
            bail!("Réponse inattendue du poste distant.");
        };
        let (rd, wr) = sock.into_split();
        return Ok(Opened {
            reader: SecureReader::new(rd, opener),
            writer: SecureWriter::new(wr, sealer),
            hostname,
            os,
            monitors,
        });
    }
    bail!("Réponse inattendue du poste distant.")
}

async fn writer_loop(mut writer: SecureWriter<OwnedWriteHalf>, mut rx: mpsc::Receiver<Control>) {
    let mut ping = tokio::time::interval(Duration::from_secs(10));
    ping.tick().await;
    loop {
        let msg = tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => msg,
                None => break,
            },
            _ = ping.tick() => Control::Ping,
        };
        let bye = msg == Control::Bye;
        if writer.send(&msg.encode()).await.is_err() || bye {
            break;
        }
    }
}

/// Appelé par la fenêtre du visualiseur : démarre la réception des images.
pub fn attach(
    app: &AppHandle,
    sid: u32,
    on_frame: Channel,
    on_event: Channel<ViewerEvent>,
) -> Result<SessionInfo, String> {
    let state = app.state::<AppState>();
    let session = get(app, sid)?;
    let mut reader = session
        .reader
        .lock()
        .unwrap()
        .take()
        .ok_or("Cette session est déjà affichée.")?;

    let monitor = session.monitors.iter().position(|m| m.primary).unwrap_or(0);
    let quality = state.config().quality;
    let cancel = session.cancel.clone();

    tauri::async_runtime::spawn(async move {
        let reason = loop {
            let msg = tokio::select! {
                _ = cancel.cancelled() => break "Session terminée.".to_string(),
                msg = reader.recv() => msg,
            };
            let Ok(mut msg) = msg else {
                break "Connexion perdue avec le poste distant.".into();
            };
            if msg.first() == Some(&TAG_FRAME) {
                msg.drain(..1);
                if on_frame.send(InvokeResponseBody::Raw(msg)).is_err() {
                    break "Fenêtre fermée.".into();
                }
                continue;
            }
            match decode(&msg) {
                Ok(Incoming::Control(Control::Bye)) => {
                    break "Session terminée par le poste distant.".into();
                }
                Ok(_) => {}
                Err(_) => break "Message invalide reçu du poste distant.".into(),
            }
        };
        cancel.cancel();
        let _ = on_event.send(ViewerEvent::Closed { reason });
    });

    session.send(Control::Start { monitor, quality });
    Ok(SessionInfo {
        host_id: format_id(&session.host_id),
        hostname: session.hostname.clone(),
        os: session.host_os.clone(),
        monitors: session.monitors.clone(),
        monitor,
        quality,
    })
}

pub fn get(app: &AppHandle, sid: u32) -> Result<Arc<ViewerSession>, String> {
    app.state::<AppState>()
        .viewers
        .lock()
        .unwrap()
        .get(&sid)
        .cloned()
        .ok_or_else(|| "Session introuvable.".to_string())
}

pub fn disconnect(app: &AppHandle, sid: u32) {
    let session = app.state::<AppState>().viewers.lock().unwrap().remove(&sid);
    if let Some(session) = session {
        session.send(Control::Bye);
        session.cancel.cancel();
    }
}
