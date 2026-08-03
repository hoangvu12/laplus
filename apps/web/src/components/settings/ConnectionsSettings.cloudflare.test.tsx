import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import {
  CloudflaredInstallationPanel,
  CloudflareLayeredHealth,
  CloudflareTunnelSettingsRow,
  ManagedCloudflareConnectorPanel,
  managedCloudflareCompactState,
  offersCloudflaredInstallation,
} from "./ConnectionsSettings";

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

describe("managed Cloudflare connector", () => {
  const snapshot = {
    configured: true,
    ownership: "laplus",
    remoteOwnership: "cloudflare",
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

    expect(html).toContain("Connector ready");
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
