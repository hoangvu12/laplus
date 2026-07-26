# 05 — Project registry

**What to build:** A developer adds a local folder as a project, sees it in their
project list, and finds it still there after restarting the app. Removing a
project takes it off the list without touching anything on disk.

This is the first slice with durable state, so it brings the database with it.
Persistence is not a separate later phase — each slice owns the storage it needs,
and this one establishes the store that subsequent slices extend.

**Blocked by:** 03 (Socket endpoint, local handshake, and the configuration
method).

**Status:** ready-for-agent

- [ ] A folder can be added as a project and appears in the project list
- [ ] The project list survives a server restart
- [ ] Removing a project removes it from the list and leaves the folder on disk
      untouched
- [ ] Adding a path that does not exist, is not a directory, or is not readable
      fails with a message naming the problem
- [ ] Adding the same folder twice does not create a duplicate entry
- [ ] The database is created on first run without manual setup
- [ ] Tests drive add, list, remove and restart through the socket boundary
