//! Async equivalent of `watch_neighbors.rs`, using `AsyncClient` (the
//! `tokio` feature).
//!
//! Not run in CI (needs a live lldpd and blocks waiting for changes); run it
//! locally with:
//!
//! ```sh
//! cargo run --example watch_neighbors_async --features tokio [socket-path]
//! ```

use rlldpctl::{AsyncClient, NeighborChangeKind};

#[tokio::main]
async fn main() -> rlldpctl::Result<()> {
    let socket_path = std::env::args().nth(1);
    let client = match &socket_path {
        Some(path) => AsyncClient::connect_to(path).await?,
        None => AsyncClient::connect().await?,
    };

    println!("subscribed - waiting for neighbor changes (Ctrl-C to stop)");
    let mut sub = client.subscribe().await?;
    while let Some(change) = sub.next_change().await? {
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
