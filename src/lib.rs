//! # rlldpctl
//!
//! A pure-Rust client for [`lldpd`](https://lldpd.github.io/)'s control
//! socket: list interfaces and read discovered LLDP neighbors without
//! shelling out to `lldpcli` or linking `liblldpctl.so`.
//!
//! ```no_run
//! # fn main() -> rlldpctl::Result<()> {
//! let mut client = rlldpctl::Client::connect()?;
//! for iface in client.interfaces()? {
//!     let details = client.interface(&iface.name)?;
//!     for neighbor in &details.neighbors {
//!         println!(
//!             "{}: {} ({:?})",
//!             iface.name,
//!             neighbor.chassis.name.as_deref().unwrap_or("?"),
//!             neighbor.port_id_str(),
//!         );
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## What this actually talks to, and why that's fragile
//!
//! `lldpd` does not define a stable IPC protocol for `lldpcli`/`liblldpctl`
//! to talk to it with. What's actually on the wire is `lldpd`'s own internal
//! C structures (`struct lldpd_port`, `struct lldpd_chassis`, ...), `memcpy`'d
//! with a small pointer-graph-following envelope around them (see
//! `src/marshal.c` and `src/ctl.c` upstream). It is coupled to:
//!
//! - **The exact `lldpd` version - and this is a narrower window than you'd
//!   guess.** Diffing `src/lldpd-structs.h` across every tag from `0.9.0` to
//!   `master` shows the framing and marshaling *mechanism* has been stable
//!   since 2008, but several fields this crate actually reads are recent:
//!   `lldpd_interface.alias` and `lldpd_hardware.h_ifalias` were only added
//!   in **`1.0.21`**, `lldpd_port.p_vlan_advertise_pattern` in `1.0.20`, and
//!   the `hmsg_type` enum gained a new member at `1.0.14` that shifts
//!   `GET_INTERFACE`'s numeric value from 5 to 6 (sending 6 to an older
//!   daemon would silently hit `GET_DEFAULT_PORT` instead). `master` has
//!   already added an unreleased `lldpd_hardware.h_flags_previous` field on
//!   top of `1.0.22`. Net effect: **this crate is only verified against
//!   `lldpd` 1.0.21-1.0.22** (matching this workspace's Yocto-packaged
//!   `1.0.22`) - not "any 1.0.x", and not yet whatever ships after `1.0.22`.
//!   A different version can and does change these structs, which would
//!   silently desync parsing rather than fail loudly.
//! - **The build-time feature flags `lldpd` was compiled with.** Several
//!   struct fields exist only under `#ifdef ENABLE_DOT1` /
//!   `ENABLE_DOT3` / `ENABLE_LLDPMED` / `ENABLE_CUSTOM`. This crate assumes
//!   `dot1`, `dot3`, `cdp`, `fdp` and `lldpmed` are enabled and `custom` is
//!   not, matching this workspace's `lldpd` recipe
//!   (`PACKAGECONFIG ??= "cdp fdp edp sonmp lldpmed dot1 dot3"`).
//! - **The host's C ABI**, specifically pointer width (raw structs are
//!   declared `#[repr(C)]` so this is handled automatically for whichever
//!   target you build for, *as long as* it matches the `lldpd` binary you're
//!   talking to) and `time_t`'s width tracking pointer width (true of the
//!   traditional glibc/musl Linux ABI on both 32- and 64-bit; not true of a
//!   32-bit target using a 64-bit-`time_t` "time64" C library).
//!
//! In short: this is not a stable protocol client the way an HTTP or DBus
//! client is. It's the same trade-off upstream itself accepts internally
//! between `lldpd` and `liblldpctl.so` - just reimplemented without linking
//! that library. See `README.md` for the full rationale and scope.
//!
//! ## Scope
//!
//! v1 implements five requests: `GET_INTERFACES` (list interfaces),
//! `GET_INTERFACE` (one interface's local info + discovered neighbors),
//! `SUBSCRIBE`/`NOTIFICATION` (a live feed of neighbor changes, via
//! [`Client::subscribe`] and [`Subscription`]), and `SET_PORT` (currently
//! just overriding a port's description - see
//! [`Client::set_port_description`]). Nothing else that changes daemon state
//! (`SET_CHASSIS`, `SET_CONFIG`, ...) is implemented.
//!
//! ## Async
//!
//! Enable the `tokio` feature for `AsyncClient`/`AsyncSubscription`, the
//! same API backed by `tokio::net::UnixStream` instead of
//! `std::os::unix::net::UnixStream`. Both share this crate's wire
//! encode/decode logic verbatim - only the I/O differs - so they can't
//! disagree about the wire format. This is entirely additive: the sync API
//! has no tokio dependency and keeps working with the `tokio` feature off.

#[cfg(feature = "tokio")]
mod async_client;
mod client;
mod error;
mod model;
mod subscription;
mod transport;
mod wire;

#[cfg(feature = "tokio")]
pub use async_client::{AsyncClient, AsyncSubscription};
pub use client::{Client, DEFAULT_SOCKET_PATH};
pub use error::{Error, Result};
pub use model::{
    Capabilities, Chassis, ChassisIdSubtype, Interface, InterfaceDetails, ManagementAddress,
    MedInventory, Neighbor, NeighborChange, NeighborChangeKind, PortIdSubtype,
};
pub use subscription::Subscription;
