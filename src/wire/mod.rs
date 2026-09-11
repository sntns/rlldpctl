//! Decoder for lldpd's internal "marshal" payload encoding (`src/marshal.c`
//! upstream).
//!
//! Each message payload is a tree of chunks. A chunk is:
//!
//! ```text
//! [padding to align(pointer width)] [orig: usize] [size: usize] [size bytes of body]
//! ```
//!
//! `body` is either the flat, `#[repr(C)]`-compatible raw bytes of a C struct
//! (see [`raw`]), or a string. A struct's *own* pointer-shaped fields (a
//! `char *`, a `TAILQ_HEAD`/`TAILQ_ENTRY` link) are, in that raw body, either
//! zero (the field was absent) or a small "dummy" reference id - and if
//! non-zero, are followed immediately by one more chunk holding whatever that
//! pointer referred to, *in the exact field declaration order of the C
//! struct*. Lists (`TAILQ`) are just chains of such pointers - there's no
//! explicit length prefix, you follow pointers until you hit zero.
//!
//! This is not a designed wire format: it is `lldpd`'s internal C structures,
//! `memcpy`'d. See the crate-level docs for what that implies.

mod cursor;
mod decode;
pub(crate) mod raw;

pub(crate) use decode::{decode_hardware, decode_interfaces, decode_neighbor_change};

/// Encodes the payload for a `GET_INTERFACE` request: a single chunk holding
/// the interface name as a null-terminated string (mirrors
/// `marshal_serialize(string, name, buffer)` on the client side upstream).
pub(crate) fn encode_interface_name_request(name: &str) -> Vec<u8> {
    let mut body = name.as_bytes().to_vec();
    body.push(0);
    let mut out = 1usize.to_ne_bytes().to_vec(); // orig = 1: first (and only) reference in this message
    out.extend_from_slice(&body.len().to_ne_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod encode_tests {
    use super::*;

    #[test]
    fn interface_name_request_roundtrips_through_the_decoder() {
        let payload = encode_interface_name_request("eth0");
        let mut cursor = cursor::Cursor::new(&payload);
        let (orig, body) = cursor.chunk().unwrap();
        assert_eq!(orig, 1);
        assert_eq!(body, b"eth0\0");
    }
}
