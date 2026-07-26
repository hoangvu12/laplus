# 21 — Branches: list, switch, create, init

**What to build:** A developer manages branches without dropping to a shell. They
see what branches exist, switch between them to keep work separated, create a new
one to start work, and initialise a repository in a project that has none so that
agent changes become reviewable.

**Blocked by:** 19 (Working tree status with live refresh).

**Status:** ready-for-agent

- [ ] Branches are listed with the current one indicated
- [ ] Switching branches updates the working tree and the displayed status
- [ ] A new branch can be created from the current position
- [ ] A repository can be initialised in a project that has none, after which
      status works
- [ ] A switch blocked by uncommitted changes is refused with an explanation, not
      silently or destructively
- [ ] Creating a branch whose name already exists fails with a clear message
- [ ] An invalid branch name is rejected before it reaches the git binary
- [ ] Tests cover list, switch, create, init and the blocked-switch case through
      the socket boundary
