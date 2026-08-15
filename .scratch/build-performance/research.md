# Build-performance research

Status: ready-for-human

Research date: 2026-08-15

## Bottom line

The 20-minute wait is not primarily a pnpm or Vite problem. Recent successful
GitHub runs show the TypeScript workflow completing in **2m36s**, including a
**26s** web build. The Rust workflow takes **19m20s** because the Windows test
step alone takes **16m22s**. The v0.1.4 release took **36m29s**: its Windows
test step took **20m19s**, followed by **10m57s** to build, install, and measure
the Tauri installer.

Evidence: [CI run 31732757151](https://github.com/hoangvu12/laplus/actions/runs/31732757151),
[Rust run 31814886103](https://github.com/hoangvu12/laplus/actions/runs/31814886103), and
[release run 31732758904](https://github.com/hoangvu12/laplus/actions/runs/31732758904).
The timings above come from those runs' job/step timestamps queried with
`gh run view <id> --json jobs`.

The highest-value plan is therefore:

1. Stop treating “build everything” as the normal edit/verify loop. Use the
   repository's already-narrow commands for the half that changed.
2. Instrument the slow Windows test suite and production shell build before
   changing compiler settings.
3. Split the Windows integration-test binaries into a small balanced CI matrix,
   while preserving `--test-threads=1` inside each binary.
4. Keep the size-tuned production profile for actual artifacts, but introduce a
   faster non-shipping profile if developers currently invoke a release shell
   build merely to validate it.
5. Only after timings show linking is material, test reduced debug information
   and a faster linker for local Linux development.

## What is actually slow

### TypeScript is already comparatively fast

The successful CI run spent 6s installing dependencies, 14s typechecking, 86s
testing, 2s linting, and 26s building the web bundle. The release workflow
builds the same web bundle twice in parallel jobs, but each build is only
27–31s and neither is on the release critical path after the long installer
job begins. Caching is already configured through `actions/setup-node` with
`cache: pnpm`; GitHub documents this as the supported minimal configuration for
pnpm dependency caching. [GitHub dependency-caching reference](https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching)

Conclusion: do not start with a remote JS build cache or a bundler migration.
Together they can save tens of seconds here, not the stressful 20 minutes.

### Windows test execution is the dominant feedback delay

In Rust run 31814886103, compilation after cache restore took 48s, but test
execution took 16m22s. Linux completed its entire job in 4m56s. The Windows log
shows especially slow integration binaries including `socket_opencode_turn`
(151.46s) and `socket_codex_turn` (67.75s), followed by several 10–20s socket
binaries. The workflow intentionally passes `--test-threads=1`, so Cargo runs
every test within an integration binary serially; Cargo also invokes the many
integration binaries one after another.

This serialization has a documented reliability reason in `rust.yml`, so the
safe speed lever is **CI-level sharding by integration binary**, not simply
removing `--test-threads=1`. GitHub supports matrix jobs specifically to run
variations in parallel. [GitHub matrix documentation](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/run-job-variations)

A practical pilot:

- Keep one Windows compile/check job.
- Add 3–4 Windows test shards, each listing integration test targets with
  `cargo test -p laplus-server --test <name> -- --test-threads=1`.
- Put `socket_opencode_turn` alone or with only tiny binaries; balance the
  remaining shards from measured run times.
- Keep a small unit/doc/`xtask` shard so coverage does not silently disappear.
- Reuse the same Rust cache inputs in every shard and compare total runner
  minutes as well as wall-clock latency. This trades more runner consumption
  for substantially shorter feedback.

The existing separate `cargo build` before `cargo test` costs another ~48s on
cached Windows runs and is not required for compilation coverage because
`cargo test` compiles its selected targets. Keep it only if the clearer failure
classification is worth that minute. Similarly, non-gating Clippy adds ~75s;
it can run in parallel with tests in a separate job if faster red/green feedback
is more valuable than runner-minutes. GitHub jobs run concurrently unless
ordered with `needs`. [GitHub jobs documentation](https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-jobs)

### Release repeats the slowest tests, then deliberately performs an expensive build

The v0.1.4 installer job runs the same serial Windows suite for 20m19s and then
spends 10m57s in `cargo xtask release --measure-install`. That production build
uses:

```toml
[profile.release]
strip = true
lto = true
codegen-units = 1
opt-level = "z"
```

These settings optimize artifact size, not build latency. Cargo documents that
fat LTO costs longer link time, while additional codegen units allow more
parallel work and can reduce compile time. It also supports custom profiles for
separate workflows. [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)

Do not weaken the real release profile without re-running the project's
artifact-size and runtime checks. Instead:

- For local shell validation, use the normal dev profile unless an installer is
  actually required.
- If a release-shaped local artifact is necessary, benchmark a custom
  `release-fast` profile inheriting `release` but using `lto = "thin"` (or off)
  and more codegen units. Cargo says thin LTO takes substantially less time than
  fat LTO while retaining similar performance gains; this artifact must remain
  explicitly non-shipping.
- For tagged releases, keep the tuned profile. The 11-minute artifact build is
  the work that produces the product users receive.
- Avoid running the full serial Windows suite twice for the same commit. A
  robust implementation should make the release depend on a successful,
  immutable test result for the exact commit (for example through a reusable
  workflow or an explicit commit-status gate), while retaining a safe path for
  tags whose commit has not been tested. Do not merely delete the release test
  step: current path filters mean a tag itself does not trigger `rust.yml`.

The release workflow already makes several sound choices: it disables
incremental compilation on disposable runners, caches Cargo outputs, excludes
Cargo directories from Defender, downloads a prebuilt Tauri CLI, and runs the
five server platform builds concurrently. Those are not the next bottleneck.

## Measure before changing Cargo settings

Add an opt-in diagnostic workflow or run locally on the slow command:

```bash
cd server
cargo build -p laplus-shell --release --timings
```

Cargo writes `target/cargo-timings/cargo-timing.html`, showing the critical
path, crate/codegen durations, build scripts, and concurrency. Cargo's own
guidance is to use it to find slow dependencies and features, duplicate crate
versions, large crates, and crates blocking many downstream units.
[Cargo timings](https://doc.rust-lang.org/cargo/reference/timings.html)

Upload that HTML as a diagnostic artifact on manual runs. Also record:

- cold build, warm no-op build, and one-file edit rebuild separately;
- cache restore/save duration and size;
- compile time versus test execution time;
- release binary and installer size for every profile experiment.

This distinction matters because the current GitHub evidence says the cached
Rust **compile** is already under a minute in routine CI; most of the wait is
test process execution. Compiler-cache tuning cannot fix that part.

## Low-risk local Rust experiments

Cargo's current build-performance guide recommends reducing dev debug
information:

```toml
[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
debug = false
```

This retains useful panic locations for workspace code, omits dependency debug
information, and is expected to reduce code generation, link time, and target
disk usage. Provide a separate full-debug profile for debugger sessions.
[Cargo build-performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html#reduce-amount-of-generated-debug-information)

On Linux only, measure `mold` or LLD after `--timings` confirms linking matters.
Cargo notes that alternative linkers can make link-heavy incremental builds
faster, but may not support every C/C++ dependency. That caveat matters here
because `rusqlite` compiles bundled SQLite. Do not make a linker mandatory
until clean builds and tests pass on supported Linux environments.
[Cargo alternative-linker guidance](https://doc.rust-lang.org/cargo/guide/build-performance.html#use-an-alternative-linker)

Cargo documents `RUSTC_WRAPPER=sccache` as a way to share compiled dependencies
across workspaces. It may help developers or ephemeral CI after measuring cache
hit rates, but the existing `Swatinem/rust-cache` warm-run compile times suggest
test sharding has much higher immediate value. Do not layer caches blindly:
GitHub notes that caches have restore/upload cost and should contain expensive,
reusable outputs. [Cargo build cache](https://doc.rust-lang.org/cargo/reference/build-cache.html),
[GitHub caching concepts](https://docs.github.com/en/actions/concepts/workflows-and-actions/dependency-caching)

## Vite/pnpm follow-ups, after Rust

If the goal later becomes shaving the remaining ~30s web build, Vite provides
`vite --debug plugin-transform` and `vite --profile` to identify slow plugin
hooks. That is especially relevant because the web config runs TanStack Router,
React, Babel's React Compiler preset, and Tailwind plugins. Profile before
removing any of them; each has behavior attached. Vite also recommends avoiding
barrel imports, reducing resolution work, and dynamically importing large
dependencies used only on some paths. [Vite performance guide](https://vite.dev/guide/performance.html)

The pnpm store is already cached correctly through setup-node. `pnpm install`
takes only 6s in the successful CI run and 20s in the Windows release job, so
caching `node_modules` itself would add complexity and cache-transfer risk for
little critical-path gain. Keep the frozen lockfile and existing store cache.

## Recommended order of work

1. Document fast local commands by change scope (`pnpm build:web`, focused
   `vp test run`, `cargo test -p laplus-server --test <name>`, and dev shell
   builds). Never use the size-tuned release build as routine validation.
2. Capture Cargo timing HTML for cold/warm shell builds and retain one baseline.
3. Extract Windows integration-binary durations from CI and prototype 3–4
   balanced test shards, preserving per-binary serialization.
4. Parallelize non-gating Clippy or decide explicitly that its extra minute is
   worth sequential simplicity.
5. Design an exact-commit test gate so releases can reuse prior Windows test
   evidence rather than execute the full suite again.
6. Benchmark dev debug-info reduction. Try a faster Linux linker only if the
   timing report shows link time on the critical path.
7. Benchmark a clearly non-shipping `release-fast` shell profile against the
   production profile, reporting time and size together.

Success should be measured as two separate targets: routine PR feedback below
roughly 7 minutes through Windows sharding, and an unchanged production artifact
with the tagged release's unavoidable optimized build still measured honestly.
