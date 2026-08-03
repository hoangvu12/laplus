# 02 — Connector-token setup and supervision

**What to build:** Let an administrative developer use a compatible existing `cloudflared` executable and a tunnel-specific connector token to run a laplus-managed connector, observe readiness independently from public endpoint verification, and retain the connector across restarts without surrendering Cloudflare control-plane ownership.

**Blocked by:** 01 — External tunnel endpoint registration and verification.

**Status:** ready-for-human

- [x] The wizard discovers compatible system executables, accepts a user-selected executable, reports detected incompatibility, and never overwrites or removes an executable laplus does not own.
- [x] A connector token is accepted into a private token file and never placed in process arguments, contracts, logs, errors, or non-secret persistence.
- [x] The configured hostname, loopback origin, executable selection, remote ownership, private secret reference, and desired running state survive restart.
- [x] Laplus starts the connector with explicit private configuration, token-file, and loopback metrics settings and reports `/ready` independently from public endpoint verification.
- [x] The compact row and wizard distinguish starting, locally ready, publicly verified, degraded, restart-exhausted, stopped, and recoverable failure states.
- [x] Supervision tolerates child replacement, performs bounded restarts without wall-clock assertions, exposes redacted actionable logs, and requires explicit retry after exhausting its budget.
- [x] A stable connector starts with its owning shell or headless server and shuts down gracefully with that owner; an externally managed connector is never started or stopped.
- [x] Stop preserves the tunnel configuration and secret, while a later start restores the same connector and re-verifies the endpoint.
- [x] Repeated commands and reconnects reconcile observed state rather than launching duplicate connectors.
- [ ] Running-server and UI-driver tests use a fake cloudflared process to prove restart, shutdown, readiness, persistence, verification, and secret-redaction boundaries. **The running-server half proves all six; the UI-driver half proves four.** `cloudflare-tunnel.mjs` now drives readiness, stop-and-restart, graceful shutdown and secret redaction against its stand-in `cloudflared`, reading each answer off the server. It does not drive **persistence** — each of its two worlds starts one server and never restarts it — and it cannot drive **verification** at all, for the reason tickets 05, 07 and 01 all recorded: a scratch world has no public DNS name and no inbound HTTPS path. It also never drives **this ticket's own connector-token panel**: both walkthroughs take the Cloudflare account path, and the driver's stand-in has no `--token-file` branch. That is the honest remainder, and it is a driver gap rather than a product one.

## Note — the executable picker landed with ticket 04

Checkbox 1's UI half was missing: the server discovered compatible executables
and reported each one's source, version and compatibility, and
`ConnectionsSettings.tsx` rendered a bare text field that showed none of it.
`CloudflaredExecutablePicker` now renders the discovered list as a selection,
with a hand-typed path joining it rather than replacing it. Landed in ticket
04's commit because the account path needs to name which cloudflared it signs in
with.

## Comments

**2026-08-03 — closed out.** The code landed in `ab0849c` and the boxes were
never ticked. An audit found six partial; four of those were already true and
merely unrecorded, and two needed real work — one of which turned out to be a
defect rather than a missing test.

**Already true, and only unrecorded.** Boxes 2, 4 and 9 were complete when they
landed. The token is written to a private 0600 file inside a 0700 directory after
every refusal that could reject it, passed as `--token-file` and never as a
value, absent from the snapshot's declared shape, and scanned for in the argv
trace and the database bytes. The connector is launched with laplus's own
`--config`, never `~/.cloudflared/config.yml`, and a freshly bound
`127.0.0.1:0` metrics address whose `/ready` is a bounded probe. Readiness and
public verification are separate keys reached at separate times, which is what
stops "connected" overstating what works. Repeated starts leave the launch count
unchanged, and a reconfigure replaces rather than stacks.

**What needed work.**

- **`connector_state` was the last bare `String` in the Cloudflare code**, and
  the exact shape `closed_vocabulary!` exists to abolish — twelve literal writes
  and three string comparisons in one file, with the contract pinning the same
  eight words and nothing making the two agree. It is
  `ConnectorState` now, with `settled()` and `awaiting_retry()` replacing the two
  open-coded `matches!` blocks that a cleanup and a parked supervision loop
  depend on, and a test that fails if a variant is added here and not in
  `remoteAccess.ts`. This is the leftover the closeout brief flagged; it was a
  clean win and was taken.
- **Seven of the eight compact-row states had no test.** Only "Restart
  exhausted" was asserted, and every row test in `ConnectionsSettings.logic.test.ts`
  stubs `managedStateLabel`, so the real function was unreached. All eight are
  driven through the real DOM now, including the `ready` split — "Locally ready"
  versus "Publicly verified" is two of the seven states this box names, and it is
  decided by the endpoint's verification rather than by the connector's own word.
  A test pins that a ready connector whose endpoint failed is never called
  publicly verified.
- **The picker's metadata was untested.** Selection was driven; what the list
  _reports_ was not, and the existing radio was matched on a path substring that
  would still pass with the summary line deleted. Source, version, compatibility
  and the incompatibility sentence — with its detected version, which is what
  makes the failure actionable — are asserted now.
- **Box 3's fields were proven by implication.** A restored connector reaching
  `ready` implies the executable path survived; it does not assert it. Each field
  the box names is now read back by name after a real restart.
- **Re-verification after an explicit start was unwitnessed.** A stopped
  connector's endpoint row still reads `verified` — verification is a fact about
  the last attempt and a stop is not an attempt — so a start that verified
  nothing looked identical. A counting verifier makes the second check the
  assertion. Confirmed to fail when the spawn in `mutate_cloudflare_connector`
  is removed.

**The defect: log redaction was a single point of failure, and the point failed
at the worst moment.** `record_log` opened the run-credential file _at the moment
a log line arrived_ and matched against what it found. A file it could not read
yielded no secrets and redacted nothing — and the arrival of a log line is not an
arbitrary moment. A connector's stderr is drained when its child exits, and
Forget stops the connector and _then_ removes its credential (ADR-0052), so the
one moment `cloudflared` is most likely to be complaining about its token is the
one moment laplus could no longer recognise it. Secrets are now learned while the
file is certainly readable — at boot, at configure, and before each launch —
remembered for the life of the process and merged rather than replaced, and the
snapshot redacts `logs` and `failureMessage` again on the way out. Two points,
one rule, **ADR-0053**. Both halves are pinned by tests confirmed to fail against
the old behaviour, and the UI-driver now reads a redacted log line back off the
server after stopping a connector whose stand-in printed its own `TunnelSecret`.

**Why the client was not given a masking affordance.** The audit noted that
`RedactedSensitiveText` exists and `ConnectionsSettings.tsx` does not import it,
and that `logs` is a plain string array in the contract. Both were left as they
are, deliberately, and ADR-0053 records why: blurring a multi-line log blurs the
diagnosis along with the secret, and a client-side mask is not a boundary at all
— a value the browser can un-blur is a value that already crossed the wire. The
answer is that the server sends nothing to reveal.

**Box 7, and exactly what is proven.** The headless half is proven end to end
with the _real_ `laplus-server` binary, a pre-written `connector.json` at
`desiredState: running`, a real `SIGTERM`, and a fake that can only write its
`stopped` line from its own signal handler — a hard kill cannot produce it
(`tests/process_shutdown.rs`). The external half is proven twice over: an
external endpoint never acquires a connector lifecycle, and a supervised
connector cannot be re-claimed as external. **The shell half is by construction
rather than by test**: `laplus-shell/src/main.rs` calls the same
`server.shutdown()` on both of its exit paths, and that function is what the
headless test exercises — but `crates/laplus-shell/` has no `tests/` directory,
is not a default workspace member, and needs a display, so nothing here runs it.
The box says "shell **or** headless server" and the headless owner is proven; a
reader who wants the shell path covered needs a Tauri test harness that does not
exist in this repository, which is a larger piece of work than this closeout.

**What is left, and it is the last box.** The UI-driver does not drive the
connector-token panel — the surface this ticket is named for. Both of its
walkthroughs take the account path, and its stand-in `cloudflared` reads
`--config` only, with no `--token-file` branch. Adding that walkthrough is
self-contained: teach the stand-in the flag, and drive
`ManagedCloudflareConnectorPanel` on a third scratch world. It was left undone
rather than half-done, because a driver that pressed the buttons without a
stand-in that could answer them would be the fourth hollow claim in this feature.
