# 04 — Stopping a session

**What to build:** the developer can end the agent process behind a conversation
without interrupting a turn and without closing the window, and the conversation
is left intact and resumable.

Today the stop control dispatches a command the server refuses. The only ways to
get rid of an agent process are to interrupt a turn — which is a different act,
aimed at a turn rather than at the process — or to restart the server.

The distinction worth holding onto: **interrupting** asks a running turn to stop.
**Stopping** ends the session. A wedged or idle agent holding resources is the
case this exists for, and it has no turn to interrupt.

Stopping must not lose the conversation. The thread survives, its transcript
survives, and the agent session identifier survives, because that identifier is
the whole of how continuity outlives the process — it is what the CLI is handed
back to resume the conversation it was holding.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [x] An unknown thread is refused with a sentence naming it.
- [x] Stopping a thread with no session behind it is answered rather than
      treated as an error — there is nothing to stop and nothing went wrong.
- [x] The session status the developer sees afterwards reflects that the process
      went away, and settles the latest turn accordingly.
- [x] The thread, its transcript and its work log all survive.
- [x] The agent session identifier survives, so the conversation can be resumed.
- [x] A subsequent turn on the same thread starts a new session and continues the
      same conversation rather than beginning a fresh one.
- [x] The change is published on the thread's own feed and reaches the project
      list, which renders session state.
- [x] A subscriber on a second connection sees it.
- [x] No agent process is left behind: the server's own count of live agents
      returns to what it was before the session started.

## What it came out as

**A signal, not a closed pipe.** Dropping the prompt channel is how a shutdown and
a deleted project say "no more turns", and it was tempting here: the CLI exits on
EOF, and closing stdin is even what unwedges an agent stopped on a permission
nobody answered. But it has no bound. The driver only leaves its loop when the
agent's _output_ ends, so a child that ignores its stdin is one this server waits
on forever — and "a wedged agent holding resources" is the case the ticket names.
So a stop is a `Signal::Stop` on the channel a decision and an interrupt already
travel, and the driver answers it by leaving the loop: the ending it then runs
closes stdin, waits, and kills if waiting was not enough. Two steps, the first of
which usually suffices, which is `crate::agent`'s own account of termination.

**Two things had to be marked as the developer's doing.** A driver whose agent
goes away mid-turn reports that the agent stopped before the turn finished and
settles the session as `error`; and it checkpoints the turn it lost. Neither is
true of a stop somebody asked for, and the second is worse than untrue — there is
no checkpoint status meaning "the developer ended the session", so any row it
wrote would relabel the turn.

**The slot is freed at once, and that needed a session epoch.** The next turn has
to start a _new_ session — the branch toolbar stops one and moves the conversation
in the same breath — so the registry gives the slot up before the child has been
reaped. That leaves a driver alive with no slot, and an unguarded one would then
detach the session that replaced it, taking a live agent off the gauge while its
child ran on, and publish an ending over a turn that had just started. Each
session now carries which of the conversation's it is, and a driver gives up only
its own.

**A stop mid-turn settles the turn, and which state it settles to depends on
whether the agent had said anything** — which is the client's arithmetic rather
than a decision here. The receipt stops the session; a partial reply is then
closed with a buffered message, and a buffered message settles the latest turn
once the session is no longer running it, as `completed`. With nothing streamed to
close — mid tool call — there is no such message, and the session's own `stopped`
settles it as `interrupted`. One rule read at two moments, and `threadReducer.ts`
reads it the same way at both: making the server say something else would put it in
disagreement with every window folding the same events, which is the one thing the
fold exists to prevent. The turn is not the subject of this ticket in any case; a
wedged or idle agent has no turn at all.

**Driven in the real client**, through the only route that reaches this command:
the delete flow, which sends the stop and then a `thread.delete` this server still
refuses (ticket 10). `tools/ui-driver/probe-session-stop.mjs` is that run — the
stop is dispatched and answered, the server reports the session `stopped` with no
active turn, the transcript is untouched, and no `claude` child is left behind.
The other call site, moving a conversation to another worktree, is unreachable
here because this server refuses to prepare one.
