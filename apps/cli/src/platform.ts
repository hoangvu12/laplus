/**
 * Which package carries the server binary for the machine this is running on.
 *
 * The binary is Rust, so unlike the reference server there is no one artifact
 * that runs everywhere: `npx laplus` has to land a *different* executable on
 * Windows than on Linux. The mechanism is npm's own — one package per platform,
 * declared as `optionalDependencies` with `os` and `cpu` fields, so the
 * installer picks the single one that matches and skips the rest. It is what
 * esbuild, swc and rolldown do, and it is the only shape that survives
 * `--ignore-scripts`: a `postinstall` downloader is a network fetch that a
 * hardened install disables silently, leaving a command that installs cleanly
 * and cannot run.
 *
 * **The table is the list of platforms CI compiles**, and it is deliberately
 * not longer than that. `.github/workflows/release.yml` builds exactly these
 * five and the publish step refuses to run unless all five arrived; a sixth
 * name here would be a package npm resolves to nothing, which is the same
 * failure as an unsupported platform but arriving later and reading worse.
 */

/** The package that carries a machine's binary, and what it is called inside. */
export type Target = {
  readonly package: string;
  readonly binary: string;
};

/**
 * Keyed by `${process.platform} ${process.arch}` — Node's own names, so nothing
 * here has to translate between Node's vocabulary and Rust's. The mapping to
 * Rust target triples lives in the release workflow, which is the one place
 * that needs it.
 */
export const TARGETS: Readonly<Record<string, Target>> = {
  "darwin arm64": { package: "@laplus/server-darwin-arm64", binary: "laplus-server" },
  "darwin x64": { package: "@laplus/server-darwin-x64", binary: "laplus-server" },
  "linux arm64": { package: "@laplus/server-linux-arm64", binary: "laplus-server" },
  "linux x64": { package: "@laplus/server-linux-x64", binary: "laplus-server" },
  "win32 x64": { package: "@laplus/server-win32-x64", binary: "laplus-server.exe" },
};

/** The package for this platform and architecture, or nothing if there is none. */
export function targetFor(platform: string, architecture: string): Target | undefined {
  return TARGETS[`${platform} ${architecture}`];
}

/**
 * What to say to somebody whose machine has no published binary.
 *
 * It names the platform rather than only the supported set, because the first
 * question a reader has is whether the tool has mistaken their machine for
 * something else — and it points at building from source, which is a real
 * answer here: `laplus-server` is a plain cargo binary, and
 * `server/docs/running-headless.md` is the page for it.
 */
export function unsupportedMessage(platform: string, architecture: string): string {
  const supported = Object.keys(TARGETS).sort().join(", ");
  return [
    `laplus: no published server binary for ${platform} ${architecture}.`,
    `laplus: published binaries: ${supported}.`,
    "laplus: build one with `cargo build -p laplus-server --release` — see",
    "laplus: https://github.com/hoangvu12/laplus/blob/main/server/docs/running-headless.md",
  ].join("\n");
}

/**
 * What to say when the platform is supported and its package is not installed.
 *
 * Two ways to get here and the message has to serve both. `npm install
 * --no-optional` and `--ignore-optional` skip every optional dependency by
 * definition, and a lockfile committed by a machine of one platform, installed
 * on another with `--frozen-lockfile`, can resolve to a set that has the wrong
 * binary in it. Neither is a bug in this package, and both look identical from
 * here: the package this platform needs is simply not on disk.
 */
export function missingPackageMessage(target: Target): string {
  return [
    `laplus: the server binary for this platform is not installed (${target.package}).`,
    "laplus: it is an optional dependency, so `--no-optional`, `--ignore-optional`",
    "laplus: or a lockfile resolved on another platform will have skipped it.",
    "laplus: reinstall without those flags, or run `npx laplus@latest` to install fresh.",
  ].join("\n");
}
