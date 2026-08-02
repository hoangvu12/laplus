import { describe, expect, it } from "vite-plus/test";
import * as Schema from "effect/Schema";

import { ExternalTunnelEndpointSnapshot } from "./remoteAccess.ts";

const decode = Schema.decodeUnknownSync(ExternalTunnelEndpointSnapshot);

describe("ExternalTunnelEndpointSnapshot", () => {
  it("decodes every closed verification state and failure kind", () => {
    for (const verificationState of ["unconfigured", "pending", "verified", "failed"] as const) {
      expect(
        decode({
          configured: verificationState !== "unconfigured",
          httpsOrigin: verificationState === "unconfigured" ? null : "https://laplus.example.com",
          wssOrigin: verificationState === "unconfigured" ? null : "wss://laplus.example.com",
          ownership: "external",
          health: { connector: "external", https: "unknown", webSocket: "unknown" },
          verificationState,
          failureKind: null,
          failureMessage: null,
          lastAttemptAt: null,
          lastVerifiedAt: null,
          advertisedEndpoint: null,
        }).verificationState,
      ).toBe(verificationState);
    }
    for (const failureKind of [
      "dns",
      "tls",
      "destination",
      "http",
      "identity",
      "wrong-environment",
      "authentication",
      "websocket",
      "cloudflare-access",
      "cloudflare-access-websocket",
    ] as const) {
      expect(
        decode({
          configured: true,
          httpsOrigin: "https://laplus.example.com",
          wssOrigin: "wss://laplus.example.com",
          ownership: "external",
          health: { connector: "external", https: "failed", webSocket: "unknown" },
          verificationState: "failed",
          failureKind,
          failureMessage: "Verification failed.",
          lastAttemptAt: "2026-08-02T10:00:00.000Z",
          lastVerifiedAt: null,
          advertisedEndpoint: null,
        }).failureKind,
      ).toBe(failureKind);
    }
  });

  it("decodes verified external ownership and its advertised HTTPS/WSS endpoint", () => {
    const snapshot = decode({
      configured: true,
      httpsOrigin: "https://laplus.example.com",
      wssOrigin: "wss://laplus.example.com",
      ownership: "external",
      health: { connector: "external", https: "healthy", webSocket: "healthy" },
      verificationState: "verified",
      failureKind: null,
      failureMessage: null,
      lastAttemptAt: "2026-08-02T10:00:00.000Z",
      lastVerifiedAt: "2026-08-02T10:00:00.000Z",
      diagnosticCredential: "must-not-cross-the-contract",
      advertisedEndpoint: {
        id: "cloudflare-external:https://laplus.example.com",
        label: "Cloudflare Tunnel",
        provider: { id: "cloudflare", label: "Cloudflare Tunnel", kind: "tunnel", isAddon: true },
        httpBaseUrl: "https://laplus.example.com",
        wsBaseUrl: "wss://laplus.example.com",
        reachability: "public",
        compatibility: { hostedHttpsApp: "compatible", desktopApp: "compatible" },
        source: "user",
        status: "available",
      },
    });

    expect(snapshot.advertisedEndpoint?.status).toBe("available");
    expect(snapshot).not.toHaveProperty("diagnosticCredential");
  });

  it("keeps a distinct Cloudflare Access interception outcome", () => {
    const snapshot = decode({
      configured: true,
      httpsOrigin: "https://laplus.example.com",
      wssOrigin: "wss://laplus.example.com",
      ownership: "external",
      health: { connector: "external", https: "failed", webSocket: "unknown" },
      verificationState: "failed",
      failureKind: "cloudflare-access",
      failureMessage: "An HTML access page intercepted the environment descriptor.",
      lastAttemptAt: "2026-08-02T10:00:00.000Z",
      lastVerifiedAt: null,
      advertisedEndpoint: null,
    });
    expect(snapshot.failureKind).toBe("cloudflare-access");
  });

  it("can retain HTTPS health when Access intercepts only the WebSocket upgrade", () => {
    const snapshot = decode({
      configured: true,
      httpsOrigin: "https://laplus.example.com",
      wssOrigin: "wss://laplus.example.com",
      ownership: "external",
      health: { connector: "external", https: "healthy", webSocket: "failed" },
      verificationState: "failed",
      failureKind: "cloudflare-access-websocket",
      failureMessage: "An access page intercepted the WebSocket upgrade.",
      lastAttemptAt: "2026-08-02T10:00:00.000Z",
      lastVerifiedAt: null,
      advertisedEndpoint: null,
    });
    expect(snapshot.health).toEqual({
      connector: "external",
      https: "healthy",
      webSocket: "failed",
    });
  });
});
