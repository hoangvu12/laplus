# 08 — Live file tree

**What to build:** The file tree reflects what is actually on disk while the
developer works. When the agent creates, edits, deletes or moves files, the tree
updates without a manual refresh — so what the developer sees stays true during a
session.

**Blocked by:** 06 (Filesystem browse and file tree), 04 (First streaming
subscription).

**Status:** done

- [x] Creating a file outside the app makes it appear in the tree
- [x] Deleting a file outside the app removes it from the tree
- [x] Renames and moves are reflected correctly rather than appearing as an
      unrelated create and delete
- [x] A burst of rapid changes is coalesced rather than flooding the UI with
      updates
- [x] Watching does not recurse into ignored directories such as build output and
      dependency trees
- [x] Watchers are released when a project is closed, and no file handles or
      threads are leaked
- [x] Watching a very large repository does not exhaust system watch limits or
      pin a core
- [x] Tests assert that a change on disk produces the expected event sequence
      through the socket boundary
- [~] "**without a manual refresh**" — **declared divergence**, see below. The
      server notices every change and every call afterwards tells the truth, but
      the *mounted tree* still redraws on the refresh button, on reopening the
      project and on reconnect, because the contract has no frame that could
      make it redraw on anything else.

## Comments

### The declared divergence: there is no file-tree subscription to push to

The ticket is blocked on 04 because the plan assumed the tree would be pushed.
`HANDOFF-rust-server-tauri.md` says so outright — "file watching via `notify`,
feeding the file-tree and git-status subscriptions". **The file-tree half of that
sentence describes a subscription that does not exist**, and the whole shape of
this ticket follows from that, so the evidence is worth setting out in full.

- `WS_METHODS` in `t3code/packages/contracts/src/rpc.ts` declares exactly eight
  `subscribe*` methods: `subscribeVcsStatus`, `subscribeTerminalEvents`,
  `subscribeTerminalMetadata`, `subscribePreviewEvents`,
  `subscribeDiscoveredLocalServers`, `subscribeServerConfig`,
  `subscribeServerLifecycle`, `subscribeAuthAccess`, plus orchestration's
  `subscribeShell` and `subscribeThread`. None is about files. A grep for
  `watch` across `packages/contracts` returns nothing.
- The tree is a **query**, not a stream.
  `client-runtime/src/state/projectCommands.ts` builds `listEntries` as
  `createEnvironmentRpcQueryAtomFamily(..., { staleTimeMs: 30_000, idleTtlMs: 5 *
  60_000 })`, and `FileBrowserPanel.tsx` reads it through
  `useProjectEntriesQuery`.
- That query revalidates on **mount, focus and reconnect, and never on a
  timer**. `Atom.swr` (`effect-smol/.../reactivity/Atom.ts:1788`) refreshes only
  when the atom is read and `shouldRevalidateSWR` says it is stale, or when a
  `focusSignal` fires — and `listEntries` passes no focus signal. The one other
  trigger is `rpcGenerationAtom`, which changes on reconnect.
- `getProjectEntriesQueryAtom` has exactly one caller in the whole app, and
  nothing else refreshes it.

So while `FileBrowserPanel` stays mounted, the only things that redraw it are the
refresh button and a reconnect. **Upstream has the same limitation** — its server
watches `keybindings.ts`, `serverSettings.ts` and git, and nothing else — so this
is not lightcode falling short of the reference server. It is the ticket
describing a capability the UI was never built to receive.

Review caught that this is in fact **slightly worse than the paragraph above
says**, and the correction is worth keeping: `staleTimeMs: 30_000` means even
*remounting* the panel inside thirty seconds does not re-fetch, because
`shouldRevalidateSWR` finds the cached answer still fresh. So the honest list of
things that redraw a mounted tree is the refresh button, a reconnect, and a
remount more than thirty seconds after the last one.

The two ways to deliver the headline literally were both rejected:

1. **Invent a `subscribeProjectEntries` method.** The client has no decoder for
   it, so it would be a stream nothing ever opens — the same mistake
   `crate::orchestration` documents about `projects.list`/`add`/`remove`, which
   are dead strings in `WS_METHODS` that no `Rpc.make` defines.
2. **Patch `apps/web`.** "The UI needs no changes" is the project's premise, not
   a convenience; the moment the UI forks, the argument for reusing it at all
   starts unwinding.

**What was built instead is the whole of the ticket that the wire can carry**, and
it is not a consolation prize: the server now *knows*, so every answer after a
change is a true one. The user-visible half is the composer's `@` mention, which
reads the held scan and so was the one place a stale index was actually
observable — an agent that creates `src/handler.rs` mid-turn now has it offered
on the next keystroke, where before this ticket it was invisible until the
project was listed again. `projects.listEntries` was already truthful because it
always rescans, and it still does, deliberately — see below.

### The mechanism: forget, do not rescan

`crate::watcher` turns an operating-system event into "this workspace, this
relative path" and hands it to `filesystem::changed`, which drops the held scan.
That is all. It does **not** rescan in the background, and the reasons are the
two failures the ticket names.

A rescan shells out to `git ls-files` twice and costs around a tenth of a second
on a large repository. A `cargo build` or an `npm install` is thousands of events
over minutes, so a background rescan would be doing that over and over for a
workspace nobody is currently searching — which is exactly "pins a core". And it
would have to be debounced to be bearable, which trades a bounded cost for
unbounded staleness during a sustained burst.

**Forgetting is where the coalescing comes from, and it needs no timer.** A
thousand changes and one search cost one scan, however fast the thousand arrived,
because forgetting something already forgotten is free. Better than a debounce in
both directions: cheaper under load, and never stale for longer than it takes the
first caller to ask.

`listEntries` still rescans unconditionally rather than reading the now-watched
index, and that is a decision rather than an oversight. Watchers drop events —
`need_rescan` exists in `notify` for exactly that — and the refresh button is the
user's escape hatch. A refresh button that returned a cached answer would be a
button that cannot fix anything.

### The ignore filter is the last listing, because the server has no rules of its own

A recursive watch cannot be told to skip a subtree: Windows'
`ReadDirectoryChangesW` is all-or-nothing, and `notify` exposes no filter. So
"watching does not recurse into ignored directories" has to be done on the
events — and the server has no ignore rules to do it with, because what is in a
workspace is whatever `git ls-files --exclude-standard` said (ticket 07). Asking
git per event would cost far more than the scan being avoided.

**The last listing is the ignore rule, already computed.** `Listing::is_interesting`
is one line: the changed path's parent must be the workspace root or a directory
the listing names. `node_modules` is not in the listing, so nothing under
`node_modules/left-pad/` has a parent it names, and an `npm install` passes
without a single invalidation. The same holds for `target/debug/…`, for
`.git/objects/…`, and for whatever a particular project ignores that this server
has never heard of.

Two things it deliberately does *not* filter, and both are load-bearing:

- **A new directory directly in a known parent.** Creating `node_modules` itself
  costs one invalidation, and it has to: until the workspace is scanned again
  there is no way to know whether a new directory is ignored or is the user's new
  feature. After that scan it is absent from the listing and its whole subtree
  goes quiet.
- **A workspace with nothing held.** There is nothing to invalidate, so the
  *rest* of a long build is free even before the filter gets a say.

The consequence worth naming: a file two levels below the last listing —
`src/newthing/a.rs` where `src/newthing` is also new — is not reported. It does
not need to be, because the creation of `src/newthing` was, so the scan is
already forgotten by the time the file lands.

Pinned case by case in
`filesystem::tests::a_change_matters_only_where_the_listing_names_its_parent`.

**The filter has one hole, and it is the same case it was built for.** When the
backend's buffer overflows, `notify` raises `need_rescan`, and the only correct
response is to invalidate every watched workspace at its root — which
`is_interesting("")` always accepts, because a dropped event could have named
anything. `ReadDirectoryChangesW` overflows precisely under sustained churn, so
a large enough `npm install` defeats the filter it is meant to survive. That is
not a bug to fix — ignoring a dropped-events signal would be wrong — but the
claim above is about the ordinary case and should not be read as a guarantee.
`a_dependency_tree_filling_up_never_reaches_the_tree` writes fifty files, well
under any real threshold, so it exercises the filter and not this.

### Renames read as renames because nothing is being patched

The criterion guards against a create and a delete arriving out of order and
leaving a file present under both names or neither. There is no such window here:
the answer to a search is a fresh description of the workspace, not a patch
applied to an old one, so there are no two events to reorder. Driven from both
sides in `socket_watch.rs` — a rename within a directory, and a move across two.

### One watcher, one thread, and a ceiling on the roots

`notify`'s Windows backend spawns a thread per `RecommendedWatcher` and that
thread services every path registered on it, so there is a single instance and
workspaces are added to and removed from it. "No threads leaked" is then a
property of the design rather than something to remember at each release, and the
instance is created lazily on the first watch, so a server nobody opens a project
in spawns nothing.

`MAX_WATCHED` is 16. Not an operating-system limit — on Windows each root is one
directory handle, and a session with more than a handful of projects open does
not exist. It is there because `projects.listEntries` takes the workspace from the
client, so without a ceiling the number of handles the server holds is decided by
whatever is on the other end of the socket.

**Past the ceiling the least recently listed workspace is evicted**, and the
first version of this got that backwards. It refused the *new* workspace, which
hands the ceiling to whoever fills it first: sixteen folders a client listed once
and abandoned would lock out the project the user is actually working in, and the
only sign of it would be a line on stderr. There is no release path for a `cwd`
that was listed but never registered as a project, so that state was reachable
and permanent. Evicting by least-recently-listed makes the surviving set the
projects being *used*, because `listEntries` is the UI opening a project or
pressing its refresh button — and an evicted workspace loses only freshness, with
the next listing both rescanning and rewatching it. Pinned by
`watcher::tests::past_the_ceiling_the_least_recently_listed_workspace_is_evicted`.

Release is `project.delete`, which is the only thing on this wire that means "this
project is closed". Making that reachable needed `store::Removal::Committed` to
carry the deleted project's `canonical_root` — read inside the same transaction
*before* the delete, because afterwards there is no row to read it out of, and a
deleted project the caller cannot name is one whose handle stays held.

### A large repository costs what an empty one costs — on Windows

A recursive watch on Windows is a single registration on the root directory, so a
repository of twenty-five thousand files costs one handle, the same as an empty
one. Nothing walks, scans or polls, so there is no per-file work anywhere and
nothing that could occupy a core.

**That is a property of the backend, and it does not survive a port.** `inotify`
on Linux registers a watch per *directory*, so a large repository there can meet
`max_user_watches` (commonly 8192) and `Watcher::watch` will start refusing. The
refusal is already handled the way it should be — logged, the workspace left
unwatched, everything about it still working — but a Linux build would want
subtree exclusions before it could claim the first paragraph. Recorded in the
module header so the port meets it rather than discovers it.

### What review caught, and what the evidence was

- **A nested project was silently never updated.** `deliver` routed each event
  path with `find_map` over the watched roots, so it stopped at the *first* root
  containing the path. With a repository and one of its packages both open as
  projects, every event under the package was attributed to the repository —
  which is watching a superset and whose own listing does not care — and the
  package's scan was never invalidated. Not "a bit stale": permanently inert for
  the inner project, and nothing tested two roots at all. It is now a loop over
  every matching root, and a duplicate report is harmless because invalidation is
  idempotent. `watcher::tests::a_change_inside_a_nested_workspace_is_reported_to_both_of_them`
  fails against the old routing.
- **The ceiling evicted the wrong end.** See `MAX_WATCHED` above.
- **A refused delete could still report success.** `remove_project` defaulted the
  removed project's `canonical_root` to `""` when the pre-delete read came back
  empty, which is not a harmless fallback: it is a value the caller releases
  nothing under and is never told about, so a watcher would be held for a project
  that no longer exists with no way to find out. It is now checked *before* the
  commit, so the transaction rolls back — the same shape as the symmetric check
  in `insert_project`.

### The gap that was left: a change during the opening scan

`rescan` scans and then holds, and a change landing in between is lost. The
window is one scan — around a tenth of a second on a large repository, at the
moment a project is opened — and the consequence is that one file may be missing
from the `@` mention until the next `listEntries`.

It is left rather than fixed because every correct fix is a redesign of the hold
protocol, and each version costs more than the race does. Watching before
scanning does not help on its own: `changed` cannot invalidate a workspace
nothing is held for, so the report still falls into the gap. Marking the
workspace dirty during the scan does close it, but during a burst with nothing
held there is no listing to filter against, so every event marks dirty, no scan
is ever allowed to be cached, and the "pins a core" failure moves onto the
request path — a worse outcome, in the exact case the filter exists for, traded
for a race that heals on its own.

And it does heal: `listEntries` always rescans, so the refresh button, reopening
the project and reconnecting all fix it. Those are precisely the three events the
declared divergence already says the tree redraws on, so this race is invisible
against the behaviour actually being delivered rather than a new hole in it.

### What the tests can and cannot assert

- **`filesystem::tests::a_change_matters_only_where_the_listing_names_its_parent`**
  and **`a_reported_change_drops_the_scan_only_when_it_could_have_changed_it`**
  drive the decision without an operating system in the way, so the ignore filter
  is pinned deterministically rather than through a timing window.
- **`socket_watch.rs`** drives the whole path through the socket: created,
  deleted, renamed, moved, a two-hundred-file burst, a dependency tree filling up
  during a session, a project closing, three reconnections, and a workspace
  deleted out from under the server. Every wait is a bounded poll rather than a
  sleep, so "never arrives" fails with a sentence.
- **A negative assertion about the filter cannot be made from the socket.** "This
  event did not invalidate anything" is indistinguishable from "the event has not
  been delivered yet", so what `a_dependency_tree_filling_up_never_reaches_the_tree`
  asserts is the outcome — ignored files never reach the tree or the mention
  picker, while the user's own file, written after all that noise, does.
- **`a_search_reads_the_held_scan_and_a_listing_replaces_it` lost an assertion.**
  It used to claim a keystroke sees a stale index after a file appears; that is
  now false, which is the feature. The claim that survives is
  `a_second_search_reads_the_scan_the_first_one_took`, which compares `Arc`
  identity across two reads with nothing changing between them — the one form of
  "a keystroke does not pay for a rescan" that is not a race with the watcher.

### Not covered automatically

- **`max_user_watches` being exhausted.** Only reachable on Linux, which v1 does
  not ship, and only on a machine whose limit has actually been reached.
- **A `need_rescan` event.** `notify` raises it when the backend drops events —
  an overflowed buffer under extreme churn. It is handled (every watched
  workspace is invalidated at its root, which no listing can dismiss) but there is
  no way to make one happen on demand, so neither the handling nor the hole it
  leaves in the ignore filter is driven.
- **A genuinely large repository.** The claim that a recursive Windows watch
  costs one handle whatever the repository holds is a property of
  `ReadDirectoryChangesW` and is argued rather than measured; the largest tree any
  test builds is a couple of hundred files. What *is* driven is that a burst of
  two hundred changes is answered once and completely.
- **An evicted workspace still working.** The ceiling and the eviction order are
  pinned; that a workspace past it still lists, searches and reads exactly as it
  did before the ticket is argued from the code rather than driven.
- **The race during the opening scan.** See above — it is a window of one scan,
  and a test that tried to hit it would be asserting on a timing gap rather than
  on a behaviour.
