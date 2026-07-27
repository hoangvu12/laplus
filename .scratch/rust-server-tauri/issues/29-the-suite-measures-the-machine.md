# 29 — Two tests measure the machine rather than the code

**What to build:** a suite that says the same thing on a busy laptop as on an
idle one.

Two assertions in this repo are wall-clock deadlines with no clock injected, so
what they actually test is how loaded the machine is. Both went red during ticket
28's review pass on a box that was also running a game, and both pass alone.

**Status:** needs-triage

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
