# lightcode

A Rust server + Tauri shell that drives the `claude` CLI directly, wearing the
`apps/web` UI from **laplus**. See `HANDOFF-rust-server-tauri.md` for the plan
and `spike-claude-protocol/README.md` for the STEP 1 protocol spike that gated
it (answered; its code now lives in the workspace).

## The UI lives in another repository

`laplus` — <https://github.com/hoangvu12/laplus>, this project's fork of
`pingdotgg/t3code` — is cloned **beside** this repo, not inside it:

```
nguyenvu/
├── lightcode/   ← here. the Rust server, the shell, the tickets
└── laplus/      ← the UI. `apps/web` builds the bundle the shell embeds
```

The shell's build script reads `../laplus/apps/web/dist`, and says what to clone
if it is not there. Building it is `pnpm install && pnpm --filter @t3tools/web
build` in `laplus/`. Ticket 32 and `docs/adr/0012` are why it is a fork we own
rather than the read-only checkout it used to be; the short version is that
three tickets were all "the client does something we cannot change".

Upstream is a remote of that repo (`upstream`, push disabled), so a sync is a
`git merge` there rather than a re-vendoring here. Its `@t3tools/*` package scope
is deliberately **not** renamed — it appears in 1,069 files, and renaming it
would conflict with upstream on nearly every merge.

`t3code/` may still be present here as the old depth-1 read-only checkout. It is
gitignored, nothing builds from it any more, and it is worth keeping only as the
*unmodified* upstream UI that user story 57 asks to connect to this server.

## Layout

- `crates/lightcode-server/` — the server. Cargo workspace root is the repo root.
- `crates/lightcode-shell/` — the desktop application: a Tauri window with the
  server running inside it. Its build script embeds `../laplus/apps/web/dist`,
  so it needs that checkout built. **It is not a default workspace member**
  for exactly that reason: `cargo build` and `cargo test` cover the server only,
  and the shell is asked for by name (`cargo run -p lightcode-shell`,
  `cargo test -p lightcode-shell`). `nsis/installer.nsi` is tauri-bundler's
  installer template, vendored and changed in two places so that lightcode
  installs into `%LOCALAPPDATA%\Programs\lightcode` rather than on top of its
  own database — ticket 30 and `docs/adr/0013`. **Re-vendor it by hand when
  tauri-cli is upgraded**; its header says how, and `cargo test -p xtask` fails
  if either change goes missing.
- `xtask/` — how a release is made: `cargo xtask release` builds the Windows
  installer *and* measures it, because the project exists for that number. Writes
  `docs/artifact-size.md`. `--measure-install` additionally installs, weighs and
  uninstalls, which is opt-in because it touches the machine — and refuses if
  lightcode is already installed, rather than uninstalling someone's copy.
  **Needs `cargo install tauri-cli --version "^2" --locked` first**: the Tauri
  CLI is not a workspace dependency and cannot be one, and the first release
  build downloads NSIS, so a fresh clone needs that command and a network before
  it can produce an installer. Nothing else in the repo needs either.
- `fixtures/` — committed test inputs for the two protocols: `socket-wire/` is
  what the UI speaks, `claude-cli/` is what the agent speaks. Both have READMEs.
- `tools/wire-capture/` — the recording proxy used to produce `socket-wire/`.
- `tools/ui-driver/` — a headless browser pointed at a running lightcode, over
  the DevTools protocol. The other end of the same wire: `wire-capture` records
  what the *reference server* answers, this drives what the *real client* does,
  and it is the only way the UI half of this application can be checked. Has a
  README.
- `.scratch/` — tracker files (see below) and raw capture evidence.

## Running the tests

`cargo test` covers the server. Two things about *how* to run it, both of which
have already cost someone an afternoon (ticket 29):

- **Use `--no-fail-fast`.** `cargo test` stops at the first failing binary, so
  one failing lib test means no integration binary runs at all — and the summary
  line then reads like a suite that lost two hundred tests rather than one that
  never started them.
- **Redirect to a file and grep the file; never pipe into `head`.** Piping kills
  cargo mid-run and orphans the `git` children it had spawned, which then compete
  with the *next* run. Several confusing failures have been self-inflicted this
  way.

A test in this repo **does not assert on elapsed wall-clock time.** It asserts on
the decision the code made; timeouts exist to catch a hang, not to enforce a
budget. `READ_TIMEOUT` in `tests/harness/mod.rs` carries the full reasoning. If
the suite is slow on a loaded machine, `--test-threads` is the lever — raising a
timeout until the machine passes trades a test that fails when it should not for
one that passes when it should not.

## Agent skills

### Issue tracker

Local markdown — issues and specs live as files under `.scratch/<feature-slug>/`; this repo has no git remote. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, unchanged (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line in each issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — one `CONTEXT.md` and one `docs/adr/` at the repo root. See `docs/agents/domain.md`.
