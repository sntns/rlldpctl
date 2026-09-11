use thiserror::Error;

/// Errors returned by this crate.
#[derive(Error, Debug)]
pub enum Error {
    /// Could not connect to (or lost connection with) the lldpd control socket.
    #[error("I/O error talking to lldpd: {0}")]
    Io(#[from] std::io::Error),

    /// The daemon answered with a message type we didn't ask for.
    #[error("unexpected reply from lldpd: expected message type {expected}, got {got}")]
    UnexpectedMessageType { expected: u32, got: u32 },

    /// lldpd reported it could not honor the request (empty payload with no error type).
    #[error("lldpd rejected the request (interface not found, or feature disabled)")]
    RequestRejected,

    /// The byte stream did not match the shape this crate expects from lldpd's
    /// control protocol. This protocol is an internal, per-version wire format
    /// (see the crate README), so this is the error you'll get if the daemon
    /// on the other end is a version this crate was not pinned against.
    #[error("malformed lldpd control-protocol message: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;
