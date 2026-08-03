# ADR-0045 — Cloudflare setup is local, but existing tunnel ownership stays explicit

Date: 2026-08-02
Status: Accepted

Laplus creates a stable Cloudflare tunnel through the installed `cloudflared`
CLI, with Cloudflare's browser authorization as the only step outside the
laplus UI. This locally managed workflow is chosen over Cloudflare's recommended
dashboard-managed workflow because terminal-free setup is the feature's purpose:
laplus can create the tunnel and DNS route, keep an isolated configuration, and
run thereafter with the tunnel-specific credential. It never edits the user's
default Cloudflare configuration, and uses the account-wide certificate only
for an explicit account-management action.

An inactive existing tunnel can become a dedicated laplus tunnel after explicit
confirmation. An active tunnel remains externally managed: laplus may verify
and advertise a hostname already routed to this server, but cannot change its
ingress, start a differently configured replica, restart it or delete it. This
gives every lifecycle action one owner and avoids intermittently routing a
shared hostname to connectors with different ingress rules.
