//! Messages échangés avec le serveur relais (JSON, avant la mise en relation).

use serde::{Deserialize, Serialize};

/// Premier message envoyé par un client au relais.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// L'hôte s'annonce et garde cette connexion ouverte (canal de contrôle).
    /// `secret` prouve au relais que l'ID appartient bien à cette installation :
    /// il permet de remplacer une ancienne connexion restée « fantôme ».
    Register {
        id: String,
        secret: String,
        version: u32,
        #[serde(default)]
        key: Option<String>,
    },
    /// Le visualiseur demande à joindre l'hôte `id`.
    Connect {
        id: String,
        version: u32,
        #[serde(default)]
        key: Option<String>,
    },
    /// L'hôte ouvre la connexion de données correspondant à `token`.
    Accept { token: String },
    /// Maintien de la connexion de contrôle.
    Ping,
}

/// Réponses et notifications du relais.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayMsg {
    Registered,
    /// Un visualiseur veut se connecter : l'hôte doit ouvrir une connexion `Accept`.
    Incoming { token: String },
    /// Les deux côtés sont reliés : tout ce qui suit est relayé tel quel.
    Paired,
    Pong,
    Error { code: RelayError },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RelayError {
    /// L'ID est déjà enregistré par une autre installation.
    IdTaken,
    /// Une autre instance de la même installation a pris le relais
    /// (application ouverte deux fois avec le même profil).
    Replaced,
    /// Aucun hôte en ligne avec cet ID.
    Offline,
    /// L'hôte n'a pas répondu à temps.
    Timeout,
    /// Clé du relais absente ou incorrecte.
    Unauthorized,
    /// Version de protocole différente de celle du relais.
    VersionMismatch,
    /// Trop de tentatives depuis cette adresse.
    RateLimited,
    BadRequest,
}

impl RelayError {
    /// Message lisible par l'utilisateur.
    pub fn message(self) -> &'static str {
        match self {
            RelayError::IdTaken => "Cet ID est déjà utilisé par un autre poste.",
            RelayError::Replaced => "Distant est déjà ouvert ailleurs avec cet ID.",
            RelayError::Offline => "Le poste distant est hors ligne ou l'ID est inconnu.",
            RelayError::Timeout => "Le poste distant n'a pas répondu à temps.",
            RelayError::Unauthorized => "Clé du relais incorrecte.",
            RelayError::VersionMismatch => "Version incompatible avec le serveur relais.",
            RelayError::RateLimited => "Trop de tentatives, réessayez dans une minute.",
            RelayError::BadRequest => "Requête refusée par le relais.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape() {
        let msg = ClientMsg::Connect { id: "123456789".into(), version: 1, key: None };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"t":"connect","id":"123456789","version":1,"key":null}"#);
        let err = RelayMsg::Error { code: RelayError::Offline };
        assert_eq!(serde_json::to_string(&err).unwrap(), r#"{"t":"error","code":"offline"}"#);
    }
}
