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

**Status:** needs-triage

- [ ] `thread.turn.start` refuses a runtime or interaction mode the contract does
      not name, before the world is consulted, naming the mode and the thread.
- [ ] `thread.create` does the same.
- [ ] An absent per-turn mode still means "unchanged" rather than "the default" —
      the existing behaviour, and the reason neither field has a serde default on
      `StartTurn`.
- [ ] The existing turn, permission and continuity suites stay green: every mode
      they send is already one the contract names.
