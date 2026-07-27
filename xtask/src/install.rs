//! Running the installer to find out what it actually costs a disk.
//!
//! Opt-in, because everything here writes to the machine doing the build. The
//! alternative — inferring the footprint from the files the bundle ships — is
//! in `main.rs` and is what runs by default.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::notice;
use crate::report::{Footprint, Source};
use crate::tree::walk;
use crate::{run, weigh};

/// Tauri's NSIS writes an ordinary uninstall key under the **product name** —
/// `lightcode`, not the bundle identifier.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\lightcode";

/// Install, weigh what appeared, and put the machine back as it was.
pub fn measure(installer: &Path) -> Result<Footprint, String> {
    // The refusal that keeps the rest of this honest, and it is not a nicety.
    //
    // What this measures is what the installer *adds* to a directory, because
    // the directory is not empty: NSIS's per-user default is
    // `%LOCALAPPDATA%\lightcode` and `lightcode_server::config::data_dir` is the
    // same path (ticket 30), so a machine that has run this application has its
    // `state.sqlite` and `logs/` sitting exactly where the installer writes.
    // Weighing everything would bill the artifact for a developer's database.
    //
    // An install *over an install* defeats that completely: every file the
    // installer writes was already there, so "what appeared" is nothing and the
    // figure comes out at zero — inside the target, 318× smaller than upstream,
    // and pure fiction. Refusing is the only version of this that cannot report
    // a number that looks fine and is wrong, which is the whole subject of
    // ticket 24.
    //
    // It also means this never runs an uninstaller it did not cause. Silently
    // removing the copy of lightcode a developer actually uses, in order to
    // print a size, is not a trade a build tool gets to make.
    if let Some(existing) = installed_at() {
        return Err(format!(
            "lightcode is already installed at {}.\n\
             \n\
             This measures what the installer adds to that directory, and an install \
             over an install adds nothing it can see — the figure would come out at zero \
             and look like a triumph. It would also mean uninstalling a copy of lightcode \
             this build did not put there.\n\
             \n\
             Uninstall it first ({}) and run this again, or drop --measure-install to \
             have the footprint inferred from what the bundle ships.",
            existing.display(),
            existing.join("uninstall.exe").display(),
        ));
    }

    // Whatever is in that directory now belongs to the developer, not to this
    // artifact. Taken before the install, since afterwards the two are mixed.
    let theirs = default_directory().map(|directory| files_in(&directory)).unwrap_or_default();

    run(Command::new(installer).arg("/S"))?;

    let directory = installed_at().ok_or_else(|| {
        format!("the installer ran but wrote no InstallLocation under HKCU/HKLM {UNINSTALL_KEY}")
    })?;

    let measured = weigh_added(&directory, &theirs);
    // The strongest form of ticket 24's licence criterion, and the only one
    // about the artifact rather than about the configuration describing it:
    // after a real install, is upstream's notice on the disk.
    let notice_landed = directory.join(notice::NOTICE).exists();

    // Unconditional, and before any `?` below. A build tool that leaves a
    // developer with an application they did not ask to install, because a
    // measurement failed, has done more harm than the measurement was worth.
    let uninstaller = directory.join("uninstall.exe");
    if uninstaller.exists() {
        let _ = run(Command::new(&uninstaller).arg("/S"));
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
            "the installer ran and added nothing to {}. Refusing to report a footprint of zero.",
            directory.display()
        ));
    }

    Ok(Footprint {
        bytes,
        files,
        source: Source::Installed,
    })
}

/// Where lightcode is installed, if it is.
///
/// Asked of the uninstall key rather than assumed, which is both shorter than
/// reproducing NSIS's default and correct if `nsis.installMode` ever changes
/// it — hence both hives: a per-user install records itself in `HKCU` and a
/// per-machine one in `HKLM`.
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

/// Where Tauri's NSIS puts a per-user install unless told otherwise.
fn default_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("lightcode"))
}

/// Weigh everything under `root` that is not in `theirs`.
fn weigh_added(root: &Path, theirs: &HashSet<PathBuf>) -> std::io::Result<(u64, usize)> {
    let mut bytes = 0;
    let mut files = 0;

    walk(root, &mut |path| {
        if !theirs.contains(path) {
            bytes += path.metadata()?.len();
            files += 1;
        }
        Ok(())
    })?;

    Ok((bytes, files))
}

/// Every file under a directory, or nothing if it is not there — which is the
/// ordinary case on a machine installing lightcode for the first time.
fn files_in(root: &Path) -> HashSet<PathBuf> {
    let mut found = HashSet::new();
    let _ = walk(root, &mut |path| {
        found.insert(path.to_path_buf());
        Ok(())
    });
    found
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
