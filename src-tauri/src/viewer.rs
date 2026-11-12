//! Côté « visualiseur » : ce poste contrôle un autre poste.
//!
//! `connect` s'authentifie depuis la fenêtre principale, puis ouvre une
//! fenêtre dédiée. Cette fenêtre appelle `attach` pour recevoir les images
//! (en binaire, via un `Channel` Tauri) et envoie clavier/souris par commandes.
//! Elle peut aussi envoyer des fichiers au poste distant (voir [`crate::files`]).

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
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
    transport::Stream,
    PROTOCOL_VERSION,
};
use serde::Serialize;
use tauri::{ipc::Channel, ipc::InvokeResponseBody, AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::{
    io::{ReadHalf, WriteHalf},
    sync::mpsc,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{from_hex, to_hex, Config},
    files::{self, SendOutcome},
    net::connect_relay,
    state::AppState,
};

/// Intervalle minimal entre deux événements de progression d'un envoi.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub struct ViewerSession {
    pub host_id: String,
    pub hostname: String,
    pub host_os: String,
    pub monitors: Vec<MonitorInfo>,
    out: mpsc::Sender<Control>,
    cancel: CancellationToken,
    /// Flux entrant, récupéré par la fenêtre du visualiseur à son ouverture.
    reader: Mutex<Option<SecureReader<ReadHalf<Stream>>>>,
    /// Canal d'événements de la fenêtre, connu une fois celle-ci ouverte.
    events: Mutex<Option<Channel<ViewerEvent>>>,
    /// Fichiers à envoyer, traités un par un par [`file_sender`].
    jobs: mpsc::UnboundedSender<FileJob>,
    next_file: AtomicU32,
    /// Envois annulés depuis la fenêtre et pas encore pris en compte.
    cancelled_files: Mutex<HashSet<u32>>,
}

struct FileJob {
    id: u32,
    path: PathBuf,
}

/// Fichier accepté (ou refusé, avec `error`) dans la file d'envoi.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedFile {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub error: Option<String>,
}

impl ViewerSession {
    /// Envoi non bloquant ; un mouvement de souris perdu si le réseau sature n'a pas d'importance.
    pub fn send(&self, control: Control) {
        let _ = self.out.try_send(control);
    }

    fn emit(&self, event: ViewerEvent) {
        if let Some(channel) = self.events.lock().unwrap().as_ref() {
            let _ = channel.send(event);
        }
    }

    /// Met des fichiers en file d'envoi vers le poste distant.
    pub async fn send_files(&self, paths: Vec<PathBuf>) -> Vec<QueuedFile> {
        let mut queued = Vec::with_capacity(paths.len());
        for path in paths {
            let id = self.next_file.fetch_add(1, Ordering::Relaxed);
            queued.push(match files::describe(&path).await {
                Ok((name, size)) => {
                    let _ = self.jobs.send(FileJob { id, path });
                    QueuedFile { id, name, size, error: None }
                }
                Err(e) => {
                    let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
                    QueuedFile { id, name, size: 0, error: Some(e.to_string()) }
                }
            });
        }
        queued
    }

    /// Annule l'envoi `id`, qu'il soit en cours ou encore en attente.
    pub fn cancel_file(&self, id: u32) {
        self.cancelled_files.lock().unwrap().insert(id);
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
    /// `sent` octets du fichier `id` sont partis.
    FileProgress { id: u32, sent: u64 },
    /// Tout le fichier est parti ; on attend la confirmation du poste distant.
    FileSent { id: u32 },
    /// Le poste distant a enregistré le fichier sous le nom `name`.
    FileDone { id: u32, name: String },
    FileError { id: u32, message: String },
}

/// Comment s'authentifier auprès du poste distant.
pub enum Credential {
    /// Mot de passe saisi ; `remember` le mémorise (sous forme de secret dérivé).
    Password { password: String, remember: bool },
    /// Secret mémorisé lors d'une connexion précédente.
    Saved,
}

struct Opened {
    reader: SecureReader<ReadHalf<Stream>>,
    writer: SecureWriter<WriteHalf<Stream>>,
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
    // Très court : la lecture des fichiers suit le débit du réseau.
    let (file_tx, file_rx) = mpsc::channel::<Vec<u8>>(2);
    let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
    tauri::async_runtime::spawn(writer_loop(opened.writer, out_rx, file_rx));

    let session = Arc::new(ViewerSession {
        host_id: id.clone(),
        hostname: opened.hostname,
        host_os: opened.os,
        monitors: opened.monitors,
        out: out_tx,
        cancel: CancellationToken::new(),
        reader: Mutex::new(Some(opened.reader)),
        events: Mutex::new(None),
        jobs: jobs_tx,
        next_file: AtomicU32::new(1),
        cancelled_files: Mutex::new(HashSet::new()),
    });
    tauri::async_runtime::spawn(file_sender(session.clone(), jobs_rx, file_tx));
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
        let (rd, wr) = tokio::io::split(sock);
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

async fn writer_loop(
    mut writer: SecureWriter<WriteHalf<Stream>>,
    mut rx: mpsc::Receiver<Control>,
    mut files: mpsc::Receiver<Vec<u8>>,
) {
    let mut ping = tokio::time::interval(Duration::from_secs(10));
    ping.tick().await;
    loop {
        // Clavier, souris et accusés de réception passent avant les fichiers :
        // le poste distant reste réactif pendant un envoi.
        let (payload, bye) = tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(msg) => (msg.encode(), msg == Control::Bye),
                None => break,
            },
            _ = ping.tick() => (Control::Ping.encode(), false),
            Some(payload) = files.recv() => (payload, false),
        };
        if writer.send(&payload).await.is_err() || bye {
            break;
        }
    }
}

/// Envoie les fichiers de la file, un à la fois, jusqu'à la fin de la session.
async fn file_sender(
    session: Arc<ViewerSession>,
    mut jobs: mpsc::UnboundedReceiver<FileJob>,
    out: mpsc::Sender<Vec<u8>>,
) {
    let cancel = session.cancel.clone();
    loop {
        let job = tokio::select! {
            _ = cancel.cancelled() => break,
            job = jobs.recv() => match job {
                Some(job) => job,
                None => break,
            },
        };
        let id = job.id;
        let cancelled = || session.cancelled_files.lock().unwrap().contains(&id);
        let mut last_progress = Instant::now();
        let progress = |sent| {
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                last_progress = Instant::now();
                session.emit(ViewerEvent::FileProgress { id, sent });
            }
        };
        // Annulé avant d'avoir commencé : rien n'est envoyé.
        let result = if cancelled() {
            Ok(SendOutcome::Cancelled)
        } else {
            tokio::select! {
                _ = cancel.cancelled() => break,
                result = files::send_file(id, &job.path, &out, cancelled, progress) => result,
            }
        };
        match result {
            Ok(SendOutcome::Sent) => session.emit(ViewerEvent::FileSent { id }),
            Ok(SendOutcome::Cancelled) => {}
            Err(e) => session.emit(ViewerEvent::FileError { id, message: e.to_string() }),
        }
        session.cancelled_files.lock().unwrap().remove(&id);
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
    *session.events.lock().unwrap() = Some(on_event);

    let events = session.clone();
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
                Ok(Incoming::Control(Control::FileDone { id, name })) => {
                    events.emit(ViewerEvent::FileDone { id, name });
                }
                Ok(Incoming::Control(Control::FileError { id, message })) => {
                    // Le poste distant a abandonné ce fichier : inutile d'envoyer la suite.
                    events.cancel_file(id);
                    events.emit(ViewerEvent::FileError { id, message });
                }
                Ok(_) => {}
                Err(_) => break "Message invalide reçu du poste distant.".into(),
            }
        };
        cancel.cancel();
        events.emit(ViewerEvent::Closed { reason });
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
