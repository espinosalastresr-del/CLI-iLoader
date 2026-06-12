FROM ghcr.io/cross-rs/x86_64-unknown-linux-musl:edge

RUN apt-get update && apt-get install -y \
    pkg-config \
    libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*
