# 36 — The server is tested only on the machine that changed it

**What to build:** a CI workflow that runs the Rust suite on every change to
`server/`, and a written-down account of the eight upstream workflows this fork
carries but has never run.

**Status:** done

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

### 2026-07-28 — agent. The dormancy ended on the first push

The body above says the fork had never run a workflow and that adding one would
not wake the others. The first half was true and is now history; the second was
true about the _file_ and beside the point about the _push_.

Pushing this ticket's commit to PR #2 started five runs at once — `Rust`,
`CI`, `Mobile EAS Preview` (skipped), `PR Size` and `PR Vouch` (both green).
Nothing about `rust.yml` caused that: those four fire on pull-request events and
would have fired for any push to this branch. Why they had not already fired on
PR #2's _opening_ is unexplained.

One prediction did survive the test. `CI` went **queued and stayed queued**,
which is what a job asking for a `blacksmith-8vcpu-ubuntu-2404` runner this fork
cannot reach looks like. That was offered above as "not confirmed"; it is now
the best-supported explanation for the whole dormant period.

The practical consequence is the one worth carrying forward: **merging PR #2
will push to `main`, and `deploy-relay.yml` fires on any push to `main`.**
`release.yml`'s three-hourly `schedule` is also now live. Neither is this
ticket's to fix — `gh workflow disable` on the ones this project does not use is
a decision about the fork, not a consequence of adding Rust CI — but nobody
should be surprised by it. `server/CLAUDE.md` has been corrected to say so.

### 2026-07-28 — agent. Green on the first run. Done

Run [30318228631](https://github.com/hoangvu12/laplus/actions/runs/30318228631),
triggered by `pull_request`, **6m5s**, every step green:

```
test result: ok  ×29 binaries → 708 passed, 0 failed
```

The same 708 the author's machine reports, which is the number this ticket
existed to stop depending on. `xtask`'s NSIS checks are in that total, as
`default-members` intends. Nothing flaked, so the `--test-threads` worry under
"Worth settling" did not materialise — worth re-reading if it ever does, because
raising a timeout is specifically the wrong answer.

Six minutes is a cold build: `rusqlite`'s `bundled` feature compiles the SQLite
C amalgamation from source, and `Cache cargo` had nothing to restore on a first
run. Later runs should be substantially quicker.

**One annotation, and it is not ours.** The run is decorated with
`git.exe failed with exit code 128`, which reads like a test failure and is not
— it is in **Post Checkout**, `actions/checkout`'s cleanup, and GitHub labels
annotations with the _job_ name, which here is "Test".

```
fatal: No url found for submodule path
       '.repos/alchemy-effect/.vendor/alchemy' in .gitmodules
```

The tree carries a gitlink at that path and the repository has **no
`.gitmodules` at all**, so git cannot resolve a URL for it. It is in `main` as
well, so it predates every commit on this branch; it is upstream's tree shape,
inherited. It fails nothing and will decorate every `actions/checkout` run in
this repository until someone either restores a `.gitmodules` entry or drops the
gitlink — both of which are changes to a path upstream owns, so neither is done
here.
