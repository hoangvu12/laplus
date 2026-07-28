# ADR-0008 — A turn is a photograph of the working tree, not a record of who typed

Date: 2026-07-27
Status: Accepted

## Context

Ticket 20 asks for two views: the diff of a single agent turn, and the
cumulative diff of a whole conversation. Both need a _before_, and the developer
has not committed anything — the whole point of the feature is reviewing work
that is not on a branch yet. Nothing in git records what the working tree looked
like when the developer pressed enter, so the server has to.

Three things bound the answer.

**The developer's repository is theirs.** `git add` and `git commit` move `HEAD`,
rewrite `.git/index` and put work on a branch. A review feature that committed
for the developer would be the worst kind of surprise, and one that staged for
them would silently destroy the staging they had.

**A turn's changes are not attributable.** The agent writes files through its own
tools, in a child process, and the server hears about it — if at all — through a
filesystem watcher that reports paths and not authors. There is no channel that
says "the agent wrote this line and the developer wrote that one", and there
could not be one without instrumenting every tool the CLI has.

**Untracked files are the interesting case.** A file the agent has just created
is the most common thing a turn produces, and `git diff` over a working tree has
nothing whatever to say about one.

Three shapes were available.

| Option                                                                                   | Cost                                                                                                                                                                                |
| ---------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Watch the filesystem and accumulate a per-turn change set                                | Needs before-and-after content for every path, which is a second copy of the working tree kept by hand; loses anything written between two watcher events; cannot survive a restart |
| Diff the working tree against `HEAD` and subtract                                        | Only ever answers "since the last commit", so every turn of a conversation shows the same diff; a commit mid-conversation erases every earlier turn                                 |
| Record the whole tree at each turn boundary, as an object git already knows how to store | One `git add -A` per turn boundary                                                                                                                                                  |

## Decision

**A turn boundary writes the entire working tree — tracked, staged, untracked
and all — as a parentless commit under `refs/lightcode/checkpoints/…`, and a
diff is `git diff` between two of those commits.**

It is done with plumbing (`read-tree`, `add -A`, `write-tree`, `commit-tree`,
`update-ref`) against a **temporary index** supplied through `GIT_INDEX_FILE`, so
none of the developer's own state is read or written. The commit has no parent,
which keeps it out of `git log`, `git branch --contains` and every push.

The consequence that matters is in the title. A checkpoint is a photograph of a
folder at a moment. It does not know who changed what, and it does not need to:
the question the diff panel asks is _what is different now_, and a photograph
answers exactly that. So a file the developer edited by hand between two turns
appears in the diff of the turn it happened during, alongside the agent's own
work.

That is the right answer as well as the only available one. A review pass is
about the state of the code, not about authorship; a diff that hid the
developer's own edit would show a change set that does not compile on its own,
and one that showed it separately would be inventing a distinction the working
tree does not contain.

## Consequences

- **Five of the ticket's criteria are properties of `git diff` rather than code
  here.** Added, modified, deleted and renamed files; an empty diff for a turn
  that changed nothing; untracked files appearing as additions; binary files
  named rather than rendered. Each is a test rather than an implementation.
- **A capture re-hashes the working tree.** The temporary index starts from
  `HEAD` with no stat cache, so `add -A` reads every file git does not already
  know. That is the cost of a cold `git status`, paid once per turn boundary. It
  is affordable because a turn is a human-scale event, and it is the reason
  checkpoints are not taken more often than that. Ignored files cost nothing.
- **There is a window at the end of a turn.** The capture happens _after_ the
  turn has settled, because blocking the "the agent is done" signal on a
  `git add -A` would make a large repository feel like a slow agent. An edit made
  inside that window — a fraction of a second — lands in the next turn rather
  than this one. The `thread.turn-diff-completed` event is what says the window
  has closed, and it is what the panel adds the turn to its list on.

  The window is also the reason `crate::turn`'s loop drains its signal channel
  before starting a queued turn. A stop click arriving during a capture would
  otherwise be handled _after_ the next prompt had been sent, and would stop the
  turn it started rather than being the no-op the client meant it as.

- **A turn the developer stopped gets no checkpoint.** `status` is read by the
  client as how the turn went, not as whether the tree was recorded:
  `threadReducer.ts` sets `latestTurn.state` from
  `checkpointStatusToTurnState(status)` on every checkpoint it folds, and that
  function has three inputs and two outputs — `ready` and `missing` both mean
  `completed`, `error` means `error`. **There is no status that means
  interrupted.** So a stopped turn is not recorded at all, and its changes fall
  into the diff of the turn that follows: the alternative is a row that relabels
  the turn as finished, undoing the settle ticket 14 exists for and leaving this
  server and the client describing the same turn differently. Upstream sends
  `missing` and takes the relabelling.
- **The refs are kept forever.** Nothing deletes them, so a long-lived project
  accumulates a ref and a commit per turn, and the objects stay alive against
  `git gc`. Deliberate for v1: a diff a developer can still open tomorrow is the
  point of the feature. `git for-each-ref refs/lightcode` finds them and
  `git update-ref -d` removes one, which is the whole of the cleanup a later
  ticket would add.
- **A project with no repository has no turns to review**, and that is reported
  as nothing rather than as an error. `vcs.init` (ticket 21) is the door out, and
  the first turn after it becomes the new baseline — so a conversation started
  before the repository existed becomes reviewable from that point on rather than
  not at all.
- **A checkpoint must not trigger a working tree refresh.** It writes a ref
  under `.git`, which `crate::git` watches. `refs/lightcode/checkpoints` is
  therefore excluded from what counts as a change, for the same reason
  `.git/objects` is — see ADR-0006, whose sharpest edge this is a second instance
  of.
