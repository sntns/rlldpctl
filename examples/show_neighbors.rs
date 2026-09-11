//! Connects to a real, running lldpd and prints what it knows - the
//! equivalent of `lldpcli show neighbors`, minus the formatting.
//!
//! Not run in CI (it needs a live lldpd on the control socket); run it
//! locally with:
//!
//! ```sh
//! cargo run --example show_neighbors [socket-path]
//! ```

use rlldpctl::Client;

fn main() -> rlldpctl::Result<()> {
    let socket_path = std::env::args().nth(1);
    let mut client = match &socket_path {
        Some(path) => Client::connect_to(path)?,
        None => Client::connect()?,
    };

    for iface in client.interfaces()? {
        let details = client.interface(&iface.name)?;
        println!("{} ({:02x?})", details.name, details.mac_address);
        if let Some(chassis) = &details.local_chassis {
            println!(
                "  local chassis: {}",
                chassis.name.as_deref().unwrap_or("?")
            );
        }
        if details.neighbors.is_empty() {
            println!("  (no neighbors)");
        }
        for neighbor in &details.neighbors {
            println!(
                "  neighbor: {} port={:?} ttl={}s",
                neighbor
                    .chassis
                    .name
                    .as_deref()
                    .unwrap_or_else(|| neighbor.chassis.id_str().unwrap_or("?")),
                neighbor.port_id_str().unwrap_or("<binary>"),
                neighbor.ttl,
            );
            if let Some(descr) = &neighbor.chassis.description {
                println!("      {descr}");
            }
            for addr in &neighbor.chassis.management_addresses {
                match addr.ip() {
                    Some(ip) => println!("      management address: {ip}"),
                    None => println!(
                        "      management address: {:02x?} (unknown family)",
                        addr.raw_octets()
                    ),
                }
            }
        }
    }
    Ok(())
}
