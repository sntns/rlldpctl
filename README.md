# rlldpctl

A pure-Rust client for [`lldpd`](https://lldpd.github.io/)'s control socket:
list interfaces and read discovered LLDP neighbors without shelling out to
`lldpcli` or linking `liblldpctl.so`.

```rust
let mut client = rlldpctl::Client::connect()?;
for iface in client.interfaces()? {
    let details = client.interface(&iface.name)?;
    for neighbor in &details.neighbors {
        println!(
            "{}: {} ({:?})",
            iface.name,
            neighbor.chassis.name.as_deref().unwrap_or("?"),
            neighbor.port_id_str(),
        );
    }
}
```

Run it live against a real daemon with:

```sh
cargo run --example show_neighbors [socket-path]
```

## Why this exists, and why it's fragile

`lldpd` does not define a stable IPC protocol for `lldpcli`/`liblldpctl` to
talk to it with. What's actually on the wire is `lldpd`'s own internal C
structures (`struct lldpd_port`, `struct lldpd_chassis`, ...), `memcpy`'d with
a small pointer-graph-following envelope around them (`src/marshal.c` and
`src/ctl.c` upstream). This crate reimplements that envelope and mirrors the
relevant structs directly (`src/wire/raw.rs`), rather than linking
`liblldpctl.so` or shelling out to `lldpcli`.

That means it's coupled to, and only known to work with:

- **A narrow `lldpd` version window - narrower than you'd expect.** Diffing
  `src/lldpd-structs.h` across every tag from `0.9.0` (2008-ish) to `master`
  shows the framing and marshaling *mechanism* has been untouched that whole
  time, but several fields this crate actually reads are recent additions:

  | Version    | Change                                                              |
  |------------|----------------------------------------------------------------------|
  | `< 1.0.14` | no `SET_CHASSIS` in the `hmsg_type` enum -> `GET_INTERFACE` is `5`, not `6` |
  | `< 1.0.20` | no `lldpd_port.p_vlan_advertise_pattern`                            |
  | `< 1.0.21` | no `lldpd_interface.alias` / `lldpd_hardware.h_ifalias`             |
  | `master`   | unreleased `lldpd_hardware.h_flags_previous` added after `1.0.22`  |

  Net effect: **this crate is verified against `lldpd` 1.0.21 and 1.0.22
  only** (matching this workspace's Yocto-packaged `1.0.22`) - not "any
  1.0.x", and not yet whatever ships after `1.0.22`. Sending our hardcoded
  `GET_INTERFACE = 6` to anything older than `1.0.14` doesn't even fail
  loudly: the daemon just answers a different request (`GET_DEFAULT_PORT`)
  instead. A struct layout mismatch below `1.0.21` would desync parsing the
  same way.
- **The build-time feature flags `lldpd` was compiled with.** Several struct
  fields only exist under `ENABLE_DOT1` / `ENABLE_DOT3` / `ENABLE_LLDPMED` /
  `ENABLE_CUSTOM`. This crate assumes `dot1`, `dot3`, `cdp`, `fdp` and
  `lldpmed` on and `custom` off, matching this workspace's `lldpd` recipe
  (`PACKAGECONFIG ??= "cdp fdp edp sonmp lldpmed dot1 dot3"`).
- **The host's C ABI** - pointer width (handled automatically via
  `#[repr(C)]`, as long as you build for the same target `lldpd` runs on) and
  `time_t` tracking pointer width (true of the traditional glibc/musl Linux
  ABI on 32- and 64-bit; not true of a 32-bit "time64" C library).

In short: don't expect this to survive an `lldpd` upgrade unverified, and
don't expect it to work talking to some other machine's `lldpd` over a
different architecture. This is the same trade-off upstream itself accepts
between `lldpd` and `liblldpctl.so` internally - just without linking that
library. If you need a protocol that doesn't have this problem, you want the
real [`liblldpctl`](https://github.com/lldpd/lldpd/blob/master/src/lib/lldpctl.h)
C API, or shelling out to `lldpcli -f json`.

## Scope

v1 implements exactly two requests: `GET_INTERFACES` (list interfaces) and
`GET_INTERFACE` (one interface's local info + discovered neighbors). Chassis
name/description/capabilities/management addresses and LLDP-MED inventory
TLVs are decoded; per-port VLAN/PPVID/PI TLVs are walked (for correct byte
accounting) but not yet surfaced in the model. Nothing that changes daemon
state (`SET_PORT`, `SET_CONFIG`, ...) or streams live updates (`SUBSCRIBE`) is
implemented.

## Testing

- `src/wire/decode.rs` has unit tests that hand-build wire messages (using
  `#[repr(C)]` reinterpretation of the same raw structs the decoder reads,
  so they don't require a live `lldpd`) covering interface lists, a neighbor
  with a chassis and a management address, and the chassis-deduplication
  path (two neighbor ports sharing one chassis pointer, as `lldpd` does for
  a device seen via more than one discovery protocol).
- `tests/integration.rs` drives the public `Client` API over a real Unix
  socket against a hand-rolled fake `lldpd`, independently of the crate's
  own encoder.
- `examples/show_neighbors.rs` talks to a real, running `lldpd` - not run in
  CI, but useful for a manual sanity check on real hardware.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
```

## License

MIT
