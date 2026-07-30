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

**Status:** done

**Left open:** the window was never driven. The spec makes that a completion
condition — "the work is not done until the window has been driven" — and this
ticket exists because driving found what a green suite could not. It was skipped at
the requester's instruction, so every line below is evidence from the socket and
none of it is evidence from a browser. See the last section.

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

- [x] Both commands are parsed before the world is consulted.
- [x] A blank or whitespace-only title is refused, with a sentence naming the
      problem and the thing it applies to.
- [x] A blank identifier is refused.
- [x] An unknown thread or project is refused.
- [x] Each command answers with the sequence it committed at.
- [x] A renamed thread publishes on its own feed and on the project list.
- [x] A renamed project publishes on the project list.
- [x] A subscriber on a second connection sees both renames.
- [x] Both titles survive a restart.
- [x] A fresh subscriber sees the new titles, which is what proves they were
      stored rather than only broadcast.
- [x] Renaming to the title already held is harmless.

## Comments

`thread.meta.update` and `project.meta.update`, at the three seams the spec
assigns: payload validation in place in `orchestration.rs`, the appended store
method's own tests in `store.rs`, and `tests/socket_renaming.rs` for the wire —
the sequence, both feeds, a second connection, a fresh subscriber and a restart.

**`thread.meta.update` carries four fields, not one.** Title, model selection,
branch and worktree path are all already columns on the thread and already
published, so this needed no migration and no new read-model shape. Each field is
applied only if it arrived, and the event names only the fields that did — which
is the client's own reducer mirrored, and is what keeps a title-only rename from
republishing a branch nobody touched.

The three-state field is the one thing here that is not obvious. `null` and
_absent_ mean opposite things on this command — "clear it" against "leave it
alone" — and serde reads both as `None`, so `threads::Given` is a
`Option<Option<T>>` with a deserializer that makes the outer layer mean presence.
The composer relies on the distinction directly: it moves a conversation onto a
branch by sending `{branch, worktreePath: null}`.

**A project rename is the title and nothing else.** `project.meta.update` also
declares `workspaceRoot`, `defaultModelSelection` and `scripts`; this registry
stores none of them, and each is refused by name rather than accepted and dropped.
That matters for one of them in practice — the script editor sends
`{projectId, scripts}` — where answering with a sequence would tell the developer
their script was saved and leave them to find out at the next restart that it was
not. A refusal is the same outcome they got before this ticket, with a sentence
saying why.

A command that asks for **nothing** is refused too. It would otherwise publish a
`thread.meta-updated` carrying nothing but a new `updatedAt`, which the client
folds as an update — so a payload that asked for no change would move the
conversation in a list ordered by when things changed.

**Four refusals here are not in the spec's invariants table**, which gives these
two commands only "the subject is unknown; the new title is blank": the three
unstored project fields, a payload asking for nothing, a blank `branch` or
`worktreePath`, and a `modelSelection` that is not an object. The table is the
_world's_ refusals; the spec assigns payload validation to its own seam and asks
that seam to cover "a blank title" among others, so these belong to it. The widest
of them is the project one — `ProjectMetaUpdateCommand` permits a title alongside
the other three fields and this server now refuses that combination — and it is
the one to revisit if a client is ever written that sends them together.

`expectedBranch` is read by nothing. It is a compare-and-swap on the branch that
no call site in `apps/web` or `packages/client-runtime` builds, so honouring it
would mean inventing the semantics of a guard no client asks for; the decision to
ignore it is recorded on `UpdateThreadMetaPayload`.

**Not driven in a window.** The spec asks for it and ticket 02's run is the reason
to — this ticket exists because driving found what a green suite could not. It was
skipped here at the requester's instruction, so "the composer sends this first and
its refusal swallowed the message" is still ticket 02's observation rather than one
re-confirmed against the fixed server. `tests/socket_renaming.rs`'s
`the_payload_the_composer_sends_before_a_message_is_answered` drives that exact
payload over the socket, which is the claim about the _server_ half of it.
