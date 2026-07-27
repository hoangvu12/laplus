# 06 — Filesystem browse and file tree

**What to build:** A developer browses the filesystem from the UI to pick a folder
when adding a project, and once a project is open, sees its file tree and
navigates it. Directories expand on demand rather than all at once, so opening a
large repository does not stall the interface.

**Blocked by:** 05 (Project registry).

**Status:** done

- [x] The filesystem can be browsed from the UI to select a folder for a new
      project
- [x] An open project renders its file tree
- [x] Directories load their contents when expanded, not eagerly for the whole
      tree
- [x] A repository with a very large number of files opens without the UI becoming
      unresponsive
- [x] Entries the process cannot read are reported in place rather than failing
      the whole listing
- [x] Symlinks are handled without infinite recursion
- [x] Listings are correct for paths containing spaces and non-ASCII characters
- [x] Tests drive browse and listing through the socket boundary

## Comments

### The two methods, and which half of the ticket each answers

`filesystem.browse` and `projects.listEntries`. Both live in
`crates/lightcode-server/src/filesystem.rs`; the namespaces are upstream's
rather than a boundary.

Browse is the folder picker: one directory, **directories only**, filtered by
whatever the user has typed so far. The command palette sends it on every
keystroke (`CommandPalette.tsx`), so a trailing separator means "list this
directory" and anything else means "complete this name". listEntries is the
file tree: the whole workspace, files and directories, in one answer.

### "Directories load their contents when expanded" is a client behaviour, not a protocol one

The wording implies a per-directory fetch, and the UI does not offer the server
that shape. `ProjectListEntriesInput` is `{ cwd }` and nothing else — no
directory, no cursor, no depth — and `FileBrowserPanel.tsx` calls it **once per
project**, with the workspace root, then hands the whole flat array to
`@pierre/trees` as `paths`. A server that answered one directory at a time would
be answering a call no client makes.

The laziness is real, it just lives on the other side: the tree opens one level
(`initialExpansion: 1`) and materialises rows as they are revealed. Upstream's
server is the same shape — its `WorkspaceSearchIndex.list()` runs a single empty
query against the whole index and returns up to 25,000 entries.

So the criterion is met twice over, once per surface, and neither is a
per-directory tree fetch:

- **Browse genuinely does read one directory per request** and never walks a
  tree. That is the ticket's expand-on-demand behaviour, and it is the surface
  the first criterion is about.
- **For the tree**, what the server owes is that the single listing is bounded
  and does not stall anything — which is the next two sections.

### The first method that had to wait, and what that cost

Ticket 03 left a note in `server.rs`: "the first method that has to wait is the
one that should spawn". This is it.

The connection loop reads frames one at a time and answers each before taking
the next. That is right while every method answers from memory. Walking twenty
thousand files is not memory, and answering it inline would hold the socket's
only reader for the length of the walk — the `Ack` that releases a
subscription's next chunk, the `Ping` the UI sends every five seconds, and every
other call the window makes would all queue behind the disk. The file tree would
arrive and the rest of the app would have stopped.

So `Answer` gained a third variant, `Deferred`: the method hands back the work
instead of the answer, and the connection runs it on a blocking thread that
writes its own `Exit`. Correlation on this wire is by `requestId` and never by
order, so answering out of order is what the reference server already does.

`socket_filesystem.rs::a_large_repository_does_not_stall_the_connection_while_it_is_listed`
is the test that made it necessary, and it has teeth: with the listing answered
inline it fails, because the `Ping` sent immediately after the request comes
back *after* the two-thousand-file listing rather than before it.

Blocking rather than `async`, deliberately. There is no non-blocking way to
enumerate a directory; an `async` wrapper would move the same stall onto a
runtime worker.

`DispatchError` was folded from a variant per method (`Command(CommandError)`)
to `Declared(Value)` at the same time. Dispatch has no business enumerating the
error type of every method it routes to — what it needs to tell apart is "no
such method", which is the server's answer, from "the method refused", which is
the method's. That keeps it at two variants for the remaining fifteen tickets.

### Declared divergence: `.git` is the one name the walk skips, and there is no ignore-file support

lightcode has **no `.gitignore` semantics**. Upstream gets them free from the
`fff` indexer; implementing them by hand is a subtle piece of work (negations,
anchoring, nested ignore files) and doing it approximately would hide files from
the user, which is worse than showing too many. The alternative — pulling in the
`ignore` crate — roughly doubles the dependency graph of a project whose entire
reason for existing is size.

`.git` is the single exception, skipped at every level. It is in every
repository, it is machine state rather than source, and on its own it is large
enough to spend the whole limit on loose objects. Upstream's indexer does not
surface it either.

**The cost is larger than "the tree shows `node_modules`", and worth stating
plainly, because it interacts with the breadth-first walk.** A JavaScript
project's `node_modules` contributes on the order of a thousand entries at depth
two and tens of thousands at depth three — so in a repository like that the
25,000-entry budget is spent inside `node_modules` before the walk reaches
depth three anywhere else. The user's own source is present down to depth two
and can be **missing below it**: in a monorepo, `packages/web/src` appears in
the tree with nothing inside it.

The only signal the user gets is the "· partial" badge the `truncated` flag
renders. That is honest but thin. Upstream never meets this because `fff`
honours ignore files, so this is a real behaviour gap against the reference and
not merely a stylistic difference.

It is left as it is here rather than patched with a guessed skip list
(`node_modules`, `target`, `dist`, …), because which directories a user wants
hidden is a product decision and a wrong guess hides files silently. Ticket 25
carries the fix.

**Since closed.** Ticket 25 was built as part of ticket 07 — search made it
urgent rather than optional — and the scan now asks
`git ls-files --cached --others --exclude-standard`, keeping the walk described
here as the fallback for a folder that is not a repository. Everything below
about breadth-first truncation and the cycle guard still describes that
fallback.

### The walk is breadth-first, and that is load-bearing

A listing that stops at the limit stops somewhere, and where decides what the
user sees. Depth-first would spend the whole budget inside the first directory
and leave the workspace root half-described. Breadth-first fills the shallow
levels completely and drops the deepest, and — because a directory is always
emitted before anything inside it — a truncated listing can never contain a path
whose parent is missing, which the tree would otherwise have to invent.

It also means the entry limit, not the cycle guard, is what guarantees
termination. A filesystem that cycles can only make the listing *fill up*.

### Symlinks: followed once per distinct target

A directory symlink is followed if its target has not been walked yet, and
listed-but-not-entered if it has. The set of already-walked targets is seeded
with the workspace root, so a link pointing back at the project is a leaf rather
than a hall of mirrors. Only symlinks are canonicalised — a plain directory
cannot contain itself, so canonicalising every one of them would be a syscall
per directory to answer a question only symlinks can raise.

Net effect: a directory's contents appear once in the listing however many names
lead to them. Pinned by
`filesystem::tests::a_symlinked_directory_is_walked_once_and_a_cycle_is_not_walked_at_all`,
which is skipped — noisily — on a machine that will not create directory
symlinks (Windows without Developer Mode).

### Not covered automatically

A directory the **operating system refuses to open** is reported in place: it
keeps its entry in the tree, loses only its children, and the listing as a whole
still succeeds. Only the root is different, because its contents *are* the
answer.

That branch is not exercised by the suite. Making a genuinely unreadable
directory needs `icacls` on Windows and `chmod` elsewhere and would not run the
same way on a developer's machine and in CI — the same call ticket 05 made for
`projects::tests::a_folder_the_server_may_not_open_is_reported_as_unreadable`.
What *is* tested is the case a test can make anywhere: a dangling symlink, an
entry whose target the process cannot reach, which keeps its place while the
workspace around it is described in full.

The count goes to the log rather than the wire, because `ProjectEntry` is a path
and a kind with nowhere to say "and this one refused". It also covers the one
case that genuinely cannot be reported in place — a directory entry the
filesystem would not name at all, where there is no name to put in the tree.

### Where the tests sit, and why not all of them are at the socket

The ticket's last line asks for browse and listing **through the socket
boundary**, and `tests/socket_filesystem.rs` is that: nine tests covering the
picker, the tree, `.git` exclusion, spaces and non-ASCII names, both refusals,
concurrency, disconnection, and the responsiveness claim.

Three rules are pinned by unit tests beside the module instead, and each for a
reason rather than convenience:

- **Truncation.** `truncated: true` needs more entries than the limit, and the
  limit is 25,000. `list(cwd, limit)` takes the limit as a parameter so the
  behaviour can be driven with a tree of three files. That is a real parameter,
  not a test seam — the socket path passes `MAX_ENTRIES`.
- **Symlink cycles**, which need a symlink the machine may refuse to create, so
  the test announces a skip rather than failing on a locked-down CI box.
- **Path arithmetic** — prefix matching, hidden-name rules, `~`, explicit
  relative paths — which is pure string work and worth having fast and precise.

The socket suite therefore asserts `truncated == false` and never `true`. That
is the one acceptance criterion whose evidence is a level below the boundary.

### Two smaller notes

- **No path confinement, deliberately.** Neither method restricts where it may
  look. The picker's whole purpose is to walk a filesystem the server has no
  project for yet, so a confinement rule would have to admit every path anyway.
  Reachability is the boundary — the socket is loopback-only — and both methods
  only read. Ticket 07's `readFile`/`writeFile` are a different case and will
  confine themselves to a workspace root.
- **`parentPath` and `normalizedCwd` are tidied.** The picker's own "list this
  directory" spelling ends in a separator, and echoing that back would give the
  client `C:\repo` and `C:\repo\` as two different places.
