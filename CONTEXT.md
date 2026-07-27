# Context

laplus's domain language. One term per entry, as the code uses it.

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

**Snapshot** — a subscription's opening description of the world, and the thing
every event after it is a *diff against*. Not an optimisation and not merely the
first chunk: the client folds an event only into state it already holds, so a
subscription that opens without a snapshot has its whole contents discarded on
arrival, silently. Ticket 28 was that, for a whole turn. The question to ask of
any subscription here is never "are the events right" but "does the client have
anything to fold them into".

**Resume** — a subscription from a client that says it already holds the
conversation, by sending `afterSequence`. The one case that is *not* refused for
a thread this server does not have, because a client with its own copy can still
draw it and an empty snapshot would be a claim that copy is wrong.
`crate::threads::Watch::resuming`.

Only the cursor's *presence* is read. Its value is ignored, and what that costs
depends on the case: for a conversation this server holds, the client asked for
the tail after its cursor and gets the whole thing as a snapshot instead — more
bytes than the reference server sends, and correct, because a snapshot replaces
what the client holds rather than being folded into it. For one this server does
not hold there is nothing to send either way.

**Draft** — a conversation the client has made up and the server has never heard
of. Where every new conversation starts: the composer mints the id, and the
thread reaches this server only when the first turn is dispatched carrying
`bootstrap.createThread`. Not a state this server stores — it is precisely the
absence of one, which is why a subscription to a draft is refused rather than
opened. The opposite of a **resume**: a draft is a client with an id and nothing
else, a resume is a client with the conversation and no need of us to prove it
exists.

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

## Settings and keybindings

**Preferences** — the directory this server keeps the developer's own files in:
`settings.json`, `keybindings.json`, the logs and the registry. On
`ServerConfig` and deliberately off the wire; the paths that *are* on the wire
are derived from it.

**Patch** — a settings change, every field optional, where an absent field means
**unchanged** rather than "set to the default". What `server.updateSettings`
takes, and also how a stored file is read: one is all-or-nothing so a refusal
changes nothing, the other is per-field so a key from another build costs only
itself. `crate::settings`.

**Rule** and **resolved keybinding** — the two forms of a binding. A rule is
what a person writes and what is in the file (`mod+shift+d`); a resolved
keybinding is what the UI consumes — the shortcut split into the flags a
`KeyboardEvent` carries, and the `when` expression parsed into a tree. Compiling
one into the other is `crate::keybindings`, because the client holds no file.

**Merge by command** — a custom rule replaces the default for the *same
command*, however either is spelled. What makes rebinding one shortcut leave the
other forty alone, and what makes removing one bring its default back rather
than leaving nothing.

**Issue** — something that went wrong assembling the configuration, shown in the
UI rather than logged. Its `kind` is one of two literals the contract names, not
a label: `ServerConfig.issues` is an array of a closed union, so an invented kind
fails the client's decode of the whole payload — which is why a settings problem
is logged instead, having no member of its own.

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

## Shell

**Shell** — laplus as a desktop application: a window, and the server
running inside the same process. `crates/laplus-shell`. The server is a
library to it, not a service it talks to, which is why closing the window is
what reaps the agents.

**laplus** — the UI, as a repository this project owns: a fork of t3code, cloned
beside this one. Where the window's contents are changed, and the answer to
"can we fix that in the client?", which used to be no. See ADR-0012. Also, since
the rename, what this whole product is called.

**lightcode** — what laplus was called until ticket 30 closed. Retired from the
live code entirely, and deliberately *not* rewritten anywhere it is a record of
something: `fixtures/` are captures of real traffic and the paths inside them
happened, `.scratch/` tickets and `docs/adr/` are dated statements of what was
decided at the time, and editing either would make them claim to be evidence of
things that never occurred. So roughly sixty mentions survive on purpose. If a
mention is in code, configuration or living documentation, it is a bug.

**Bundle** — laplus's built web application, `laplus/apps/web/dist`, as it
reaches the executable: a table of names and bytes generated at build time.
Source maps are the one thing dropped, and they are two thirds of it.

**Upstream** — `pingdotgg/t3code`. Still the origin of every line of the UI and
of the contract, and still the thing laplus is measured against — but now a
remote to merge from rather than a checkout to read.

**Assets** — the bundle as the server holds it, and the rules for answering a
request from it. `crate::ui::Assets`. Empty for every server but the shell's,
which is what keeps a UI out of the test binaries and out of the plain server.

**Entry point** — `index.html`, and the answer to any path that looks like one
of the UI's own **routes** rather than a file. The UI routes in the browser, so
`/settings` is a path this server has never heard of and must not 404. A missing
*file* still must.

**Server version** — `environment.serverVersion`, and in the shell it is the
**bundle's** version rather than the crate's. Not a claim about the binary: the
client compares that field against the version compiled into its own page and
warns about a skew, which between a UI and the executable it ships inside cannot
happen. Made equal so the comparison finds nothing, and vestigial rather than
satisfied. The plain server, which ships no bundle, keeps the crate version —
there a difference is real. See ADR-0011.

**Origin** — scheme, host and port together, and the reason the port is fixed.
The window is pointed at `http://127.0.0.1:4773/`, so the port is part of what
the browser scopes `localStorage` by — and `localStorage` is where the UI keeps
the developer's layout, drafts and open thread. See ADR-0010.

**Install directory** — `%LOCALAPPDATA%\Programs\laplus`, where the
installer writes. Named separately from the **data directory** —
`%LOCALAPPDATA%\laplus`, `config::data_dir`, which holds `state.sqlite`,
`keybindings.json` and `logs/` — because until ticket 30 they were one
directory and nothing in either half said so. Moving the install is what
separated them; the data has not moved. See ADR-0013.

## Measuring the artifact

The vocabulary of `cargo xtask release`, which exists because the project has a
number to hit. Note that **bundle** is *not* in this list: it means the web
bundle above and nothing else, which is why the command is `release`.

**Artifact** — what a developer ends up with. Three figures rather than one, and
they differ by more than rounding: the **installer** is what they download
(5.06 MB, LZMA-compressed), the **footprint** is what it leaves on their disk
(24.33 MB), and the **binary** is the application alone. The spec's "under
~30 MB" story is about the first.

**Footprint** — a directory, weighed. Measured two ways and it says which:
**installed** ran the real installer and weighed the **install directory** it
made, **payload** weighed the files the bundle ships without installing
anything. Only the first is the truth; the second is what runs when the machine
has not been volunteered.

**Target** — 20–30 MB, against upstream's 318 MB **baseline**. Only the ceiling
can be *missed*; coming in under the floor is the project working, not a fault,
which is why `size::Verdict` has three cases and not two.

**Breakdown** — a Rust source, split into total, comments, `#[cfg(test)]` unit
tests, blank, and what is left. What is left is **production code**, and it is
the figure set against the spec's ~20K **signal** — the total is three times
larger, mostly prose and this server's own tests, and reporting *it* would trip
an alarm about scope creep that has not happened.

**Balanced** — a scan that ended where a Rust file is allowed to end: outside
every comment, literal and `#[cfg(test)]` region. Its own correctness check. An
unbalanced scan reports no line count at all, because the way this measurement
fails is silently, with a number that looks fine.
