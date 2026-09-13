#!/bin/sh
# Sets up a real point-to-point link (a veth pair - not a bridge, which would
# silently drop LLDP's 01:80:C2:00:00:0E multicast destination) and starts
# two independent real `lldpd` instances, one per end, then runs
# `tests/real_lldpd.rs` against both of their control sockets.
set -eu

ip link add veth-a type veth peer name veth-b
ip link set veth-a up
ip link set veth-b up

# lldpd daemonizes itself by default (no -d), so these return once each
# instance is up.
lldpd -I veth-a -u /run/lldpd-a.sock -p /run/lldpd-a.pid
lldpd -I veth-b -u /run/lldpd-b.sock -p /run/lldpd-b.pid

export RLLDPCTL_REAL_LLDPD=1
export RLLDPCTL_SOCK_A=/run/lldpd-a.sock
export RLLDPCTL_SOCK_B=/run/lldpd-b.sock
export RLLDPCTL_IFACE_A=veth-a
export RLLDPCTL_IFACE_B=veth-b

exec cargo test --release --features tokio --test real_lldpd -- --nocapture
