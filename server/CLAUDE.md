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
├── reference/      upstream's TypeScript server, as a specification
├── server/         ← you are here: the Rust server, the shell, the release
└── .scratch/       the tickets, for both halves
```

**The reference server is `reference/t3code-server/`,** and it is an ordinary
directory you can open — not a sparse-checkout, not something to recover from
the object store. This repository has no history before its first commit and no
upstream remote, so the older instructions to run
`git show HEAD:apps/server/src/ws.ts` or `git sparse-checkout disable` no longer
describe anything; tickets written before the move still say so, and they are
wrong about the mechanism and right about why it matters. The reference
implementation is often the only specification a divergence can be argued
against, which is why it was carried across rather than left behind.
`reference/README.md` has the rules — read it, do not edit it, do not import
from it.

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
  **Needs `cargo install tauri-cli --version "^2" --locked` first**: the Tauri
  CLI is not a workspace dependency and cannot be one, and the first release
  build downloads NSIS, so a fresh clone needs that command and a network before
  it can produce an installer. Nothing else in the repo needs either.
  `.github/workflows/release.yml` runs the same command on a Windows runner when
  a `v*.*.*` tag is pushed, and publishes the installer to a GitHub Release —
  the tag has to match `tauri.conf.json`'s version or the job refuses. What this
  fork publishes, and why it does not yet update itself, is `docs/adr/0020`.
- `fixtures/` — committed test inputs for the two protocols: `socket-wire/` is
  what the UI speaks, `claude-cli/` is what the agent speaks. Both have READMEs.
- `tools/wire-capture/` — the recording proxy used to produce `socket-wire/`.
- `tools/ui-driver/` — a headless browser pointed at a running laplus, over
  the DevTools protocol. The other end of the same wire: `wire-capture` records
  what the _reference server_ answers, this drives what the _real client_ does,
  and it is the only way the UI half of this application can be checked. Has a
  README.
- `../.scratch/` — tracker files (see below) and raw capture evidence. At the
  repository root rather than in here, because the tickets cover the UI as much
  as the server: 31 is a client fix, 26 was one, and 24's open question is about
  the web bundle.

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

`.github/workflows/rust.yml` runs `cargo test --no-fail-fast` on
`windows-latest` for any change under `server/`. Windows rather than the cheaper
Linux runner because that is the only platform laplus ships — a Linux runner
would never compile the crate's `cfg(windows)` blocks or the ConPTY behind
`portable-pty`. It gates on the suite only: clippy reports without
`-D warnings`, and `cargo fmt --check` is absent because this tree has never
been rustfmt-formatted and fails on all 29 files. Ticket 36.

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
