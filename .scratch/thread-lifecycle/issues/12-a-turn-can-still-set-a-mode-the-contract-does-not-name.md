# 12 — A turn can still set a mode the contract does not name

**What to build:** the mode vocabulary ticket 02 introduced should guard every
door a mode arrives through, not only the two commands ticket 02 added.

Found by reviewing ticket 02 against its own spec. `named_by_the_contract` is
called from the `thread.runtime-mode.set` and `thread.interaction-mode.set` parse
arms and nowhere else. Two older doors are still open:

- **`thread.turn.start`** — `StartTurn::runtime_mode` and `interaction_mode` are
  `Option<String>` with no validation, and `Change::TurnRequested` writes
  whichever arrives straight onto the thread and publishes it.
- **`thread.create`** — `ThreadFields::runtime_mode` and `interaction_mode` are
  `String` with a serde default and the same absence of a check.

So `{"type":"thread.turn.start", …, "runtimeMode":"bypassPermissions"}` is
accepted, stored, and published on a wire where the contract types the field as a
closed `RuntimeMode` union. The client's decode of the whole thread payload fails
on a literal it does not know, so the cost is not a wrong badge — it is a
conversation the UI cannot draw at all.

This is the **hot** path rather than an edge one, which is what makes it worth a
ticket. Driving ticket 02 in a real window showed the composer sends the per-turn
override on _every_ send (`persistThreadSettingsForNextTurn`, then
`startThreadTurn` carrying `runtimeMode` again), so the unguarded door is the one
almost every mode change actually goes through. Ticket 02's guard only catches the
command the composer sends _beside_ it.

## Why it was not folded into ticket 02

Ticket 02 scoped itself to "only the two commands that write them and the two
events that announce it", and closing this widens a **refusal** on two commands
that have shipped since ticket 10. A stricter server can break a client that a
looser one accepted, and this repository treats refusing a value on the way in as
a posture with an ADR behind it (ADR-0009, a declined setting) rather than as a
free win. The practical risk looks like nil — every mode the real client can send
is one of the contract's literals, because the contract's own types make it so —
but "looks like nil" is a judgement for a human to make about their own users.

The fix itself is small: reuse `RUNTIME_MODES` and `INTERACTION_MODES` and the
existing `named_by_the_contract` in the two older parse arms.

**Blocked by:** None. Wants the call on whether to tighten shipped commands.

**Status:** done

- [x] `thread.turn.start` refuses a runtime or interaction mode the contract does
      not name, before the world is consulted, naming the mode and the thread.
- [x] `thread.create` does the same.
- [x] An absent per-turn mode still means "unchanged" rather than "the default" —
      the existing behaviour, and the reason neither field has a serde default on
      `StartTurn`.
- [x] The existing turn, permission and continuity suites stay green: every mode
      they send is already one the contract names.

## Comments

**Delivered.** The call to tighten was made when this was picked up.

`named_by_the_contract` turned from a function that consumes a mode and hands it
back into one that checks a borrowed one. That is what let the same rule reach
the two doors that carry their modes inside a `ThreadFields`, where taking the
string out to validate it would have meant rebuilding the struct around it.

The ticket named two doors and there are in fact **three**, which is why the
`ThreadFields` check is a method on the struct rather than a line in a parse arm:

- `thread.turn.start`'s per-turn override, the hot one — checked only when it
  arrived, so absent still means "leave the thread's alone".
- `thread.turn.start`'s `bootstrap.createThread`, which is how the composer
  creates a conversation for a first message.
- `thread.create`, the client-runtime's path when a conversation starts anywhere
  other than the composer's draft.

The last two are the same `ThreadFields` — one struct, as its own comment says,
so one check on it closes both and any door added later.

**On the risk the ticket left for a human.** It looks like nil, and this is now
checked rather than assumed. `apps/web/src/composerDraftStore.ts` filters what it
reads back from `localStorage`, so even a stale draft holding a mode from another
build cannot be sent — by `Schema.is(RuntimeMode)` for the runtime mode (lines
1547, 1679 and 2803) and by comparison against the two literals for the
interaction mode (1551). Two different mechanisms for the same rule, worth
knowing before trusting one of them.

Every runtime-mode literal in `apps/web` and `packages/client-runtime` outside
the contract is in a test file and every one is `full-access`. Interaction-mode
literals do appear in shipped code — `ChatView.tsx:5122` and `:5145`,
`proposedPlan.ts:85` and `:91` — and all four are `default` or `plan`.

**One seam, as the spec assigns.** Payload validation is "one sentence about one
payload", so this is four unit tests in `orchestration.rs` and no new socket
binary. Two of them assert against a thread that does not exist, which is what
makes the refusal say which check ran — the unknown thread is the world's answer
and the mode is the payload's, so the ordering is visible rather than claimed.

Whole suite green, 590 lib tests and every integration binary. Not driven in a
window: this adds no control and no behaviour a developer can reach, and
`probe-thread-modes.mjs` — the probe that would exercise the composer's send —
wants a real profile database copied in to have a conversation to work on.

**One residual, deliberately left.** The guard is at the door, so a row already in
the database holding a mode the contract does not name is still read back and
still published — and the doors were open until now, so such a row is possible
rather than hypothetical. Closing that is a read-side question (refuse, or round
on the way out, or a migration) and it is a different decision from this one.
`CONTEXT.md` records what the door now promises; nothing yet records what the
store may still hold.
