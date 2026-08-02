import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import { CloudflareLayeredHealth, CloudflareTunnelSettingsRow } from "./ConnectionsSettings";

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
