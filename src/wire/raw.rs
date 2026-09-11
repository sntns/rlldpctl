//! Byte-for-byte mirrors of the C structures lldpd 1.0.22 puts on the wire
//! (`src/lldpd-structs.h`, upstream tag `1.0.22`).
//!
//! lldpd's control protocol does not define its own wire schema: it `memcpy`s
//! its internal C structs as-is (see the crate-level docs for why). So instead
//! of hand-deriving byte offsets, every struct below is declared `#[repr(C)]`
//! with the *same field order and types* as the C original, and Rust's C
//! layout algorithm (which follows the same ABI rules as the C compiler that
//! built lldpd, for a given pointer width) reproduces the exact same size,
//! offsets and padding automatically.
//!
//! Pointer-typed fields (`char *`, `TAILQ_ENTRY`/`TAILQ_HEAD` links, etc.) are
//! represented as plain `usize` here: this crate never dereferences the
//! sender's original addresses, it only cares whether a pointer field is zero
//! (absent) or not (a chunk follows on the wire - see [`super::cursor`]).
//!
//! These types assume lldpd was configured with `--enable-dot1 --enable-dot3
//! --enable-lldpmed --enable-cdp --enable-fdp --disable-custom`, matching this
//! project's Yocto recipe (`PACKAGECONFIG ??= "cdp fdp edp sonmp lldpmed dot1
//! dot3"`, i.e. `custom` off). A build with different `PACKAGECONFIG` flags
//! changes these struct layouts and this crate would need matching variants.

/// `struct lldpd_mgmt` (a management address, member of a chassis).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMgmt {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub m_family: i32,
    pub m_addr: [u8; 16],
    pub m_addrsize: usize,
    pub m_iface: u32,
}

/// `struct lldpd_vlan` (`ENABLE_DOT1`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawVlan {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub v_name: usize,
    pub v_vid: u16,
}

/// `struct lldpd_ppvid` (`ENABLE_DOT1`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPpvid {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub p_cap_status: u8,
    pub p_ppvid: u16,
}

/// `struct lldpd_pi` (`ENABLE_DOT1`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPi {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub p_pi: usize,
    pub p_pi_len: i32,
}

/// `struct lldpd_med_policy` (`ENABLE_LLDPMED`). Plain data, never itself
/// carries a further wire chunk.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMedPolicy {
    pub index: u8,
    pub kind: u8, // C field name is `type`, a reserved word in Rust
    pub unknown: u8,
    pub tagged: u8,
    pub vid: u16,
    pub priority: u8,
    pub dscp: u8,
}

/// `struct lldpd_med_loc` (`ENABLE_LLDPMED`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMedLoc {
    pub index: u8,
    pub format: u8,
    pub data: usize,
    pub data_len: i32,
}

/// `struct lldpd_med_power` (`ENABLE_LLDPMED`). Plain data.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawMedPower {
    pub devicetype: u8,
    pub source: u8,
    pub priority: u8,
    pub val: u16,
}

/// `struct lldpd_dot3_macphy` (`ENABLE_DOT3`). Plain data.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawDot3MacPhy {
    pub autoneg_support: u8,
    pub autoneg_enabled: u8,
    pub autoneg_advertised: u16,
    pub mau_type: u16,
}

/// `struct lldpd_dot3_power` (`ENABLE_DOT3`). Plain data.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawDot3Power {
    pub devicetype: u8,
    pub supported: u8,
    pub enabled: u8,
    pub paircontrol: u8,
    pub pairs: u8,
    pub class: u8,
    pub powertype: u8,
    pub source: u8,
    pub priority: u8,
    pub requested: u16,
    pub allocated: u16,
    pub pd_4pid: u8,
    pub requested_a: u16,
    pub requested_b: u16,
    pub allocated_a: u16,
    pub allocated_b: u16,
    pub pse_status: u16,
    pub pd_status: u8,
    pub pse_pairs_ext: u8,
    pub class_a: u8,
    pub class_b: u8,
    pub class_ext: u8,
    pub type_ext: u8,
    pub pd_load: u8,
    pub pse_max: u16,
}

/// `struct cdpv2_power` (`ENABLE_CDP || ENABLE_FDP`). Plain data.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawCdpPower {
    pub request_id: u16,
    pub management_id: u16,
}

/// `struct lldpd_chassis`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawChassis {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub c_refcount: u16,
    pub c_index: u16,
    pub c_protocol: u8,
    pub c_id_subtype: u8,
    pub c_id: usize,
    pub c_id_len: i32,
    pub c_name: usize,
    pub c_descr: usize,
    pub c_cap_available: u16,
    pub c_cap_enabled: u16,
    pub c_mgmt_tqh_first: usize,
    pub c_mgmt_tqh_last: usize,
    pub c_med_cap_available: u16,
    pub c_med_type: u8,
    pub c_med_hw: usize,
    pub c_med_fw: usize,
    pub c_med_sw: usize,
    pub c_med_sn: usize,
    pub c_med_manuf: usize,
    pub c_med_model: usize,
    pub c_med_asset: usize,
}

/// `LLDP_MED_APPTYPE_LAST` from `src/lldp-const.h`: size of `p_med_policy[]`.
pub const MED_APPTYPE_LAST: usize = 8;
/// `LLDP_MED_LOCFORMAT_LAST` from `src/lldp-const.h`: size of `p_med_location[]`.
pub const MED_LOCFORMAT_LAST: usize = 3;

/// `struct lldpd_port`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawPort {
    pub tqe_next: usize,
    pub tqe_prev: usize,
    pub p_chassis: usize,
    pub p_lastchange: isize,
    pub p_lastupdate: isize,
    pub p_lastremove: isize,
    pub p_lastframe: usize,
    pub p_protocol: u8,
    /// Packs `p_hidden_in:1, p_hidden_out:1, p_disable_rx:1, p_disable_tx:1`;
    /// not exposed via the public model (v1 only surfaces neighbor data).
    pub p_bitfield_flags: u8,
    pub p_hardware_flags: i32,
    pub p_id_subtype: u8,
    pub p_id: usize,
    pub p_id_len: i32,
    pub p_descr: usize,
    pub p_descr_force: i32,
    pub p_mfs: u16,
    pub p_ttl: u16,
    pub p_vlan_tx_tag: i32,
    pub p_vlan_tx_enabled: i32,
    pub p_vlan_advertise_pattern: usize,
    pub p_aggregid: u32,
    pub p_macphy: RawDot3MacPhy,
    pub p_power: RawDot3Power,
    pub p_med_cap_enabled: u16,
    pub p_med_policy: [RawMedPolicy; MED_APPTYPE_LAST],
    pub p_med_location: [RawMedLoc; MED_LOCFORMAT_LAST],
    pub p_med_power: RawMedPower,
    pub p_cdp_power: RawCdpPower,
    pub p_pvid: u16,
    pub p_vlans_tqh_first: usize,
    pub p_vlans_tqh_last: usize,
    pub p_ppvids_tqh_first: usize,
    pub p_ppvids_tqh_last: usize,
    pub p_pids_tqh_first: usize,
    pub p_pids_tqh_last: usize,
}

/// `struct lldpd_interface`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawInterface {
    pub next_tqe_next: usize,
    pub next_tqe_prev: usize,
    pub name: usize,
    pub alias: usize,
}

/// `TAILQ_HEAD(lldpd_interface_list, lldpd_interface)`: response to `GET_INTERFACES`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawInterfaceList {
    pub tqh_first: usize,
    pub tqh_last: usize,
}

/// `struct lldpd_hardware`: response to `GET_INTERFACE`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawHardware {
    pub h_entries_tqe_next: usize,
    pub h_entries_tqe_prev: usize,
    pub h_cfg: usize,
    pub h_recv: usize,
    pub h_sendfd: i32,
    pub h_mangle: i32,
    pub h_ops: usize,
    pub h_data: usize,
    pub h_timer: usize,
    pub h_mtu: i32,
    pub h_flags: i32,
    pub h_ifindex: i32,
    pub h_ifindex_changed: i32,
    pub h_ifname: [u8; 16], // IFNAMSIZ
    pub h_ifalias: usize,
    pub h_lladdr: [u8; 6], // ETHER_ADDR_LEN
    pub h_tx_cnt: u64,
    pub h_rx_cnt: u64,
    pub h_rx_discarded_cnt: u64,
    pub h_rx_unrecognized_cnt: u64,
    pub h_ageout_cnt: u64,
    pub h_insert_cnt: u64,
    pub h_delete_cnt: u64,
    pub h_drop_cnt: u64,
    pub h_lport_previous: usize,
    pub h_lport_previous_len: isize,
    pub h_lchassis_previous_id_subtype: u8,
    pub h_lchassis_previous_id: usize,
    pub h_lchassis_previous_id_len: i32,
    pub h_lport_previous_id_subtype: u8,
    pub h_lport_previous_id: usize,
    pub h_lport_previous_id_len: i32,
    pub h_ifdescr_previous: usize,
    pub h_lport: RawPort,
    pub h_rports_tqh_first: usize,
    pub h_rports_tqh_last: usize,
    pub h_tx_fast: i32,
}

/// `struct lldpd_neighbor_change`: payload of a `NOTIFICATION` message,
/// pushed unprompted once a client has subscribed (`SUBSCRIBE`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawNeighborChange {
    pub ifname: usize,
    pub ifalias: usize,
    pub state: i32,
    pub neighbor: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    // These don't prove byte-for-byte parity with lldpd's own C compiler
    // output (that needs the integration checks against a live daemon - see
    // the crate README), but they catch the easy mistake of a struct whose
    // size isn't a multiple of its own alignment, which `#[repr(C)]` would
    // otherwise silently pad in a way that no longer matches a flat array
    // (`p_med_policy`, `p_med_location`) stride computed by a C compiler.
    #[test]
    fn sizes_are_aligned() {
        assert_eq!(size_of::<RawMedPolicy>() % align_of::<RawMedPolicy>(), 0);
        assert_eq!(size_of::<RawMedLoc>() % align_of::<RawMedLoc>(), 0);
        assert_eq!(size_of::<RawMgmt>() % align_of::<RawMgmt>(), 0);
        assert_eq!(size_of::<RawPort>() % align_of::<RawPort>(), 0);
        assert_eq!(size_of::<RawHardware>() % align_of::<RawHardware>(), 0);
    }

    fn align_of<T>() -> usize {
        std::mem::align_of::<T>()
    }
}
