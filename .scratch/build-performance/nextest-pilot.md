# cargo-nextest pilot on the Windows CI shards

Status: ready-for-human

Decided: 2026-08-24, following `.scratch/build-performance/research.md` and
`popular-projects.md`, which ranked a nextest pilot above any compiler-cache
work (sccache/kache) because warm compilation was already under a minute while
Windows test execution was 16m22s.

## Why this can work at all

The old constraint was `--test-threads=1` inside every Windows shard because
"provider doubles become unreliable when tests inside one binary run
concurrently". Reading the doubles (server/crates/laplus-server/tests/harness/)
shows two distinct failure classes were being conflated:

1. **Shared-process state.** The six http_cloudflare tests override
   process-wide env vars and serialize with an in-process `ONE_AT_A_TIME`
   mutex. This class is real — but every one of those files is
   `#![cfg(unix)]`-gated, so it never ran in the Windows shards to begin with,
   and nextest's separate process per test covers it where it does run.
2. **Load sensitivity.** The `.cmd`/PowerShell script doubles are timing-
   sensitive when the runner is saturated. This class is about how many tests
   run at once, not about which process they share.

So the constraint nextest relaxes on Windows is class 2 only: parallelism
starts at `-j 2`, not at full CPUs, and is raised only on evidence. No test
groups were configured — see `server/.config/nextest.toml` for why each
candidate is deliberately absent.

## What changed

- `server/.config/nextest.toml` — new, and intentionally near-empty: no
  retries (a pilot must see flakiness), no global thread cap (CI passes `-j`
  explicitly), no test groups (the env-var interference class is unix-only;
  reasoning is in the file).
- `.github/workflows/rust.yml` — the four Windows shards now run
  `cargo nextest run ... -j 2` with identical target selection; installs
  nextest via taiki-e/install-action. Linux, xtask, and doctest steps
  untouched. There is no `--no-fail-fast` to carry over: nextest always runs
  everything selected.
- `server/CLAUDE.md` — the "In CI" paragraph describes the new arrangement.

## What the human needs to do

Watch the four `Test (windows …)` jobs across several PR runs:

1. **Wall time per shard.** The point of the pilot. Expect roughly half of the
   old serial time if flakiness doesn't bite; record medians.
2. **Failure count and shape.** Any new failure gets triaged against the two
   classes above before touching config — a red here is signal, not noise.
3. Only after several fully green runs: raise `-j 2` to `-j 3`, then `-j 4`,
   one step at a time, same observation each time.

## Later, only if the pilot holds

- Replace the hand-maintained binary lists with nextest's built-in partitions
  (`--partition hash:m/n`) or test groups, so sharding rebalances itself when
  tests move. That was nextest's main maintainability payoff all along.
- Revisit whether the four jobs can collapse into fewer.

## Rollback

Revert the four `command:` blocks in `.github/workflows/rust.yml` to
`cargo test ... -- --test-threads=1`. Nothing else depends on nextest; local
development never required it.
