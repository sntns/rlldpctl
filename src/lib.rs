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
//! - **The exact `lldpd` version.** This crate mirrors the struct layouts of
//!   upstream tag `1.0.22` (matching this workspace's Yocto-packaged
//!   version). A different `lldpd` release can and does change these
//!   structs, which would silently desync parsing.
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
//! v1 implements exactly two requests: `GET_INTERFACES` (list interfaces)
//! and `GET_INTERFACE` (one interface's local info + discovered neighbors).
//! Nothing that changes daemon state (`SET_PORT`, `SET_CONFIG`, ...) or
//! streams live updates (`SUBSCRIBE`) is implemented.

mod client;
mod error;
mod model;
mod transport;
mod wire;

pub use client::{Client, DEFAULT_SOCKET_PATH};
pub use error::{Error, Result};
pub use model::{
    Capabilities, Chassis, ChassisIdSubtype, Interface, InterfaceDetails, ManagementAddress,
    MedInventory, Neighbor, PortIdSubtype,
};
