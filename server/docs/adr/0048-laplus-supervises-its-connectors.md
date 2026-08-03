# ADR-0048 — Laplus supervises its connectors

Date: 2026-08-02
Status: Accepted

A laplus-managed connector is a child of the shell or `laplus-server`, including
when that server itself runs under systemd. Persisted stable configuration makes
the connector start with its owner and after service reboot; laplus owns
readiness, logs, bounded restart and graceful shutdown. This trades the extra
availability of an independently installed `cloudflared` service for one
cross-platform lifecycle that the laplus UI can actually control, without
elevation or two competing owners. A connector already managed by systemd,
Docker or another operator remains external and is only verified by laplus.
