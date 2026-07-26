//! Making a project on disk, and reading the answers a socket gives about one.
//!
//! Every ticket from 06 onwards drives methods that take a workspace root, so
//! the fixture and the two accessors are here rather than in each test file —
//! `mod.rs` says the harness is where later tickets add calls rather than
//! plumbing, and these are plumbing.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A throwaway project, written out from a list of paths.
///
/// A path ending in `/` is an empty directory; anything else is a file, and
/// `contents` is what goes in every one of them. Tests that care what is *in* a
/// file write it themselves — most only care that it is there.
pub struct Workspace {
    directory: tempfile::TempDir,
}

impl Workspace {
    pub fn with(paths: &[&str]) -> Workspace {
        let workspace = Workspace {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };
        for path in paths {
            workspace.put(path, "contents");
        }
        workspace
    }

    /// Write one file, creating whatever directories it needs.
    pub fn put(&self, path: &str, contents: &str) -> PathBuf {
        let full = self.path().join(path.trim_end_matches('/'));
        if path.ends_with('/') {
            std::fs::create_dir_all(&full).expect("creates the directory");
        } else {
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("creates the parents");
            std::fs::write(&full, contents).expect("writes the file");
        }
        full
    }

    pub fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.path().join(path)).expect("the file is readable")
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    /// The root as the client spells it in a `cwd` or `partialPath`.
    pub fn cwd(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    /// The root with the platform's separator on the end — the picker's "list
    /// this directory" spelling.
    pub fn inside(&self) -> String {
        inside(self.path())
    }

    /// Make this workspace a git repository.
    ///
    /// The scan asks git what is in a project, so a test about ignore
    /// semantics has to have one. Not skippable: the spec commits to shelling
    /// out to `git`, so a machine without it cannot run this suite anyway.
    pub fn init_repository(&self) -> &Workspace {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(self.path())
            .arg("init")
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git init failed");
        self
    }
}

/// A path with the platform's separator on the end.
pub fn inside(path: &Path) -> String {
    format!("{}{}", path.to_string_lossy(), std::path::MAIN_SEPARATOR)
}

/// The `path` of each entry in a `listEntries` or `searchEntries` answer.
pub fn paths(answer: &Value) -> Vec<&str> {
    entries(answer, "path")
}

/// The `name` of each entry in a `filesystem.browse` answer.
pub fn names(answer: &Value) -> Vec<&str> {
    entries(answer, "name")
}

fn entries<'a>(answer: &'a Value, field: &str) -> Vec<&'a str> {
    answer["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of entries: {answer}"))
        .iter()
        .map(|entry| {
            entry[field]
                .as_str()
                .unwrap_or_else(|| panic!("a {field}: {entry}"))
        })
        .collect()
}

/// Make a symbolic link, or say it could not be made.
///
/// Windows refuses these outside Developer Mode, so the tests that need one
/// announce a skip rather than failing on a locked-down machine.
pub fn symlink(target: &Path, link: &Path, directory: bool) -> bool {
    #[cfg(windows)]
    {
        if directory {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        } else {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = directory;
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}
