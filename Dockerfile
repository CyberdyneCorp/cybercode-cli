# syntax=docker/dockerfile:1
# Cyber Code container (`installation-upgrade` → Container and CI usage).
#
# The image copies a prebuilt static binary, so building it needs no Rust toolchain:
#   mkdir -p dist/amd64 && tar -xzf cyber-<v>-x86_64-unknown-linux-musl.tar.gz -C dist/amd64
#   docker build -t cyber .
#   docker run --rm -v "$PWD:/work" cyber exec "explain this repository"
# dist/<arch>/ uses Docker's TARGETARCH names (amd64, arm64).
FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8

ARG TARGETARCH

# The base image is pinned by digest; its packages take bookworm's security updates.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git ripgrep bubblewrap \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 10001 cyber \
    && useradd --uid 10001 --gid 10001 --create-home --shell /bin/sh cyber \
    && install -d -o cyber -g cyber /work

COPY --chmod=0755 dist/${TARGETARCH}/cyber dist/${TARGETARCH}/cyber-sandbox-exec /usr/local/bin/

ENV CYBER_HOME=/home/cyber/.cyber \
    CI=true

USER cyber
WORKDIR /work
ENTRYPOINT ["cyber"]
