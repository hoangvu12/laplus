# 02 — Runtime and interaction mode become editable

**What to build:** the developer can change a conversation's runtime mode and its
interaction mode after the conversation has started, from the pickers that
already exist, without starting a new thread.

Today both are write-once. They are set when the thread is created and can only
be moved by the per-turn overrides that ride along with a turn request, so the
pickers change nothing on their own. Two commands close that.

This is the cheapest real win in the whole effort and should be built first:
both columns already exist on the thread, both values are already published, and
both are already read back after a restart. Nothing is added to the schema and
nothing is added to the read model — only the two commands that write them and
the two events that announce it.

A mode set now applies to the **next** turn, not the one already running. The
rules an agent is working under must not move under its feet mid-turn, and the
per-turn override already carries the composer's selection for the turn being
started.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] Both commands are parsed before the world is consulted, so a malformed
      payload is refused at the door with a sentence naming what was wrong and
      which thread it was wrong about.
- [x] A mode the contract does not name is refused rather than rounded to the
      nearest one the server understands.
- [x] A blank thread identifier is refused.
- [x] An unknown thread is refused.
- [x] Each command answers with the sequence it committed at.
- [x] Each change is published on the thread's own feed and on the project list,
      because the list renders the mode.
- [x] A subscriber on a second connection sees the change.
- [x] The new mode survives a restart and is what the picker shows next time.
- [x] A mode set while a turn is running does not change that turn; the next turn
      starts under the new mode.
- [x] Setting a mode to the value it already holds is harmless.

## Comments

**Delivered.** `crate::orchestration` gained the two commands and the mode
vocabularies they validate against; `crate::threads` gained
`Change::RuntimeModeSet` and `Change::InteractionModeSet`, which reach both feeds
and write the thread's row like every other change. No migration, no new field,
no new read-model shape — the columns and the published values were already
there, as the ticket said.

Two seams, as the spec asks. Payload validation is unit-tested in place in
`orchestration.rs`; the socket seam is a new binary,
`tests/socket_thread_modes.rs`, which drives each command as a real
`orchestration.dispatchCommand` and asserts the sequence, both feeds, a
subscriber on a second connection, a _fresh_ subscriber, and a restart.

### Driven in the window

`server/tools/ui-driver/probe-thread-modes.mjs`, new here, against a laplus on a
scratch profile with a copy of a real database and an agent binary that is not an
agent. It picks Supervised, toggles to Plan, sends a message, and reads the modes
back off `/api/orchestration/threads/{id}`. Green: both commands go out — in that
order, before `thread.turn.start` — neither is refused, and
`approval-required`/`plan` is what the server holds after a reload.

Three things that drive turned up, none of which the suite could have:

**The picker does not dispatch on click.** `handleRuntimeModeChange` writes the
composer's draft in `localStorage` and nothing else; both commands go out from
`persistThreadSettingsForNextTurn` on **send**. So the ticket's "from the pickers
that already exist" is right, and the moment is the developer's next message —
which is also why "a mode set now applies to the next turn" needed no work: the
client already sequences it that way.

**The picker's label was never evidence.** `ChatView` reads
`composerRuntimeMode ?? activeThread?.runtimeMode`, so a mode the server refused
outright still survived a reload on the same origin. The first version of the
probe believed the label and went green against the unfixed server. It now reads
the server's copy.

**And the real payoff is gated on ticket 03** — see the note added there.

### One thing found and deliberately left

**A reused CLI process does not honour a new runtime mode.** One child serves a
whole conversation, `--permission-mode` is given once at launch
(`crate::turn::open`, via `agent::permission_mode_for`), and the agent protocol
has no control request that moves it afterwards. So after tightening
`full-access` to `approval-required` mid-conversation, the next turn is
_requested_ under the new mode — the thread carries it, the picker shows it, and
`Session.runtimeMode` on the turn's `starting` event reports it — but the running
`claude` keeps bypassing permissions until its process is replaced.

The same is true of the per-turn override that has shipped since ticket 10, so
this is older than these two commands and not something they introduced. It is
also visible a second way: the long-lived driver publishes `session.runtimeMode`
from the `Start` it captured at the _first_ turn, so a later turn's `running`
event reports the mode the conversation began with even though its `starting`
event reported the new one.

Both are one question — _does a runtime-mode change need the session
restarted?_ — and answering it means deciding whether to kill and `--resume` a
child on a mode change, which is design work with a user-visible cost (a
re-`init`, a lost warm context window). Out of scope here, which said "only the
two commands that write them and the two events that announce it". Worth its own
ticket.
