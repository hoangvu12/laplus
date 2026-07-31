# 02 — Remove a worktree

**What to build:** `vcs.removeWorktree`, answered rather than refused, so that a
developer deleting a conversation that lives in a worktree can say yes to the
offer to remove the worktree and have it actually happen.

Today that offer is reachable and its yes branch fails: the conversation is
deleted, the worktree stays on disk, and the developer is shown an error after
the fact. This is the one flow in the whole effort with a live UI path.

The method takes a working tree and a path, and removes the checkout at that
path. Without force, git refuses a worktree with uncommitted changes in it and
that refusal reaches the developer intact — laplus does not soften it. With
force, the removal goes ahead, which is what the delete-conversation flow asks
for. The ref the worktree held always survives: removing a checkout is not
deleting a branch. A path that is not a worktree of this repository is refused by
git in git's own words rather than pre-checked, because git's message is better
than the one a pre-check would write.

Follows the shape `crate::refs` established — a read of the payload that yields
either a typed request or a refusal, and a run that takes the shared working-tree
registry — and uses the error union `crate::git` already builds. On the way out
it disturbs the kept working tree, exactly as a switch and an init already do;
see ADR-0006 for why that marks rather than reads. Failing to disturb never
fails a removal that succeeded.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] A worktree named by path is removed, and the folder is gone from disk
- [x] The ref the worktree held is still listed by the branch picker afterwards
- [x] A worktree with uncommitted changes is refused when force is not asked
      for, and the developer sees git's own reason
- [x] The same worktree is removed when force is asked for
- [x] A path that is not a worktree of this repository is refused, and nothing
      on disk is deleted
- [x] A working tree that is not a repository is refused the way the existing
      ref methods refuse it
- [x] The kept working tree is marked stale after a successful removal, so the
      status panel reflects it without a manual refresh
- [x] Tested through the socket against a real repository built with the `git`
      binary, asserting both what happened on disk and what the status panel
      says — nothing reaches into `crate::refs` or `crate::git` directly

**Verification in the running app** (the suite is not evidence the app works):

1. Make a worktree by hand and start a conversation on the ref it holds
2. Delete that conversation and answer yes to the offer to remove the worktree
3. Confirm the worktree folder is gone and no error toast appears
4. Confirm the branch it held is still in the picker

**Outstanding.** The four steps above have not been run. The machine this landed
on is aarch64, where there is no Chrome to drive with: Google publishes Chrome
for Testing for `linux64` only, and the chromium in this Ubuntu's archive is
85 — older than `--headless=new` and older than the bundle's own syntax. So
`server/tools/ui-driver/` cannot be pointed at anything here, and this joins
`capabilities.connectionProbe` on the list of things waiting for a machine with a
browser. It is the piece of this effort most worth driving and it is the piece
that was not driven; nothing below substitutes for it.

**Where it landed.** `refs::RemoveWorktree` — `read` yields the typed request or
a refusal, `run` takes the shared registry, and both use the error union
`crate::git` already builds, which is `crate::refs`'s shape and the spec's
decision. `git worktree remove [--force] -- <path>` is the whole of the work. The
`--` is not decoration: the path is the one argument here that is neither a flag
nor a name this module checked, and the command it is passed to has a `--force`
of its own.

**One thing is checked before git and it is not the path.** An empty path is
refused, for the reason `git::workspace` refuses an empty `cwd` — it is not a
mistyped path, it is the server process's own directory. Whether the path is a
worktree of this repository is git's question: it already refuses, it already
says which path, and a pre-check that got it wrong would be a folder deleted on
this module's authority rather than a worse sentence.

**The disturb is real and the test for it is not.** `Repositories::disturb` is
called on the way out, as the spec asks and as a switch and an init already do.
But the watcher is recursive over a subscribed workspace, so it notices the
folder going on its own — the disturb makes the panel prompt rather than
eventual, and prompt is a claim about elapsed time, which no test in this tree
asserts on. So
`socket_managing_worktrees.rs::the_status_panel_notices_a_removed_worktree_without_being_asked`
asserts the outcome and would pass against a removal that forgot to disturb. Its
doc comment says so. The worktree in that test is nested inside the project on
purpose: git reports a linked worktree under the main one as one untracked
directory, and that is the only layout where a removal is visible in the
_project's_ status at all.

**Nothing disturbs the removed worktree's own entry, deliberately.** The
registry can be keeping a status for the folder that was just deleted — in the
target flow it very likely was, since a conversation lived there. It is left
alone: the spec asks a create to disturb the new worktree's folder and says
nothing about the mirror case, and the mirror case is not the same shape. A read
of a folder that is gone fails, a failed read publishes nothing and logs once,
and the only subscriber it could have had was the pane of a conversation that has
just been deleted. Evicting it would be `git::Repositories` gaining a `forget`
for one caller. Worth revisiting if a second one appears.

**A sixth test for the flow rather than the method.** `useThreadActions.ts`
sends three calls in order — the deletion, the removal with `force: true`, then
`vcs.refreshStatus` — and the failure this ticket is about happened on the
second, after the first had already succeeded. So the sequence is sent in that
order against a real repository. It is not a substitute for step 2 above: it
sends the payloads the client builds, not the clicks that build them.

**No UI change was needed.** The client already calls this method with exactly
this payload, and already refreshes the status after it. What was missing was an
answer.

**ADR-0007 gained an amendment note, because this contradicts a sentence in it.**
Its Consequences said "there is no flag anywhere in `crate::refs` that could
throw work away", and now there is one. The decision it records is untouched and
the sentence's intent survives — it was about a _switch_, and a switch still
cannot be forced — but the claim as written is false, and the note says why the
reasoning still permits the removal: the flag is a payload field defaulting to
false, git refuses without it, and the only caller that sends `true` is a
dialogue the developer has already answered. `CONTEXT.md`'s **Disturb** entry
also named only a switch and an init; it names a removal now. The **Worktree**
glossary entry is ticket 05's and is deliberately not written here.

**The ledger was re-derived rather than adjusted**, per its own instruction: its
own commands were re-run and its headline, its cluster table and its suggested
order now agree with them. The figures are in the ledger and are deliberately not
repeated here. `rpc.rs`'s module doc carried one — "Twenty-five are implemented",
against a real thirty-five — and it was **dropped rather than incremented**,
which is `524d6ec`'s rule ("no file outside the ledger carries a parity figure")
applied to the one file that had been missed.

**The suite.** `cargo test --no-fail-fast` on Linux, twice: 43 binaries green
both times, and
`watcher::tests::a_file_written_outside_the_server_is_reported_relative_to_its_workspace`
red both times, which is the failure the spec names as pre-existing here.

The first run also failed
`socket_terminal::a_flood_of_output_neither_stalls_the_socket_nor_loses_the_terminal`,
which is **not** the second failure the spec names — that is
`a_call_that_names_no_size_does_not_resize_the_terminal`. It passed on its own,
passed three times over in its own binary afterwards, and did not recur in the
second full run. So the PTY timing sensitivity the spec warns about moves between
tests under a loaded parallel run rather than sitting on one, and the spec's
naming of a specific second test is worth less than its warning. Nothing here
touches terminals. Clippy reports the same three warnings as a clean tree.
