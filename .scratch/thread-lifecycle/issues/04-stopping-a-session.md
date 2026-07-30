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

**Status:** ready-for-agent

- [ ] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [ ] An unknown thread is refused with a sentence naming it.
- [ ] Stopping a thread with no session behind it is answered rather than
      treated as an error — there is nothing to stop and nothing went wrong.
- [ ] The session status the developer sees afterwards reflects that the process
      went away, and settles the latest turn accordingly.
- [ ] The thread, its transcript and its work log all survive.
- [ ] The agent session identifier survives, so the conversation can be resumed.
- [ ] A subsequent turn on the same thread starts a new session and continues the
      same conversation rather than beginning a fresh one.
- [ ] The change is published on the thread's own feed and reaches the project
      list, which renders session state.
- [ ] A subscriber on a second connection sees it.
- [ ] No agent process is left behind: the server's own count of live agents
      returns to what it was before the session started.
