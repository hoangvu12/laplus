# ADR-0013 — The installer writes where the database does not, and the NSIS template is ours to say so

Date: 2026-07-28
Status: Accepted

## Context

Two defaults met and nothing pointed at the other. Tauri's NSIS puts a per-user
install in `$LOCALAPPDATA\${PRODUCTNAME}`; `lightcode_server::config::data_dir`
puts `state.sqlite`, `keybindings.json` and `logs/` in `%LOCALAPPDATA%\lightcode`.
The same directory. A developer's conversations, projects and checkpoints sat
beside `lightcode.exe` in its own program directory, and it took a real install
under ticket 24 to see.

Nothing had lost data, and the reasons were accidents. The uninstaller deletes
the files it installed by name and then calls `RMDir "$INSTDIR"` — the
non-recursive form, which fails silently on a directory that is not empty; had
it been `RMDir /r`, an uninstall would have taken the whole history with it
without asking.

Ticket 30 costed both sides. Moving `data_dir()` to
`%LOCALAPPDATA%\com.lightcode.desktop` is the larger call — it needs a migration
for every existing install, and ticket 23 ticked "application state is stored in
the appropriate per-user location" against the path it would move. Moving the
install touches nobody's existing data.

## Decision

**A per-user install goes to `%LOCALAPPDATA%\Programs\lightcode`, and the NSIS
template that puts it there is vendored at `crates/lightcode-shell/nsis/`.**

The vendored file is tauri-bundler 2.9.4's `installer.nsi` — the bundler inside
the tauri-cli version `CLAUDE.md` tells a fresh clone to install — copied
verbatim and changed in two places, both marked `LIGHTCODE:`:

1. `.onInit`'s per-user default becomes `$LOCALAPPDATA\Programs\${PRODUCTNAME}`.
2. `RestorePreviousInstallLocation` honours a remembered install directory only
   if the application is still in it.

Both are in the `currentUser` branch, which is the mode lightcode ships:
`nsis.installMode` is not set. `perMachine` installs to `$PROGRAMFILES` and
never had this problem; `both` derives a per-user directory from
`MULTIUSER_INSTALLMODE_INSTDIR`, which is untouched and would land on the data
directory again. Rather than patch a third line for a mode nothing uses,
`xtask::install::redirected` refuses to build if `installMode` appears in
`tauri.conf.json` at all — the same argument as everywhere else here, that an
unused path is an unchecked one.

**The second is not a tidy-up, and it was measured rather than reasoned.** With
only the first change, `cargo xtask release --measure-install` installed into
`%LOCALAPPDATA%\lightcode` anyway. NSIS remembers where it last installed in
`Software\lightcode\lightcode` and restores it over the default — and the
uninstaller clears that value only when the "delete application data" checkbox
is ticked, which a silent uninstall never does. So the value outlives the
installation, and on any machine that has ever installed lightcode the moved
default is put straight back. The fix would have read as done and done nothing,
on exactly the machines with the problem. Guarding the restore on the binary
still being there leaves a genuine upgrade landing on top of itself, which is
what that function is for.

A whole template for two lines is the part that needed deciding. `bundle.windows.nsis`
has no install-path option; its only two escape hatches are this file and
`installerHooks`, and the hooks are not one:

- `NSIS_HOOK_PREINSTALL` runs *after* `SetOutPath $INSTDIR`, so a redirect from
  there leaves the old directory already created and the directory page already
  showing a path the installer did not use.
- The hooks file *is* `!include`d early enough — before `InstallDir` and before
  the pages are declared — but nothing it can say survives. The template
  `!define`s the install-dir symbols after the include, and `!define` on an
  existing symbol aborts the build rather than overriding it.

So the choice was between a vendored template and leaving the collision.

## Consequences

- **`%LOCALAPPDATA%\lightcode` holds only what the server writes.** The
  uninstaller's non-recursive `RMDir` stops being what protects the database,
  which is the point: it was protection by accident.
- **The uninstaller's "delete application data" checkbox is still a no-op, and
  that is knowing rather than overlooked.** It removes
  `$LOCALAPPDATA\${BUNDLEID}` — `%LOCALAPPDATA%\com.lightcode.desktop` — which
  this server has never written to. A user who ticks it keeps their
  conversations, and a user who leaves it unticked keeps them too. Failing in
  that direction is the safe one, but it is a control that lies about what it
  does. Making it honest is the `data_dir()` move, with the migration, and that
  is a separate decision.
- **Upstream's template has to be re-vendored by hand on every tauri-cli
  upgrade.** Two guards, neither of which is the whole answer: `cargo test -p
  xtask` fails if either changed line is gone or upstream's default is back, and
  `cargo xtask release` refuses to build in the same case. Neither can tell you
  upstream changed something *else*, so the re-vendoring instruction in the
  file's header says to read the diff.
- **`xtask::install` is now the one place that knows where lightcode installs.**
  The static check and the footprint measurement live together deliberately —
  ticket 30 is what happens when the two facts live apart — and
  `--measure-install` fails if a real install does not land where the template
  says.
- **Existing installs are not migrated, and keep the collision until they are
  uninstalled.** An upgrade over `%LOCALAPPDATA%\lightcode` still lands there:
  the binary is present, so the remembered location is honoured. Nothing about
  those installs gets worse, and no data moves. Uninstalling and installing
  again is the migration, and it is the developer's to run.
- **The footprint measurement got simpler.** It weighed *what the installer
  added* against a snapshot taken beforehand, because the directory already had
  a developer's database in it. It now weighs the directory.
