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

**Status:** ready-for-agent

- [ ] An unrecognised item kind increments the drift counter and the session
      carries on.
- [ ] An unrecognised notification method increments the drift counter and the
      session carries on.
- [ ] A line that does not parse is counted separately from one that parsed into
      something unrecognised.
- [ ] A turn reports its own drift; the session reports its total.
- [ ] A hand-written capture covering degradation a healthy codex never emits is
      committed, with an expected fold, and is marked in the captures README as
      synthetic rather than recorded.
- [ ] Every committed Codex capture is folded through a fresh state against an
      expected JSON, so a protocol change shows up as a failing diff.
- [ ] The whole suite runs offline, for free, on a machine that has never had
      `codex` installed. CI never spawns the agent.
- [ ] The captures README states the re-recording rule: every capture does both
      jobs, so re-recording after a codex release is worth doing even when the
      golden files still match.
