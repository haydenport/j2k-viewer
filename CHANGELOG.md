# Changelog

## 0.1.0

Initial release.

* Viewer for lossless 8-bit grayscale JPEG 2000 / HTJ2K images, including gigapixel scans.
* Visible-tile-only decoding on background threads, always-resident overview, layer prefetch,
  LRU GPU tile cache.
* Layer-folder navigation, Open dialog, drag and drop, pan/zoom with mouse and keyboard.
* `tools` CLI: encode PNG to HTJ2K, check, benchmark, test-image generation, snapshot comparison.
* Windows and Linux builds, Linux container build, pixel-exact smoke test.
