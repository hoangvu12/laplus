# 07 — Ownership-safe stop, forget, and delete

**What to build:** Give an administrative developer distinct, truthful controls to stop a connector, forget laplus-owned local setup, or explicitly delete the exact Cloudflare resources laplus created, including recoverable handling of partial remote cleanup.

**Blocked by:** 05 — Adopt an inactive dedicated tunnel; 06 — Create a stable dedicated tunnel.

**Status:** ready-for-human

- [x] Stop changes desired connector state and gracefully stops only a laplus-managed connector while preserving tunnel, DNS, credentials, configuration, ownership, and restartable setup.
- [x] Forget separately confirms and removes only laplus-owned local configuration and secrets after stopping its connector; system/user executables, account certificates, adopted tunnel allocations, external endpoints, and external connectors remain untouched.
- [x] Delete everywhere is shown only for a laplus-created tunnel and names the exact recorded tunnel and DNS resources in a separate destructive confirmation.
- [x] Remote deletion requires fresh `access:write` authorization and sufficient Cloudflare account/DNS authority; missing authority produces a recoverable state rather than weakening the operation.
- [x] Tunnel and DNS deletion are separate journaled steps, and a partial failure preserves exact remaining work for idempotent retry after restart.
- [x] Adopted tunnels and external tunnel endpoints can never reach a Cloudflare tunnel or DNS deletion command, including through repeated, stale, or forged client requests.
- [x] Cleanup never revokes a Cloudflare account token and never copies, replaces, moves, or deletes an account certificate.
- [x] The compact row and wizard report stopped, forgotten, cleanup-required, partially deleted, and fully removed states truthfully and do not advertise an endpoint after its usable local setup is removed.
- [x] Running-server tests cover every ownership/action matrix, restart at each cleanup boundary, refusal behavior, idempotence, exact-resource targeting, and secret redaction.
- [ ] The UI-driver closeout covers create or adopt, verify and pair, stop and restart, forget, and the separately confirmed laplus-created-only delete path without leaving development servers or fake connectors running. **All but verify and pair, which a driver cannot reach.** The run covers adoption, creation, a partial creation and its retry, stop and restart on both servers, forget, and the separately confirmed delete — 80 verdicts, exit 0, and it leaves no server, browser or fake connector behind. Verification needs a hostname that genuinely resolves in public DNS and a public HTTPS path back to this machine, and pairing is only offered for a verified endpoint; the whole point of the scratch world the driver builds is that there is neither, and faking either would make the driver assert against itself. Both are covered against the hermetic verifier in `http_cloudflare_adoption.rs` and `http_cloudflare_creation.rs`. Ticket 05 recorded the same limit for the same reason.

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

## What was built

**Server** — three verbs that are deliberately not each other.

_Stop_ was already `POST /api/access/cloudflare/connector/stop` and needed no
new route; what it needed was to be truthful about itself, which found a real
supervision defect (below).

_Forget_ — `POST /api/access/cloudflare/forget`, rewritten. It stops the
connector and waits for it, removes laplus's own ingress file, settings file and
run credentials, removes the endpoint row, and clears the account selection so
the wizard does not resume on a setup that no longer exists. Both removals are
journaled under `MutationIntent::Forget`, so a forget interrupted between them
reports `cleanup-required` with the exact outstanding step and finishes on a
repeat. It runs no Cloudflare command for any ownership, and removes no
executable — including the app-managed `cloudflared`, which lives in the same
private directory and is a _tool_ rather than this exposure's setup (ADR-0052
decides that deliberately rather than by which files happened to be named).

_Delete everywhere_ — two routes.
`POST /api/access/cloudflare/account/deletion` reads the endpoint row, refuses
anything but `laplus-created`, and mints a one-time confirmation naming the exact
tunnel id, DNS record and endpoint.
`POST /api/access/cloudflare/account/delete` spends it: stop the connector,
delete the DNS record through Cloudflare's DNS API, `cloudflared tunnel delete`
the tunnel, then do what forget does. Four journaled steps, each skipped when it
is already done.

`crate::cloudflare_dns` is the new module — the first thing in this repository to
call Cloudflare's REST API. It resolves the record laplus could only _name_
(ADR-0051) through the zone list its token can see, writes the resolved
identifiers back onto the row with `address_public_exposure_dns_record`, and
reads Cloudflare's `81044` as already-done. `LAPLUS_CLOUDFLARE_API` overrides its
origin towards loopback only, the way the release feed already does — the request
carries an API token in a header.

**The fresh-authorization decision is ADR-0052**, and it is what makes checkbox 6
hold against a replay rather than against a hidden button: the confirmation is
removed from memory when read, expires in five minutes, is checked against the
endpoint row as it stands, and does not survive a restart. Three refusals cover
the three ways a request can be wrong — `not-laplus-created`,
`confirmation-required`, `dns-authority-required` — and the last of those refuses
_before_ the first step is journaled, because a deletion that removed the tunnel
and left a dangling CNAME would be a weaker operation rather than a recoverable
state.

**The cleanup report is derived, not stored.** `CleanupState` is
`intact | stopped | cleanup-required | partially-deleted | forgotten |
fully-removed`, read from what is observably gone on disk _or_ settled in the
journal — the second source is what stops a finished forget from being reported
as outstanding forever once a new setup puts a credential back, and the first is
what lets a cleanup killed between removing a file and settling its entry finish
rather than repeat. A finished removal is reported only while no endpoint is
recorded; outstanding work is reported either way. `CleanupState::advertisable`
is what stops a verified endpoint being offered for pairing while its DNS record
is being deleted — verification is a fact about the _last_ attempt, and a row
still reading `verified` is exactly what a just-deleted record leaves behind.

**Contract** — `offerCloudflareDeletion`, `deleteCloudflareTunnel`,
`CloudflareDeletionPlan`, `DeleteCloudflareTunnelInput`,
`PublicExposureCleanupState`, `PublicExposureCleanupReport`, `cleanup` on the
endpoint snapshot, and two refusal reasons.

**UI** — `Forget local setup` and `Delete everywhere…` on the dedicated connector
panel, the second only when the server says `deletableAtCloudflare`;
`CloudflareDeletionConfirmation`, which renders the server's plan and asks for the
DNS token; and a `cleanup` wizard step that outranks every other screen while work
is outstanding, because a partially deleted tunnel still has a connector and an
endpoint row and the dedicated panel would describe it as healthy.

**UI-driver** — the same file, now 80 verdicts across two servers and a stand-in
Cloudflare DNS API, covering stop, restart, forget and the confirmed delete on
top of what 05 and 06 drove. Verified to exit 1 by hand.

## Defects found here

- **A connector could report `starting` for as long as it stayed stopped.** The
  supervision loop writes `starting` optimistically when it is told to replace a
  child, and the outer loop then parks without touching the word if the connector
  is no longer wanted — so a connector that was reconfigured and then stopped in
  quick succession sat at `starting` forever. The compact row said "Starting" for
  a connector with no child at all, and this ticket's cleanups waited twenty
  seconds for a word that could never arrive. Found by `stop_and_settle` timing
  out in `http_cloudflare_adoption.rs`; fixed where the loop parks.
- **`stop_and_settle` waited for one word where four mean the same thing.** A
  connector in `restart-exhausted` or `failed` has no child and already carries
  `desired_state: stopped`, so `set_desired` has nothing to change and the word
  never moves. A cleanup would have refused to remove the setup of a connector
  that had already died.
- **The DNS API's loopback override refused IPv6.** `Url::host_str` keeps an IPv6
  literal's brackets, and `[::1]` does not parse as an address — so on an
  IPv6-only machine the override would have silently declined and sent a request
  carrying a Cloudflare API token to the real API. `cloudflare_install.rs` has the
  same shape and is left alone; it is ticket 03's and nothing here needs it.

## Notes for whoever picks this up

- **`cleanup_completed` is the one answer**, shared by the snapshot, forget and
  delete. If a future cleanup step is added, it goes in `DELETION_STEPS` or
  `FORGET_STEPS` and in that function — the report a developer reads after a
  restart and the work a retry actually skips must not be able to disagree.
- **The confirmation map is in memory on purpose.** Persisting it would make an
  offer survive a restart, which is the one thing ADR-0052 says it must not do.
- **Nothing rolls back, here either.** A deletion that removed the DNS record and
  could not remove the tunnel says exactly that; it does not try to re-create the
  record. Same argument as ADR-0051's.
