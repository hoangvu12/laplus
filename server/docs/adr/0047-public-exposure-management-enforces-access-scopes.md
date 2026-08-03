# ADR-0047 — Public-exposure management enforces access scopes

Date: 2026-08-02
Status: Accepted

Cloudflare status requires `access:read`, and every operation that downloads an
executable, authenticates Cloudflare, changes a tunnel or DNS route, or starts
and stops exposure requires `access:write`. This is the first server control
surface that must enforce the scopes laplus previously only recorded: the
desktop and headless boot grants are administrative, while ordinary phone
pairing is not. Peer address cannot substitute for authorization because a
public tunnel's requests also arrive from a loopback `cloudflared` process; a
denied client learns only that administrator access is required and receives no
Cloudflare account or configuration state.
