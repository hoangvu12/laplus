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

## Comments

### The size question is already answered: 21.25 MB, inside the target

Measured ahead of this ticket by a throwaway spike, because this is the
measurement that decides whether the project achieved its purpose and it was
scheduled last — so the answer would have arrived after every decision it could
inform. Ticket 23 says the shell "can be pulled earlier — the only hard
requirement is a running server", true since ticket 03, so the Rust half was
measurable now and a miss would have been worth knowing before building the
shell for real.

**Context pointer:** branch `spike/tauri-weight`, commit `62d017b`. The spike is
deliberately not on `master`; `spike-tauri-weight/README.md` on that branch
carries the full method and figures. What follows is the verdict only.

| | size |
|---|---|
| Server alone, cargo default release profile | 5.83 MB |
| Server alone, shipped profile | 2.52 MB |
| Server + Tauri, stub UI | 4.29 MB |
| **Server + Tauri + full UI embedded** | **21.25 MB** |
| Web assets shipped (`t3code/apps/web/dist`, excluding `.map`) | 16.90 MB / 407 files |

Shipped profile is `strip`, `lto`, `opt-level = "z"`, `panic = "abort"`,
`codegen-units = 1`. Both binaries measured under it — comparing a tuned binary
against a cargo-default one would have flattered Tauri by 3.3 MB.

**21.25 MB against a 20–30 MB target and upstream's 318 MB — about 15× smaller,
and inside the range before an installer compresses anything.** The project's
rationale holds.

Three things this ticket should carry forward:

- **Tauri's own overhead is 1.77 MB**, and is not worth optimising.
- **The UI is 80% of the artifact** (16.96 MB of 21.25 MB). Every size
  conclusion from here is a conclusion about the web bundle; the Rust side is
  not the risk.
- **Embedding does not compress** — `dist/` is 16.90 MB on disk and adds
  16.96 MB to the binary, so anything trimmed from `dist/` comes off the
  artifact one-for-one. Two contributors dominate: `assets/index-*.js` at
  3.40 MB, and 3.45 MB of syntax-highlighting grammar chunks across 53 files
  for languages this UI may never render. That is a third of the artifact.
  Not acted on — trimming vendored upstream code needs its own decision.

Still open here, and needing real machines or `cargo tauri` (not installed):
the NSIS installer figure, a clean-machine install, and launching where the
WebView2 runtime is absent. 21.25 MB is a genuine upper bound on the download,
since NSIS only compresses — gzip alone takes the same `.exe` to 6.31 MB.

### The real shell now exists, and weighs 24.16 MB

Ticket 23 built it, so the figure above is no longer a spike's. `cargo build
--release -p lightcode-shell` produces one `lightcode.exe` at **24.16 MB** —
against the spike's 21.25 MB for a stub. Still inside the 20–30 MB target, still
about 13× smaller than upstream's 318 MB, with **5.84 MB of headroom**.

The gap to the spike is two things, and only one of them is code. The real
server's full surface is part of it; the rest is **1.95 MB** of unwinding tables,
because ticket 23 dropped `panic = "abort"` from the shipped profile on purpose.
Aborting orphans the agent processes and shells on any panic, which is the leak
ticket 23 is required to prevent. That reasoning is in the root `Cargo.toml`
beside the setting; this ticket should not quietly put it back to make a number
look better.

The shipped profile is now the workspace's own `[profile.release]`, so this
figure is what an ordinary release build gives and needs no reproducing by hand.

What this ticket still has to do is unchanged: the NSIS installer figure, the
installed-on-disk footprint, a clean-machine install and a machine with no
WebView2. `cargo tauri` is still not installed. `bundle.targets` and
`webviewInstallMode: downloadBootstrapper` are configured in
`crates/lightcode-shell/tauri.conf.json` and untried.

The copyright item has a start: `bundle.copyright` in that file carries
upstream's MIT notice. Whether that is *enough* — `THIRD_PARTY_NOTICES.md` and
the `LICENSE` file are both in the vendored checkout and neither is shipped — is
this ticket's to settle.

### This ticket must be told which line count to report

The checklist item above says to report the Rust server's line count against the
spec's ~20K signal, which spec line 434 calls "the signal to stop and
re-evaluate". A naive `wc -l` over `crates/lightcode-server/src` prints **31,351
and trips that alarm**. Measured with 21 of 25 tickets done:

| | lines |
|---|---|
| total | 31,351 |
| comments | 8,145 |
| `#[cfg(test)]` unit tests | 9,708 |
| blank | 1,582 |
| **production code** | **~11,900** |

Plus 16,205 lines of integration tests in `tests/`, which are not server code
either. So the real figure is **~12K against a 20K signal — comfortably
inside**. Report the total and this build writes a false alarm into itself.
Settle which number is reported before starting this ticket, not during.
