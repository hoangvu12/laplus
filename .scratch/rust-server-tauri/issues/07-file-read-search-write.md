# 07 — File read, search, write, and external editor

**What to build:** A developer opens a file from the tree and reads it, searches
the project by filename to jump somewhere without walking the tree, makes a small
correction and saves it, and can hand a file off to their normal editor when they
would rather work there.

Binary and very large files are refused with an explanation rather than hanging
the UI or rendering garbage.

**Blocked by:** 06 (Filesystem browse and file tree).

**Status:** ready-for-human

- [x] A file opened from the tree displays its contents
- [x] Searching by filename within a project returns matches, and returns them
      fast enough to type against on a large repository
- [x] An edit made in the UI is saved to disk
- [x] A file can be opened in the configured external editor
- [x] A binary file is refused with a message saying so, not rendered
- [~] A file above a size threshold is refused with a message naming the limit
      — **declared divergence**, see below: it is truncated rather than refused,
      because that is what the contract and the UI are built for
- [x] Reading or writing outside an open project's directory is refused
- [x] A failed write reports why and leaves the file on disk unchanged
- [x] Tests drive read, search, write and the refusal cases through the socket
      boundary

## Comments

### Ticket 25 was folded in, because search made it urgent

Ticket 06 left ignore semantics as a follow-up. Building search is what showed
it was not one: `projects.searchEntries` is the composer's `@` mention, and in a
JavaScript project without ignore support, typing `index` returns hundreds of
`node_modules` hits before any of the user's own files. That is a more visible
failure than the tree truncation ticket 25 was raised for, and it cannot be
papered over with a "partial" badge.

So the scan is now **`git ls-files --cached --others --exclude-standard -z`**,
with the plain walk from ticket 06 kept as the fallback for a folder that is not
a repository. The reasoning is in `filesystem::scan`; the short version is that
the spec already commits to shelling out to `git` for tickets 19–21, so this
adds no dependency and no bytes to the artifact — where the `ignore` crate would
have roughly doubled the dependency graph of a project whose whole reason is
size.

Two details that are not obvious and are both load-bearing:

- **`--deleted` is subtracted.** `--cached` lists what the *index* holds, which
  includes a file the user deleted without staging the deletion. Without the
  second call the tree offers files that are not there and every attempt to open
  one fails. Pinned by
  `filesystem::tests::a_file_deleted_without_staging_is_not_listed`.
- **Directories are inferred.** Git names files and never the folders holding
  them, so ancestors are synthesised — the same manoeuvre upstream's indexer
  makes. The consequence is that an *empty* directory does not appear in the
  tree, because git has nothing to say about one.

### The index, and why freshness is decided by the method rather than a clock

The composer debounces at 120 ms and asks for eighty matches; scanning this
repository's vendored checkout takes about 100 ms. One scan per keystroke would
be the difference between a picker that keeps up with typing and one that does
not, so `filesystem::Index` holds the last scan per workspace.

There is no time-to-live, and that is deliberate — a number would be a guess at
how stale is too stale. Instead:

- `listEntries` is the UI saying "show me this project", on opening it and on
  the refresh button, so it **always rescans** and leaves the result in the
  index.
- `searchEntries` is a keystroke, so it **reads whatever is held** and only
  scans when there is nothing.
- `writeFile` **forgets**, so a file the user has just created can be mentioned
  on the next keystroke. Upstream refreshes its indexer at exactly this point.

Ticket 08's watcher is the honest answer to "what if the agent changes something
we did not write", and it will invalidate through the same door.

### Declared divergence: a large file is truncated, not refused

The ticket asks for a file above a size threshold to be "refused with a message
naming the limit". The contract disagrees, and the contract wins:
`ProjectReadFileResult` carries `truncated` and `byteLength` for exactly this
case, and the editor pane is built to show a partial file with a banner saying
how much is missing.

Refusing would mean a 2 MB log file cannot be looked at at all — worse for the
user, and it would leave two contract fields that no response could ever set.

**The criterion is not met in full, and the gap is worth stating plainly.** The
response does not *name* the limit anywhere: it carries `truncated: true` and
the file's true `byteLength`, and the megabyte is implied by how much `contents`
came back. A UI that wanted to say "showing the first 1 MB of 2.4 MB" can
compute both numbers, but nothing on the wire spells the threshold out. Adding a
field for it would be inventing one the client cannot decode. The megabyte is
upstream's number, kept so that a client and server do not disagree about which
files are large.

### Declared divergence: `cwd` in `shell.openInEditor` is not a working directory

`LaunchEditorInput` names its only path field `cwd`. It is the **target** — the
UI passes whatever the user asked to open (`editorPreferences.ts` forwards its
`targetPath`), which is a file as often as a folder, and it may carry a `:line`
or `:line:column` suffix. The field name is upstream's and is kept because it is
on the wire; `editor.rs` treats it as what it is.

The position suffix needs one piece of care on this project's only platform: a
Windows path begins `C:\`, so a rule that read every colon as a position marker
would turn every absolute path into a line number. Only trailing all-digit
groups count — `editor::tests::a_windows_drive_letter_is_not_mistaken_for_a_position`.

`availableEditors` in `server.getConfig` is filled at the same time. The two are
halves of one feature: a picker offering an editor the machine does not have
would produce a failure the user can do nothing about. Removing the stub also
removed a declared divergence from `socket_conformance.rs`, which failed until
the declaration was deleted — the mechanism working as intended.

### Confinement is checked twice, and the second check is the one that matters

`readFile` and `writeFile` are the first methods with a rule about *where* they
may look, and it is enforced in two passes:

1. **Lexically**, before touching the disk. Note that `Path::components()` does
   **not** resolve `..`, so `root.join("../secret.txt")` still literally begins
   with the root — a naive `strip_prefix` check passes it. Each `..` has to be
   applied and the result re-checked one component at a time; `files::descend`
   is that, and the test that caught it is
   `a_path_outside_the_project_is_refused_before_the_disk_is_touched`.
2. **After resolving symlinks**, on both the root and the target. A path can be
   perfectly well-behaved as a string and still land outside — `notes.txt`
   inside the project can be a link to `~/.ssh/id_rsa`. The contract has a
   separate failure literal for this (`resolved_path_outside_root`) precisely
   because it is a different fact from the lexical one.

A write checks its *parent directory* rather than the file, because a file that
does not exist yet has no real path to resolve — and the check happens after the
parent is created and before any bytes are written, so a link out of the project
is caught before the write rather than after.

### Three things review caught, and what the evidence was

- **A refused write could still create directories outside the project.**
  `create_dir_all` ran before the symlink check, and it follows symlinks — so
  `link/nested/file.txt`, where `link` points out of the project, created
  `nested` outside it and *then* refused the call. Refusing a write that has
  already made directories somewhere it should not is not refusing it. The
  confinement check now runs first against the deepest ancestor that exists, and
  again once the parent has been created. Pinned by
  `a_write_through_a_symlinked_directory_creates_nothing_outside_the_project`,
  which fails against the old ordering.
- **`shell.openInEditor` answers `null`, not `{}`.**
  `WsShellOpenInEditorRpc` declares no `success`, and `Rpc.make` defaults that
  to `Schema.Void` (`effect/unstable/rpc/Rpc.ts:957`); `Schema.Void`'s JSON
  codec is `undefinedToNull` (`effect/SchemaAST.ts:868`), so `null` is what the
  reference server puts on the wire. `{}` would in fact have decoded — Void's
  parser is `fromConst`, which ignores its input — but working by accident is
  not a reason to send the wrong thing.
- **`ExternalLauncher*` errors carry no `message`.** Unlike
  `ProjectReadFileError`, whose schema declares one and whose captured payload
  carries it, all five launcher errors define `message` as an override *getter*
  over their structured fields. The client computes the sentence, so a `message`
  from here would be a property the reference server never sends. The spawn
  error also gained `cause`, which `ExternalLauncherSpawnFields` types as a bare
  `Schema.Defect()` — required, not optional, so an error without it does not
  decode. The test harness's `expect_declared` was relaxed to match: it no
  longer demands a sentence from every refusal, because whether one is on the
  wire is the error schema's business.

One consequence is worth naming: the launcher's error union has **no member for
"the request named no path"**, so a malformed payload is refused as
`ExternalLauncherUnknownEditorError` and the specific complaint is lost. Only
reachable by a client that is not the UI — `editorPreferences.ts` always sends a
target — and the alternative is an error that fails to decode, which costs the
connection rather than the call.

### Not covered automatically

- **A write that fails for a permission reason.** The failure a test can make on
  any machine is writing over a directory, and that is what
  `a_failed_write_leaves_the_file_unchanged` drives. A genuine permission denial
  needs `icacls` on Windows and `chmod` elsewhere — the same call tickets 05 and
  06 made.
- **Actually launching an editor.** `editor::tests::the_file_manager_can_be_started`
  opens a real window on the developer's desktop, so it runs only when
  `LIGHTCODE_TEST_LAUNCH_EDITOR` is set and announces the skip otherwise. What
  is tested unconditionally is everything up to the spawn: which editors are
  advertised, which command each resolves to, and the arguments each family is
  given.
- **The `:line:column` suffix reaching a real editor.** The argument shapes are
  pinned per editor family, but no editor is run to confirm it opens at the
  right line.
