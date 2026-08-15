# What fast Rust projects actually do

Status: ready-for-human

Research date: 2026-08-15

## Answer in one paragraph

Popular Rust projects do not have a secret Cargo switch that would turn this
release into five minutes. They combine more CPU, test-aware parallelism,
compiler caches, prebuilt tools, and independent platform artifact jobs. For
laplus, the best untried experiment is a **larger Windows runner for the one
Tauri installer job**. The best test follow-up is **nextest only if it can
replace the hand-maintained shards without reintroducing the known Windows
process-double failures**. `sccache` is worth a measured pilot, but warm laplus
compilation is already under a minute and sccache cannot cache final binary,
proc-macro, or linker work. `cargo-chef`, a linker swap, and dependency surgery
do not match the measured release bottleneck.

This conclusion is based on laplus's measured runs in
[the baseline note](./research.md), the repository's current workflows, and the
primary project/configuration sources linked below.

## What comparable projects do

### Zed: powerful runners, nextest, and a remote sccache

Zed is the clearest example of what a large native Rust desktop project does
when latency matters. Its current test workflow runs Windows Clippy on a
`self-32vcpu-windows-2022` runner, Linux Clippy on a 16-vCPU runner, and restores
a remote R2-backed sccache before compiling. Its release workflow likewise runs
Windows tests on the 32-vCPU machine and invokes `cargo nextest run`; it prints
sccache statistics after the job. Zed also computes changed workspace packages
and constructs nextest reverse-dependency filters, avoiding unrelated test work
on ordinary changes. [Zed test workflow](https://github.com/zed-industries/zed/blob/main/.github/workflows/run_tests.yml),
[Zed release workflow](https://github.com/zed-industries/zed/blob/main/.github/workflows/release.yml)

What transfers to laplus:

- A larger Windows machine is credible, widely used build infrastructure, not
  speculative compiler tuning. GitHub offers Windows larger runners with 4,
  8, and 16 vCPUs (and larger sizes depending on plan). The catch is direct
  runner cost and potentially lower availability.
  [GitHub larger-runner specifications](https://docs.github.com/en/actions/reference/runners/larger-runners)
- Remote sccache is useful when many jobs and commits compile the same crates.
  Mozilla documents it as a rustc wrapper with cloud backends and a GitHub
  Actions backend. It also documents the important exclusions: crates that
  invoke the system linker—including binaries and proc macros—cannot be cached,
  nor can incremental compilation units.
  [sccache README and caveats](https://github.com/mozilla/sccache/blob/main/README.md)
- Changed-package filtering is not attractive yet: laplus's Rust workspace is
  small and the dominant socket tests all exercise the same server package.
  It becomes relevant only after the server is split into genuinely independent
  workspace crates.

### uv: nextest sharding, fast linkers, selective cache writes, and big runners

uv runs its Linux Cargo tests on a 16-vCPU Depot runner, installs `mold`, uses
`cargo nextest`, and restores `Swatinem/rust-cache`. It selectively saves that
cache rather than having every job race to update it, and prunes superseded
workspace artifacts. Its release matrix builds platforms independently and
uploads each binary as an artifact. On macOS it uses Rust's bundled LLD with
identical-code folding, with comments recording the measured binary-size gain.
[uv test workflow](https://github.com/astral-sh/uv/blob/main/.github/workflows/test.yml),
[uv release-binary workflow](https://github.com/astral-sh/uv/blob/main/.github/workflows/build-release-binaries.yml)

What transfers to laplus:

- The runner choice reinforces Zed's example. More CPU is useful for a cold
  dependency graph and optimized code generation, although the previous laplus
  profile benchmark shows its release critical path is not perfectly parallel.
- Nextest has built-in `slice:m/n` and deterministic `hash:m/n` partitions and
  can build once, archive the build, and reuse it across shard jobs. This could
  replace laplus's brittle lists of integration binaries and rebalance itself
  when tests move. [Official nextest partitioning guide](https://nexte.st/docs/ci-features/partitioning/)
- A linker experiment should stay low priority. Cargo itself warns that the
  default linker is often already fast enough and recommends measuring first.
  More importantly, laplus's measured Windows release cost includes compilation,
  Tauri/NSIS bundling, install, and uninstall; uv's `mold` example is Linux and
  does not validate a Windows MSVC/Tauri linker change.
  [Cargo build-performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html#use-an-alternative-linker)

Ruff makes the Windows comparison especially direct: upstream Windows tests use
a custom 16-vCPU/32-GB runner while forks fall back to `windows-latest`; they use
nextest but retain a separate `cargo test --doc`. That is strong evidence for
benchmarking compute before redesigning laplus's test semantics.
[Ruff CI workflow](https://github.com/astral-sh/ruff/blob/main/.github/workflows/ci.yaml)

### Tauri itself: matrices, Cargo target caching, and prebuilt helper tools

Tauri's core test workflow uses a platform/feature matrix, disables dev debug
information to reduce target-directory bloat and improve cache efficiency, and
uses `Swatinem/rust-cache`. Its CLI publish workflow builds target binaries in
parallel, uploads them, and has a small downstream job publish the collected
artifacts. It installs cross through `taiki-e/install-action` rather than
compiling the helper in the job.
[Tauri core-test workflow](https://github.com/tauri-apps/tauri/blob/dev/.github/workflows/test-core.yml),
[Tauri CLI publish workflow](https://github.com/tauri-apps/tauri/blob/dev/.github/workflows/publish-cli-rs.yml)

laplus already follows the material parts: the five server targets are parallel
jobs, Cargo targets are cached, and Tauri CLI installation uses a prebuilt
`cargo-binstall` path with a source-build fallback. The remaining installer is
intrinsically Windows-only, so there is no platform matrix available to split
that single artifact further.

### Lapce and Nushell: parallel artifacts and cache boundaries

Lapce builds Windows, Linux, distribution packages, and proxy targets as
independent jobs and uploads artifacts; its Windows job produces the installer
and portable executable from the same target tree. Nushell divides its workspace
and targets through a matrix, uses `Swatinem/rust-cache`, and explicitly enables
workspace-crate caching. These are useful confirmations of the standard pattern,
but not new levers for laplus: its release already runs the shell and five server
platforms concurrently, and the shell job is the critical path.
[Lapce release workflow](https://github.com/lapce/lapce/blob/master/.github/workflows/release.yml),
[Nushell CI workflow](https://github.com/nushell/nushell/blob/main/.github/workflows/ci.yml)

Helix shows the artifact-reuse pattern at a useful boundary: it generates
tree-sitter grammar inputs once, uploads them, and lets every platform job
download them before building; the final job collects completed artifacts.
laplus has no similarly expensive shared generated input—the web build is about
26 seconds—so artifact transfer would probably move seconds, not minutes.
[Helix release workflow](https://github.com/helix-editor/helix/blob/master/.github/workflows/release.yml)

## Ranked recommendations for laplus

### 1. Benchmark one larger Windows installer runner

Run the existing installer job unchanged on 8- and 16-vCPU Windows larger
runners and record the same step timings. This is the only remaining lever that
could materially reduce both cold Rust compilation and some bundler work without
altering the artifact or adding release ceremony.

**Decision rule:** adopt only if median tag-to-artifact time across at least
three comparable runs improves enough to justify the billed minutes. Do not
infer the benefit from Linux or from a no-LTO build. **Catches:** paid GitHub
plan/runner group setup, higher per-minute cost, and diminishing returns in
serial linking, NSIS, install, and uninstall phases.

The availability catch may decide this before any benchmark: GitHub-hosted
larger runners require an organization on Team or Enterprise Cloud. They do not
consume included minutes and are billed whenever they run. Current x64 Windows
rates are **$0.042/minute for 8 cores** and **$0.082/minute for 16 cores**, versus
the standard 2-core billing SKU at $0.010/minute; GitHub rounds partial minutes
up. A ten-minute 16-core installer job is therefore roughly $0.82 before other
release jobs. [Official Actions runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing),
[larger-runner availability](https://docs.github.com/en/actions/concepts/runners/larger-runners)

### 2. Measure the release job step-by-step before removing any check

The current `cargo xtask release --measure-install` combines optimized build,
bundling, installation, weighing, and uninstall into one Actions step. Add
timestamps inside xtask or separate timing output so the next decision is based
on the installer build versus install audit, not the aggregate ~11 minutes.
This is lower risk and more informative than copying another project's linker.

If install/uninstall is several minutes, the product decision is explicit:
retain clean-install evidence on every release, or move it to a scheduled audit
and accept that packaging failures can reach a tag. Nothing in the surveyed
projects makes that risk disappear.

### 3. Pilot nextest on Windows tests, not immediately on releases

Nextest is compelling for maintainability: built-in partitions remove the
manual target lists, can shard individual tests rather than whole binaries, and
can archive one compiled test build for all shards. It also supports repository
test groups that cap concurrency for mutually exclusive resources.
[Nextest configuration and test groups](https://nexte.st/docs/configuring-nextest/)

But laplus serializes tests because provider process doubles are unreliable
under load. A naive nextest migration could make CI faster and flaky. Pilot it
on `rust.yml`, configure the affected tests in a group with `max-threads = 1`,
and compare failures and wall time over several runs. Also preserve doctests:
nextest does not execute doctests, so the existing explicit Cargo doc-test step
must stay. If the manual four shards already reach 5–8 minutes reliably,
nextest's main payoff is automatic balancing and simpler maintenance rather
than a guaranteed large speedup.

### 4. Pilot sccache only with hit-rate reporting

Use a prebuilt sccache action/binary, set `RUSTC_WRAPPER=sccache`, retain
`CARGO_INCREMENTAL=0`, and print `sccache --show-stats`. Compare cache transfer
time, compile time, hit rate, and total job time against the existing Cargo
cache. Do not blindly stack a second large cache on every shard.

This ranks fourth because the successful warm Windows compile was already about
48 seconds while test execution was 16m22s. It may help cold release builds
after dependency changes, but its documented linker/binary/proc-macro misses
place a ceiling on the win. A shared object store also adds credentials,
retention, and cache-poisoning boundaries; the GitHub Actions backend avoids
external credentials but still consumes cache storage and transfer time.

### 5. Consider architectural dependency reduction only for product reasons

Cargo recommends inspecting timings, duplicate dependency versions, unused
features, and large crates; splitting a large binary into a library plus a thin
binary can also increase sccache coverage. Those techniques improve repeated
development builds when a stable dependency boundary exists.
[Cargo build-performance guide](https://doc.rust-lang.org/cargo/guide/build-performance.html)

Do not split laplus merely to make CI green sooner. The Tauri shell already
depends on the server library, and moving code across crates creates API and
maintenance costs. Act only if Cargo timings identify a large frequently
recompiled leaf or `cargo tree --duplicates` reveals removable duplication.

## Techniques not recommended now

- **`cargo-chef`:** it is designed to cache dependency layers in Docker builds.
  laplus builds on GitHub-hosted VMs and already restores Cargo outputs with
  `Swatinem/rust-cache`; it would add a recipe/planner stage without touching
  test execution or NSIS. [cargo-chef README](https://github.com/LukeMathWalker/cargo-chef)
- **More release-profile tuning:** the laplus benchmark already found Thin LTO
  saved only 49 seconds for 1.90 MiB of executable growth. Keep the size-tuned
  production profile.
- **A Windows linker change without timings:** Cargo's linker advice is
  conditional, and the surveyed production use of LLD/mold was platform- and
  measurement-specific. MSVC compatibility and signing/bundling verification
  would be required for a small likely ceiling.
- **Building artifacts on every commit:** uv and Zed can afford extensive
  infrastructure, but the user explicitly wants tag-driven releases and pushes
  frequently. Keep `push v* -> build -> publish`; spend money or engineering on
  making that path faster, not on moving the same work to every commit.
- **More artifact-job splitting:** laplus already builds the shell and server
  platforms concurrently. Splitting serial phases of one signed Windows
  installer across machines would require transferring large target trees and
  carefully preserving signing inputs, likely costing more than it saves.

## Practical target

With exact-commit CI reuse, the release should no longer repeat the 16–20 minute
Windows suite. The realistic next objective is therefore to reduce the roughly
11-minute installer step. A larger-runner benchmark can show whether **6–9
minutes total release latency** is attainable without weakening checks. None of
the primary-source examples supports promising five minutes on standard
`windows-latest`; if the larger runner does not materially improve the isolated
build phase, the remaining wait is product packaging work rather than missing
CI configuration.
