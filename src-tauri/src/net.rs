//! Connexion au serveur relais.

use std::time::Duration;

use anyhow::anyhow;
use distant_proto::DEFAULT_RELAY_PORT;
use tokio::{net::TcpStream, time::timeout};

/// Complète l'adresse saisie avec le port par défaut si besoin.
/// Accepte `relais.exemple.fr`, `relais.exemple.fr:9000`, `1.2.3.4`, `[::1]:21116`…
pub fn relay_addr(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return format!("127.0.0.1:{DEFAULT_RELAY_PORT}");
    }
    if s.starts_with('[') {
        if s.contains("]:") {
            s.to_string()
        } else {
            format!("{s}:{DEFAULT_RELAY_PORT}")
        }
    } else {
        match s.matches(':').count() {
            0 => format!("{s}:{DEFAULT_RELAY_PORT}"),
            1 => s.to_string(),
            _ => format!("[{s}]:{DEFAULT_RELAY_PORT}"), // IPv6 sans crochets
        }
    }
}

pub async fn connect_relay(input: &str) -> anyhow::Result<TcpStream> {
    let addr = relay_addr(input);
    let sock = timeout(Duration::from_secs(8), TcpStream::connect(&addr))
        .await
        .map_err(|_| anyhow!("Relais injoignable ({addr}) : délai dépassé."))?
        .map_err(|e| anyhow!("Relais injoignable ({addr}) : {e}"))?;
    sock.set_nodelay(true)?;
    Ok(sock)
}

/// Annule la tâche quand la valeur est détruite.
pub struct AbortOnDrop<T>(pub tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(relay_addr("relais.fr"), "relais.fr:21116");
        assert_eq!(relay_addr(" relais.fr:9000 "), "relais.fr:9000");
        assert_eq!(relay_addr("::1"), "[::1]:21116");
        assert_eq!(relay_addr("[::1]"), "[::1]:21116");
        assert_eq!(relay_addr("[::1]:80"), "[::1]:80");
    }
}
