# 07 — Ownership-safe stop, forget, and delete

**What to build:** Give an administrative developer distinct, truthful controls to stop a connector, forget laplus-owned local setup, or explicitly delete the exact Cloudflare resources laplus created, including recoverable handling of partial remote cleanup.

**Blocked by:** 05 — Adopt an inactive dedicated tunnel; 06 — Create a stable dedicated tunnel.

**Status:** ready-for-agent

- [ ] Stop changes desired connector state and gracefully stops only a laplus-managed connector while preserving tunnel, DNS, credentials, configuration, ownership, and restartable setup.
- [ ] Forget separately confirms and removes only laplus-owned local configuration and secrets after stopping its connector; system/user executables, account certificates, adopted tunnel allocations, external endpoints, and external connectors remain untouched.
- [ ] Delete everywhere is shown only for a laplus-created tunnel and names the exact recorded tunnel and DNS resources in a separate destructive confirmation.
- [ ] Remote deletion requires fresh `access:write` authorization and sufficient Cloudflare account/DNS authority; missing authority produces a recoverable state rather than weakening the operation.
- [ ] Tunnel and DNS deletion are separate journaled steps, and a partial failure preserves exact remaining work for idempotent retry after restart.
- [ ] Adopted tunnels and external tunnel endpoints can never reach a Cloudflare tunnel or DNS deletion command, including through repeated, stale, or forged client requests.
- [ ] Cleanup never revokes a Cloudflare account token and never copies, replaces, moves, or deletes an account certificate.
- [ ] The compact row and wizard report stopped, forgotten, cleanup-required, partially deleted, and fully removed states truthfully and do not advertise an endpoint after its usable local setup is removed.
- [ ] Running-server tests cover every ownership/action matrix, restart at each cleanup boundary, refusal behavior, idempotence, exact-resource targeting, and secret redaction.
- [ ] The UI-driver closeout covers create or adopt, verify and pair, stop and restart, forget, and the separately confirmed laplus-created-only delete path without leaving development servers or fake connectors running.
