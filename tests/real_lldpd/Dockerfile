# Builds a real `lldpd` from source (at $LLDPD_REF - a tag like "1.0.22", or
# "master") and packages it with this crate so `tests/real_lldpd.rs` can run
# against it inside the container - see that file and `entrypoint.sh` for
# what actually exercises it.
#
# The lldpd `./configure` flags mirror lldpd/lldpd's own top-level
# `Dockerfile` (its officially-tested build recipe), except:
# - privsep runs as root/root here since this image is a throwaway test
#   environment, not a deployment - that avoids provisioning a dedicated
#   unprivileged user for no benefit.
# - every `--enable-*`/`--disable-*` protocol flag is pinned explicitly,
#   matching src/lib.rs's documented assumption (dot1/dot3/cdp/fdp/edp/sonmp/
#   lldpmed on, custom off - this workspace's Yocto `lldpd` recipe's
#   `PACKAGECONFIG`) instead of trusting upstream's own `./configure`
#   defaults. This matters: `--enable-custom` defaults to **yes** upstream,
#   which adds extra TLV fields to the wire structs this crate does not
#   know about - building without pinning it desyncs decoding entirely
#   (confirmed while building this test: real lldpd 1.0.22 with default
#   flags fails every request with "chunk declares 0 bytes, smaller than
#   its own 16-byte header").
FROM rust:1-alpine

ARG LLDPD_REF=1.0.22

RUN apk add --no-cache \
    autoconf automake libtool alpine-sdk git \
    libevent-dev libxml2-dev jansson-dev readline-dev libcap-dev \
    bsd-compat-headers iproute2

RUN git clone --depth 1 --branch "${LLDPD_REF}" https://github.com/lldpd/lldpd.git /lldpd-src \
    && cd /lldpd-src \
    && ./autogen.sh \
    && ./configure \
        --prefix=/usr/local \
        --sysconfdir=/etc \
        --without-embedded-libevent \
        --without-snmp \
        --with-xml \
        --with-privsep-user=root \
        --with-privsep-group=root \
        --with-lldpd-ctl-socket=/run/lldpd.socket \
        --with-lldpd-pid-file=/run/lldpd.pid \
        --enable-cdp \
        --enable-fdp \
        --enable-edp \
        --enable-sonmp \
        --enable-lldpmed \
        --enable-dot1 \
        --enable-dot3 \
        --disable-custom \
    && make -j"$(nproc)" \
    && make install \
    && rm -rf /lldpd-src

WORKDIR /crate
COPY . .

ENTRYPOINT ["tests/real_lldpd/entrypoint.sh"]
