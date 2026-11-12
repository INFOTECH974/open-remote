//! Côté « hôte » : ce poste peut être contrôlé à distance.
//!
//! Une tâche de fond garde une connexion ouverte avec le relais. Quand un
//! visualiseur demande notre ID, le relais nous prévient et on ouvre une
//! connexion dédiée pour la session (voir [`session`]).

mod capture;
mod input;
mod session;

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail};
use distant_proto::{
    frame::{read_json, write_json},
    relay::{ClientMsg, RelayError, RelayMsg},
    PROTOCOL_VERSION,
};
use tauri::{AppHandle, Manager};
use tokio::{sync::mpsc, time::timeout};

use crate::{
    config::{new_id, Config},
    net::{connect_relay, relay_addr, AbortOnDrop},
    state::{set_host_status, AppState},
};

const PING_INTERVAL: Duration = Duration::from_secs(20);
const MIN_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub fn spawn_service(app: AppHandle) {
    tauri::async_runtime::spawn(service_loop(app));
}

/// Pourquoi la connexion de contrôle s'est arrêtée.
enum Stop {
    /// Problème passager : on réessaie après un délai.
    Retry(String),
    /// Problème qui ne se réglera pas seul : on attend un changement de réglages.
    Halt(String),
}

async fn service_loop(app: AppHandle) {
    let mut restart = app.state::<AppState>().host_restart.subscribe();
    let mut backoff = MIN_BACKOFF;

    loop {
        restart.borrow_and_update();
        let config = app.state::<AppState>().config();

        if !config.accept_incoming {
            set_host_status(&app, "disabled", "Connexions entrantes désactivées.");
            if restart.changed().await.is_err() {
                return;
            }
            continue;
        }

        set_host_status(
            &app,
            "connecting",
            format!("Connexion au relais {}…", relay_addr(&config.relay)),
        );
        let registered = AtomicBool::new(false);
        let stop = tokio::select! {
            stop = run_control(&app, &config, &registered) => stop,
            _ = restart.changed() => {
                backoff = MIN_BACKOFF;
                continue;
            }
        };
        if registered.load(Ordering::Relaxed) {
            backoff = MIN_BACKOFF;
        }

        match stop {
            Stop::Halt(message) => {
                set_host_status(&app, "error", message);
                if restart.changed().await.is_err() {
                    return;
                }
            }
            Stop::Retry(message) => {
                set_host_status(
                    &app,
                    "error",
                    format!("{message} Nouvel essai dans {} s.", backoff.as_secs()),
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = restart.changed() => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

async fn run_control(app: &AppHandle, config: &Config, registered: &AtomicBool) -> Stop {
    match control(app, config, registered).await {
        Ok(stop) => stop,
        Err(e) => Stop::Retry(format!("{e:#}")),
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

async fn control(app: &AppHandle, config: &Config, registered: &AtomicBool) -> anyhow::Result<Stop> {
    let sock = connect_relay(&config.relay).await?;
    let (mut rd, mut wr) = tokio::io::split(sock);

    write_json(
        &mut wr,
        &ClientMsg::Register {
            id: config.id.clone(),
            secret: config.owner_secret.clone(),
            version: PROTOCOL_VERSION,
            key: non_empty(&config.relay_key),
        },
    )
    .await?;

    let reply: RelayMsg = timeout(Duration::from_secs(10), read_json(&mut rd))
        .await
        .map_err(|_| anyhow!("Le relais ne répond pas."))??;
    match reply {
        RelayMsg::Registered => {}
        RelayMsg::Error { code: RelayError::IdTaken } => {
            // Collision d'ID (très improbable) : on en tire un nouveau. Le
            // mot de passe permanent, salé par l'ancien ID, doit être redéfini.
            let state = app.state::<AppState>();
            state.update_config(|c| {
                c.id = new_id();
                c.unattended_secret = None;
            })?;
            return Ok(Stop::Retry(
                "ID déjà pris sur ce relais : nouvel ID attribué, redéfinissez le mot de passe permanent."
                    .into(),
            ));
        }
        RelayMsg::Error {
            code: code @ (RelayError::Unauthorized | RelayError::VersionMismatch | RelayError::Replaced),
        } => return Ok(Stop::Halt(code.message().into())),
        RelayMsg::Error { code } => return Ok(Stop::Retry(code.message().into())),
        other => bail!("Réponse inattendue du relais : {other:?}"),
    }

    registered.store(true, Ordering::Relaxed);
    set_host_status(app, "online", "Prêt : communiquez votre ID et votre mot de passe.");

    // Lecture dans une tâche séparée (lecture non annulable dans un select!).
    let (tx, mut rx) = mpsc::channel::<RelayMsg>(8);
    let _reader = AbortOnDrop(tokio::spawn(async move {
        while let Ok(msg) = read_json::<RelayMsg, _>(&mut rd).await {
            if tx.send(msg).await.is_err() {
                break;
            }
        }
    }));

    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_seen = Instant::now();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                last_seen = Instant::now();
                match msg {
                    Some(RelayMsg::Incoming { token }) => {
                        let app = app.clone();
                        let config = config.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = session::handle_incoming(app, config, token).await {
                                tracing::warn!("connexion entrante : {e:#}");
                            }
                        });
                    }
                    Some(RelayMsg::Error { code: RelayError::Replaced }) => {
                        return Ok(Stop::Halt(RelayError::Replaced.message().into()));
                    }
                    Some(_) => {}
                    None => return Ok(Stop::Retry("Connexion au relais perdue.".into())),
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > PING_INTERVAL * 3 {
                    return Ok(Stop::Retry("Le relais ne répond plus.".into()));
                }
                write_json(&mut wr, &ClientMsg::Ping).await?;
            }
        }
    }
}
