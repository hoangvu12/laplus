# 24 — Bundle and measure

**What to build:** A shippable Windows installer, and a number that says whether
this project achieved what it set out to. The whole effort exists to replace a
318 MB installer with one in the 20–30 MB range; that measurement becomes part of
the build rather than a thing someone checks occasionally.

If the measured artifact lands materially above the target range, that is a
finding, not a footnote — it weakens the project's rationale and makes the
Electron-pruning fallback worth reconsidering.

**Blocked by:** 23 (Tauri shell).

**Status:** ready-for-human

- [x] A Windows installer is produced by a repeatable build
- [x] The installer's size is measured and reported by the build
- [x] The installed-on-disk footprint is measured and reported alongside it
- [x] Both figures are recorded against the 20–30 MB target and upstream's 318 MB
      baseline
- [ ] Installing on a clean Windows machine produces a working application
      — installed and working on *this* machine; "clean" needs one this agent
      cannot provision
- [ ] The application launches on a machine where the webview runtime is absent,
      provisioning it via the bootstrapper — same, and the same reason
- [x] The upstream copyright notice is retained in the distributed artifact
- [x] Rust server line count is reported, so the roughly 20K LOC scope-creep signal
      is observable
- [x] If the size target is missed, the shortfall is written up with the largest
      contributors identified — not missed, and the build says so when it is

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

### The installer exists, and it is 5.05 MB

`cargo tauri build` was never run before this ticket because the CLI was not
installed. It is now, and the answer it gives is the one figure nobody had:

| | size | against 20–30 MB |
|---|---|---|
| **Installer (NSIS)** | **5.05 MB** | 14.95 MB *under* the range |
| **Installed on disk** | **24.27 MB** | inside, 5.73 MB of headroom |
| Application binary | 24.19 MB | inside, 5.81 MB of headroom |

(The installer moves by a kilobyte or two between builds — 5.04 and 5.05 both
appeared. NSIS is not reproducible byte for byte and nothing here needs it to
be.)

**The download is 5.05 MB — 63× smaller than upstream's 318 MB**, and what it
puts on disk is 24.27 MB, 13× smaller. The earlier comment called 21.25 MB "a
genuine upper bound on the download, since NSIS only compresses", and that was
right and very conservative: gzip managed 6.31 MB on the stub, NSIS's solid
LZMA manages 5.05 MB on a binary three megabytes larger. The project's rationale
does not merely hold; the number that matters most — what a developer waits for
before they can try this — beats the target by a factor of four.

The installed figure counts **what the installer added**, not what is in the
directory. That distinction turned out to matter and is ticket 30.

### The measurement is now the build

`cargo xtask release` runs `cargo tauri build`, then weighs the installer,
weighs what gets installed, counts the Rust, checks upstream's licence is still
shipping, and writes `docs/artifact-size.md`. There is no build-without-measure
flag, because the version of this that gets skipped is the version where someone
has to remember — which is what "part of the build rather than a thing someone
checks occasionally" is asking for.

**Being precise about how strong that is**, since the criterion is about
inevitability and this is a convention: `cargo tauri build` still exists and
still produces an installer with nothing measured. What stops that being the
normal path is documentation, not the toolchain — `CLAUDE.md` names
`cargo xtask release` as how a release is made, and nothing in this repo runs a
release build any other way. There is no CI here to enforce it. A real gate
would need one, and this project has no remote to run it on.

What *is* enforced, in `cargo test`: the licence check against the real
configuration, and the line classifier against the real 32,000 lines. Those were
the two that could rot silently between releases.

The arithmetic is in `xtask/`, under `cargo test` with the server: what counts
as production code (`loc`), what the target range means (`size`), whether the
licence still ships (`notice`), what the report says (`report`). The process and
filesystem work in `main.rs` is the only untested part, and it is the part with
nothing to decide.

Two things it does that are worth knowing:

- **It refuses before it builds if the licence is not shipping.** A licence
  problem is a reason not to distribute the artifact, so learning about it once
  the artifact exists is learning too late.
- **`--measure-install` is opt-in, and refuses if lightcode is already
  installed.** Both halves were found by review. Weighing a real install means
  running the installer on the machine doing the build, and the uninstall
  afterwards is not conditional on the measurement succeeding — a build tool
  that leaves a developer with an application they did not ask to install,
  because a `?` returned early, is worse than one that reports nothing. The
  refusal is the more interesting half; see below.

If the installer ever passes 30 MB the report says **MISSED** and by how much,
in the open. It does not fail the build: the spec calls 20–30 MB and 20K LOC
signals to "stop and re-evaluate", and a gate would turn a decision a person
should make into a threshold someone edits to get their build through.

### `bundle.copyright` was not the notice, and now something is

The previous comment left this open — whether a copyright line is *enough*. It
is not, and the reason is in the licence's own words: MIT asks that "the above
copyright notice **and this permission notice**" be included. `bundle.copyright`
is a string in the executable's version resource that names the holder and omits
the permission text, which is the half being asked for. Four fifths of this
artifact by size is upstream's software, so this is an obligation rather than a
formality.

So `THIRD_PARTY_NOTICES.md` now carries upstream's MIT text verbatim, and ships
two ways: `bundle.licenseFile` puts it on the installer's licence page, and
`bundle.resources` installs it beside the executable. Verified by installing:
`THIRD_PARTY_NOTICES.md`, 2,366 bytes, in the install directory. `cargo xtask
bundle` fails if either route stops working, and `--measure-install` additionally
fails if the file is not on disk after a real install.

**One half of this is deliberately incomplete and says so.** The Rust
dependencies are statically linked and in the artifact too, and no per-crate
licence audit has been run — they are overwhelmingly MIT/Apache-2.0 and SQLite
is public domain, but "overwhelmingly" is not an audit. The notice file states
this rather than implying coverage it does not have. Worth its own ticket; not
this one's, which is about upstream's UI and covers it.

### What a real install proved, and what still needs a second machine

Installed silently, launched, driven, and uninstalled on this machine:

| | |
|---|---|
| Install directory | `%LOCALAPPDATA%\lightcode`, **3 files**, 24.27 MB |
| | `lightcode.exe` 25,368,064 · `uninstall.exe` 79,223 · `THIRD_PARTY_NOTICES.md` 2,366 |
| Launch | the installed binary started, and had a main window to close |
| Server | `GET /` → `200 text/html`, 3,195 bytes |
| The real bundle | `/assets/index-Cz93BjSz.js` → `200 text/javascript`, **3,562,609 bytes** |
| Close | window closed, process gone, port 4773 free |
| Uninstall | every installed file and the registry key gone; no desktop shortcut |

So the artifact this build produces installs, runs, serves the actual UI, and
removes itself. That is most of "produces a working application" — but not the
word **clean**, and the distinction is the whole point of the criterion. This
machine has WebView2, the Rust toolchain, and lightcode's own `state.sqlite`
from earlier work. What it cannot rule out is a dependency this application
carries by accident and only a machine without it would reveal.

**The webview criterion is untested and cannot be faked.** `webviewInstallMode:
downloadBootstrapper` is configured and the bundler honours it; the one thing
that can be said from here is negative and still useful: at 5.05 MB the
installer demonstrably carries no rendering engine, since the embedded and
offline options would add well over a hundred megabytes. Whether the
bootstrapper actually provisions WebView2 on a machine that lacks it is
unverified. Both remaining boxes need a clean Windows VM, which is why this is
`ready-for-human` rather than done.

### Review found this ticket committing this ticket's own sin

Worth recording in full, because it is the exact failure mode the ticket is
written against, in the code written to prevent it.

**The footprint could have reported 0.00 MB, and called it a triumph.** The
measurement weighs what the installer *adds* to the install directory, because
that directory also holds the developer's database (ticket 30, below). The
"already there" set was snapshotted before installing. On any machine with a
previous install, that set is `lightcode.exe`, `uninstall.exe` and
`THIRD_PARTY_NOTICES.md` — the entire artifact. The installer would rewrite
those same three paths, the walk would skip all three, and the report would say
**0.00 MB, 20.00 MB under the range, 318× smaller than upstream**, in the
document whose only job is to be trusted.

It never fired here because this machine had no previous install when the real
figures were taken. That is luck, not design, and luck is what the balance check
in `loc` exists to refuse.

The fix is to **refuse rather than measure**: if lightcode is already installed,
`--measure-install` stops and says so. That kills the zero at the root instead of
compensating for it, and it removes a second problem review raised in the same
breath — the old code would have run `uninstall.exe` on an installation it did
not create, quietly removing the copy a developer actually uses in order to
print a size. There is also a floor check now: added nothing is an error, not a
figure.

**A latent version of the same thing in the line counter.** `#[cfg(test)]` was
matched as an exact line, so `#[cfg(test)] mod tests {` on one line, or
`#[cfg(all(test, …))]`, would have silently moved a test module into
**production code** — the number the project is judged by. Nothing would have
noticed: the braces still balance, so the scan stays `balanced` and reports a
wrong figure confidently. All 33 occurrences in the server are the bare form
today, which is a fact about today. Now matched by predicate, with
`#[cfg(not(test))]` explicitly excluded, and all three spellings pinned by tests.

**And the classifier now runs against the real 32,000 lines in `cargo test`.**
It previously only ever ran inside a three-minute release build, so a construct
that defeated it would have been met by the person least able to stop and fix
it.

### Two things about the Tauri CLI, for whoever runs this next

**It is not installed by anything.** `cargo tauri` is not a dependency of this
workspace and cannot be, so `cargo xtask release` on a fresh machine fails at the
build step until someone runs `cargo install tauri-cli --version "^2" --locked`.
It then downloads NSIS 3.11 and `nsis_tauri_utils.dll` on first bundle, verifying
hashes — so the first release build of a clone needs a network as well.

**It edits `crates/lightcode-shell/Cargo.toml` behind you.** Every
`cargo tauri build` rewrote `tauri-build = { version = "2", default-features =
false }` to `… default-features = false , features = [] }` — a no-op with a
stray space, from the CLI's "check mismatched versions" pass. Reverted here.
Expect it in `git status` after a release build; it means nothing.

### A finding that only installing could produce

**lightcode installs itself into the same directory it keeps its database in.**
Tauri's per-user NSIS default is `%LOCALAPPDATA%\lightcode`; `config::data_dir`
is `%LOCALAPPDATA%\lightcode`. So `state.sqlite` sits beside `lightcode.exe`,
the uninstaller's "delete application data" option points at a directory this
server never writes to, and the developer's history survives an uninstall
because `RMDir "$INSTDIR"` happens not to recurse.

Filed as **ticket 30** rather than fixed here. It is not a size question, and
moving where a developer's conversations live is not something to slip into a
measuring run. It is also the reason the installed figure counts added files
only: weighing the directory put 84 KB of this agent's own database into the
artifact's number, which is exactly the kind of quietly wrong figure this ticket
exists to prevent.

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

### Settled: the whole split, with production code as the figure

Asked and answered before any of the above was written. The build reports **all
five numbers** and sets **production code** against the 20K signal, because
either half alone misleads: the total on its own trips an alarm about scope that
does not exist, and production code on its own hides that this server is 32,134
lines of file.

Measured by `cargo xtask release`, with tickets 26–29 landed since the estimate
above:

| | lines | then |
|---|---|---|
| total | 32,134 | 31,351 |
| comments | 8,379 | 8,145 |
| `#[cfg(test)]` unit tests | 10,052 | 9,708 |
| blank | 1,607 | 1,582 |
| **production code** | **12,096** | ~11,900 |

**12,096 against 20,000 — 7,904 lines inside it.** The estimate above was
arrived at by hand and came within 200 lines of what the classifier says, which
is the main reason to believe the classifier.

`xtask::loc` is not `grep`, and the difference is the point. `trim().starts_with("//")`
is wrong in both directions on this codebase — it calls `let s = "// ...";` prose
and every line of a block comment code — so what is there is a byte scanner that
knows where string literals, character literals, raw strings and nested block
comments begin and end, because that is where a `//` or a `{` can appear and mean
nothing. A brace hiding in `r#"{"method": "ping"}"#` would close `mod tests`
early and hand nine thousand lines of tests back to production, silently.

Which is why it also checks itself: a file cannot end inside a comment, a literal
or a `#[cfg(test)]` region, so if the scan thinks one did, the report says the
sources **could not be classified** and prints no number at all. Across all 34
files of the real server it ends balanced. A wrong line count is worse than none,
and this ticket is entirely about numbers that could be wrong without looking
wrong.
