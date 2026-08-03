# ADR-0046 — cloudflared can be an app-managed tool

Date: 2026-08-02
Status: Accepted

Cloudflare setup must not stop at a terminal prerequisite, so laplus may install
`cloudflared` as an app-managed tool after explicit user approval. It downloads
an identified official Cloudflare release, verifies its published checksum and
keeps the executable in laplus's data directory without modifying the system
`PATH` or requiring elevation. A compatible system or user-selected executable
is preferred and never overwritten or removed by laplus. Laplus has no
`cloudflared` update product: an app-managed copy retains Cloudflare's built-in
update behaviour, while a system installation remains the system's concern.
Connector supervision therefore follows readiness rather than assuming the
original child PID lives forever. This accepts responsibility for a narrow
initial executable supply chain in exchange for the feature's terminal-free
setup promise, without taking ownership of Cloudflare's release lifecycle.
