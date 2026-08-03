# 06 — Create a stable dedicated tunnel

**What to build:** Let an administrative developer create a new stable tunnel and DNS route for this laplus environment, recover safely from partial setup, and run, verify, advertise, and pair through its laplus-managed connector.

**Blocked by:** 04 — Cloudflare sign-in and existing-tunnel discovery.

**Status:** done

- [x] The wizard clearly labels the tunnel as locally managed on this computer and previews its name, exact HTTPS hostname, DNS change, loopback target, credential location, and public-exposure warning before confirmation.
- [x] Creation validates the hostname and intended dedicated ownership, then journals intent and the exact tunnel and DNS resources before and after every Cloudflare mutation. **The DNS resource is its name.** `cloudflared tunnel route dns` reports no zone or record id and the account certificate's contents are never read (ADR-0045), so the name is the only identifier creation can truthfully record; ADR-0051 is the decision and ticket 07 resolves the rest.
- [x] Laplus creates the tunnel with an explicit private credential path, creates the exact DNS route, and writes an isolated laplus-owned ingress configuration without modifying cloudflared defaults.
- [x] The account certificate is used only in place for these explicit mutations and is never copied, replaced, moved, deleted, or retained by laplus.
- [x] The narrow tunnel credential and local configuration are sufficient for steady-state operation after account-management authorization is no longer being used.
- [x] Repeating or resuming creation after timeout, disconnect, or restart reconciles the recorded tunnel, DNS, credential, and configuration state and does not duplicate resources.
- [x] Partial failure identifies completed and pending work, offers safe retry or explicit cleanup, and never claims an automatic rollback that did not occur. **Safe retry**, which is the half of the "or" this ticket owns; explicit cleanup is ticket 07's Forget and Delete everywhere.
- [x] The completed connector follows the existing supervision, readiness, public verification, advertisement, pairing, stop, and restart behavior.
- [x] The compact row identifies the endpoint as a laplus-created tunnel and preserves that ownership across restart.
- [x] Fake Cloudflare/cloudflared integration and UI-driver coverage prove success at each resumable boundary, mutation idempotence, public warnings, verification, pairing, and secret redaction. **Creation's own coverage was always complete; the fixture is no longer unused.** Every mutation creation performs is a `cloudflared` verb, so `FakeCloudflareApi` could never appear in _this_ ticket's path — it models the one operation the CLI cannot do. Ticket 07 is its first caller, and it now exercises the record this ticket created, through both `http_cloudflare_cleanup.rs` and a stand-in DNS API in the driver. What this box asked to be proven is proven: the three resumable boundaries and mutation idempotence in `http_cloudflare_creation.rs` and again through the browser, the public warnings in the component test and eight driver verdicts, verification and pairing against the hermetic verifier, and secret redaction across four haystacks.

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

## What was built

**Server** — `POST /api/access/cloudflare/account/create`, taking an executable,
a tunnel name and a hostname. It validates both answers before anything runs —
each with its own reason, because "cloudflared said no" does not say which box to
fix — refuses to take over an exposure some other owner already has, then
journals and performs three steps: `cloudflared tunnel --origincert … create
--credentials-file <private> --output json <name>`, `tunnel route dns <uuid>
<hostname>`, and laplus's own `--config` file. The endpoint row becomes
`laplus-created` with the allocated UUID, the DNS record's name and both paths;
the wizard's step becomes `creating`. `server.rs`, `cloudflare_account.rs`,
`cloudflare_connector.rs`, `store.rs`; `tests/http_cloudflare_creation.rs`, three
tests.

**Three steps, and no `Credential` among them.** `tunnel create
--credentials-file` allocates the tunnel _and_ writes the narrow credential in
one call, so a fourth journal entry would name a boundary a creation can never be
interrupted at. What each step is skipped on differs, and that is the whole of
the resumability: the allocation is read off the credential file, which names the
tunnel Cloudflare made; the configuration is read off the connector's own
settings; and the DNS route — which leaves nothing on this machine — is read off
the endpoint row, and failing that this intent's journal. ADR-0051.

**Nothing rolls back.** There is no `tunnel delete` in the creation path. A
refused route leaves a real tunnel and says so, naming the completed and the
outstanding work, because a rollback that can itself fail describes the world
worse than the failure it was tidying — and removing a Cloudflare resource is
07's separately confirmed operation.

**Contract** — `createCloudflareTunnel`, `CreateCloudflareTunnelInput`, the
`creating` setup step, `created` on the tunnel selection, `credentialPath` on the
connector snapshot, and `tunnel-name-invalid` in the refusal vocabulary.

**UI** — `create-tunnel` is a _client_ step, because a name and a hostname that
have only been typed are not durable state; `creating` is the server's, and is
what is true once the mutations happened. `CloudflareCreationOffer` asks for both
and previews everything the confirmation is a confirmation of, derived by
`cloudflareCreationPreview` so that what may be confirmed and what is shown are
one decision. `ACCOUNT_STEPS` gained a third fork. The `creating` step reuses
`CloudflareDedicatedConnectorPanel`, which already branches on
`deletableAtCloudflare` — so the sentence about deletion is the server's verdict
rather than a layout choice.

**UI-driver** — `cloudflare-tunnel.mjs` now walks both paths, on two isolated
servers, in one browser: adoption on `<port>` and creation on `<port>+1`, because
each ends with a connector laplus supervises and the wizard rightly will not
offer setup for an exposure that already exists. 48 verdicts. The creation half
drives a **partial creation** — the stand-in refuses `route dns`, the screen has
to name the work already done, and the retry must allocate no second tunnel —
which is this ticket's heart driven through the real wizard rather than asserted
against a route. Verified to exit 1 by making the route record `Adopted`: four
verdicts fail and the screen is identical.

## Defects found here

- **`DnsRecord` could not express what creation actually learns.** All three
  columns were required, and `cloudflared tunnel route dns` reports none of the
  identifiers — so a laplus-created tunnel would have had to record no DNS record
  at all, losing the fact that laplus made one. The name is now what makes a
  record and `addressable()` is how a caller asks whether it can be reached
  through the API. ADR-0051.

## For ticket 07

- **The DNS record on the row is a name, not an address.** `dns_record.zone_id`
  and `record_id` are `Option`, and creation leaves both `None`. Delete
  everywhere must resolve the name to a zone and a record with DNS authority of
  its own before deleting, and should write the identifiers back onto the row —
  `DnsRecord::addressable()` is the question to ask. `FakeCloudflareApi` is
  untouched and is still exactly the fixture for that call.
- **`CREATION_STEPS` is the list a cleanup has to undo, in reverse.** Three
  steps, and the journal is cleared for `MutationIntent::Create` on success — so
  a residual `create` journal is a creation that never finished, which is
  distinct from the `delete-everywhere` residue 07's `cleanup-required` reports.
- **`remaining_steps` is shared; the step lists are not.** Ticket 05 said an
  abstraction over two intents would have to be undone; subtracting a completed
  list from a constant one turned out to be the same sentence for both, and the
  constants stayed separate. A third intent should reuse the arithmetic and bring
  its own list.
- **Creation refuses a second tunnel while one is recorded**, on the endpoint row
  _and_ on the connector — the connector is written first, so it is the record
  that survives a crash the row did not. Forget is what should release that, and
  today's local-only Forget removes the row and leaves the connector running:
  the same gap ticket 05 recorded, now reachable from one more path.
- **`tunnel_name` is public in `cloudflare_account.rs`** and is the only
  validation of a Cloudflare tunnel name in the tree. A rename, if 07 ever grows
  one, should use it rather than a second copy.
- **A resume finishes the tunnel that exists, not the name most recently typed.**
  If a creation is retried with a different name after the allocation succeeded,
  the credential on disk wins and the earlier tunnel is completed. That is the
  safe direction — the alternative strands a real Cloudflare resource — and the
  way to a different tunnel is 07's delete or forget.

## Closed out by ticket 07 (2026-08-03)

Every handover this ticket wrote for 07 was taken up as written.

- **The DNS record's identifiers are resolved and written back.**
  `crate::cloudflare_dns` finds the zone the recorded name sits under from the
  zones its token can see, then the record within it, and
  `Database::address_public_exposure_dns_record` puts both on the row — so a
  deletion interrupted between the lookup and the removal addresses the record it
  already found. `DnsRecord::address()` is what asks.
- **`CREATION_STEPS` is not what a cleanup undoes.** `DELETION_STEPS` is written
  out separately, in the order a deletion actually takes: the DNS record before
  the tunnel, because the reverse leaves a CNAME pointing at nothing, and the two
  local steps last, because they are what a retry needs in order to reach
  Cloudflare again. `remaining_steps` is shared; the lists are not, exactly as
  this ticket said.
- **A residual `create` journal is still distinct from a cleanup residue.** They
  are separate intents and `cleanup_report` reads only the cleanup ones; a
  successful forget or deletion clears the `create` and `adopt` journals, because
  the setup they described is gone.
- **Creation's refusal while another dedicated tunnel's credential is on disk is
  no longer a dead end.** Forget removes that credential, so the way to a
  different tunnel is a forget or a deletion — which is what this ticket said it
  should be. The test that pinned the laundering guard now reaches the
  credential-on-disk-with-no-row state through a _partial adoption_ rather than
  through a forget, because a forget no longer leaves one.
- **`normalize_tunnel_name` was not copied**; ticket 07 grew no rename.
