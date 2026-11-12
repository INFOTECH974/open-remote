//! Client de test : se connecte à un hôte Distant via le relais, reçoit des
//! images pendant quelques secondes et affiche des statistiques.
//! N'envoie aucun événement clavier/souris. Avec `--send`, envoie un fichier
//! à l'hôte (il arrive dans son dossier Téléchargements) au lieu de recevoir des images.
//!
//! ```text
//! cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe [secondes]
//! cargo run -p distant-proto --example probe -- 127.0.0.1:21116 555123456 mot-de-passe --send fichier
//! cargo run -p distant-proto --example probe -- --secret 555123456 mot-de-passe
//! cargo run -p distant-proto --features tls --example probe -- wss://relais.exemple.fr 555123456 mot-de-passe
//! ```
//!
//! La clé du relais, s'il en exige une, se donne par `DISTANT_RELAY_KEY`.

use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use distant_proto::{
    frame::{read_frame, read_json, write_frame, write_json},
    relay::{ClientMsg, RelayMsg},
    secure::{derive_secret, handshake, Role},
    session::{
        decode, encode_file_chunk, frame_seq, Control, Incoming, AUTH_FAILED, AUTH_RETRY,
        FILE_CHUNK_LEN, STATUS_OK,
    },
    transport, PROTOCOL_VERSION,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let send_path = match args.iter().position(|a| a == "--send") {
        Some(i) if i + 1 < args.len() => {
            let path = args.remove(i + 1);
            args.remove(i);
            Some(path)
        }
        Some(_) => bail!("--send attend un chemin de fichier"),
        None => None,
    };
    if args.first().map(String::as_str) == Some("--secret") {
        let secret = derive_secret(&args[2], &args[1]);
        println!("{}", secret.iter().map(|b| format!("{b:02x}")).collect::<String>());
        return Ok(());
    }
    let [relay, id, password, rest @ ..] = args.as_slice() else {
        bail!("usage : probe <relais> <id> <mot de passe> [secondes]");
    };
    let seconds: u64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(5);

    let mut sock = transport::connect(relay).await.context("relais injoignable")?;
    let key = std::env::var("DISTANT_RELAY_KEY").ok().filter(|k| !k.is_empty());
    write_json(&mut sock, &ClientMsg::Connect { id: id.clone(), version: PROTOCOL_VERSION, key }).await?;
    match read_json::<RelayMsg, _>(&mut sock).await? {
        RelayMsg::Paired => println!("relais : mis en relation"),
        other => bail!("relais : {other:?}"),
    }
    let status = read_frame(&mut sock).await?;
    if status != STATUS_OK {
        bail!("hôte : {}", String::from_utf8_lossy(&status));
    }

    let secret = derive_secret(password, id);
    let hello = Control::Hello { name: "probe".into(), os: "test".into(), version: PROTOCOL_VERSION }.encode();
    let mut session = None;
    for round in 1..=3 {
        let (mut sealer, mut opener) = handshake(&mut sock, &secret, id, Role::Viewer).await?;
        write_frame(&mut sock, &sealer.seal(&hello)).await?;
        let reply = read_frame(&mut sock).await?;
        if reply == AUTH_RETRY {
            println!("auth : essai {round} refusé, l'hôte propose un autre secret");
            continue;
        }
        if reply == AUTH_FAILED {
            println!("auth : MOT DE PASSE REFUSÉ");
            return Ok(());
        }
        let plaintext = opener.open(&reply)?;
        session = Some((sealer, opener, plaintext));
        break;
    }
    let Some((mut sealer, mut opener, welcome)) = session else { bail!("pas de réponse") };
    let Incoming::Control(Control::Welcome { hostname, os, monitors, warnings, .. }) = decode(&welcome)? else {
        bail!("Welcome attendu");
    };
    println!("auth : OK — hôte « {hostname} » ({os}), {} écran(s)", monitors.len());
    if !warnings.is_empty() {
        println!("  autorisations manquantes sur l'hôte : {}", warnings.join(", "));
    }
    for (i, m) in monitors.iter().enumerate() {
        println!("  écran {i} : {} {}×{}{}", m.name, m.width, m.height, if m.primary { " (principal)" } else { "" });
    }
    let primary = monitors.iter().position(|m| m.primary).unwrap_or(0);

    let send = |sealer: &mut distant_proto::secure::Sealer, c: Control| sealer.seal(&c.encode());

    if let Some(path) = send_path {
        let content = std::fs::read(&path).with_context(|| format!("lecture de {path}"))?;
        let name = std::path::Path::new(&path)
            .file_name()
            .context("nom de fichier")?
            .to_string_lossy()
            .into_owned();
        let started = Instant::now();
        let start = Control::FileStart { id: 1, name, size: content.len() as u64 };
        write_frame(&mut sock, &send(&mut sealer, start)).await?;
        for chunk in content.chunks(FILE_CHUNK_LEN) {
            write_frame(&mut sock, &sealer.seal(&encode_file_chunk(1, chunk))).await?;
        }
        write_frame(&mut sock, &send(&mut sealer, Control::FileEnd { id: 1 })).await?;
        // L'hôte confirme une fois le fichier enregistré.
        loop {
            let ct = tokio::time::timeout(Duration::from_secs(30), read_frame(&mut sock))
                .await
                .context("pas de confirmation de l'hôte")??;
            match decode(&opener.open(&ct)?)? {
                Incoming::Control(Control::FileDone { name, .. }) => {
                    println!(
                        "fichier : {} octets envoyés en {} ms, enregistré sous « {name} »",
                        content.len(),
                        started.elapsed().as_millis()
                    );
                    break;
                }
                Incoming::Control(Control::FileError { message, .. }) => {
                    println!("fichier : REFUSÉ — {message}");
                    break;
                }
                _ => {}
            }
        }
        write_frame(&mut sock, &send(&mut sealer, Control::Bye)).await?;
        return Ok(());
    }
    let start = Control::Start { monitor: primary, quality: 2, stats: true };
    write_frame(&mut sock, &send(&mut sealer, start)).await?;

    let started = Instant::now();
    let mut frames = 0u32;
    let mut tiles_total = 0u64;
    let mut bytes = 0u64;
    let mut first: Option<(Duration, usize, u16, u16, u16)> = None;
    let mut switched = false;
    let mut after_switch: Option<(u16, usize)> = None;

    while started.elapsed() < Duration::from_secs(seconds) {
        let ct = match tokio::time::timeout(Duration::from_secs(2), read_frame(&mut sock)).await {
            Ok(r) => r?,
            Err(_) => continue, // écran figé : rien à envoyer
        };
        let msg = opener.open(&ct)?;
        let body = match decode(&msg)? {
            Incoming::Frame(body) => body,
            Incoming::Control(Control::Stats { frames, capture, encode, wait }) => {
                println!("  hôte : {frames} i/s, capture {capture} ms, encodage {encode} ms, attente {wait} ms");
                continue;
            }
            _ => continue,
        };
        let seq = frame_seq(body).context("image tronquée")?;
        let w = u16::from_be_bytes([body[4], body[5]]);
        let h = u16::from_be_bytes([body[6], body[7]]);
        let n = u16::from_be_bytes([body[8], body[9]]);
        // Vérifie chaque tuile : bornes et marqueur JPEG.
        let mut off = 10;
        for _ in 0..n {
            let (x, y, tw, th) = (
                u16::from_be_bytes([body[off], body[off + 1]]),
                u16::from_be_bytes([body[off + 2], body[off + 3]]),
                u16::from_be_bytes([body[off + 4], body[off + 5]]),
                u16::from_be_bytes([body[off + 6], body[off + 7]]),
            );
            let len = u32::from_be_bytes(body[off + 8..off + 12].try_into()?) as usize;
            off += 12;
            if x + tw > w || y + th > h || body[off..off + 2] != [0xFF, 0xD8] {
                bail!("tuile invalide ({x},{y},{tw}×{th})");
            }
            off += len;
        }
        frames += 1;
        tiles_total += n as u64;
        bytes += body.len() as u64;
        if first.is_none() {
            first = Some((started.elapsed(), body.len(), w, h, n));
        }
        if switched && after_switch.is_none() {
            after_switch = Some((n, body.len()));
        }
        write_frame(&mut sock, &send(&mut sealer, Control::FrameAck { seq })).await?;

        // À mi-parcours : passage en qualité haute, qui doit renvoyer une image complète.
        if !switched && started.elapsed() > Duration::from_secs(seconds / 2) {
            write_frame(&mut sock, &send(&mut sealer, Control::SetQuality { quality: 3 })).await?;
            switched = true;
        }
    }
    write_frame(&mut sock, &send(&mut sealer, Control::Bye)).await?;

    let elapsed = started.elapsed().as_secs_f64();
    if let Some((latency, size, w, h, n)) = first {
        println!(
            "1re image : {} ms après Start, {w}×{h}, {n} bande(s), {} Ko",
            latency.as_millis(),
            size / 1024
        );
    }
    if let Some((n, size)) = after_switch {
        println!("après passage en qualité haute : {n} bande(s), {} Ko", size / 1024);
    }
    println!(
        "{frames} images en {elapsed:.1} s ({:.1} i/s), {tiles_total} tuiles, {:.0} Ko/s en moyenne",
        frames as f64 / elapsed,
        bytes as f64 / 1024.0 / elapsed
    );
    Ok(())
}
