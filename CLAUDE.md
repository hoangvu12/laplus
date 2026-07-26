# lightcode

A Rust server + Tauri shell that drives the `claude` CLI directly, reusing
t3code's `apps/web` UI. See `HANDOFF-rust-server-tauri.md` for the plan and
`spike-claude-protocol/README.md` for the STEP 1 protocol spike that gates it.

`t3code/` is a vendored upstream checkout with its own `.git` — reference only,
never committed here (see `.gitignore`).

## Agent skills

### Issue tracker

Local markdown — issues and specs live as files under `.scratch/<feature-slug>/`; this repo has no git remote. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, unchanged (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`), recorded as a `Status:` line in each issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — one `CONTEXT.md` and one `docs/adr/` at the repo root. See `docs/agents/domain.md`.
