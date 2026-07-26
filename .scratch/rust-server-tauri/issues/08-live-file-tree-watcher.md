# 08 — Live file tree

**What to build:** The file tree reflects what is actually on disk while the
developer works. When the agent creates, edits, deletes or moves files, the tree
updates without a manual refresh — so what the developer sees stays true during a
session.

**Blocked by:** 06 (Filesystem browse and file tree), 04 (First streaming
subscription).

**Status:** ready-for-agent

- [ ] Creating a file outside the app makes it appear in the tree
- [ ] Deleting a file outside the app removes it from the tree
- [ ] Renames and moves are reflected correctly rather than appearing as an
      unrelated create and delete
- [ ] A burst of rapid changes is coalesced rather than flooding the UI with
      updates
- [ ] Watching does not recurse into ignored directories such as build output and
      dependency trees
- [ ] Watchers are released when a project is closed, and no file handles or
      threads are leaked
- [ ] Watching a very large repository does not exhaust system watch limits or
      pin a core
- [ ] Tests assert that a change on disk produces the expected event sequence
      through the socket boundary
