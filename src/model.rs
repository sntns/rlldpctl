//! The ergonomic, safe-Rust view of what lldpd knows: interfaces and the
//! neighbors discovered on them.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::rc::Rc;

/// One local interface, as returned by [`crate::Client::interfaces`].
///
/// This is the cheap, no-neighbor-query view (`GET_INTERFACES`); pass its
/// [`name`](Interface::name) to [`crate::Client::interface`] to get full
/// details including discovered neighbors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub alias: Option<String>,
}

/// Full state of one local interface (`GET_INTERFACE`): its own identity plus
/// every neighbor lldpd has heard from on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceDetails {
    pub name: String,
    pub alias: Option<String>,
    pub mac_address: [u8; 6],
    /// The chassis lldpd advertises as *us* on this interface (i.e. what a
    /// neighbor would see if it looked back at this machine).
    pub local_chassis: Option<Rc<Chassis>>,
    pub neighbors: Vec<Neighbor>,
}

/// One neighbor device seen on an interface.
///
/// `chassis` is reference-counted because lldpd deduplicates chassis
/// internally: two neighbor entries reached via different protocols (say,
/// LLDP and CDP) but originating from the same physical device share one
/// `Chassis`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbor {
    pub chassis: Rc<Chassis>,
    pub port_id_subtype: PortIdSubtype,
    /// Raw port identifier bytes; interpretation depends on `port_id_subtype`
    /// (e.g. 6 raw bytes for `MacAddress`, a printable string for `IfName`).
    pub port_id: Vec<u8>,
    pub port_description: Option<String>,
    /// Time-to-live announced for this neighbor entry, in seconds.
    pub ttl: u16,
}

impl Neighbor {
    /// Convenience accessor for the common case where `port_id` is a
    /// printable string (true for `IfAlias`, `IfName`, `Local`, and often in
    /// practice for others too). Returns `None` on invalid UTF-8.
    pub fn port_id_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.port_id).ok()
    }
}

/// A neighbor (or local) chassis: the "this whole box" identity, as opposed
/// to one of its ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chassis {
    pub id_subtype: ChassisIdSubtype,
    /// Raw chassis identifier bytes; interpretation depends on `id_subtype`.
    pub id: Vec<u8>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub capabilities_available: Capabilities,
    pub capabilities_enabled: Capabilities,
    pub management_addresses: Vec<ManagementAddress>,
    /// LLDP-MED inventory TLVs, if the neighbor sent any (all fields are
    /// `None` when it didn't, rather than this being wrapped in an `Option`).
    pub med: MedInventory,
}

impl Chassis {
    /// Convenience accessor for the common case where `id` is a printable
    /// string (true for `ChassisComponent`, `IfAlias`, `IfName`, `Local`).
    pub fn id_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.id).ok()
    }
}

/// LLDP-MED "Inventory" TLVs (IEC/TR 61936, TIA-1057 §12.3). Populated only if
/// the neighbor implements LLDP-MED and advertises them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MedInventory {
    pub hardware_revision: Option<String>,
    pub firmware_revision: Option<String>,
    pub software_revision: Option<String>,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub asset_id: Option<String>,
}

/// A management address advertised by a chassis (IEEE 802.1AB Management
/// Address TLV).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagementAddress {
    family: u32,
    octets: Vec<u8>,
    pub interface_index: u32,
}

impl ManagementAddress {
    pub(crate) fn new(family: u32, octets: Vec<u8>, interface_index: u32) -> Self {
        Self {
            family,
            octets,
            interface_index,
        }
    }

    /// Decodes the address as an [`IpAddr`], if it is one lldpd recognizes as
    /// IPv4 or IPv6 (lldpd can in principle carry other address families, in
    /// which case this returns `None` - use [`Self::raw_octets`] instead).
    pub fn ip(&self) -> Option<IpAddr> {
        match self.family {
            1 if self.octets.len() >= 4 => Some(IpAddr::V4(Ipv4Addr::new(
                self.octets[0],
                self.octets[1],
                self.octets[2],
                self.octets[3],
            ))),
            2 if self.octets.len() >= 16 => {
                let mut b = [0u8; 16];
                b.copy_from_slice(&self.octets[..16]);
                Some(IpAddr::V6(Ipv6Addr::from(b)))
            }
            _ => None,
        }
    }

    /// The raw address bytes as lldpd stored them (network byte order),
    /// regardless of address family.
    pub fn raw_octets(&self) -> &[u8] {
        &self.octets
    }
}

/// `IEEE 802.1AB` chassis ID subtype (`LLDP_CHASSISID_SUBTYPE_*` in
/// `lldp-const.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChassisIdSubtype {
    ChassisComponent,
    IfAlias,
    PortComponent,
    MacAddress,
    NetworkAddress,
    IfName,
    Local,
    /// A value this crate doesn't have a name for (future TLV extension).
    Other(u8),
}

impl From<u8> for ChassisIdSubtype {
    fn from(v: u8) -> Self {
        match v {
            1 => Self::ChassisComponent,
            2 => Self::IfAlias,
            3 => Self::PortComponent,
            4 => Self::MacAddress,
            5 => Self::NetworkAddress,
            6 => Self::IfName,
            7 => Self::Local,
            other => Self::Other(other),
        }
    }
}

/// `IEEE 802.1AB` port ID subtype (`LLDP_PORTID_SUBTYPE_*` in `lldp-const.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortIdSubtype {
    Unknown,
    IfAlias,
    PortComponent,
    MacAddress,
    NetworkAddress,
    IfName,
    AgentCircuitId,
    Local,
    /// A value this crate doesn't have a name for (future TLV extension).
    Other(u8),
}

impl From<u8> for PortIdSubtype {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Unknown,
            1 => Self::IfAlias,
            2 => Self::PortComponent,
            3 => Self::MacAddress,
            4 => Self::NetworkAddress,
            5 => Self::IfName,
            6 => Self::AgentCircuitId,
            7 => Self::Local,
            other => Self::Other(other),
        }
    }
}

/// System capabilities bitmap (IEEE 802.1AB Table 8-4). Wraps the raw `u16`
/// lldpd sends; use [`Capabilities::contains`] to test individual bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities(pub u16);

impl Capabilities {
    pub const OTHER: u16 = 0x0001;
    pub const REPEATER: u16 = 0x0002;
    pub const BRIDGE: u16 = 0x0004;
    pub const WLAN_ACCESS_POINT: u16 = 0x0008;
    pub const ROUTER: u16 = 0x0010;
    pub const TELEPHONE: u16 = 0x0020;
    pub const DOCSIS_CABLE_DEVICE: u16 = 0x0040;
    pub const STATION_ONLY: u16 = 0x0080;
    pub const CVLAN_COMPONENT: u16 = 0x0100;
    pub const SVLAN_COMPONENT: u16 = 0x0200;
    pub const TWO_PORT_MAC_RELAY: u16 = 0x0400;

    pub fn contains(self, bit: u16) -> bool {
        self.0 & bit == bit
    }
}
