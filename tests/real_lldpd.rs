//! Integration tests against a **real** `lldpd`, as opposed to
//! `tests/integration.rs`/`tests/async_integration.rs`'s hand-rolled fake
//! server.
//!
//! These only do anything inside the container built from
//! `tests/real_lldpd/Dockerfile` (see `tests/real_lldpd/entrypoint.sh`),
//! which starts two real `lldpd` instances talking over a genuine veth pair
//! and sets `RLLDPCTL_REAL_LLDPD=1` before running `cargo test`. Outside
//! that container - a plain local `cargo test`, or the existing `build` CI
//! job - every test here no-ops immediately, since there's no real `lldpd`
//! (and no root/veth) to talk to.

use std::time::{Duration, Instant};

use rlldpctl::{ChassisIdSubtype, Client, InterfaceDetails};

const POLL_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn enabled() -> bool {
    if std::env::var_os("RLLDPCTL_REAL_LLDPD").is_none() {
        eprintln!(
            "skipping: RLLDPCTL_REAL_LLDPD not set (this test only runs inside \
             tests/real_lldpd/Dockerfile's container - see that file)"
        );
        return false;
    }
    true
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// The peer's real MAC address, read directly from the kernel - the ground
/// truth this test checks the crate's *decoded* neighbor data against,
/// independently of anything `lldpd` or this crate computed.
fn real_mac_address(iface: &str) -> [u8; 6] {
    let raw = std::fs::read_to_string(format!("/sys/class/net/{iface}/address"))
        .unwrap_or_else(|e| panic!("reading MAC address of {iface}: {e}"));
    let mut mac = [0u8; 6];
    for (i, part) in raw.trim().split(':').enumerate() {
        mac[i] = u8::from_str_radix(part, 16).expect("valid hex octet in sysfs MAC address");
    }
    mac
}

fn assert_neighbor_matches_peer(details: &InterfaceDetails, peer_iface: &str, peer_mac: [u8; 6]) {
    assert!(
        !details.neighbors.is_empty(),
        "expected at least one real LLDP neighbor on {}, got none",
        details.name
    );
    let neighbor = &details.neighbors[0];

    assert_eq!(
        neighbor.chassis.id_subtype,
        ChassisIdSubtype::MacAddress,
        "a plain veth's chassis ID should be its MAC address"
    );
    assert_eq!(
        neighbor.chassis.id, peer_mac,
        "decoded chassis ID should equal {peer_iface}'s real MAC address"
    );
    assert_eq!(
        neighbor.port_id, peer_mac,
        "decoded port ID should equal {peer_iface}'s real MAC address"
    );
}

#[test]
fn sync_client_decodes_a_real_lldpd_neighbor() {
    if !enabled() {
        return;
    }

    let sock_a = env_or("RLLDPCTL_SOCK_A", "/run/lldpd-a.sock");
    let sock_b = env_or("RLLDPCTL_SOCK_B", "/run/lldpd-b.sock");
    let iface_a = env_or("RLLDPCTL_IFACE_A", "veth-a");
    let iface_b = env_or("RLLDPCTL_IFACE_B", "veth-b");

    let mac_a = real_mac_address(&iface_a);
    let mac_b = real_mac_address(&iface_b);

    let mut client_a = Client::connect_to(&sock_a).expect("connect to lldpd A's control socket");
    let mut client_b = Client::connect_to(&sock_b).expect("connect to lldpd B's control socket");

    let details_a = poll_until_neighbor(|| client_a.interface(&iface_a));
    let details_b = poll_until_neighbor(|| client_b.interface(&iface_b));

    assert_neighbor_matches_peer(&details_a, &iface_b, mac_b);
    assert_neighbor_matches_peer(&details_b, &iface_a, mac_a);
}

/// Retries on *any* error too, not just an empty neighbor list: right after
/// `lldpd` starts, `GET_INTERFACE` can transiently answer before the
/// interface/port is fully registered - that's a startup race, not a
/// decode bug, so it shouldn't fail the test outright. Only the last
/// attempt's outcome (error or empty) is what actually gets asserted on by
/// the caller once the deadline passes.
fn poll_until_neighbor(
    mut fetch: impl FnMut() -> rlldpctl::Result<InterfaceDetails>,
) -> InterfaceDetails {
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let outcome = fetch();
        let timed_out = Instant::now() >= deadline;
        match outcome {
            Ok(details) if !details.neighbors.is_empty() => return details,
            Ok(details) if timed_out => return details,
            Err(e) if timed_out => panic!("still failing after {POLL_TIMEOUT:?}: {e}"),
            _ => {}
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(feature = "tokio")]
mod async_tests {
    use super::*;
    use rlldpctl::AsyncClient;

    /// See the sync `poll_until_neighbor`'s doc comment: retries on error too,
    /// to ride out the startup race rather than failing on a transient
    /// answer from `lldpd` before it's fully registered the interface.
    async fn poll_until_neighbor_async(client: &mut AsyncClient, ifname: &str) -> InterfaceDetails {
        let deadline = Instant::now() + POLL_TIMEOUT;
        loop {
            let outcome = client.interface(ifname).await;
            let timed_out = Instant::now() >= deadline;
            match outcome {
                Ok(details) if !details.neighbors.is_empty() => return details,
                Ok(details) if timed_out => return details,
                Err(e) if timed_out => panic!("still failing after {POLL_TIMEOUT:?}: {e}"),
                _ => {}
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    #[tokio::test]
    async fn async_client_decodes_a_real_lldpd_neighbor() {
        if !enabled() {
            return;
        }

        let sock_a = env_or("RLLDPCTL_SOCK_A", "/run/lldpd-a.sock");
        let sock_b = env_or("RLLDPCTL_SOCK_B", "/run/lldpd-b.sock");
        let iface_a = env_or("RLLDPCTL_IFACE_A", "veth-a");
        let iface_b = env_or("RLLDPCTL_IFACE_B", "veth-b");

        let mac_a = real_mac_address(&iface_a);
        let mac_b = real_mac_address(&iface_b);

        let mut client_a = AsyncClient::connect_to(&sock_a)
            .await
            .expect("connect to lldpd A's control socket");
        let mut client_b = AsyncClient::connect_to(&sock_b)
            .await
            .expect("connect to lldpd B's control socket");

        let details_a = poll_until_neighbor_async(&mut client_a, &iface_a).await;
        let details_b = poll_until_neighbor_async(&mut client_b, &iface_b).await;

        assert_neighbor_matches_peer(&details_a, &iface_b, mac_b);
        assert_neighbor_matches_peer(&details_b, &iface_a, mac_a);
    }
}
