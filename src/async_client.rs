//! Tokio-based equivalents of [`crate::Client`]/[`crate::Subscription`],
//! gated behind the `tokio` feature.
//!
//! This shares [`crate::wire`] (the pure, no-I/O encode/decode logic) with
//! the sync API; only the transport - reading and writing the Unix socket -
//! is different, so the two implementations can't silently disagree about
//! what's actually on the wire.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::client::DEFAULT_SOCKET_PATH;
use crate::error::{Error, Result};
use crate::model::{Interface, InterfaceDetails, NeighborChange};
use crate::transport::{decode_header, encode_header, header_len, HmsgType, HMSG_MAX_SIZE};
use crate::wire;

/// Async equivalent of [`crate::Client`]. See its docs for the request/
/// response model this follows - only the I/O here is non-blocking.
#[derive(Debug)]
pub struct AsyncClient {
    stream: UnixStream,
    socket_path: PathBuf,
}

impl AsyncClient {
    /// Connects to lldpd at the default socket path
    /// ([`crate::DEFAULT_SOCKET_PATH`]).
    pub async fn connect() -> Result<Self> {
        Self::connect_to(DEFAULT_SOCKET_PATH).await
    }

    /// Connects to lldpd listening on the Unix socket at `path`.
    pub async fn connect_to<P: AsRef<Path>>(path: P) -> Result<Self> {
        let stream = UnixStream::connect(path.as_ref()).await?;
        Ok(Self {
            stream,
            socket_path: path.as_ref().to_path_buf(),
        })
    }

    /// Lists every interface lldpd knows about (`GET_INTERFACES`).
    pub async fn interfaces(&mut self) -> Result<Vec<Interface>> {
        let payload = request(&mut self.stream, HmsgType::GetInterfaces, &[]).await?;
        wire::decode_interfaces(&payload)
    }

    /// Fetches full state for one interface (`GET_INTERFACE`), including
    /// every neighbor lldpd has discovered on it.
    pub async fn interface(&mut self, ifname: &str) -> Result<InterfaceDetails> {
        let request_payload = wire::encode_interface_name_request(ifname);
        let payload = request(&mut self.stream, HmsgType::GetInterface, &request_payload).await?;
        wire::decode_hardware(&payload)
    }

    /// Convenience helper: [`AsyncClient::interfaces`] followed by
    /// [`AsyncClient::interface`] on each of them, skipping any that
    /// disappear between the two calls rather than failing the whole batch.
    pub async fn all_interfaces(&mut self) -> Result<Vec<InterfaceDetails>> {
        let mut out = Vec::new();
        for iface in self.interfaces().await? {
            match self.interface(&iface.name).await {
                Ok(details) => out.push(details),
                Err(Error::RequestRejected) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Path this client connected to, mostly useful for logging.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Subscribes to live neighbor-change notifications (`SUBSCRIBE`),
    /// returning an [`AsyncSubscription`]. Consumes the client for the same
    /// reason [`crate::Client::subscribe`] does - see its docs.
    pub async fn subscribe(mut self) -> Result<AsyncSubscription> {
        request(&mut self.stream, HmsgType::Subscribe, &[]).await?;
        Ok(AsyncSubscription {
            stream: self.stream,
        })
    }
}

/// Async equivalent of [`crate::Subscription`]. There is no `Stream` impl in
/// v1 - `next_change` is a plain `async fn`, in the same spirit as
/// `tokio::sync::mpsc::Receiver::recv`. Wrap it with
/// `futures::stream::unfold` yourself if you want a `Stream`.
#[derive(Debug)]
pub struct AsyncSubscription {
    stream: UnixStream,
}

impl AsyncSubscription {
    /// Waits for the next neighbor-change notification. Returns `Ok(None)`
    /// once lldpd closes the connection.
    pub async fn next_change(&mut self) -> Result<Option<NeighborChange>> {
        match recv_notification(&mut self.stream).await {
            Ok(payload) => Ok(Some(wire::decode_neighbor_change(&payload)?)),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

async fn request(stream: &mut UnixStream, ty: HmsgType, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = encode_header(ty, payload.len());
    out.extend_from_slice(payload);
    stream.write_all(&out).await?;
    read_message(stream, ty).await
}

async fn recv_notification(stream: &mut UnixStream) -> Result<Vec<u8>> {
    read_message(stream, HmsgType::Notification).await
}

/// Async twin of `transport::read_message` - same framing, same rules about
/// draining the payload before deciding what to return, same `NONE`-means-
/// rejected special case. Kept separate rather than made generic over
/// `Read`/`AsyncRead` because there's no `dyn`-friendly way to share async
/// I/O code between blocking and non-blocking callers without pulling in
/// another abstraction crate for what is, in the end, three lines of logic.
async fn read_message(stream: &mut UnixStream, expected: HmsgType) -> Result<Vec<u8>> {
    let mut header = vec![0u8; header_len()];
    stream.read_exact(&mut header).await?;
    let (got_ty, len) = decode_header(&header);
    if len > HMSG_MAX_SIZE {
        return Err(Error::Protocol(format!(
            "reply announces a {len}-byte payload, larger than the {HMSG_MAX_SIZE}-byte cap"
        )));
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await?;

    if got_ty == HmsgType::None as i32 {
        return Err(Error::RequestRejected);
    }
    if !expected.matches(got_ty) {
        return Err(Error::UnexpectedMessageType {
            expected: expected as i32 as u32,
            got: got_ty as u32,
        });
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_over_socketpair_roundtrips() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let handle = tokio::spawn(async move {
            let mut header = vec![0u8; header_len()];
            server.read_exact(&mut header).await.unwrap();
            let (ty, len) = decode_header(&header);
            assert_eq!(ty, HmsgType::GetInterfaces as i32);
            assert_eq!(len, 0);
            let mut reply = encode_header(HmsgType::GetInterfaces, 3);
            reply.extend_from_slice(b"abc");
            server.write_all(&reply).await.unwrap();
        });
        let reply = request(&mut client, HmsgType::GetInterfaces, &[])
            .await
            .unwrap();
        assert_eq!(reply, b"abc");
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn interface_not_found_surfaces_as_request_rejected() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let handle = tokio::spawn(async move {
            let mut header = vec![0u8; header_len()];
            server.read_exact(&mut header).await.unwrap();
            server
                .write_all(&encode_header(HmsgType::None, 0))
                .await
                .unwrap();
        });
        let err = request(&mut client, HmsgType::GetInterface, b"nope\0")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::RequestRejected));
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn next_change_ends_cleanly_when_lldpd_closes_the_connection() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        let mut sub = AsyncSubscription { stream: client };
        assert_eq!(sub.next_change().await.unwrap(), None);
    }
}
