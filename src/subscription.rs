use std::os::unix::net::UnixStream;

use crate::error::{Error, Result};
use crate::model::NeighborChange;
use crate::transport;
use crate::wire;

/// A live feed of neighbor-table changes, obtained from
/// [`crate::Client::subscribe`] (`SUBSCRIBE` + `NOTIFICATION`).
///
/// lldpd pushes one [`NeighborChange`] per added, updated, or expired
/// neighbor on *any* local interface - the protocol has no per-interface
/// filtering, so filter on [`NeighborChange::interface`] yourself if you
/// only care about some of them. Iterate it directly:
///
/// ```no_run
/// # fn main() -> rlldpctl::Result<()> {
/// let client = rlldpctl::Client::connect()?;
/// for change in client.subscribe()? {
///     let change = change?;
///     println!("{:?} on {}", change.kind, change.interface);
/// }
/// # Ok(())
/// # }
/// ```
///
/// Iteration blocks between notifications - this crate has no async runtime
/// (see the crate-level docs) - and ends (`next()` returns `None`) only when
/// lldpd closes the connection.
#[derive(Debug)]
pub struct Subscription {
    pub(crate) stream: UnixStream,
}

impl Iterator for Subscription {
    type Item = Result<NeighborChange>;

    fn next(&mut self) -> Option<Self::Item> {
        match transport::recv_notification(&mut self.stream) {
            Ok(payload) => Some(wire::decode_neighbor_change(&payload)),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn ends_cleanly_when_lldpd_closes_the_connection() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server); // simulate lldpd hanging up
        let mut sub = Subscription { stream: client };
        assert!(sub.next().is_none());
    }

    #[test]
    fn surfaces_a_decode_error_without_treating_it_as_end_of_stream() {
        let (client, mut server) = UnixStream::pair().unwrap();
        // A well-framed NOTIFICATION with an empty payload: the transport
        // layer reads it fine, but there aren't enough bytes for even the
        // top-level `lldpd_neighbor_change` chunk header, so decoding fails.
        let ptr_size = std::mem::size_of::<usize>();
        let mut frame = vec![0u8; 4];
        frame[0..4].copy_from_slice(&10i32.to_ne_bytes()); // NOTIFICATION
        let pad = (ptr_size - 4 % ptr_size) % ptr_size;
        frame.extend(std::iter::repeat_n(0u8, pad));
        frame.extend_from_slice(&0usize.to_ne_bytes()); // len = 0
        server.write_all(&frame).unwrap();
        let mut sub = Subscription { stream: client };
        assert!(sub.next().unwrap().is_err());
    }
}
