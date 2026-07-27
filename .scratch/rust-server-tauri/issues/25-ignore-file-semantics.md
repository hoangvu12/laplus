# 25 — Ignore-file semantics for the file tree

**What to build:** A developer opens a JavaScript project and sees their own
source in the file tree, not a thousand packages from `node_modules`. Whatever
the repository already says should be ignored is ignored, so the tree matches
what the developer thinks is in the project.

**Blocked by:** 06 (Filesystem browse and file tree), which is what this
corrects.

**Status:** done

- [x] A JavaScript project with `node_modules` renders a tree whose own source
      is complete
- [x] Files the repository ignores do not appear in the tree
- [x] A folder that is not a repository still lists
- [x] Whatever mechanism is chosen, the artifact size is measured against the
      spec's 20–30 MB target and recorded
- [x] Tests drive the ignored-file behaviour through the socket boundary

## Answer

**Shelling out to `git ls-files`**, with ticket 06's walk kept as the fallback.
Built as part of ticket 07 rather than after it — see that ticket's comments for
why search made this urgent rather than optional.

The scan is:

```
git -C <root> ls-files --cached --others --exclude-standard -z
git -C <root> ls-files --deleted -z          # subtracted
```

and the walk runs unchanged when git is absent or the folder is not a
repository.

### Why this over the alternatives

- **The `ignore` crate** would be correct in non-repositories too, and brings
  symlink-loop handling with it. It also pulls `globset`, `regex-automata`,
  `aho-corasick` and `bstr` — roughly doubling a dependency graph that is
  currently ten direct crates, in a project whose entire reason for existing is
  a 20–30 MB artifact against upstream's 318 MB.
- **A hand-written matcher** was rejected in ticket 06 and is still rejected:
  negations, anchoring and nested ignore files have enough subtlety that an
  approximate implementation hides files silently.
- **A fixed exclusion list** is a guess, and a wrong guess hides a directory the
  user wanted.

Asking git costs one process spawn per scan — about 100 ms on this repository's
6,000-file vendored checkout — and that cost is already off the read loop
(`rpc::Deferred`) and paid once per project open rather than per keystroke,
because `filesystem::Index` holds the result.

### Artifact size: 3.8 MB, unchanged by this work

`cargo build --release` produces a **3.8 MB** `lightcode-server.exe` on Windows,
against the spec's 20–30 MB target for the whole bundled artifact. That leaves
the budget almost entirely to the Tauri shell and the UI assets, which tickets
23 and 24 add and measure.

No dependency was added, so the number is the same before and after —
`Cargo.lock` is untouched by this work. The spec already commits to shelling out
to the `git` binary for tickets 19–21, so this takes a dependency the project
had decided on rather than adding one. For contrast, the `ignore` crate would
have brought `globset`, `regex-automata`, `aho-corasick` and `bstr` with it;
that cost was avoided rather than measured.

### What it costs

- **An empty directory does not appear in the tree.** Git names files and never
  the folders holding them, so ancestors are synthesised from the file list; a
  folder with nothing in it has nothing to synthesise from. Upstream's indexer
  shows one. The tree is otherwise identical.
- **Submodules are not descended into.** `git ls-files` reports a submodule as a
  single gitlink entry rather than listing its contents. A developer working
  inside a submodule sees it as an empty folder. Not addressed here; it needs a
  decision about whether a submodule is part of the project or a project of its
  own, which is the git tickets' territory.
- **A repository whose git binary is missing falls back to the walk**, and
  therefore back to ticket 06's behaviour, ignore semantics and all. That is the
  degraded case rather than a failure, and it is the same fallback a folder that
  is not a repository takes.

### Where it is pinned

- `filesystem::tests::a_repository_does_not_list_what_it_ignores`
- `filesystem::tests::a_file_deleted_without_staging_is_not_listed`
- `filesystem::tests::a_folder_that_is_not_a_repository_falls_back_to_the_walk`
- `filesystem::tests::directories_are_inferred_from_the_files_git_names`
- `socket_files.rs::a_repository_offers_neither_ignored_files_nor_its_own_git_directory`,
  which is the socket-boundary one and checks that the tree and the search hide
  the same files — otherwise the composer would offer a mention the tree has no
  row for.
