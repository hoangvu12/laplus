# ADR-0006 — A change marks a working tree stale; it does not read it

Date: 2026-07-27
Status: Accepted

## Context

The working tree status is the one thing in the app that goes stale _because_
the agent is working. It is how a developer tells what the agent actually did,
so a status that had to be asked for would be wrong exactly when it mattered —
which is why ticket 19 asks for it to refresh on its own.

The events are already there. `crate::watcher` reports every change under a
watched workspace, and `crate::filesystem::Index` already listens. What was open
is what a status does with one.

Two facts bound the answer.

**A read is expensive and is a child process.** `git status` on a large
repository is tens to hundreds of milliseconds, and there is no non-blocking way
to run it. It cannot happen on the connection's read loop, where it would stall
every other call on the socket, and it cannot happen on a subscription's pump,
where `EventSource::describe` runs on a `tokio` worker.

**Changes arrive in bursts of thousands.** A `cargo build` or an `npm install`
produces thousands of events in seconds, and the file tree's own module already
records what that costs: "asking a background thread to do that over and over
for a workspace nobody is currently searching … is exactly the 'pins a core'
failure."

The file tree solved its version of this by **forgetting**: a change drops the
held scan and the next caller who actually needs one pays for it. That works
because `searchEntries` is a _pull_ — somebody asks, and the answer can be
computed then. A subscription is a _push_, and there is no next caller to hand
the cost to. So the file tree's answer does not transfer, and three were
available.

| Option                                             | Cost                                                                                                                            |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Read on every change                               | Thousands of `git status` runs during any build; a core pinned, and a status per file sent to a panel that renders the last one |
| Forget on change, read when a client asks          | No client asks — the whole point is that nothing asks. A subscription would go silent                                           |
| Mark stale, and let one reader chase the staleness | One more piece of per-repository state                                                                                          |

## Decision

**A change marks the working tree stale and returns. A single refresh thread
per repository reads until the staleness stops arriving**, pausing for a
coalescing window before each read.

Three properties come out of that one sentence, and they are the reason it is
one mechanism rather than three:

- **Coalescing** is the pause. A thousand changes inside one window are one
  read, because marking something that is already stale is free.
- **No lost change** is the re-check. Staleness is cleared _before_ the read
  starts, so anything that arrives during the read marks it again and the thread
  goes round rather than exiting on a status that was already out of date.
- **No pile-up** is the single thread. At most one read per repository is in
  flight, so a repository that takes a second to read cannot accumulate readers
  behind it.

The subscription never runs git at all. It describes itself from the last read
and is fed by the thread — which is what keeps a stream's snapshot off the
critical path of a runtime worker. A subscription opened before any read has
finished describes itself with _nothing_, because an empty status is a claim
that the tree is clean rather than an admission that nothing is known yet.

## Consequences

- **A busy workspace refreshes continuously**, once per window plus the length
  of a read, rather than going quiet until the writing stops. That is the right
  way round here: a status that only appeared once the build finished would be
  useless during the build, which is when the developer is watching it. It is
  also the difference between this and a debounce, which is what a first reading
  of "coalesce rapid changes" suggests.
- **The window is a latency floor.** A developer who saves a file waits the
  window before the status moves. 150 ms is under the threshold where that reads
  as lag, and it is the same order as the composer's own 120 ms debounce.
- **git must not trigger itself.** `git status` opportunistically rewrites
  `.git/index`, which is a path under the watch — a read that caused the next
  read, forever. Every `git` this server runs therefore passes
  `--no-optional-locks`, and `.git/objects`, `.git/logs` and lock files are
  filtered out of what counts as a change. This is the sharpest edge in the
  design and it is invisible until it is a spinning core.
- **A read that fails logs once**, not once per window, or a repository git
  cannot read would fill the log for as long as the developer kept typing.
- **The status a subscriber sees is always whole.** Because a read produces
  every field at once, there is only ever a `snapshot` to send — never the
  contract's `localUpdated` or `remoteUpdated`. See `crate::git` for why the
  reference server needs both and this does not.
