# 71 — Remove the source-control hosting surface

**What to build:** the deletion of every UI that talks to a git _host_ — GitHub
or GitLab — leaving local `git` alone. Ledger §3 already decided this
("Source-control hosting … No GitHub/GitLab/PRs/stacked diffs/clone/publish.
Local `git` only"), so this ticket is the removal, not the decision.

**Status:** done

**Found by:** the 2026-07-28 cleanup pass, after commits `94da6be`
(cloud/relay/Clerk) and `9aca0e9` (WSL). Same standard as both: ledger §3, and
every backing method verified unimplemented rather than merely listed in
`refusals.rs`.

## Read this first, or you will mis-scope it

**`refusals.rs` is not a list of unimplemented methods.** It is the error-_tag_
table for every method `rpc.ts` declares, and `refusals::refusal` is reached
only via `DispatchError::UnknownMethod` (`rpc.rs:193`) — after the dispatcher
fails to match. `filesystem.browse` is in that table _and_ implemented in
`filesystem.rs`. The file's own doc gives the real figure: 39 unimplemented
methods, not 70. Check dispatch, not the table:

```sh
grep -rq '"vcs.refreshStatus"' server/crates/laplus-server/src \
  --include=*.rs --exclude=refusals.rs   # → implemented, in git.rs
```

Every method below was checked that way.

## The six methods, all unimplemented

| Method                            | What it backs                        |
| --------------------------------- | ------------------------------------ |
| `server.discoverSourceControl`    | the provider settings page           |
| `sourceControl.lookupRepository`  | clone-a-repo when adding a project   |
| `sourceControl.cloneRepository`   | the same                             |
| `sourceControl.publishRepository` | publish a local repo to a host       |
| `git.resolvePullRequest`          | PR checkout dialog                   |
| `git.preparePullRequestThread`    | the same                             |
| `git.runStackedAction`            | the Commit / Push / Create-PR button |

`vcs.createWorktree` and `vcs.removeWorktree` are also unimplemented, so the PR
dialog's "checkout into a dedicated worktree" mode has no backing either. Leave
those two methods in the contract — worktrees are not source-control hosting and
ticket 64 wants them.

**Still implemented, and must keep working:** `subscribeVcsStatus` and
`vcs.refreshStatus`, both in `git.rs`. Local git status is in scope.

## The six user-facing features

1. **PR checkout into a thread** — paste a URL, `gh pr checkout …`, or `#42`;
   resolve it against the host; check it out locally or into a worktree; open a
   draft thread on it. `PullRequestThreadDialog`, reached from `BranchToolbar`.
2. **Publish repository** — push a local repo to a new GitHub/GitLab repo,
   picking visibility and ssh/https. `PublishRepositoryDialog`,
   `GitActionsControl.tsx:374-968`.
3. **Clone when adding a project** — the command palette's add-project flow can
   look a repo up on a host and clone it.
4. **Provider settings** — `/settings/source-control`, which discovers which
   host each repo belongs to and shows auth per provider.
5. **Commit / Push / Create-PR** — the split button in the chat header, whose
   label follows repo state.
6. **PR status badge** — open/merged/closed on a thread row,
   `ThreadStatusIndicators.tsx:257`.

## The decision already taken: `GitActionsControl` goes whole

The obvious plan is to carve the dead actions out and keep the live status. **It
does not work.** All three actions route through `git.runStackedAction`; strip
them and a 2,035-line split button has nothing to click. There is no coherent
thing to carve to.

The real cost of removing it: `GitActionsControl` is the **only** place in the UI
that renders `aheadCount` / `behindCount` —

```sh
grep -rn "aheadCount\|behindCount" apps/web/src --include=*.tsx --include=*.ts \
  | grep -v "GitActionsControl\|\.test\."   # → no other hits
```

so the "3 unpushed commits" indicator goes with it. The user accepted that
(`gh` covers it) and asked for it to be recorded as reversible: a read-only
badge over the still-live `subscribeVcsStatus` is roughly 50 lines, and is a
separate ticket if it is ever missed.

## Scope, honestly

**~5,667 lines**, not the 2,060 first quoted. That figure came from a filename
match and missed `GitActionsControl.tsx` (2,035) and its test (1,155), which are
mostly this feature.

```
2035  components/GitActionsControl.tsx
1155  components/GitActionsControl.logic.test.ts
 518  components/settings/SourceControlSettings.tsx
 417  components/GitActionsControl.logic.ts
 390  state/sourceControlActions.ts
 304  components/PullRequestThreadDialog.tsx
 232  packages/shared/src/sourceControl.ts
 183  packages/contracts/src/sourceControl.ts
  78  packages/shared/src/sourceControl.test.ts
  75  lib/openPullRequestLink.ts
  73  pullRequestReference.test.ts
  62  sourceControlPresentation.ts
  59  pullRequestReference.ts
  41  packages/client-runtime/src/state/sourceControl.ts
  30  lib/openPullRequestLink.test.ts
  10  lib/sourceControlActions.ts
   5  state/sourceControl.ts
```

Plus `routes/settings.source-control.tsx` and its `routeTree.gen.ts` entry.

## Consumers, by how much they carry

| File                           | Hits | What it needs                                                                                                      |
| ------------------------------ | ---- | ------------------------------------------------------------------------------------------------------------------ |
| `ChatView.tsx`                 | 21   | PR dialog state, `openPullRequestDialog`, `handlePreparedPullRequestThread`, the `<PullRequestThreadDialog>` mount |
| `CommandPalette.tsx`           | 18   | the whole clone-a-repo add-project branch, `openSourceControlSettings`                                             |
| `ThreadStatusIndicators.tsx`   | 5    | the PR badge and `resolveChangeRequestPresentation`                                                                |
| `BranchToolbar.tsx` + logic    | 6    | the `onCheckoutPullRequestRequest` hook that opens the dialog                                                      |
| `chat/ChatHeader.tsx`          | 2    | the `<GitActionsControl>` mount at line 164                                                                        |
| `Sidebar.tsx`, `SidebarV2.tsx` | 4    | presentation helpers only                                                                                          |
| `localApi.ts`                  | 1    | one type import                                                                                                    |

`packages/contracts/src/git.ts` is mixed — it holds `GitRunStackedActionInput`,
`GitActionProgressEvent` and the PR schemas alongside live local-git types. Carve
it; do not delete it.

Drop from `rpc.ts`: `sourceControl.{lookupRepository,cloneRepository,publishRepository}`
and `git.{resolvePullRequest,preparePullRequestThread,runStackedAction}` — and
their entries in `refusals.rs`, whose `the_table_is_the_contract` test reads
`rpc.ts` and will fail if the two part company.

## Acceptance criteria

- [ ] The six methods above are gone from `packages/contracts/src/rpc.ts` and
      from `REFUSALS`, and `refusals::tests::the_table_is_the_contract` passes
- [ ] No UI reaches a git host: no PR checkout, publish, clone, provider
      discovery, or Commit/Push/Create-PR control
- [ ] `/settings/source-control` is gone from the route tree and from the
      settings sidebar, with no dead link left behind
- [ ] Local git still works: the branch selector, and file-change indicators over
      `subscribeVcsStatus` / `vcs.refreshStatus`
- [ ] `packages/contracts/src/git.ts` keeps its local-git types
- [ ] `vcs.createWorktree` / `vcs.removeWorktree` are untouched (ticket 64)
- [ ] Typecheck, lint and `vp run -r test` are green across all five packages
- [ ] `pnpm build:web` succeeds
- [ ] The window opens and a thread still shows its branch —
      `server/tools/ui-driver/`, because a green suite is not evidence

## Comments

### The one test that is red before you start

`apps/web/src/lib/stashImageCompression.test.ts` times out at its 15s limit
under full-suite parallel load and passes on its own. It is unrelated to this
work and was red before it. Do not chase it; do not let it mask a real failure
either — check the name.

### Method

Delete the owned files first and let `tsgo --noEmit -p apps/web` enumerate the
breaks, rather than reading for callers. That is how `94da6be` and `9aca0e9`
were done, and in both the compiler found consumers the greps had missed. Run
`vp lint` afterwards for the imports and locals that typecheck cannot see — both
prior commits needed a second pass for exactly that.

### What the removal found

**Seven methods, not six.** The table above lists seven rows under a heading
that says six, and seven is right. `rpc.ts` declared 68 before this and declares
61 after.

**`the_table_is_the_contract` was already red.** Not by anything here: `94da6be`
dropped `cloud.getRelayClientStatus` and `cloud.installRelayClient` from the
contract and left their rows in `REFUSALS`, so the table named two methods
`rpc.ts` did not. Both rows go with this change, which is what makes the
acceptance criterion reachable at all. The counts in that file's own docs were
computed against the stale table and are now recomputed: 61 declared, 30
unimplemented, 13 whose error union is `EnvironmentAuthorizationError` alone.

**The compiler found five files the ticket's consumer table did not:**
`packages/contracts/src/ipc.ts` (the whole `sourceControl` and `git` sections of
the local-API surface), `client-runtime`'s `state/git.ts`, `state/gitActions.ts`
and `state/vcsAction.ts` — 1,037 lines that exist only for the stacked
action — and `operations/projects.ts`, which carried the add-project clone
flow's provider-readiness logic.

`git.runStackedAction` was also the only **streaming command** in the contract,
so `EnvironmentStreamCommandRpcTag` had no members left. The layer above it went
too: `runStream`, `createEnvironmentRpcStreamCommand`,
`createEnvironmentStreamCommand`, `createRuntimeStreamCommand` and
`runStreamInEnvironment`. `EnvironmentRegistry.runStream` stays — it is the
connection layer's "provide the supervisor to this stream" primitive rather than
this feature's, and `registry.test.ts` covers it on its own account.

### Two things kept, deliberately

**`pr` stays in `VcsStatusResult`; `sourceControlProvider` does not.** The rule
is that the contract describes what the server sends. `git.rs` sends `pr` and
always sends it null; it has never sent `sourceControlProvider`, which was a
declared divergence. So the first stays and the second goes, and the divergence
`socket_conformance.rs` declares for it is now about the capture only.

**`resolveThreadPr` stays.** The PR _badge_ was the feature — it is gone from
the sidebar, from Sidebar v2, from the branch toolbar and from
`ThreadStatusIndicators`. But `effectiveSettled` takes a `changeRequestState`,
and `resolveThreadPr` is what feeds it from the still-live status. Removing it
would have changed thread settlement, which is not this ticket's.

### The aheadCount/behindCount indicator is gone, as agreed

Nothing renders it now. The reversal is still the ~50 lines over
`subscribeVcsStatus` this ticket describes, and still a separate ticket.
