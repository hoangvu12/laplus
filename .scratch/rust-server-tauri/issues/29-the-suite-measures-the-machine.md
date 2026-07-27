# 29 — Two tests measure the machine rather than the code

**What to build:** a suite that says the same thing on a busy laptop as on an
idle one.

Two assertions in this repo are wall-clock deadlines with no clock injected, so
what they actually test is how loaded the machine is. Both went red during ticket
28's review pass on a box that was also running a game, and both pass alone.

**Status:** done

**Found by:** ticket 28, while trying to establish that a fix had not broken
anything. That is the expensive part: a suite that fails for reasons unrelated to
the diff costs the thing it exists to give, which is a trustworthy answer to "did
I break this". It took three runs and a process census to conclude "no".

## The two

### `provider::tests::a_binary_that_does_not_answer_is_given_up_on`

`crates/lightcode-server/src/provider.rs`. Gives `probe` a patience of 50ms
against a fake that dawdles for a second, then asserts:

```rust
assert!(started.elapsed() < Duration::from_millis(900), ...)
```

Measured **1.279s** under load. The property is real and worth keeping — a binary
that never answers must not hold the thread that asked — but `900ms` is a proxy
for "much less than the second the fake sleeps", and the gap between 50ms and
900ms is entirely process-spawn time, which is exactly what a busy machine
stretches.

### The git-heavy socket binaries

`socket_diffs` went **0 of 12** in parallel and **12 of 12** single-threaded, in
78.9s. `socket_branches` and `socket_git` failed partially the same way. All of
them fail as `a frame arrives within the timeout` — the harness's `READ_TIMEOUT`,
five seconds, in `tests/harness/mod.rs`.

Nothing is wrong with the server. These tests each drive real `git` in a real
temporary workspace, a dozen at a time, and five seconds of wall clock is a
generous budget right up until it isn't. Note the shape: **the failure is a
timeout on the socket**, so it reads like a server that stopped answering rather
than like a machine that ran out of room.

## Why it is one ticket

Because it is one decision: whether a test in this repo is allowed to assert on
elapsed real time at all, and what it should do instead. The two cases want
different answers — one is a deadline the code owns, the other is a budget the
harness owns — but answering them apart is how a suite ends up with two
conventions.

Worth settling first: **is the second one even a test problem?** A five-second
read timeout is also what a developer waits, and a suite that takes 79 seconds
serially where it takes three in parallel is not obviously the version to keep.
`--test-threads` is a real answer, and so is leaving it alone and knowing.

## What is not the answer

Raising `900ms` and `READ_TIMEOUT` until this machine passes. That trades a test
that fails when it should not for one that passes when it should not, and the
next person meets it on a slower box with less to go on.

## Comments

### The bit that will waste someone's afternoon

`cargo test` **fails fast**: the lib tests failing on the provider deadline meant
no integration binary ran at all, and the summary line said `396 passed` — which
looks like a suite that shrank by two hundred tests. `--no-fail-fast` is what
shows the real picture.

Separately, and not this repo's fault: piping `cargo test` into `head` kills
cargo mid-run and orphans the `git` children it had spawned, which then slow the
*next* run. Several of the confusing failures above were self-inflicted that way.
Redirect to a file and grep the file.

### Seen again, on a diff that touches none of it

Ticket 24 hit this and paid the tax the ticket above predicts. Its changes are a
new `xtask` crate, a licence file and two lines of bundle configuration — nothing
the server can see — and `socket_branches` came back **6 of 19 failed**, all
`a frame arrives within the timeout`. Re-run alone: **4 of 19 failed**, a
different four. Re-run with `--test-threads=1`: **19 of 19 passed** in 55.98s.

The load was real and self-inflicted in a way worth naming, because ticket 24's
work makes it likelier rather than rarer: `cargo tauri build` had been run four
times in that session, each a three-minute release build of a thousand Tauri
crates. Anyone measuring the artifact is by definition on a machine that has just
been compiling hard, so the ticket whose job is to build releases is the ticket
most likely to meet this — and to spend its time deciding whether it broke the
git suite.

Which is the cost this ticket is about, quantified once more: three runs to
conclude "no".

### 2026-07-27 — triage. The convention, and why the two halves differ

Taken first of the four open tickets, because it is the only one whose cost is
paid by every *other* ticket. Two have already paid it — 28 spent three runs and
a process census, and 24 met it on a diff the server cannot see.

**The convention: a test may not assert on elapsed wall-clock time as a proxy for
a property.** If the property is "this gives up rather than waiting", assert that
it *gave up* — not that it did so within N milliseconds. The two cases below are
the same rule applied to a deadline the code owns and a budget the harness owns,
which is why they are one ticket and not two.

#### The provider deadline: assert the decision, not the duration

Replace the `elapsed() < 900ms` assertion with one on `probe`'s **outcome** — that
it returned the give-up result rather than the fake's eventual answer. Have the
fake block until the test releases it (or sleep long enough to be unmistakable)
so that the two outcomes are genuinely distinguishable by value alone.

This is strictly stronger than what is there now. Today a `probe` that waited the
full second and *then* returned the give-up result would still pass on an idle
machine; under the change it cannot. And a `probe` that wrongly waits for the
binary no longer fails on a threshold — the test hangs, and the harness's timeout
catches it. A hang is a better failure than a flaky number: it says "this never
gave up", which is the actual bug.

#### The git suite: `READ_TIMEOUT` is a hang detector, not a budget

This one is not a test problem, and answering it as one is how the wrong fix gets
made. Nothing is wrong with the server; a dozen real `git` invocations in real
temporary workspaces genuinely take longer than five seconds on a loaded box.

So change what `READ_TIMEOUT` *is*. Its only job is to stop a wedged test hanging
the suite forever, which means it should be generously large — the cost of a big
timeout is paid only when something is actually broken, and the cost of a small
one is paid on every busy machine, forever.

**This deliberately brushes against "what is not the answer" above, so be precise
about the difference.** That section forbids raising the number until this machine
passes, and it is right. The change proposed here is not a bigger budget; it is
the abandonment of the claim that this is a budget at all. A five-second timeout
that fails as `a frame arrives within the timeout` reads like a server that
stopped answering — the ticket says so — and that misdirection is the real defect,
not the duration. A sixty-second hang detector is never mistaken for a
performance assertion by the person reading the failure.

If the suite is still uncomfortably slow after that, `--test-threads` on the
git-heavy binaries is the lever, and it should be set in the repo with a comment
rather than remembered by each person who gets bitten.

#### Also worth folding in

The `## Comments` above contain two findings that cost real time and currently
live only in this ticket: that `cargo test` fails fast — so a lib-test failure
means no integration binary runs at all, and the summary reads like a suite that
lost two hundred tests — and that piping `cargo test` into `head` orphans `git`
children that then slow the next run. Both belong somewhere a person looks
*before* their afternoon disappears, not in a closed ticket. Put them in the
contributing notes or beside the harness.

**Status set to `ready-for-agent` on the strength of this comment.** The
implementation is mechanical once the convention is settled; the convention above
is this triage pass's proposal, not a maintainer ruling, and overruling either
half is one sentence.

### 2026-07-27 — done. Both halves, and the second one proven by experiment

`cargo test -p lightcode-server --no-fail-fast`: **649 passed, 0 failed.**
`cargo clippy --all-targets`: no warnings.

#### The provider deadline was simpler than the proposal above, and worth saying why

The triage comment proposed asserting on the outcome instead of the duration.
Reading `probe` first turned up something stronger: **that assertion was already
there, and the wall-clock one was strictly redundant.**

`Probed::TimedOut` is constructed in exactly one place — the deadline arm of the
poll loop. Every other exit from that loop `break`s and goes on to read the
child's output, returning `Version` / `Unreadable` / `Failed`. So a probe that
waited for the child *cannot* return `TimedOut`, and `assert_eq!(…, TimedOut)`
was already the full proof that the deadline ended the wait. The
`elapsed() < 900ms` line could not fail without the assert_eq failing first. It
contributed nothing but a measurement of process-spawn time on the machine
running it — which is precisely why it went red under load against correct code.

Deleted rather than replaced. No clock injected, no seam added, nothing to
maintain.

**One thing was genuinely weak, and is now fixed.** `Fake::dawdling` printed
nothing at all, so had the probe wrongly waited for the child it would have
returned `Unreadable { output: "" }` — still not `TimedOut`, so the test was
correct, but only by accident, and a fake that can never answer makes "timed out"
un-meaningful. It now prints `9.9.9 (Claude Code)` after its sleep, and the test
asserts **both** directions: `TimedOut` at 50ms of patience, and
`Version("9.9.9")` at `PROBE_TIMEOUT`. The second assertion is what stops the
first from being vacuous — the same reasoning as ticket 03's
`the_comparison_catches_each_kind_of_drift`.

**It costs about a second**, because the fake really does sleep. That is the one
place this ticket made the suite slower, deliberately, and it buys the difference
between "gave up early" and "never got a version".

#### The harness half: 5s → 60s, and a controlled experiment rather than a hope

`READ_TIMEOUT` is now 60 seconds and its doc comment says what it is — a hang
detector, not a budget — and says `--test-threads` is the lever if the suite is
slow. The two `expect` messages that caused the misdirection are reworded:
`a frame arrives within the timeout` → `no frame within READ_TIMEOUT — wedged,
not merely slow`.

A green suite on an idle machine proves nothing here; the old value passed on an
idle machine too. So the load was reproduced deliberately: **24 busy-loop
processes on 16 cores**, with `socket_diffs`, `socket_branches` and `socket_git`
run concurrently against each other. Same load, same binaries, same machine, one
constant changed:

| `READ_TIMEOUT` | socket_diffs | socket_branches | socket_git | total |
|---|---|---|---|---|
| **60s** | 12 / 12 | 19 / 19 | 14 / 14 | **45 pass, 0 fail** |
| 5s (control) | 1 / 12 | 13 / 19 | 6 / 14 | **20 pass, 25 fail** |

Every control failure was `no frame within READ_TIMEOUT`. That reproduces this
ticket's original report almost exactly — it recorded `socket_diffs` at 0 of 12 —
and it is the part that makes this more than a number being raised: the failures
are demonstrably caused by the constant, and demonstrably not by the server.

#### Being honest about what 60 seconds is

It is **not** derived from a worst-case read measurement. It is chosen to sit far
above any plausible one, and the experiment shows it is sufficient for this load
on this machine — not that it is sufficient universally. The trade it accepts is
stated in the doc comment: a genuinely wedged read now takes a minute to surface
instead of five seconds. That is the right way round, since a wedge is rare and a
busy machine is not, but it is a real cost and not a free win.

If this ever fails again, the answer is `--test-threads`, not a larger number.
The ticket's own "what is not the answer" section still governs.

#### The two findings that were trapped in this file

`CLAUDE.md` now has a **Running the tests** section carrying them, because both
cost an afternoon and neither was anywhere a person looks first: `cargo test`
fails fast, so one failing lib test means no integration binary runs and the
summary reads like a suite that lost two hundred tests; and piping `cargo test`
into `head` orphans the `git` children it spawned, which then slow the next run.
The no-wall-clock-assertions convention is recorded there too, which is the part
that has to outlive this ticket.
