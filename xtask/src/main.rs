//! The build that measures itself.
//!
//! laplus exists for one number. The spec puts it plainly — "the artifact is
//! measured at every build, and 20–30 MB is the target against upstream's 318 MB
//! Windows installer" — and ticket 24 turns that into work: the measurement
//! "becomes part of the build rather than a thing someone checks occasionally".
//!
//! So `cargo xtask release` is how a release of laplus is made. It produces
//! the installer and, in the same run, weighs it, weighs what it installs,
//! counts the Rust, checks that upstream's licence still ships, and writes
//! `docs/artifact-size.md`. Building without measuring is not offered here,
//! because the version of this that gets skipped is the version where someone
//! has to remember.
//!
//! `release` rather than `bundle`, which is what it was first called: `CONTEXT.md`
//! already gives **bundle** to upstream's built web application, and one word for
//! two things in a repository that keeps a domain glossary is a bad trade for a
//! familiar verb.
//!
//! Everything here is process and filesystem work. The judgements — what counts
//! as production code, what "inside the target" means, what the report says —
//! live in [`loc`], [`size`], [`notice`] and [`report`], where they are tested.
//!
//! ```text
//! cargo xtask release [--measure-install]
//!   │
//!   ├─ refuse now if upstream's licence is not shipping
//!   ├─ refuse now if the installer would write into the data directory
//!   ├─ cargo tauri build           → target/release/bundle/nsis/*-setup.exe
//!   ├─ weigh the installer and the binary
//!   ├─ weigh what is installed     → payload, or a real install with the flag
//!   ├─ count crates/laplus-server/src
//!   └─ docs/artifact-size.md, and the same thing on stdout
//! ```
//!
//! **`cargo tauri` is not installed by this workspace and cannot be.** A fresh
//! clone needs `cargo install tauri-cli --version "^2" --locked`, and the first
//! release build then downloads NSIS. Both are recorded in `CLAUDE.md`.

mod install;
mod loc;
mod notice;
mod report;
mod size;
mod tree;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use report::Measurements;

const SHELL: &str = "crates/laplus-shell";
const SERVER_SOURCE: &str = "crates/laplus-server/src";
const REPORT: &str = "docs/artifact-size.md";
const USAGE: &str = "usage: cargo xtask release [--measure-install]";

fn main() -> ExitCode {
    let mut measure_install = false;

    // Unknown arguments are refused rather than ignored. The one that matters
    // is a mistyped `--measure-install`: ignoring it would produce a report
    // that looks the same and quietly carries the weaker figure, which is the
    // exact failure this whole tool is built to avoid.
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "release" => {}
            "--measure-install" => measure_install = true,
            unknown => {
                eprintln!("xtask: {unknown} is not an argument this understands.\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    match release(measure_install) {
        Ok(written) => {
            println!("\n{written}\n\nwritten to {REPORT}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("xtask: {problem}");
            ExitCode::FAILURE
        }
    }
}

fn release(measure_install: bool) -> Result<String, String> {
    let root = repo_root()?;

    // Before the build rather than after it: a licence problem is a reason not
    // to distribute the thing, so finding out once it exists is finding out too
    // late.
    let config = read(&root.join(SHELL).join("tauri.conf.json"))?;
    let text = read(&root.join(notice::NOTICE))?;
    notice::retained(&config, &text)
        .map_err(|missing| format!("upstream's licence is not in the artifact: {missing:?}"))?;

    // The same argument, for the same reason. An installer that writes over a
    // developer's database is a reason not to hand it to anyone, and this is
    // three seconds against the three minutes of finding out afterwards.
    let template = read(&root.join(SHELL).join(install::TEMPLATE))?;
    install::redirected(&config, &template).map_err(|astray| {
        format!("this build would install on top of the developer's data (ticket 30): {astray:?}")
    })?;

    run(Command::new("cargo").arg("tauri").arg("build").current_dir(root.join(SHELL)))?;

    let installer = installer_path(&root)?;
    let binary = root.join("target/release/laplus.exe");

    let measured = Measurements {
        installer: weigh(&installer)?,
        binary: weigh(&binary)?,
        installed: if measure_install {
            install::measure(&installer)?
        } else {
            install::payload(&root, &binary)?
        },
        server: loc::breakdown_tree(&root.join(SERVER_SOURCE))
            .map_err(|error| format!("cannot read {SERVER_SOURCE}: {error}"))?,
    };

    let written = report::render(&measured);
    let destination = root.join(REPORT);
    fs::write(&destination, &written)
        .map_err(|error| format!("cannot write {}: {error}", destination.display()))?;

    Ok(written)
}

/// The repository root, from this crate's own location rather than from the
/// working directory — `cargo xtask` can be run from anywhere in the tree.
fn repo_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask is not in a workspace".to_string())
}

fn installer_path(root: &Path) -> Result<PathBuf, String> {
    let directory = root.join("target/release/bundle/nsis");
    let mut installers: Vec<PathBuf> = fs::read_dir(&directory)
        .map_err(|error| format!("no bundle at {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "exe"))
        .collect();
    installers.sort();

    // The version is in the file name, so a stale bundle from an earlier
    // version would sit here beside the new one and could be measured instead.
    match installers.len() {
        1 => Ok(installers.remove(0)),
        0 => Err(format!("no installer in {}", directory.display())),
        _ => Err(format!(
            "{} installers in {}. Which one is this build's is not knowable — \
             delete the directory and build again.",
            installers.len(),
            directory.display()
        )),
    }
}

pub fn weigh(path: &Path) -> Result<u64, String> {
    fs::metadata(path)
        .map(|found| found.len())
        .map_err(|error| format!("cannot weigh {}: {error}", path.display()))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

pub fn run(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("cannot run {:?}: {error}", command.get_program()))?;

    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{:?} failed: {status}", command.get_program()))
}
