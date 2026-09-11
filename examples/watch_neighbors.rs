//! Subscribes to a real, running lldpd and prints neighbor changes as they
//! happen - the equivalent of watching `lldpcli -w` output.
//!
//! Not run in CI (it needs a live lldpd and blocks waiting for changes); run
//! it locally with:
//!
//! ```sh
//! cargo run --example watch_neighbors [socket-path]
//! ```

use rlldpctl::{Client, NeighborChangeKind};

fn main() -> rlldpctl::Result<()> {
    let socket_path = std::env::args().nth(1);
    let client = match &socket_path {
        Some(path) => Client::connect_to(path)?,
        None => Client::connect()?,
    };

    println!("subscribed - waiting for neighbor changes (Ctrl-C to stop)");
    for change in client.subscribe()? {
        let change = change?;
        let verb = match change.kind {
            NeighborChangeKind::Added => "added",
            NeighborChangeKind::Updated => "updated",
            NeighborChangeKind::Deleted => "deleted",
            NeighborChangeKind::Other(v) => {
                println!("{}: unrecognized change kind {v}", change.interface);
                continue;
            }
        };
        let who = change
            .neighbor
            .as_ref()
            .and_then(|n| {
                n.chassis
                    .name
                    .clone()
                    .or_else(|| n.chassis.id_str().map(str::to_string))
            })
            .unwrap_or_else(|| "?".to_string());
        println!("{}: {verb} {who}", change.interface);
    }
    println!("lldpd closed the subscription");
    Ok(())
}
