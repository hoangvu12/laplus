# 01 — A conversation in a worktree runs in that worktree

**What to build:** A developer who points a conversation at a worktree — by
picking a ref that is current in one — gets a conversation whose agent edits that
worktree, whose diff panel shows the changes the agent actually made, and whose
revert puts back the tree the agent changed.

Today those three disagree. The agent runs in the project's own folder while the
checkpoints, the diff panel and a revert all read and write the worktree, so the
developer reviews a tree the agent never touched and a revert writes over a
different checkout. Nothing warns them.

There is one rule for where a conversation's work happens — the worktree when it
has one, the project's folder otherwise — and after this ticket it is stated once
and used by both the turn path and the review path, rather than being written in
one and described in a comment on the other. The comment in the review path that
already claims the two agree becomes true.

This needs none of the three new methods. A worktree made by hand at a terminal
is enough to reach it.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] A conversation carrying a worktree path runs its agent in that worktree
- [x] A conversation carrying no worktree path runs its agent in the project's
      folder, exactly as before
- [x] The checkpoint taken for a turn of a conversation in a worktree records
      that same tree, so the turn's diff shows the agent's own changes
- [x] A revert on a conversation in a worktree restores the tree the agent
      edited
- [x] A terminal opened for a conversation in a worktree lands in the same
      folder the agent is working in
- [x] The rule is expressed once; the review path and the turn path both read it
      rather than each deciding for themselves
- [x] A test drives a conversation with a worktree path through a turn and
      asserts both halves — the agent's edits land in the worktree, and the
      checkpoint recorded for that turn is of the worktree. The absence of a
      test like this is why the two halves drifted, so it is the test that keeps
      them together
- [x] Existing coverage for conversations without a worktree still passes
      unchanged

**Verification in the running app** (borrowed from upstream's plan format; the
suite is not evidence the app works, and `server/tools/ui-driver/` is the other
half):

1. Make a worktree by hand for a project laplus knows about
2. Start a conversation and pick the ref that is current in that worktree
3. Send a turn that edits a file, and confirm the edit landed in the worktree
   rather than the project folder
4. Open the diff panel for that turn and confirm it shows the edit
5. Revert the turn and confirm the worktree went back, and the project folder
   was never touched

**Where it landed.** `orchestration::where_the_work_happens(&thread, &project)`
is the rule. `Shell::reviewing` and `Shell::start_turn` both call it;
`revert_checkpoint` and `checkpoints::Diff` read the same `Reviewing`, and the
turn's own checkpoint capture reads `Start::workspace_root`, so all four views of
a turn resolve one folder. The change to the turn path is one argument — it
passed `&project.workspace_root` unconditionally — and the comment on the review
path that claimed the two agreed is now a call rather than a claim. It is not
called `working_folder`: `CONTEXT.md` already has a **Working tree** entry
meaning something else, and ticket 05 is where the glossary gains **Worktree**.

**A correction to this ticket's own problem statement, and to the spec's.** Both
say the checkpoints and the diff panel "read and write the worktree" alongside
the revert. Only the revert did. Checkpoints are _captured_ on the turn path,
from `Start::workspace_root` — the project's folder, the same side as the agent —
and a patch is `git diff` between two refs, which never reads a tree. Refs are
shared with a linked worktree, so a patch run in the worktree resolved
checkpoints captured from the project's folder and showed the agent's own changes
after all: **the diff panel was right by accident.** `checkpoints::restore` does
write a tree, and it wrote the project's recorded tree into the worktree, over a
checkout the agent had never touched. That is the damage, and it is the one thing
here that could have cost a developer work. Verified by experiment (`git diff`
between two refs written from the main tree resolves and runs inside a linked
worktree) rather than assumed, because the doc comments now assert it.

`crates/laplus-server/tests/socket_worktrees.rs` is the test that keeps the two
paths together: a real `git worktree add`, a real turn that edits two files, and
both folders asserted of one turn — the agent's marker and edits in the worktree,
the project's own folder untouched, and the turn diff read back over the wire.
The diff assertion is evidence only in company: it would pass against the old
server too, and what makes it mean "the checkpoint is of the worktree" is the
assertion beside it that the project's folder never changed. A second test
reverts and asserts both trees. Against the old turn path both tests fail, at the
first folder assertion and at the project-was-written-to assertion respectively.

The harness grew `Workspace::worktree` (a second checkout in a temporary
directory of its own) and `create_thread_at` / `open_conversation_at` (a thread
the composer pointed somewhere). Three helpers this file wanted already existed
as copies in `socket_revert.rs` and `socket_diffs.rs`, so rather than write a
third they moved into `harness::conversation` — `revert_checkpoint`,
`SocketClient::events_through_the_revert` and `SocketClient::turn_diff` — and
both existing files now call them. `harness/mod.rs` says the harness is where
plumbing lives; a third copy is where that stops being true.

**The terminal needed no server change.** A terminal's `cwd` comes from the
client, and the client already resolves it with `projectScriptCwd` in
`packages/shared/src/projectScripts.ts` — worktree first, project's folder
otherwise, with its own test. What was untrue was the _other_ half of that
sentence: the folder the agent was working in. Fixing the turn path is what makes
"the same folder" true, so this is the criterion the client had already met and
the server had not.

**No test was added for the no-worktree branch**, because the criterion asks that
existing coverage still pass rather than that new coverage exist:
`socket_turn.rs::the_agent_runs_in_the_projects_directory` is that test and it is
untouched.

**Not touched, deliberately.** No validation was added to `worktreePath` on the
way in. A path that is not there fails where it is used, by name, the way any
other bad working directory does — and a pre-check would be a second opinion
about a folder that can go away between the check and the turn. One consequence
is now written down on `cleared_or_named`: its blank-string rule runs on
`thread.meta.update` only, so a `thread.create` carrying `worktreePath: ""` gives
a conversation whose turns refuse rather than one that quietly runs in the
project's folder. No client sends it and the contract forbids it; the refusal is
the right failure, and it is named rather than left to be rediscovered. The
vocabulary and the ADR-0003 note the spec asks for are ticket 05's.

**Suite.** `cargo test -p laplus-server --no-fail-fast` on Linux: 737 lib tests
and every integration binary, with only the two failures the spec names as
pre-existing on this platform —
`watcher::tests::a_file_written_outside_the_server_is_reported_relative_to_its_workspace`
and `a_call_that_names_no_size_does_not_resize_the_terminal`. Both fail on a
clean tree; neither is touched by anything here. (The terminal one is flaky
rather than reliably red — it passed on one of two runs.)

**The running-app verification above was not performed.** The five steps need a
laplus window and a real `claude` binary, and this work was done on a headless
box with neither. The suite is not evidence the application works, so this is an
outstanding item rather than a covered one — it belongs with the
`capabilities.connectionProbe` drive the spec already lists as owed.
