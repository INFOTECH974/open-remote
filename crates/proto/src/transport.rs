//! Transport entre un poste et le relais.
//!
//! Deux façons de joindre le relais, pour le même protocole au-dessus :
//!
//! - **TCP direct** (`hôte:port`), le plus simple ;
//! - **WebSocket** (`wss://relais.exemple.fr`), pour les réseaux qui ne
//!   laissent sortir que le web : la connexion part vers le port 443 comme
//!   n'importe quelle page HTTPS, et le serveur web qui tient ce port la
//!   transmet au relais.
//!
//! Dans les deux cas le reste du code voit un simple flux d'octets
//! ([`Stream`]) ; le relais reconnaît seul le mode de chaque connexion.

use std::{
    io,
    net::IpAddr,
    pin::Pin,
    task::{ready, Context, Poll},
};

use anyhow::anyhow;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tokio_tungstenite::{
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        Bytes, Error as WsError, Message,
    },
    WebSocketStream,
};

/// Flux d'octets dans les deux sens, quel que soit le transport.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

pub type Stream = Box<dyn Io>;

/// Taille maximale d'un message WebSocket émis : une grande trame (une image
/// complète) part en plusieurs messages, bien en dessous des limites des
/// serveurs web intermédiaires.
const WS_CHUNK: usize = 256 * 1024;

/// Vrai si l'adresse désigne un relais joint en WebSocket.
pub fn is_websocket(addr: &str) -> bool {
    addr.starts_with("ws://") || addr.starts_with("wss://")
}

/// Ouvre une connexion vers le relais : `hôte:port` en TCP, `ws://` ou `wss://` en WebSocket.
pub async fn connect(addr: &str) -> anyhow::Result<Stream> {
    if is_websocket(addr) {
        // rustls demande qu'un fournisseur cryptographique soit choisi une
        // fois pour le processus ; sans effet s'il l'est déjà.
        #[cfg(feature = "tls")]
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (ws, _) = tokio_tungstenite::connect_async_with_config(addr, None, true)
            .await
            .map_err(|e| anyhow!("{}", describe(&e)))?;
        Ok(Box::new(WsIo::new(ws)))
    } else {
        let sock = TcpStream::connect(addr).await?;
        sock.set_nodelay(true)?;
        Ok(Box::new(sock))
    }
}

/// Message lisible pour les échecs de connexion les plus courants.
fn describe(error: &WsError) -> String {
    match error {
        WsError::Http(response) => {
            format!("le serveur a répondu {} au lieu d'ouvrir la connexion", response.status())
        }
        WsError::Io(e) => e.to_string(),
        WsError::Tls(e) => format!("connexion sécurisée refusée ({e})"),
        other => other.to_string(),
    }
}

/// Côté relais : prend en charge une connexion entrante, qu'elle soit en TCP
/// direct ou en WebSocket.
///
/// Renvoie aussi l'adresse du client annoncée par le serveur web
/// (`X-Real-IP` ou `X-Forwarded-For`) quand la connexion arrive en WebSocket :
/// derrière un serveur web, l'adresse TCP est celle du serveur web lui-même.
pub async fn accept(sock: TcpStream) -> io::Result<(Stream, Option<IpAddr>)> {
    // Une requête HTTP commence par « GET » ; le protocole direct par la
    // longueur d'une trame, dont le premier octet est toujours nul.
    let mut first = [0u8; 1];
    if sock.peek(&mut first).await? == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if first[0] != b'G' {
        return Ok((Box::new(sock), None));
    }

    let mut forwarded = None;
    // Le type d'erreur de cette fonction de rappel est imposé par la bibliothèque.
    #[allow(clippy::result_large_err)]
    let ws = tokio_tungstenite::accept_hdr_async(sock, |request: &Request, response: Response| {
        forwarded = ["x-real-ip", "x-forwarded-for"]
            .iter()
            .filter_map(|name| request.headers().get(*name)?.to_str().ok())
            .filter_map(|value| value.split(',').next()?.trim().parse().ok())
            .next();
        Ok::<_, ErrorResponse>(response)
    })
    .await
    .map_err(to_io)?;
    Ok((Box::new(WsIo::new(ws)), forwarded))
}

fn to_io(error: WsError) -> io::Error {
    match error {
        WsError::Io(e) => e,
        other => io::Error::other(other),
    }
}

/// Présente une connexion WebSocket comme un flux d'octets : chaque écriture
/// part en message binaire, les messages reçus sont lus à la suite.
pub struct WsIo<S> {
    ws: WebSocketStream<S>,
    /// Reste du dernier message reçu, pas encore lu.
    pending: Bytes,
}

impl<S> WsIo<S> {
    pub fn new(ws: WebSocketStream<S>) -> Self {
        Self { ws, pending: Bytes::new() }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for WsIo<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.remaining());
                buf.put_slice(&self.pending.split_to(n));
                return Poll::Ready(Ok(()));
            }
            match ready!(self.ws.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(data))) => self.pending = data,
                // Fermeture : fin de flux, comme une connexion TCP fermée.
                Some(Ok(Message::Close(_)))
                | Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed))
                | None => return Poll::Ready(Ok(())),
                // Ping et pong sont traités par la bibliothèque.
                Some(Ok(_)) => {}
                Some(Err(e)) => return Poll::Ready(Err(to_io(e))),
            }
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for WsIo<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        ready!(self.ws.poll_ready_unpin(cx)).map_err(to_io)?;
        let n = buf.len().min(WS_CHUNK);
        self.ws.start_send_unpin(Message::Binary(Bytes::copy_from_slice(&buf[..n]))).map_err(to_io)?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.ws.poll_flush_unpin(cx).map_err(to_io)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match ready!(self.ws.poll_close_unpin(cx)) {
            Ok(()) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(to_io(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;
    use crate::frame::{read_frame, write_frame};

    fn sample(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Relais minimal : renvoie chaque trame reçue, quel que soit le transport.
    async fn echo_server() -> (String, tokio::task::JoinHandle<Vec<Option<IpAddr>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let mut forwarded = Vec::new();
            for _ in 0..2 {
                let (sock, _) = listener.accept().await.unwrap();
                let (mut stream, ip) = accept(sock).await.unwrap();
                forwarded.push(ip);
                while let Ok(frame) = read_frame(&mut stream).await {
                    write_frame(&mut stream, &frame).await.unwrap();
                }
            }
            forwarded
        });
        (format!("127.0.0.1:{port}"), server)
    }

    #[tokio::test]
    async fn same_protocol_over_tcp_and_websocket() {
        let (addr, server) = echo_server().await;
        // Une trame plus grande qu'un message WebSocket : elle est découpée puis recollée.
        let big = sample(WS_CHUNK * 3 + 17);

        for target in [addr.clone(), format!("ws://{addr}/")] {
            let mut stream = connect(&target).await.unwrap();
            for payload in [&b"bonjour"[..], &b""[..], &big[..]] {
                write_frame(&mut stream, payload).await.unwrap();
                assert_eq!(read_frame(&mut stream).await.unwrap(), payload, "{target}");
            }
            stream.shutdown().await.unwrap();
        }
        assert_eq!(server.await.unwrap(), [None, None]);
    }

    #[tokio::test]
    async fn websocket_reports_the_address_given_by_the_proxy() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            accept(sock).await.unwrap().1
        });

        // Ce qu'envoie un serveur web placé devant le relais.
        let mut sock = TcpStream::connect(addr).await.unwrap();
        let request = "GET / HTTP/1.1\r\nHost: relais.exemple.fr\r\nUpgrade: websocket\r\nConnection: upgrade\r\n\
                       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
                       X-Real-IP: 203.0.113.7\r\nX-Forwarded-For: 198.51.100.1, 203.0.113.7\r\n\r\n";
        sock.write_all(request.as_bytes()).await.unwrap();
        let mut reply = [0u8; 12];
        sock.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"HTTP/1.1 101");
        assert_eq!(server.await.unwrap(), Some("203.0.113.7".parse().unwrap()));
    }

    #[tokio::test]
    async fn closed_websocket_reads_as_end_of_stream() {
        let (a, b) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move { WsIo::new(tokio_tungstenite::accept_async(a).await.unwrap()) });
        let (client, _) = tokio_tungstenite::client_async("ws://relais/", b).await.unwrap();
        let mut client = WsIo::new(client);
        let mut server = server.await.unwrap();

        client.write_all(b"abc").await.unwrap();
        client.shutdown().await.unwrap();
        let mut received = Vec::new();
        server.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, b"abc");
    }

    #[test]
    fn websocket_addresses() {
        assert!(is_websocket("wss://relais.exemple.fr"));
        assert!(is_websocket("ws://127.0.0.1:21116/"));
        assert!(!is_websocket("relais.exemple.fr:21116"));
    }
}
