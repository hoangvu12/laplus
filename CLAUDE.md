# lightcode

A Rust server + Tauri shell that drives the `claude` CLI directly, reusing
t3code's `apps/web` UI. See `HANDOFF-rust-server-tauri.md` for the plan and
`spike-claude-protocol/README.md` for the STEP 1 protocol spike that gated it
(answered; its code now lives in the workspace).

`t3code/` is a vendored upstream checkout with its own `.git` — reference only,
never committed here (see `.gitignore`).

## Layout

- `crates/lightcode-server/` — the server. Cargo workspace root is the repo root.
- `crates/lightcode-shell/` — the desktop application: a Tauri window with the
  server running inside it. Its build script embeds `t3code/apps/web/dist`, so
  it needs the vendored checkout built. **It is not a default workspace member**
  for exactly that reason: `cargo build` and `cargo test` cover the server only,
  and the shell is asked for by name (`cargo run -p lightcode-shell`,
  `cargo test -p lightcode-shell`).
- `fixtures/` — committed test inputs for the two protocols: `socket-wire/` is
  what the UI speaks, `claude-cli/` is what the agent speaks. Both have READMEs.
- `tools/wire-capture/` — the recording proxy used to produce `socket-wire/`.
- `tools/ui-driver/` — a headless browser pointed at a running lightcode, over
  the DevTools protocol. The other end of the same wire: `wire-capture` records
  what the *reference server* answers, this drives what the *real client* does,
  and it is the only way the UI half of this application can be checked. Has a
  README.
- `.scratch/` — tracker files (see below) and raw capture evidence.

## Agent skills

### Issue tracker

Local markdown — issues and specs live as files under `.scratch/<feature-slug>/`; this repo has no git remote. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, unchanged (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line in each issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — one `CONTEXT.md` and one `docs/adr/` at the repo root. See `docs/agents/domain.md`.
