# Cloudflare Tunnel prototype verdict

Status: ready-for-agent

Date: 2026-08-02

## Question

Which Cloudflare Tunnel setup and management experience feels native inside
laplus Settings → Connections?

## Verdict

**Variant A — compact Settings row plus modal wizard.**

The compact row matches the density and scanning behaviour of the existing
Connections page. The modal gives installation, account authorization, tunnel
ownership, public-exposure warnings, verification, pairing, and destructive
removal choices enough focused space without turning routine Settings browsing
into a permanently expanded setup flow.

## Pieces retained from the other variants

- From B, retain an explicit resumable-progress summary in the compact row and
  show the current step inside the wizard.
- From C, treat the connected tunnel as an advertised endpoint: show layered
  connector/HTTPS/WebSocket health, provide pairing from that endpoint, and
  make ownership visible beside status.
- Keep C's distinction between a laplus-managed connector and a verified
  externally managed endpoint.

## Rejected structures

- B made a rare, multi-step setup dominate the Connections page even when the
  developer only wanted to inspect current connectivity.
- C was strongest after connection but made first-time setup less discoverable
  and gave Cloudflare disproportionate weight beside ordinary endpoint rows.

## Prototype source

The full throwaway prototype is captured on branch
`prototype/cloudflare-tunnel-settings` at its tip commit. It must be treated as
a primary source, not promoted directly into production.

The next session should use this verdict plus ADRs 0045–0048 to produce the
design/spec. Production implementation should rewrite the chosen experience
with real contracts, errors, tests, and access-scope enforcement.
