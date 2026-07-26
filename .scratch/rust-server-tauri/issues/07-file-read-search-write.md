# 07 — File read, search, write, and external editor

**What to build:** A developer opens a file from the tree and reads it, searches
the project by filename to jump somewhere without walking the tree, makes a small
correction and saves it, and can hand a file off to their normal editor when they
would rather work there.

Binary and very large files are refused with an explanation rather than hanging
the UI or rendering garbage.

**Blocked by:** 06 (Filesystem browse and file tree).

**Status:** ready-for-agent

- [ ] A file opened from the tree displays its contents
- [ ] Searching by filename within a project returns matches, and returns them
      fast enough to type against on a large repository
- [ ] An edit made in the UI is saved to disk
- [ ] A file can be opened in the configured external editor
- [ ] A binary file is refused with a message saying so, not rendered
- [ ] A file above a size threshold is refused with a message naming the limit
- [ ] Reading or writing outside an open project's directory is refused
- [ ] A failed write reports why and leaves the file on disk unchanged
- [ ] Tests drive read, search, write and the refusal cases through the socket
      boundary
