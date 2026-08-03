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

## Comments

**2026-08-03 — the foundation this now sits on already exists.** A Cloudflare
cleanup pass built the parts 05, 06 and 07 all needed, so none of the below is
yours to build.

- **Ownership is persisted.** `crate::public_exposure::TunnelOwnership` is
  `external | adopted | laplus-created`, on the `public_exposure_endpoint` row
  along with tunnel id, DNS zone/record/name, credential path and configuration
  path — which is exactly the "exact tunnel and DNS resources" your acceptance
  boxes ask to be recorded. `NewPublicExposure` in `store.rs` writes one;
  `server/docs/adr/0049` is the decision. Creation writes `LaplusCreated`, and
  it is the only ownership `deletable_at_cloudflare()` allows, which is what
  makes box 9 of your list and ticket 07's delete path work.
  `tests/http_public_exposure.rs::tunnel_ownership_survives_a_restart_and_is_not_the_clients_to_change`
  already proves it survives restart; box 9 needs the row's _wording_, and
  `cloudflareOwnershipLabel` in `ConnectionsSettings.logic.ts` supplies it
  ("laplus-created").
- **The journal is there and is the whole of your resumability box.**
  `begin_mutation_step` / `settle_mutation_step` / `mutation_journal` /
  `clear_mutation_journal`, with `MutationIntent::Create` and
  `MutationStep::{Credential, TunnelCreate, DnsRoute, Configuration}` spelled.
  Journal before the mutation and settle after with the resource it _actually_
  made — `tunnel create` is asked for a name and allocates a UUID, and cleanup
  targets the UUID. `Pending` after a restart is the remaining work.
- **Refusals are tagged and carry both lists.** `Refused::rejected(reason, …)`
  in `server.rs`, plus `Refused::after(&completed, &remaining)`, which is
  unused so far and marked `#[allow(dead_code)]` — you are its first caller.
  That is how "never claims an automatic rollback that did not occur" is said
  to the client; `cloudflareRefusalSummary` renders it.
- **The connector no longer records ownership.** `Configuration` in
  `cloudflare_connector.rs` carries what it needs to run; ownership is read from
  the endpoint row by `managed_connector_snapshot`. One record of one fact.
  Note `configure()` currently only knows the connector-token shape
  (`--token-file`); a created tunnel runs with `--credentials-file` and a
  laplus-owned ingress YAML, which is a change to `supervise()`'s argv.
- **The fake cloudflared answers every command you need.** One copy, in
  `tests/harness/cloudflare.rs`: `tunnel create --credentials-file --output
json` (writes a private `0o600` credential and reports
  `CREATED_TUNNEL_ID`, deliberately different from the name), `tunnel route
dns`, `tunnel delete`. `rehearse("create-fails")` and `rehearse("route-fails")`
  are your resumable boundaries.
  `http_cloudflare_account.rs::the_fixture_answers_every_command_a_dedicated_tunnel_needs`
  pins the argument shapes. `client_with(server, scopes)` and `VerifiedEndpoint`
  are there too — do not write a fifth copy.
- **The steps are enums.** Adding `creating` to `SetupStep` in
  `cloudflare_account.rs` is a compile error in `step()` and a type error in
  `WIZARD_STEP_LABELS` in `ConnectionsSettings.logic.ts`. `ACCOUNT_STEPS` there
  is still a flat four-element array and will need to fork per path (create vs
  adopt); that was left for whichever of 05 and 06 lands first.
