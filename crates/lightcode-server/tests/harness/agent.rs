//! A stand-in for the `claude` binary.
//!
//! The suite has to run offline, for free, and on a machine that has never had
//! Claude Code installed — spec story 61, and the last criterion of ticket 09 in
//! so many words. So every test that needs an agent binary writes one: a file
//! this platform agrees is a program, which answers `--version` however the test
//! needs it answered.
//!
//! `provider.rs` has a smaller copy of this for its own unit tests, because an
//! integration test is a separate crate and cannot see the library's
//! `#[cfg(test)]` items. That much duplication is the language's rather than a
//! choice — but *duplicated tests* would not be, so the two files divide the work
//! rather than covering the same ground twice: the rules are driven in
//! `provider.rs`, what the UI observes is driven in `socket_provider.rs`, and its
//! header says which is which.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A fake agent binary in a directory of its own, so the directory can be handed
/// to a lookup as if it were on `PATH`.
pub struct FakeAgent {
    directory: tempfile::TempDir,
}

impl FakeAgent {
    /// A binary that reports `version` the way the real one does —
    /// `2.1.220 (Claude Code)`, the version followed by the product name.
    pub fn reporting(version: &str) -> FakeAgent {
        FakeAgent::saying(&format!("echo {version} (Claude Code)"))
    }

    /// A binary that exits non-zero, like an install whose runtime is broken.
    pub fn failing() -> FakeAgent {
        FakeAgent::saying(match cfg!(windows) {
            true => "exit /b 1",
            false => "exit 1",
        })
    }

    /// A binary running one line of the platform's own script language.
    pub fn saying(script: &str) -> FakeAgent {
        let agent = FakeAgent {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };
        let path = agent.path();

        if cfg!(windows) {
            // A `.cmd` rather than an `.exe`, because a test cannot compile one.
            // `std::process::Command` runs a batch file through `cmd.exe`, which
            // is precisely why this server does not need upstream's npm-shim
            // resolution — see `provider`'s module docs.
            std::fs::write(&path, format!("@echo off\r\n{script}\r\n"))
                .expect("writes the batch file");
        } else {
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n"))
                .expect("writes the shell script");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                    .expect("sets the mode");
            }
        }
        agent
    }

    /// Where the binary is, under the name the resolver looks for.
    pub fn path(&self) -> PathBuf {
        self.directory.path().join(match cfg!(windows) {
            true => "claude.cmd",
            false => "claude",
        })
    }

    /// The path as the `binaryPath` setting would spell it.
    pub fn configured(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    /// The directory to put on a lookup's `PATH`.
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// A path inside this directory that does not exist — a `binaryPath` setting
    /// that has outlived its install.
    pub fn stale_path(&self) -> String {
        self.directory
            .path()
            .join("moved-away")
            .join(match cfg!(windows) {
                true => "claude.cmd",
                false => "claude",
            })
            .to_string_lossy()
            .into_owned()
    }
}
