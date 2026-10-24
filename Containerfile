# Reproducible Linux build and test environment for the viewer.
#
#   podman build -t j2k-viewer-linux -f Containerfile .                      # build + unit tests
#   podman build --platform linux/arm64 -t j2k-viewer-linux-arm64 -f Containerfile .
#
# See scripts/build-linux.sh, which also copies the binaries out and runs the smoke test.

ARG RUST_VERSION=1.99

# ---------------------------------------------------------------- build
FROM docker.io/library/rust:${RUST_VERSION}-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libxkbcommon-dev libwayland-dev libx11-dev libvulkan-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# The image already carries the pinned toolchain; do not let rustup try to fetch another.
RUN rm -f rust-toolchain.toml
RUN cargo build --release --locked -p viewer -p tools
RUN cargo test --release --locked -p decoder -p viewer

# ---------------------------------------------------------------- runtime
# Software Vulkan (lavapipe) lets the viewer and the smoke test run with no GPU, which is also the
# no-dedicated-GPU fallback path.
FROM docker.io/library/debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        libvulkan1 mesa-vulkan-drivers libxkbcommon0 libwayland-client0 libx11-6 libxcb1 libegl1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/viewer /usr/local/bin/viewer
COPY --from=build /src/target/release/tools /usr/local/bin/tools
COPY scripts/smoke.sh /usr/local/bin/smoke.sh
ENTRYPOINT ["/usr/local/bin/viewer"]
