# 02 — Connector-token setup and supervision

**What to build:** Let an administrative developer use a compatible existing `cloudflared` executable and a tunnel-specific connector token to run a laplus-managed connector, observe readiness independently from public endpoint verification, and retain the connector across restarts without surrendering Cloudflare control-plane ownership.

**Blocked by:** 01 — External tunnel endpoint registration and verification.

**Status:** ready-for-agent

- [ ] The wizard discovers compatible system executables, accepts a user-selected executable, reports detected incompatibility, and never overwrites or removes an executable laplus does not own.
- [ ] A connector token is accepted into a private token file and never placed in process arguments, contracts, logs, errors, or non-secret persistence.
- [ ] The configured hostname, loopback origin, executable selection, remote ownership, private secret reference, and desired running state survive restart.
- [ ] Laplus starts the connector with explicit private configuration, token-file, and loopback metrics settings and reports `/ready` independently from public endpoint verification.
- [ ] The compact row and wizard distinguish starting, locally ready, publicly verified, degraded, restart-exhausted, stopped, and recoverable failure states.
- [ ] Supervision tolerates child replacement, performs bounded restarts without wall-clock assertions, exposes redacted actionable logs, and requires explicit retry after exhausting its budget.
- [ ] A stable connector starts with its owning shell or headless server and shuts down gracefully with that owner; an externally managed connector is never started or stopped.
- [ ] Stop preserves the tunnel configuration and secret, while a later start restores the same connector and re-verifies the endpoint.
- [ ] Repeated commands and reconnects reconcile observed state rather than launching duplicate connectors.
- [ ] Running-server and UI-driver tests use a fake cloudflared process to prove restart, shutdown, readiness, persistence, verification, and secret-redaction boundaries.
