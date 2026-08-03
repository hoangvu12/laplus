# 04 — Cloudflare sign-in and existing-tunnel discovery

**What to build:** Let an administrative developer authorize Cloudflare in the browser, knowingly use a cloudflared-owned account certificate, discover existing tunnels, and choose the correct next path based on whether a tunnel is active or inactive.

**Blocked by:** 02 — Connector-token setup and supervision.

**Status:** done

- [x] The modal wizard launches and tracks cloudflared browser authorization without requiring terminal input, and cancellation, timeout, process failure, or restart leaves setup resumable.
- [x] Before using a detected account certificate, the wizard explains its broad, long-lived account authority and requires explicit consent.
- [x] Laplus uses an account certificate only in place for the requested account-management action and never copies, replaces, moves, deletes, or exposes it.
- [x] Sign-in, certificate use, listing, and refresh require `access:write`; Cloudflare state remains hidden from sessions without `access:read`.
- [x] Authenticated discovery parses structured tunnel identifiers, names, timestamps, and connection state without inferring a hostname or management mode the output does not provide.
- [x] The wizard asks for and verifies the public hostname rather than inventing it from tunnel metadata.
- [x] An active existing tunnel is classified as an external tunnel endpoint and can follow the external verification/pairing path without any laplus lifecycle or configuration action.
- [x] An inactive existing tunnel is offered for explicit dedication/adoption but is not treated as laplus-managed until that later confirmation succeeds.
- [x] Repeated login/list operations and interrupted discovery reconcile current state without duplicating Cloudflare mutations.
- [x] Fake cloudflared integration and UI coverage prove consent, structured parsing, active/inactive branching, restart recovery, refusal behavior, and certificate secrecy.

## What was built

**Server** — `crates/laplus-server/src/cloudflare_account.rs` and six routes under
`/api/access/cloudflare/account`. Browser sign-in is spawned, tracked and
cancellable; the certificate is used where cloudflared put it and never moved;
`tunnel list --output json` is parsed for what it proves and nothing more; the
step an interrupted setup resumes at is computed from what is durable rather
than remembered. `tests/http_cloudflare_account.rs`, five tests against a fake
cloudflared.

**Contract** — `CloudflareAccountSnapshot` and its parts in
`packages/contracts/src/remoteAccess.ts`, and eight endpoints in
`environmentHttp.ts`: the six account routes and the two `/challenge` routes
ticket 01 landed without declaring. `.scratch/contract-parity/ledger.md` gained
a third headline row and a Gap 4 for them, because a contract can be behind its
server as easily as ahead of it and only one direction had been counted.

**UI** — the flat three-panel dialog is now a step machine. The pure derivation
is `cloudflareWizardState` in `ConnectionsSettings.logic.ts`, driven by the
server's own snapshots; the component renders one step at a time and holds no
progress of its own. New screens: path choice, sign-in, certificate consent,
choose-tunnel, verify-hostname, dedication offer. The compact row names the step
an interrupted setup stopped at.

## Boundaries worth knowing

- **The dedication offer has no confirm control, on purpose.** Confirming
  dedication retrieves a narrow run credential and writes an isolated connector
  configuration, which is ticket 05's first checkbox. This ticket's screen
  presents the tunnel, the hostname, the observed inactivity and the ownership
  consequences, and states plainly that laplus manages nothing yet —
  `adoptionConfirmed` stays `false`, which is what the checkbox above means by
  "not treated as laplus-managed". Ticket 05 adds the button and the loopback
  target beside it.
- **`certificatePath` crosses the wire; the certificate never does.** The
  schema carries the reasoning: consent has to name the file it is consent to
  use, and reading the snapshot already requires `access:read`.
- **The timeout branch of checkbox 1 is reasoned, not tested.** `LOGIN_TIMEOUT`
  is ten minutes with no injection point, and this repo does not assert on
  elapsed wall-clock time. Cancellation, process failure and restart are each
  driven end to end; the timeout reaches resumability through the same
  `finish(state, message)` the other two use, and its state is decoded by the
  contract test. Giving it a seam is worth doing when something else needs one.
- **Account refusals are still undeclared in the contract.** `409` and `400`
  carry an untagged `{ "message": … }` body, the Cloudflare convention since
  ticket 01, which does not decode as a tagged error. Declared error sets say
  only what is true. Fixing it is eleven handlers at once — see Gap 4 in the
  parity ledger.

## Defects fixed here, from ticket 01's UI

- **Ownership conflation.** The external Register/Update button was outside the
  branch it belonged to and shared one hostname field with the managed panel, so
  a configured managed connector could have its own hostname registered as an
  _external_ endpoint — one lifecycle with two owners, which ADR-0045 forbids.
  The two panels now have independent hostname fields and registration belongs
  to one step — **and the route says no too.** Hiding a control stops a person;
  `managed_connector_already_owns_exposure` in `server.rs` stops a stale tab, a
  script or a second window from overwriting the endpoint record the connector
  restores itself from at boot. It refuses nothing tickets 05 and 06 need: a
  tunnel is chosen before its connector is configured.
- **Unreachable pairing QR.** The QR block was nested under
  `!managed?.configured` while its button was not, so pairing from a managed
  connector minted a credential nothing rendered. The QR is now outside every
  step, because a pairing link belongs to whichever endpoint was verified.
- **Stale scope doc.** `server.rs`, `pairing.rs` and `store.rs` all claimed
  nothing in this server gates on a scope. Twelve handlers and ADR-0047 say
  otherwise. `store.rs` was a third copy the brief had not spotted; its
  fail-closed default is now stated as the reason it is still the right default.
- **A refused administrator read the transport's summary.** A `403` surfaced as
  "Primary environment request failed during list-cloudflare-tunnels (HTTP
  403)". ADR-0047 says a denied client learns only that administrator access is
  required, so that is now what it says.

## Also landed

- **Ticket 02's executable picker.** Discovery already reported each
  cloudflared's path, source, version and compatibility, and the UI was a bare
  text field. The discovered list is now selectable, and a hand-typed path joins
  it rather than replacing it.
- **Real component tests.** All eleven Cloudflare UI tests before this were
  `renderToStaticMarkup` plus `toContain` and not one fired a handler.
  `ConnectionsSettings.cloudflareWizard.test.tsx` drives the component through
  the real HTTP client against the contract's own handlers, and asserts on the
  request it made and the screen the answer moved it to. This needed `happy-dom`
  and `@testing-library/react` as `apps/web` dev dependencies; there was no DOM
  test environment in this repository before.
