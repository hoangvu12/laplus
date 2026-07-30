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
    /// The temporary directory, held so that it is cleaned up.
    directory: tempfile::TempDir,
    /// Where this workspace actually is, which is usually the directory itself.
    /// [`Workspace::worktree`] is the exception: it roots one level inside its
    /// own, because `git worktree add` wants a path that is not there yet.
    root: PathBuf,
}

impl Workspace {
    pub fn with(paths: &[&str]) -> Workspace {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let workspace = Workspace {
            root: directory.path().to_path_buf(),
            directory,
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
        &self.root
    }

    /// Check `ref_name` out into a worktree of this repository, on a new ref of
    /// that name, and hand back a handle to the folder it went in.
    ///
    /// Somewhere else entirely, in a temporary directory of its own: a developer
    /// who runs `git worktree add` puts the second checkout beside the first
    /// rather than inside it, and a worktree nested in its own repository would
    /// turn up as an untracked directory in every status the project answers.
    ///
    /// This is a developer at a terminal, not a laplus method — the point of the
    /// case is that a conversation can be pointed at a worktree this server
    /// neither made nor knows about.
    pub fn worktree(&self, ref_name: &str) -> Workspace {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("checkout");
        self.git(&[
            "worktree",
            "add",
            "-b",
            ref_name,
            &root.to_string_lossy(),
        ]);
        Workspace { directory, root }
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
    /// semantics has to have one, and so does every test about status. Not
    /// skippable: the spec commits to shelling out to `git`, so a machine
    /// without it cannot run this suite anyway.
    ///
    /// Four things are pinned rather than inherited from the machine, because
    /// each of them is something a developer's own global configuration can set
    /// and each would change what a test observes:
    ///
    /// - **the initial branch**, so a test may assert a branch by name;
    /// - **an identity**, without which `git commit` refuses;
    /// - **no signing**, because a machine configured to sign every commit
    ///   would prompt for a key inside a test;
    /// - **no line-ending translation**, because a status counts lines and
    ///   `core.autocrlf` changes how many there are.
    pub fn init_repository(&self) -> &Workspace {
        self.git(&["init", "-b", "main"]);
        self.git(&["config", "user.name", "laplus tests"]);
        self.git(&["config", "user.email", "tests@laplus.invalid"]);
        self.git(&["config", "commit.gpgsign", "false"]);
        self.git(&["config", "core.autocrlf", "false"]);
        self
    }

    /// Run one `git` in this workspace, failing the test if it refuses.
    pub fn git(&self, arguments: &[&str]) -> String {
        let output = self.try_git(arguments);
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run one `git` and hand back whatever happened.
    ///
    /// For the commands a test *wants* to fail: a merge that conflicts is how a
    /// repository is put into the mid-merge state, and it exits non-zero when it
    /// works.
    pub fn try_git(&self, arguments: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args(arguments)
            .output()
            .expect("git runs")
    }

    /// Commit everything in the workspace. The thing a status is measured
    /// against.
    pub fn commit(&self, message: &str) -> &Workspace {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
        self
    }

    /// Delete a file, the way a developer or an agent would.
    pub fn remove(&self, path: &str) {
        std::fs::remove_file(self.path().join(path)).expect("removes the file");
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
