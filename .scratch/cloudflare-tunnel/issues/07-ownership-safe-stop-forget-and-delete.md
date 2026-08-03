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

## Comments

**2026-08-03 — the foundation this now sits on already exists.** A Cloudflare
cleanup pass built the parts 05, 06 and 07 all needed, so none of the below is
yours to build.

- **Your acceptance matrix has a type.** `crate::public_exposure::TunnelOwnership`
  is `external | adopted | laplus-created`, persisted on the
  `public_exposure_endpoint` row (`server/docs/adr/0049`). "Delete everywhere is
  shown only for a laplus-created tunnel" is
  `TunnelOwnership::deletable_at_cloudflare()`, and it is a **server-side
  refusal** rather than a hidden button: `not-laplus-created` is already a
  declared reason in `packages/contracts/src/environmentHttp.ts`, and
  `ownership_is_not_the_clients_to_change` in `server.rs` is the precedent for
  refusing a request that tries to launder ownership. Wire your delete route to
  the recorded ownership, never to anything the client sent.
- **The exact resources are recorded.** The row carries tunnel id, DNS zone id,
  record id and record name, credential path and configuration path, so a
  destructive confirmation can name what it will remove and the deletion can
  target that record and no other. `DnsRecord` in `store.rs`; all three DNS
  columns are read as a unit, because a record laplus can name but not address
  is not one it could delete.
- **`cloudflared` has no `route dns delete`.** Deleting the record is a
  Cloudflare API call needing DNS authority of its own, and the fixture models
  it as a real local HTTP server rather than a CLI verb —
  `harness::cloudflare::FakeCloudflareApi`, with `FakeRelease` as the precedent
  for wiring one through an environment variable. It answers `404` with
  Cloudflare's `81044` for a record already gone, which is what an idempotent
  retry after partial cleanup has to read as already-done.
  `http_cloudflare_account.rs::deleting_a_dns_record_is_an_api_call_the_cli_cannot_make`
  asserts the CLI has no such verb, so the fixture cannot quietly grow one.
- **The journal is there.** `begin_mutation_step` / `settle_mutation_step` /
  `mutation_journal` / `clear_mutation_journal`, with
  `MutationIntent::{DeleteEverywhere, Forget}` and
  `MutationStep::{DnsRecordDelete, TunnelDelete, ConfigurationRemove,
CredentialRemove}` spelled. It is deliberately **not** keyed to the endpoint
  row: a half-done deletion outlives the endpoint it was deleting, and that
  residue is your `cleanup-required` state. `clear_mutation_journal` is scoped
  to one intent so a finished creation does not erase it.
- **Refusals are tagged and carry both lists.** `Refused::precondition` /
  `Refused::rejected` answer 409/400 with a closed `reason`
  (`not-laplus-created` and `cleanup-required` are declared), a sentence, and
  `completed`/`remaining`. `Refused::after(&completed, &remaining)` is unused so
  far and marked `#[allow(dead_code)]` — that is how "preserves exact remaining
  work for idempotent retry" reaches the client, and `cloudflareRefusalSummary`
  in `ConnectionsSettings.logic.ts` renders it without claiming a rollback that
  did not occur.
- **The row has the vocabulary.** `cloudflareOwnershipLabel` is an exhaustive
  `Record<TunnelOwnership, string>`; the `stopped`/`cleanup-required`/
  `partially-deleted`/`fully-removed` _states_ are yours to add, and adding one
  to `SetupStep` is a compile error in `step()` and a type error in
  `WIZARD_STEP_LABELS`.
- **Graceful shutdown is real now.** `asked_to_stop` in `server.rs` handles
  `SIGTERM` as well as `SIGINT`; before this, `systemctl stop` killed the server
  outright and left the connector — in its own process group — serving the
  public hostname. `tests/process_shutdown.rs` holds the proof, and
  `FakeCloudflared::stopped_gracefully()` is the assertion.
- **One fake cloudflared, in `tests/harness/cloudflare.rs`**, with
  `rehearse("delete-fails")` for your partial remote cleanup.
  `client_with(server, scopes)` and `VerifiedEndpoint` live there too — do not
  write a fifth copy.
