import { describe, expect, it } from "vite-plus/test";

import { missingPackageMessage, TARGETS, targetFor, unsupportedMessage } from "./platform.ts";

describe("targetFor", () => {
  it("names the package npm installs for this machine", () => {
    expect(targetFor("linux", "x64")).toEqual({
      package: "@laplus/server-linux-x64",
      binary: "laplus-server",
    });
  });

  it("asks for the .exe on Windows and nowhere else", () => {
    expect(targetFor("win32", "x64")?.binary).toBe("laplus-server.exe");
    for (const [key, target] of Object.entries(TARGETS)) {
      if (!key.startsWith("win32")) expect(target.binary).toBe("laplus-server");
    }
  });

  // 32-bit Windows and Linux on RISC-V are real machines that this does not
  // publish for, and the answer has to be a message rather than a package name
  // npm would resolve to nothing.
  it.each([
    ["win32", "ia32"],
    ["linux", "riscv64"],
    ["freebsd", "x64"],
  ])("has nothing for %s %s", (platform, architecture) => {
    expect(targetFor(platform, architecture)).toBeUndefined();
  });
});

describe("the messages", () => {
  it("names the machine it could not serve, and how to build one", () => {
    const message = unsupportedMessage("freebsd", "x64");
    expect(message).toContain("freebsd x64");
    expect(message).toContain("cargo build -p laplus-server");
  });

  it("names the package that is missing and why it might be", () => {
    const message = missingPackageMessage({ package: "@laplus/server-linux-x64", binary: "x" });
    expect(message).toContain("@laplus/server-linux-x64");
    expect(message).toContain("--no-optional");
  });
});

// Every name here is a package the release workflow has to build and publish;
// the two lists drifting apart is a platform that installs and cannot run.
describe("the published set", () => {
  it("is scoped, and named for the platform key it answers", () => {
    for (const [key, target] of Object.entries(TARGETS)) {
      expect(target.package).toBe(`@laplus/server-${key.replace(" ", "-")}`);
    }
  });
});
