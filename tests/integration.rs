//! Black-box tests driving [`rlldpctl::Client`] over a real Unix socket
//! against a hand-rolled fake `lldpd`, using only the crate's public API.
//!
//! The wire bytes here are built independently of `src/wire`'s own encoder
//! (this file has no access to the crate's private internals from outside
//! the crate anyway), so a passing test is evidence the transport and
//! decoder genuinely agree on the framing - not just that the crate is
//! internally self-consistent.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use rlldpctl::{Client, Error};

const PTR_SIZE: usize = std::mem::size_of::<usize>();

/// Message types from upstream `src/ctl.h`'s `enum hmsg_type` that this test
/// needs to speak.
const GET_INTERFACES: i32 = 3;

fn temp_socket_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rlldpctl-test-{name}-{}.sock", std::process::id()))
}

/// Appends alignment padding so the next write starts at a pointer-aligned
/// offset within `buf` - mirrors `marshal.c`'s use of
/// `ALIGNOF(struct marshal_serialized)`.
fn pad(buf: &mut Vec<u8>) {
    let p = (PTR_SIZE - buf.len() % PTR_SIZE) % PTR_SIZE;
    buf.extend(std::iter::repeat_n(0u8, p));
}

fn push_chunk(buf: &mut Vec<u8>, orig: usize, body: &[u8]) {
    pad(buf);
    buf.extend_from_slice(&orig.to_ne_bytes());
    buf.extend_from_slice(&body.len().to_ne_bytes());
    buf.extend_from_slice(body);
}

fn push_usize(buf: &mut Vec<u8>, v: usize) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

/// Frames `payload` behind lldpd's outer `struct hmsg_header` (see
/// `src/ctl.h`): a 4-byte type, padded up to pointer width, then a
/// pointer-width length.
fn frame(ty: i32, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    out[0..4].copy_from_slice(&ty.to_ne_bytes());
    pad(&mut out);
    push_usize(&mut out, payload.len());
    out.extend_from_slice(payload);
    out
}

fn read_frame(stream: &mut UnixStream) -> (i32, Vec<u8>) {
    // Mirrors `frame()`'s layout: a 4-byte type, padded up to pointer width,
    // then a pointer-width length.
    let header_len = 4 + (PTR_SIZE - 4 % PTR_SIZE) % PTR_SIZE + PTR_SIZE;
    let mut header = vec![0u8; header_len];
    stream.read_exact(&mut header).unwrap();
    let ty = i32::from_ne_bytes(header[0..4].try_into().unwrap());
    let len_off = header_len - PTR_SIZE;
    let mut len_bytes = [0u8; PTR_SIZE];
    len_bytes.copy_from_slice(&header[len_off..]);
    let len = usize::from_ne_bytes(len_bytes);
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).unwrap();
    (ty, payload)
}

#[test]
fn connect_to_missing_socket_is_an_io_error() {
    let err = Client::connect_to("/nonexistent/rlldpctl-test.sock").unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}

#[test]
fn interfaces_round_trips_over_a_real_socket() {
    let path = temp_socket_path("interfaces");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();

    let server = std::thread::spawn({
        let path = path.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (ty, req_payload) = read_frame(&mut stream);
            assert_eq!(ty, GET_INTERFACES);
            assert!(req_payload.is_empty());

            // RawInterfaceList { tqh_first: 2, tqh_last: 0 }
            let mut list_body = Vec::new();
            push_usize(&mut list_body, 2); // tqh_first -> chunk with orig=2 follows
            push_usize(&mut list_body, 0);

            // RawInterface { next_tqe_next: 0, next_tqe_prev: 0, name: 3, alias: 0 }
            let mut if_body = Vec::new();
            push_usize(&mut if_body, 0);
            push_usize(&mut if_body, 0);
            push_usize(&mut if_body, 3); // name -> chunk with orig=3 follows
            push_usize(&mut if_body, 0);

            let mut payload = Vec::new();
            push_chunk(&mut payload, 1, &list_body);
            push_chunk(&mut payload, 2, &if_body);
            push_chunk(&mut payload, 3, b"eth0\0");

            stream.write_all(&frame(GET_INTERFACES, &payload)).unwrap();
            let _ = std::fs::remove_file(&path);
        }
    });

    let mut client = Client::connect_to(&path).unwrap();
    let interfaces = client.interfaces().unwrap();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].name, "eth0");
    assert_eq!(interfaces[0].alias, None);

    server.join().unwrap();
}

#[test]
fn interface_not_found_surfaces_as_request_rejected() {
    let path = temp_socket_path("rejected");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();

    let server = std::thread::spawn({
        let path = path.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_ty, _payload) = read_frame(&mut stream);
            // lldpd replies with type NONE (0) and an empty payload when it
            // can't honor a request - see `client_handle_get_interface`
            // upstream.
            stream.write_all(&frame(0, &[])).unwrap();
            let _ = std::fs::remove_file(&path);
        }
    });

    let mut client = Client::connect_to(&path).unwrap();
    let err = client.interface("does-not-exist").unwrap_err();
    assert!(matches!(err, Error::RequestRejected));

    server.join().unwrap();
}
