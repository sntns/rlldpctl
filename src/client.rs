use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::{Interface, InterfaceDetails};
use crate::subscription::Subscription;
use crate::transport::{self, HmsgType};
use crate::wire;

/// Default lldpd control socket path (`--with-lldpd-ctl-socket`'s upstream
/// default, `${runstatedir}/lldpd.socket`, which is `/var/run/lldpd.socket`
/// on any system where `/var/run` is the usual symlink to `/run`).
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/lldpd.socket";

/// A connection to a running `lldpd`.
///
/// lldpd's control protocol is a synchronous, one-request-at-a-time exchange
/// over a single stream connection (see the crate-level docs), so each
/// method here sends one request and waits for its reply; there is no
/// background thread or event loop.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    socket_path: PathBuf,
}

impl Client {
    /// Connects to lldpd at the default socket path
    /// ([`DEFAULT_SOCKET_PATH`]).
    pub fn connect() -> Result<Self> {
        Self::connect_to(DEFAULT_SOCKET_PATH)
    }

    /// Connects to lldpd listening on the Unix socket at `path`.
    pub fn connect_to<P: AsRef<Path>>(path: P) -> Result<Self> {
        let stream = UnixStream::connect(path.as_ref())?;
        Ok(Self {
            stream,
            socket_path: path.as_ref().to_path_buf(),
        })
    }

    /// Lists every interface lldpd knows about (`GET_INTERFACES`).
    ///
    /// This is the cheap call: it does not include neighbor data. Follow up
    /// with [`Client::interface`] for interfaces you actually care about.
    pub fn interfaces(&mut self) -> Result<Vec<Interface>> {
        let payload = transport::request(&mut self.stream, HmsgType::GetInterfaces, &[])?;
        wire::decode_interfaces(&payload)
    }

    /// Fetches full state for one interface (`GET_INTERFACE`), including
    /// every neighbor lldpd has discovered on it.
    ///
    /// `ifname` should be a name as returned by [`Client::interfaces`] (e.g.
    /// `"eth0"`); an interface lldpd doesn't know about surfaces as
    /// [`crate::Error::RequestRejected`].
    pub fn interface(&mut self, ifname: &str) -> Result<InterfaceDetails> {
        let request_payload = wire::encode_interface_name_request(ifname);
        let payload =
            transport::request(&mut self.stream, HmsgType::GetInterface, &request_payload)?;
        wire::decode_hardware(&payload)
    }

    /// Convenience helper: [`Client::interfaces`] followed by
    /// [`Client::interface`] on each of them, skipping any that disappear
    /// (e.g. an interface goes down) between the two calls rather than
    /// failing the whole batch.
    pub fn all_interfaces(&mut self) -> Result<Vec<InterfaceDetails>> {
        let mut out = Vec::new();
        for iface in self.interfaces()? {
            match self.interface(&iface.name) {
                Ok(details) => out.push(details),
                Err(crate::error::Error::RequestRejected) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Path this client connected to, mostly useful for logging.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Subscribes to live neighbor-change notifications (`SUBSCRIBE`),
    /// returning a [`Subscription`] you can iterate for [`NeighborChange`]s.
    ///
    /// This consumes the client: lldpd's control protocol does not allow
    /// further `GET_INTERFACES`/`GET_INTERFACE` calls on a connection once it
    /// has subscribed (`src/lib/atom.c` upstream explicitly refuses further
    /// requests once "watching" starts) - open a separate [`Client`] if you
    /// still need those.
    ///
    /// [`NeighborChange`]: crate::NeighborChange
    pub fn subscribe(mut self) -> Result<Subscription> {
        transport::request(&mut self.stream, HmsgType::Subscribe, &[])?;
        Ok(Subscription {
            stream: self.stream,
        })
    }
}
