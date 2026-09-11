//! Decoder for lldpd's internal "marshal" payload encoding (`src/marshal.c`
//! upstream).
//!
//! Each message payload is a tree of chunks. A chunk is:
//!
//! ```text
//! [padding to align(pointer width)] [orig: usize] [size: usize] [content]
//! ```
//!
//! `size` is **not** `content`'s length - it's `lldpd`'s own bookkeeping
//! value, the *entire* serialized length of this chunk (header included)
//! plus every chunk nested inside it, recursively. For a leaf chunk (a
//! string, or a fixed-length byte string - never anything nested inside it),
//! that means `content`'s real length is `size` minus the 2-`usize` header.
//! For a struct chunk, `content` is the flat, `#[repr(C)]`-compatible raw
//! bytes of a C struct (see [`raw`]) - always exactly `size_of` that struct,
//! regardless of what the header's `size` says, because the *nested* chunks
//! for the struct's own pointer-shaped fields (a `char *`, a
//! `TAILQ_HEAD`/`TAILQ_ENTRY` link) come right after `content`, not inside
//! the range `size` would suggest if you (wrongly, as this crate did until
//! it was tested against a real `lldpd` for the first time) treated it as a
//! content length. A pointer-shaped field's raw value is either zero (the
//! field was absent) or a small "dummy" reference id, telling you whether
//! the next chunk exists at all - never its size. Lists (`TAILQ`) are just
//! chains of such pointers - there's no explicit length prefix, you follow
//! pointers until you hit zero.
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
    // The declared size is the header's own length plus the content length,
    // not just the content length - see
    // `wire::cursor::Cursor::chunk_header`'s doc comment for why (matches
    // real `lldpcli` traffic byte for byte, confirmed by capturing it).
    let header_len = 2 * std::mem::size_of::<usize>();
    let mut out = 1usize.to_ne_bytes().to_vec(); // orig = 1: first (and only) reference in this message
    out.extend_from_slice(&(header_len + body.len()).to_ne_bytes());
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
        // `ptr_field` only gates whether a chunk is read at all here (any
        // non-zero value does) - it isn't cross-checked against the chunk's
        // own `orig`.
        assert_eq!(
            cursor::read_opt_cstring(&mut cursor, 1).unwrap(),
            Some("eth0".to_string())
        );
    }
}
