# 34 — The snapshot the client already has is sent to it again

**What to build:** a subscription that answers a cursor it cannot replay from by
saying nothing, when there is nothing to say.

**Status:** done

**Found by:** ticket 31's own close-out, finding #3, and carried forward in the
handoff it wrote. Not a regression — the two halves were each harmless before
they met.

## What happens

Holding an HTTP snapshot is what makes the client send a cursor.
`packages/client-runtime/src/state/shell.ts` returns
`{ afterSequence: httpSnapshot.value.snapshotSequence }` on a successful load and
nothing at all otherwise; `state/threads.ts` has the same shape via `canResume`.
So ticket 31 is what put cursors on this wire, and this server ignored them:

```
→ Request  orchestration.subscribeShell  {"afterSequence":0}
← Chunk    {"kind":"snapshot","snapshot":{"projects":[],"snapshotSequence":0,…}}
```

The client's own comment gives the motive the routes exist for — the response
"keeps the (potentially multi-KB) snapshot off the socket" — and after ticket 31
it did the opposite, putting a second copy on it. Nothing is user-visible: the
shell renders, conversations render. This is bytes and honesty.

Two documents overstated it and are corrected here. `crate::orchestration`'s
divergence read "`afterSequence` is honoured by over-answering", when
`Shell::subscribe` did not read the field at all; `CONTEXT.md`'s **Resume** entry
said the value was ignored.

## What "fixed" means

Not a replay log. `apps/server/src/ws.ts` replays with
`orchestrationEngine.readEvents`, which is `eventStore.readFromSequence` — the
reference server is event-sourced, its snapshots are projections of a log it
keeps anyway. This server's source of truth is rows and memory, and its events
are announcements nothing retains.

What it can do is the degenerate case: a cursor that is still the newest number
handed out is a replay of _no events_, and that is answered exactly, by opening
with no snapshot. Upstream takes the same branch — with `replayGap === 0` it
sends the marker alone, which is what `fixtures/socket-wire/01-browser-session.ndjson`
captures at requests `3` and `4`.

Every other cursor keeps the answer this server already gave, and the comparison
is **equality** rather than "at least". A cursor ahead of this server's number is
not an early client but one holding a number from a previous run — the counter
resumes from the last durable write, so a run reissues everything the run before
it did not write down. Upstream guards the same case with `replayGap < 0` and
calls it invalid.

`docs/adr/0016` is the whole argument.

## Acceptance

- A subscription whose cursor equals the current sequence opens with no
  snapshot, on both `subscribeShell` and `subscribeThread`.
- A cursor behind it, and a cursor ahead of it, are both answered with the whole
  snapshot.
- A subscriber that opened caught up and then fell a backlog behind is
  re-described with a snapshot.
- Ticket 28 is intact: a subscription to a thread this server does not have is
  still refused unless a cursor is present.
- The two routes' payloads are unchanged, and the HTTP/socket agreement tests
  still pass.

## Comments

### 2026-07-28 — agent. Done

`Sequences::caught_up` is the rule and both subscriptions call it; there is no
new state on a subscription and no change to `packages/client-runtime`.

Three things worth recording.

**The rule is re-read rather than remembered, and that is load-bearing.**
`EventSource::describe` runs again whenever a subscriber falls a whole backlog
behind, and that second description has to be a snapshot. Nothing tracks which
call it is, because every event on every feed carries a number taken from
`Sequences` — so falling behind is itself what makes a cursor stale.
`a_subscription_that_opened_caught_up_is_re_described_once_it_has_not` pins it,
and is what would fail if an event were ever published without taking a number.

**A caught-up subscription can legitimately send nothing at all.** laplus
advertises neither `shellResumeCompletionMarker` nor `threadResumeCompletionMarker`,
so the real client sends a bare cursor and is owed no marker — and the pump sends
no chunk for an empty description. The client is already written for it:
`shell.ts` goes live off the HTTP snapshot, `threads.ts` sets itself live on
`!supportsCompletionMarker && canResume`. The wire test asserts that silence
directly rather than inferring it from what arrives next.

**`apps/server` is readable after all.** It is not in the working tree under this
repo's `sparse-checkout`, which reads as absent — but it is in the object store,
so `git show HEAD:apps/server/src/ws.ts` works and the reference implementation
can be consulted without changing the checkout. `server/CLAUDE.md` said only that
it is not in the working tree; that is true and misleading, and the sentence has
been extended.

#### Observed, on a real boot

`tools/ui-driver/probe-boot.mjs` against a release shell on a scratch profile
(`LOCALAPPDATA=/tmp/lc-probe LAPLUS_PORT=4774`), with one project registered over
the socket so the registry is not empty — the empty case was checked first and
says less. Inlined because this output lives nowhere that survives.

`GET /api/orchestration/shell` → 200, 358 bytes, `"snapshotSequence":1`. Then:

```
→ Request  orchestration.subscribeShell  {"afterSequence":1}
(no chunk)
```

No `"kind":"snapshot"` frame anywhere in the boot — the count is zero, where the
same probe before this change had one. The registry renders.

Two things in that run that are **not** regressions:

- **The console carries a 404 storm.** It is the draft pane, and it is ticket
  31's out-of-scope item unchanged: `GET /api/orchestration/threads/{draft}`
  correctly 404s four times a second until a first prompt creates the thread.
  Confirmed by hand — the shell route answers 200 and the draft thread route
  answers 404 for the id the storm names. The empty-registry boot has no storm
  because there is no pane open.
- **Those `subscribeThread` requests carry no `afterSequence`.** Which is ticket
  28 working: the HTTP snapshot 404s, so `canResume` stays false, so no cursor is
  sent, so the subscription is refused (`was not found`) and the client polls.

The thread half of _this_ ticket is pinned by
`a_thread_snapshot_read_over_http_does_not_travel_again_on_the_socket` rather
than by the probe. `probe-open-thread.mjs` names a thread id and a sidebar row
from another machine, and driving a real conversation costs an agent turn.

#### Also

- `rpc::resume_cursor` now takes a non-negative integer where the cursor used to
  be read as any non-null value. The contract says `NonNegativeInt`, so no
  conforming client changes behaviour; a client that sent nonsense used to be
  handed an open stream with nothing to fold events into, which is ticket 28's
  failure exactly.
- The draft-pane 404 volume is untouched and still out of scope. It is a client
  change.

### 2026-07-28 — agent. Review close-out

`/code-review` against the commit before this work. Four things acted on, none
of them a defect in what the ticket asked for.

**The two subscriptions were parsing the same field by different rules.**
`Shell::subscribe` read `afterSequence` with a bare `as_i64` while `Watch::read`
filtered it to non-negative, so the "malformed cursor" rule above was true of one
route and not the other. Inert — a negative can never equal a non-negative
watermark — but the ADR states one rule, and there is now one reader for it:
`rpc::resume_cursor`, beside `non_blank`, which is where this crate already keeps
the payload-field readers that more than one method needs. Not `store.rs`: that
file's header is explicit that its vocabulary is the registry's and not the
wire's, and it does not import `serde_json`.

**Two behaviours were unpinned, and both are now.** The re-description invariant
was tested on `subscribeThread` only, so the registry feed's identical closure
had nothing holding it — `orchestration::tests::a_subscription_that_opened_caught_up_is_re_described_once_it_has_not`
is the missing half. And nothing tested the non-negative filter at all;
`rpc::tests::only_a_non_negative_integer_is_a_cursor` does.

Both were written against a mutant rather than trusted for going green, which is
the only thing that makes a test added after the code worth having. Hoisting
`caught_up` out of the shell's description closure — the "remembered rather than
re-read" mistake — fails the first test and _nothing else in the suite_. Dropping
the `>= 0` filter fails the second and nothing else in the suite. Each was
therefore closing a real hole rather than restating a covered one.

**The heading was wrong.** `## Notes` where `docs/agents/issue-tracker.md` says
`## Comments`; tickets 31 and 33 both had it right.
