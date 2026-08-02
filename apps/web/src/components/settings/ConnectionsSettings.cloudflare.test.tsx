import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import {
  CloudflareLayeredHealth,
  CloudflareTunnelSettingsRow,
  ManagedCloudflareConnectorPanel,
  managedCloudflareCompactState,
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
