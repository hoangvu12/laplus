# laplus — the server and the shell

**This file covers `server/`.** The repository root has its own `CLAUDE.md`,
which is upstream's and is about the TypeScript side.

A Rust server + Tauri shell that drives the `claude` CLI directly, wearing the
`apps/web` UI from this same repository. See `HANDOFF-rust-server-tauri.md` for
the plan and `spike-claude-protocol/README.md` for the STEP 1 protocol spike
that gated it (answered; its code now lives in the workspace).

**This project was called `lightcode` until the rename.** Every ticket in
`.scratch/`, every ADR, and every capture in `fixtures/` was written under that
name and still says it — see the **lightcode** entry in `CONTEXT.md`. Nothing in
the live code does.

## Where this sits in the repository

```
laplus/
├── apps/web        the UI this shell embeds
├── packages/       @t3tools/{contracts,client-runtime,shared} — the contract
├── server/         ← you are here: the Rust server, the shell, the release
└── .scratch/       the issue tracker, for both halves
```

**There is no reference server in the tree.** `reference/t3code-server/` held
upstream's TypeScript implementation as a specification; it was deleted, along
with the ticket set that was written against it. This repository has no history
before its first commit and no upstream remote, so neither
`git show HEAD:apps/server/src/ws.ts` nor a sparse-checkout will bring it back.

When a divergence needs arguing against the original, read it at
`github.com/pingdotgg/t3code` under `apps/server/` — the doc comments in
`crates/laplus-server/src/` cite it in that form. It is MIT, it has moved on
since the fork, and it is evidence of an implementation rather than a
dependency.

The shell's build script reads `../../../apps/web/dist` and says what to run if
it is not there: `pnpm install && pnpm --filter @t3tools/web build`, from the
repository root. `cargo build` will not do that for you — a stale `dist/` is a
bug in this project rather than a vendoring detail, which is what ticket 32
changed. `docs/adr/0014` is why the two trees are one repository.

**There is no `upstream` remote and there are no more syncs.** This project
stopped merging from `pingdotgg/t3code`: laplus answers 26 of the 71 methods the
contract declares, so every feature upstream ships is more UI calling a method
this server does not have — a sync widens the gap rather than closing it, which
is what the one sync that was taken demonstrated (ticket 33). The UI is now
maintained here.

The `@t3tools/*` package scope is kept anyway. It appears in over a thousand
files, and renaming it now buys a nicer name in exchange for touching every
import in the tree.

## Layout

All paths below are relative to `server/`, which is the Cargo workspace root —
`Cargo.toml`, `.cargo/config.toml` and `target/` are here, not at the repository
root, so cargo commands are run from this directory.

- `crates/laplus-server/` — the server.
- `crates/laplus-shell/` — the desktop application: a Tauri window with the
  server running inside it. Its build script embeds the repository's
  `apps/web/dist`, so it needs that built by `pnpm`. **It is not a default
  workspace member** for exactly that reason: `cargo build` and `cargo test`
  cover the server only,
  and the shell is asked for by name (`cargo run -p laplus-shell`,
  `cargo test -p laplus-shell`). `nsis/installer.nsi` is tauri-bundler's
  installer template, vendored and changed in two places so that laplus
  installs into `%LOCALAPPDATA%\Programs\laplus` rather than on top of its
  own database — ticket 30 and `docs/adr/0013`. **Re-vendor it by hand when
  tauri-cli is upgraded**; its header says how, and `cargo test -p xtask` fails
  if either change goes missing.
- `xtask/` — how a release is made: `cargo xtask release` builds the Windows
  installer _and_ measures it, because the project exists for that number. Writes
  `docs/artifact-size.md`. `--measure-install` additionally installs, weighs and
  uninstalls, which is opt-in because it touches the machine — and refuses if
  laplus is already installed, rather than uninstalling someone's copy.
  **Needs the Tauri CLI installed first**: it is not a workspace dependency and
  cannot be one, and the first release build downloads NSIS, so a fresh clone
  needs both a CLI and a network before it can produce an installer. Nothing
  else in the repo needs either.

  ```
  cargo binstall --locked tauri-cli --version "^2"   # downloads a prebuilt binary
  cargo install  --locked tauri-cli --version "^2"   # compiles it, ~minutes
  ```

  Both leave the same `cargo-tauri` in `~/.cargo/bin`, so `cargo tauri build`
  works either way — `cargo install` has no notion of a prebuilt binary and
  always builds from source, which is why `release.yml` uses the first form and
  Tauri's own documentation advises against the second in CI.
  `.github/workflows/release.yml` runs the same command on a Windows runner when
  a `v*.*.*` tag is pushed, and publishes the installer to a GitHub Release —
  the tag has to match `tauri.conf.json`'s version or the job refuses. What this
  fork publishes is `docs/adr/0020`.
  **A release build now needs `TAURI_SIGNING_PRIVATE_KEY` in its environment**
  (ticket 74): `bundle.createUpdaterArtifacts` is on, so the bundler signs what
  it produces, and an installed laplus accepts only an update signed by the key
  whose public half is in `tauri.conf.json`. The key lives in `~/.tauri/` and in
  the repository secret of the same name — never in the tree, which `.gitignore`
  enforces. Losing it strands every existing install.

- `fixtures/` — committed test inputs for the two protocols: `socket-wire/` is
  what the UI speaks, `claude-cli/` is what the agent speaks. Both have READMEs.
- `tools/wire-capture/` — the recording proxy used to produce `socket-wire/`.
- `tools/ui-driver/` — a headless browser pointed at a running laplus, over
  the DevTools protocol. The other end of the same wire: `wire-capture` records
  what the _reference server_ answers, this drives what the _real client_ does,
  and it is the only way the UI half of this application can be checked. Has a
  README.
- `../.scratch/` — tracker files (see below) and raw capture evidence. At the
  repository root rather than in here, because tickets cover the UI as much as
  the server. Its `rust-server-tauri/` directory was deleted on 2026-07-29.

## Running the tests

`cargo test` covers the server. Two things about _how_ to run it, both of which
have already cost someone an afternoon (ticket 29):

- **Use `--no-fail-fast`.** `cargo test` stops at the first failing binary, so
  one failing lib test means no integration binary runs at all — and the summary
  line then reads like a suite that lost two hundred tests rather than one that
  never started them.
- **Redirect to a file and grep the file; never pipe into `head`.** Piping kills
  cargo mid-run and orphans the `git` children it had spawned, which then compete
  with the _next_ run. Several confusing failures have been self-inflicted this
  way.

A test in this repo **does not assert on elapsed wall-clock time.** It asserts on
the decision the code made; timeouts exist to catch a hang, not to enforce a
budget. `READ_TIMEOUT` in `tests/harness/mod.rs` carries the full reasoning. If
the suite is slow on a loaded machine, `--test-threads` is the lever — raising a
timeout until the machine passes trades a test that fails when it should not for
one that passes when it should not.

### In CI

`.github/workflows/rust.yml` runs `cargo test --no-fail-fast` for any change
under `server/`, on **two runners**:

- **`windows-latest`**, the whole default set — `laplus-server` and `xtask`.
  This is the platform laplus installs on, and the only one that exercises the
  crate's `cfg(windows)` blocks or the ConPTY behind `portable-pty`.
- **`ubuntu-latest`**, `-p laplus-server` only. Added by ticket 05 of the
  headless-Linux effort, because every `#[cfg(not(windows))]` twin in the crate
  had been written and never compiled. `xtask` is left out on purpose: it builds
  and measures a Windows installer, so its tests on Linux would be a second
  opinion about string constants.

`fail-fast: false`, so one platform going red still leaves the other's answer.
It gates on the build and the suite only: clippy reports without `-D warnings`,
and `cargo fmt --check` is absent because this tree has never been
rustfmt-formatted and fails on all 29 files. Tickets 36 and 05.

**What CI does not cover is the application on Linux.** No hand-driven session
has run there — see `docs/running-headless.md`, which also carries the build
prerequisites (a C compiler, for `rusqlite`'s bundled SQLite).

**There are two workflows in this repository and both are ours** — `rust.yml`
and `ci.yml`, the latter covering `apps/web` and the three packages on Linux.
Ticket 70 was about the nine upstream workflows that came with the fork and
started firing once Actions woke up: a three-hourly `release.yml` that tried to
publish, a `deploy-relay.yml` on every push to `main`, three mobile EAS jobs.
None of them exist here. That ticket is closed by construction rather than by a
`gh workflow disable`, and nothing in this repository builds a mobile app, a
relay, or a marketing site to put back.

## Agent skills

### Issue tracker

Local markdown — issues and specs live as files under `.scratch/<feature-slug>/` at the repository root, not on GitHub. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, unchanged (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line in each issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — one `CONTEXT.md` and one `docs/adr/`, both at the root of `server/` rather than of the repository, because they are this half's vocabulary and this half's decisions. See `docs/agents/domain.md`.
