//! Transfert de fichiers pendant une session.
//!
//! L'expéditeur annonce chaque fichier (`FileStart`), envoie son contenu par
//! morceaux, puis `FileEnd`. Le destinataire écrit dans un fichier `.part` du
//! dossier de destination et ne lui donne son nom définitif qu'une fois le
//! contenu complet : un transfert interrompu ne laisse rien derrière lui.

use std::{
    collections::HashMap,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context};
use distant_proto::session::{encode_file_chunk, Control, FILE_CHUNK_LEN};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

/// Fichiers en cours de réception en même temps (l'expéditeur les envoie un par un).
const MAX_OPEN: usize = 4;
/// Longueur maximale d'un nom de fichier, en octets : laisse la place au
/// suffixe « (12) » et à l'extension `.part` sous la limite de 255 des systèmes de fichiers.
const MAX_NAME_BYTES: usize = 200;
const PART_SUFFIX: &str = ".part";

// ── Réception ───────────────────────────────────────────────────────────

struct Receiving {
    file: File,
    part: PathBuf,
    name: String,
    size: u64,
    received: u64,
}

/// Fichiers reçus pendant une session, enregistrés dans `dir`.
pub struct Receiver {
    dir: PathBuf,
    open: HashMap<u32, Receiving>,
}

impl Receiver {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, open: HashMap::new() }
    }

    /// Dossier de destination.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Prépare la réception du fichier `id`.
    pub async fn start(&mut self, id: u32, name: &str, size: u64) -> anyhow::Result<()> {
        if self.open.contains_key(&id) || self.open.len() >= MAX_OPEN {
            bail!("Trop de fichiers envoyés en même temps.");
        }
        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("Dossier {} inaccessible", self.dir.display()))?;

        let wanted = sanitize_name(name);
        // Le nom définitif est réservé par la création du fichier `.part`.
        for n in 0..1000 {
            let name = numbered(&wanted, n);
            if tokio::fs::try_exists(self.dir.join(&name)).await.unwrap_or(true) {
                continue;
            }
            let part = self.dir.join(format!("{name}{PART_SUFFIX}"));
            match OpenOptions::new().write(true).create_new(true).open(&part).await {
                Ok(file) => {
                    self.open.insert(id, Receiving { file, part, name, size, received: 0 });
                    return Ok(());
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(anyhow!("Enregistrement impossible : {e}")),
            }
        }
        bail!("Enregistrement impossible : trop de fichiers portent déjà ce nom.")
    }

    /// Écrit un morceau du fichier `id`. Les morceaux d'un fichier inconnu
    /// (refusé ou annulé entre-temps) sont ignorés.
    pub async fn chunk(&mut self, id: u32, data: &[u8]) -> anyhow::Result<()> {
        let Some(entry) = self.open.get_mut(&id) else { return Ok(()) };
        let result = if entry.received + data.len() as u64 > entry.size {
            Err(anyhow!("Le fichier reçu dépasse la taille annoncée."))
        } else {
            entry.file.write_all(data).await.map_err(|e| anyhow!("Enregistrement impossible : {e}"))
        };
        match result {
            Ok(()) => {
                entry.received += data.len() as u64;
                Ok(())
            }
            Err(e) => {
                self.discard(id).await;
                Err(e)
            }
        }
    }

    /// Termine le fichier `id` et renvoie son chemin définitif (`None` si le
    /// fichier est inconnu).
    pub async fn finish(&mut self, id: u32) -> anyhow::Result<Option<PathBuf>> {
        let Some(mut entry) = self.open.remove(&id) else { return Ok(None) };
        let result = async {
            if entry.received != entry.size {
                bail!("Fichier incomplet : {} octets reçus sur {}.", entry.received, entry.size);
            }
            entry.file.flush().await?;
            // Fermé avant d'être renommé (obligatoire sous Windows).
            drop(entry.file.into_std().await);

            // Le nom a pu être pris pendant le transfert : on ne remplace jamais un fichier existant.
            let mut target = self.dir.join(&entry.name);
            let mut n = 0;
            while tokio::fs::try_exists(&target).await.unwrap_or(true) {
                n += 1;
                if n >= 1000 {
                    bail!("trop de fichiers portent déjà ce nom");
                }
                target = self.dir.join(numbered(&entry.name, n));
            }
            tokio::fs::rename(&entry.part, &target).await?;
            Ok(target)
        }
        .await;
        match result {
            Ok(target) => Ok(Some(target)),
            Err(e) => {
                let _ = tokio::fs::remove_file(&entry.part).await;
                Err(anyhow!("Enregistrement impossible : {e}"))
            }
        }
    }

    /// Abandonne le fichier `id` (annulé par l'expéditeur).
    pub async fn cancel(&mut self, id: u32) {
        self.discard(id).await;
    }

    async fn discard(&mut self, id: u32) {
        if let Some(entry) = self.open.remove(&id) {
            drop(entry.file.into_std().await);
            let _ = tokio::fs::remove_file(&entry.part).await;
        }
    }
}

impl Drop for Receiver {
    /// Fin de session : les fichiers inachevés sont supprimés.
    fn drop(&mut self) {
        for (_, entry) in self.open.drain() {
            let part = entry.part;
            match entry.file.try_into_std() {
                Ok(file) => {
                    drop(file);
                    let _ = std::fs::remove_file(&part);
                }
                // Une écriture est encore en cours : on supprime dès qu'elle s'achève.
                Err(file) => {
                    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                        runtime.spawn(async move {
                            drop(file.into_std().await);
                            let _ = tokio::fs::remove_file(&part).await;
                        });
                    }
                }
            }
        }
    }
}

/// Nom de fichier sûr à partir du nom annoncé par l'autre poste : jamais de
/// dossier, de caractère interdit ni de nom réservé par Windows.
pub fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect();
    // Windows refuse les noms qui finissent par un point ou une espace.
    let mut clean = clean.trim().trim_end_matches(['.', ' ']).to_string();

    if clean.len() > MAX_NAME_BYTES {
        let (stem, ext) = split_ext(&clean);
        let (stem, ext) = if ext.len() > 20 { (clean.as_str(), "") } else { (stem, ext) };
        let mut keep = (MAX_NAME_BYTES - ext.len()).min(stem.len());
        while !stem.is_char_boundary(keep) {
            keep -= 1;
        }
        clean = format!("{}{ext}", stem[..keep].trim_end());
    }
    if clean.is_empty() {
        return "fichier".into();
    }

    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let device = clean.split('.').next().unwrap_or_default().trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(device)) {
        clean.insert(0, '_');
    }
    clean
}

/// Sépare le nom de son extension (point compris). Un fichier caché comme
/// `.profile` n'a pas d'extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// `rapport.pdf`, puis `rapport (1).pdf`, `rapport (2).pdf`…
fn numbered(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    format!("{stem} ({n}){ext}")
}

// ── Envoi ───────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// Tout le contenu est parti ; le destinataire confirmera l'enregistrement.
    Sent,
    Cancelled,
}

/// Nom et taille d'un fichier à envoyer. Refuse les dossiers.
pub async fn describe(path: &Path) -> anyhow::Result<(String, u64)> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| anyhow!("Fichier illisible : {e}"))?;
    if meta.is_dir() {
        bail!("Les dossiers ne peuvent pas être envoyés : compressez-le d'abord.");
    }
    if !meta.is_file() {
        bail!("Ce n'est pas un fichier.");
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow!("Nom de fichier invalide."))?;
    Ok((name, meta.len()))
}

/// Envoie le fichier `path` dans `out`, un morceau à la fois : `out` étant
/// borné, la lecture suit le débit du réseau.
///
/// `cancelled` est consulté entre deux morceaux ; `progress` reçoit le nombre
/// d'octets envoyés.
pub async fn send_file(
    id: u32,
    path: &Path,
    out: &mpsc::Sender<Vec<u8>>,
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(u64),
) -> anyhow::Result<SendOutcome> {
    let closed = |_| anyhow!("Session terminée.");
    let (name, size) = describe(path).await?;
    let mut file = File::open(path).await.map_err(|e| anyhow!("Fichier illisible : {e}"))?;
    out.send(Control::FileStart { id, name, size }.encode()).await.map_err(closed)?;

    let mut buf = vec![0u8; FILE_CHUNK_LEN];
    let mut sent = 0u64;
    let failure = loop {
        if cancelled() {
            break None;
        }
        if sent == size {
            out.send(Control::FileEnd { id }.encode()).await.map_err(closed)?;
            return Ok(SendOutcome::Sent);
        }
        // Jamais plus que la taille annoncée, même si le fichier grossit entre-temps.
        let want = (size - sent).min(FILE_CHUNK_LEN as u64) as usize;
        match file.read(&mut buf[..want]).await {
            Ok(0) => break Some(anyhow!("Le fichier a été modifié pendant l'envoi.")),
            Ok(n) => {
                out.send(encode_file_chunk(id, &buf[..n])).await.map_err(closed)?;
                sent += n as u64;
                progress(sent);
            }
            Err(e) => break Some(anyhow!("Lecture impossible : {e}")),
        }
    };
    // Le destinataire supprime ce qu'il a déjà reçu.
    out.send(Control::FileCancel { id }.encode()).await.map_err(closed)?;
    match failure {
        Some(e) => Err(e),
        None => Ok(SendOutcome::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use distant_proto::session::{decode, Incoming};

    use super::*;

    /// Dossier temporaire supprimé en fin de test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("distant-test-{:016x}", rand::random::<u64>()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Applique au destinataire les messages produits par l'expéditeur, comme
    /// le fait la session. Renvoie les chemins enregistrés et les erreurs.
    async fn receive(rx: &mut mpsc::Receiver<Vec<u8>>, receiver: &mut Receiver) -> (Vec<PathBuf>, Vec<String>) {
        let (mut saved, mut errors) = (Vec::new(), Vec::new());
        while let Some(msg) = rx.recv().await {
            let result = match decode(&msg).unwrap() {
                Incoming::File { id, data } => receiver.chunk(id, data).await,
                Incoming::Control(Control::FileStart { id, name, size }) => receiver.start(id, &name, size).await,
                Incoming::Control(Control::FileEnd { id }) => {
                    receiver.finish(id).await.map(|path| saved.extend(path))
                }
                Incoming::Control(Control::FileCancel { id }) => {
                    receiver.cancel(id).await;
                    Ok(())
                }
                _ => panic!("message inattendu"),
            };
            if let Err(e) = result {
                errors.push(e.to_string());
            }
        }
        (saved, errors)
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("rapport.pdf"), "rapport.pdf");
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name(r"C:\Windows\System32\cmd.exe"), "cmd.exe");
        assert_eq!(sanitize_name("a<b>:c|d?.txt"), "abcd.txt");
        assert_eq!(sanitize_name("fin. . "), "fin");
        assert_eq!(sanitize_name(".."), "fichier");
        assert_eq!(sanitize_name("dossier/"), "fichier");
        assert_eq!(sanitize_name("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_name("COM1"), "_COM1");
        assert_eq!(sanitize_name("communication.txt"), "communication.txt");
        assert_eq!(sanitize_name(".profile"), ".profile");
        assert_eq!(sanitize_name("été\u{0}\n.txt"), "été.txt");
    }

    #[test]
    fn long_names_keep_their_extension() {
        let name = sanitize_name(&format!("{}.tar", "é".repeat(300)));
        assert!(name.len() <= MAX_NAME_BYTES);
        assert!(name.ends_with(".tar"));
        assert!(name.starts_with("ééé"));
    }

    #[test]
    fn numbering_goes_before_the_extension() {
        assert_eq!(numbered("rapport.pdf", 0), "rapport.pdf");
        assert_eq!(numbered("rapport.pdf", 2), "rapport (2).pdf");
        assert_eq!(numbered("archive.tar.gz", 1), "archive.tar (1).gz");
        assert_eq!(numbered("LISEZMOI", 1), "LISEZMOI (1)");
        assert_eq!(numbered(".profile", 1), ".profile (1)");
    }

    #[tokio::test]
    async fn file_is_sent_and_saved() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        // Plusieurs morceaux, le dernier incomplet.
        let content = sample(FILE_CHUNK_LEN * 2 + 1234);
        let path = src.0.join("photo.jpg");
        std::fs::write(&path, &content).unwrap();

        let (tx, mut rx) = mpsc::channel(2);
        let mut last = 0;
        let sender = async {
            let outcome = send_file(1, &path, &tx, || false, |sent| last = sent).await;
            drop(tx);
            outcome
        };
        let mut receiver = Receiver::new(dst.0.clone());
        let (outcome, (saved, errors)) = tokio::join!(sender, receive(&mut rx, &mut receiver));

        assert_eq!(outcome.unwrap(), SendOutcome::Sent);
        assert_eq!(last, content.len() as u64);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(saved, [dst.0.join("photo.jpg")]);
        assert_eq!(std::fs::read(&saved[0]).unwrap(), content);
        assert_eq!(dst.names(), ["photo.jpg"]);
    }

    #[tokio::test]
    async fn existing_files_are_never_replaced() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        std::fs::write(dst.0.join("notes.txt"), b"ancien").unwrap();
        let path = src.0.join("notes.txt");
        std::fs::write(&path, b"nouveau").unwrap();

        let mut receiver = Receiver::new(dst.0.clone());
        for id in 1..=2 {
            let (tx, mut rx) = mpsc::channel(2);
            let sender = async {
                send_file(id, &path, &tx, || false, |_| {}).await.unwrap();
                drop(tx);
            };
            tokio::join!(sender, receive(&mut rx, &mut receiver));
        }
        assert_eq!(dst.names(), ["notes (1).txt", "notes (2).txt", "notes.txt"]);
        assert_eq!(std::fs::read(dst.0.join("notes.txt")).unwrap(), b"ancien");
        assert_eq!(std::fs::read(dst.0.join("notes (2).txt")).unwrap(), b"nouveau");
    }

    #[tokio::test]
    async fn empty_file_is_saved() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let path = src.0.join("vide.txt");
        std::fs::write(&path, b"").unwrap();
        let (tx, mut rx) = mpsc::channel(2);
        let mut receiver = Receiver::new(dst.0.clone());
        let sender = async {
            send_file(1, &path, &tx, || false, |_| {}).await.unwrap();
            drop(tx);
        };
        tokio::join!(sender, receive(&mut rx, &mut receiver));
        assert_eq!(std::fs::read(dst.0.join("vide.txt")).unwrap(), b"");
    }

    #[tokio::test]
    async fn cancelled_transfer_leaves_nothing() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let path = src.0.join("gros.bin");
        std::fs::write(&path, sample(FILE_CHUNK_LEN * 4)).unwrap();

        let (tx, mut rx) = mpsc::channel(2);
        let stop = AtomicBool::new(false);
        let sender = async {
            // Annulé après le deuxième morceau.
            let outcome =
                send_file(1, &path, &tx, || stop.load(Ordering::Relaxed), |sent| {
                    stop.store(sent >= 2 * FILE_CHUNK_LEN as u64, Ordering::Relaxed)
                })
                .await;
            drop(tx);
            outcome
        };
        let mut receiver = Receiver::new(dst.0.clone());
        let (outcome, (saved, errors)) = tokio::join!(sender, receive(&mut rx, &mut receiver));

        assert_eq!(outcome.unwrap(), SendOutcome::Cancelled);
        assert!(saved.is_empty() && errors.is_empty());
        assert!(dst.names().is_empty(), "{:?}", dst.names());
    }

    #[tokio::test]
    async fn oversized_and_truncated_files_are_rejected() {
        let dst = TempDir::new();
        let mut receiver = Receiver::new(dst.0.clone());

        // Plus de données qu'annoncé.
        receiver.start(1, "a.bin", 4).await.unwrap();
        assert!(receiver.chunk(1, b"12345").await.is_err());
        // La suite du même fichier est ignorée sans erreur.
        receiver.chunk(1, b"6").await.unwrap();
        assert_eq!(receiver.finish(1).await.unwrap(), None);

        // Moins de données qu'annoncé.
        receiver.start(2, "b.bin", 4).await.unwrap();
        receiver.chunk(2, b"12").await.unwrap();
        assert!(receiver.finish(2).await.is_err());

        assert!(dst.names().is_empty(), "{:?}", dst.names());
    }

    #[tokio::test]
    async fn unfinished_files_are_removed_when_the_session_ends() {
        let dst = TempDir::new();
        let mut receiver = Receiver::new(dst.0.clone());
        receiver.start(1, "a.bin", 10).await.unwrap();
        receiver.chunk(1, b"123").await.unwrap();
        assert_eq!(dst.names(), ["a.bin.part"]);
        // Le nom en cours de réception est réservé.
        receiver.start(2, "a.bin", 10).await.unwrap();
        assert_eq!(dst.names(), ["a (1).bin.part", "a.bin.part"]);
        drop(receiver);
        // La suppression peut attendre la fin d'une écriture en cours.
        for _ in 0..50 {
            if dst.names().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(dst.names().is_empty(), "{:?}", dst.names());
    }

    #[tokio::test]
    async fn folders_are_refused() {
        let src = TempDir::new();
        let (tx, _rx) = mpsc::channel(2);
        assert!(send_file(1, &src.0, &tx, || false, |_| {}).await.is_err());
        assert!(send_file(1, &src.0.join("absent.txt"), &tx, || false, |_| {}).await.is_err());
    }
}
