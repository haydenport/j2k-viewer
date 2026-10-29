<p align="center"><img src="assets/icon-256.png" width="128" alt="j2k viewer icon"></p>

# j2k viewer

A fast desktop viewer for huge, lossless JPEG 2000 / HTJ2K images (8-bit grayscale), built for very
large stitched scans and folders of them ("layers"). Decoding is pure Rust and CPU-only (the `j2k`
crate); drawing uses wgpu (Vulkan on Linux, DX12 or Vulkan on Windows, with a software adapter
fallback).

## Status and limits

This is an early release (0.1.0). Be aware of what it does and does not do:

* 8-bit grayscale images only. Other bit depths and colour images are not supported.
* Decoding runs on the CPU (background threads); there is no GPU decoding.
* Windows and Linux (x86_64 and aarch64 on Linux; Windows ARM64 builds are best-effort).
* Linux needs a Vulkan driver: `libvulkan1` plus a GPU driver, or `mesa-vulkan-drivers` for software
  rendering (lavapipe). The Open dialog needs an XDG desktop portal.
* The Windows executable is not code-signed, so Windows SmartScreen may warn the first time you run it.

## Install

**Windows (recommended):** download `j2k-viewer-<version>-windows-x86_64-setup.exe` from the
[Releases page](https://github.com/haydenport/j2k-viewer/releases) and run it. It installs for your
user account only (no administrator prompt) under `%LOCALAPPDATA%\Programs`, adds a Start menu
entry, and registers `.jp2 .j2k .j2c .jph .jhc .jpc` so j2k viewer shows up under **Open with**
and in **Settings > Apps > Default apps**. Windows does not let installers take over a file type
silently, so on the last page tick "Open Windows Default apps" (or right-click a `.jp2` and choose
Open with > Choose another app > Always) to make it the default. Uninstall from **Settings > Apps**;
that removes everything the installer added and does not touch other programs' associations.

**Portable / Linux:** use the archives instead:

1. Download the archive for your platform from the
   [Releases page](https://github.com/haydenport/j2k-viewer/releases):
   `j2k-viewer-<version>-windows-x86_64.zip`, `...-linux-x86_64.tar.gz`, `...-linux-aarch64.tar.gz`
   (or `...-windows-aarch64.zip` when available). `SHA256SUMS.txt` lists the checksums.
2. Extract it. It contains `viewer`, `tools`, the licenses and this README.
3. Run it:

```
viewer.exe <file-or-folder>      # Windows
./viewer <file-or-folder>        # Linux
```

With no argument the window opens empty; use the Menu button (top right) and Open, or press `O`.
Opening a file makes its folder the layer list; a folder opens directly.

### Controls

| Input | Action |
| --- | --- |
| `Left` / `Right`, `PageUp` / `PageDown` | previous / next layer |
| `Home` / `End` | first / last layer |
| `F` | fit to window |
| `1` | 100% (1 screen pixel per image pixel) |
| `+` / `-` | zoom |
| `O` | open a file (its folder becomes the layer list) |
| `Shift+O` | pick a folder directly |
| `I` | toggle the **image info** panel (see below) |
| `D` | toggle the debug statistics panel (cache, decode times, frame time) |
| `Esc` | quit |
| Mouse wheel | zoom at the cursor |
| Left-drag, or hold the middle (wheel) button | pan |
| Menu button (top right) | Open, image info, debug stats |
| Drop a file or folder on the window | open it |

### Image info panel

Press `I` (or Menu > Image info) to show facts about the current file, read from its headers (it
reads the structure only, so it is quick even for huge files, and it runs in the background):

* the file: JP2 wrapper and brand, or a raw codestream; file size; resolution, components and bit
  depth; colour space
* the codec: **HTJ2K (Part 15)** or classic JPEG 2000 Part 1; wavelet (5/3 reversible lossless or
  9/7 lossy); quantization; decomposition levels, quality layers and progression order
* the layout: tile grid and tile size, number of tile-parts, code-block size, precincts
* optional markers: **TLM** (tile-part lengths), **PLM** and **PLT** (packet lengths), PPM/PPT, SOP,
  EPH, MCT, and any other markers present (CAP, POC, RGN, ...)
* the bit rate and compression ratio, any encoder comment (often names the encoder), and a one-line
  verdict on how well the layout suits the viewer's decode-only-what-is-visible approach (for example
  "poor: one tile, no precincts" or "good: tiled with resolution levels")

`tools info <file>...` prints the same text on the command line.

### Other modes

```
viewer --snapshot out.png --zoom 1 --at 23552,19688 file.j2c   # offscreen render, no window
viewer --bench <folder> [--steps N --pan N --cache-mb N]        # layer step / pan / cache benchmark
tools info <file>...                                             # the image info panel's text, for any files
tools bench|verify|encode|check|mktest|cmp-snapshot ...          # decoder benchmarks and checks
```

Run `viewer --help` for all options (`--workers`, `--cache-mb`, `--prefetch`, ...). On Windows the
viewer is a GUI-subsystem program; when started from a terminal it reattaches to that terminal so
`--snapshot`, `--bench` and error messages still print.

## How it stays fast

* Only the tiles in view are decoded, at the pyramid level the zoom needs (`decode_region_scaled`),
  on background threads. A coarse overview of the whole image is always resident, so something is
  on screen immediately and detail fills in.
* Neighbouring layers are prefetched with the same view, so stepping to the next layer is
  instant. The GPU tile cache has a byte budget (`--cache-mb`) with LRU eviction.

On the author's machine (RTX 5080), a gigapixel image is on screen in well under a second.
Your numbers will depend on your CPU, disk and the file's encoding.

## Encoding files for best speed

Random access is only as good as the file's structure:

* Use power-of-two tiles such as 1024x1024.
* Use 5-6 wavelet levels.
* Avoid a tiny sliver tile at the right/bottom edge: if the encoder limits the number of levels
  to the smallest tile, a sliver cuts 5 levels down to 2 and makes zoomed-out views slow.

`tools encode in.png out.jp2` encodes a PNG to lossless HTJ2K and picks a tile size that leaves no
sliver tile.

## Build from source

You need a recent stable Rust toolchain (`rust-toolchain.toml` selects `stable`). On Linux also
install `pkg-config libxkbcommon-dev libwayland-dev libx11-dev libvulkan-dev`.

```
cargo build --release          # target/release/viewer and target/release/tools
cargo test --workspace
```

### Windows installer and icon

```
scripts/build-installer.ps1        # needs Inno Setup 6: winget install JRSoftware.InnoSetup --scope user
```

builds `dist/j2k-viewer-<version>-windows-x86_64-setup.exe` from `installer/j2k-viewer.iss`. The icon
(`assets/icon.ico`, `assets/icon-256.png`) is generated by `assets/make-icon.ps1` and embedded into
`viewer.exe` by `crates/viewer/build.rs`.

### Linux build in a container

For reproducible Linux binaries use Podman (inside WSL2 on Windows):

```
scripts/build-linux.sh                        # x86_64: build, unit tests, pixel-exact smoke test
PLATFORM=linux/arm64 scripts/build-linux.sh   # aarch64 via QEMU
```

The smoke test (`scripts/smoke.sh <viewer> <tools>`) renders with software Vulkan (lavapipe), so it
also covers the no-GPU path. Binaries land in `dist/linux-<arch>/`.

## Layout

* `crates/decoder`: tiled HTJ2K / JPEG 2000 region decoding on top of `j2k`.
* `crates/viewer`: the GUI app (`viewer`).
* `crates/tools`: command-line helpers (`tools`).
* `vendor/j2k-core`: see below.

Releases are made with `scripts/release.ps1`; see [RELEASING.md](RELEASING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.

`vendor/j2k-core` is a copy of the upstream `j2k-core` 0.12.0 crate, which is itself licensed
`MIT OR Apache-2.0` (see its `Cargo.toml` and README). It carries one small local patch: the host
allocation cap in `vendor/j2k-core/src/buffer.rs` is raised from 512 MiB to 16 GiB so gigapixel
images can be decoded. It is wired in through `[patch.crates-io]` in the root `Cargo.toml`.
