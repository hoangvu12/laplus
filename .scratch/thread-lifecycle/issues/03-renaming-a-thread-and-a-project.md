# 03 — Renaming a thread and a project

**What to build:** the developer can rename a thread and rename a project, and
the new title reaches every open view immediately and is still there after a
restart.

Today both rename controls dispatch a command the server refuses, so a
conversation is stuck with the title it was seeded with and a project is stuck
with its folder's name.

Both are cheap for the same reason: the title column already exists on both the
thread and the project, and both titles are already published and already read
back. **Neither needs the lifecycle migration** — this ticket is deliberately
independent of it, and of everything else.

The contract types a title as trimmed and non-empty, so a blank one is refused
rather than stored. A thread nobody can name in a list is not a smaller thing
than a named one; it is a row the developer cannot pick out.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

**This is bigger than a rename control, and it may deserve to come first —
that call is the maintainer's.** Driving ticket 02 in a
real window found that `thread.meta.update` is not only the rename control — the
composer sends it on **every** message whose model or branch differs from the
thread's, from `persistThreadSettingsForNextTurn`, and it sends it _first_. The
send is then guarded by `if (failure === null)`, so the refusal stops the two mode
commands **and the turn itself**. Observed: picking a runtime mode and pressing
enter dispatched exactly one command, `thread.meta.update`, was refused, and the
message was never sent.

So this ticket is not "the rename controls are decoration" — it is a refusal on
the ordinary send path that swallows whatever was queued behind it, and it gates
ticket 02's whole user-visible payoff. See ticket 02's comments for the run.

- [ ] Both commands are parsed before the world is consulted.
- [ ] A blank or whitespace-only title is refused, with a sentence naming the
      problem and the thing it applies to.
- [ ] A blank identifier is refused.
- [ ] An unknown thread or project is refused.
- [ ] Each command answers with the sequence it committed at.
- [ ] A renamed thread publishes on its own feed and on the project list.
- [ ] A renamed project publishes on the project list.
- [ ] A subscriber on a second connection sees both renames.
- [ ] Both titles survive a restart.
- [ ] A fresh subscriber sees the new titles, which is what proves they were
      stored rather than only broadcast.
- [ ] Renaming to the title already held is harmless.
