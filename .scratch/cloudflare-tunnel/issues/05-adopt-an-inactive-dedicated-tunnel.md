# 05 — Adopt an inactive dedicated tunnel

**What to build:** Let an administrative developer explicitly dedicate an inactive existing tunnel to this laplus environment, configure and supervise its connector, and verify and advertise its hostname without claiming ownership of the Cloudflare tunnel allocation or DNS record.

**Blocked by:** 04 — Cloudflare sign-in and existing-tunnel discovery.

**Status:** ready-for-human

- [x] The wizard shows the selected tunnel identifier, supplied hostname, loopback target, observed inactivity, and ownership consequences before requiring explicit dedication confirmation.
- [x] Adoption rechecks that the tunnel is inactive immediately before mutation and falls back to external ownership if an active connector appears.
- [x] Laplus retrieves or creates only the narrow run credential needed for the selected tunnel and stores it privately without exposing it in arguments, contracts, logs, errors, or non-secret persistence.
- [x] Laplus writes only its isolated connector configuration and never edits the user's default cloudflared configuration or installs a system service.
- [x] The adopted tunnel is persisted as dedicated and laplus-managed locally, while its Cloudflare allocation and DNS route remain externally owned and ineligible for deletion.
- [x] The connector follows the existing supervision, readiness, restart, shutdown, public verification, advertisement, and pairing behavior.
- [x] An interrupted adoption journals completed actions and resumes by reconciling observed state instead of repeating credential or configuration mutations.
- [ ] Stop and forget remain available, but Delete everywhere is never offered for an adopted tunnel. **Two of three.** Stop is available and changes nothing at Cloudflare; Delete everywhere is never offered, and the server states that verdict (`deletableAtCloudflare`) rather than leaving a client to draw or not draw a control. Forget is _not_ offered for an adopted tunnel, for the same reason it has never been offered for a laplus-run connector-token one: the route removes the endpoint row and stops nothing, so it would leave a connector running against a hostname nothing records. The forget a supervised connector needs is ticket 07's.
- [ ] Running-server and UI-driver tests cover successful adoption, an activation race, partial failure/restart recovery, secret redaction, verification, pairing, stop, and ownership-safe forget. **All but ownership-safe forget, which does not exist to test.** The running-server tests cover every other item; the UI-driver covers adoption, the activation race and its recovery, redaction and stop. Verification and pairing are not reachable from the driver — they need a hostname that genuinely resolves — and are covered by `http_cloudflare_adoption.rs` against the hermetic verifier.

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

## What was built

**Server** — `POST /api/access/cloudflare/account/adopt`. It refuses a selection
that is not adoptable, answers an adoption already recorded with what it
recorded, re-reads the tunnel's activity immediately before mutating, then
journals and performs two steps: `cloudflared tunnel token --cred-file` into
laplus's private directory, and laplus's own `--config` file naming the tunnel,
the credential and the ingress. The endpoint row becomes `adopted` with the
tunnel id and both paths; the account's selection becomes confirmed and the
wizard's step becomes `adopting`. `server.rs`, `cloudflare_account.rs`,
`cloudflare_connector.rs`; `tests/http_cloudflare_adoption.rs`, four tests.

`RunCredential` in `cloudflare_connector.rs` is the new shape: a connector token
for a tunnel Cloudflare configures, a `<UUID>.json` tunnel credential for one
laplus configures. Untagged and flattened, so every settings file written before
adoption existed still reads — which is what a unit test now pins, because the
first version of it did not and a headless connector silently stopped restarting.

**Contract** — `adoptCloudflareTunnel`, the `adopting` setup step, and
`deletableAtCloudflare` on both the endpoint and connector snapshots.

**UI** — the dedication offer gained the loopback target and the confirm button;
`adopting` is a new step with its own panel, because a dedicated tunnel has no
hostname or connector token to offer. `ACCOUNT_STEPS` forked into a four-step
external path and a five-step adoption path, and `revisitingTunnelChoice` is
the way back to the tunnel list after an activation race — without it the
fallback this ticket requires left the developer on `verify-hostname` with no
route anywhere else.

**UI-driver** — `server/tools/ui-driver/cloudflare-tunnel.mjs`, the first
Cloudflare driver in this repository. It starts its own `laplus-server` against
a stand-in `cloudflared` on a scratch `PATH`, drives the wizard from the path
choice to the dedication, and asserts on `/api/access/cloudflare` and
`/api/access/cloudflare/connector`. Verified to exit 1 by making the route
record `External` instead of `Adopted`: the screen is identical and three
verdicts fail.

## Defects fixed here

- **`private_write` leaked its temporary on every failure**, and it is opened
  with `create_new` — so one failed write made every later write to the same
  path fail for a reason that had nothing to do with it. A retried adoption
  failed a second time at a step whose cause had already been removed.
- **A refused `configure` wrote the connector token first.** Hostname and
  executable validation now runs before the secret reaches disk.
- **The Cloudflare row was nested under `canManageNetworkAccess`**, which is a
  desktop bridge — so a browser pointed at a headless `laplus-server` could
  never reach Cloudflare setup at all. ADR-0047 gates it on a scope and ADR-0048
  makes a headless server a connector's owner; the row is now outside that
  branch.
- **A refused dedication left the client holding a stale account snapshot**, so
  after an activation race the screen went on offering to dedicate a tunnel the
  server had just disowned — with the refusal's sentence above a button that
  could only be refused again. Found by driving the wizard headlessly, which is
  the whole argument for the driver existing.
- **A failed credential retrieval left its wreckage.** cloudflared creates the
  file it is pointed at and can still exit non-zero, and the resume decides by
  looking — so a truncated `<UUID>.json` made the next attempt skip a retrieval
  it still needed. It is removed on failure now, and a credential counts only if
  it parses and names this tunnel.
- **`rename_all` on an untagged enum renames variants, not fields.** The first
  version of `RunCredential` silently wrote a settings file no build could read
  back, and the headless connector that then stopped restarting looked like a
  supervision bug.

## For tickets 06 and 07

- **`ADOPTION_STEPS` and `settle_adoption_step` in `server.rs` are the pattern**,
  not a shared helper: creation journals four steps and cleanup journals four
  different ones, and a premature abstraction over two of them would have to be
  undone. `Refusal::after(&completed, &remaining)` is what both must keep doing.
- **Do not re-run the activity recheck for a tunnel already recorded.** After a
  successful adoption the account listing shows laplus's _own_ connections, so a
  recheck would disown a tunnel this environment is correctly running. ADR-0050.
- **Forget is available and incomplete, deliberately.** It removes the endpoint
  row and nothing else, so forgetting an adopted tunnel today leaves the
  connector running and its credential and configuration on disk — and the next
  boot restores the hostname as `external`, because the connector's settings
  file says nothing about ownership. Losing a record is the safe direction, but
  it is a record laplus needs. `http_cloudflare_adoption.rs` pins the half that
  is true now (local only, nothing at Cloudflare); the rest is 07's, and that
  assertion is 07's to extend. A refusal was tried here and reverted: this
  ticket's acceptance requires forget to stay available.
- **`deletableAtCloudflare` is the deletion verdict on the wire.** Ticket 07's
  command must refuse on `TunnelOwnership::deletable_at_cloudflare`, the same
  value, so the offer and the refusal cannot disagree.
- **Delete everywhere has no route yet**, so the last checkbox is met by the
  offer never being made and by ownership being unchangeable through every route
  that writes the endpoint row — register, select, configure and forget all
  refuse. The refusal on a deletion _command_ is 07's to add.
