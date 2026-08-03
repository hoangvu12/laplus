# 01 — External tunnel endpoint registration and verification

**What to build:** Let an administrative developer register an externally managed Cloudflare HTTPS hostname, verify that it reaches this laplus environment over authenticated HTTP and WebSocket paths, and pair another device from the verified endpoint through the compact Connections row and modal wizard.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] The Cloudflare Tunnel compact row and modal wizard allow an administrative session to register a normalized HTTPS hostname as an external tunnel endpoint and clearly state that its connector remains operator-owned.
- [x] Reading Cloudflare setup and verification state requires `access:read`; registration, Test now, forget, and every other mutation require `access:write`; refused clients learn only the required administrative scope.
- [x] Registration and incomplete verification state survive a server restart and reopen at the truthful wizard step.
- [x] Verification is restricted to the configured hostname, disables redirects, rejects private or otherwise disallowed destinations, and cannot be used as an arbitrary URL probe.
- [x] Verification separately proves the public environment identity, a one-time authenticated HTTP challenge, and an authenticated WebSocket upgrade without transmitting a durable administrator credential.
- [x] DNS, TLS, wrong-environment, authentication, WebSocket, and Cloudflare Access interception failures are distinguishable; the last successful verification and stale state are retained.
- [x] Background verification uses bounded backoff and jitter, while Test now requests an immediate bounded check without creating concurrent probe storms.
- [x] Only a verified endpoint is advertised as available; its HTTPS/WSS endpoint, external ownership, and layered health appear in Connections.
- [x] The verified endpoint can mint and present a pairing link and QR code using existing endpoint language, without granting the paired client Cloudflare administration scopes.
- [ ] Focused contract, running-server, UI logic/component, and UI-driver coverage proves the path and asserts that diagnostic and pairing credentials never appear in snapshots, logs, errors, or non-secret persistence. **All but the UI-driver half of "proves the path".** The leak assertions are complete — `http_public_exposure.rs` scans the diagnostic tokens and now the pairing credential across snapshots, refusals, laplus's own Cloudflare directory and the database bytes. Contract, running-server and UI logic/component coverage all exist. The driver now registers an external hostname through the real wizard and reads the ownership, the normalization, the derived `wss://` origin and the absence of any connector back off the server — but it cannot verify or pair, for the reason tickets 05 and 07 recorded: both need a hostname that genuinely resolves in public DNS and a public HTTPS path back to this machine, and the whole point of the scratch world the driver builds is that there is neither. Faking either would make the driver assert against itself. Both are covered against the hermetic verifier in `http_public_exposure.rs`.

## Comments

**2026-08-03 — closed out.** The code landed across `494fb5d`, `13cbb54`,
`021f4e2` and `840fb01` and the boxes were never ticked. An audit found five
partial and one unmet; what follows is what was actually missing, because most of
it had been filled in by tickets 04–07 and the ownership refactor and did not
need building twice.

**Already true, and only unrecorded.** Boxes 2, 4, 5 and 6 were complete when
they landed. Scope refusal discloses only the required scope and is answered
before any public-exposure refusal (`server.rs`'s `require_scope`,
`setup_state_requires_read_and_mutations_require_write_without_disclosing_state`).
Verification takes no caller-supplied URL at all — the origin is read from the
endpoint row and the route has no request body — with redirects disabled, every
resolved address checked against `public_address`, and a 64 KiB streaming cap.
The three proofs are separate and the two diagnostic tokens are distinct,
protocol-bound and single-use. All ten failure kinds are distinguishable and a
failure leaves the previous success in place.

**What was genuinely missing, and now exists.**

- **The external path's resume had never been driven.** `cloudflareWizardState`
  reopens on `external-endpoint` from `configured` alone, and the only test that
  reached that branch supplied an already-_verified_ snapshot — so the half of
  box 3 that says "and **incomplete** verification state" was untested. There is
  now a real DOM test that mounts a registered, unverified endpoint and asserts
  the dialog opens on the hostname it recorded, carries it in the field, offers
  Update rather than Register, and claims neither a verification nor a pairing
  it has not earned.
- **The operator-ownership sentence had no test.** "laplus will never start,
  stop, reconfigure, or delete its connector" is the whole of what box 1's second
  clause promises and it could have been deleted with a green suite. Asserted
  now, together with the two public-exposure warnings beside it.
- **The QR could be deleted with a green suite too.** `<QRCodeSvg>` was
  unlabelled, so it had no accessible name and no test could find it; all three
  existing pairing tests read the textarea only. It is titled now — which is what
  gives it `role="img"` — and asserted. The same test pairs from a _purely
  external_ endpoint, which is this ticket's actual subject and the one branch of
  `createPairing` that had never run: every previous pairing test supplied a
  laplus-managed connector. It also asserts the request asks for no scopes, so
  "without granting the paired client Cloudflare administration scopes" is a
  property of the payload rather than of the sentence above it.
- **`wssOrigin` reached the client and was rendered nowhere.** Box 8 says the
  HTTPS/WSS endpoint appears in Connections; it appeared in the snapshot.
  `CloudflareEndpointOrigins` now shows both, and the layered health beside it is
  asserted in a real DOM test rather than by the `renderToStaticMarkup` string
  check that was its only previous evidence.
- **The jitter was a term, not a decision.** Backoff had
  `next_background_delay` and a unit test; the jitter it is specified alongside
  was three inline lines in a spawned loop, and deleting `+ jitter` broke
  nothing. It is `public_exposure::background_jitter` now, taking the clock
  rather than reading it, with a test that pins both the bound and the spread —
  a jitter that always answered zero would satisfy a bound alone.
- **Nothing scanned for a pairing credential.** The diagnostic tokens had the
  byte-scan idiom and the credential the pairing button mints had no assertion
  anywhere.
  `a_pairing_credential_minted_for_a_verified_endpoint_stays_out_of_cloudflare_surfaces`
  covers all four Cloudflare read surfaces, the refusals two of them can answer
  with, and laplus's own private Cloudflare directory.

**One thing the checkbox asks for that is deliberately excluded.**
`auth_pairing_links` stores the pairing code in plaintext, on purpose and for a
reason `pairing.rs` records: Settings lists an active link so a developer can
copy it again from the machine that minted it, and a hash makes that impossible.
That table is the secret store rather than the "non-secret persistence" box 10
is about, and the test says so where it scopes the scan.

**One thing that could not be proven and was not faked.** The UI-driver cannot
verify or pair. It is not a gap in the driver — it is that a browser on a scratch
`PATH` has no public DNS name and no inbound HTTPS path, and pairing is only
offered for a verified endpoint. Ticket 05 recorded the same limit and ticket 07
recorded it again. What the driver _can_ reach it now does.

**A defect found on the way.** Nothing in this ticket, but worth knowing here:
an earlier draft of the pairing leak scan passed the bare credential as a
hostname and the route answered `200`. A twelve-character pairing code is a
perfectly good hostname, so the probe re-registered the endpoint instead of
being refused, and proved nothing. The test now refuses on a scheme and an
executable path, which is what makes it a test of whether a _refusal_ quotes its
input.
