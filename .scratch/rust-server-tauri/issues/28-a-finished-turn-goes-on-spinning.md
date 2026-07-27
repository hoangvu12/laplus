# 28 — A finished turn goes on spinning in the window

**What to build:** a composer that stops saying "Working" when the work is done.

The agent answers, the server records the answer, and the thread view keeps
showing `Working for 3m 22s` with the reply never rendered. Observed at 3m 22s
against a turn the server finished in **5.4 seconds**.

**Status:** done

**Found by:** ticket 23, the first time a person sent a message through the real
UI. Nothing about it is shell-specific; the same server and the same client in a
browser would do the same thing, and it has presumably been true since ticket 10.

- [x] A turn sent from the composer of a new conversation renders its reply and
      stops spinning
- [x] A conversation opened from the sidebar still renders, as it already did
- [x] The whole suite still passes, and the tests no longer assert the thing
      that broke the window

## What the server has

Everything, and it is all correct. Read straight off `orchestration.subscribeThread`
while the window was still spinning:

| | |
|---|---|
| user | `Hey` |
| assistant | `Hey! What can I help you with in lightcode today?` |
| user | `Hello` |
| assistant | `Hi again! I'm ready when you are — …` |

Both messages `streaming: false`. Both turns `state: "completed"` —
`turn-…-1` in 3.7s, `turn-…-10` in 5.4s, `stopReason: end_turn`, no parse errors,
no unknown events. `session.status: "ready"`, `session.activeTurnId: null`.
Two checkpoints written, both `status: "ready"`. The `claude` child was alive and
idle, having been reused across both turns as continuity requires.

So the agent, the fold, the transcript, the checkpoints and the session lifecycle
all worked. **What failed is the client's picture of them**, and this ticket is
about finding out why.

## What the window showed

- The user's `Hello` bubble, and nothing else — neither assistant reply, and not
  the earlier `Hey` exchange, though both were in the same thread.
- `Working for 3m 22s`, counting from a `requestedAt` the client clearly had.
- Breadcrumb `lightcode / New thread`, while the sidebar — fed by
  `orchestration.subscribeShell` — correctly showed the thread under its
  generated title.

That split is the shape of the bug: **the shell subscription updated and the
thread subscription did not.** The client is watching a thread id that is not
receiving, or is receiving and not folding.

## Answer

**Receiving, and not folding.** The second one.

The events were arriving on the right subscription, in order, complete. The
client discarded every one of them, and was right to.

`client-runtime/src/state/threads.ts`, in `applyItem`:

```ts
const current = yield* SubscriptionRef.get(state);
if (Option.isNone(current.data)) {
  if (item.event.type === "thread.deleted") { yield* setDeleted(); }
  return;                          // ← every other event, dropped
}
```

A thread event is a **diff**. The client folds one into the conversation it
already holds, and the only thing that gives it one is a `snapshot`. So a
subscription that never opens with a snapshot is a subscription whose entire
contents are discarded on arrival — silently, with no error anywhere, which is
why the server looked correct from every angle. It *was* correct. It was
narrating the turn to a client that had nowhere to put it.

This server opened the composer's draft subscription with `{"kind":"synchronized"}`
and nothing else, then streamed the turn into it.

### What the reference server does, and the sentence that gives it away

`apps/server/src/ws.ts` refuses:

```ts
if (Option.isNone(snapshot)) {
  return yield* new OrchestrationGetSnapshotError({
    message: `Thread ${input.threadId} was not found`, cause: input.threadId });
}
```

And the client is *written for that refusal*. `subscribeDynamic` is given
`retryExpectedFailureAfter: "250 millis"` for this subscription — one of the two
places in the client-runtime that asks for it, the other being `state/shell.ts` —
so a refusal is not an error to a draft pane, it is a **poll**. The retry that
lands after the first prompt creates the thread opens with a snapshot, and the
conversation appears.

The `synchronized`-only opening is real, and this is the part worth keeping: it
is the answer to a **resume**, not to a draft. In
`fixtures/socket-wire/01-browser-session.ndjson`, request `3` — the capture this
server was built against — carries `afterSequence: 2`, and upstream's cursor
branch never asks whether the thread exists at all. This server was giving every
subscription the answer that belongs to one kind.

So there are two cases and this server had merged them:

| The client sends | It means | The answer |
|---|---|---|
| no `afterSequence` | "I have nothing" | the conversation, or a refusal if there is none |
| `afterSequence` | "I have it up to here" | what came after — and never a refusal, because it can already draw |

### What changed

- `crate::threads::Threads::subscribe` refuses a thread this server does not
  hold, with the declared `OrchestrationGetSnapshotError`. **Declared** is
  load-bearing: a defect would fail every other subscription on the socket
  (`crate::rpc::DispatchError`).
- Unless the call is a resume. `crate::threads::Watch` reads `afterSequence` —
  only whether it is *there*, not its value — because a client that sends one has
  somewhere to put an event. Refusing a resume would take a conversation the
  client can still draw from its own cache and replace it with an error.

  **This half was not needed to stop the spinner**, and it is worth saying so:
  the client sets `canResume` from whether it holds the thread, so it never sends
  a cursor for a draft and the exception cannot reach the bug. It is there
  because the refusal was a new way for this server to say no, and a client with
  its own copy is the one caller that must never be told it.
- A plain subscription no longer allocates a slot; a resume still does, because
  it is owed the events an absent conversation would produce and they arrive on
  that slot's channel. So a draft that is never sent no longer leaves one behind
  for the life of the process — which mattered little at one subscription per
  draft and matters more at four a second — while a resume for a thread that
  never exists still does.

### What it cost the tests

Nearly every conversation test opened on a draft and let the first turn create
the thread underneath a subscription that was already open. That is the shape
the client cannot use — the suite had encoded the bug as its model of the
client, which is why 647 tests never caught it.

`open_conversation` now creates the thread before watching it, so every event of
the turn still arrives live and every assertion about the *order* of a turn is
unchanged and still deterministic. Only `thread.created` moved out of the stream
and into the opening snapshot, which cost exactly one assertion.

The composer's own path is covered by
`socket_turn.rs::a_draft_becomes_the_conversation_the_composer_is_watching`,
which does the whole dance: refusal, dispatch, retry, snapshot, turn. Its
sibling covers the resume that must *not* be refused.

## How it was found, and the thing worth keeping

The server could not be debugged from the server. Every log, every event, every
snapshot said it was working, because it was.

What answered it was a headless browser on `http://127.0.0.1:4773/` with the
CDP network domain turned on, printing the client's own frames. It took about
ten minutes to see, once the frames were on screen, that the pane's subscription
was receiving events it did not render.

That driver is in **`tools/ui-driver/`**, with a README. It is the sibling of
`tools/wire-capture/` — that one records what the reference server answers, this
one drives what the real client does — and it is the only way anything about the
UI half of this application can be checked. Ticket 27 will want it.

The three checks it gave, in the order they were worth running:

1. `probe-open-thread.mjs` — the free one this ticket already named. The
   conversation still in the registry **rendered**, four messages and both
   `Worked for` durations. So the thread subscription was not broken generally,
   and the bug was the draft transition.
2. `probe-boot.mjs` — the client subscribes to the *real* thread id it is about
   to create, before creating it. So the id was never wrong, which was the other
   open theory.
3. `repro.mjs` — types into the composer and watches the pane. Red at
   `Working for 30s`; green at `Worked for 4.7s` with the reply drawn, in the
   shipped release binary.

## What was ruled out

**"A subscription opened on a thread that does not exist yet never wakes up."**
This was the first theory and it was recorded here as **wrong**. It was wrong
about the mechanism and right about the symptom, and the correction is worth
keeping straight: the feed does wake up, the events do arrive, and the pane
still never renders. Acknowledgement was never the problem — the real client
acknowledges everything. The problem was that the first thing the feed ever
carried was not a snapshot.

The earlier probe that stalled after one chunk was still at fault, and the note
it left is still true for anyone writing one: `Ack` is real back-pressure, the
server sends **one** unacknowledged chunk and stops, and in .NET a pending
`ReceiveAsync` must not be cancelled to implement a timeout. None of that is
this bug.

## Comments

### The rule this leaves behind

A snapshot is not an optimisation and not the first of a series. It is the
**only** thing that makes every event after it mean anything, because an event
is a diff and a diff needs something to apply to. A subscription that opens
without one is not a subscription that is merely quiet — it is one whose entire
contents will be discarded, by a client that will report nothing and show a
spinner.

Worth carrying to any subscription this server grows: the question is never
"are the events right", it is "does the client have anything to fold them into".

### What would have prevented it

Not a better server test. The server was right, and a suite of 647 tests written
against this server's own idea of the client passed throughout — because the
harness modelled the client from the same misreading the implementation did.
Two witnesses, one source.

The thing that would have caught it is the thing that eventually did: driving
the **real client** once, against the real server, before declaring the seam
done. That is now cheap — one command, `tools/ui-driver/repro.mjs`. It costs
an agent turn and about forty seconds.

Ticket 23 ticked "a full agent conversation works end to end inside the window"
on the strength of reading the events off the socket, and the note beside it
says the window showed none of it. That is the gap, and it is a process one
rather than an architectural one: the criterion said *inside the window*, and
what was checked was the wire.
