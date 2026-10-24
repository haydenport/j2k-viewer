#!/usr/bin/env bash
# End-to-end check: synthesise an HTJ2K image, render a 1:1 view offscreen with the viewer, and
# require the result to be pixel-identical to the source. Works with any Vulkan driver, including
# software ones (lavapipe), so it runs in the container without a GPU.
#
#   scripts/smoke.sh <viewer> <tools>
set -euo pipefail
viewer=${1:?usage: smoke.sh <viewer> <tools>}
tools=${2:?usage: smoke.sh <viewer> <tools>}
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

"$tools" mktest "$dir/t.j2c" "$dir/t.png" 3000 2000
# A 800x600 view centred on (1500,1000) at 1:1 starts at image pixel (1100,700).
"$viewer" --snapshot "$dir/s.png" --size 800x600 --zoom 1 --at 1500,1000 "$dir/t.j2c"
"$tools" cmp-snapshot "$dir/s.png" "$dir/t.png" 1100 700
