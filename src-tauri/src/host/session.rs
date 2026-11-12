//! Une session entrante : authentification, envoi des images, réception des
//! événements clavier/souris et des fichiers.

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, bail};
use distant_proto::{
    frame::{read_frame, read_json, write_frame, write_json},
    relay::{ClientMsg, RelayMsg},
    secure::{derive_secret, handshake, Opener, Role, Sealer, SecureReader, SecureWriter},
    session::{
        decode, Control, Incoming, AUTH_FAILED, AUTH_RETRY, STATUS_BUSY, STATUS_LOCKED, STATUS_OK,
    },
};
use tauri::{AppHandle, Manager};
use tokio::{
    net::{tcp::OwnedWriteHalf, TcpStream},
    sync::mpsc,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

use super::{
    capture::{self, CaptureSettings},
    input::InputWorker,
};
use crate::{
    config::{from_hex, Config},
    files,
    net::connect_relay,
    state::{emit_incoming, emit_info, emit_notice, AppState, IncomingInfo, IncomingSession},
};

const STEP_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn handle_incoming(app: AppHandle, config: Config, token: String) -> anyhow::Result<()> {
    let state = app.state::<AppState>();

    let mut sock = connect_relay(&config.relay).await?;
    write_json(&mut sock, &ClientMsg::Accept { token }).await?;
    let reply: RelayMsg = timeout(STEP_TIMEOUT, read_json(&mut sock))
        .await
        .map_err(|_| anyhow!("le relais ne répond pas"))??;
    if reply != RelayMsg::Paired {
        bail!("mise en relation refusée : {reply:?}");
    }

    // Une seule connexion entrante à la fois (authentification comprise).
    let Some(_busy) = state.try_reserve_host() else {
        write_frame(&mut sock, STATUS_BUSY).await?;
        return Ok(());
    };
    let locked = state.lockout.lock().unwrap().is_locked();
    if locked {
        write_frame(&mut sock, STATUS_LOCKED).await?;
        return Ok(());
    }
    write_frame(&mut sock, STATUS_OK).await?;

    // Secrets acceptés : mot de passe temporaire, puis mot de passe permanent
    // (accès sans surveillance) s'il est défini.
    let password = state.password.lock().unwrap().clone();
    let host_id = config.id.clone();
    let temporary = tokio::task::spawn_blocking(move || derive_secret(&password, &host_id)).await?;
    let mut candidates = vec![temporary];
    if let Some(unattended) = config.unattended_secret.as_deref().and_then(from_hex) {
        candidates.push(unattended);
    }

    // Pour chaque secret : échange SPAKE2, puis le premier message chiffré
    // doit être lisible. Sinon on propose le suivant, puis on refuse.
    let mut authenticated = None;
    for (i, secret) in candidates.iter().enumerate() {
        let (sealer, mut opener) = timeout(STEP_TIMEOUT, handshake(&mut sock, secret, &config.id, Role::Host))
            .await
            .map_err(|_| anyhow!("échange de clés trop long"))??;
        let first = timeout(STEP_TIMEOUT, read_frame(&mut sock))
            .await
            .map_err(|_| anyhow!("visualiseur muet"))??;
        if let Ok(plaintext) = opener.open(&first) {
            authenticated = Some((sealer, opener, plaintext));
            break;
        }
        if i + 1 < candidates.len() {
            write_frame(&mut sock, AUTH_RETRY).await?;
        }
    }
    let Some((sealer, opener, hello)) = authenticated else {
        state.lockout.lock().unwrap().record_failure();
        let _ = write_frame(&mut sock, AUTH_FAILED).await;
        emit_notice(&app, "Tentative de connexion refusée : mot de passe incorrect.");
        return Ok(());
    };
    state.lockout.lock().unwrap().reset();

    let Incoming::Control(Control::Hello { name, os, .. }) = decode(&hello)? else {
        bail!("premier message inattendu");
    };
    let name = clean_label(&name);
    let os = clean_label(&os);

    let cancel = CancellationToken::new();
    *state.incoming.lock().unwrap() = Some(IncomingSession {
        peer_name: name.clone(),
        peer_os: os.clone(),
        cancel: cancel.clone(),
    });
    emit_incoming(&app, IncomingInfo { active: true, name: name.clone(), os: os.clone() });
    tracing::info!("session entrante depuis « {name} » ({os})");

    let result = run_session(&app, &name, sock, sealer, opener, cancel).await;

    *state.incoming.lock().unwrap() = None;
    emit_incoming(&app, IncomingInfo { active: false, name: name.clone(), os });
    tracing::info!("session avec « {name} » terminée");
    result
}

/// Nom affiché venant de l'autre poste : on le borne et on retire les caractères de contrôle.
fn clean_label(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
    if cleaned.trim().is_empty() { "inconnu".into() } else { cleaned }
}

/// Dossier où arrivent les fichiers envoyés par le visualiseur.
fn downloads_dir(app: &AppHandle) -> PathBuf {
    let path = app.path();
    path.download_dir()
        .or_else(|_| path.home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(|_| std::env::temp_dir())
}

async fn run_session(
    app: &AppHandle,
    peer: &str,
    sock: TcpStream,
    sealer: Sealer,
    opener: Opener,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let (rd, wr) = sock.into_split();
    let mut reader = SecureReader::new(rd, opener);
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(4);
    let writer = tokio::spawn(writer_loop(SecureWriter::new(wr, sealer), out_rx, cancel.clone()));

    let monitors = capture::list_monitors();
    let hostname = crate::hostname();
    out_tx
        .send(
            Control::Welcome { hostname, os: std::env::consts::OS.into(), monitors }.encode(),
        )
        .await?;

    let settings = Arc::new(CaptureSettings::new());
    let input = InputWorker::spawn(settings.clone());
    let (ack_tx, ack_rx) = std::sync::mpsc::channel::<u32>();
    let mut ack_rx = Some(ack_rx);

    // Maintien de connexion : un message toutes les 10 s, même écran figé.
    {
        let out_tx = out_tx.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tick.tick() => { let _ = out_tx.try_send(Control::Ping.encode()); }
                }
            }
        });
    }

    let mut received = files::Receiver::new(downloads_dir(app));
    let read_loop = async {
        loop {
            let msg = reader.recv().await?;
            let control = match decode(&msg)? {
                Incoming::Control(control) => control,
                Incoming::File { id, data } => {
                    if let Err(e) = received.chunk(id, data).await {
                        out_tx.send(Control::FileError { id, message: e.to_string() }.encode()).await?;
                    }
                    continue;
                }
                Incoming::Frame(_) => continue,
            };
            match control {
                Control::Start { monitor, quality } => {
                    if let Some(acks) = ack_rx.take() {
                        settings.set_monitor(monitor);
                        settings.set_quality(quality);
                        capture::spawn(settings.clone(), out_tx.clone(), acks, cancel.clone());
                    }
                }
                Control::SetMonitor { index } => settings.set_monitor(index),
                Control::SetQuality { quality } => settings.set_quality(quality),
                Control::FrameAck { seq } => {
                    let _ = ack_tx.send(seq);
                }
                Control::Input { event } => input.send(event),
                Control::FileStart { id, name, size } => {
                    if let Err(e) = received.start(id, &name, size).await {
                        out_tx.send(Control::FileError { id, message: e.to_string() }.encode()).await?;
                    }
                }
                Control::FileEnd { id } => {
                    let reply = match received.finish(id).await {
                        Ok(Some(path)) => {
                            let name =
                                path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                            tracing::info!("fichier reçu : {}", path.display());
                            emit_info(
                                app,
                                format!("Fichier reçu de « {peer} » : {name}, dans {}.", received.dir().display()),
                            );
                            Control::FileDone { id, name }
                        }
                        Ok(None) => continue,
                        Err(e) => Control::FileError { id, message: e.to_string() },
                    };
                    out_tx.send(reply.encode()).await?;
                }
                Control::FileCancel { id } => received.cancel(id).await,
                Control::Bye => return anyhow::Ok(()),
                _ => {}
            }
        }
    };

    let result = tokio::select! {
        r = read_loop => r,
        _ = cancel.cancelled() => Ok(()),
    };

    cancel.cancel();
    drop(input); // relâche les touches et boutons encore enfoncés
    drop(out_tx);
    let _ = timeout(Duration::from_secs(2), writer).await;
    result
}

async fn writer_loop(
    mut writer: SecureWriter<OwnedWriteHalf>,
    mut rx: mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // Fin de session côté hôte : on prévient le visualiseur.
                let _ = timeout(Duration::from_secs(1), writer.send(&Control::Bye.encode())).await;
                break;
            }
            msg = rx.recv() => match msg {
                Some(payload) => {
                    if writer.send(&payload).await.is_err() {
                        cancel.cancel();
                        break;
                    }
                }
                None => break,
            },
        }
    }
}
