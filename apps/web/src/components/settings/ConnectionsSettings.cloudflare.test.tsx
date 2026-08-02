import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vite-plus/test";

import { CloudflareTunnelSettingsRow } from "./ConnectionsSettings";

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
});
