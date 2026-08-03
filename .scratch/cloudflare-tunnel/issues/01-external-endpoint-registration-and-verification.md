# 01 — External tunnel endpoint registration and verification

**What to build:** Let an administrative developer register an externally managed Cloudflare HTTPS hostname, verify that it reaches this laplus environment over authenticated HTTP and WebSocket paths, and pair another device from the verified endpoint through the compact Connections row and modal wizard.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] The Cloudflare Tunnel compact row and modal wizard allow an administrative session to register a normalized HTTPS hostname as an external tunnel endpoint and clearly state that its connector remains operator-owned.
- [ ] Reading Cloudflare setup and verification state requires `access:read`; registration, Test now, forget, and every other mutation require `access:write`; refused clients learn only the required administrative scope.
- [ ] Registration and incomplete verification state survive a server restart and reopen at the truthful wizard step.
- [ ] Verification is restricted to the configured hostname, disables redirects, rejects private or otherwise disallowed destinations, and cannot be used as an arbitrary URL probe.
- [ ] Verification separately proves the public environment identity, a one-time authenticated HTTP challenge, and an authenticated WebSocket upgrade without transmitting a durable administrator credential.
- [ ] DNS, TLS, wrong-environment, authentication, WebSocket, and Cloudflare Access interception failures are distinguishable; the last successful verification and stale state are retained.
- [ ] Background verification uses bounded backoff and jitter, while Test now requests an immediate bounded check without creating concurrent probe storms.
- [ ] Only a verified endpoint is advertised as available; its HTTPS/WSS endpoint, external ownership, and layered health appear in Connections.
- [ ] The verified endpoint can mint and present a pairing link and QR code using existing endpoint language, without granting the paired client Cloudflare administration scopes.
- [ ] Focused contract, running-server, UI logic/component, and UI-driver coverage proves the path and asserts that diagnostic and pairing credentials never appear in snapshots, logs, errors, or non-secret persistence.
