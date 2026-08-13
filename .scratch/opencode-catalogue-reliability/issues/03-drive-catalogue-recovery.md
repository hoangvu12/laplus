# 03 — Drive remembered catalogue behavior in the application

**What to build:** Complete the feature by exercising its user-visible startup
and failure behavior in a rebuilt running Laplus application.

**Blocked by:** 02 — Fast and resilient local OpenCode discovery.

**Status:** done

- [x] Run focused Rust, contract, client, and UI verification for tickets 01–02.
- [x] Drive first launch with a remembered catalogue and observe the checking
      status before live discovery completes.
- [x] Drive successful removal, transient failure retention, changed-instance
      invalidation, and unavailable-model refusal.
- [x] Confirm no silent model substitution and no secret-bearing cache content.
- [x] Record the walkthrough under `## Comments` and stop all test processes.

## Comments

- On 2026-08-13, rebuilt `apps/web/dist` and `laplus-server`, then served that
  bundle from an isolated application profile against a deterministic external
  OpenCode peer. The first model-picker visit showed `Checking OpenCode…
Remembered models remain available.` together with `Remembered Model` before
  live discovery settled.
- An immediate successful inventory replaced the remembered row exactly with
  `Replacement Model`; the old row disappeared. A transport failure on the next
  launch published `catalogueState: stale` / `status: warning`, retained the
  replacement row, and gave the retry-provider-discovery warning. Changing the
  same instance's endpoint identity produced an authoritative error snapshot
  with no remembered models.
- Driving the real startup path found and fixed a scoped-thread panic caused by
  constructing Tokio's timeout before entering the external-discovery runtime.
  A no-ambient-runtime regression test now covers the application startup seam.
- Driving selection through a successful removal found and fixed client draft
  reconciliation silently replacing the removed OpenCode slug. The repeated
  flow dispatched `anthropic/replacement` after the inventory contained only
  `anthropic/remembered`, and the application rendered: `OpenCode model
'anthropic/replacement' is no longer available ... Laplus did not substitute
another one.`
- The generated `provider-catalogues.json` was 575 bytes and contained only the
  schema version, instance/driver identity, timestamps/version, and model
  capabilities. The configured `walkthrough-secret-sentinel` password was
  absent. All temporary servers, browsers, and provider processes were stopped
  after the drive.
