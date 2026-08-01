import { describe, expect, it } from "vite-plus/test";

import { invocation } from "./invocation.ts";

const bundle = { directory: "/pkg/ui/dist", present: true };
const missing = { directory: "/pkg/ui/dist", present: false };

describe("invocation", () => {
  it("passes every argument through and supplies the bundled UI through the environment", () => {
    const argv = ["auth", "pairing", "create", "--help"];

    expect(invocation({ argv, bundle, environment: { KEEP: "yes" } })).toEqual({
      arguments: argv,
      environment: { KEEP: "yes", LAPLUS_UI: "/pkg/ui/dist" },
      warnings: [],
    });
  });

  it("preserves a caller-supplied UI environment", () => {
    expect(invocation({ argv: [], bundle, environment: { LAPLUS_UI: "/elsewhere" } })).toEqual({
      arguments: [],
      environment: { LAPLUS_UI: "/elsewhere" },
      warnings: [],
    });
  });

  it("replaces a blank UI environment with the bundled UI", () => {
    expect(invocation({ argv: [], bundle, environment: { LAPLUS_UI: "  " } })).toMatchObject({
      environment: { LAPLUS_UI: "/pkg/ui/dist" },
    });
  });

  it("warns about a missing bundle and starts the server without a UI override", () => {
    const decided = invocation({ argv: ["--version"], bundle: missing, environment: {} });

    expect(decided.arguments).toEqual(["--version"]);
    expect(decided.environment).toEqual({});
    expect(decided.warnings.length).toBeGreaterThan(0);
  });
});
