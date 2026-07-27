# Context

lightcode's domain language. One term per entry, as the code uses it.

Two vocabularies meet in this project and it is worth naming which is which:
the **agent protocol** is what the `claude` CLI speaks, the **contract** is what
the reused UI speaks. Where a term exists on both sides, the entry says so.

## Conversation

**Thread** — one conversation, scoped to a project. What the UI reads. Survives
a restart; the agent process behind it does not. `crate::threads`.

**Turn** — one exchange within a thread: the developer's prompt and everything
the agent does before it goes quiet. Has an id the client mints.

**Session** — the agent process behind a thread, as the client sees it. A thread
with no session is normal — after a restart, every thread has none.

**Agent session id** — the `claude` CLI's own handle on a conversation, given
back to it as `--resume`. The one piece of agent-protocol vocabulary that
reaches the database, because continuity depends on it outliving the process.

**Work log** — the row-per-thing-that-happened view beside the transcript: tool
calls, thinking, permission requests and their resolutions. `crate::worklog`.

**Activity** — one row in the work log.

## Lifecycle

**Session status** — what the agent process is doing. The contract's seven:
`idle`, `starting`, `running`, `ready`, `interrupted`, `stopped`, `error`.
`crate::settling::SessionStatus`.

**Turn state** — how the most recent turn went. The contract's four: `running`,
`completed`, `interrupted`, `error`. `crate::settling::TurnState`.

**Settling** — reading a session status as a turn state. Leaving `running` is
the end of a turn, not the last assistant message, which is what makes a turn's
duration cover the whole turn. Upstream's word (`decider.settled.test.ts`,
`threadSettled.test.ts`), kept.

Note the two are not opposites of each other. `interrupted` and `stopped` are
different *statuses* — the developer asked, versus the process went away — and
both settle a turn as `interrupted`, because from the turn's point of view they
are the same thing: it did not finish.

**Ending** — how a turn ended, as the driver knows it: completed, failed, or
stopped. Distinct from turn state because the CLI reports a stopped turn as a
failed one, so only this server's own knowledge that it asked can tell them
apart. `crate::turn::Ending`.

## Protocol

**Drift counter** — a tally of agent-protocol events this build did not
recognise. Unknown variants increment it instead of failing, so a CLI upgrade is
learned from a number rather than a bug report. Two of them, because they are two
failures: an unrecognised event type and a line that is not JSON at all.
`crate::protocol::Drift`, which is subtractable — a turn reports its own, the
session reports its total.

**Compaction** — the agent summarising its own conversation to make room, and
carrying on. A fact about what the *agent* can still see; the transcript is this
server's own copy and is untouched by one. `crate::protocol::Compaction`.

**Standing** — how the developer's account is placed against its usage limits,
as the CLI reports it: allowed, close to the limit, or refused.
`crate::protocol::RateLimit`. Agent-protocol vocabulary with no contract
equivalent, so it reaches the developer as an activity rather than as a field.

**Reconciliation** — assistant text arrives twice, as deltas and again as a
buffered message. The deltas drive live rendering; the buffered message is
authoritative and replaces the accumulation. Whether the two agreed is recorded.

**Join** — a place where the agent protocol and the contract meet. `crate::turn`
is the declared one; `crate::worklog` is a second.

## Working tree

**Working tree status** — what has changed in a project's folder, as the UI
reads it: the branch, the changed files with their line counts, and how the
branch stands against its tracking ref. `crate::git`. The contract calls the two
halves **local** and **remote**; here they are read together, because neither
costs a network.

**Read** — running git and turning what it says into a status. The unit of work
this subsystem does; everything else is about when to do one.

**Stale** — a working tree that has changed since the last read started. What a
file change produces; a read is what clears it. The distinction is load-bearing
— see ADR-0006.

**Coalescing window** — the pause before each read, in which a burst of changes
becomes one read.

**Kept** — a working tree the server is holding a status for and watching.
`crate::git::Repositories`. Bounded at the same number as watched workspaces,
because a status that cannot be watched cannot stay true.

**Disturb** — telling a kept working tree it is stale because *this server*
just changed it, rather than waiting for the watcher to notice. What a switch
and an init do. The same door a file change comes through, opened from the
inside; see ADR-0006 for why it marks rather than reads.

## Review

**Checkpoint** — what a project's working tree looked like at one turn boundary,
kept as a parentless commit under a ref of the project's own repository.
`crate::checkpoints`. The contract's word, and the thing that makes a turn a
point in time a diff can run to.

**Baseline** — the checkpoint a turn is diffed *from*: the one taken before the
prompt reached the agent. Turn one's baseline is turn count zero; every later
turn's baseline is the checkpoint the turn before it ended with, which is what
makes a conversation's checkpoints a chain rather than a set of pairs.

**Turn count** — how many turns of a conversation have been recorded. Also the
name of the checkpoint that recorded the last of them, so a turn's diff runs
from `n - 1` to `n` and a whole conversation's from `0` to `n`.

**Turn diff** and **thread diff** — one step in isolation, and the session as one
coherent change. Two methods over one range: a thread diff is a turn diff whose
`fromTurnCount` is zero.

A checkpoint is a *photograph*, not a record of authorship — see ADR-0008. It
does not know who changed a file, so an edit the developer made by hand between
two turns belongs to the turn it happened during, beside the agent's own.

**Checkpoint status** — how the turn a checkpoint records *went*, not whether
recording it worked. The contract's three (`ready`, `missing`, `error`), of which
this server sends two: the client reads the status back into the turn's state, so
a status that disagreed with how the turn ended would relabel it. There is none
that means interrupted, which is why a turn the developer stopped gets no
checkpoint. `crate::turn::Ending::checkpoint_status`.

## Refs

**Ref** — a branch, in the contract's word. The UI says `refName` everywhere
because a git branch and a jj bookmark are the same field to it, and this
server keeps the word rather than translating it back. `crate::refs`.

Not every ref is a branch: a **remote ref** (`origin/main`) is a record of
where a branch was on a remote, and is not something a working tree can be
*on*. Switching to one means making the local branch that tracks it.

**Current** — the ref this workspace has checked out. A property of a place,
not of a repository: the same branch is current in one worktree and merely
**checked out** in another, and only one of those can be switched away from.

**Default ref** — what a repository considers its trunk: the remote's recorded
`HEAD` if there is one, and otherwise whichever of `main` and `master` exists.
A convention where git has no answer, never a guess where it has none.

**Fold** — dropping a remote ref that has a local branch of the same name.
`origin/main` beside `main` is a row that says nothing, so the picker does not
show one unless the client asks (`includeMatchingRemoteRefs`).

## Terminal

**Terminal** — one shell in a project's folder, named by the client and unique
within a thread. The client always chooses the name; the server never allocates
one. `crate::terminal`.

**Pane** — the terminal as the developer sees it: the emulator half, in the UI.
The server owns the pty half and the wire between them, and nothing else. Not a
type here; the word matters because it is what "resize the terminal" is a
consequence of.

**Scrollback** — the server's copy of what a terminal has shown, sent to a
client that attaches or that fell too far behind to catch up. Not the stream: it
is replayed into a live emulator, so the questions are taken out of it.
Bounded, at the same number the client's own buffer is bounded at.

**Question** — a control sequence that asks the emulator something rather than
telling it something: cursor position, device attributes, the colour queries. A
shell blocks on the first one it asks. Kept out of scrollback and remembered
instead, so that whoever attaches next is asked it. See ADR-0005.

**Attachment** — one `terminal.attach` subscription. A terminal outlives every
attachment to it, which is what makes reattaching a thing rather than a restart.

**Detaching** — not a call. Navigating away from a pane cancels its attachment
and touches nothing else, so the shell keeps working with nobody listening and
everything it says goes into the scrollback the next attachment is sent.

**Exited** and **closed** — two different endings, and the contract has an event
for each. A terminal whose shell exited is still on the list, still readable,
and can be given a new shell by name. A terminal that was *closed* is gone: the
developer said so, the process was killed and waited for, and the id no longer
names anything. Reaping happens at the second, never the first.
