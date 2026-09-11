//! Async equivalent of `show_neighbors.rs`, using `AsyncClient` (the `tokio`
//! feature).
//!
//! Not run in CI (needs a live lldpd); run it locally with:
//!
//! ```sh
//! cargo run --example show_neighbors_async --features tokio [socket-path]
//! ```

use rlldpctl::AsyncClient;

#[tokio::main]
async fn main() -> rlldpctl::Result<()> {
    let socket_path = std::env::args().nth(1);
    let mut client = match &socket_path {
        Some(path) => AsyncClient::connect_to(path).await?,
        None => AsyncClient::connect().await?,
    };

    for iface in client.interfaces().await? {
        let details = client.interface(&iface.name).await?;
        println!("{} ({:02x?})", details.name, details.mac_address);
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
        }
    }
    Ok(())
}
