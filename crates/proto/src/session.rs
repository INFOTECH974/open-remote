//! Messages de session, échangés une fois le canal chiffré établi.
//!
//! Chaque message déchiffré commence par un octet de type :
//! - [`TAG_CONTROL`] : un [`Control`] encodé en JSON ;
//! - [`TAG_FRAME`] : une image (voir [`encode_frame`]), en binaire ;
//! - [`TAG_FILE`] : un morceau de fichier (voir [`encode_file_chunk`]), en binaire.

use serde::{Deserialize, Serialize};

pub const TAG_CONTROL: u8 = 0;
pub const TAG_FRAME: u8 = 1;
pub const TAG_FILE: u8 = 2;

/// Taille d'un morceau de fichier. Assez petit pour qu'un clic ou une frappe
/// n'attende jamais longtemps derrière un morceau en cours d'envoi.
pub const FILE_CHUNK_LEN: usize = 64 * 1024;

/// Statut envoyé en clair par l'hôte juste après la mise en relation.
pub const STATUS_OK: &[u8] = b"OK";
/// L'hôte est déjà contrôlé par quelqu'un d'autre.
pub const STATUS_BUSY: &[u8] = b"BUSY";
/// Trop d'échecs de mot de passe récents : l'hôte refuse temporairement.
pub const STATUS_LOCKED: &[u8] = b"LOCKED";
/// Envoyé en clair par l'hôte quand le premier message chiffré est illisible,
/// c'est-à-dire quand le mot de passe ne correspond pas.
pub const AUTH_FAILED: &[u8] = b"AUTH_FAILED";
/// Le mot de passe ne correspond pas au mot de passe temporaire : l'hôte
/// propose un second échange SPAKE2, cette fois avec son mot de passe permanent.
pub const AUTH_RETRY: &[u8] = b"AUTH_RETRY";

/// Préréglages de qualité d'image.
pub const QUALITY_FAST: u8 = 1;
pub const QUALITY_BALANCED: u8 = 2;
pub const QUALITY_HIGH: u8 = 3;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Control {
    /// Visualiseur → hôte, premier message chiffré.
    Hello { name: String, os: String, version: u32 },
    /// Hôte → visualiseur, réponse à `Hello`.
    Welcome { hostname: String, os: String, monitors: Vec<MonitorInfo> },
    /// Visualiseur → hôte : démarrer l'envoi des images.
    Start { monitor: usize, quality: u8 },
    SetMonitor { index: usize },
    SetQuality { quality: u8 },
    /// Le visualiseur a affiché l'image `seq` (contrôle de flux).
    FrameAck { seq: u32 },
    Input { event: InputEvent },
    /// Début d'un envoi de fichier : les données suivent en messages
    /// [`TAG_FILE`] portant le même `id`, puis `FileEnd`.
    FileStart { id: u32, name: String, size: u64 },
    /// Toutes les données du fichier `id` ont été envoyées.
    FileEnd { id: u32 },
    /// L'expéditeur abandonne le fichier `id`.
    FileCancel { id: u32 },
    /// Destinataire → expéditeur : fichier enregistré sous le nom `name`.
    FileDone { id: u32, name: String },
    /// Destinataire → expéditeur : fichier refusé ou enregistrement impossible.
    FileError { id: u32, message: String },
    /// Maintien de la connexion, ignoré à la réception.
    Ping,
    /// Fin de session demandée.
    Bye,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// Événement clavier/souris envoyé par le visualiseur.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEvent {
    /// Position normalisée (0..1) dans l'écran partagé.
    MouseMove { x: f32, y: f32 },
    MouseButton { button: MouseButton, down: bool },
    /// Défilement en 1/120 de cran, comme `WHEEL_DELTA` sous Windows
    /// (positif = vers le bas / vers la droite).
    Wheel { dx: i32, dy: i32 },
    /// `code` et `key` reprennent `KeyboardEvent.code` et `KeyboardEvent.key`.
    Key { code: String, key: String, down: bool },
    /// Relâche toutes les touches et boutons (perte de focus côté visualiseur).
    ReleaseAll,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Control {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![TAG_CONTROL];
        // La sérialisation d'un enum sans map à clé non-texte ne peut pas échouer.
        serde_json::to_writer(&mut out, self).expect("sérialisation JSON");
        out
    }
}

/// Message de session décodé.
pub enum Incoming<'a> {
    Control(Control),
    /// Contenu binaire de l'image, sans l'octet de type.
    Frame(&'a [u8]),
    /// Morceau du fichier `id`.
    File { id: u32, data: &'a [u8] },
}

pub fn decode(payload: &[u8]) -> anyhow::Result<Incoming<'_>> {
    match payload.split_first() {
        Some((&TAG_CONTROL, rest)) => Ok(Incoming::Control(serde_json::from_slice(rest)?)),
        Some((&TAG_FRAME, rest)) => Ok(Incoming::Frame(rest)),
        Some((&TAG_FILE, rest)) => match rest.split_first_chunk::<4>() {
            Some((id, data)) => Ok(Incoming::File { id: u32::from_be_bytes(*id), data }),
            None => anyhow::bail!("morceau de fichier tronqué"),
        },
        _ => anyhow::bail!("message de session inconnu"),
    }
}

/// Encode un morceau de fichier (avec l'octet [`TAG_FILE`]) : `id u32`, puis les données.
pub fn encode_file_chunk(id: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + data.len());
    out.push(TAG_FILE);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Une tuile d'image encodée en JPEG, positionnée dans l'écran.
pub struct Tile {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub jpeg: Vec<u8>,
}

/// Encode une image (avec l'octet [`TAG_FRAME`]).
///
/// Format : `seq u32 | largeur u16 | hauteur u16 | nb_tuiles u16`, puis pour
/// chaque tuile `x u16 | y u16 | l u16 | h u16 | taille u32 | jpeg`.
/// Tout est en big-endian ; l'interface le relit avec un `DataView`.
pub fn encode_frame(seq: u32, width: u16, height: u16, tiles: &[Tile]) -> Vec<u8> {
    let size: usize = tiles.iter().map(|t| 12 + t.jpeg.len()).sum();
    let mut out = Vec::with_capacity(1 + 10 + size);
    out.push(TAG_FRAME);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&(tiles.len() as u16).to_be_bytes());
    for t in tiles {
        out.extend_from_slice(&t.x.to_be_bytes());
        out.extend_from_slice(&t.y.to_be_bytes());
        out.extend_from_slice(&t.w.to_be_bytes());
        out.extend_from_slice(&t.h.to_be_bytes());
        out.extend_from_slice(&(t.jpeg.len() as u32).to_be_bytes());
        out.extend_from_slice(&t.jpeg);
    }
    out
}

/// Numéro de séquence d'une image (contenu sans l'octet de type).
pub fn frame_seq(frame: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(frame.get(0..4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip() {
        let msg = Control::Input {
            event: InputEvent::Key { code: "KeyA".into(), key: "a".into(), down: true },
        };
        let bytes = msg.encode();
        match decode(&bytes).unwrap() {
            Incoming::Control(c) => assert_eq!(c, msg),
            _ => panic!("type inattendu"),
        }
    }

    #[test]
    fn file_chunk_roundtrip() {
        let bytes = encode_file_chunk(7, b"abc");
        let Incoming::File { id, data } = decode(&bytes).unwrap() else { panic!() };
        assert_eq!((id, data), (7, &b"abc"[..]));
        // Un morceau vide est valide, un en-tête incomplet ne l'est pas.
        assert!(matches!(decode(&encode_file_chunk(7, b"")), Ok(Incoming::File { id: 7, data: [] })));
        assert!(decode(&[TAG_FILE, 0, 0]).is_err());
    }

    #[test]
    fn file_control_json() {
        let msg = Control::FileStart { id: 1, name: "rapport.pdf".into(), size: 5_000_000_000 };
        let json = String::from_utf8(msg.encode()[1..].to_vec()).unwrap();
        assert_eq!(json, r#"{"t":"file_start","id":1,"name":"rapport.pdf","size":5000000000}"#);
    }

    #[test]
    fn input_json_from_ui() {
        // Format envoyé par l'interface JavaScript.
        let ev: InputEvent =
            serde_json::from_str(r#"{"kind":"mouse_button","button":"left","down":true}"#).unwrap();
        assert_eq!(ev, InputEvent::MouseButton { button: MouseButton::Left, down: true });
    }

    #[test]
    fn frame_layout() {
        let tiles = [Tile { x: 128, y: 0, w: 64, h: 32, jpeg: vec![1, 2, 3] }];
        let bytes = encode_frame(42, 1920, 1080, &tiles);
        let Incoming::Frame(body) = decode(&bytes).unwrap() else { panic!() };
        assert_eq!(frame_seq(body), Some(42));
        assert_eq!(&body[4..6], &1920u16.to_be_bytes());
        assert_eq!(&body[8..10], &1u16.to_be_bytes());
        assert_eq!(&body[10..12], &128u16.to_be_bytes());
        assert_eq!(&body[18..22], &3u32.to_be_bytes());
        assert_eq!(&body[22..], &[1, 2, 3]);
    }
}
