# ADR-0003 — Independent conversations are not conflict-free ones

Date: 2026-07-27
Status: Accepted

## Context

Ticket 16 asks that two conversations in the same project "remain independent".
There are two readings of that word and they are different promises:

- **Independent** — nothing on the server crosses. Separate transcripts,
  separate event feeds, separate sessions, separate subprocesses.
- **Conflict-free** — the two agents cannot tread on each other's work on disk.

Upstream delivers the second by giving each thread its own **git worktree**. This
project's spec excludes worktrees by name, and `orchestration.rs` refuses to
prepare one in so many words: a turn whose bootstrap asks for one is declined
with "This server cannot prepare a git worktree for a thread". Without them there
is nowhere else for a second agent to run.

## Decision

**Same-project isolation is server-side only.** Two conversations in one project
run in the *same folder at the same time* and may edit the same files. Last write
wins. That is a limitation of this server, recorded rather than hidden, and it is
not a defect to be fixed inside the current shape — the fix is worktrees, which
the spec rules out.

The ticket's word is *independent*, and that is what is delivered and tested:
`socket_concurrency.rs::two_conversations_in_one_project_stay_independent` drives
two threads in one project through one socket and asserts two children, two
transcripts, two sessions and two event feeds that never carry each other's
events.

## Consequences

- A developer running two conversations against one project can lose work to a
  race between them. Nothing in the UI warns about it, because nothing in the
  contract has a place to say so.
- Do not read "independent" in ticket 16, or in this file's tests, as a claim
  about the filesystem. The test file's header says so where a reader will meet
  it.
- If worktrees ever come into scope, this is the decision they reopen — and the
  refusal in `orchestration.rs` is where the work starts.
