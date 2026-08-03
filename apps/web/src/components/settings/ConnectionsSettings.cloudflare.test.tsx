import type {
  ExternalTunnelEndpointSnapshot,
  ExternalTunnelFailureKind,
  ManagedCloudflareConnectorSnapshot,
} from "@t3tools/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import {
  CloudflaredInstallationPanel,
  CloudflareConnectorStatus,
  CloudflareDedicatedConnectorPanel,
  CloudflareLayeredHealth,
  CloudflareTunnelSettingsRow,
  CloudflareVerificationFailure,
  ManagedCloudflareConnectorPanel,
  managedCloudflareCompactState,
  offersCloudflaredInstallation,
} from "./ConnectionsSettings";

/**
 * A setup nothing has removed anything from — what these fixtures are unless
 * they say otherwise. Ticket 07 made the cleanup report part of every endpoint
 * snapshot, because it is the one answer that survives the setup it describes.
 */
const INTACT_CLEANUP = {
  state: "intact",
  completed: [],
  remaining: [],
  tunnelId: null,
  dnsRecordName: null,
} as const;

describe("CloudflareTunnelSettingsRow", () => {
  it("presents external hostname registration as a compact Connections row", () => {
    const html = renderToStaticMarkup(<CloudflareTunnelSettingsRow canWrite />);

    expect(html).toContain("Cloudflare Tunnel");
    expect(html).toContain("Register an externally managed HTTPS hostname.");
    expect(html).toContain("Set up");
  });

  it("keeps the setup discoverable for an access:read-only administrator", () => {
    const html = renderToStaticMarkup(<CloudflareTunnelSettingsRow canWrite={false} />);

    expect(html).toContain("Cloudflare Tunnel");
    expect(html).toContain("Set up");
  });

  it("presents connector, HTTPS, and WebSocket health as separate layers", () => {
    const html = renderToStaticMarkup(
      <CloudflareLayeredHealth
        snapshot={{
          configured: true,
          httpsOrigin: "https://laplus.example.com",
          wssOrigin: "wss://laplus.example.com",
          ownership: "external",
          deletableAtCloudflare: false,
          cleanup: INTACT_CLEANUP,
          health: { connector: "external", https: "healthy", webSocket: "failed" },
          verificationState: "failed",
          failureKind: "websocket",
          failureMessage: "The WebSocket upgrade failed.",
          lastAttemptAt: "2026-08-02T12:00:00.000Z",
          lastVerifiedAt: "2026-08-02T11:00:00.000Z",
          advertisedEndpoint: null,
        }}
      />,
    );
    expect(html).toContain("Connector external");
    expect(html).toContain("HTTPS healthy");
    expect(html).toContain("WebSocket failed");
  });
});

/**
 * **Checkbox 6, on the screen rather than on the wire.** The contract has
 * declared ten typed failure kinds since the first slice and the server sets
 * every one of them; the client rendered `failureMessage` and nothing else, so
 * a developer was told a sentence about the probe and never which of ten
 * different things to go and fix.
 */
describe("a failed verification names the kind of failure", () => {
  const failureFor = (
    failureKind: ExternalTunnelFailureKind,
    failureMessage: string | null = null,
  ) =>
    renderToStaticMarkup(
      <CloudflareVerificationFailure failureKind={failureKind} failureMessage={failureMessage} />,
    );

  it("tells a DNS failure apart from a Cloudflare Access interception", () => {
    const dns = failureFor("dns", "DNS lookup failed.");
    const access = failureFor("cloudflare-access", "An access page intercepted the challenge.");

    // Both keep what the server observed…
    expect(dns).toContain("DNS lookup failed.");
    expect(access).toContain("An access page intercepted the challenge.");
    // …and each says which class of failure that observation is.
    expect(dns).toContain("DNS");
    expect(access).toContain("Cloudflare Access");
    expect(dns).not.toBe(access);
  });

  /**
   * The distinction the layered health already draws — HTTPS healthy, WebSocket
   * failed — said in words, because "exempt the WebSocket path too" is a
   * different Access policy edit from "exempt laplus".
   */
  it("separates an intercepted WebSocket upgrade from a plain one", () => {
    expect(failureFor("cloudflare-access-websocket")).toContain("WebSocket");
    expect(failureFor("cloudflare-access-websocket")).not.toBe(failureFor("websocket"));
  });

  /** A verification that did not fail draws nothing at all. */
  it("renders nothing when nothing failed", () => {
    expect(
      renderToStaticMarkup(
        <CloudflareVerificationFailure failureKind={null} failureMessage={null} />,
      ),
    ).toBe("");
  });
});

describe("managed Cloudflare connector", () => {
  const snapshot = {
    configured: true,
    ownership: "laplus",
    tunnelOwnership: "external",
    deletableAtCloudflare: false,
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
    logs: ["Connected to Cloudflare edge"],
    verificationState: "failed",
    failureKind: "websocket",
    publicFailureMessage: "WebSocket upgrade failed.",
    lastVerifiedAt: null,
  } as const;
  const executables = [
    {
      path: "/usr/bin/cloudflared",
      selected: true,
      source: "system",
      version: "2026.7.0",
      compatibility: "compatible",
      failureMessage: null,
    },
  ] as const;

  it("shows local readiness separately from public endpoint verification", () => {
    const html = renderToStaticMarkup(
      <ManagedCloudflareConnectorPanel
        snapshot={snapshot}
        executables={executables}
        canWrite
        busy={false}
        hostname="laplus.example.com"
        executablePath="/usr/bin/cloudflared"
        connectorToken=""
        onHostnameChange={() => {}}
        onExecutablePathChange={() => {}}
        onConnectorTokenChange={() => {}}
        onConfigure={() => {}}
        onStart={() => {}}
        onStop={() => {}}
        onRetry={() => {}}
      />,
    );

    // "Locally ready" rather than "ready": this connector reached the edge and
    // its public endpoint still failed verification, which is the distinction
    // the next line is about.
    expect(html).toContain("Connector: Locally ready");
    expect(html).toContain("Public endpoint failed");
    expect(html).toContain("WebSocket upgrade failed.");
  });

  it("offers explicit stop and hides retry until restart exhaustion", () => {
    const html = renderToStaticMarkup(
      <ManagedCloudflareConnectorPanel
        snapshot={snapshot}
        executables={executables}
        canWrite
        busy={false}
        hostname="laplus.example.com"
        executablePath="/usr/bin/cloudflared"
        connectorToken=""
        onHostnameChange={() => {}}
        onExecutablePathChange={() => {}}
        onConnectorTokenChange={() => {}}
        onConfigure={() => {}}
        onStart={() => {}}
        onStop={() => {}}
        onRetry={() => {}}
      />,
    );
    expect(html).toContain("Stop connector");
    expect(html).not.toContain("Retry connector");
    expect(
      managedCloudflareCompactState({
        ...snapshot,
        connectorState: "restart-exhausted",
        readiness: false,
      }),
    ).toBe("Restart exhausted");
  });

  /**
   * **The wizard says the same word the row does.** `readiness` is a tri-state,
   * so a connector that is starting, one that is degraded and one that has spent
   * its restart budget all read as "Connector not ready" — three situations that
   * want three different things from a developer. Ticket 02 asks the compact row
   * *and* the wizard to tell them apart, and one shared vocabulary is how they
   * cannot come to disagree.
   */
  it("names the connector state in the wizard, not just readiness", () => {
    const statusFor = (connectorState: ManagedCloudflareConnectorSnapshot["connectorState"]) =>
      renderToStaticMarkup(
        <CloudflareConnectorStatus snapshot={{ ...snapshot, connectorState, readiness: false }} />,
      );

    expect(statusFor("starting")).toContain("Starting");
    expect(statusFor("degraded")).toContain("Degraded");
    expect(statusFor("restart-exhausted")).toContain("Restart exhausted");
    // Three states that used to be one sentence are now three.
    expect(statusFor("starting")).not.toBe(statusFor("degraded"));
    expect(statusFor("degraded")).not.toBe(statusFor("restart-exhausted"));
  });

  /**
   * **A connector snapshot carries the same typed kind the endpoint row does**,
   * and this panel dropped it just as thoroughly: "WebSocket upgrade failed."
   * is what the probe saw, and the connector being locally ready while its
   * public socket is intercepted is what a developer has to act on.
   */
  it("says which class of failure kept the public endpoint unverified", () => {
    const html = renderToStaticMarkup(<CloudflareConnectorStatus snapshot={snapshot} />);

    expect(html).toContain("WebSocket upgrade failed.");
    expect(html).toContain("WebSocket");
    expect(html).not.toBe(
      renderToStaticMarkup(
        <CloudflareConnectorStatus snapshot={{ ...snapshot, failureKind: "dns" }} />,
      ),
    );
  });

  /**
   * **Checkbox 8, for the ownership it was never true of.** The dedicated panel
   * is the only screen a laplus-created or adopted tunnel ever lands on, and it
   * stated one origin — so the address the WebSocket will actually use appeared
   * nowhere for the two ownerships laplus itself set up.
   */
  it("states both advertised origins for a tunnel laplus supervises", () => {
    const endpoint: ExternalTunnelEndpointSnapshot = {
      configured: true,
      httpsOrigin: "https://laplus.example.com",
      wssOrigin: "wss://laplus.example.com",
      ownership: "laplus-created",
      deletableAtCloudflare: true,
      cleanup: INTACT_CLEANUP,
      health: { connector: "laplus", https: "healthy", webSocket: "healthy" },
      verificationState: "verified",
      failureKind: null,
      failureMessage: null,
      lastAttemptAt: "2026-08-03T09:00:00.000Z",
      lastVerifiedAt: "2026-08-03T09:00:00.000Z",
      advertisedEndpoint: null,
    };
    const html = renderToStaticMarkup(
      <CloudflareDedicatedConnectorPanel
        snapshot={{ ...snapshot, tunnelOwnership: "laplus-created", deletableAtCloudflare: true }}
        endpoint={endpoint}
        canWrite
        busy={false}
        onStart={() => {}}
        onStop={() => {}}
        onRetry={() => {}}
        onForget={() => {}}
        onDelete={() => {}}
      />,
    );

    expect(html).toContain("https://laplus.example.com");
    expect(html).toContain("wss://laplus.example.com");
  });
});

describe("app-managed cloudflared installation", () => {
  const available = {
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
  } as const;

  it("previews the exact release, platform, source, and ownership before download", () => {
    const html = renderToStaticMarkup(
      <CloudflaredInstallationPanel
        snapshot={available}
        canWrite
        busy={false}
        onInstall={() => {}}
      />,
    );

    expect(html).toContain("2026.7.3");
    expect(html).toContain("cloudflared-linux-amd64");
    expect(html).toContain("releases/download/2026.7.3/cloudflared-linux-amd64");
    expect(html).toContain("9d71c677db00134c1bd4144b7783486b654ad281b1ea62b4972098d19f770f17");
    expect(html).toContain("App-managed");
    expect(html).toContain("no PATH change");
    expect(html).toContain("Download and verify cloudflared 2026.7.3");
  });

  it("withholds the download action from an administrator who cannot write", () => {
    const html = renderToStaticMarkup(
      <CloudflaredInstallationPanel
        snapshot={available}
        canWrite={false}
        busy={false}
        onInstall={() => {}}
      />,
    );

    expect(html).toContain("2026.7.3");
    expect(html).not.toContain("Download and verify");
  });

  it("keeps a failed attempt retryable and says why it failed", () => {
    const html = renderToStaticMarkup(
      <CloudflaredInstallationPanel
        snapshot={{
          ...available,
          state: "failed",
          failureMessage:
            "The downloaded cloudflared did not match Cloudflare's published checksum, so it was discarded.",
        }}
        canWrite
        busy={false}
        onInstall={() => {}}
      />,
    );

    expect(html).toContain("published checksum");
    expect(html).toContain("Retry cloudflared 2026.7.3");
  });

  it("names the installed copy laplus owns", () => {
    const html = renderToStaticMarkup(
      <CloudflaredInstallationPanel
        snapshot={{
          ...available,
          state: "installed",
          installedPath: "/data/laplus/cloudflare/tools/cloudflared-2026.7.3",
          installedVersion: "2026.7.3",
          detectedVersion: "cloudflared version 2026.7.3",
          installedAt: "2026-08-02T10:00:00.000Z",
        }}
        canWrite
        busy={false}
        onInstall={() => {}}
      />,
    );

    expect(html).toContain("cloudflared 2026.7.3 installed");
    expect(html).toContain("/data/laplus/cloudflare/tools/cloudflared-2026.7.3");
    expect(html).toContain("replaces or removes only this copy");
    expect(html).not.toContain("Download and verify");
  });

  it("explains a platform laplus will not install on, and offers nothing", () => {
    const html = renderToStaticMarkup(
      <CloudflaredInstallationPanel
        snapshot={{
          ...available,
          supported: false,
          platform: "macos",
          architecture: "aarch64",
          assetName: null,
          unsupportedMessage:
            "Cloudflare publishes cloudflared for macOS only as an archive. Install it yourself — `brew install cloudflared` — and select the executable above.",
          release: null,
          releaseFailureMessage: "Cloudflare publishes cloudflared for macOS only as an archive.",
        }}
        canWrite
        busy={false}
        onInstall={() => {}}
      />,
    );

    expect(html).toContain("cannot install cloudflared here");
    expect(html).toContain("brew install cloudflared");
    expect(html).not.toContain("Download and verify");
  });

  it("offers installation only when nothing compatible is already usable", () => {
    const compatibleSystem = [
      {
        path: "/usr/bin/cloudflared",
        selected: false,
        source: "system",
        version: "2026.7.0",
        compatibility: "compatible",
        failureMessage: null,
      },
    ] as const;
    const incompatibleSystem = [
      {
        path: "/usr/bin/cloudflared",
        selected: false,
        source: "system",
        version: "2023.10.0",
        compatibility: "incompatible",
        failureMessage: "This cloudflared executable is incompatible.",
      },
    ] as const;

    const appManaged = [
      ...compatibleSystem,
      {
        path: "/data/laplus/cloudflare/tools/cloudflared-2026.7.3",
        selected: true,
        source: "app-managed",
        version: "2026.7.3",
        compatibility: "compatible",
        failureMessage: null,
      },
    ] as const;

    expect(offersCloudflaredInstallation(compatibleSystem)).toBe(false);
    expect(offersCloudflaredInstallation(incompatibleSystem)).toBe(true);
    expect(offersCloudflaredInstallation([])).toBe(true);
    // A copy laplus installed keeps the panel — that is where its ownership is
    // stated — even once a compatible system executable turns up beside it.
    expect(offersCloudflaredInstallation(appManaged)).toBe(true);
  });
});
