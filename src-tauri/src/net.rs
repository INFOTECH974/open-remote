//! Connexion au serveur relais.

use std::time::Duration;

use anyhow::anyhow;
use distant_proto::{
    transport::{self, Stream},
    DEFAULT_RELAY_PORT,
};
use tokio::time::timeout;

/// Délai pour joindre le relais, négociation TLS et WebSocket comprises.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Complète l'adresse saisie avec le port par défaut si besoin.
/// Accepte `relais.exemple.fr`, `relais.exemple.fr:9000`, `1.2.3.4`, `[::1]:21116`…
/// et, pour passer par le port 443 comme une page web, `wss://relais.exemple.fr`
/// (`https://` est compris de la même façon).
pub fn relay_addr(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return format!("127.0.0.1:{DEFAULT_RELAY_PORT}");
    }
    for (typed, scheme) in [("wss://", "wss://"), ("ws://", "ws://"), ("https://", "wss://"), ("http://", "ws://")] {
        if s.len() >= typed.len() && s[..typed.len()].eq_ignore_ascii_case(typed) {
            return format!("{scheme}{}", &s[typed.len()..]);
        }
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

pub async fn connect_relay(input: &str) -> anyhow::Result<Stream> {
    let addr = relay_addr(input);
    timeout(CONNECT_TIMEOUT, transport::connect(&addr))
        .await
        .map_err(|_| anyhow!("Relais injoignable ({addr}) : délai dépassé."))?
        .map_err(|e| anyhow!("Relais injoignable ({addr}) : {e}"))
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
        // Par le port 443, comme une page web.
        assert_eq!(relay_addr("wss://relais.fr"), "wss://relais.fr");
        assert_eq!(relay_addr(" https://relais.fr/distant "), "wss://relais.fr/distant");
        assert_eq!(relay_addr("WS://127.0.0.1:21116"), "ws://127.0.0.1:21116");
    }
}
