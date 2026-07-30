# 01 — Lifecycle fields reach the client as stored state

**What to build:** the six lifecycle fields the contract already declares on a
thread stop being hardcoded nulls and become real, persisted state — archived-at,
settled-override, settled-at, snoozed-until, snoozed-at and deleted-at.

No new commands. Nothing the developer can do changes. What changes is that the
thread read model can now _express_ an archived, settled, snoozed or deleted
conversation, which is the precondition for every ticket that lets one be made.

This is the expand step: the new form is added beside nothing, so no existing
behaviour moves and the suite stays green throughout.

Two things make this a ticket of its own rather than a limb of the archive
ticket. First, the fields gate four separate slices — archive, settle, snooze and
delete — and hanging them off any one of those would draw blocking edges that
misdescribe why the others wait. Second, the fields are emitted twice, on the
thread value and on the thread shell summary the project list carries, and those
two are currently independent literals that must agree forever. Give them one
shared shape so a later field cannot be added to one and forgotten on the other.

The client needs nothing else: the shell summary already carries pending
approvals, pending user input, the latest turn, the session and the latest user
message time, which is everything the bundled runtime's own classification reads
apart from these six.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] The schema gains the six fields through one appended migration entry, at
      the next version. No released migration is rewritten.
- [ ] The fields are nullable with no default, so a thread that has never been
      archived, settled, snoozed or deleted is indistinguishable from a row
      written before the migration existed.
- [ ] A database at the previous schema version opens, migrates, and its existing
      threads read back as never archived, settled, snoozed or deleted.
- [ ] Opening an already-migrated database applies no migration twice — the
      existing store test for this still passes.
- [ ] The thread value and the thread shell summary both carry all six fields,
      sourced from one shared shape rather than two independent literals.
- [ ] The fields survive a restart: a value written directly to the store is
      still there, and still on the wire, after the server is restarted against
      the same database.
- [ ] A fresh subscriber on the project list and on a thread's own feed sees the
      six fields.
- [ ] The whole existing suite is green, on both platforms CI covers.
