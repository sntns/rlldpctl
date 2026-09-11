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

    /// Reads one `struct marshal_serialized` chunk: skips the alignment
    /// padding that precedes it, reads its `(orig, size)` header, and returns
    /// `(orig, body)`, having advanced past the whole chunk.
    ///
    /// `orig` is the sender's "dummy" reference id (see `marshal.c`): a small
    /// sequential integer, never zero for a real chunk, used to detect a
    /// pointer that is shared between two places in the source struct graph
    /// (in practice, only `lldpd_port.p_chassis` - see `wire::decode`).
    pub(super) fn chunk(&mut self) -> Result<(usize, &'a [u8])> {
        let pad = align_up(self.pos, PTR_SIZE) - self.pos;
        self.pos += pad;

        let header_len = 2 * PTR_SIZE;
        self.need(header_len)?;
        let orig = read_native_usize(&self.buf[self.pos..self.pos + PTR_SIZE]);
        let size = read_native_usize(&self.buf[self.pos + PTR_SIZE..self.pos + header_len]);
        self.pos += header_len;

        self.need(size)?;
        let body = &self.buf[self.pos..self.pos + size];
        self.pos += size;
        Ok((orig, body))
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

/// Reads the next chunk and interprets its body as a `T` (used for every
/// "real", non-embedded struct: the top-level message body, and anything
/// reached through a `pointer`-kind field).
pub(super) fn read_chunk_pod<T: Copy>(cursor: &mut Cursor) -> Result<T> {
    let (_orig, body) = cursor.chunk()?;
    read_pod(body)
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
    let (_orig, body) = cursor.chunk()?;
    let bytes = body.strip_suffix(&[0u8]).unwrap_or(body);
    Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
}

/// Reads a fixed-length (not null-terminated) byte-string chunk, if
/// `ptr_field` says one is present.
pub(super) fn read_opt_bytes(cursor: &mut Cursor, ptr_field: usize) -> Result<Option<Vec<u8>>> {
    if ptr_field == 0 {
        return Ok(None);
    }
    let (_orig, body) = cursor.chunk()?;
    Ok(Some(body.to_vec()))
}

/// Consumes the empty marker chunk emitted for an embedded (non-pointer)
/// substructure (`MARSHAL_SUBSTRUCT` upstream): its scalar fields are already
/// part of the parent's raw body, so only its own further pointer fields (if
/// any) produce real chunks - those are read right after this by the caller.
pub(super) fn consume_substruct_marker(cursor: &mut Cursor) -> Result<()> {
    let (_orig, body) = cursor.chunk()?;
    if !body.is_empty() {
        return Err(Error::Protocol(
            "expected an empty substructure marker chunk".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_bytes(orig: usize, body: &[u8]) -> Vec<u8> {
        let mut out = orig.to_ne_bytes().to_vec();
        out.extend_from_slice(&body.len().to_ne_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_a_single_chunk() {
        let buf = chunk_bytes(1, b"hi");
        let mut c = Cursor::new(&buf);
        let (orig, body) = c.chunk().unwrap();
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
        let (orig1, body1) = c.chunk().unwrap();
        assert_eq!((orig1, body1), (1, &b"x"[..]));
        let (orig2, body2) = c.chunk().unwrap();
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
        assert!(c.chunk().is_err());
    }
}
