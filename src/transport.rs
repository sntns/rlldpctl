//! Framing for lldpd's control protocol (`src/ctl.c` / `src/ctl.h` upstream).
//!
//! A message on the wire is a `struct hmsg_header { enum hmsg_type type; size_t
//! len; }` (raw, native C layout - so its size depends on the pointer width of
//! the machine lldpd was built for) followed by `len` bytes of payload. This
//! module only deals with that outer framing; the payload itself is handled by
//! [`crate::wire`].

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use crate::error::{Error, Result};

/// Mirrors `enum hmsg_type` from `src/ctl.h`. Only the variants this crate
/// actually speaks are given names; the rest exist so a stray reply can still
/// be reported with its real numeric value instead of panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HmsgType {
    None = 0,
    GetInterfaces = 3,
    GetInterface = 6,
    SetPort = 8,
    Subscribe = 9,
    Notification = 10,
}

impl HmsgType {
    pub(crate) fn matches(self, got: i32) -> bool {
        self as i32 == got
    }
}

/// Same constant as upstream's `HMSG_MAX_SIZE` (`1 << 19`, i.e. 512 KiB) - the
/// daemon refuses to build (and we refuse to trust) anything larger.
pub(crate) const HMSG_MAX_SIZE: usize = 1 << 19;

/// Width of `size_t`/pointers on this machine. lldpd's control protocol
/// memcpy's its C structures as-is, so this crate is only correct when built
/// for the same pointer width as the lldpd it talks to (see the crate-level
/// docs).
const PTR_SIZE: usize = std::mem::size_of::<usize>();

/// Size in bytes of `struct hmsg_header` on this target: a 4-byte `enum`,
/// padded up to `PTR_SIZE`, followed by a `size_t`.
pub(crate) fn header_len() -> usize {
    align_up(4, PTR_SIZE) + PTR_SIZE
}

/// Round `len` up to the next multiple of `align` (mirrors the padding
/// computation in `marshal.c`'s use of `ALIGNOF(struct marshal_serialized)`).
pub(crate) fn align_up(len: usize, align: usize) -> usize {
    len + (align - len % align) % align
}

pub(crate) fn encode_header(ty: HmsgType, payload_len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; header_len()];
    buf[0..4].copy_from_slice(&(ty as i32).to_ne_bytes());
    // Bytes [4..align_up(4, PTR_SIZE)) are C struct padding, left at zero.
    let len_off = align_up(4, PTR_SIZE);
    buf[len_off..len_off + PTR_SIZE]
        .copy_from_slice(&(payload_len as u64).to_ne_bytes()[..PTR_SIZE]);
    buf
}

pub(crate) fn decode_header(buf: &[u8]) -> (i32, usize) {
    let ty = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
    let len_off = align_up(4, PTR_SIZE);
    let mut len_bytes = [0u8; 8];
    len_bytes[..PTR_SIZE].copy_from_slice(&buf[len_off..len_off + PTR_SIZE]);
    (ty, u64::from_ne_bytes(len_bytes) as usize)
}

/// Sends one request (message type + already-serialized payload) and waits
/// for the matching reply, returning its raw payload bytes.
///
/// This is a simple request/response round-trip: lldpd's control protocol is
/// synchronous over a single connection, so this crate opens a fresh
/// connection per request rather than pipelining (matching how `lldpcli`
/// itself uses `liblldpctl`).
pub(crate) fn request(stream: &mut UnixStream, ty: HmsgType, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = encode_header(ty, payload.len());
    out.extend_from_slice(payload);
    stream.write_all(&out)?;
    read_message(stream, ty)
}

/// Blocks until lldpd pushes the next `NOTIFICATION` on an already-subscribed
/// connection (see [`crate::Subscription`]) - unlike [`request`], nothing is
/// written first: the daemon sends these unprompted.
pub(crate) fn recv_notification(stream: &mut UnixStream) -> Result<Vec<u8>> {
    read_message(stream, HmsgType::Notification)
}

/// Reads one framed message and returns its payload, checking it against
/// `expected` the same way for both a request's reply and an unprompted
/// notification.
fn read_message(stream: &mut UnixStream, expected: HmsgType) -> Result<Vec<u8>> {
    let mut header = vec![0u8; header_len()];
    stream.read_exact(&mut header)?;
    let (got_ty, len) = decode_header(&header);
    if len > HMSG_MAX_SIZE {
        return Err(Error::Protocol(format!(
            "reply announces a {len}-byte payload, larger than the {HMSG_MAX_SIZE}-byte cap"
        )));
    }
    // Read the payload before deciding what to return: even a rejected or
    // mismatched reply's declared length must be drained so the stream stays
    // framed correctly (this crate opens a fresh connection per request, but
    // being sloppy here would still leave a corrupt read on that connection).
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload)?;

    if got_ty == HmsgType::None as i32 {
        // lldpd's `client_handle_get_interface` (and friends) reply with
        // type NONE and an empty payload to say "no" (e.g. interface not
        // found) rather than answering the request type - see
        // `src/daemon/client.c` upstream.
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

    #[test]
    fn header_len_is_two_pointers_wide() {
        // On every target we support (32- and 64-bit), the padded header
        // happens to be exactly 2 * PTR_SIZE (see module docs on transport).
        assert_eq!(header_len(), 2 * PTR_SIZE);
    }

    #[test]
    fn header_roundtrips() {
        let buf = encode_header(HmsgType::GetInterface, 42);
        assert_eq!(decode_header(&buf), (HmsgType::GetInterface as i32, 42));
    }

    #[test]
    fn align_up_examples() {
        assert_eq!(align_up(0, 8), 0);
        assert_eq!(align_up(1, 8), 8);
        assert_eq!(align_up(8, 8), 8);
        assert_eq!(align_up(9, 8), 16);
    }

    #[test]
    fn request_over_socketpair_roundtrips() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let mut header = vec![0u8; header_len()];
            server.read_exact(&mut header).unwrap();
            let (ty, len) = decode_header(&header);
            assert_eq!(ty, HmsgType::GetInterfaces as i32);
            assert_eq!(len, 0);
            let mut reply = encode_header(HmsgType::GetInterfaces, 3);
            reply.extend_from_slice(b"abc");
            server.write_all(&reply).unwrap();
        });
        let reply = request(&mut client, HmsgType::GetInterfaces, &[]).unwrap();
        assert_eq!(reply, b"abc");
        handle.join().unwrap();
    }
}
