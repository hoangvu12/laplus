# 36 — The server is tested only on the machine that changed it

**What to build:** a CI workflow that runs the Rust suite on every change to
`server/`, and a written-down account of the eight upstream workflows this fork
carries but has never run.

**Status:** ready-for-agent

**Found by:** opening PR #2 and noticing it reported no checks at all.

## What happens

Nothing in CI runs this project's Rust half. Every workflow in
`.github/workflows/` is upstream's TypeScript/Electron pipeline, and **no
workflow invokes `cargo`** — grepping the directory for "rust" hits only
`AZURE_TRUSTED_SIGNING` as a substring.

So the 708 tests that gate this project's entire reason for existing run exactly
when a developer types `cargo test` locally, on Windows, by hand. Ticket 29 is
already a whole ticket about how easy that suite is to run wrongly
(`--no-fail-fast`, never piping into `head`), which is an argument for a machine
doing it the same way every time rather than a person doing it from memory.

### The fork has never run a workflow at all

Worth stating separately, because it is the more surprising half:

```
gh api repos/hoangvu12/laplus/actions/permissions → {"enabled": true, …}
gh api repos/hoangvu12/laplus/actions/runs        → total_count: 0
```

Actions are **enabled**, and in this repository's whole history **zero workflow
runs have ever happened** — not for PR #1, not for PR #2, not for any push to
`main`. `ci.yml` triggers on every `pull_request` with no path filter, so it
should have run and did not. The likeliest reason is its runner label,
`blacksmith-8vcpu-ubuntu-2404`, which is a third-party runner this fork has no
access to; that has not been confirmed.

That dormancy is currently doing useful work, and whoever changes this should
know what it is holding back. The inherited workflows include:

| Workflow                 | Trigger                               | What it would do           |
| ------------------------ | ------------------------------------- | -------------------------- |
| `release.yml`            | **`schedule: 0 */3 * * *`** and tags  | Build and publish releases |
| `deploy-relay.yml`       | push to `main`                        | Deploy the relay           |
| `mobile-eas-preview.yml` | every pull request                    | Expo EAS build             |
| `pr-size.yml`            | `pull_request_target`                 | Runs with a write token    |
| `pr-vouch.yml`           | `pull_request_target`, issue comments | Runs with a write token    |
| `ci.yml`                 | every PR, push to `main`              | `vp check` + Electron      |

Most would simply fail for want of secrets, which is noise rather than damage —
but `release.yml` fires **every three hours** and `deploy-relay.yml` fires on
the merge of any PR to `main`, so the noise would be continuous and some of it
would be attempts to publish. None of this is hypothetical about _this_ ticket
— adding a new workflow file does not wake the others — but it is the context
anyone touching CI here needs, and nobody has written it down.

## What "fixed" means

A **new file**, `.github/workflows/rust.yml`, rather than an edit to upstream's
`ci.yml`. `docs/adr/0012` is the standing decision not to take changes that turn
a sync into a fight; a file upstream does not have cannot conflict with a file
upstream does have. Editing `ci.yml` to bolt a cargo job onto upstream's
Electron pipeline is the version of this that costs something on every merge.

### It runs on Windows

Not the cheaper Linux runner, and this is the one real decision in the ticket.

The crate is very likely portable — every dependency is cross-platform (`axum`,
`notify`, `rusqlite` with `bundled`, `tokio`, `portable-pty`, `tempfile`,
`tokio-tungstenite`), there are no target-gated dependencies, and only six
`cfg(windows)` sites exist in the whole crate. Linux would probably be green and
would certainly be faster.

It is still the wrong choice. laplus ships a Windows installer and nothing else:
NSIS, `%LOCALAPPDATA%`, ConPTY behind `portable-pty`, and a `panic = "abort"`
decision in the workspace manifest argued entirely from "Windows does not reap a
child when its parent dies". Those six `cfg(windows)` blocks are precisely the
code a Linux runner would never compile, which would leave the platform-specific
half of a Windows-only product permanently untested while the badge stayed
green. Test the thing that ships.

### It does not gate on clippy or fmt

Both would fail today for reasons that have nothing to do with a change under
review, and a check that is red on arrival teaches everyone to ignore it:

- `cargo clippy` reports one pre-existing `result_large_err` in `server.rs:494`.
  Clippy runs, but without `-D warnings`, until that is dealt with.
- `cargo fmt --check` fails on **all 29 files** in the crate. This tree has never
  been rustfmt-formatted, and running `cargo fmt` to fix it would be a
  single-commit reformat of the entire server — a separate decision, and
  probably its own ticket.

## Acceptance

- A new `.github/workflows/rust.yml` runs `cargo test --no-fail-fast` from
  `server/` on `windows-latest`, for pull requests and pushes to `main` that
  touch `server/` or the workflow itself.
- The run is green on the current tip: 708 passed, 0 failed.
- No existing workflow file is edited, so the sync surface is unchanged.
- Cargo's registry and `target/` are cached, or the ticket says why not.
- The dormant-workflow table above is recorded somewhere durable — this ticket
  counts, but `server/CLAUDE.md` is the likelier home if anyone is expected to
  find it.

## Worth settling before starting

- **The suite may be flaky on a two-core runner.** `server/CLAUDE.md` is
  explicit that a test here never asserts on wall-clock time and that
  `--test-threads` is the lever when a machine is loaded — CI runners are small
  and loaded. If it flakes, that lever is the first thing to reach for, and
  raising a timeout is specifically the wrong answer.
- **`xtask` is in `default-members`,** so a bare `cargo test` runs its NSIS
  vendoring checks too. That is intended (the manifest says so), but it means CI
  covers the release tooling as well as the server, and a tauri-cli upgrade will
  fail CI rather than a developer's machine.
- **`laplus-shell` is deliberately not covered,** because it needs a `pnpm` build
  of `apps/web/dist` first. Pulling it in means putting the whole Node toolchain
  in this workflow, which is a different and much larger ticket.

## Comments

### 2026-07-28 — agent. Filed

Filed while closing out ticket 34's PR. The Rust half having no CI is the
finding; the fork having never run _any_ workflow is the thing that took longer
to establish and is the reason the "what would I be waking up" table exists.

The Windows-versus-Linux question is answered in the body rather than left open,
unlike ticket 35's — the evidence points one way and the cost of being wrong is
a slower runner, not a divergence this project has to carry forever.
