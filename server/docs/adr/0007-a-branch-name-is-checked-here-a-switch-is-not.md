# ADR-0007 — A branch name is checked here; a switch is not

Date: 2026-07-27
Status: Accepted

## Context

Ticket 21 asks for two refusals that look like the same kind of thing and are
not:

- **"An invalid branch name is rejected before it reaches the git binary."**
- **"A switch blocked by uncommitted changes is refused with an explanation,
  not silently or destructively."**

Both are cases where git would also refuse. The question either way is whether
this server does the checking or carries git's answer through — and answering
it the same way twice would get one of them wrong.

Three things separate them.

**Where the input comes from.** A branch name is the one input in this
subsystem that is neither a path nor a fixed flag: it is typed into a text box
and goes straight onto a command line. Whether a switch is safe is not an input
at all — it is a fact about the working tree, which only the working tree knows.

**Whether the answer is knowable without running git.** `git-check-ref-format(1)`
is a closed list of rules about a string, and it does not change between
repositories. Whether a checkout would overwrite something depends on the
index, the two trees and the developer's own `.gitignore`, and reimplementing
that would be reimplementing git.

**Whose vocabulary the sentence should be in.** `git branch` refuses a bad name
with `fatal: 'feature x' is not a valid branch name`, which is git being
generous; `git switch` refuses a dangerous switch with three lines that name
the files in the way and say to commit or stash them, which no sentence written
here could improve on.

## Decision

**A branch name is validated in `crate::refs::check_name` before any git runs.
Everything else git would refuse is git's refusal, carried through with git's
own words.**

Two consequences, and they are the reason it is worth writing down:

- **The rules are duplicated on purpose.** `check_name` is a second copy of
  `git-check-ref-format`, and copies drift. What it buys is that a name the
  developer typed never becomes an argument — `-rf` never reaches a command
  line as a flag, `main@{yesterday}` never reaches one as a revision — and that
  the sentence names a _branch name_ rather than a ref. The drift risk is
  bounded because the rules have not changed in a decade and because a name git
  refuses and this accepts still fails, just later and with a worse message.
- **`crate::git::refusal` keeps whole lines, not the first one.** The status
  read only ever needed the first line, because git's status failures are
  one-liners. A blocked switch is not: its first line names the problem in the
  abstract and its _last_ line is the one worth reading. So the refusal keeps
  every non-empty line of stderr, bounded at 20 lines and 2,000 characters —
  a switch blocked by a thousand files must not put a thousand paths in an
  error frame.

## Consequences

A `GitCommandError` from this subsystem can now carry a multi-line `detail`.
The client composes its own message around it (`Git command failed in
${operation} (${cwd}): ${detail}`), so this is a rendering question and not a
decoding one.

Nothing here passes `--force`, `--discard-changes` or `-B`. That is what makes
"refused, not destructively" a property of the code rather than of the tests:
there is no flag anywhere in `crate::refs` that could throw work away, so the
only way to lose uncommitted changes through this server is for git itself to
decide they were not at risk.

The validator is one of the few places in this repository where a rule is
copied from another program rather than delegated to it. If that copy is ever
suspected of being wrong, `git check-ref-format --branch <name>` is the oracle,
and the test
`a_name_git_would_refuse_is_refused_before_git_sees_it` is the list to compare
against.
