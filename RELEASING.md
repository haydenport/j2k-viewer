# Releasing

Releases are built by GitHub Actions (`.github/workflows/release.yml`) when a `v*` tag is pushed.
It builds Windows x86_64, Linux x86_64 (on Ubuntu 22.04 for older glibc), Linux aarch64 and a
best-effort Windows aarch64 build (allowed to fail without blocking the release), packages them as
`j2k-viewer-<version>-<os>-<arch>.zip` / `.tar.gz`, builds the Windows installers
(`j2k-viewer-<version>-windows-<arch>-setup.exe`, with Inno Setup via `scripts/build-installer.ps1`),
writes `SHA256SUMS.txt` and creates the GitHub Release. Tags with a hyphen (e.g. `v0.2.0-rc1`) are
marked pre-release. To build the installer locally: `scripts/build-installer.ps1`.

## Steps

1. Land everything on `main` and make sure CI is green.
2. In `CHANGELOG.md` rename the `## Unreleased` heading to `## 0.2.0` (or add that section) and
   commit it.
3. From a clean `main` checkout run (PowerShell):

   ```
   scripts/release.ps1 -Version 0.2.0
   ```

   It refuses to run on a dirty tree or off `main`, checks the changelog section, bumps
   `[workspace.package] version` in `Cargo.toml`, refreshes `Cargo.lock`, runs
   `cargo test --workspace`, commits "Release v0.2.0", creates the annotated tag `v0.2.0`, and,
   after you confirm (or with `-Yes`), pushes the commit and the tag.
4. Watch it: `gh run watch` (or `gh run list --workflow release.yml`), then check the release page
   (`gh release view v0.2.0`).

The workflow fails early if the tag does not match the version in `Cargo.toml`.

## If the release build fails

* Fix forward: commit the fix to `main`, then cut a new patch version (`-Version 0.2.1`, with a
  changelog entry).
* Only if no one has downloaded the release yet, you may reuse the version: delete the release and
  tag, then recreate and push the tag on the fixed commit:

  ```
  gh release delete v0.2.0 --cleanup-tag --yes   # if a release was created
  git tag -d v0.2.0
  git tag -a v0.2.0 -m "Release v0.2.0"
  git push origin v0.2.0
  ```

  Make sure `Cargo.toml` still says `0.2.0` on the commit you tag.
* A failed Windows aarch64 job is expected to be ignorable; the release is still created.
* `workflow_dispatch` on the Release workflow (with an existing tag as input) re-runs the build and
  release for that tag.
