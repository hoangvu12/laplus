import { describe, expect, it } from "vite-plus/test";

import { invocation } from "./invocation.ts";

const bundle = { directory: "/pkg/ui/dist", present: true };
const missing = { directory: "/pkg/ui/dist", present: false };

describe("invocation", () => {
  it("points the server at the bundle this package carries", () => {
    expect(invocation({ argv: [], bundle, environment: {} })).toEqual({
      kind: "run",
      arguments: ["--ui", "/pkg/ui/dist"],
      warnings: [],
    });
  });

  it("passes the caller's own flags through untouched", () => {
    const decided = invocation({
      argv: ["--port", "5000", "--network"],
      bundle,
      environment: {},
    });
    expect(decided).toMatchObject({
      arguments: ["--port", "5000", "--network", "--ui", "/pkg/ui/dist"],
    });
  });

  // The server refuses a flag given twice, so appending here would turn a
  // deliberate override into a server that will not start.
  it.each([["--ui", "/elsewhere"], ["--ui=/elsewhere"]])(
    "leaves a caller who said %s alone",
    (...argv) => {
      expect(invocation({ argv, bundle, environment: {} })).toEqual({
        kind: "run",
        arguments: argv,
        warnings: [],
      });
    },
  );

  it("leaves LAPLUS_UI alone, which the flag would otherwise have overridden", () => {
    expect(invocation({ argv: [], bundle, environment: { LAPLUS_UI: "/elsewhere" } })).toEqual({
      kind: "run",
      arguments: [],
      warnings: [],
    });
  });

  it("ignores an empty LAPLUS_UI, which the server ignores too", () => {
    expect(invocation({ argv: [], bundle, environment: { LAPLUS_UI: "  " } })).toMatchObject({
      arguments: ["--ui", "/pkg/ui/dist"],
    });
  });

  it("warns about a missing bundle and starts the server anyway", () => {
    const decided = invocation({ argv: ["--port", "5000"], bundle: missing, environment: {} });
    expect(decided).toMatchObject({ kind: "run", arguments: ["--port", "5000"] });
    expect(decided.kind === "run" && decided.warnings.length).toBeGreaterThan(0);
  });

  it.each(["--help", "-h"])("answers %s itself, because the server has none", (flag) => {
    expect(invocation({ argv: [flag], bundle, environment: {} })).toEqual({ kind: "help" });
  });

  it.each(["--version", "-v"])("answers %s itself", (flag) => {
    expect(invocation({ argv: [flag], bundle, environment: {} })).toEqual({ kind: "version" });
  });
});
