//! Réglages persistants (fichier JSON dans le dossier de configuration de l'app).

use std::path::Path;

use distant_proto::{is_valid_id, session::QUALITY_BALANCED, DEFAULT_RELAY_PORT};
use rand::{
    distr::{Alphanumeric, SampleString},
    Rng,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// ID à 9 chiffres de ce poste, fixe d'un lancement à l'autre.
    pub id: String,
    /// Secret propre à cette installation : prouve au relais que l'ID nous appartient.
    pub owner_secret: String,
    /// Adresse du relais, `hôte` ou `hôte:port`.
    pub relay: String,
    /// Clé du relais, si le relais en exige une.
    pub relay_key: String,
    /// Accepter les demandes de prise en main.
    pub accept_incoming: bool,
    /// Qualité d'image demandée en tant que visualiseur.
    pub quality: u8,
    /// Accès sans surveillance : secret dérivé du mot de passe permanent (hex).
    /// Le mot de passe lui-même n'est jamais enregistré.
    pub unattended_secret: Option<String>,
    /// Partager le presse-papiers avec les postes que l'on contrôle.
    pub clipboard_sync: bool,
    /// Installer les mises à jour sans rien demander, dès que le poste est libre.
    pub auto_update: bool,
    /// Posé juste avant une mise à jour installée fenêtre fermée : le
    /// lancement suivant reste réduit dans la zone de notification.
    pub start_hidden_once: bool,
    /// Postes déjà contactés, du plus récent au plus ancien.
    pub machines: Vec<Machine>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub os: String,
    /// Dernière connexion réussie (secondes Unix).
    pub last: u64,
    /// Secret dérivé du mot de passe, si l'utilisateur a choisi de le mémoriser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            id: new_id(),
            owner_secret: new_secret(),
            relay: format!("127.0.0.1:{DEFAULT_RELAY_PORT}"),
            relay_key: String::new(),
            accept_incoming: true,
            quality: QUALITY_BALANCED,
            unattended_secret: None,
            clipboard_sync: true,
            // Sous macOS, une application sans certificat perd ses autorisations
            // (enregistrement de l'écran, accessibilité) à chaque nouvelle version :
            // un Mac sans surveillance ne doit pas se mettre à jour tout seul.
            auto_update: !cfg!(target_os = "macos"),
            start_hidden_once: false,
            machines: Vec::new(),
        }
    }
}

impl Config {
    /// Charge les réglages, ou les crée au premier lancement.
    pub fn load(path: &Path) -> Self {
        let mut config: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        if !is_valid_id(&config.id) {
            config.id = new_id();
        }
        if config.owner_secret.len() < 16 {
            config.owner_secret = new_secret();
        }
        config.machines.retain(|m| is_valid_id(&m.id));
        if let Err(e) = config.save(path) {
            tracing::warn!("impossible d'enregistrer les réglages : {e:#}");
        }
        config
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Écriture dans un fichier temporaire puis renommage : pas de fichier
        // à moitié écrit si l'application est coupée au mauvais moment.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Oublie le mot de passe mémorisé d'un poste, qui reste dans la liste.
    pub fn forget_secret(&mut self, id: &str) {
        if let Some(machine) = self.machines.iter_mut().find(|m| m.id == id) {
            machine.secret = None;
        }
    }

    pub fn machine(&self, id: &str) -> Option<&Machine> {
        self.machines.iter().find(|m| m.id == id)
    }

    /// Enregistre une connexion réussie et place le poste en tête de liste.
    /// `secret` : `Some(Some(_))` mémorise, `Some(None)` oublie, `None` garde l'existant.
    pub fn record_machine(&mut self, id: &str, name: &str, os: &str, secret: Option<Option<String>>) {
        let previous = self.machines.iter().position(|m| m.id == id).map(|i| self.machines.remove(i));
        let secret = match secret {
            Some(s) => s,
            None => previous.and_then(|m| m.secret),
        };
        self.machines.insert(
            0,
            Machine { id: id.into(), name: name.into(), os: os.into(), last: unix_now(), secret },
        );
        self.machines.truncate(20);
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn new_id() -> String {
    let mut rng = rand::rng();
    format!("{}{:08}", rng.random_range(1..=9), rng.random_range(0..100_000_000u32))
}

fn new_secret() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 32)
}

/// Mot de passe de session : 8 caractères sans ambiguïté (pas de 0/o, 1/l/i).
pub fn new_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..8).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_values_are_valid() {
        for _ in 0..1000 {
            assert!(is_valid_id(&new_id()));
            let pw = new_password();
            assert_eq!(pw.len(), 8);
            assert_eq!(distant_proto::secure::normalize_password(&pw), pw);
        }
    }

    #[test]
    fn hex_roundtrip() {
        let bytes = [0xABu8; 32];
        assert_eq!(from_hex(&to_hex(&bytes)), Some(bytes));
        assert_eq!(from_hex("zz"), None);
    }

    #[test]
    fn forgetting_a_secret_keeps_the_machine() {
        let mut c = Config::default();
        c.record_machine("123456789", "Mac", "macos", Some(Some("ab".into())));
        c.forget_secret("123456789");
        c.forget_secret("987654321"); // poste inconnu : sans effet
        assert_eq!(c.machines.len(), 1);
        assert!(c.machine("123456789").unwrap().secret.is_none());
    }

    #[test]
    fn machines_keep_saved_secret() {
        let mut c = Config::default();
        c.record_machine("123456789", "PC", "windows", Some(Some("s".into())));
        c.record_machine("123456789", "PC", "windows", None);
        assert_eq!(c.machine("123456789").unwrap().secret.as_deref(), Some("s"));
        c.record_machine("123456789", "PC", "windows", Some(None));
        assert!(c.machine("123456789").unwrap().secret.is_none());
        assert_eq!(c.machines.len(), 1);
    }
}
