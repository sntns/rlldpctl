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
const SET_PORT: i32 = 8;

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

#[test]
fn set_port_description_sends_the_real_wire_bytes_and_succeeds_on_an_empty_ack() {
    let path = temp_socket_path("set-port-description");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();

    // Captured via `strace -x -s 4096 -e trace=write -f lldpcli configure
    // ports eth0 lldp portdescription 'test-desc-XYZ'` against a real `lldpd
    // 1.0.22` - the payload half of the same fixture
    // `wire::encode_tests::set_port_description_request_matches_a_real_lldpd_capture`
    // asserts against directly; this test instead proves the *transport*
    // (`Client::set_port_description`) sends exactly those bytes end to end,
    // and that a real successful ack (`SET_PORT` type, empty payload -
    // confirmed by the same capture) is treated as success.
    let expected_payload: &[u8] = b"\x01\x00\x00\x00\x00\x00\x00\x00\x96\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x15\x00\x00\x00\x00\x00\x00\x00\x65\x74\x68\x30\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x1e\x00\x00\x00\x00\x00\x00\x00\x74\x65\x73\x74\x2d\x64\x65\x73\x63\x2d\x58\x59\x5a\x00";

    let server = std::thread::spawn({
        let path = path.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (ty, req_payload) = read_frame(&mut stream);
            assert_eq!(ty, SET_PORT);
            assert_eq!(req_payload, expected_payload);

            // A successful SET_PORT acks with its own type and an empty
            // payload (confirmed by the same real capture).
            stream.write_all(&frame(SET_PORT, &[])).unwrap();
            let _ = std::fs::remove_file(&path);
        }
    });

    let mut client = Client::connect_to(&path).unwrap();
    client
        .set_port_description("eth0", "test-desc-XYZ")
        .unwrap();

    server.join().unwrap();
}

#[test]
fn set_port_description_for_an_unknown_interface_surfaces_as_request_rejected() {
    let path = temp_socket_path("set-port-description-rejected");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();

    let server = std::thread::spawn({
        let path = path.clone();
        move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_ty, _payload) = read_frame(&mut stream);
            stream.write_all(&frame(0, &[])).unwrap();
            let _ = std::fs::remove_file(&path);
        }
    });

    let mut client = Client::connect_to(&path).unwrap();
    let err = client
        .set_port_description("does-not-exist", "x")
        .unwrap_err();
    assert!(matches!(err, Error::RequestRejected));

    server.join().unwrap();
}
