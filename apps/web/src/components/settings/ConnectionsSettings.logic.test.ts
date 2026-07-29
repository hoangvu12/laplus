import { describe, expect, it } from "vite-plus/test";

import { formatRemoteBackendHost } from "./ConnectionsSettings.logic";

describe("the host a remote environment was paired with", () => {
  /**
   * Ticket 06 of the headless-Linux effort gave every laplus its own
   * environment id, and its drive found that the id is never shown: the saved
   * row renders `environment.label`, which is the machine's hostname, so two
   * data directories on one machine produce two identical rows. This is the
   * text that tells them apart, and the port is the whole reason it is the host
   * rather than the label.
   */
  it("is the host and the port, which is what distinguishes two servers on one machine", () => {
    expect(formatRemoteBackendHost("http://127.0.0.1:5774")).toBe("127.0.0.1:5774");
    expect(formatRemoteBackendHost("http://127.0.0.1:5775")).toBe("127.0.0.1:5775");
    expect(formatRemoteBackendHost("http://192.168.1.42:4773/")).toBe("192.168.1.42:4773");
  });

  /**
   * A default port is not shown, because `laplus.example.com:443` is noise on
   * the one shape where the hostname is already unique — a tunnel, which is how
   * the 2026-07-30 phone drive reached its box.
   */
  it("drops a default port", () => {
    expect(formatRemoteBackendHost("https://laplus.example.com")).toBe("laplus.example.com");
    expect(formatRemoteBackendHost("https://laplus.example.com:443")).toBe("laplus.example.com");
    expect(formatRemoteBackendHost("http://box.example.com:80")).toBe("box.example.com");
  });

  /**
   * **A stored profile is not a value this can validate.** It was written by an
   * older build, or by hand, or by a version that stored something else — and
   * the answer to any of those is to show what is there rather than to render
   * nothing, because nothing is the bug this function exists to fix.
   */
  it("shows what it was given when that is not a URL", () => {
    expect(formatRemoteBackendHost("not a url")).toBe("not a url");
    expect(formatRemoteBackendHost("  spaced.example.com  ")).toBe("spaced.example.com");
  });

  /** Nothing to say rather than an empty line under the label. */
  it("has no answer for an empty value", () => {
    expect(formatRemoteBackendHost("")).toBe(null);
    expect(formatRemoteBackendHost("   ")).toBe(null);
  });
});
