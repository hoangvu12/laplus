# 05 — Project registry

**What to build:** A developer adds a local folder as a project, sees it in their
project list, and finds it still there after restarting the app. Removing a
project takes it off the list without touching anything on disk.

This is the first slice with durable state, so it brings the database with it.
Persistence is not a separate later phase — each slice owns the storage it needs,
and this one establishes the store that subsequent slices extend.

**Blocked by:** 03 (Socket endpoint, local handshake, and the configuration
method).

**Status:** done

- [x] A folder can be added as a project and appears in the project list
- [x] The project list survives a server restart
- [x] Removing a project removes it from the list and leaves the folder on disk
      untouched
- [x] Adding a path that does not exist, is not a directory, or is not readable
      fails with a message naming the problem
- [x] Adding the same folder twice does not create a duplicate entry
- [x] The database is created on first run without manual setup
- [x] Tests drive add, list, remove and restart through the socket boundary

## Comments

### The registry is not where the ticket's wording implies

`WS_METHODS` in the contracts package declares `projects.list`, `projects.add`
and `projects.remove` under a comment reading `// Project registry methods`.
**All three are dead strings** — no `Rpc.make` defines them, the RPC group does
not register them, and nothing in the upstream server or UI sends or answers
one. Implementing them would have produced a server no client ever calls.

The registry the UI really drives is the orchestration shell, captured whole in
`fixtures/socket-wire/05-orchestration-and-backpressure.ndjson`:

- adding is `orchestration.dispatchCommand` with a `project.create` command,
- removing is the same method with `project.delete`,
- the *list* is `orchestration.subscribeShell` — its snapshot, plus
  `project-upserted` / `project-removed` deltas,
- and the two are joined by a **sequence** the client de-duplicates on, which is
  why the database persists it rather than counting from zero at each boot.

So this ticket lands the first two orchestration methods rather than a
projects-namespace surface. Threads are an empty array in the snapshot until
tickets 10 and 11, which extend the same subscription and share the sequence.

### Declared divergence: a missing folder is refused, not created

`project.create` carries `createWorkspaceRootIfMissing`, and the upstream UI
sends it as `true` on every add. The reference server obeys, so upstream turns a
mistyped path into a new empty directory and reports success.

lightcode ignores the flag and refuses, naming the path. Two reasons:

1. This ticket asks for exactly that — "adding a path that does not exist … fails
   with a message naming the problem". A typo that silently creates a directory
   is not a diagnostic.
2. v1's socket auth is permissive by design; loopback is the boundary and no
   credential is verified. Honouring the flag would let any local process that
   can open the socket make the server create directories at paths of its
   choosing.

Reachable only by typing a path by hand — the folder picker in ticket 06 offers
only folders that exist — and the answer is one clear sentence. Pinned by
`orchestration::tests::a_missing_folder_is_refused_even_when_the_client_asks_for_it_to_be_created`.

### Persistence arrived here rather than in the spec's phase 7

The spec's build order lists persistence eighth. That ordering is rejected: a
project registry that forgets its projects is not a smaller version of the
feature, it is a different one. `crates/lightcode-server/src/store.rs` is the
store the later slices extend — schema versioned by `user_version`, SQLite
bundled so a first run needs nothing installed, and the database used as the
clock so there is only one answer to "when did this happen".

### Not covered automatically

"Is not readable" is pinned as a mapping unit test rather than by making a
genuinely unreadable directory, which needs `icacls` on Windows and `chmod`
elsewhere and would not run the same way on a developer's machine and in CI.
See `projects::tests::a_folder_the_server_may_not_open_is_reported_as_unreadable`.
