import { describe, expect, it } from "vite-plus/test";
import * as Schema from "effect/Schema";

import {
  CloudflareAccountSnapshot,
  CloudflaredInstallationSnapshot,
  ExternalTunnelEndpointSnapshot,
  ManagedCloudflareConnectorSnapshot,
  TunnelOwnership,
} from "./remoteAccess.ts";

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
          deletableAtCloudflare: false,
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
          deletableAtCloudflare: false,
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
      deletableAtCloudflare: false,
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
      deletableAtCloudflare: false,
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
      deletableAtCloudflare: false,
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

describe("ManagedCloudflareConnectorSnapshot", () => {
  it("keeps local connector readiness independent from public verification", () => {
    const decodeManaged = Schema.decodeUnknownSync(ManagedCloudflareConnectorSnapshot);
    const snapshot = decodeManaged({
      configured: true,
      ownership: "laplus",
      tunnelOwnership: "laplus-created",
      deletableAtCloudflare: true,
      desiredState: "running",
      connectorState: "ready",
      readiness: true,
      httpsOrigin: "https://laplus.example.com",
      loopbackOrigin: "http://127.0.0.1:4773",
      executablePath: "/usr/bin/cloudflared",
      detectedVersion: "2026.7.0",
      metricsOrigin: "http://127.0.0.1:12345",
      failureMessage: null,
      restartCount: 0,
      logs: ["connector established"],
      verificationState: "failed",
      failureKind: "websocket",
      publicFailureMessage: "The WebSocket upgrade failed.",
      lastVerifiedAt: null,
    });

    expect(snapshot.readiness).toBe(true);
    expect(snapshot.verificationState).toBe("failed");
    expect(snapshot).not.toHaveProperty("connectorToken");
    // Who runs the connector and who owns the tunnel are two answers, and they
    // were one string literal each until the ownership model landed. laplus
    // supervises this connector; the tunnel behind it is laplus's to delete.
    expect(snapshot.ownership).toBe("laplus");
    expect(snapshot.tunnelOwnership).toBe("laplus-created");
  });

  /**
   * Ticket 07's whole stop/forget/delete matrix is indexed by this value, so
   * all three have to survive the wire — and an invented fourth must not.
   */
  it("decodes every tunnel ownership and refuses one it does not know", () => {
    const decodeManaged = Schema.decodeUnknownSync(ManagedCloudflareConnectorSnapshot);
    const base = {
      configured: true,
      ownership: "laplus",
      deletableAtCloudflare: false,
      desiredState: "running",
      connectorState: "ready",
      readiness: true,
      httpsOrigin: "https://laplus.example.com",
      executablePath: "/usr/bin/cloudflared",
      detectedVersion: "2026.7.0",
      metricsOrigin: "http://127.0.0.1:12345",
      failureMessage: null,
      restartCount: 0,
      logs: [],
      verificationState: "verified",
      failureKind: null,
      publicFailureMessage: null,
      lastVerifiedAt: "2026-08-03T09:00:00.000Z",
    };
    for (const tunnelOwnership of TunnelOwnership.literals) {
      // The server states the deletion verdict beside the ownership rather than
      // leaving a client to derive it, so that "Delete everywhere is never
      // offered for an adopted tunnel" is one answer rather than two that could
      // disagree. Only `laplus-created` is laplus's to delete — ADR-0045.
      const deletableAtCloudflare = tunnelOwnership === "laplus-created";
      const snapshot = decodeManaged({ ...base, tunnelOwnership, deletableAtCloudflare });
      expect(snapshot.tunnelOwnership).toBe(tunnelOwnership);
      expect(snapshot.deletableAtCloudflare).toBe(deletableAtCloudflare);
    }
    expect(TunnelOwnership.literals).toEqual(["external", "adopted", "laplus-created"]);
    expect(() => decodeManaged({ ...base, tunnelOwnership: "cloudflare" })).toThrow();
  });

  it("represents an unconfigured connector without exposing a secret", () => {
    const decodeManaged = Schema.decodeUnknownSync(ManagedCloudflareConnectorSnapshot);
    const snapshot = decodeManaged({
      configured: false,
      ownership: "laplus",
      tunnelOwnership: "external",
      deletableAtCloudflare: false,
      desiredState: "stopped",
      connectorState: "stopped",
      readiness: null,
      httpsOrigin: null,
      executablePath: null,
      detectedVersion: null,
      metricsOrigin: null,
      failureMessage: null,
      restartCount: 0,
      logs: [],
      verificationState: "unconfigured",
      failureKind: null,
      publicFailureMessage: null,
      lastVerifiedAt: null,
    });

    expect(snapshot.configured).toBe(false);
  });
});

describe("CloudflaredInstallationSnapshot", () => {
  it("previews the exact release an installation would fetch", () => {
    const decodeInstallation = Schema.decodeUnknownSync(CloudflaredInstallationSnapshot);
    const snapshot = decodeInstallation({
      supported: true,
      platform: "linux",
      architecture: "x86_64",
      assetName: "cloudflared-linux-amd64",
      ownership: "app-managed",
      unsupportedMessage: null,
      state: "not-installed",
      installedPath: null,
      installedVersion: null,
      detectedVersion: null,
      installedAt: null,
      failureMessage: null,
      release: {
        version: "2026.7.3",
        assetName: "cloudflared-linux-amd64",
        downloadUrl:
          "https://github.com/cloudflare/cloudflared/releases/download/2026.7.3/cloudflared-linux-amd64",
        checksum: "9d71c677db00134c1bd4144b7783486b654ad281b1ea62b4972098d19f770f17",
      },
      releaseFailureMessage: null,
    });

    expect(snapshot.release?.version).toBe("2026.7.3");
    expect(snapshot.ownership).toBe("app-managed");
  });

  it("decodes every installation state, including a platform with no offer", () => {
    const decodeInstallation = Schema.decodeUnknownSync(CloudflaredInstallationSnapshot);
    for (const state of ["not-installed", "installing", "installed", "failed"] as const) {
      expect(
        decodeInstallation({
          supported: true,
          platform: "linux",
          architecture: "x86_64",
          assetName: "cloudflared-linux-amd64",
          ownership: "app-managed",
          unsupportedMessage: null,
          state,
          installedPath:
            state === "installed" ? "/data/cloudflare/tools/cloudflared-2026.7.3" : null,
          installedVersion: state === "installed" ? "2026.7.3" : null,
          detectedVersion: state === "installed" ? "cloudflared version 2026.7.3" : null,
          installedAt: state === "installed" ? "2026-08-02T10:00:00.000Z" : null,
          failureMessage: state === "failed" ? "The checksum did not match." : null,
          release: null,
          releaseFailureMessage: null,
        }).state,
      ).toBe(state);
    }

    const unsupported = decodeInstallation({
      supported: false,
      platform: "macos",
      architecture: "aarch64",
      assetName: null,
      ownership: "app-managed",
      unsupportedMessage: "Cloudflare publishes cloudflared for macOS only as an archive.",
      state: "not-installed",
      installedPath: null,
      installedVersion: null,
      detectedVersion: null,
      installedAt: null,
      failureMessage: null,
      release: null,
      releaseFailureMessage: "Cloudflare publishes cloudflared for macOS only as an archive.",
    });
    expect(unsupported.supported).toBe(false);
    expect(unsupported.unsupportedMessage).toContain("archive");
  });
});

describe("CloudflareAccountSnapshot", () => {
  const decodeAccount = Schema.decodeUnknownSync(CloudflareAccountSnapshot);
  const warning =
    "The Cloudflare account certificate can create, list, route, and delete every tunnel in your account, and stays valid for years. laplus uses it where cloudflared put it and never copies, moves, replaces, or deletes it.";
  const nothingYet = {
    certificateDetected: false,
    certificatePath: "/home/dev/.cloudflared/cert.pem",
    certificateConsentedAt: null,
    certificateWarning: warning,
    loginState: "not-started",
    authorizationUrl: null,
    failureMessage: null,
    tunnels: [],
    listedAt: null,
    selection: null,
    step: "sign-in",
    unfinishedCreation: null,
  } as const;
  const active = {
    id: "11111111-1111-1111-1111-111111111111",
    name: "already-running",
    createdAt: "2026-01-01T00:00:00Z",
    connectionCount: 2,
    activity: "active",
    classification: "external",
  } as const;
  const inactive = {
    id: "22222222-2222-2222-2222-222222222222",
    name: "spare",
    createdAt: "2026-02-02T00:00:00Z",
    connectionCount: 0,
    activity: "inactive",
    classification: "adoptable",
  } as const;

  it("decodes every step an interrupted setup can resume at", () => {
    expect(decodeAccount(nothingYet).step).toBe("sign-in");

    const detected = {
      ...nothingYet,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
      unfinishedCreation: null,
    } as const;
    expect(decodeAccount(detected).step).toBe("consent");
    // A certificate that is merely present has consented to nothing.
    expect(decodeAccount(detected).certificateConsentedAt).toBeNull();

    const consented = {
      ...detected,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      step: "choose-tunnel",
      unfinishedCreation: null,
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
    } as const;
    expect(decodeAccount(consented).step).toBe("choose-tunnel");
    expect(decodeAccount(consented).tunnels).toHaveLength(2);

    expect(
      decodeAccount({
        ...consented,
        step: "verify-hostname",
        unfinishedCreation: null,
        selection: {
          tunnelId: active.id,
          name: active.name,
          classification: "external",
          httpsOrigin: "https://laplus.example.com",
          adoptionConfirmed: false,
          created: false,
        },
      }).step,
    ).toBe("verify-hostname");

    const adoptable = decodeAccount({
      ...consented,
      step: "confirm-adoption",
      unfinishedCreation: null,
      selection: {
        tunnelId: inactive.id,
        name: inactive.name,
        classification: "adoptable",
        httpsOrigin: "https://laplus.example.com",
        adoptionConfirmed: false,
        created: false,
      },
    });
    expect(adoptable.step).toBe("confirm-adoption");
    // ADR-0045: choosing an inactive tunnel is a candidate, not a dedication.
    expect(adoptable.selection?.adoptionConfirmed).toBe(false);

    // And dedication confirmed is a *different* step, so a reopened dialog and
    // a restarted server agree that the offer has been answered rather than
    // presenting it again over a connector laplus is already supervising.
    const adopting = decodeAccount({
      ...consented,
      step: "adopting",
      unfinishedCreation: null,
      selection: {
        tunnelId: inactive.id,
        name: inactive.name,
        classification: "adoptable",
        httpsOrigin: "https://laplus.example.com",
        adoptionConfirmed: true,
        created: false,
      },
    });
    expect(adopting.step).toBe("adopting");
    expect(adopting.selection?.adoptionConfirmed).toBe(true);
    // Still `adoptable`: the listing's word for what may be done with a tunnel
    // never became a claim about who owns its Cloudflare allocation. That is
    // `tunnelOwnership` on the endpoint, and adoption makes it `adopted`.
    expect(adopting.selection?.classification).toBe("adoptable");
    expect(adopting.selection).not.toHaveProperty("ownership");
    // Adopted is not created: the two ways a tunnel becomes dedicated are never
    // both true, and only the second authorizes a Cloudflare deletion.
    expect(adopting.selection?.created).toBe(false);

    // The creation twin, which is its own step for exactly that reason.
    const creating = decodeAccount({
      ...consented,
      step: "creating",
      unfinishedCreation: null,
      selection: {
        tunnelId: "44444444-4444-4444-4444-444444444444",
        name: "laplus-workstation",
        classification: "adoptable",
        httpsOrigin: "https://stable.example.com",
        adoptionConfirmed: false,
        created: true,
      },
    });
    expect(creating.step).toBe("creating");
    expect(creating.selection?.created).toBe(true);
    expect(creating.selection?.adoptionConfirmed).toBe(false);

    expect(() => decodeAccount({ ...consented, step: "adopted" })).toThrow();
  });

  it("decodes every browser-authorization state, including the ones that end it", () => {
    for (const loginState of [
      "not-started",
      "awaiting-browser",
      "complete",
      "cancelled",
      "timed-out",
      "failed",
    ] as const) {
      expect(
        decodeAccount({
          ...nothingYet,
          loginState,
          authorizationUrl:
            loginState === "awaiting-browser"
              ? "https://dash.cloudflare.com/argotunnel?callback=test"
              : null,
          failureMessage:
            loginState === "timed-out"
              ? "Cloudflare authorization timed out. Start it again when you are ready."
              : null,
        }).loginState,
      ).toBe(loginState);
    }
  });

  it("carries the certificate warning and its location, and never the certificate", () => {
    const snapshot = decodeAccount({
      ...nothingYet,
      certificateDetected: true,
      loginState: "complete",
      step: "consent",
      unfinishedCreation: null,
      certificate: "FAKE-ACCOUNT-CERTIFICATE-SECRET",
    });

    expect(snapshot.certificateWarning).toContain("create, list, route, and delete every tunnel");
    expect(snapshot.certificateWarning).toContain("never copies, moves, replaces, or deletes it");
    // A path, so consent can name the file it is consent to use — and nothing
    // that would let a client read it. See the schema's own comment.
    expect(snapshot.certificatePath).toBe("/home/dev/.cloudflared/cert.pem");
    expect(snapshot).not.toHaveProperty("certificate");
  });

  it("reads activity from connections and never invents a hostname or ownership", () => {
    const snapshot = decodeAccount({
      ...nothingYet,
      certificateDetected: true,
      certificateConsentedAt: "2026-08-03T09:00:00.000Z",
      loginState: "complete",
      step: "choose-tunnel",
      unfinishedCreation: null,
      tunnels: [active, inactive],
      listedAt: "2026-08-03T09:00:01.000Z",
    });

    expect(snapshot.tunnels[0]?.activity).toBe("active");
    expect(snapshot.tunnels[0]?.classification).toBe("external");
    expect(snapshot.tunnels[1]?.activity).toBe("inactive");
    expect(snapshot.tunnels[1]?.classification).toBe("adoptable");
    for (const tunnel of snapshot.tunnels) {
      expect(tunnel).not.toHaveProperty("hostname");
      expect(tunnel).not.toHaveProperty("managementMode");
    }
  });
});
