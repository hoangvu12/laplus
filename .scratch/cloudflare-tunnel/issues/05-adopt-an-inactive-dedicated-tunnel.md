# 05 — Adopt an inactive dedicated tunnel

**What to build:** Let an administrative developer explicitly dedicate an inactive existing tunnel to this laplus environment, configure and supervise its connector, and verify and advertise its hostname without claiming ownership of the Cloudflare tunnel allocation or DNS record.

**Blocked by:** 04 — Cloudflare sign-in and existing-tunnel discovery.

**Status:** ready-for-agent

- [ ] The wizard shows the selected tunnel identifier, supplied hostname, loopback target, observed inactivity, and ownership consequences before requiring explicit dedication confirmation.
- [ ] Adoption rechecks that the tunnel is inactive immediately before mutation and falls back to external ownership if an active connector appears.
- [ ] Laplus retrieves or creates only the narrow run credential needed for the selected tunnel and stores it privately without exposing it in arguments, contracts, logs, errors, or non-secret persistence.
- [ ] Laplus writes only its isolated connector configuration and never edits the user's default cloudflared configuration or installs a system service.
- [ ] The adopted tunnel is persisted as dedicated and laplus-managed locally, while its Cloudflare allocation and DNS route remain externally owned and ineligible for deletion.
- [ ] The connector follows the existing supervision, readiness, restart, shutdown, public verification, advertisement, and pairing behavior.
- [ ] An interrupted adoption journals completed actions and resumes by reconciling observed state instead of repeating credential or configuration mutations.
- [ ] Stop and forget remain available, but Delete everywhere is never offered for an adopted tunnel.
- [ ] Running-server and UI-driver tests cover successful adoption, an activation race, partial failure/restart recovery, secret redaction, verification, pairing, stop, and ownership-safe forget.
