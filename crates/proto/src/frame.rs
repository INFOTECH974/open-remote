//! Découpage du flux TCP en trames : `u32` big-endian (longueur) + contenu.

use std::io;

use serde::{de::DeserializeOwned, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Taille maximale d'une trame (une image 4K en JPEG tient largement dedans).
pub const MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let len = r.read_u32().await? as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "trame trop grande"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "trame trop grande"));
    }
    // Un seul write pour éviter d'envoyer l'en-tête dans un paquet à part.
    let mut buf = Vec::with_capacity(4 + data.len());
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
    w.write_all(&buf).await?;
    w.flush().await
}

pub async fn read_json<T: DeserializeOwned, R: AsyncRead + Unpin>(r: &mut R) -> anyhow::Result<T> {
    let buf = read_frame(r).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn write_json<T: Serialize, W: AsyncWrite + Unpin>(w: &mut W, msg: &T) -> anyhow::Result<()> {
    let buf = serde_json::to_vec(msg)?;
    write_frame(w, &buf).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_frame(&mut a, b"bonjour").await.unwrap();
        write_frame(&mut a, b"").await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), b"bonjour");
        assert_eq!(read_frame(&mut b).await.unwrap(), b"");
    }
}
