# Linux test image: Rust plus the sandbox prerequisites (bubblewrap) and tools the
# tests call. Used by `just test-linux`; tests run as an unprivileged user.
FROM rust:latest
RUN apt-get update \
 && apt-get install -y --no-install-recommends bubblewrap git curl python3 \
 && rm -rf /var/lib/apt/lists/*
RUN useradd -m dev
USER dev
WORKDIR /src
