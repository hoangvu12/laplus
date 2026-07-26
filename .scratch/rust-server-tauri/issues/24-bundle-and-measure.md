# 24 — Bundle and measure

**What to build:** A shippable Windows installer, and a number that says whether
this project achieved what it set out to. The whole effort exists to replace a
318 MB installer with one in the 20–30 MB range; that measurement becomes part of
the build rather than a thing someone checks occasionally.

If the measured artifact lands materially above the target range, that is a
finding, not a footnote — it weakens the project's rationale and makes the
Electron-pruning fallback worth reconsidering.

**Blocked by:** 23 (Tauri shell).

**Status:** ready-for-agent

- [ ] A Windows installer is produced by a repeatable build
- [ ] The installer's size is measured and reported by the build
- [ ] The installed-on-disk footprint is measured and reported alongside it
- [ ] Both figures are recorded against the 20–30 MB target and upstream's 318 MB
      baseline
- [ ] Installing on a clean Windows machine produces a working application
- [ ] The application launches on a machine where the webview runtime is absent,
      provisioning it via the bootstrapper
- [ ] The upstream copyright notice is retained in the distributed artifact
- [ ] Rust server line count is reported, so the roughly 20K LOC scope-creep signal
      is observable
- [ ] If the size target is missed, the shortfall is written up with the largest
      contributors identified
