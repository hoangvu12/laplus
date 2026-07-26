# 25 — Ignore-file semantics for the file tree

**What to build:** A developer opens a JavaScript project and sees their own
source in the file tree, not a thousand packages from `node_modules`. Whatever
the repository already says should be ignored is ignored, so the tree matches
what the developer thinks is in the project.

**Blocked by:** 06 (Filesystem browse and file tree), which is what this
corrects.

**Status:** needs-triage

## Why this exists

Ticket 06 shipped a walk that skips `.git` and nothing else, because
`.gitignore` semantics are a real piece of work and guessing a skip list hides
files silently. That was the right call for one ticket and it leaves a genuine
gap, described in full under "Declared divergence" in
`06-filesystem-browse-file-tree.md`:

The walk is breadth-first and bounded at 25,000 entries. In a JavaScript project
`node_modules` contributes roughly a thousand entries at depth two and tens of
thousands at depth three, so the budget is spent inside it before the walk
reaches depth three anywhere else. The user's own source is present down to
depth two and can be missing below it — in a monorepo, `packages/web/src`
appears in the tree with nothing inside it. The only signal is the "· partial"
badge.

Upstream does not meet this, because its `fff` indexer honours ignore files. So
this is a behaviour gap against the reference server rather than a stylistic
difference.

## What to decide before building

The choice of mechanism is the substance of this ticket, and it is a size
trade-off — which is the project's whole reason for existing:

- **Shell out to `git ls-files --cached --others --exclude-standard`.** Free,
  exactly correct for repositories, honours global excludes and
  `.git/info/exclude` too, and the spec already commits to shelling out to
  `git` (tickets 19–21). Needs a fallback walk for a folder that is not a
  repository, and costs a process spawn per listing.
- **The `ignore` crate.** Correct everywhere, repository or not, and it brings
  symlink-loop handling and parallel walking with it. Roughly doubles the
  dependency graph — `globset`, `regex-automata`, `aho-corasick`, `bstr`.
  Measure the artifact before and after; the spec's target is 20–30 MB.
- **A fixed exclusion list.** Cheapest and a guess. Only worth it if the two
  above are both refused.

## Acceptance criteria

- [ ] A JavaScript project with `node_modules` renders a tree whose own source
      is complete
- [ ] Files the repository ignores do not appear in the tree
- [ ] A folder that is not a repository still lists
- [ ] Whatever mechanism is chosen, the artifact size is measured against the
      spec's 20–30 MB target and recorded
- [ ] Tests drive the ignored-file behaviour through the socket boundary
