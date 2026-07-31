# 11 — Unrecognised Codex protocol is counted, never fatal

**What to build:** A codex release never kills a developer's session, and a
maintainer learns about protocol drift from a failing golden diff rather than
from a bug report.

The Codex protocol types are hand-written for the subset a v1 uses — roughly six
of the eighteen item kinds, a dozen notifications, the four request/response
pairs and the approval requests. **Everything unrecognised degrades into the
existing drift counter** rather than failing a session. This ticket is where that
claim is proved rather than asserted.

Generated types were rejected for having the opposite failure mode: strict
decoding turns the next item kind OpenAI ships into a dead session, and the
protocol moved eighty releases in seven months. OpenAI's own crate is published
but frozen at a version seven months behind the CLI. The scale of what is _not_
handled is the argument: the notification schema declares around seventy methods
and a v1 handles a dozen.

**One capture is hand-written rather than recorded**, covering degradation a
healthy codex never emits — the twelve unhandled item kinds, unhandled
notification methods, and a line that is not what its envelope claims. There is
no way to record this from a working agent, which is exactly why it has to exist:
the failure it guards against only happens on a version nobody has yet.

The drift counter is subtractable and already reports two numbers, because they
are two failures with two fixes: an unrecognised event type is the agent having
grown something new, and an unparseable line is a line that is not JSON at all.

**Blocked by:** 06, 09.

**Status:** done

- [x] An unrecognised item kind increments the drift counter and the session
      carries on.
- [x] An unrecognised notification method increments the drift counter and the
      session carries on.
- [x] A line that does not parse is counted separately from one that parsed into
      something unrecognised.
- [x] A turn reports its own drift; the session reports its total.
- [x] A hand-written capture covering degradation a healthy codex never emits is
      committed, with an expected fold, and is marked in the captures README as
      synthetic rather than recorded.
- [x] Every committed Codex capture is folded through a fresh state against an
      expected JSON, so a protocol change shows up as a failing diff.
- [x] The whole suite runs offline, for free, on a machine that has never had
      `codex` installed. CI never spawns the agent.
- [x] The captures README states the re-recording rule: every capture does both
      jobs, so re-recording after a codex release is worth doing even when the
      golden files still match.

**Implemented.** `ConversationState` now classifies unknown item kinds,
notification methods, and parsed envelope/shape mismatches as unknown events;
only JSON parse failures increment the separate parse-error count. Codex uses
the existing subtractable `Drift`, so each completion summary reports only drift
not reported by an earlier turn while its payload carries the cumulative fold
totals. The scripted socket replay runs two turns through one app-server and
proves recognized output still arrives after the synthetic drift.

The protocol golden discovers every `fixtures/codex-app-server/*.jsonl` rather
than maintaining a filename list. `07-synthetic-drift` covers the twelve
unhandled v0.146.0 item kinds, unknown methods during startup and a turn, parsed
item and result shape mismatches, and an unparseable raw line. Notifications
observed while a startup request awaits its response are deferred into the
conversation fold rather than discarded. The provider capture now has both the
same fresh conversation fold as every other capture and its separate
probe-decoder snapshot.

Post-fix review tightened that startup boundary: malformed lines and envelopes
are retained while `initialize`, `thread/start`, or `thread/resume` waits for its
correlated response. Unknown server requests are both declined for progress and
retained for drift counting. A malformed correlated `turn/start` result is
classified with the method context only the transport has, and its later
completion can still settle the turn. Capture 06's socket stand-in now derives
its initialize response, warning, refusal, and outbound prefix from capture 06;
only the fresh fallback thread and completed turn are synthetic.

**Verification.** `cargo test --no-fail-fast -p laplus-server --test
protocol_golden` passes 7 tests, `cargo test --no-fail-fast -p laplus-server
--lib codex_protocol::tests` passes 11 tests, `cargo test --no-fail-fast -p
laplus-server --test socket_codex_turn` passes 26 tests, and
`cargo check -p laplus-server --tests` passes. The focused tests use only
repository fixture data and temporary stand-in scripts; none resolves or invokes
a Codex installation.
