# 30 — The installer and the database want the same directory

**What is wrong:** lightcode installs itself into `%LOCALAPPDATA%\lightcode`,
and stores `state.sqlite`, `keybindings` and `logs/` in `%LOCALAPPDATA%\lightcode`.
The same directory. A developer's conversations, projects and checkpoints sit
beside `lightcode.exe` in its own program directory.

Two independent defaults met: Tauri's NSIS per-user install path is
`$LOCALAPPDATA\${PRODUCTNAME}`, and `lightcode_server::config::data_dir` is
`%LOCALAPPDATA%\lightcode`. Neither is wrong on its own and nothing points at
the other, which is why it took a real install to see.

**Status:** done

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

## Comments

### 2026-07-27 — triage. Option 1 now; option 2 is a separate decision

**Move the install**, to `%LOCALAPPDATA%\Programs\lightcode`. It is where
per-user applications more usually go, it separates the two directories, and it
touches nobody's existing data — which is the whole reason to prefer it over
moving `data_dir()`. Ticket 23's "application state is stored in the appropriate
per-user location" stays true rather than needing revisiting.

**What this deliberately does not fix, and the ticket must say so when it
lands:** the uninstaller's "delete application data" checkbox still points at
`%LOCALAPPDATA%\com.lightcode.desktop`, which lightcode still never writes to.
After option 1 the checkbox is _still_ a no-op. That is the safe direction to
fail in — a user who ticks it keeps their conversations rather than losing them
silently — but it is a control that lies about what it does, and shipping it
knowingly is a choice rather than an oversight. Record it in the ticket, not just
here.

Option 2 — moving `data_dir()` to `%LOCALAPPDATA%\com.lightcode.desktop`, which
is what would make that checkbox mean what it says — needs a migration for every
existing install and is the larger call. Not folded in here. If it is ever taken,
the migration is the whole of the work; the path change is one line.

**One thing to verify rather than assume:** that `RMDir "$INSTDIR"` is
non-recursive is what currently protects the database, and after this change the
install directory no longer holds it, so that protection stops mattering — which
is good. But confirm the new install path is what NSIS actually uses before
trusting it; the ticket above exists precisely because two defaults were assumed
to point at different places and did not.

### 2026-07-28 — agent. Done

Option 1, and "the path change is one line" was wrong twice over. It is a
**vendored NSIS template**, `crates/lightcode-shell/nsis/installer.nsi`, changed
in **two** places. `docs/adr/0013` carries the reasoning; the short version:

- **`bundle.windows.nsis` has no install-path option.** Its escape hatches are a
  custom template and `installerHooks`, and the hooks cannot do it —
  `NSIS_HOOK_PREINSTALL` runs after `SetOutPath $INSTDIR`, so a redirect from
  there would leave the old directory already created and the directory page
  already showing a path the installer did not use. Vendoring 977 lines of
  upstream's template to change two of them is the cost, and the file's header
  says how to re-vendor it when tauri-cli is upgraded.
- **Moving the default was not enough, and the verification step this ticket
  asked for is the only reason we know.** With the moved default alone,
  `--measure-install` installed into `%LOCALAPPDATA%\lightcode` anyway. NSIS
  remembers its last install directory in `Software\lightcode\lightcode` and
  `RestorePreviousInstallLocation` restores it over the default — and the
  uninstaller clears that value only when the "delete application data" checkbox
  is ticked, which `/S` never does. So the value outlives the installation, and
  ticket 24's own install-and-uninstall had left one behind pointing at the data
  directory. On every machine that has ever installed lightcode, the fix would
  have been inert while reading as done. The second change guards the restore on
  the binary still being there.
- **Both changes are in the `currentUser` branch**, which is the mode this ships
  — `nsis.installMode` is not set. `perMachine` goes to `$PROGRAMFILES` and never
  had this problem; `both` would compute a per-user directory from a third line
  that still names the data directory. That line is left alone and
  `redirected()` refuses to build if `installMode` appears in the configuration
  at all, rather than patching a mode nothing uses and nothing checks.

**What this deliberately does not fix, recorded here as triage asked:** the
uninstaller's "delete application data" checkbox is **still a no-op**. It removes
`$LOCALAPPDATA\${BUNDLEID}` — `%LOCALAPPDATA%\com.lightcode.desktop` — which this
server has never written to. A user who ticks it keeps their conversations; a
user who leaves it unticked keeps them too. That is the safe direction to fail
in, and it is still a control that lies about what it does. Making it honest is
option 2, `data_dir()` plus a migration, and it is not folded in here.

**Existing installs are not migrated.** An upgrade over `%LOCALAPPDATA%\lightcode`
still lands there — the binary is present, so the remembered location is
honoured, which is what that function is for. Those installs keep the collision
until they are uninstalled and reinstalled. No data moves, either way.

#### What a real install proved

`cargo xtask release --measure-install`, on the machine that found the bug, with
ticket 24's stale remembered location deliberately left in place:

|                   |                                                                                                                                            |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Install directory | `%LOCALAPPDATA%\Programs\lightcode`, **3 files**, 24.33 MB                                                                                 |
| Data directory    | `%LOCALAPPDATA%\lightcode`, `state.sqlite` only, 86,016 bytes, untouched                                                                   |
| Uninstall         | the install directory is **gone entirely** — `RMDir "$INSTDIR"` now succeeds, because there is nothing of the developer's in it to stop it |

The last row is the ticket's first bullet, inverted. The database used to be
saved by a `RMDir` that happened not to recurse; now nothing needs saving, and
the non-recursive form removes the whole directory because the whole directory
is ours.

#### Where this is enforced

`xtask::install` is now the single place that knows where lightcode installs —
which is the lesson of this ticket, since it was two places that did not know
about each other. Three guards, and they catch different things:

- `cargo test -p xtask` — `redirected()` fails if `tauri.conf.json` stops naming
  the template, if the moved default goes missing or upstream's comes back, or
  if the restore loses its guard. One of its tests reads the real configuration
  and the real template rather than a fixture, which is the one that will
  actually catch something.
- `cargo xtask release` — the same check before the build, beside the licence
  one and for the same reason: an installer that writes over a developer's
  database is a reason not to hand it to anyone.
- `--measure-install` — refuses to report a footprint if a real install did not
  land where the template says. This is the one that caught the restore.

The footprint measurement got simpler on the way: it used to weigh _what the
installer added_ against a snapshot taken beforehand, because the directory
already had a developer's database in it. It weighs the directory now.
