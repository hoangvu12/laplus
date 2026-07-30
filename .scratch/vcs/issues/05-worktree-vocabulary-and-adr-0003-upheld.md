# 05 — Worktree vocabulary, and ADR-0003 upheld

**What to build:** The written half of this effort, so that the next reader does
not spend an afternoon re-deciding what was already decided on evidence.

**`CONTEXT.md` gains a worktree vocabulary.** It has none today, even though
`crate::refs` has reported each ref's worktree since it shipped, and even though
the **Current** entry already carries the distinction the word needs — the same
branch is current in one worktree and merely _checked out_ in another, and only
one of those can be switched away from. A **Worktree** entry goes under Refs,
defined against that, plus a **Managed worktree** note for the ones laplus made
and where it puts them. The glossary gets the words only; implementation detail
stays in the spec.

**ADR-0003 gains a status note.** Its last line says that if worktrees ever come
into scope, it is the decision they reopen and the refusal in the turn path is
where the work starts. They came into scope, it was reopened, and it is
**upheld** — so it says so rather than being left to read as untouched. The note
records three things:

- worktrees were reconsidered on the evidence in `.scratch/vcs/`
- these methods let a developer manage worktrees; they do not give a thread one,
  the turn bootstrap still refuses to prepare one, and same-project isolation is
  still server-side only
- the missing prerequisite is **project setup scripts** — a fresh worktree holds
  tracked files only, so without a script that runs on creation it starts with no
  untracked files and nothing that builds. That is a file format, a loader and a
  runner, and it is its own effort

ADR-0031's treatment of ADR-0020 is the shape to copy — this is a note on an
upheld decision, not a supersession, and ADR-0003 stays Accepted.

**Blocked by:** 02 (Remove a worktree), 03 (Create a worktree). Neither gates it
technically — the decision was made before either was written — but both the
vocabulary and the note read as records of what shipped, so they are written once
it has.

**Status:** ready-for-agent

- [ ] `CONTEXT.md` has a **Worktree** entry under Refs, defined against the
      existing **Current** entry rather than restating it
- [ ] `CONTEXT.md` has a **Managed worktree** note naming where laplus puts the
      ones it makes
- [ ] Neither entry carries implementation detail that the spec already holds
- [ ] ADR-0003 carries a note that it was reopened, examined and upheld, and its
      status stays Accepted
- [ ] The note names project setup scripts as the prerequisite, and says plainly
      what goes wrong without them
- [ ] The note does not restate the parity figure — no file outside the ledger
      carries one
- [ ] A reader arriving at ADR-0003's last line is sent to `.scratch/vcs/`
      rather than left to re-derive the decision
