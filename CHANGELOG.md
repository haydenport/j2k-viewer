# Changelog

## Unreleased

* New **Metadata** panel (`M`, or Menu > Metadata): shows the XMP metadata embedded in the current
  file (a JP2 `uuid` box), grouped by namespace in file order, values shown in full with wrapping.
  It follows layer changes, costs nothing while hidden, and works for any XMP-tagged JP2. The three
  info panels (Image info, Metadata, Debug stats) can be open at once; Metadata sits top-right under
  the Menu button, or stacks in the left column in a narrow window.
* `tools meta [--json] <file>...` prints the same metadata (text, or one JSON object per file);
  `tools attach-xmp` builds test files with an embedded packet. `--meta` snapshot flag.
* The XMP reader (`decoder::read_xmp`) walks only the top-level boxes, honours 32-bit, 64-bit and
  run-to-end box lengths, merges multiple XMP boxes, handles attribute and element property forms
  and `rdf:Seq`/`Bag`/`Alt` lists, rejects DTDs, caps packets at 16 MiB, and never panics on
  damaged input.
* Image info lists extra JP2 boxes, e.g. `Extra boxes: uuid (XMP, 2.0 KB)`.
* Accented Latin letters (Latin-1) now render in the on-screen text instead of showing as `?`.

## 0.3.0

* New **Image info** panel (`I`, or Menu > Image info): resolution, components and bit depth, JP2
  wrapper or raw codestream, HTJ2K vs classic JPEG 2000, wavelet and quantization, levels, layers,
  progression order, tile grid, tile-parts, code-blocks, precincts, whether the optional TLM, PLM,
  PLT, PPM/PPT, SOP, EPH, CAP and other markers are present, bit rate, encoder comment, and a verdict
  on random-access friendliness. It is read from the file headers on a background thread.
* The debug statistics panel is now toggled with `D` (previously `I`) and listed in the menu as
  "Debug stats". The two panels can be shown together.
* `tools info <file>...` prints the same description on the command line. The new `--info` snapshot
  flag renders the panel offscreen.

## 0.2.0

* Windows installer (`j2k-viewer-<version>-windows-<arch>-setup.exe`, built with Inno Setup by
  `scripts/build-installer.ps1` and by the release workflow): per-user install (no administrator
  prompt), Start menu entry, optional desktop shortcut, uninstaller, and registration of
  `.jp2 .j2k .j2c .jph .jhc .jpc` so the viewer appears under "Open with" and in Default apps. The
  uninstaller removes everything it added and leaves other programs' file associations alone.
* New application icon (a tile grid under a magnifying glass), embedded in `viewer.exe` together
  with Windows version information, and used as the window icon on every platform.
  `assets/make-icon.ps1` regenerates the icon files.

## 0.1.0

Initial release.

* Viewer for lossless 8-bit grayscale JPEG 2000 / HTJ2K images, including gigapixel scans.
* Visible-tile-only decoding on background threads, always-resident overview, layer prefetch,
  LRU GPU tile cache.
* Layer-folder navigation, Open dialog, drag and drop, pan/zoom with mouse and keyboard.
* `tools` CLI: encode PNG to HTJ2K, check, benchmark, test-image generation, snapshot comparison.
* Windows and Linux builds, Linux container build, pixel-exact smoke test.
