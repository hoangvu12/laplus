// @effect-diagnostics nodeBuiltinImport:off - Covers release tooling, which is `node` and a filesystem.
import * as NodeFS from "node:fs";
import * as NodeOS from "node:os";
import * as NodePath from "node:path";

import { afterEach, describe, expect, it } from "vite-plus/test";

import { TARGETS } from "./platform.ts";
import { artifactName, launcherManifest, platformManifest, stage } from "./release.ts";

const temporary: string[] = [];

afterEach(() => {
  for (const directory of temporary.splice(0)) {
    NodeFS.rmSync(directory, { recursive: true, force: true });
  }
});

function scratch(): string {
  const directory = NodeFS.mkdtempSync(NodePath.join(NodeOS.tmpdir(), "laplus-release-"));
  temporary.push(directory);
  return directory;
}

/** A repository laid out the way the real one is, with a built UI in it. */
function fakeRepo(): string {
  const root = scratch();
  NodeFS.mkdirSync(NodePath.join(root, "apps", "cli"), { recursive: true });
  NodeFS.mkdirSync(NodePath.join(root, "apps", "web", "dist", "assets"), { recursive: true });
  NodeFS.writeFileSync(
    NodePath.join(root, "apps", "cli", "package.json"),
    JSON.stringify({ name: "laplus", version: "0.0.0", bin: { laplus: "./dist/bin.mjs" } }),
  );
  NodeFS.writeFileSync(
    NodePath.join(root, "apps", "web", "package.json"),
    JSON.stringify({ name: "@t3tools/web", version: "0.0.28" }),
  );
  NodeFS.writeFileSync(NodePath.join(root, "apps", "web", "dist", "index.html"), "<html></html>");
  NodeFS.writeFileSync(NodePath.join(root, "apps", "web", "dist", "assets", "app.js"), "//");
  NodeFS.mkdirSync(NodePath.join(root, "server"), { recursive: true });
  NodeFS.writeFileSync(NodePath.join(root, "server", "THIRD_PARTY_NOTICES.md"), "# Notices");
  NodeFS.writeFileSync(NodePath.join(root, "LICENSE"), "MIT License");
  return root;
}

/** What the release workflow's downloaded artifacts look like on disk. */
function fakeBinaries(only?: readonly string[]): string {
  const root = scratch();
  for (const [key, target] of Object.entries(TARGETS)) {
    if (only !== undefined && !only.includes(key)) continue;
    const directory = NodePath.join(root, artifactName(key));
    NodeFS.mkdirSync(directory, { recursive: true });
    NodeFS.writeFileSync(NodePath.join(directory, target.binary), "ELF");
  }
  return root;
}

// The `slug` field of the release workflow's build matrix, written down. These
// five strings are a contract with a YAML file no test can read: the workflow
// uploads `server-<slug>` and this is what goes looking for it. Naming the
// artifact after the platform *key* instead — which has a space in it — is
// exactly how the first dispatch run failed.
describe("artifactName", () => {
  it("is the dashed platform key, for all five", () => {
    expect(Object.keys(TARGETS).map(artifactName)).toEqual([
      "server-darwin-arm64",
      "server-darwin-x64",
      "server-linux-arm64",
      "server-linux-x64",
      "server-win32-x64",
    ]);
  });
});

describe("platformManifest", () => {
  it("takes npm's os and cpu straight from the platform key", () => {
    expect(platformManifest({ key: "darwin arm64", version: "1.2.3" })).toMatchObject({
      name: "@laplus/server-darwin-arm64",
      version: "1.2.3",
      os: ["darwin"],
      cpu: ["arm64"],
      files: ["laplus-server", "THIRD_PARTY_NOTICES.md"],
    });
  });

  it("ships the .exe on Windows", () => {
    expect(platformManifest({ key: "win32 x64", version: "1.2.3" })).toMatchObject({
      files: ["laplus-server.exe", "THIRD_PARTY_NOTICES.md"],
    });
  });

  it("refuses a platform nothing publishes", () => {
    expect(() => platformManifest({ key: "freebsd x64", version: "1.2.3" })).toThrow(
      "not a published platform",
    );
  });
});

describe("launcherManifest", () => {
  it("pins every platform package to this release exactly", () => {
    const manifest = launcherManifest({
      base: { name: "laplus", version: "0.0.0" },
      version: "1.2.3",
    });
    expect(manifest.version).toBe("1.2.3");
    expect(manifest.optionalDependencies).toEqual({
      "@laplus/server-darwin-arm64": "1.2.3",
      "@laplus/server-darwin-x64": "1.2.3",
      "@laplus/server-linux-arm64": "1.2.3",
      "@laplus/server-linux-x64": "1.2.3",
      "@laplus/server-win32-x64": "1.2.3",
    });
  });

  it("keeps everything else the committed manifest said", () => {
    const manifest = launcherManifest({
      base: { name: "laplus", version: "0.0.0", bin: { laplus: "./dist/bin.mjs" } },
      version: "1.2.3",
    });
    expect(manifest.bin).toEqual({ laplus: "./dist/bin.mjs" });
  });
});

describe("stage", () => {
  it("writes a package per platform, with the binary in it", () => {
    const repoRoot = fakeRepo();
    const out = scratch();
    stage({ version: "1.2.3", binaries: fakeBinaries(), out, repoRoot });

    for (const target of Object.values(TARGETS)) {
      const directory = NodePath.join(out, target.package);
      expect(NodeFS.existsSync(NodePath.join(directory, "package.json"))).toBe(true);
      expect(NodeFS.readFileSync(NodePath.join(directory, target.binary), "utf8")).toBe("ELF");
    }
  });

  // The bit GitHub's artifact upload drops. Windows has no execute bit to
  // check, so the assertion is about the mode npm will pack on the runner that
  // packs it — which is Linux.
  // oxlint-disable-next-line t3code/no-global-process-runtime -- which machine is running the suite, not a decision the code under test makes.
  it.skipIf(process.platform === "win32")("makes the binaries executable", () => {
    const out = scratch();
    stage({ version: "1.2.3", binaries: fakeBinaries(), out, repoRoot: fakeRepo() });
    const linux = NodePath.join(out, "@laplus/server-linux-x64", "laplus-server");
    expect(NodeFS.statSync(linux).mode & 0o111).toBe(0o111);
  });

  // The binary carries compiled Rust crates and the launcher carries upstream's
  // web bundle, and both licences want their notice travelling with the
  // artifact rather than staying in the repository it was built from.
  it("puts the notice and the licence in everything it publishes", () => {
    const repoRoot = fakeRepo();
    const out = scratch();
    stage({ version: "1.2.3", binaries: fakeBinaries(), out, repoRoot });

    for (const target of Object.values(TARGETS)) {
      const directory = NodePath.join(out, target.package);
      expect(NodeFS.existsSync(NodePath.join(directory, "THIRD_PARTY_NOTICES.md"))).toBe(true);
      expect(NodeFS.existsSync(NodePath.join(directory, "LICENSE"))).toBe(true);
      expect(platformManifest({ key: "linux x64", version: "1.2.3" }).files).toContain(
        "THIRD_PARTY_NOTICES.md",
      );
    }

    expect(
      NodeFS.existsSync(NodePath.join(repoRoot, "apps", "cli", "THIRD_PARTY_NOTICES.md")),
    ).toBe(true);
  });

  it("refuses to publish a set with a platform missing from it", () => {
    expect(() =>
      stage({
        version: "1.2.3",
        binaries: fakeBinaries(["linux x64", "win32 x64"]),
        out: scratch(),
        repoRoot: fakeRepo(),
      }),
    ).toThrow(/no binary for/);
  });

  it("stages the UI with the product version shared by the release", () => {
    const repoRoot = fakeRepo();
    stage({ version: "1.2.3", binaries: fakeBinaries(), out: scratch(), repoRoot });

    const ui = NodePath.join(repoRoot, "apps", "cli", "ui");
    expect(NodeFS.existsSync(NodePath.join(ui, "dist", "index.html"))).toBe(true);
    expect(NodeFS.existsSync(NodePath.join(ui, "dist", "assets", "app.js"))).toBe(true);
    expect(JSON.parse(NodeFS.readFileSync(NodePath.join(ui, "package.json"), "utf8")).version).toBe(
      "1.2.3",
    );
  });

  it("refuses to stage a launcher with no UI in it", () => {
    const repoRoot = fakeRepo();
    NodeFS.rmSync(NodePath.join(repoRoot, "apps", "web", "dist"), { recursive: true });
    expect(() =>
      stage({ version: "1.2.3", binaries: fakeBinaries(), out: scratch(), repoRoot }),
    ).toThrow(/pnpm build:web/);
  });
});
