# ADR-0026 — The npm launcher ships a binary rather than building one

Date: 2026-07-30
Status: Accepted

## Context

Upstream's answer to "how does somebody try this without cloning it" is one
line: `npx t3@latest`. It works because `pingdotgg/t3code`'s server is Node.
`apps/server` is published as the package `t3`, `vp pack` folds the whole Effect
server _and_ the web bundle into a single `dist/bin.mjs`, and one tarball then
runs on every machine that has Node 22 or newer.

The same sentence is worth having here — it is the difference between an
installer somebody has to find on a Releases page and a command they can paste —
and none of the mechanism transfers. ADR-0014 moved the server into Rust.
`laplus-server` is a compiled artifact per platform, and `apps/web/dist` is not
inside it: ADR-0010 has the shell compile the bundle in through
`laplus-shell/build.rs`, and the plain server serves pages only when `--ui`
points it at a directory.

So three questions, and this records the answer to each.

## Decision

### One launcher, five binaries, resolved by npm

`laplus` is a package with no dependencies and no runtime that finds a prebuilt
`laplus-server` and runs it. The binaries are `@laplus/server-<platform>-<arch>`
packages, declared as `optionalDependencies` with npm's `os` and `cpu` fields,
so an install downloads exactly one of them and skips the rest.

The alternative was a `postinstall` script that downloads the right binary from
the GitHub Release the installer already goes to. It was rejected because
`--ignore-scripts` is a supported, common and increasingly default-ish way to
install, and it disables that script **silently** — leaving a package that
installs cleanly and cannot run, with nothing at install time to say why. The
optional-dependency shape has no such state: either npm placed the binary or the
error at startup can say precisely which package is missing.

`apps/cli/src/platform.ts` holds the platform table, and it is the same list the
release workflow builds and `apps/cli/src/release.ts` refuses to publish without.
One list, three readers.

### The UI rides in the launcher, not in the binary

`npx laplus` has to serve a page, and there were two ways to give it one: a
cargo feature that embeds `apps/web/dist` into `laplus-server` the way
`laplus-shell` already does, or the bundle as files in the npm package with
`--ui` pointed at them.

The bundle travels as files. It costs no Rust change at all — `crate::ui` is
already written as policy over a table handed in at startup, and
`Assets::from_directory` is already the path `--ui` takes — and it keeps the
17 MB out of five binaries, where it would otherwise be copied five times to be
downloaded once.

It also keeps ADR-0011 true by accident, and then on purpose. `Assets` reports
the version it finds in a `package.json` inside the bundle or in the directory
above it, so `ui/package.json` is staged beside `ui/dist` carrying **the UI's**
version. Without that file the search would walk up into the launcher's own
manifest and report the npm release's version as the UI's — a different number
that looks entirely plausible.

### Only the platforms CI compiles

Five: Windows x64, Linux x64 and arm64, macOS on both architectures. The Linux
pair are built on Ubuntu 24.04 and therefore need glibc 2.39, which rules out
Debian 12 and RHEL 9; the Intel Mac binary is cross-compiled from the Apple
Silicon runner rather than built on a runner label Apple's hardware transition
keeps taking away.

A sixth name in the table with no build behind it would be a package npm
resolves to nothing — the same failure as an unsupported platform, arriving
later and reading worse. So the table is the matrix, `stage` refuses to assemble
a release with a platform missing from it, and anything outside the five is told
plainly to build from source. `laplus-server` is a plain cargo binary and
`server/docs/running-headless.md` is the page for that.

## Consequences

- **`npx laplus` is a supported entry point, and a commitment.** A tag now
  publishes six npm packages as well as an installer, and the `@laplus` scope
  plus an `NPM_TOKEN` secret are things this repository depends on.
- **The version is written at release time, not committed.** `apps/cli`'s
  manifest carries `0.0.0` and no `optionalDependencies` in the tree, because a
  committed pin would be a version that is wrong on every commit and a package
  the registry had never heard of before the first release —
  `pnpm install --frozen-lockfile` would have to resolve it on every developer
  machine. `launcherManifest` writes both at the moment the tarball is made.
- **The order of publication matters.** The launcher pins its binaries exactly,
  so the binaries go up first; a launcher published first is an install that
  resolves against versions the registry does not have yet.
- **`THIRD_PARTY_NOTICES.md` now travels three ways rather than one.** It was
  written for the installer, and it answers for both halves of what is
  published here — upstream's web bundle, which rides in the launcher, and the
  Rust crates compiled into the binaries. `stage` copies it into all six
  packages, because a file in this repository is not a file in the tarball
  somebody downloaded.
- **Two version numbers, still.** The npm release takes the tag, which is
  `tauri.conf.json`'s number; the UI reports its own. ADR-0011 already argued
  that they differ on purpose, and this adds a third place the distinction has
  to be got right rather than a third number.
- **The desktop app is untouched.** It still embeds its bundle, still installs
  from a Release, and still updates through the Tauri updater. This is a second
  way in, not a replacement for the first — ADR-0020 stands.
