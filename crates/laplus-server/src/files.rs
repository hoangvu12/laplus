//! Reading and writing one file inside a project.
//!
//! [`crate::filesystem`] enumerates names anywhere on the disk; this module
//! opens what those names point at, and it is the first in the build that
//! **writes**. That difference is the whole reason it is a separate module with
//! a rule of its own.
//!
//! ## The rule: nothing outside the workspace root
//!
//! The picker may look anywhere, because a user adding a project is by
//! definition looking outside every project they have. `projects.readFile` and
//! `projects.writeFile` are the opposite case: the client names a project and a
//! path *inside* it, so anything resolving outside is a bug or an attack and
//! neither deserves an answer.
//!
//! Confinement is checked **twice**, and the second check is the one that
//! matters:
//!
//! 1. **Lexically**, before touching the disk — an absolute path is refused,
//!    and so is one whose normalised form climbs out with `..`. This catches
//!    the ordinary mistake and costs nothing.
//! 2. **After resolving symlinks**, on both the root and the target. A path can
//!    be perfectly well-behaved as a string and still land outside: `notes.txt`
//!    inside the project can be a link to `C:\Users\me\.ssh\id_rsa`. Only
//!    asking the filesystem where a path really goes answers that, and the
//!    contract has a distinct failure literal for it
//!    (`resolved_path_outside_root`) precisely because it is a different fact
//!    from the lexical one.
//!
//! The reference server checks in the same order for the same reason; the two
//! literals are its `workspace_path_outside_root` and `resolved_path_outside_root`.
//!
//! ## What a read refuses
//!
//! A file the UI cannot render is refused with a sentence rather than sent and
//! left to the browser: a **binary** file (a NUL byte in what was read) and
//! anything past [`MAX_BYTES`], which comes back as its first megabyte with
//! `truncated` set and the true `byteLength` beside it. Both are upstream's
//! rules and upstream's numbers, because the UI's editor is built around them.
//!
//! Shapes are hand-written from `ProjectReadFileResult`, `ProjectWriteFileResult`
//! and `ProjectFileFailure` in `t3code/packages/contracts/src/project.ts`, and
//! the error shape is the one captured whole in
//! `fixtures/socket-wire/03-typed-error.ndjson` — a real `ProjectReadFileError`
//! from the reference server, field for field.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::filesystem::Index;
use crate::projects::WorkspaceRoot;
use crate::rpc::{declared, non_blank};

/// One file's contents, for the editor pane.
pub const READ_FILE: &str = "projects.readFile";

/// One file's contents, saved.
pub const WRITE_FILE: &str = "projects.writeFile";

/// The most of a file a read will return.
///
/// Upstream's megabyte, kept for the same reason [`crate::filesystem::MAX_ENTRIES`]
/// is: the UI renders `truncated` and shows the real `byteLength` beside it, so
/// a server with a different threshold would tell users a different story about
/// the same file.
///
/// A read past this is *not* a refusal. The ticket asks for a file above the
/// threshold to be "refused with a message naming the limit", and the contract
/// disagrees — `ProjectReadFileResult` carries `truncated` for exactly this,
/// and the editor pane is built to show a partial file with a banner. Refusing
/// would mean a 2 MB log file cannot be looked at at all. See the ticket's
/// comments; the limit is still named, in the response rather than in an error.
pub const MAX_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// projects.readFile
// ---------------------------------------------------------------------------

/// A validated `projects.readFile` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFile {
    cwd: String,
    relative_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadFilePayload {
    cwd: String,
    relative_path: String,
}

impl ReadFile {
    pub fn read(payload: &Value) -> Result<ReadFile, Value> {
        let read: ReadFilePayload = serde_json::from_value(payload.clone())
            .map_err(|error| declared(READ_ERROR, format_args!("{READ_FILE} is malformed: {error}")))?;

        Ok(ReadFile {
            cwd: non_blank(&read.cwd, READ_ERROR, "workspace root")?,
            relative_path: non_blank(&read.relative_path, READ_ERROR, "path")?,
        })
    }

    /// Do the work. Blocking, and called from a blocking task.
    pub fn run(self) -> Result<Value, Value> {
        let target = match Confined::resolve(&self.cwd, &self.relative_path) {
            Ok(target) => target,
            Err(refusal) => return Err(self.to_error(&refusal)),
        };

        match target.contents() {
            Ok(file) => Ok(json!({
                "relativePath": target.relative_path,
                "contents": file.text,
                // The whole file's size, not the part that was read — the UI
                // puts it next to the truncation banner, so reporting the
                // truncated length would say the file is exactly as big as the
                // part we chose to show.
                "byteLength": file.byte_length,
                "truncated": file.truncated,
            })),
            Err(refusal) => Err(self.to_error(&refusal)),
        }
    }

    fn to_error(&self, refusal: &Refusal) -> Value {
        refusal.to_error(READ_ERROR, &self.cwd, &self.relative_path)
    }
}

/// What came back off the disk.
struct Contents {
    text: String,
    /// The file's real size, which may be larger than what `text` holds.
    byte_length: u64,
    truncated: bool,
}

// ---------------------------------------------------------------------------
// projects.writeFile
// ---------------------------------------------------------------------------

/// A validated `projects.writeFile` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFile {
    cwd: String,
    relative_path: String,
    contents: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteFilePayload {
    cwd: String,
    relative_path: String,
    contents: String,
}

impl WriteFile {
    pub fn read(payload: &Value) -> Result<WriteFile, Value> {
        let read: WriteFilePayload = serde_json::from_value(payload.clone()).map_err(|error| {
            declared(WRITE_ERROR, format_args!("{WRITE_FILE} is malformed: {error}"))
        })?;

        Ok(WriteFile {
            cwd: non_blank(&read.cwd, WRITE_ERROR, "workspace root")?,
            relative_path: non_blank(&read.relative_path, WRITE_ERROR, "path")?,
            // Empty contents are a legitimate save — a user can clear a file —
            // so this is the one field that is taken as given.
            contents: read.contents,
        })
    }

    /// Do the work. Blocking, and called from a blocking task.
    ///
    /// The index is forgotten afterwards rather than updated: the write may
    /// have created a file, and the composer's `@` mention should be able to
    /// find it on the next keystroke. Upstream refreshes its indexer at exactly
    /// this point.
    pub fn run(self, index: &Index) -> Result<Value, Value> {
        let target = match Confined::resolve(&self.cwd, &self.relative_path) {
            Ok(target) => target,
            Err(refusal) => return Err(self.to_error(&refusal)),
        };

        if let Err(refusal) = target.write(&self.contents) {
            return Err(self.to_error(&refusal));
        }

        index.forget(&self.cwd);
        Ok(json!({"relativePath": target.relative_path}))
    }

    fn to_error(&self, refusal: &Refusal) -> Value {
        refusal.to_error(WRITE_ERROR, &self.cwd, &self.relative_path)
    }
}

// ---------------------------------------------------------------------------
// Confinement, and the disk behind it
// ---------------------------------------------------------------------------

/// A path that has been proved to be inside its workspace root.
///
/// Constructing one is the proof, the same way [`WorkspaceRoot`] works: there
/// is no way to hold a `Target` for a path that was not, at that moment, inside
/// the project both as a string and as somewhere on the disk.
struct Confined {
    /// The project this path had to be inside, absolute.
    root: PathBuf,
    absolute: PathBuf,
    /// The path as the client will see it echoed back — forward slashes,
    /// relative to the root.
    relative_path: String,
}

impl Confined {
    fn resolve(cwd: &str, relative_path: &str) -> Result<Confined, Refusal> {
        let root = WorkspaceRoot::check(cwd).map_err(|rejection| Refusal::Operation {
            failure: "operation_failed",
            operation: "realpath-workspace-root",
            operation_path: cwd.to_string(),
            resolved_path: None,
            detail: rejection.message(),
        })?;
        let root = PathBuf::from(root.display());

        let absolute = descend(&root, Path::new(relative_path.trim()))?;
        let relative_path = absolute
            .strip_prefix(&root)
            .map_err(|_| Refusal::OutsideRoot)?
            .to_string_lossy()
            .replace('\\', "/");

        Ok(Confined {
            root,
            absolute,
            relative_path,
        })
    }

    /// Where a path really goes, once every link on the way has been followed —
    /// and a refusal if that is not inside the project after all.
    ///
    /// Only answerable for a path that exists, which is why a write checks its
    /// parent directory rather than the file it is about to create.
    fn confirm_real(&self, path: &Path) -> Result<PathBuf, Refusal> {
        let real_root = canonical(&self.root, "realpath-workspace-root", &self.root)?;
        let real_path = canonical(path, "realpath-target", path)?;

        if real_path.starts_with(&real_root) {
            Ok(real_path)
        } else {
            Err(Refusal::EscapedRoot {
                resolved_path: real_path.to_string_lossy().into_owned(),
                resolved_root: real_root.to_string_lossy().into_owned(),
            })
        }
    }

    fn contents(&self) -> Result<Contents, Refusal> {
        let real = self.confirm_real(&self.absolute)?;

        let metadata = std::fs::metadata(&real).map_err(|error| Refusal::io("stat", &real, &error))?;
        if !metadata.is_file() {
            return Err(Refusal::NotAFile);
        }

        let byte_length = metadata.len();
        let wanted = byte_length.min(MAX_BYTES) as usize;
        let bytes = read_at_most(&real, wanted)?;

        // Upstream's test: a NUL byte anywhere in what was read. It is a
        // heuristic rather than a proof, and it is the same heuristic `git` and
        // `grep` use, so a file one of those calls binary is a file the UI will
        // refuse too.
        if bytes.contains(&0) {
            return Err(Refusal::Binary);
        }

        Ok(Contents {
            // Lossy rather than a refusal: a file that is *nearly* UTF-8 — one
            // stray byte from a different encoding — is still worth showing,
            // and the alternative is a blank pane and an error the user cannot
            // act on. Truncation can also cut a multi-byte character in half at
            // exactly the megabyte, which is nobody's fault and must not fail
            // the read.
            text: String::from_utf8_lossy(&bytes).into_owned(),
            byte_length,
            truncated: byte_length > MAX_BYTES,
        })
    }

    fn write(&self, contents: &str) -> Result<(), Refusal> {
        let parent = self
            .absolute
            .parent()
            .ok_or(Refusal::OutsideRoot)?
            .to_path_buf();

        // **Before creating anything.** A path that does not exist cannot be
        // resolved, so what is checked is the deepest part of it that *does* —
        // and it has to be checked first, because `create_dir_all` follows
        // symlinks. Given `link/nested/file.txt` where `link` points out of the
        // project, creating the directories before checking would put `nested`
        // outside the project and only then refuse the call. Refusing a write
        // that has already made directories somewhere it should not is not
        // refusing it.
        self.confirm_real(&existing_ancestor(&parent))?;

        std::fs::create_dir_all(&parent)
            .map_err(|error| Refusal::io("make-directory", &parent, &error))?;

        // And again now the parent exists, because the first check could only
        // see as far as what was already there. This is the one that proves the
        // directory the bytes are about to land in is inside the project.
        let real_parent = self.confirm_real(&parent)?;
        let destination = real_parent.join(self.absolute.file_name().ok_or(Refusal::OutsideRoot)?);
        if let Ok(existing) = std::fs::metadata(&destination) {
            if existing.is_dir() {
                return Err(Refusal::NotAFile);
            }
        }

        std::fs::write(&destination, contents)
            .map_err(|error| Refusal::io("write-file", &destination, &error))
    }
}

/// The deepest part of `path` that is already on the disk.
///
/// A path that does not exist cannot be canonicalised, so this is what a
/// confinement check has to be made against before any of it is created. In the
/// worst case it walks up to the workspace root, which [`WorkspaceRoot::check`]
/// has already proved exists.
fn existing_ancestor(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(path)
        .to_path_buf()
}

/// Walk `requested` downwards from `root`, refusing anything that does not stay
/// inside it.
///
/// The lexical half of the confinement rule, and the reason it is a fold rather
/// than a `join` and a prefix test: `Path::components` does **not** resolve
/// `..`, so `root.join("../secret.txt")` still literally begins with the root
/// and would pass a `strip_prefix` check while pointing at the root's sibling.
/// Each `..` has to be applied, and the result checked, one component at a time.
///
/// Refuses four things: an absolute path, a path that climbs past the root, a
/// path that lands *on* the root — the project directory is not a file in the
/// project — and, on Windows, a bare drive letter.
fn descend(root: &Path, requested: &Path) -> Result<PathBuf, Refusal> {
    use std::path::Component;

    let mut inside = root.to_path_buf();
    for component in requested.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !inside.pop() || !inside.starts_with(root) {
                    return Err(Refusal::OutsideRoot);
                }
            }
            Component::Normal(part) => inside.push(part),
            Component::RootDir | Component::Prefix(_) => return Err(Refusal::OutsideRoot),
        }
    }

    if inside == root {
        return Err(Refusal::OutsideRoot);
    }
    Ok(inside)
}

/// Read at most `wanted` bytes, without asking for a buffer the size of the
/// file. A 4 GB video in a project would otherwise be a 4 GB allocation before
/// anyone noticed it was not text.
fn read_at_most(path: &Path, wanted: usize) -> Result<Vec<u8>, Refusal> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|error| Refusal::io("open", path, &error))?;
    let mut bytes = Vec::with_capacity(wanted);
    file.take(wanted as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| Refusal::io("read", path, &error))?;
    Ok(bytes)
}

fn canonical(
    path: &Path,
    operation: &'static str,
    operation_path: &Path,
) -> Result<PathBuf, Refusal> {
    std::fs::canonicalize(path).map_err(|error| Refusal::Operation {
        failure: "operation_failed",
        operation,
        operation_path: operation_path.to_string_lossy().into_owned(),
        resolved_path: Some(path.to_string_lossy().into_owned()),
        detail: error.to_string(),
    })
}

/// Why a file could not be read or written.
///
/// Every variant maps to one of the contract's five `ProjectFileFailure`
/// literals; the client switches on that string, so there is deliberately no
/// sixth.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// The path is absolute, or climbs out of the project as written.
    OutsideRoot,
    /// The path stays inside as a string but lands outside once links are
    /// followed.
    EscapedRoot {
        resolved_path: String,
        resolved_root: String,
    },
    NotAFile,
    Binary,
    Operation {
        failure: &'static str,
        operation: &'static str,
        operation_path: String,
        resolved_path: Option<String>,
        detail: String,
    },
}

impl Refusal {
    fn io(operation: &'static str, path: &Path, error: &std::io::Error) -> Refusal {
        Refusal::Operation {
            failure: "operation_failed",
            operation,
            operation_path: path.to_string_lossy().into_owned(),
            resolved_path: Some(path.to_string_lossy().into_owned()),
            detail: error.to_string(),
        }
    }

    fn failure(&self) -> &'static str {
        match self {
            Refusal::OutsideRoot => "workspace_path_outside_root",
            Refusal::EscapedRoot { .. } => "resolved_path_outside_root",
            Refusal::NotAFile => "path_not_file",
            Refusal::Binary => "binary_file",
            Refusal::Operation { failure, .. } => failure,
        }
    }

    /// The sentence the UI shows. `ProjectReadFileError` carries structured
    /// fields as well, but the pane renders the message, so each one names both
    /// the file and what was wrong with it.
    fn message(&self, cwd: &str, relative_path: &str) -> String {
        match self {
            Refusal::OutsideRoot => format!(
                "File path must be inside the project: '{relative_path}' is not inside '{cwd}'."
            ),
            Refusal::EscapedRoot { resolved_path, .. } => format!(
                "File '{relative_path}' leads outside the project '{cwd}': {resolved_path}"
            ),
            Refusal::NotAFile => {
                format!("Workspace path '{relative_path}' in '{cwd}' is not a file.")
            }
            Refusal::Binary => format!(
                "File '{relative_path}' in '{cwd}' is binary and cannot be shown as text."
            ),
            Refusal::Operation {
                operation, detail, ..
            } => format!("Could not {operation} '{relative_path}' in '{cwd}': {detail}"),
        }
    }

    /// The typed error, with the request echoed back into it the way the
    /// reference server does — see the `ProjectReadFileError` captured in
    /// `fixtures/socket-wire/03-typed-error.ndjson`, which carries its `cwd`,
    /// `relativePath`, `failure`, `operation` and `operationPath` alongside the
    /// sentence.
    fn to_error(&self, tag: &'static str, cwd: &str, relative_path: &str) -> Value {
        let mut error = json!({
            "_tag": tag,
            "cwd": cwd,
            "relativePath": relative_path,
            "failure": self.failure(),
            "message": self.message(cwd, relative_path),
        });

        match self {
            Refusal::EscapedRoot {
                resolved_path,
                resolved_root,
            } => {
                error["resolvedPath"] = json!(resolved_path);
                error["resolvedWorkspaceRoot"] = json!(resolved_root);
            }
            Refusal::Operation {
                operation,
                operation_path,
                resolved_path,
                ..
            } => {
                error["operation"] = json!(operation);
                error["operationPath"] = json!(operation_path);
                if let Some(resolved) = resolved_path {
                    error["resolvedPath"] = json!(resolved);
                }
            }
            Refusal::OutsideRoot | Refusal::NotAFile | Refusal::Binary => {}
        }
        error
    }
}

const READ_ERROR: &str = "ProjectReadFileError";
const WRITE_ERROR: &str = "ProjectWriteFileError";

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for (path, contents) in files {
            let full = directory.path().join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("creates the parents");
            std::fs::write(&full, contents).expect("writes the file");
        }
        directory
    }

    fn read(directory: &Path, relative_path: &str) -> Result<Value, Value> {
        ReadFile::read(&json!({
            "cwd": directory.to_string_lossy(),
            "relativePath": relative_path,
        }))?
        .run()
    }

    fn write(directory: &Path, relative_path: &str, contents: &str) -> Result<Value, Value> {
        WriteFile::read(&json!({
            "cwd": directory.to_string_lossy(),
            "relativePath": relative_path,
            "contents": contents,
        }))?
        .run(&Index::new())
    }

    /// The ticket's first line: a file opened from the tree shows what is in
    /// it, named the way the tree named it.
    #[test]
    fn a_file_is_read_back_with_its_contents_and_size() {
        let directory = project(&[("src/main.rs", "fn main() {}\n")]);

        let result = read(directory.path(), "src/main.rs").expect("a readable file");

        assert_eq!(
            result,
            json!({
                "relativePath": "src/main.rs",
                "contents": "fn main() {}\n",
                "byteLength": 13,
                "truncated": false,
            })
        );
    }

    /// The tree hands back forward slashes; a user's keyboard and the
    /// filesystem underneath may not. All three name the same file.
    #[test]
    fn a_path_is_accepted_in_whichever_separator_it_arrives_with() {
        let directory = project(&[("src/lib/util.rs", "pub fn go() {}\n")]);

        for spelling in ["src/lib/util.rs", r"src\lib\util.rs", "./src/lib/util.rs"] {
            let result = read(directory.path(), spelling).expect(spelling);
            assert_eq!(result["relativePath"], "src/lib/util.rs", "{spelling}");
        }
    }

    /// The ticket's third line, and the reason writes exist at all.
    #[test]
    fn a_write_lands_on_disk_and_reads_back() {
        let directory = project(&[("notes.md", "before\n")]);

        let result = write(directory.path(), "notes.md", "after\n").expect("a writable file");

        assert_eq!(result, json!({"relativePath": "notes.md"}));
        assert_eq!(
            std::fs::read_to_string(directory.path().join("notes.md")).expect("the file"),
            "after\n"
        );
        assert_eq!(
            read(directory.path(), "notes.md").expect("readable")["contents"],
            "after\n"
        );
    }

    /// A save into a folder that does not exist yet is an ordinary save — the
    /// editor pane offers "save as" into new directories.
    #[test]
    fn a_write_creates_the_directories_it_needs() {
        let directory = project(&[("notes.md", "x")]);

        write(directory.path(), "docs/adr/0001-why.md", "# Why\n").expect("a writable path");

        assert_eq!(
            std::fs::read_to_string(directory.path().join("docs/adr/0001-why.md"))
                .expect("the file"),
            "# Why\n"
        );
    }

    /// Clearing a file is a save like any other, and must not be mistaken for a
    /// missing field.
    #[test]
    fn a_write_may_be_empty() {
        let directory = project(&[("notes.md", "something")]);

        write(directory.path(), "notes.md", "").expect("an empty save");

        assert_eq!(
            std::fs::read_to_string(directory.path().join("notes.md")).expect("the file"),
            ""
        );
    }

    /// The ticket's seventh line, in the form a mistake takes: a path that
    /// climbs out of the project, or one that was never relative to begin with.
    #[test]
    fn a_path_outside_the_project_is_refused_before_the_disk_is_touched() {
        let directory = project(&[("inside.txt", "mine")]);
        let outside = directory.path().parent().expect("a parent").join("secret.txt");
        std::fs::write(&outside, "not mine").expect("writes the file");

        for escape in [
            "../secret.txt",
            r"..\secret.txt",
            "src/../../secret.txt",
            ".",
        ] {
            let error = read(directory.path(), escape).expect_err(escape);
            assert_eq!(error["_tag"], "ProjectReadFileError", "{escape}");
            assert_eq!(error["failure"], "workspace_path_outside_root", "{escape}");
            assert!(
                error["message"].as_str().expect("a message").contains(escape),
                "{escape}"
            );
        }

        let absolute = outside.to_string_lossy().into_owned();
        assert_eq!(
            read(directory.path(), &absolute).expect_err("absolute")["failure"],
            "workspace_path_outside_root"
        );

        // And a write is refused the same way, without creating anything.
        assert_eq!(
            write(directory.path(), "../made-up.txt", "x").expect_err("escapes")["failure"],
            "workspace_path_outside_root"
        );
        assert!(!directory
            .path()
            .parent()
            .expect("a parent")
            .join("made-up.txt")
            .exists());

        assert_eq!(
            std::fs::read_to_string(&outside).expect("untouched"),
            "not mine"
        );
    }

    /// The second confinement check, which the first cannot make: a path that
    /// is well-behaved as a string and still lands outside once the filesystem
    /// is asked where it really goes.
    #[test]
    fn a_symlink_leading_out_of_the_project_is_refused_once_it_is_resolved() {
        let directory = project(&[("inside.txt", "mine")]);
        let elsewhere = tempfile::tempdir().expect("a second directory");
        let secret = elsewhere.path().join("id_rsa");
        std::fs::write(&secret, "PRIVATE KEY").expect("writes the file");

        let link = directory.path().join("innocent.txt");
        if !symlink_file(&secret, &link) {
            eprintln!("skipped: this machine will not create file symlinks");
            return;
        }

        let error = read(directory.path(), "innocent.txt").expect_err("leads outside");

        assert_eq!(error["failure"], "resolved_path_outside_root");
        assert!(error["resolvedPath"].is_string(), "{error}");
        assert!(error["resolvedWorkspaceRoot"].is_string(), "{error}");
        assert!(!error["contents"].is_string(), "the file was read anyway");
    }

    #[cfg(windows)]
    fn symlink_file(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    #[cfg(not(windows))]
    fn symlink_file(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// A refused write must not have *already* changed the filesystem outside
    /// the project.
    ///
    /// The path here is well-behaved as a string, so the lexical check passes
    /// it; the link is only visible once the disk is asked. Creating the
    /// directories before that check would follow the link and put `nested`
    /// outside the project — the call would still be refused, and a directory
    /// would still have been made somewhere it should not be.
    #[test]
    fn a_write_through_a_symlinked_directory_creates_nothing_outside_the_project() {
        let directory = project(&[("inside.txt", "mine")]);
        let elsewhere = tempfile::tempdir().expect("a second directory");

        let link = directory.path().join("link");
        if !symlink_dir(elsewhere.path(), &link) {
            eprintln!("skipped: this machine will not create directory symlinks");
            return;
        }

        let error = write(directory.path(), "link/nested/notes.md", "leaked")
            .expect_err("the parent leads outside the project");

        assert_eq!(error["failure"], "resolved_path_outside_root");
        assert!(
            !elsewhere.path().join("nested").exists(),
            "a directory was created outside the project before the refusal"
        );
        assert!(!elsewhere.path().join("nested/notes.md").exists());
    }

    #[cfg(windows)]
    fn symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(not(windows))]
    fn symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// The ticket's fifth line. A NUL byte is the same test `git` and `grep`
    /// use, so a file one of those calls binary is one the UI refuses too.
    #[test]
    fn a_binary_file_is_refused_rather_than_rendered() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join("logo.png"), [0x89, b'P', b'N', b'G', 0x00, 0x1a])
            .expect("writes the file");

        let error = read(directory.path(), "logo.png").expect_err("binary");

        assert_eq!(error["failure"], "binary_file");
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("binary"));
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("logo.png"));
    }

    /// The ticket's sixth line, answered as the contract asks rather than as
    /// the ticket's wording does — see this module's [`MAX_BYTES`]. The UI
    /// shows the first megabyte with a banner, and the banner needs the true
    /// size to say how much is missing.
    #[test]
    fn a_file_past_the_limit_comes_back_truncated_with_its_real_size() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let oversized = "x".repeat(MAX_BYTES as usize + 4_096);
        std::fs::write(directory.path().join("huge.log"), &oversized).expect("writes the file");

        let result = read(directory.path(), "huge.log").expect("a large file is still readable");

        assert_eq!(result["truncated"], json!(true));
        assert_eq!(result["byteLength"], json!(MAX_BYTES + 4_096));
        assert_eq!(
            result["contents"].as_str().expect("contents").len(),
            MAX_BYTES as usize,
            "more than the limit was sent"
        );
    }

    /// A directory is not a file, and neither is a path that is not there. Both
    /// have their own literal so the pane can tell "gone" from "not text".
    #[test]
    fn a_path_that_is_not_a_readable_file_says_which_kind_of_not() {
        let directory = project(&[("src/main.rs", "fn main() {}")]);

        let error = read(directory.path(), "src").expect_err("a directory");
        assert_eq!(error["failure"], "path_not_file");
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("is not a file"));

        let error = read(directory.path(), "not-there.txt").expect_err("missing");
        assert_eq!(error["failure"], "operation_failed");
        assert_eq!(error["operation"], "realpath-target");
        assert!(error["operationPath"].is_string(), "{error}");
    }

    /// The ticket's eighth line. The write fails, says why, and the file that
    /// was there is exactly as it was.
    #[test]
    fn a_failed_write_leaves_the_file_unchanged() {
        let directory = project(&[("src/main.rs", "fn main() {}\n")]);

        // Writing *over a directory* is the failure a test can make on any
        // machine; a permission denial would need `icacls` or `chmod`.
        let error = write(directory.path(), "src", "clobbered").expect_err("a directory");

        assert_eq!(error["_tag"], "ProjectWriteFileError");
        assert_eq!(error["failure"], "path_not_file");
        assert_eq!(
            std::fs::read_to_string(directory.path().join("src/main.rs")).expect("the file"),
            "fn main() {}\n",
            "the write reached the disk after all"
        );
    }

    /// Neither method may be asked for nothing. Both refuse with their own
    /// `_tag`, so the client decodes the error rather than the connection
    /// breaking.
    #[test]
    fn a_payload_that_names_nothing_is_refused_by_its_own_error() {
        for payload in [
            json!({}),
            json!({"cwd": "   ", "relativePath": "a.txt"}),
            json!({"cwd": "C:/x", "relativePath": "  "}),
        ] {
            let error = ReadFile::read(&payload).expect_err("not a read");
            assert_eq!(error["_tag"], "ProjectReadFileError");
            assert!(error["message"].is_string(), "{error}");

            let mut write_payload = payload.clone();
            write_payload["contents"] = json!("x");
            let error = WriteFile::read(&write_payload).expect_err("not a write");
            assert_eq!(error["_tag"], "ProjectWriteFileError");
            assert!(error["message"].is_string(), "{error}");
        }
    }

}
