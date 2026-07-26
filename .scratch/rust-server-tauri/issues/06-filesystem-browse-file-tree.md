# 06 — Filesystem browse and file tree

**What to build:** A developer browses the filesystem from the UI to pick a folder
when adding a project, and once a project is open, sees its file tree and
navigates it. Directories expand on demand rather than all at once, so opening a
large repository does not stall the interface.

**Blocked by:** 05 (Project registry).

**Status:** ready-for-agent

- [ ] The filesystem can be browsed from the UI to select a folder for a new
      project
- [ ] An open project renders its file tree
- [ ] Directories load their contents when expanded, not eagerly for the whole
      tree
- [ ] A repository with a very large number of files opens without the UI becoming
      unresponsive
- [ ] Entries the process cannot read are reported in place rather than failing
      the whole listing
- [ ] Symlinks are handled without infinite recursion
- [ ] Listings are correct for paths containing spaces and non-ASCII characters
- [ ] Tests drive browse and listing through the socket boundary
