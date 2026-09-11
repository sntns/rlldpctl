//! Black-box tests for `AsyncClient` over a real Unix socket, mirroring
//! `tests/integration.rs` for the sync `Client`. The whole file compiles to
//! nothing when the `tokio` feature is off.

#![cfg(feature = "tokio")]

use std::os::unix::net::UnixListener as StdUnixListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use rlldpctl::{AsyncClient, Error};

const PTR_SIZE: usize = std::mem::size_of::<usize>();
const GET_INTERFACES: i32 = 3;

fn temp_socket_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "rlldpctl-async-test-{name}-{}.sock",
        std::process::id()
    ))
}

fn pad(buf: &mut Vec<u8>) {
    let p = (PTR_SIZE - buf.len() % PTR_SIZE) % PTR_SIZE;
    buf.extend(std::iter::repeat_n(0u8, p));
}

/// Builds one chunk the way real `marshal_serialize_` output does: the
/// declared size is the header's own length plus the content length, not
/// just the content length (confirmed against real `lldpcli` traffic - see
/// `src/wire/cursor.rs`'s `Cursor::chunk_header` doc comment).
fn push_chunk(buf: &mut Vec<u8>, orig: usize, body: &[u8]) {
    pad(buf);
    let header_len = 2 * PTR_SIZE;
    buf.extend_from_slice(&orig.to_ne_bytes());
    buf.extend_from_slice(&(header_len + body.len()).to_ne_bytes());
    buf.extend_from_slice(body);
}

fn push_usize(buf: &mut Vec<u8>, v: usize) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

fn frame(ty: i32, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    out[0..4].copy_from_slice(&ty.to_ne_bytes());
    pad(&mut out);
    push_usize(&mut out, payload.len());
    out.extend_from_slice(payload);
    out
}

async fn read_frame(stream: &mut UnixStream) -> (i32, Vec<u8>) {
    let header_len = 4 + (PTR_SIZE - 4 % PTR_SIZE) % PTR_SIZE + PTR_SIZE;
    let mut header = vec![0u8; header_len];
    stream.read_exact(&mut header).await.unwrap();
    let ty = i32::from_ne_bytes(header[0..4].try_into().unwrap());
    let len_off = header_len - PTR_SIZE;
    let mut len_bytes = [0u8; PTR_SIZE];
    len_bytes.copy_from_slice(&header[len_off..]);
    let len = usize::from_ne_bytes(len_bytes);
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.unwrap();
    (ty, payload)
}

#[tokio::test]
async fn connect_to_missing_socket_is_an_io_error() {
    let err = AsyncClient::connect_to("/nonexistent/rlldpctl-async-test.sock")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}

#[tokio::test]
async fn interfaces_round_trips_over_a_real_socket() {
    let path = temp_socket_path("interfaces");
    let _ = std::fs::remove_file(&path);
    let std_listener = StdUnixListener::bind(&path).unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let listener = tokio::net::UnixListener::from_std(std_listener).unwrap();

    let server = tokio::spawn({
        let path = path.clone();
        async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (ty, req_payload) = read_frame(&mut stream).await;
            assert_eq!(ty, GET_INTERFACES);
            assert!(req_payload.is_empty());

            let mut list_body = Vec::new();
            push_usize(&mut list_body, 2);
            push_usize(&mut list_body, 0);

            let mut if_body = Vec::new();
            push_usize(&mut if_body, 0);
            push_usize(&mut if_body, 0);
            push_usize(&mut if_body, 3);
            push_usize(&mut if_body, 0);

            let mut payload = Vec::new();
            push_chunk(&mut payload, 1, &list_body);
            push_chunk(&mut payload, 2, &if_body);
            push_chunk(&mut payload, 3, b"eth0\0");

            stream
                .write_all(&frame(GET_INTERFACES, &payload))
                .await
                .unwrap();
            let _ = std::fs::remove_file(&path);
        }
    });

    let mut client = AsyncClient::connect_to(&path).await.unwrap();
    let interfaces = client.interfaces().await.unwrap();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].name, "eth0");

    server.await.unwrap();
}
