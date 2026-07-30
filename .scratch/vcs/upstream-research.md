# Upstream research — `vcs.pull`, `vcs.createWorktree`, `vcs.removeWorktree`

Written 2026-07-30, before any spec. The question this answers: **how does
`pingdotgg/t3code` implement the three methods laplus declares and refuses, and
what would laplus have to build to match?**

Read against a shallow clone at `/tmp/t3code` (`git clone --depth 1`). There is
still no `upstream` remote and no checkout in this tree — see `server/CLAUDE.md`.
Line numbers below are that clone's; re-clone before trusting them, upstream
moves.

## Summary

Upstream does **not** treat these as three peer RPC methods. `createWorktree` is
a shared primitive with two callers — the RPC handler and the turn bootstrap —
and the second is what gives a thread its own checkout. Answering the RPC
methods and giving threads worktrees are the same ~30 lines of git plus a
different caller.

Two facts that bear on scope more than the method bodies do:

1. **A fresh worktree is empty of untracked files**, so upstream runs a
   project setup script into it immediately. laplus has no project scripts at
   all, and that is the real prerequisite for per-thread worktrees here.
2. **laplus already reads a thread's `worktreePath` in one place and ignores it
   in another**, and the two disagree. That is a live inconsistency independent
   of which scope is chosen. See "What laplus already has", below.

## `vcs.pull`

`GitVcsDriverCore.ts:1995-2043`, as `pullCurrentBranch(cwd)`. The whole method:

1. Read status details. `refName` is the current branch.
2. No branch → `GitCommandError`, detail `"Cannot pull from detached HEAD."`
3. No upstream → `GitCommandError`, detail
   `"Current branch has no upstream configured. Push with upstream first."`
4. `git rev-parse HEAD` → `beforeSha`.
5. `git pull --ff-only`, 30s timeout, fallback detail `"git pull failed"`.
6. `git rev-parse HEAD` → `afterSha`.
7. Re-read status. Return
   `{ status: beforeSha === afterSha ? "skipped_up_to_date" : "pulled", refName, upstreamRef }`.

`upstreamRef` comes from the **refreshed** status, not the one read at step 1.

The RPC handler (`ws.ts:1747-1755`) is that call plus a status refresh of the
same `cwd`, and the refresh failing does not fail the pull
(`Effect.ignore({ log: true })`).

**`--ff-only` is the whole safety story.** There is no stash, no rebase, no merge
strategy, no dirty-tree check. A pull that would need a merge fails as a
`GitCommandError` carrying git's own stderr.

**Upstream has no test for it.** `GitVcsDriverCore.test.ts` does not mention
`pull`. laplus's discipline is stricter than that, so the spec writes its own.

## `vcs.createWorktree`

`GitVcsDriverCore.ts:2579-2613`. In full:

- `targetBranch = newRefName ?? refName`
- `sanitizedBranch = targetBranch.replace(/\//g, "-")`
- `worktreePath = path ?? join(worktreesDir, basename(cwd), sanitizedBranch)`
- `git worktree add -b <newRefName> <worktreePath> <refName>` when `newRefName`
  is given, `git worktree add <worktreePath> <refName>` when it is not
- when **both** `newRefName` and `baseRefName` are given, additionally
  `git config branch.<newRefName>.gh-merge-base <baseBranch>`, where `baseBranch`
  is `baseRefName` with a recognised remote prefix stripped
- returns `{ worktree: { path: worktreePath, refName: targetBranch } }`

### The three ref inputs are two legal shapes

The handoff asked which combinations are legal. Upstream's answer:

| Given                    | Means                                                |
| ------------------------ | ---------------------------------------------------- |
| `refName` alone          | check the existing ref out into a new worktree       |
| `refName` + `newRefName` | create `newRefName` at `refName`, check it out there |
| `+ baseRefName`          | only ever written to git config as a merge-base hint |

`baseRefName` is **not** a third checkout input. It is metadata for PR
merge-base resolution, and it is ignored entirely without `newRefName`. So there
are two arms, not eight.

### Where a worktree lives

`worktreesDir` is `join(baseDir, "worktrees")` — `config.ts:120`, created
eagerly by `ensureServerDirectories` at `config.ts:145`. Not beside the checkout.
laplus's equivalent of `baseDir` is `~/.laplus/`, so the matching answer is
`~/.laplus/worktrees/<repo>/<branch-with-slashes-flattened>`.

Note that upstream's own tests never exercise the `path: null` default — both
worktree tests pass an explicit path (`GitVcsDriverCore.test.ts:1229`, `:1396`).
The layout above is read from the implementation, not from a test that pins it.

## `vcs.removeWorktree`

`GitVcsDriverCore.ts:2708-2721`. `git worktree remove [--force] <path>`, 15s
timeout, fallback detail `"git worktree remove failed"`. That is the entire
method — no branch deletion, no `git worktree prune`, no check that the path
belongs to this repository. Removing a path that is not a worktree fails with
git's own stderr (`GitVcsDriverCore.test.ts:733-738`).

## The bootstrap arm — what makes a thread's worktree

`ws.ts:901-931`. When `bootstrap.prepareWorktree` is present:

1. If `startFromOrigin`, `fetchRemote({ remoteName: "origin" })` then
   `resolveRemoteTrackingCommit` — and the base becomes that **commit sha**.
2. `createWorktree({ cwd: projectCwd, refName: base, newRefName: branch, baseRefName: baseBranch, path: null })`
   — the same function the RPC method calls, with `path: null` so the server
   picks the location.
3. `thread.meta.update` with `{ branch, worktreePath }`.
4. Refresh git status for the new worktree path.

Then `runSetupProgram()` (`ws.ts:845-881`), covered below.

The client half already exists in this repository, unchanged from upstream:
`ChatView.tsx:4627-4634` sends `prepareWorktree` with
`{ projectCwd, baseBranch, branch: buildTemporaryWorktreeBranchName(randomHex), startFromOrigin? }`
and `runSetupScript: true`. `orchestration.rs:1231-1236` refuses the whole turn
when it sees it.

## The setup script, and why it is the real prerequisite

A git worktree contains tracked files only. No `node_modules`, no `.env`, no
`target/`, no build output. Upstream closes that with a **project script marked
to run on worktree creation**:

- `packages/contracts/src/t3ProjectFile.ts` (88 lines) — a checked-in `t3.json`
  at the workspace root, with a `scripts` array. Each script has
  `name`, `command`, and optionally `runOnWorktreeCreate: boolean`, described as
  _"When true, the script runs automatically after a worktree is created for a
  new thread."_
- `apps/server/src/project/T3ProjectFileLoader.ts` (108 lines) — reads it.
- `apps/server/src/project/ProjectSetupScriptRunner.ts` (188 lines) — picks the
  flagged script and runs it in a terminal whose cwd is the new worktree
  (`:134-146`), recording started/failed on the thread.

**laplus has none of this.** `projects.rs:56` and `:305` answer `"scripts": []`
unconditionally, and the doc comment at `projects.rs:19` records scripts as
_"not in v1's scope at all; the contract requires the key"_. The client side is
partly here — `apps/web/src/projectScripts.ts:14` already knows
`runOnWorktreeCreate` — so this is a server gap, not a contract gap.

Consequence for this repository specifically: a laplus-created worktree of
`laplus` would have no `node_modules` and no `apps/web/dist`, so nothing in it
builds until `pnpm install` is run by hand.

## What laplus already has

- `git.rs` (1797 lines) — runs git, builds `GitCommandError` (`ERROR` at
  `:108`), owns the `Repositories` registry and the status watcher.
- `refs.rs` (1461 lines) — the model to copy. Each method is a `read(payload)` /
  `run(repositories)` pair (`:145`, `:538`, `:623`, `:739`), same error union,
  same `cwd`-rooted payload.
- `refs.rs:483` — `worktrees()` already shells `git worktree list --porcelain`
  and reports each ref's `worktreePath` through `vcs.listRefs`. laplus can
  already **see** worktrees; it just cannot make or remove them.
- `Repositories` is in `rpc::Services`, so a new method gets it for free.

### Two gaps found while measuring

**`git.rs` throws away the upstream ref name.** `git.rs:1017` matches
`branch.upstream` and records only that an upstream _exists_:

```rust
Some(("branch.upstream", _)) => {
    read.upstream = Some(read.upstream.unwrap_or_default())
}
```

`VcsPullResult.upstreamRef` (`packages/contracts/src/git.ts:177`) needs that
string. So `vcs.pull` cannot be written without first keeping the value the
porcelain already hands over. Small, but it is in the parser, not in the new
method.

**A thread's `worktreePath` is honoured for diffs and ignored for the agent.**
`orchestration.rs:404-420` (`reviewing`) resolves a checkpoint's workspace root
as `thread.worktree_path.unwrap_or(project.workspace_root)`, and its comment at
`:409` claims this is _"the same rule `crate::turn::starting` follows for where
the agent runs"_. It is not. The only call to `turn::starting`
(`orchestration.rs:1305-1309`) passes `&project.workspace_root` flat, with no
reference to the thread's worktree path.

So a thread that holds a `worktreePath` today runs its agent in the project root
while its checkpoint diffs are read from the worktree. Upstream resolves both
from the same expression (`ws.ts:1724`). Nothing in `tests/` covers a turn in a
worktree, which is why the two drifted.

This is reachable without any new method: `BranchToolbar.logic.ts:186-189` sets a
draft's `worktreePath` when the developer picks a branch that already lives in a
worktree, and `refs.rs` reports exactly those. A hand-made `git worktree add` is
enough to get there.

## What each scope would cost here

**The three methods only.** `pull` is the `git.rs` upstream-name fix plus a
`read`/`run` pair following `refs.rs`. `createWorktree` and `removeWorktree` are
two more pairs over `git worktree add` / `git worktree remove`, plus a decision
to write down: `~/.laplus/worktrees/<repo>/<branch>` for `path: null`. Closes the
`useThreadActions.ts:358` delete-thread hole. Leaves `orchestration.rs:1231`
refusing, so the composer's worktree mode still fails.

**Per-thread worktrees.** The above, plus the `ws.ts:901-931` arm — which needs
`fetchRemote` and `resolveRemoteTrackingCommit` for `startFromOrigin`, neither of
which laplus has — plus fixing the `turn::starting` workspace root above, plus
superseding ADR-0003. Without project scripts, every worktree it makes starts
unbuildable.

## Sources

All under `/tmp/t3code` unless marked.

| What                   | Where                                                                |
| ---------------------- | -------------------------------------------------------------------- |
| `pullCurrentBranch`    | `apps/server/src/vcs/GitVcsDriverCore.ts:1995-2043`                  |
| `createWorktree`       | `apps/server/src/vcs/GitVcsDriverCore.ts:2579-2613`                  |
| `removeWorktree`       | `apps/server/src/vcs/GitVcsDriverCore.ts:2708-2721`                  |
| RPC handlers           | `apps/server/src/ws.ts:1747-1755`, `:1802-1812`                      |
| Bootstrap worktree arm | `apps/server/src/ws.ts:901-931`                                      |
| Setup script arm       | `apps/server/src/ws.ts:845-881`                                      |
| Worktree location      | `apps/server/src/config.ts:120`, `:145`                              |
| Thread cwd rule        | `apps/server/src/ws.ts:1724`                                         |
| Project file schema    | `packages/contracts/src/t3ProjectFile.ts`                            |
| Setup script runner    | `apps/server/src/project/ProjectSetupScriptRunner.ts`                |
| Worktree tests (thin)  | `apps/server/src/vcs/GitVcsDriverCore.test.ts:733`, `:1225`, `:1390` |
| Contract shapes        | this repo, `packages/contracts/src/git.ts`                           |
| The refusal            | this repo, `server/crates/laplus-server/src/orchestration.rs:1231`   |
| The decision reopened  | this repo, `server/docs/adr/0003-…md`                                |
