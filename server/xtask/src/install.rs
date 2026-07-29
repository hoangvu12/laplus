//! Where laplus installs, and what it costs a disk.
//!
//! One module for both because ticket 30 was what happens when they are two.
//! Tauri's NSIS default put the application in `%LOCALAPPDATA%\laplus` and
//! `laplus_server::config::data_dir` put the developer's database in
//! `%LOCALAPPDATA%\laplus`; neither default was wrong on its own and
//! nothing pointed at the other, so it took a real install to see. Everything
//! in this repository that knows where the installer writes is now here, and
//! [`redirected`] is checked by `cargo test` and again before every release
//! build.
//!
//! Weighing the result is opt-in, because it writes to the machine doing the
//! build. The alternative — inferring the footprint from the files the bundle
//! ships — is [`payload`], and is what runs by default.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::notice;
use crate::report::{Footprint, Source};
use crate::tree::walk;
use crate::{run, weigh};

/// Tauri's NSIS writes an ordinary uninstall key under the **product name** —
/// `laplus`, not the bundle identifier.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\laplus";

/// The directory a per-user install is moved into, under `%LOCALAPPDATA%`.
///
/// Where per-user applications more usually go, and the point of it here is
/// the one thing it is *not*: `%LOCALAPPDATA%\laplus`, which is
/// `laplus_server::config::data_dir` and holds `state.sqlite`,
/// `keybindings.json` and `logs/`.
const PROGRAMS: &str = "Programs";

/// The directory the installer makes inside [`PROGRAMS`]. NSIS writes
/// `${PRODUCTNAME}` and takes it from `tauri.conf.json`, so this is the same
/// name as in [`UNINSTALL_KEY`] and for the same reason: what the *bundle*
/// calls this application, resolved.
const PRODUCT: &str = "laplus";

/// The NSIS template that puts it there, relative to the shell crate.
pub const TEMPLATE: &str = "nsis/installer.nsi";

/// The second half of the patch, and the half that reads like a detail.
///
/// `RestorePreviousInstallLocation` puts `$INSTDIR` back to whatever
/// `Software\laplus\laplus` remembers, and an uninstall leaves that value
/// behind unless the "delete application data" checkbox was ticked. So a
/// machine that installed laplus once keeps installing it in the same place
/// no matter what the default says. Guarding the restore on the binary still
/// being there is what makes the moved default reach anybody.
const RESTORE_GUARD: &str = r#"${AndIf} ${FileExists} "$4\${MAINBINARYNAME}.exe""#;

/// The first half: the per-user default, as the template spells it.
///
/// Built rather than written out, so that the one place naming [`PROGRAMS`] is
/// the one place the tests compare against.
fn moved_default() -> String {
    format!(r#"StrCpy $INSTDIR "$LOCALAPPDATA\{PROGRAMS}\${{PRODUCTNAME}}""#)
}

/// What has gone wrong with where laplus installs.
#[derive(Debug, PartialEq, Eq)]
pub enum Astray {
    /// `tauri.conf.json` no longer names the vendored template, so the bundler
    /// renders upstream's — and upstream's installs over the database.
    TemplateUnused,
    /// The bundle no longer calls this application [`PRODUCT`], so the
    /// directory NSIS makes is not the one [`expected_directory`] watches. The
    /// same shape as ticket 30 itself, in miniature: two configurations that
    /// have to agree and nothing making them.
    ProductRenamed,
    /// `nsis.installMode` has been set. The patch moves the **currentUser**
    /// default and nothing else — upstream's `perMachine` path is
    /// `$PROGRAMFILES`, which was never the problem, and its `both` path
    /// computes a per-user directory from `MULTIUSER_INSTALLMODE_INSTDIR`,
    /// which is still `${PRODUCTNAME}` and still the data directory. Refused
    /// rather than patched, because a mode nothing uses is a mode nothing
    /// checks.
    ///
    /// **The match is on the whole file**, so a key of this name anywhere in
    /// `tauri.conf.json` refuses the build — and ticket 74 found one that is
    /// unrelated: `plugins.updater.windows.installMode` is how *an update*
    /// presents itself while it runs, not where the installer writes. That
    /// setting was dropped rather than this check being narrowed, because its
    /// value there was the default anyway and a guard on where somebody's
    /// database lives is not the thing to make cleverer for a convenience.
    InstallModeChanged,
    /// The template is named but no longer moves the per-user default. What a
    /// re-vendoring from upstream looks like: the file is still there, still
    /// builds, and has quietly lost the one line it exists for.
    DefaultNotMoved,
    /// The default moved and the template restores a remembered install
    /// location without checking that anything is installed at it — so on any
    /// machine that has ever installed laplus, the move has no effect. The
    /// failure that reads as a fix, and the one that was actually measured.
    RestoreUnguarded,
}

/// Whether the installer still writes somewhere other than the data directory.
///
/// Read as text rather than as JSON, for the reason [`notice::retained`] gives:
/// the question is narrow and a JSON parser is a dependency this crate has no
/// other use for.
pub fn redirected(config: &str, template: &str) -> Result<(), Astray> {
    if !config.contains(TEMPLATE) {
        return Err(Astray::TemplateUnused);
    }
    if !config.contains(&format!(r#""productName": "{PRODUCT}""#)) {
        return Err(Astray::ProductRenamed);
    }
    // Quoted, and the quotes are load-bearing: `webviewInstallMode` is two
    // lines above `nsis` in the file this reads, and differs only in a capital
    // `I`.
    if config.contains(r#""installMode""#) {
        return Err(Astray::InstallModeChanged);
    }

    // The whole of the patch this repository carries against upstream's
    // template, asserted as the literal lines, because that is the granularity
    // at which it goes missing.
    //
    // The moved default is checked both ways round: the first half catches the
    // line being edited away, the second catches upstream's coming back beside
    // it in some branch this does not read.
    let upstream = r#"StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}""#;

    if !template.contains(&moved_default()) || template.contains(upstream) {
        return Err(Astray::DefaultNotMoved);
    }

    if !template.contains(RESTORE_GUARD) {
        return Err(Astray::RestoreUnguarded);
    }

    Ok(())
}

/// Install, weigh what landed, and put the machine back as it was.
pub fn measure(installer: &Path) -> Result<Footprint, String> {
    // This never runs an uninstaller it did not cause. Silently removing the
    // copy of laplus a developer actually uses, in order to print a size,
    // is not a trade a build tool gets to make — and an install over an
    // install is also a directory holding two builds' files, which is not the
    // thing the report claims to have weighed.
    if let Some(existing) = installed_at() {
        return Err(format!(
            "laplus is already installed at {}.\n\
             \n\
             This installs, weighs the directory, and uninstalls again — which here would \
             mean weighing a mixture of two builds and then removing a copy of laplus \
             this build did not put there.\n\
             \n\
             Uninstall it first ({}) and run this again, or drop --measure-install to \
             have the footprint inferred from what the bundle ships.",
            existing.display(),
            existing.join("uninstall.exe").display(),
        ));
    }

    run(Command::new(installer).arg("/S"))?;

    let directory = installed_at().ok_or_else(|| {
        format!("the installer ran but wrote no InstallLocation under HKCU/HKLM {UNINSTALL_KEY}")
    })?;

    let measured = weigh_tree(&directory);
    // The strongest form of ticket 24's licence criterion, and the only one
    // about the artifact rather than about the configuration describing it:
    // after a real install, is upstream's notice on the disk.
    let notice_landed = directory.join(notice::NOTICE).exists();
    // Ticket 30's own criterion, and the reason it is asked of a real install
    // rather than of the template: the template says where NSIS *should* put
    // this, and that ticket exists because two configurations were assumed to
    // point at different places and did not. `None` only if this machine has no
    // `%LOCALAPPDATA%`, in which case there is nothing to compare against.
    let elsewhere = expected_directory().filter(|expected| &directory != expected);

    // Unconditional, and before any `?` below. A build tool that leaves a
    // developer with an application they did not ask to install, because a
    // measurement failed, has done more harm than the measurement was worth.
    let uninstaller = directory.join("uninstall.exe");
    if uninstaller.exists() {
        let _ = run(Command::new(&uninstaller).arg("/S"));
    }

    if let Some(expected) = elsewhere {
        return Err(format!(
            "installed to {}, and the template says {}. Until those agree, nothing here \
             knows whether the application is sitting on top of the developer's database \
             (ticket 30).",
            directory.display(),
            expected.display(),
        ));
    }

    if !notice_landed {
        return Err(format!(
            "installed to {} without {}. The configuration says it ships, and it did not.",
            directory.display(),
            notice::NOTICE
        ));
    }

    let (bytes, files) = measured
        .map_err(|error| format!("installed to {} but could not weigh it: {error}", directory.display()))?;

    if files == 0 {
        return Err(format!(
            "the installer ran and left nothing in {}. Refusing to report a footprint of zero.",
            directory.display()
        ));
    }

    Ok(Footprint {
        bytes,
        files,
        source: Source::Installed,
    })
}

/// Where laplus is installed, if it is.
///
/// Asked of the uninstall key rather than assumed, which is shorter than
/// reproducing NSIS's default and reads a per-machine install too — hence both
/// hives: a per-user install records itself in `HKCU` and a per-machine one in
/// `HKLM`. What it finds is checked against [`expected_directory`], which is
/// per-user only; [`Astray::InstallModeChanged`] is what keeps that honest.
fn installed_at() -> Option<PathBuf> {
    for hive in ["HKCU", "HKLM"] {
        let read = Command::new("reg")
            .args(["query", &format!(r"{hive}\{UNINSTALL_KEY}"), "/v", "InstallLocation"])
            .output()
            .ok()?;

        let found = String::from_utf8_lossy(&read.stdout)
            .lines()
            .find_map(|line| line.split_once("REG_SZ"))
            // The value is stored quoted, and a `"C:\…"` is not a path.
            .map(|(_, location)| PathBuf::from(location.trim().trim_matches('"')));

        if found.is_some() {
            return found;
        }
    }

    None
}

/// Where the vendored template sends a per-user install on this machine.
fn expected_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| PathBuf::from(base).join(PROGRAMS).join(PRODUCT))
}

/// Weigh everything under `root`.
///
/// Everything, and not "everything the installer added since a snapshot taken
/// beforehand", which is what this was until ticket 30 moved the install out of
/// the data directory. The subtraction existed because the two shared a
/// directory and the figure would otherwise have billed the artifact for a
/// developer's database. They no longer share one, and what is left in the
/// install directory is this artifact by definition.
fn weigh_tree(root: &Path) -> std::io::Result<(u64, usize)> {
    let mut bytes = 0;
    let mut files = 0;

    walk(root, &mut |path| {
        bytes += path.metadata()?.len();
        files += 1;
        Ok(())
    })?;

    Ok((bytes, files))
}

/// What the bundle ships, weighed where it was built.
///
/// The approximation that runs unless the machine is volunteered: no install,
/// so it cannot see the uninstaller NSIS generates during one.
pub fn payload(root: &Path, binary: &Path) -> Result<Footprint, String> {
    // Written out rather than read from `bundle.resources`, and the cost of
    // that is real: a third resource added to `tauri.conf.json` would have to
    // be added here too, or this figure would quietly stop counting it. That
    // cost is accepted because this path is the approximation — the report
    // labels it as inferred, and `--measure-install` is the answer for anyone
    // who needs the number to be true.
    let shipped = [binary.to_path_buf(), root.join(notice::NOTICE)];

    let mut bytes = 0;
    for file in &shipped {
        bytes += weigh(file)?;
    }

    Ok(Footprint {
        bytes,
        files: shipped.len(),
        source: Source::Payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
      "productName": "laplus",
      "bundle": {
        "windows": {
          "nsis": { "template": "nsis/installer.nsi" }
        }
      }
    }"#;

    /// A template that passes: both halves of the patch, and nothing else.
    fn patched() -> String {
        format!("    {}\n{RESTORE_GUARD}\n", moved_default())
    }

    /// The one that will actually catch something. A release build checks this
    /// too, but a release build is three minutes and a decision, and `cargo
    /// test` is where someone finds out that an edit to `tauri.conf.json` or a
    /// re-vendoring of the template put laplus back on top of the database.
    #[test]
    fn the_installer_this_repository_ships_writes_outside_the_data_directory() {
        let shell = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask is in the workspace")
            .join("crates/laplus-shell");

        let config =
            std::fs::read_to_string(shell.join("tauri.conf.json")).expect("the shell's bundle configuration");
        let template = std::fs::read_to_string(shell.join(TEMPLATE)).expect("the vendored NSIS template");

        assert_eq!(redirected(&config, &template), Ok(()));
    }

    /// The failure that looks like nothing: the template file is still in the
    /// repository, still correct, and no longer reaches the bundler.
    #[test]
    fn a_template_the_bundle_does_not_use_is_not_a_redirection() {
        assert_eq!(
            redirected(r#"{ "bundle": { "windows": {} } }"#, &patched()),
            Err(Astray::TemplateUnused)
        );
    }

    /// What a re-vendoring from upstream looks like from here. Both spellings,
    /// because the second is the one a careless merge leaves behind: the moved
    /// line present *and* upstream's back beside it, where whichever NSIS
    /// reaches last wins and nothing in this repository would notice.
    #[test]
    fn a_template_carrying_upstreams_default_is_not_a_redirection() {
        let upstream = "    StrCpy $INSTDIR \"$LOCALAPPDATA\\${PRODUCTNAME}\"\n";

        assert_eq!(
            redirected(CONFIG, &format!("{upstream}{RESTORE_GUARD}")),
            Err(Astray::DefaultNotMoved)
        );
        assert_eq!(
            redirected(CONFIG, &format!("{}{upstream}", patched())),
            Err(Astray::DefaultNotMoved)
        );
    }

    /// Both spellings of the mistake ticket 30 is itself an instance of: two
    /// configurations that have to agree about a name, and nothing making
    /// them. A rename would otherwise surface as `--measure-install` failing
    /// with a ticket 30 error on a machine that does not have a ticket 30
    /// problem, and `installMode` would not surface at all.
    #[test]
    fn a_bundle_this_module_no_longer_describes_is_refused_rather_than_assumed() {
        assert_eq!(
            redirected(&CONFIG.replace("laplus", "lamplight"), &patched()),
            Err(Astray::ProductRenamed)
        );
        assert_eq!(
            redirected(
                &CONFIG.replace(r#""nsis": {"#, r#""nsis": { "installMode": "both","#),
                &patched()
            ),
            Err(Astray::InstallModeChanged)
        );
    }

    /// Half the patch is not half the fix. A moved default with upstream's
    /// unguarded restore behind it installs into the data directory on every
    /// machine that has ever installed laplus — which is a fix that reads
    /// as done and was measured doing nothing.
    #[test]
    fn moving_the_default_without_guarding_the_restore_is_not_a_redirection() {
        let moved = "    StrCpy $INSTDIR \"$LOCALAPPDATA\\Programs\\${PRODUCTNAME}\"\n";

        assert_eq!(redirected(CONFIG, moved), Err(Astray::RestoreUnguarded));
    }
}
