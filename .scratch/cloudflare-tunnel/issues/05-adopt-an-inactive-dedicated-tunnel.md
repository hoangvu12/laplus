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

## Comments

**2026-08-03 — the foundation this now sits on already exists.** A Cloudflare
cleanup pass built the parts 05, 06 and 07 all needed, so none of the below is
yours to build.

- **Ownership is persisted.** `crate::public_exposure::TunnelOwnership` is
  `external | adopted | laplus-created`, on the `public_exposure_endpoint` row
  along with tunnel id, DNS zone/record/name, credential path and configuration
  path. `server/docs/adr/0049` is the decision; `NewPublicExposure` in `store.rs`
  is how you write one. Adoption writes `Adopted` — nothing else does.
  `TunnelOwnership::deletable_at_cloudflare()` is the guard, and
  `ownership_is_not_the_clients_to_change` in `server.rs` already refuses a
  client trying to re-register your adopted tunnel as somebody else's hostname.
- **The connector no longer records ownership.** `Configuration` in
  `cloudflare_connector.rs` carries what it needs to run and nothing else, and
  `managed_connector_snapshot` reads `tunnelOwnership` from the endpoint row.
  One record of one fact — do not add a second.
- **The journal is there.** `begin_mutation_step` / `settle_mutation_step` /
  `mutation_journal` / `clear_mutation_journal`, with `MutationIntent::Adopt`
  and `MutationStep::{Credential, Configuration}` already spelled. Journal
  before the mutation, settle after; `Pending` after a restart is the remaining
  work, which is what the acceptance box about interrupted adoption means.
- **Refusals are tagged.** `Refused::precondition` / `Refused::rejected` in
  `server.rs` answer 409/400 with a closed `reason`, a sentence, and
  `completed`/`remaining`. `Refused::after(&[…], &[…])` attaches the journal —
  it is unused so far and marked `#[allow(dead_code)]`; you are its first
  caller. **`tunnel-became-active` is already a declared reason** for your
  activation race (`packages/contracts/src/environmentHttp.ts`).
- **The steps are enums.** `Activity`, `Classification`, `LoginState` and
  `SetupStep` in `cloudflare_account.rs` are Rust enums built by the
  `closed_vocabulary!` macro. Adding `adopting` to `SetupStep` is a compile
  error in `step()` and a type error in `WIZARD_STEP_LABELS` in
  `ConnectionsSettings.logic.ts` — which is the point. `ACCOUNT_STEPS` there is
  still a flat four-element array and will need to fork per path; that was left
  for whichever of 05 and 06 lands first.
- **The fake cloudflared answers `tunnel create`, `route dns`, `delete` and
  credential retrieval.** One copy, in `tests/harness/cloudflare.rs`, with
  `rehearse("create-fails" | "route-fails" | "delete-fails" | …)` for the
  partial-failure boundaries. `client_with(server, scopes)` and
  `VerifiedEndpoint` are there too — do not write a fifth copy.
- The compact row has ownership vocabulary: `cloudflareOwnershipLabel` and
  `cloudflareRefusalSummary` in `ConnectionsSettings.logic.ts`.
