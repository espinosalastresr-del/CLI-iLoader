FROM ghcr.io/cross-rs/x86_64-unknown-linux-musl:main

# Dependencias del sistema necesarias para libdbus-sys
RUN apk add --no-cache \
    dbus \
    dbus-dev \
    pkgconf \
    pkgconfig
