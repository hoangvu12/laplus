// @effect-diagnostics nodeBuiltinImport:off - Release tooling, run by `node` from a workflow rather than by an Effect runtime.
/**
 * Turn a tag and a pile of built binaries into the packages that get published.
 *
 * This lives beside the launcher rather than in `scripts/` because it is the
 * one piece of knowledge the launcher cannot be published without: what the
 * platform packages are called, what goes in them, and which version says so.
 * `platform.ts` already holds that list for the *running* half; a second copy
 * in another directory is the drift that makes a platform install and not run.
 *
 * Run by `.github/workflows/release.yml`:
 *
 * ```
 * node apps/cli/src/release.ts --version 1.2.3 --binaries downloads --out staged
 * ```
 *
 * It writes the platform packages into `--out` and rewrites `apps/cli` in
 * place, because a release runner is a machine that is thrown away afterwards
 * and copying a 17 MB bundle to avoid editing a file on it buys nothing.
 */
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";
import * as NodeURL from "node:url";
import * as NodeUtil from "node:util";

import { TARGETS } from "./platform.ts";

// The canonical `git+https://…​.git` form rather than the browser URL, because
// npm's provenance attestation matches this field against the workflow that
// built the tarball, and a URL it has to guess at is a signature it will not
// issue.
const REPOSITORY = "git+https://github.com/hoangvu12/laplus.git";

/**
 * The notice that has to travel with what is published, copied into every
 * package rather than pointed at.
 *
 * It covers both halves and both halves are now shipped separately: the UI
 * bundle upstream built, which rides in the launcher, and the Rust crates
 * compiled into the binary, which ride in the platform packages. A file in the
 * repository is not a file in a tarball somebody downloaded, and the licences it
 * answers for ask for the latter.
 */
const NOTICES = "THIRD_PARTY_NOTICES.md";

/** Where it is written. The Cargo workspace root, because `xtask` generates it. */
const NOTICES_SOURCE = `server/${NOTICES}`;

type Manifest = Record<string, unknown>;

/**
 * The manifest for one platform's package: a binary, and the two fields that
 * make npm skip it everywhere else.
 *
 * `os` and `cpu` take Node's own names, which is why [`TARGETS`] is keyed by
 * `${process.platform} ${process.arch}` — the key *is* the pair npm wants, and
 * splitting it here means the two can never disagree.
 *
 * `preferUnplugged` is for Yarn's benefit: without it a Yarn PnP install leaves
 * the binary inside a zip, where it is not a path anything can execute.
 */
export function platformManifest({
  key,
  version,
}: {
  readonly key: string;
  readonly version: string;
}): Manifest {
  const target = TARGETS[key];
  if (target === undefined) throw new Error(`${key} is not a published platform`);
  const [platform, architecture] = key.split(" ");
  return {
    name: target.package,
    version,
    description: `The laplus server binary for ${key}.`,
    license: "MIT",
    repository: { type: "git", url: REPOSITORY },
    os: [platform],
    cpu: [architecture],
    files: [target.binary, NOTICES],
    preferUnplugged: true,
  };
}

/**
 * The launcher's manifest, with the version the tag names and the platform
 * packages it may pull in.
 *
 * **The dependencies are added here rather than committed.** A package.json in
 * the tree that named `@laplus/server-linux-x64` would be a package.json that
 * `pnpm install --frozen-lockfile` has to resolve on every developer machine
 * and in every CI job — against a registry that has never heard of it before
 * the first release, and against a version that is wrong on every commit after
 * it. The published tarball is the only place the pin is true, so it is written
 * at the moment the tarball is made.
 *
 * Pinned exactly, not caret-ranged: the launcher and the binary are two halves
 * of one release, and a launcher that resolves a newer binary than the one it
 * was published with is a combination nobody tested.
 */
export function launcherManifest({
  base,
  version,
}: {
  readonly base: Manifest;
  readonly version: string;
}): Manifest {
  const optionalDependencies = Object.fromEntries(
    Object.values(TARGETS).map((target) => [target.package, version]),
  );
  return { ...base, version, optionalDependencies };
}

/** Everything the workflow does, in order, so a dry run does it too. */
export function stage({
  version,
  binaries,
  out,
  repoRoot,
}: {
  readonly version: string;
  readonly binaries: string;
  readonly out: string;
  readonly repoRoot: string;
}): readonly string[] {
  const staged: string[] = [];

  for (const [key, target] of Object.entries(TARGETS)) {
    const built = NodePath.join(binaries, artifactName(key), target.binary);
    if (!NodeFS.existsSync(built)) {
      // Every platform or none. A publish that quietly skipped the one whose
      // build failed would leave `npx laplus` installing cleanly on that
      // machine and then reporting a missing optional dependency, which reads
      // as the user's broken install rather than as our missing artifact.
      throw new Error(
        `no binary for ${key} at ${built} — its build did not finish, so this release cannot publish.`,
      );
    }

    const directory = NodePath.join(out, target.package);
    NodeFS.mkdirSync(directory, { recursive: true });
    NodeFS.writeFileSync(
      NodePath.join(directory, "package.json"),
      `${JSON.stringify(platformManifest({ key, version }), null, 2)}\n`,
    );
    NodeFS.copyFileSync(built, NodePath.join(directory, target.binary));
    // GitHub's artifact upload does not carry the executable bit, so what came
    // back down is mode 644 whatever cargo wrote. npm packs the mode it finds,
    // so without this the tarball is a binary nobody can run — and the failure
    // lands on a stranger's machine as EACCES.
    NodeFS.chmodSync(NodePath.join(directory, target.binary), 0o755);
    NodeFS.copyFileSync(NodePath.join(repoRoot, NOTICES_SOURCE), NodePath.join(directory, NOTICES));
    NodeFS.copyFileSync(NodePath.join(repoRoot, "LICENSE"), NodePath.join(directory, "LICENSE"));
    staged.push(directory);
  }

  const launcher = NodePath.join(repoRoot, "apps", "cli");
  const manifest = NodePath.join(launcher, "package.json");
  NodeFS.writeFileSync(
    manifest,
    `${JSON.stringify(launcherManifest({ base: readJson(manifest), version }), null, 2)}\n`,
  );

  NodeFS.copyFileSync(NodePath.join(repoRoot, NOTICES_SOURCE), NodePath.join(launcher, NOTICES));

  stageBundle({ launcher, repoRoot });
  staged.push(launcher);
  return staged;
}

/**
 * Put the built UI where `bin.ts` looks for it, with the version the server
 * reports beside it.
 *
 * `ui/package.json` is not decoration: `crate::ui::Assets::from_directory`
 * reads a `package.json` inside the bundle or in the directory above it, and
 * what it finds is the version the window shows — `server/docs/adr/0011`. The
 * one directory above `ui/dist` in an installed package is `ui/`, so without a
 * manifest there the server would walk into the *launcher's* package.json and
 * report the npm release's version as the UI's, which is a different number
 * that happens to look plausible.
 */
function stageBundle({
  launcher,
  repoRoot,
}: {
  readonly launcher: string;
  readonly repoRoot: string;
}): void {
  const web = NodePath.join(repoRoot, "apps", "web");
  const built = NodePath.join(web, "dist");
  if (!NodeFS.existsSync(NodePath.join(built, "index.html"))) {
    throw new Error(`no UI bundle at ${built} — run \`pnpm build:web\` before staging.`);
  }

  const into = NodePath.join(launcher, "ui");
  NodeFS.rmSync(into, { recursive: true, force: true });
  NodeFS.mkdirSync(into, { recursive: true });
  NodeFS.cpSync(built, NodePath.join(into, "dist"), { recursive: true });
  NodeFS.writeFileSync(
    NodePath.join(into, "package.json"),
    `${JSON.stringify({ name: "laplus-ui", version: readJson(NodePath.join(web, "package.json")).version, private: true }, null, 2)}\n`,
  );
}

/** The artifact one platform's build uploads, and this one downloads. */
export function artifactName(key: string): string {
  return `server-${key.replace(" ", "-")}`;
}

function readJson(path: string): Manifest {
  return JSON.parse(NodeFS.readFileSync(path, "utf8")) as Manifest;
}

if (
  process.argv[1] !== undefined &&
  import.meta.url === NodeURL.pathToFileURL(process.argv[1]).href
) {
  const { values } = NodeUtil.parseArgs({
    options: {
      version: { type: "string" },
      binaries: { type: "string" },
      out: { type: "string" },
    },
  });
  const version = values.version;
  if (version === undefined) throw new Error("--version is required");
  const repoRoot = NodePath.resolve(NodeURL.fileURLToPath(new URL("../../..", import.meta.url)));
  for (const directory of stage({
    version,
    binaries: NodePath.resolve(values.binaries ?? "binaries"),
    out: NodePath.resolve(values.out ?? "staged"),
    repoRoot,
  })) {
    process.stdout.write(`${directory}\n`);
  }
}
