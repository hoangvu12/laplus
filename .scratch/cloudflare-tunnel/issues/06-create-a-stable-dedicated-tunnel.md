# 06 — Create a stable dedicated tunnel

**What to build:** Let an administrative developer create a new stable tunnel and DNS route for this laplus environment, recover safely from partial setup, and run, verify, advertise, and pair through its laplus-managed connector.

**Blocked by:** 04 — Cloudflare sign-in and existing-tunnel discovery.

**Status:** ready-for-agent

- [ ] The wizard clearly labels the tunnel as locally managed on this computer and previews its name, exact HTTPS hostname, DNS change, loopback target, credential location, and public-exposure warning before confirmation.
- [ ] Creation validates the hostname and intended dedicated ownership, then journals intent and the exact tunnel and DNS resources before and after every Cloudflare mutation.
- [ ] Laplus creates the tunnel with an explicit private credential path, creates the exact DNS route, and writes an isolated laplus-owned ingress configuration without modifying cloudflared defaults.
- [ ] The account certificate is used only in place for these explicit mutations and is never copied, replaced, moved, deleted, or retained by laplus.
- [ ] The narrow tunnel credential and local configuration are sufficient for steady-state operation after account-management authorization is no longer being used.
- [ ] Repeating or resuming creation after timeout, disconnect, or restart reconciles the recorded tunnel, DNS, credential, and configuration state and does not duplicate resources.
- [ ] Partial failure identifies completed and pending work, offers safe retry or explicit cleanup, and never claims an automatic rollback that did not occur.
- [ ] The completed connector follows the existing supervision, readiness, public verification, advertisement, pairing, stop, and restart behavior.
- [ ] The compact row identifies the endpoint as a laplus-created tunnel and preserves that ownership across restart.
- [ ] Fake Cloudflare/cloudflared integration and UI-driver coverage prove success at each resumable boundary, mutation idempotence, public warnings, verification, pairing, and secret redaction.
