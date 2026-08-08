// @effect-diagnostics nodeBuiltinImport:off - Covers release tooling, which is `node` and a filesystem.
import * as NodeFS from "node:fs";
import * as NodePath from "node:path";

import { describe, expect, it } from "vite-plus/test";

const ROOT = NodePath.resolve(import.meta.dirname, "../../..");

describe("committed product version", () => {
  it("is shared by Cargo, the web UI, and Tauri", () => {
    const cargo = NodeFS.readFileSync(NodePath.join(ROOT, "server/Cargo.toml"), "utf8");
    const cargoVersion = cargo.match(/^version = "([^"]+)"$/m)?.[1];
    const web = JSON.parse(
      NodeFS.readFileSync(NodePath.join(ROOT, "apps/web/package.json"), "utf8"),
    );
    const tauri = JSON.parse(
      NodeFS.readFileSync(
        NodePath.join(ROOT, "server/crates/laplus-shell/tauri.conf.json"),
        "utf8",
      ),
    );

    expect(cargoVersion).toBeDefined();
    expect(web.version).toBe(cargoVersion);
    expect(tauri.version).toBe(cargoVersion);
  });
});
