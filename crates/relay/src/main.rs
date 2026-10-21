//! Serveur relais Distant.
//!
//! Il met en relation un visualiseur et un hôte à partir de l'ID de l'hôte,
//! puis recopie les octets entre eux. Le flux étant chiffré de bout en bout,
//! le relais ne voit ni l'écran, ni les frappes, ni le mot de passe.
//!
//! Usage : `distant-relay [--listen 0.0.0.0:21116] [--key SECRET]`
//! (ou variables d'environnement `DISTANT_RELAY_LISTEN` et `DISTANT_RELAY_KEY`).

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use distant_proto::{
    frame::{read_json, write_json},
    is_valid_id,
    relay::{ClientMsg, RelayError, RelayMsg},
    DEFAULT_RELAY_PORT, PROTOCOL_VERSION,
};
use rand::distr::{Alphanumeric, SampleString};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Semaphore},
    time::{sleep_until, timeout},
};
use tracing::{info, warn};

/// Délai pour recevoir le premier message d'un client.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Un hôte silencieux (sans ping) depuis ce délai est considéré déconnecté.
const HOST_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Délai laissé à l'hôte pour ouvrir sa connexion de données.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(15);
/// Tentatives de connexion autorisées par adresse IP et par minute.
const CONNECT_PER_MINUTE: u32 = 20;
/// Nombre maximal de connexions TCP simultanées.
const MAX_CONNECTIONS: usize = 4096;

struct HostEntry {
    conn_id: u64,
    secret: String,
    notify: mpsc::Sender<RelayMsg>,
}

struct State {
    key: Option<String>,
    hosts: Mutex<HashMap<String, HostEntry>>,
    pending: Mutex<HashMap<String, oneshot::Sender<TcpStream>>>,
    attempts: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    next_conn_id: AtomicU64,
}

impl State {
    /// Fenêtre glissante simple : `CONNECT_PER_MINUTE` essais par minute et par IP.
    fn allow_connect(&self, ip: IpAddr) -> bool {
        let mut attempts = self.attempts.lock().unwrap();
        let now = Instant::now();
        if attempts.len() > 10_000 {
            attempts.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
        }
        let entry = attempts.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= CONNECT_PER_MINUTE
    }

    fn check_key(&self, key: &Option<String>) -> bool {
        match &self.key {
            None => true,
            Some(expected) => key.as_deref() == Some(expected.as_str()),
        }
    }
}

struct Args {
    listen: String,
    key: Option<String>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut listen = std::env::var("DISTANT_RELAY_LISTEN")
        .unwrap_or_else(|_| format!("0.0.0.0:{DEFAULT_RELAY_PORT}"));
    let mut key = std::env::var("DISTANT_RELAY_KEY").ok().filter(|k| !k.is_empty());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" | "-l" => listen = args.next().context("--listen attend une adresse")?,
            "--key" | "-k" => key = Some(args.next().context("--key attend une valeur")?),
            "--help" | "-h" => {
                println!("distant-relay [--listen 0.0.0.0:{DEFAULT_RELAY_PORT}] [--key SECRET]");
                std::process::exit(0);
            }
            other => anyhow::bail!("argument inconnu : {other}"),
        }
    }
    Ok(Args { listen, key })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "distant_relay=info".into()),
        )
        .init();

    let args = parse_args()?;
    let listener = TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("impossible d'écouter sur {}", args.listen))?;
    info!(
        "relais Distant v{} à l'écoute sur {} ({})",
        env!("CARGO_PKG_VERSION"),
        args.listen,
        if args.key.is_some() { "clé requise" } else { "ouvert à tous" }
    );

    let state = Arc::new(State {
        key: args.key,
        hosts: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        attempts: Mutex::new(HashMap::new()),
        next_conn_id: AtomicU64::new(1),
    });
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));

    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("accept : {e}");
                continue;
            }
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            warn!("trop de connexions, {peer} refusé");
            continue;
        };
        let _ = sock.set_nodelay(true);
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(sock, peer, state).await {
                tracing::debug!("{peer} : {e:#}");
            }
            drop(permit);
        });
    }
}

async fn send_error(sock: &mut TcpStream, code: RelayError) -> anyhow::Result<()> {
    write_json(sock, &RelayMsg::Error { code }).await
}

async fn handle(mut sock: TcpStream, peer: SocketAddr, state: Arc<State>) -> anyhow::Result<()> {
    let first: ClientMsg = timeout(HELLO_TIMEOUT, read_json(&mut sock))
        .await
        .context("délai dépassé")??;

    match first {
        ClientMsg::Register { id, secret, version, key } => {
            if version != PROTOCOL_VERSION {
                return send_error(&mut sock, RelayError::VersionMismatch).await;
            }
            if !state.check_key(&key) {
                return send_error(&mut sock, RelayError::Unauthorized).await;
            }
            if !is_valid_id(&id) || secret.len() < 16 {
                return send_error(&mut sock, RelayError::BadRequest).await;
            }
            serve_host(sock, peer, state, id, secret).await
        }
        ClientMsg::Connect { id, version, key } => {
            if version != PROTOCOL_VERSION {
                return send_error(&mut sock, RelayError::VersionMismatch).await;
            }
            if !state.check_key(&key) {
                return send_error(&mut sock, RelayError::Unauthorized).await;
            }
            if !state.allow_connect(peer.ip()) {
                return send_error(&mut sock, RelayError::RateLimited).await;
            }
            if !is_valid_id(&id) {
                return send_error(&mut sock, RelayError::Offline).await;
            }
            serve_viewer(sock, peer, state, id).await
        }
        ClientMsg::Accept { token } => {
            let waiting = state.pending.lock().unwrap().remove(&token);
            match waiting {
                // La connexion est confiée à la tâche du visualiseur qui attend.
                Some(tx) => {
                    let _ = tx.send(sock);
                    Ok(())
                }
                None => send_error(&mut sock, RelayError::Timeout).await,
            }
        }
        ClientMsg::Ping => write_json(&mut sock, &RelayMsg::Pong).await,
    }
}

/// Canal de contrôle d'un hôte : reste ouvert tant que l'hôte est en ligne.
async fn serve_host(
    sock: TcpStream,
    peer: SocketAddr,
    state: Arc<State>,
    id: String,
    secret: String,
) -> anyhow::Result<()> {
    let (mut rd, mut wr) = sock.into_split();
    let (notify_tx, mut notify_rx) = mpsc::channel::<RelayMsg>(16);
    let conn_id = state.next_conn_id.fetch_add(1, Ordering::Relaxed);

    let taken = {
        let mut hosts = state.hosts.lock().unwrap();
        match hosts.get(&id) {
            Some(existing) if existing.secret != secret => true,
            existing => {
                // Même secret : c'est la même installation qui se reconnecte
                // (souvent une ancienne connexion morte). On prévient l'ancienne
                // connexion, puis on la remplace, ce qui termine sa tâche.
                if let Some(old) = existing {
                    let _ = old.notify.try_send(RelayMsg::Error { code: RelayError::Replaced });
                }
                hosts.insert(id.clone(), HostEntry { conn_id, secret, notify: notify_tx });
                false
            }
        }
    };
    if taken {
        return write_json(&mut wr, &RelayMsg::Error { code: RelayError::IdTaken }).await;
    }
    info!(%id, %peer, "hôte en ligne");
    write_json(&mut wr, &RelayMsg::Registered).await?;

    // Lecture dans une tâche à part : `read_json` n'est pas annulable sans
    // risque au milieu d'un `select!`.
    let (in_tx, mut in_rx) = mpsc::channel::<ClientMsg>(8);
    let reader = tokio::spawn(async move {
        while let Ok(msg) = read_json::<ClientMsg, _>(&mut rd).await {
            if in_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut deadline = tokio::time::Instant::now() + HOST_IDLE_TIMEOUT;
    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                msg = notify_rx.recv() => match msg {
                    Some(msg) => write_json(&mut wr, &msg).await?,
                    None => break, // remplacé par une nouvelle connexion
                },
                msg = in_rx.recv() => match msg {
                    Some(ClientMsg::Ping) => {
                        deadline = tokio::time::Instant::now() + HOST_IDLE_TIMEOUT;
                        write_json(&mut wr, &RelayMsg::Pong).await?;
                    }
                    Some(_) => {}
                    None => break, // connexion fermée
                },
                _ = sleep_until(deadline) => break,
            }
        }
        Ok(())
    }
    .await;

    reader.abort();
    let mut hosts = state.hosts.lock().unwrap();
    if hosts.get(&id).is_some_and(|h| h.conn_id == conn_id) {
        hosts.remove(&id);
        info!(%id, "hôte hors ligne");
    }
    result
}

/// Connexion d'un visualiseur : on prévient l'hôte, on attend sa connexion de
/// données, puis on relie les deux flux.
async fn serve_viewer(
    mut sock: TcpStream,
    peer: SocketAddr,
    state: Arc<State>,
    id: String,
) -> anyhow::Result<()> {
    let notify = state.hosts.lock().unwrap().get(&id).map(|h| h.notify.clone());
    let Some(notify) = notify else {
        return send_error(&mut sock, RelayError::Offline).await;
    };

    let token = Alphanumeric.sample_string(&mut rand::rng(), 32);
    let (tx, rx) = oneshot::channel();
    state.pending.lock().unwrap().insert(token.clone(), tx);

    if notify.send(RelayMsg::Incoming { token: token.clone() }).await.is_err() {
        state.pending.lock().unwrap().remove(&token);
        return send_error(&mut sock, RelayError::Offline).await;
    }
    drop(notify);

    let mut host_sock = match timeout(ACCEPT_TIMEOUT, rx).await {
        Ok(Ok(s)) => s,
        _ => {
            state.pending.lock().unwrap().remove(&token);
            return send_error(&mut sock, RelayError::Timeout).await;
        }
    };

    write_json(&mut host_sock, &RelayMsg::Paired).await?;
    write_json(&mut sock, &RelayMsg::Paired).await?;
    info!(%id, %peer, "session ouverte");

    let started = Instant::now();
    let result = tokio::io::copy_bidirectional(&mut sock, &mut host_sock).await;
    let (up, down) = result.unwrap_or_default();
    info!(
        %id,
        "session terminée après {}s ({} Ko envoyés, {} Ko reçus)",
        started.elapsed().as_secs(),
        up / 1024,
        down / 1024
    );
    Ok(())
}
