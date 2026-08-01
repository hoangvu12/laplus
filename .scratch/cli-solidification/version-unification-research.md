# One product version: current-code audit

Status: ready-for-human

This note describes live code as of 2026-08-01 and intentionally ignores tickets and ADR history.

## Conclusion

This is not hard for tagged releases. It is a small-to-medium release plumbing change, not an architectural rewrite. The npm launcher and its five platform package manifests already share one settled version. The real work is (1) replacing the UI's independent `0.0.28`, (2) making Rust and Tauri consume the same product version, and (3) settling the `workflow_dispatch` RC version before any Rust, UI, or installer build begins.

The safest model is one committed base product version, plus one optional build-time effective-version override for ephemeral RC builds. Tagged builds validate the tag against the committed base. Dispatch builds derive `<base>-rc.<run>` once and pass it to every build and staging step.

## Live sources and consumers

### Rust and Cargo

- The Cargo workspace version is `0.1.1` in `server/Cargo.toml:23-25`; `laplus-server`, `laplus-shell`, and `xtask` inherit it (`server/crates/laplus-server/Cargo.toml:1-5`, `server/crates/laplus-shell/Cargo.toml:1-5`, `server/xtask/Cargo.toml:1-5`). Cargo.lock consequently records the workspace packages at that version.
- The server advertises `env!("CARGO_PKG_VERSION")` when it has no UI (`server/crates/laplus-server/src/config.rs:502-509`) and sends the same value as its Codex client version (`server/crates/laplus-server/src/codex_protocol.rs:599-607`). A native `laplus-server --version` does not yet exist in the current parser; adding it should use the same product-version accessor, not introduce another source.
- When a UI is present, server binding replaces `serverVersion` with the bundle's version (`server/crates/laplus-server/src/server.rs:306-314`). Thus the wire value currently has two possible sources.

### Web/UI

- `apps/web/package.json:1-4` independently says `0.0.28`.
- Vite uses `APP_VERSION` from the process when non-empty, otherwise that package version, and compiles it into `import.meta.env.APP_VERSION` (`apps/web/vite.config.ts:26-32`, `apps/web/vite.config.ts:140-148`).
- Runtime `APP_VERSION` is exposed by `apps/web/src/branding.ts:12-20`. It is user-visible in Settings (`apps/web/src/components/settings/SettingsPanels.tsx:160-165`), compared by exact string equality to `environment.serverVersion` (`apps/web/src/versionSkew.ts:21-43`), and used as the tracing `service.version` (`apps/web/src/observability/clientTracing.ts:13-22`).
- The shell build script independently reads `apps/web/package.json`, generates `assets::VERSION`, and embeds it alongside the bytes (`server/crates/laplus-shell/build.rs:28-77`, `server/crates/laplus-shell/build.rs:127-175`). A shell test scans the compiled JS to ensure that generated value agrees with the Vite value (`server/crates/laplus-shell/src/main.rs:395-441`). This catches `APP_VERSION` overrides that disagree with package.json, but it does not create a shared version.
- A directory-loaded UI gets its version from `package.json` inside the bundle or one directory above it (`server/crates/laplus-server/src/ui.rs:274-304`). The plain server prints that version on startup (`server/crates/laplus-server/src/main.rs:61-72`).

### Tauri application and updater

- Tauri has another independent source, `0.1.1`, in `server/crates/laplus-shell/tauri.conf.json:1-5`. Tauri uses it for the application/bundler identity; the NSIS template receives `version`/`version_with_build` and writes product, file, and uninstall display versions (`server/crates/laplus-shell/nsis/installer.nsi:84-85`, `server/crates/laplus-shell/nsis/installer.nsi:131-136`, `server/crates/laplus-shell/nsis/installer.nsi:750`).
- The updater endpoint is configured in that same Tauri file (`server/crates/laplus-shell/tauri.conf.json:12-21`). The release workflow reads the Tauri version into `latest.json` (`.github/workflows/release.yml:259-290`), and the web bridge displays the plugin's returned `version` and `currentVersion` (`apps/web/src/shellUpdate.ts:50-56`, `apps/web/src/shellUpdate.ts:147-162`).

### npm launcher and platform packages

- The committed launcher manifest is a staging placeholder, `0.0.0` (`apps/cli/package.json:1-4`). The launcher currently owns `--version` and prints the manifest version bundled at pack time (`apps/cli/src/bin.ts:28-29`, `apps/cli/src/bin.ts:56-64`).
- Release staging already writes its supplied version into all five platform package manifests (`apps/cli/src/release.ts:62-82`, `apps/cli/src/release.ts:128-145`), rewrites the launcher manifest to it, and pins every optional platform dependency to exactly it (`apps/cli/src/release.ts:101-112`, `apps/cli/src/release.ts:157-162`). This part is already unified.
- The staged UI is the exception: `stageBundle` ignores the supplied release version and copies `apps/web/package.json`'s version into `ui/package.json` (`apps/cli/src/release.ts:171-203`). Its focused test explicitly expects `0.0.28` instead of release `1.2.3` (`apps/cli/src/release.test.ts:181-191`).

## Release flow and the RC gap

- A tagged installer build validates `vX` against Tauri's configured version (`.github/workflows/release.yml:108-130`), then builds the web bundle without a version override (`.github/workflows/release.yml:135-140`). Tagged stable releases can therefore be unified mainly by making all committed base-version fields agree.
- The five Rust server binaries are built and uploaded in the independent `server` matrix before npm staging (`.github/workflows/release.yml:430-475`).
- Only inside the later npm job is the effective version settled: tag => configured Tauri version; dispatch => `${configured}-rc.${GITHUB_RUN_NUMBER}` (`.github/workflows/release.yml:523-552`). That value is passed to staging only (`.github/workflows/release.yml:565-570`). The web build at lines 560-563 receives no `APP_VERSION`, and the already-built Rust binaries cannot contain the RC suffix.
- A dispatch with `build_installer=true` also builds the installer from the static base Tauri version, while npm independently names its packages with the RC suffix. Its generated updater manifest likewise reads the static Tauri value (`.github/workflows/release.yml:270-290`).
- Packing occurs after launcher staging so the rewritten npm manifest is inlined for `laplus --version` (`.github/workflows/release.yml:572-587`); this ordering is already correct.

## Recommended exact change

1. Treat `server/Cargo.toml`'s workspace package version as the committed base product version. Add a small repository script/check that reads it and verifies the Tauri and web manifest versions match. (A root `VERSION` file is also viable, but Cargo cannot inherit a package version from an arbitrary file, so that merely moves synchronization into generation.) Update all three committed values together for a stable bump and let Cargo refresh lockfile package entries.
2. Introduce one Rust `product_version()`/constant used by server wire metadata, Codex client info, and the forthcoming CLI `--version`. It should be compile-time `LAPLUS_VERSION` when supplied, otherwise `CARGO_PKG_VERSION`. This preserves truthful Cargo metadata in normal builds while allowing an RC binary to report the exact effective product version.
3. Settle the effective version once, before the `release` and `server` jobs. A small prerequisite job can output the validated tag version or `<base>-rc.<run>`, and all downstream jobs should consume that output.
4. Pass the effective version as `APP_VERSION` to every `pnpm build:web`. This makes Settings, tracing, skew comparison, embedded shell assets, and npm-served UI agree. Change shell build metadata to consume the same effective value (or validate its package fallback), rather than always rereading an independently mutable web manifest.
5. Pass `LAPLUS_VERSION` to every Rust build. For dispatch installer builds, provide a Tauri config override/generated overlay carrying the same RC value so installer metadata and updater `currentVersion` agree; generate `latest.json` from the shared output rather than rereading static `tauri.conf.json`.
6. Pass `version` into `stageBundle` and write it to `ui/package.json`. Update the focused release test to expect `1.2.3`. Keep launcher/platform staging as-is.
7. Add a release consistency test/check covering: committed base fields; built UI `APP_VERSION`; embedded assets version; launcher/platform/UI staged manifests; Rust reported version; Tauri/updater manifest version. Existing focused tests around shell bundle agreement and npm staging are natural homes.

## Difficulty and caveats

- **Tagged stable releases: low difficulty.** Three committed values become one synchronized base, and existing tag validation/staging does most of the work.
- **Exact RC identity everywhere: medium difficulty.** The implementation is straightforward, but the effective value must move earlier in the workflow and be injected into three build systems. No runtime protocol redesign is needed.
- Do not use a runtime environment override for product identity: it would let an installed binary claim an arbitrary version. The RC override should be compile-time only.
- The UI currently recognizes only `-nightly.YYYYMMDD.N` for the special Nightly label (`apps/web/src/branding.logic.ts:1-21`). An `-rc.N` remains under its normal fallback stage label; decide separately whether that display behavior should change. It does not block version equality.
- Updater UI tests use fixture versions to exercise behavior rather than asserting a repository product version; they need no broad rewrite. The release/update-manifest check is the important new coverage.

Overall estimate: roughly one focused implementation session plus CI rehearsal. Most edits are small; workflow ordering and proving the dispatch installer path are the parts deserving care.
