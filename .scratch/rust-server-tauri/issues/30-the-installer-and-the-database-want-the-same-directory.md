# 30 — The installer and the database want the same directory

**What is wrong:** lightcode installs itself into `%LOCALAPPDATA%\lightcode`,
and stores `state.sqlite`, `keybindings` and `logs/` in `%LOCALAPPDATA%\lightcode`.
The same directory. A developer's conversations, projects and checkpoints sit
beside `lightcode.exe` in its own program directory.

Two independent defaults met: Tauri's NSIS per-user install path is
`$LOCALAPPDATA\${PRODUCTNAME}`, and `lightcode_server::config::data_dir` is
`%LOCALAPPDATA%\lightcode`. Neither is wrong on its own and nothing points at
the other, which is why it took a real install to see.

**Status:** needs-triage

Found by ticket 24, on this machine, by installing the built artifact and
looking at what was in the directory afterwards.

## What actually happens

Nothing loses data today, and the reasons are all accidents:

- **The uninstaller does not delete the database.** It `Delete`s the files it
  installed by name and then calls `RMDir "$INSTDIR"` — the non-recursive
  form, which fails silently on a directory that is not empty. Verified:
  after `uninstall.exe /S`, `state.sqlite` was still there and everything else
  was gone. Had that been `RMDir /r`, an uninstall would have taken the
  developer's entire history with it without asking.
- **The uninstaller's "delete application data" checkbox does nothing.** It
  removes `$LOCALAPPDATA\${BUNDLEID}` — `%LOCALAPPDATA%\com.lightcode.desktop`
  — which this server has never written to. So a user who ticks the box to
  clear their data keeps all of it, and a user who leaves it unticked also
  keeps all of it.

So the present state is: the wrong directory is protected by a `RMDir` that
happens not to recurse, and the option meant to clean up points somewhere
lightcode does not use. Both halves are one upstream default away from being
the other thing.

## Worth deciding rather than fixing on sight

Whichever side moves, existing installs have state in the old place:

1. **Move the install** — `nsis.installMode` or an explicit install path, e.g.
   `%LOCALAPPDATA%\Programs\lightcode`, which is where per-user applications
   more usually go. Leaves every developer's data where it is.
2. **Move the data** — `data_dir()` to `%LOCALAPPDATA%\com.lightcode.desktop`,
   which is also what would make the uninstaller's checkbox mean what it says.
   Needs a migration, and ticket 23 ticked "application state is stored in the
   appropriate per-user location" against the current path.

Option 1 is smaller and touches nothing anyone has. Option 2 is the one that
makes the uninstaller honest. They are not exclusive.

Ticket 24 did not act on this: it is not a size question, and changing where a
developer's database lives is not something to slip into a measuring run.
