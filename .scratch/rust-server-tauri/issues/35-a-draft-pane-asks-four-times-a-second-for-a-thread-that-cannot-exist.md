# 35 — A draft pane asks four times a second for a thread that cannot exist

**What to build:** not yet decided — the fix is a client change to a file this
repository does not own, and that is the question this ticket is really about.

**Status:** needs-triage

**Found by:** ticket 31's close-out, carried forward unchanged by ticket 34,
which observed it again on a real boot. Deferred twice as out of scope, never
written down until now. Not a regression: the polling predates both.

## What happens

Open a laplus window with a project registered and no conversation started. The
composer mints a draft thread id, and from that moment until the first prompt is
sent the client asks this server for that thread four times a second — over
both transports — and is correctly refused each time.

```
GET /api/orchestration/threads/{draftId}   → 404      ×4/sec
orchestration.subscribeThread {threadId}   → refused  ×4/sec
```

Confirmed by hand during ticket 34: the shell route answers 200 for the same
boot, and the thread route answers 404 for exactly the id the console names. An
empty-registry boot has no storm, because no pane is open.

### The loop

`packages/client-runtime/src/state/threads.ts`, all in one subscription:

1. `makeSubscribeInput` runs. `current.data` is `None` — nothing has ever been
   loaded for a draft — so it calls `snapshotLoader.load(prepared, threadId)`
   (line 270).
2. That is `GET /api/orchestration/threads/{id}`, which 404s.
   `threadSnapshotHttp.ts` treats a 404 as expected and returns `Option.none()`,
   so `current.data` stays `None`.
3. `canResume` is therefore `false` (line 278), so **no `afterSequence` is
   sent** — which is ticket 28 working exactly as designed.
4. With no cursor, this server refuses a subscription to a thread it does not
   hold. Correct, and ADR-0016 reaffirmed it.
5. `onExpectedFailure` records the error and `retryExpectedFailureAfter:
"250 millis"` (line 295) schedules a retry. That is the four-a-second.
6. Retry re-runs `makeSubscribeInput` from step 1, including the HTTP request.

Every step is behaving as specified. The storm is what those correct behaviours
do when composed against a thread id that is _guaranteed_ not to exist yet —
the client minted it itself and knows the server has never heard of it.

## Why this is awkward, and the actual question

The server cannot fix this. Answering a draft with anything other than 404 and
a refusal would undo tickets 28 and 34 — an empty snapshot is a claim the
client's own copy is wrong, which is the thing both tickets exist to prevent.
The loop has to be broken on the client, before it asks.

But `packages/client-runtime/src/state/threads.ts` is **upstream's file**. Its
last three commits are upstream PRs (#4163, #4006, #3719), and `docs/adr/0012`
is this project's standing decision not to take changes that turn a sync into a
fight. A patch in the middle of an actively-developed upstream file is exactly
that shape.

So the question this ticket needs answered before anyone writes code:

- **Is a draft distinguishable at all where the fix would go?** There is no
  `draft` concept anywhere in `packages/client-runtime/src/state/`. The client
  knows it minted the id, but that knowledge does not appear to reach
  `makeSubscribeInput`. If it has to be plumbed there, the diff against upstream
  gets larger, not smaller.
- **Is the cheaper fix the retry interval rather than the request?** 250 ms is
  aggressive for a failure that cannot resolve until the user does something.
  A backoff would cut the volume by orders of magnitude without needing to know
  what a draft is — a smaller and more defensible divergence, though it treats
  the symptom.
- **Or is it `apps/web`'s, not `client-runtime`'s?** If the pane simply does not
  mount a thread subscription until the thread exists, nothing upstream needs
  touching. `apps/web` is already a fork this repository owns outright
  (ticket 32, `docs/adr/0014`), which makes it the cheapest place to diverge.

That third option looks best from here and is deliberately not chosen — the
purpose of this ticket is that a maintainer picks, because the choice is about
how much upstream divergence this project is willing to carry, and that is not
a decision the evidence settles on its own.

## Why it is worth doing anyway

Nothing is user-visible: the shell renders, the composer works, the first prompt
creates the thread and the storm stops. It is worth fixing for two reasons that
are not about the user.

- **It is noise over the one signal this project has.** The console 404 storm is
  the first thing anyone sees when debugging a real boot, and it is now the
  thing every future ticket has to be told to ignore. Tickets 31 and 34 both
  spent a paragraph doing exactly that.
- **It is wasted work in the loop that matters.** Four HTTP requests and four
  refused subscriptions a second, per open draft pane, for as long as a pane sits
  idle. On loopback that is cheap; as a description of what the software does, it
  is not defensible.

## Acceptance

Deliberately not written. The acceptance criteria depend on which of the three
options above is chosen, and writing them now would pick one by implication.
Whoever triages this should settle that question, then add them.

What can be said regardless:

- A draft pane that has never had a thread makes no repeated request to either
  transport while it sits idle.
- Tickets 28 and 34 are untouched: a subscription to an absent thread with no
  cursor is still refused, and a caught-up cursor still opens with no snapshot.
- The first prompt still creates the thread and the pane still goes live.

## Comments

### 2026-07-28 — agent. Filed

Written up from ticket 34's close-out rather than from a fresh investigation,
plus a read of the client loop to pin the mechanism. The `250 millis` on line
295 is where the four-a-second comes from, which neither previous ticket had
identified — both described the symptom.

Not triaged deliberately. The three options are not equivalent in cost and the
difference between them is a divergence policy question, which ADR-0012 says is
the maintainer's.
