#!/usr/bin/env bash
# Build the Linux binaries in a container, run the tests and the pixel-exact smoke test, and copy
# the results to dist/linux-<arch>/. Run from the repository root (inside WSL2 on Windows).
#
#   scripts/build-linux.sh                      # x86_64
#   PLATFORM=linux/arm64 scripts/build-linux.sh # aarch64 (needs qemu-user-static / binfmt)
set -euo pipefail
cd "$(dirname "$0")/.."

engine=${CONTAINER_ENGINE:-podman}
platform=${PLATFORM:-}
arch=${platform#linux/}
arch=${arch:-$(uname -m)}
[ "$arch" = arm64 ] && arch=aarch64
[ "$arch" = amd64 ] && arch=x86_64
tag=j2k-viewer-linux-$arch
out=dist/linux-$arch

platform_args=()
[ -n "$platform" ] && platform_args=(--platform "$platform")

echo "== build + unit tests ($arch) =="
"$engine" build "${platform_args[@]}" -t "$tag" -f Containerfile .

echo "== smoke test (software Vulkan, pixel-exact) =="
"$engine" run --rm --entrypoint /usr/local/bin/smoke.sh "${platform_args[@]}" "$tag" \
    /usr/local/bin/viewer /usr/local/bin/tools

echo "== copy binaries to $out =="
mkdir -p "$out"
id=$("$engine" create "${platform_args[@]}" "$tag")
trap '"$engine" rm -f "$id" >/dev/null' EXIT
"$engine" cp "$id:/usr/local/bin/viewer" "$out/viewer"
"$engine" cp "$id:/usr/local/bin/tools" "$out/tools"
ls -l "$out"
