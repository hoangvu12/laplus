# AGENTS.md

laplus is a Rust server and a Tauri shell driving the `claude` CLI, wearing a
React UI. **`server/AGENTS.md` and `server/CLAUDE.md` cover `server/`** — the
Rust half has its own conventions, its own ADRs, and its own test discipline.
This file covers the TypeScript half and the repository as a whole.

## Layout

```
apps/web/       the UI. React + Vite, built by pnpm, embedded into the shell
packages/       @t3tools/{contracts,client-runtime,shared} — the contract
server/         the Rust server, the Tauri shell, the release. Cargo root
.scratch/       the issue tracker, for both halves
```

The `@t3tools/*` package scope is a name, not a dependency on anyone: this
repository is self-contained and the scope is kept only because renaming it
touches over a thousand files for no behavioural gain.

## The two halves meet in one place

`server/crates/laplus-shell/build.rs` embeds `apps/web/dist` — a path, not a
search. So:

- **`cargo build -p laplus-shell` does not build the UI.** Run
  `pnpm build:web` first. A stale `dist/` is a bug in this project, not a
  vendoring detail.
- `cargo build` and `cargo test` on their own cover the server and `xtask`
  only. The shell is asked for by name.
- `pnpm dev` runs the UI against a separately-started `pnpm dev:server`.
  `pnpm app` builds nothing and runs the shell as a window.

## Task completion

- Keep verification focused on what changed. Run the smallest relevant test
  set; the full workspace suite is CI's job, not a routine completion step.
  - `vp test run <test-files>` for focused tests. `vp run test` only when the
    affected package needs its own `test` script.
  - Run targeted formatting, lint and type checks for the affected scope.
- **A green suite is not evidence the application works.** Every finding in
  `.scratch/rust-server-tauri/HANDOFF-2026-07-28-using-the-app.md` was invisible
  to a passing suite and obvious within a minute of driving the window. For any
  user-visible change, drive it: `server/tools/ui-driver/` is a headless browser
  pointed at a running laplus and has a README.
- Stop dev servers and watchers when focused verification is done.

## Package roles

- `apps/web` — React/Vite UI. Owns session UX, conversation and event
  rendering, and client-side state. Talks to the server over a WebSocket.
- `packages/contracts` — effect/Schema schemas for provider events, the socket
  protocol, and model/session types. **Schema-only; no runtime logic.** This is
  the whole vocabulary the UI can speak, and the server implements a subset of
  it — `.scratch/rust-server-tauri/PARITY-LEDGER.md` is which subset.
- `packages/shared` — runtime utilities. Explicit subpath exports
  (`@t3tools/shared/git`); no barrel index.
- `packages/client-runtime` — shared client runtime. No root export; import an
  explicit subpath. The lint config enforces this.

## Dependencies

`effect` is pinned to `4.0.0-beta.102` in the catalog **and patched**. Between
beta.78 and beta.102 its RPC client changed how it encodes request ids, and the
Rust server decodes that encoding by hand — see ticket 33. Nothing in
`packages/contracts` can see a change like that, and the conformance suite did
not catch it. Bump the pin deliberately, and open the window afterwards.

## Issue tracker

Markdown under `.scratch/<feature-slug>/`, not GitHub Issues. Status lives in a
`Status:` line in each file, using the five canonical roles (`needs-triage`,
`needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See
`server/docs/agents/issue-tracker.md`.
