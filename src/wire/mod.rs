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

/// Encodes the payload for a `SET_PORT` request overriding one local port's
/// description, leaving every other settable field on `struct lldpd_port_set`
/// untouched: `rxtx = LLDPD_RXTX_UNCHANGED` (0), `vlan_tx_enabled = -1` (the
/// sentinel `_client_handle_set_port` checks for "leave alone" - see
/// `src/daemon/client.c` upstream), every other field zero/null. Confirmed
/// byte-for-byte against real `lldpd 1.0.22` wire traffic - see
/// `encode_tests::set_port_description_request_matches_a_real_lldpd_capture`
/// below.
///
/// Layout: the root chunk (`orig = 1`) holds `RawPortSet`'s raw bytes, whose
/// `ifname`/`local_descr` fields hold *this message's own* chunk sequence
/// numbers (2 and 3) rather than real pointers - not their content's length
/// or address, just "a chunk with this `orig` follows" (see the module docs
/// on chunk framing). Those two chunks then follow in `struct
/// lldpd_port_set`'s field declaration order, each possibly preceded by a few
/// bytes of alignment padding (see `push_string_chunk`).
pub(crate) fn encode_set_port_description_request(ifname: &str, description: &str) -> Vec<u8> {
    let ptr_size = std::mem::size_of::<usize>();
    let header_len = 2 * ptr_size;

    let mut ifname_body = ifname.as_bytes().to_vec();
    ifname_body.push(0);
    let mut descr_body = description.as_bytes().to_vec();
    descr_body.push(0);

    // `Default::default()` zeroes every *field* but makes no promise about
    // the compiler-inserted padding between `vlan_tx_enabled` and
    // `med_policy` (needed to keep the latter's `usize` 8-byte-aligned) -
    // that byte range is genuinely uninitialized stack memory otherwise, and
    // reading it back (as `pod_bytes` below does) can and did leak garbage
    // into an outbound message. `mem::zeroed()` zeroes the *whole*
    // allocation, padding included, which is sound here since every field is
    // a plain integer with an all-zero-bits valid value (same invariant
    // `read_pod`'s doc comment already relies on for the decode direction).
    let mut pod: raw::RawPortSet = unsafe { std::mem::zeroed() };
    pod.ifname = 2;
    pod.local_descr = 3;
    pod.vlan_tx_enabled = -1;

    let mut body = cursor::pod_bytes(&pod);
    push_string_chunk(&mut body, 2, &ifname_body);
    push_string_chunk(&mut body, 3, &descr_body);

    let mut out = Vec::with_capacity(header_len + body.len());
    out.extend_from_slice(&1usize.to_ne_bytes());
    out.extend_from_slice(&(header_len + body.len()).to_ne_bytes());
    out.extend_from_slice(&body);
    out
}

/// Appends one string chunk (`orig`, then a `header + content`-length `size`
/// header, then `content` itself) to `buf`, first padding `buf` up to
/// pointer-width alignment if it isn't already - matching the padding
/// `wire::cursor::Cursor::chunk_header` skips over on the decode side (see
/// its doc comment), computed the same way (`transport::align_up`).
fn push_string_chunk(buf: &mut Vec<u8>, orig: usize, content: &[u8]) {
    let ptr_size = std::mem::size_of::<usize>();
    let pad = crate::transport::align_up(buf.len(), ptr_size) - buf.len();
    buf.extend(std::iter::repeat_n(0u8, pad));

    let header_len = 2 * ptr_size;
    buf.extend_from_slice(&orig.to_ne_bytes());
    buf.extend_from_slice(&(header_len + content.len()).to_ne_bytes());
    buf.extend_from_slice(content);
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

    /// Captured via `strace -x -s 4096 -e trace=write -f lldpcli configure
    /// ports eth0 lldp portdescription 'test-desc-XYZ'` against a real
    /// `lldpd 1.0.22` (this workspace's Yocto-packaged version) - the same
    /// method this crate's own docs cite for `decode_interfaces_matches_a_
    /// real_lldpd_capture`. A hand-built fixture can't catch a systematic
    /// misunderstanding of the wire format (as the chunk-`size` semantics bug
    /// this crate shipped in 1.0.0/1.0.1 proved); a real capture can.
    #[test]
    fn set_port_description_request_matches_a_real_lldpd_capture() {
        let expected: &[u8] = b"\x01\x00\x00\x00\x00\x00\x00\x00\x96\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x15\x00\x00\x00\x00\x00\x00\x00\x65\x74\x68\x30\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x1e\x00\x00\x00\x00\x00\x00\x00\x74\x65\x73\x74\x2d\x64\x65\x73\x63\x2d\x58\x59\x5a\x00";
        assert_eq!(expected.len(), 150);

        let actual = encode_set_port_description_request("eth0", "test-desc-XYZ");
        assert_eq!(actual, expected);
    }
}
