//! Protocole partagé entre l'application Distant et le serveur relais.
//!
//! Vue d'ensemble d'une session :
//!
//! 1. L'hôte garde une connexion « contrôle » ouverte vers le relais (`Register`).
//! 2. Le visualiseur demande l'hôte par son ID (`Connect`). Le relais prévient
//!    l'hôte (`Incoming`), qui ouvre une connexion « données » (`Accept`).
//! 3. Le relais relie les deux connexions et se contente ensuite de recopier
//!    les octets dans les deux sens.
//! 4. Hôte et visualiseur font un échange SPAKE2 avec le mot de passe : ils en
//!    tirent des clés de chiffrement sans que le mot de passe ne circule.
//!    Le relais ne voit que des données chiffrées.

pub mod frame;
pub mod relay;
pub mod secure;
pub mod session;
pub mod transport;

/// Port TCP par défaut du relais.
pub const DEFAULT_RELAY_PORT: u16 = 21116;

/// Version du protocole. Le relais refuse les clients d'une autre version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Un ID Distant fait 9 chiffres, le premier n'étant jamais 0.
pub fn is_valid_id(id: &str) -> bool {
    id.len() == 9 && id.bytes().all(|b| b.is_ascii_digit()) && !id.starts_with('0')
}

/// Retire les espaces et tirets saisis par l'utilisateur (« 123 456 789 »).
pub fn normalize_id(input: &str) -> String {
    input.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Formate un ID pour l'affichage : « 123 456 789 ».
pub fn format_id(id: &str) -> String {
    id.as_bytes()
        .chunks(3)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert!(is_valid_id("123456789"));
        assert!(!is_valid_id("023456789"));
        assert!(!is_valid_id("12345678"));
        assert!(!is_valid_id("12345678a"));
        assert_eq!(normalize_id(" 123 456-789 "), "123456789");
        assert_eq!(format_id("123456789"), "123 456 789");
    }
}
