//! Generic reader for the chunked, self-referential-pointer-graph encoding
//! `marshal.c` produces (see the module docs on [`super`]).

use crate::error::{Error, Result};
use crate::transport::align_up;

pub(super) const PTR_SIZE: usize = std::mem::size_of::<usize>();

/// A cursor over one already-received message payload.
pub(super) struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<()> {
        if self
            .pos
            .checked_add(n)
            .is_none_or(|end| end > self.buf.len())
        {
            return Err(Error::Protocol(format!(
                "truncated message: need {n} more bytes at offset {} (have {})",
                self.pos,
                self.buf.len()
            )));
        }
        Ok(())
    }

    /// Reads exactly `n` bytes and advances past them.
    fn read_raw(&mut self, n: usize) -> Result<&'a [u8]> {
        self.need(n)?;
        let bytes = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(bytes)
    }

    /// Skips the alignment padding that precedes a chunk, then reads its
    /// `struct marshal_serialized` header: `(orig, size)`.
    ///
    /// `orig` is the sender's "dummy" reference id (see `marshal.c`): a small
    /// sequential integer, never zero for a real chunk, used to detect a
    /// pointer that is shared between two places in the source struct graph
    /// (in practice, only `lldpd_port.p_chassis` - see `wire::decode`).
    ///
    /// `size` is **not** "how many content bytes follow" - upstream's
    /// `marshal_serialize_` sets it to the *entire* serialized length of this
    /// chunk, header included, plus every chunk nested inside it
    /// (recursively, each with its own header) - see `serialized->size = len`
    /// in `src/marshal.c`, where `len` accumulates through
    /// `len += sublen + padlen` for every pointer/substruct field. The real
    /// unmarshaler never uses it to skip bytes either: it walks structurally,
    /// using its compile-time-known field layout to know how many chunks
    /// follow and in what order. So this only returns it for chunk kinds
    /// where it is meaningful as a length (a leaf value - see
    /// [`Cursor::leaf`]); struct chunks ([`read_chunk_pod`]) and substructure
    /// markers ([`consume_substruct_marker`]) read a fixed, statically-known
    /// number of bytes instead and otherwise ignore it.
    fn chunk_header(&mut self) -> Result<(usize, usize)> {
        let pad = align_up(self.pos, PTR_SIZE) - self.pos;
        self.pos += pad;

        let header_len = 2 * PTR_SIZE;
        self.need(header_len)?;
        let orig = read_native_usize(&self.buf[self.pos..self.pos + PTR_SIZE]);
        let size = read_native_usize(&self.buf[self.pos + PTR_SIZE..self.pos + header_len]);
        self.pos += header_len;
        Ok((orig, size))
    }

    /// Reads one *leaf* chunk (a string or fixed-length byte string, with no
    /// chunks of its own nested inside it): its header's declared `size`
    /// really is `header + content` here, so the content length is that
    /// minus the header's own size.
    fn leaf(&mut self) -> Result<(usize, &'a [u8])> {
        let (orig, declared_size) = self.chunk_header()?;
        let header_len = 2 * PTR_SIZE;
        let content_len = declared_size.checked_sub(header_len).ok_or_else(|| {
            Error::Protocol(format!(
                "chunk declares {declared_size} bytes, smaller than its own {header_len}-byte header"
            ))
        })?;
        Ok((orig, self.read_raw(content_len)?))
    }
}

fn read_native_usize(bytes: &[u8]) -> usize {
    let mut b = [0u8; 8];
    b[..PTR_SIZE].copy_from_slice(bytes);
    u64::from_ne_bytes(b) as usize
}

/// Interprets a byte slice as a `T` via an unaligned copy.
///
/// # Safety invariant
/// Only call this with a `T` that is `Copy` and composed solely of plain
/// integers and byte arrays (true of every type in [`super::raw`]), so that
/// any bit pattern lldpd happens to have sent is a valid value - there is no
/// validity constraint to uphold (no `bool`, `char`, references, or
/// fixed-discriminant enums among the fields).
pub(super) fn read_pod<T: Copy>(bytes: &[u8]) -> Result<T> {
    if bytes.len() < std::mem::size_of::<T>() {
        return Err(Error::Protocol(format!(
            "chunk too small: need {} bytes, got {}",
            std::mem::size_of::<T>(),
            bytes.len()
        )));
    }
    Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) })
}

/// The encode-side counterpart to [`read_pod`]: the raw bytes of `value`, for
/// writing into an outbound request's struct-chunk content (see
/// `wire::mod::encode_set_port_description_request`). Same safety invariant
/// as `read_pod` applies in reverse - only call this with a `T` that is
/// `Copy` and composed solely of plain integers and byte arrays (true of
/// every type in [`super::raw`]), so there's no uninitialized or
/// otherwise-invalid data among the bytes read out.
pub(super) fn pod_bytes<T: Copy>(value: &T) -> Vec<u8> {
    let ptr = (value as *const T).cast::<u8>();
    unsafe { std::slice::from_raw_parts(ptr, std::mem::size_of::<T>()) }.to_vec()
}

/// Reads the next chunk's header and interprets exactly `size_of::<T>()`
/// bytes right after it as a `T` (used for every "real", non-embedded
/// struct: the top-level message body, and anything reached through a
/// `pointer`-kind field). Whatever the header's declared `size` says beyond
/// that is this struct's own nested chunks, read separately by the caller in
/// schema order - see [`Cursor::chunk_header`].
pub(super) fn read_chunk_pod<T: Copy>(cursor: &mut Cursor) -> Result<T> {
    let (_orig, declared_size) = cursor.chunk_header()?;
    let header_len = 2 * PTR_SIZE;
    let n = std::mem::size_of::<T>();
    if declared_size < header_len + n {
        return Err(Error::Protocol(format!(
            "chunk declares {declared_size} bytes, too small for its own {header_len}-byte header plus the {n}-byte struct it contains"
        )));
    }
    read_pod(cursor.read_raw(n)?)
}

/// Reads a null-terminated string chunk, if `ptr_field` (the raw pointer
/// value read from the enclosing struct) says one is present.
///
/// Invalid UTF-8 is replaced rather than treated as a parse error: this is a
/// discovery tool reading attacker-reachable^1 network-announced strings, and
/// a garbled hostname/description is a much more useful failure mode than an
/// aborted query.
///
/// ^1: not a trust boundary this crate defends - see the crate-level docs.
pub(super) fn read_opt_cstring(cursor: &mut Cursor, ptr_field: usize) -> Result<Option<String>> {
    if ptr_field == 0 {
        return Ok(None);
    }
    let (_orig, body) = cursor.leaf()?;
    let bytes = body.strip_suffix(&[0u8]).unwrap_or(body);
    Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
}

/// Reads a fixed-length (not null-terminated) byte-string chunk, if
/// `ptr_field` says one is present.
pub(super) fn read_opt_bytes(cursor: &mut Cursor, ptr_field: usize) -> Result<Option<Vec<u8>>> {
    if ptr_field == 0 {
        return Ok(None);
    }
    let (_orig, body) = cursor.leaf()?;
    Ok(Some(body.to_vec()))
}

/// Consumes the header of the marker chunk emitted for an embedded
/// (non-pointer) substructure (`MARSHAL_SUBSTRUCT` upstream, serialized with
/// `skip = 1`): its own scalar fields are already part of the parent's raw
/// body, so this chunk contributes no content of its own - only its further
/// pointer fields (if any) produce real nested chunks, read right after this
/// by the caller. Its declared size covers those nested chunks (see
/// [`Cursor::chunk_header`]), not "this marker is empty", so it isn't
/// checked here.
pub(super) fn consume_substruct_marker(cursor: &mut Cursor) -> Result<()> {
    cursor.chunk_header()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a *leaf* chunk (matching real `marshal_serialize_` output):
    /// the declared `size` is the header's own length plus the content
    /// length, not just the content length - see [`Cursor::chunk_header`].
    fn chunk_bytes(orig: usize, body: &[u8]) -> Vec<u8> {
        let header_len = 2 * PTR_SIZE;
        let mut out = orig.to_ne_bytes().to_vec();
        out.extend_from_slice(&(header_len + body.len()).to_ne_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_a_single_chunk() {
        let buf = chunk_bytes(1, b"hi");
        let mut c = Cursor::new(&buf);
        let (orig, body) = c.leaf().unwrap();
        assert_eq!(orig, 1);
        assert_eq!(body, b"hi");
    }

    #[test]
    fn pads_between_chunks_to_pointer_alignment() {
        let mut buf = chunk_bytes(1, b"x"); // odd-length body -> next chunk needs padding
        let second = chunk_bytes(2, b"second");
        buf.extend_from_slice(&second);
        // Manually reproduce the padding a real sender would insert.
        let mut padded = chunk_bytes(1, b"x");
        let pad = (PTR_SIZE - padded.len() % PTR_SIZE) % PTR_SIZE;
        padded.extend(std::iter::repeat_n(0u8, pad));
        padded.extend_from_slice(&second);

        let mut c = Cursor::new(&padded);
        let (orig1, body1) = c.leaf().unwrap();
        assert_eq!((orig1, body1), (1, &b"x"[..]));
        let (orig2, body2) = c.leaf().unwrap();
        assert_eq!((orig2, body2), (2, &b"second"[..]));
    }

    #[test]
    fn opt_cstring_strips_trailing_nul() {
        let buf = chunk_bytes(1, b"hello\0");
        let mut c = Cursor::new(&buf);
        assert_eq!(
            read_opt_cstring(&mut c, 42).unwrap(),
            Some("hello".to_string())
        );
    }

    #[test]
    fn opt_cstring_null_pointer_reads_nothing() {
        let mut c = Cursor::new(&[]);
        assert_eq!(read_opt_cstring(&mut c, 0).unwrap(), None);
    }

    #[test]
    fn truncated_message_is_a_protocol_error() {
        let mut c = Cursor::new(&[0u8; 3]);
        assert!(c.leaf().is_err());
    }
}
