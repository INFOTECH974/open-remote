//! Canal chiffré de bout en bout entre l'hôte et le visualiseur.
//!
//! - **PBKDF2-SHA256** transforme le mot de passe en un secret de 32 octets
//!   (salé par l'ID de l'hôte). L'hôte ne stocke que ce secret, jamais le
//!   mot de passe permanent en clair.
//! - **SPAKE2** (échange de clés authentifié par mot de passe) : les deux côtés
//!   dérivent une clé commune à partir de ce secret, sans jamais l'envoyer.
//!   Un relais malveillant ne peut pas tester des mots de passe hors ligne :
//!   chaque essai nécessite une vraie tentative de connexion.
//! - **HKDF-SHA256** dérive une clé par sens de communication.
//! - **ChaCha20-Poly1305** chiffre et authentifie chaque trame, avec un
//!   compteur comme nonce (jamais réutilisé, car une clé par sens).

use std::io;

use anyhow::anyhow;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::frame::{read_frame, write_frame};
use crate::PROTOCOL_VERSION;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Host,
    Viewer,
}

/// On ignore les espaces (le mot de passe temporaire s'affiche « abcd efgh »).
/// La casse compte : les mots de passe permanents sont choisis par l'utilisateur.
pub fn normalize_password(password: &str) -> String {
    password.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Nombre d'itérations PBKDF2 : environ 50 ms par connexion, mais autant de
/// travail pour chaque essai d'un attaquant qui aurait volé le secret stocké.
const PBKDF2_ROUNDS: u32 = 100_000;

/// Secret de 32 octets dérivé du mot de passe, salé par l'ID de l'hôte.
/// C'est ce secret, et non le mot de passe, qui alimente SPAKE2.
pub fn derive_secret(password: &str, host_id: &str) -> [u8; 32] {
    let password = normalize_password(password);
    let salt = format!("distant/v{PROTOCOL_VERSION}/{host_id}");
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt.as_bytes(), PBKDF2_ROUNDS, &mut out);
    out
}

pub struct Sealer {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

pub struct Opener {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(n)
}

impl Sealer {
    fn new(key: [u8; 32]) -> Self {
        Self { cipher: ChaCha20Poly1305::new(Key::from_slice(&key)), counter: 0 }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let n = nonce(self.counter);
        self.counter += 1;
        // Ne peut échouer que pour des messages de plus de 256 Go.
        self.cipher.encrypt(&n, plaintext).expect("chiffrement")
    }
}

impl Opener {
    fn new(key: [u8; 32]) -> Self {
        Self { cipher: ChaCha20Poly1305::new(Key::from_slice(&key)), counter: 0 }
    }

    pub fn open(&mut self, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let n = nonce(self.counter);
        let plaintext = self
            .cipher
            .decrypt(&n, ciphertext)
            .map_err(|_| anyhow!("message chiffré invalide"))?;
        self.counter += 1;
        Ok(plaintext)
    }
}

/// État intermédiaire de l'échange SPAKE2.
pub struct Handshake {
    state: Spake2<Ed25519Group>,
    role: Role,
}

impl Handshake {
    /// Démarre l'échange avec le secret issu de [`derive_secret`] ; renvoie
    /// le message à envoyer à l'autre côté.
    pub fn start(secret: &[u8; 32], host_id: &str, role: Role) -> (Self, Vec<u8>) {
        let identity = format!("distant/v{PROTOCOL_VERSION}/{host_id}");
        let (state, outbound) = Spake2::<Ed25519Group>::start_symmetric(
            &Password::new(secret),
            &Identity::new(identity.as_bytes()),
        );
        (Self { state, role }, outbound)
    }

    /// Termine l'échange avec le message reçu de l'autre côté.
    ///
    /// Réussit même si les mots de passe diffèrent : l'erreur n'apparaît qu'au
    /// premier déchiffrement, qui échoue alors.
    pub fn finish(self, inbound: &[u8]) -> anyhow::Result<(Sealer, Opener)> {
        let shared = self
            .state
            .finish(inbound)
            .map_err(|e| anyhow!("échange de clés invalide : {e:?}"))?;
        let hk = Hkdf::<Sha256>::new(None, &shared);
        let mut host_to_viewer = [0u8; 32];
        let mut viewer_to_host = [0u8; 32];
        hk.expand(b"distant host->viewer", &mut host_to_viewer)
            .map_err(|_| anyhow!("dérivation de clé"))?;
        hk.expand(b"distant viewer->host", &mut viewer_to_host)
            .map_err(|_| anyhow!("dérivation de clé"))?;
        Ok(match self.role {
            Role::Host => (Sealer::new(host_to_viewer), Opener::new(viewer_to_host)),
            Role::Viewer => (Sealer::new(viewer_to_host), Opener::new(host_to_viewer)),
        })
    }
}

/// Échange SPAKE2 complet sur un flux (les deux côtés écrivent puis lisent).
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    secret: &[u8; 32],
    host_id: &str,
    role: Role,
) -> anyhow::Result<(Sealer, Opener)> {
    let (hs, outbound) = Handshake::start(secret, host_id, role);
    write_frame(stream, &outbound).await?;
    let inbound = read_frame(stream).await?;
    hs.finish(&inbound)
}

pub struct SecureWriter<W> {
    inner: W,
    sealer: Sealer,
}

impl<W: AsyncWrite + Unpin> SecureWriter<W> {
    pub fn new(inner: W, sealer: Sealer) -> Self {
        Self { inner, sealer }
    }

    pub async fn send(&mut self, plaintext: &[u8]) -> io::Result<()> {
        let ciphertext = self.sealer.seal(plaintext);
        write_frame(&mut self.inner, &ciphertext).await
    }
}

pub struct SecureReader<R> {
    inner: R,
    opener: Opener,
}

impl<R: AsyncRead + Unpin> SecureReader<R> {
    pub fn new(inner: R, opener: Opener) -> Self {
        Self { inner, opener }
    }

    pub async fn recv(&mut self) -> anyhow::Result<Vec<u8>> {
        let ciphertext = read_frame(&mut self.inner).await?;
        self.opener.open(&ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pair(host_pw: &'static str, viewer_pw: &'static str) -> bool {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let host_secret = derive_secret(host_pw, "123456789");
        let viewer_secret = derive_secret(viewer_pw, "123456789");
        let host =
            tokio::spawn(async move { handshake(&mut a, &host_secret, "123456789", Role::Host).await });
        let viewer = handshake(&mut b, &viewer_secret, "123456789", Role::Viewer).await.unwrap();
        let host = host.await.unwrap().unwrap();
        let (mut v_seal, _) = viewer;
        let (_, mut h_open) = host;
        let ct = v_seal.seal(b"hello");
        h_open.open(&ct).map(|pt| pt == b"hello").unwrap_or(false)
    }

    #[tokio::test]
    async fn same_password_decrypts() {
        assert!(pair("abcd2345", " abcd 2345 ").await);
    }

    #[tokio::test]
    async fn wrong_password_fails() {
        assert!(!pair("abcd2345", "abcd2346").await);
        assert!(!pair("abcd2345", "ABCD2345").await, "la casse compte");
    }

    #[test]
    fn secret_is_salted_by_host() {
        assert_ne!(derive_secret("pw", "123456789"), derive_secret("pw", "987654321"));
    }

    #[test]
    fn nonces_advance() {
        let secret = [7u8; 32];
        let (h, hm) = Handshake::start(&secret, "123456789", Role::Host);
        let (v, vm) = Handshake::start(&secret, "123456789", Role::Viewer);
        let (mut hs, mut ho) = h.finish(&vm).unwrap();
        let (mut vs, mut vo) = v.finish(&hm).unwrap();
        let a = hs.seal(b"x");
        let b = hs.seal(b"x");
        assert_ne!(a, b, "deux messages identiques doivent donner des chiffrés différents");
        assert_eq!(vo.open(&a).unwrap(), b"x");
        assert_eq!(vo.open(&b).unwrap(), b"x");
        // Rejouer un message déjà reçu doit échouer.
        assert!(vo.open(&a).is_err());
        assert_eq!(ho.open(&vs.seal(b"y")).unwrap(), b"y");
    }
}
