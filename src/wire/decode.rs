//! Per-type decoders. Each function's *order of operations* mirrors the
//! declaration order of the matching `MARSHAL_BEGIN`/`MARSHAL_END` block in
//! upstream `src/lldpd-structs.h` exactly, because that order is the order
//! chunks appear on the wire - see the module docs on [`super`].

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::model::{
    Capabilities, Chassis, Interface, InterfaceDetails, ManagementAddress, MedInventory, Neighbor,
    NeighborChange,
};

use super::cursor::{
    consume_substruct_marker, read_chunk_pod, read_opt_bytes, read_opt_cstring, Cursor,
};
use super::raw::{
    RawChassis, RawHardware, RawInterface, RawInterfaceList, RawMedLoc, RawMgmt, RawNeighborChange,
    RawPi, RawPort, RawPpvid, RawVlan,
};

/// Chassis pointers are the one place lldpd's wire format can reference the
/// same object twice (several neighbor ports, reached via different
/// protocols, pointing at one physical chassis) - see `marshal.c`'s `refs`
/// list. Keyed by the sender's raw pointer field value (its "dummy" id).
type ChassisCache = HashMap<usize, Arc<Chassis>>;

pub(crate) fn decode_interfaces(payload: &[u8]) -> Result<Vec<Interface>> {
    let mut cursor = Cursor::new(payload);
    let list: RawInterfaceList = read_chunk_pod(&mut cursor)?;
    if list.tqh_first == 0 {
        return Ok(Vec::new());
    }
    decode_interface_chain(&mut cursor)
}

fn decode_interface_chain(cursor: &mut Cursor) -> Result<Vec<Interface>> {
    let raw: RawInterface = read_chunk_pod(cursor)?;
    // `next` is processed before `name`/`alias` in the marshal table, so the
    // rest of the list is nested *inside* this element's wire representation.
    let rest = if raw.next_tqe_next != 0 {
        decode_interface_chain(cursor)?
    } else {
        Vec::new()
    };
    let name = read_opt_cstring(cursor, raw.name)?.unwrap_or_default();
    let alias = read_opt_cstring(cursor, raw.alias)?;
    let mut out = vec![Interface { name, alias }];
    out.extend(rest);
    Ok(out)
}

fn decode_mgmt_chain(cursor: &mut Cursor) -> Result<Vec<ManagementAddress>> {
    let raw: RawMgmt = read_chunk_pod(cursor)?;
    let rest = if raw.tqe_next != 0 {
        decode_mgmt_chain(cursor)?
    } else {
        Vec::new()
    };
    let len = (raw.m_addrsize).min(raw.m_addr.len());
    let mut out = vec![ManagementAddress::new(
        raw.m_family as u32,
        raw.m_addr[..len].to_vec(),
        raw.m_iface,
    )];
    out.extend(rest);
    Ok(out)
}

fn decode_chassis(cursor: &mut Cursor) -> Result<Chassis> {
    let raw: RawChassis = read_chunk_pod(cursor)?;
    let id = read_opt_bytes(cursor, raw.c_id)?.unwrap_or_default();
    let name = read_opt_cstring(cursor, raw.c_name)?;
    let description = read_opt_cstring(cursor, raw.c_descr)?;
    let management_addresses = if raw.c_mgmt_tqh_first != 0 {
        decode_mgmt_chain(cursor)?
    } else {
        Vec::new()
    };
    let med = MedInventory {
        hardware_revision: read_opt_cstring(cursor, raw.c_med_hw)?,
        firmware_revision: read_opt_cstring(cursor, raw.c_med_fw)?,
        software_revision: read_opt_cstring(cursor, raw.c_med_sw)?,
        serial_number: read_opt_cstring(cursor, raw.c_med_sn)?,
        manufacturer: read_opt_cstring(cursor, raw.c_med_manuf)?,
        model: read_opt_cstring(cursor, raw.c_med_model)?,
        asset_id: read_opt_cstring(cursor, raw.c_med_asset)?,
    };
    Ok(Chassis {
        id_subtype: raw.c_id_subtype.into(),
        id,
        name,
        description,
        capabilities_available: Capabilities(raw.c_cap_available),
        capabilities_enabled: Capabilities(raw.c_cap_enabled),
        management_addresses,
        med,
    })
}

/// A location TLV's own data is a `MARSHAL_FSTR`, but it hangs off a
/// `MARSHAL_SUBSTRUCT` slot (one of `p_med_location[0..3]`), so decoding it
/// still starts with an empty marker chunk like any embedded substructure.
fn decode_med_location_marker(cursor: &mut Cursor, loc: &RawMedLoc) -> Result<()> {
    consume_substruct_marker(cursor)?;
    if loc.data != 0 {
        read_opt_bytes(cursor, loc.data)?; // not surfaced in v1's model - see README
    }
    Ok(())
}

/// v1 doesn't surface per-port VLAN/PPVID/PI TLVs in the model (see the
/// crate README's scope notes), but their chunks are still on the wire and
/// must be walked correctly so parsing stays in sync for whatever follows.
fn discard_vlan_chain(cursor: &mut Cursor) -> Result<()> {
    let raw: RawVlan = read_chunk_pod(cursor)?;
    if raw.tqe_next != 0 {
        discard_vlan_chain(cursor)?;
    }
    read_opt_cstring(cursor, raw.v_name)?;
    Ok(())
}

fn discard_ppvid_chain(cursor: &mut Cursor) -> Result<()> {
    let raw: RawPpvid = read_chunk_pod(cursor)?;
    if raw.tqe_next != 0 {
        discard_ppvid_chain(cursor)?;
    }
    Ok(())
}

fn discard_pi_chain(cursor: &mut Cursor) -> Result<()> {
    let raw: RawPi = read_chunk_pod(cursor)?;
    if raw.tqe_next != 0 {
        discard_pi_chain(cursor)?;
    }
    read_opt_bytes(cursor, raw.p_pi)?;
    Ok(())
}

/// The fields of a `lldpd_port` that come after its own raw body and TQE
/// pointer, shared verbatim between the local port (`h_lport`, embedded) and
/// every neighbor entry (`h_rports`, pointer-chained) - both are the same C
/// type and are marshaled the same way from this point on.
struct PortFields {
    chassis: Option<Arc<Chassis>>,
    id: Option<Vec<u8>>,
    description: Option<String>,
}

fn decode_port_fields(
    cursor: &mut Cursor,
    raw: &RawPort,
    chassis_cache: &mut ChassisCache,
) -> Result<PortFields> {
    let chassis = if raw.p_chassis == 0 {
        None
    } else if let Some(cached) = chassis_cache.get(&raw.p_chassis) {
        Some(cached.clone())
    } else {
        let decoded = Arc::new(decode_chassis(cursor)?);
        chassis_cache.insert(raw.p_chassis, decoded.clone());
        Some(decoded)
    };
    // p_lastframe is IGNORE'd upstream: no chunk on the wire for it.
    let id = read_opt_bytes(cursor, raw.p_id)?;
    let description = read_opt_cstring(cursor, raw.p_descr)?;
    read_opt_cstring(cursor, raw.p_vlan_advertise_pattern)?; // not surfaced in v1

    for loc in &raw.p_med_location {
        decode_med_location_marker(cursor, loc)?;
    }
    if raw.p_vlans_tqh_first != 0 {
        discard_vlan_chain(cursor)?;
    }
    if raw.p_ppvids_tqh_first != 0 {
        discard_ppvid_chain(cursor)?;
    }
    if raw.p_pids_tqh_first != 0 {
        discard_pi_chain(cursor)?;
    }

    Ok(PortFields {
        chassis,
        id,
        description,
    })
}

fn decode_neighbor_chain(
    cursor: &mut Cursor,
    chassis_cache: &mut ChassisCache,
) -> Result<Vec<Neighbor>> {
    let raw: RawPort = read_chunk_pod(cursor)?;
    let rest = if raw.tqe_next != 0 {
        decode_neighbor_chain(cursor, chassis_cache)?
    } else {
        Vec::new()
    };
    let mut out = vec![build_neighbor(cursor, &raw, chassis_cache)?];
    out.extend(rest);
    Ok(out)
}

/// Finishes decoding one already-chunk-read `lldpd_port` into a [`Neighbor`]:
/// reads its remaining fields (chassis, id, description, ...) and requires a
/// chassis to be present, since a neighbor entry without one would be a
/// protocol invariant violation on lldpd's side.
fn build_neighbor(
    cursor: &mut Cursor,
    raw: &RawPort,
    chassis_cache: &mut ChassisCache,
) -> Result<Neighbor> {
    let fields = decode_port_fields(cursor, raw, chassis_cache)?;
    let Some(chassis) = fields.chassis else {
        return Err(Error::Protocol(
            "neighbor port has no associated chassis".into(),
        ));
    };
    Ok(Neighbor {
        chassis,
        port_id_subtype: raw.p_id_subtype.into(),
        port_id: fields.id.unwrap_or_default(),
        port_description: fields.description,
        ttl: raw.p_ttl,
    })
}

pub(crate) fn decode_hardware(payload: &[u8]) -> Result<InterfaceDetails> {
    let mut cursor = Cursor::new(payload);
    let raw: RawHardware = read_chunk_pod(&mut cursor)?;
    let mut chassis_cache = ChassisCache::new();

    consume_substruct_marker(&mut cursor)?;
    if raw.h_lport.tqe_next != 0 {
        return Err(Error::Protocol(
            "local port unexpectedly chained to another port".into(),
        ));
    }
    let local_fields = decode_port_fields(&mut cursor, &raw.h_lport, &mut chassis_cache)?;

    let neighbors = if raw.h_rports_tqh_first != 0 {
        decode_neighbor_chain(&mut cursor, &mut chassis_cache)?
    } else {
        Vec::new()
    };

    let alias = read_opt_cstring(&mut cursor, raw.h_ifalias)?;

    let name_len = raw
        .h_ifname
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(raw.h_ifname.len());
    let name = String::from_utf8_lossy(&raw.h_ifname[..name_len]).into_owned();

    Ok(InterfaceDetails {
        name,
        alias,
        mac_address: raw.h_lladdr,
        local_chassis: local_fields.chassis,
        neighbors,
    })
}

/// `NOTIFICATION`'s payload: `lldpd_neighbor_change`. Its `neighbor` is a
/// `pointer`-kind field (unlike `h_lport`'s embedded `substruct`), so it gets
/// a real chunk of its own - but `src/daemon/event.c`'s `levent_ctl_notify`
/// explicitly zeroes the port's `p_entries` before serializing it (to avoid
/// dragging the rest of that interface's neighbor list along), so we can
/// assert it's never chained the way a `GET_INTERFACE` neighbor list is.
pub(crate) fn decode_neighbor_change(payload: &[u8]) -> Result<NeighborChange> {
    let mut cursor = Cursor::new(payload);
    let raw: RawNeighborChange = read_chunk_pod(&mut cursor)?;
    let interface = read_opt_cstring(&mut cursor, raw.ifname)?.unwrap_or_default();
    let interface_alias = read_opt_cstring(&mut cursor, raw.ifalias)?;

    let neighbor = if raw.neighbor == 0 {
        None
    } else {
        let port_raw: RawPort = read_chunk_pod(&mut cursor)?;
        if port_raw.tqe_next != 0 {
            return Err(Error::Protocol(
                "notified neighbor unexpectedly chained to another port".into(),
            ));
        }
        let mut chassis_cache = ChassisCache::new();
        Some(build_neighbor(&mut cursor, &port_raw, &mut chassis_cache)?)
    };

    Ok(NeighborChange {
        interface,
        interface_alias,
        kind: raw.state.into(),
        neighbor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NeighborChangeKind;
    use crate::model::{ChassisIdSubtype, PortIdSubtype};
    use crate::wire::raw::{
        RawHardware, RawInterface, RawInterfaceList, RawNeighborChange, RawPort,
    };

    /// Reinterprets a `#[repr(C)]` `Raw*` value as its own wire bytes - the
    /// exact inverse of [`super::super::cursor::read_pod`], so a test can
    /// build known-good messages without hand-computing offsets.
    fn raw_bytes<T: Copy>(v: &T) -> Vec<u8> {
        unsafe {
            std::slice::from_raw_parts((v as *const T).cast::<u8>(), std::mem::size_of::<T>())
                .to_vec()
        }
    }

    /// A little builder for hand-crafted wire messages, mirroring what a
    /// real `marshal_serialize_` call produces.
    #[derive(Default)]
    struct Buf(Vec<u8>);

    impl Buf {
        fn pad(&mut self) {
            let ptr_size = std::mem::size_of::<usize>();
            let pad = (ptr_size - self.0.len() % ptr_size) % ptr_size;
            self.0.extend(std::iter::repeat_n(0u8, pad));
        }

        /// Builds one chunk the way real `marshal_serialize_` output does:
        /// the declared size is the header's own length plus the content
        /// length, not just the content length - see
        /// `wire::cursor::Cursor::chunk_header`. None of these test fixtures
        /// nest further chunks inside a `chunk_pod`/`marker` call, so this is
        /// exactly the declared size a real sender would produce for them
        /// too (no need to separately account for nested content here).
        fn chunk(&mut self, orig: usize, body: &[u8]) -> &mut Self {
            self.pad();
            let header_len = 2 * std::mem::size_of::<usize>();
            self.0.extend_from_slice(&orig.to_ne_bytes());
            self.0
                .extend_from_slice(&(header_len + body.len()).to_ne_bytes());
            self.0.extend_from_slice(body);
            self
        }

        fn chunk_pod<T: Copy>(&mut self, orig: usize, v: &T) -> &mut Self {
            let bytes = raw_bytes(v);
            self.chunk(orig, &bytes)
        }

        fn marker(&mut self, orig: usize) -> &mut Self {
            self.chunk(orig, &[])
        }

        fn cstring(&mut self, orig: usize, s: &str) -> &mut Self {
            let mut b = s.as_bytes().to_vec();
            b.push(0);
            self.chunk(orig, &b)
        }

        fn into_vec(self) -> Vec<u8> {
            self.0
        }
    }

    #[test]
    fn decode_interfaces_empty_list() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawInterfaceList {
                tqh_first: 0,
                tqh_last: 0,
            },
        );
        assert_eq!(decode_interfaces(&buf.into_vec()).unwrap(), Vec::new());
    }

    #[test]
    fn decode_interfaces_two_entries_in_list_order() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawInterfaceList {
                tqh_first: 100,
                tqh_last: 0,
            },
        );
        // eth0 -> eth1 (`next` is processed before `name`/`alias`, so eth1's
        // whole subtree is nested inside eth0's on the wire).
        buf.chunk_pod(
            2,
            &RawInterface {
                next_tqe_next: 200,
                next_tqe_prev: 0,
                name: 300,
                alias: 0,
            },
        );
        buf.chunk_pod(
            3,
            &RawInterface {
                next_tqe_next: 0,
                next_tqe_prev: 0,
                name: 400,
                alias: 500,
            },
        );
        buf.cstring(4, "eth1");
        buf.cstring(5, "wan");
        buf.cstring(6, "eth0");

        let result = decode_interfaces(&buf.into_vec()).unwrap();
        assert_eq!(
            result,
            vec![
                Interface {
                    name: "eth0".into(),
                    alias: None
                },
                Interface {
                    name: "eth1".into(),
                    alias: Some("wan".into())
                },
            ]
        );
    }

    /// A real `GET_INTERFACES` response, captured with `strace` from `lldpcli`
    /// talking to a real `lldpd` 1.0.22 on an aarch64 device (`wlan0` and
    /// `eth0`, neither with an alias). This is what actually caught this
    /// crate's marshal-protocol misunderstanding - `size` header fields
    /// hand-built in the tests above happened to be internally consistent
    /// with the (wrong) decoder, so nothing here exercised real `lldpd`
    /// output until this was captured. Keep this as a real fixture, not
    /// hand-built, precisely so a future regression here can't hide the same
    /// way.
    #[test]
    fn decode_interfaces_matches_a_real_lldpd_capture() {
        let payload: &[u8] = b"\x01\x00\x00\x00\x00\x00\x00\x00\xad\x00\x00\x00\x00\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x60\x0a\x14\xb3\x55\x00\x00\x00\x02\x00\x00\x00\x00\x00\x00\x00\x8d\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\xf0\x60\x6a\xdb\x7f\x00\x00\x00\x05\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x46\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x90\xb0\x13\xb3\x55\x00\x00\x00\x04\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x04\x00\x00\x00\x00\x00\x00\x00\x16\x00\x00\x00\x00\x00\x00\x00\x77\x6c\x61\x6e\x30\x00\x00\x00\x05\x00\x00\x00\x00\x00\x00\x00\x15\x00\x00\x00\x00\x00\x00\x00\x65\x74\x68\x30\x00";
        assert_eq!(payload.len(), 173);

        let result = decode_interfaces(payload).unwrap();
        assert_eq!(
            result,
            vec![
                Interface {
                    name: "eth0".into(),
                    alias: None
                },
                Interface {
                    name: "wlan0".into(),
                    alias: None
                },
            ]
        );
    }

    /// A minimal `RawPort` with every pointer field null - callers flip on
    /// just the fields their test cares about.
    fn empty_port() -> RawPort {
        RawPort::default()
    }

    #[test]
    fn decode_hardware_local_only_no_neighbors() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawHardware {
                h_ifname: *b"eth0\0\0\0\0\0\0\0\0\0\0\0\0",
                ..Default::default()
            },
        );
        // h_lport marker, then its (all-absent) fields: still 3 med_location
        // markers, since that's a fixed-size array, not a list.
        buf.marker(2);
        buf.marker(3);
        buf.marker(4);
        buf.marker(5);
        // h_rports_tqh_first == 0: no neighbor chain follows.
        // h_ifalias == 0: no alias chunk follows.

        let details = decode_hardware(&buf.into_vec()).unwrap();
        assert_eq!(details.name, "eth0");
        assert_eq!(details.alias, None);
        assert!(details.local_chassis.is_none());
        assert!(details.neighbors.is_empty());
    }

    #[test]
    fn decode_hardware_one_neighbor_with_chassis_and_mgmt_address() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawHardware {
                h_ifname: *b"eth0\0\0\0\0\0\0\0\0\0\0\0\0",
                h_rports_tqh_first: 6,
                ..Default::default()
            },
        );
        buf.marker(2); // h_lport marker
        buf.marker(3); // h_lport.p_med_location[0]
        buf.marker(4); // [1]
        buf.marker(5); // [2]
                       // h_rports: one neighbor.
        buf.chunk_pod(
            6,
            &RawPort {
                p_chassis: 999,
                p_id_subtype: 3, // MacAddress (LLDP_PORTID_SUBTYPE_LLADDR - port and chassis subtypes are numbered differently upstream)
                p_id: 700,
                p_descr: 800,
                p_ttl: 120,
                ..empty_port()
            },
        );
        // Neighbor's chassis (first, since chassis is processed before id/descr).
        buf.chunk_pod(
            999,
            &RawChassis {
                c_id_subtype: 4, // MacAddress
                c_id: 1000,
                c_name: 1100,
                c_descr: 0,
                c_mgmt_tqh_first: 1200,
                ..Default::default()
            },
        );
        buf.chunk(1000, &[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]);
        buf.cstring(1100, "switch1");
        buf.chunk_pod(
            1200,
            &RawMgmt {
                m_family: 1,
                m_addr: {
                    let mut a = [0u8; 16];
                    a[0..4].copy_from_slice(&[192, 0, 2, 1]);
                    a
                },
                m_addrsize: 4,
                m_iface: 3,
                ..Default::default()
            },
        );
        // (no MED strings on this chassis - all pointer fields left at 0)
        buf.chunk(700, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        buf.cstring(800, "GigabitEthernet0/1");
        buf.marker(1300); // neighbor's p_med_location[0]
        buf.marker(1301);
        buf.marker(1302);

        let details = decode_hardware(&buf.into_vec()).unwrap();
        assert_eq!(details.neighbors.len(), 1);
        let n = &details.neighbors[0];
        assert_eq!(n.ttl, 120);
        assert_eq!(n.port_id_subtype, PortIdSubtype::MacAddress);
        assert_eq!(n.port_id, vec![0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(n.port_description.as_deref(), Some("GigabitEthernet0/1"));
        assert_eq!(n.chassis.id_subtype, ChassisIdSubtype::MacAddress);
        assert_eq!(n.chassis.id, vec![0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]);
        assert_eq!(n.chassis.name.as_deref(), Some("switch1"));
        assert_eq!(n.chassis.management_addresses.len(), 1);
        assert_eq!(
            n.chassis.management_addresses[0].ip(),
            Some("192.0.2.1".parse().unwrap())
        );
        assert_eq!(n.chassis.management_addresses[0].interface_index, 3);
    }

    #[test]
    fn decode_hardware_two_neighbors_sharing_one_chassis() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawHardware {
                h_ifname: *b"eth0\0\0\0\0\0\0\0\0\0\0\0\0",
                h_rports_tqh_first: 6,
                ..Default::default()
            },
        );
        buf.marker(2);
        buf.marker(3);
        buf.marker(4);
        buf.marker(5);

        // Two neighbor ports (LLDP + CDP entries for the same physical
        // switch) both pointing at the same chassis id (777).
        buf.chunk_pod(
            6,
            &RawPort {
                tqe_next: 100,
                p_chassis: 777,
                p_id_subtype: 5,
                p_id: 900,
                p_ttl: 60,
                ..empty_port()
            },
        );
        buf.chunk_pod(
            100,
            &RawPort {
                p_chassis: 777,
                p_id_subtype: 5,
                p_id: 901,
                p_ttl: 60,
                ..empty_port()
            },
        );
        // Per marshal order, the *second* (innermost/recursed-into-first)
        // neighbor's chassis chunk is the one that actually appears on the
        // wire; the first neighbor's identical `p_chassis` value is a cache
        // hit and contributes no further chunk.
        buf.chunk_pod(
            777,
            &RawChassis {
                c_id_subtype: 6,
                c_id: 950,
                c_name: 0,
                ..Default::default()
            },
        );
        buf.chunk(950, b"switch-1");
        buf.cstring(901, "eth2");
        buf.marker(1400);
        buf.marker(1401);
        buf.marker(1402);
        buf.cstring(900, "eth1");
        buf.marker(1500);
        buf.marker(1501);
        buf.marker(1502);

        let details = decode_hardware(&buf.into_vec()).unwrap();
        assert_eq!(details.neighbors.len(), 2);
        assert!(Arc::ptr_eq(
            &details.neighbors[0].chassis,
            &details.neighbors[1].chassis
        ));
        assert_eq!(details.neighbors[0].chassis.id, b"switch-1");
    }

    #[test]
    fn decode_hardware_rejects_truncated_payload() {
        let err = decode_hardware(&[0u8; 4]).unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
    }

    #[test]
    fn decode_neighbor_change_deleted_with_no_port_data() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawNeighborChange {
                ifname: 100,
                ifalias: 0,
                state: -1,
                neighbor: 0,
            },
        );
        buf.cstring(100, "eth0");

        let change = decode_neighbor_change(&buf.into_vec()).unwrap();
        assert_eq!(change.interface, "eth0");
        assert_eq!(change.interface_alias, None);
        assert_eq!(change.kind, NeighborChangeKind::Deleted);
        assert!(change.neighbor.is_none());
    }

    #[test]
    fn decode_neighbor_change_added_with_full_neighbor() {
        let mut buf = Buf::default();
        buf.chunk_pod(
            1,
            &RawNeighborChange {
                ifname: 100,
                ifalias: 0,
                state: 1,
                neighbor: 200,
            },
        );
        buf.cstring(100, "eth0");
        // The notified port's own p_entries is zeroed by lldpd before
        // serializing (see decode_neighbor_change's doc comment), so
        // tqe_next is 0 here - never a chain.
        buf.chunk_pod(
            200,
            &RawPort {
                p_chassis: 300,
                p_id_subtype: 5,
                p_id: 400,
                p_ttl: 90,
                ..empty_port()
            },
        );
        buf.chunk_pod(
            300,
            &RawChassis {
                c_id_subtype: 6,
                c_id: 500,
                c_name: 600,
                ..Default::default()
            },
        );
        buf.chunk(500, b"switch-x");
        buf.cstring(600, "switch-x-name");
        buf.chunk(400, b"eth5");
        buf.marker(700);
        buf.marker(701);
        buf.marker(702);

        let change = decode_neighbor_change(&buf.into_vec()).unwrap();
        assert_eq!(change.interface, "eth0");
        assert_eq!(change.kind, NeighborChangeKind::Added);
        let neighbor = change.neighbor.unwrap();
        assert_eq!(neighbor.ttl, 90);
        assert_eq!(neighbor.port_id, b"eth5");
        assert_eq!(neighbor.chassis.id, b"switch-x");
        assert_eq!(neighbor.chassis.name.as_deref(), Some("switch-x-name"));
    }
}
