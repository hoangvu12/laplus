//! Opening, searching and saving a project's files, driven the way the UI
//! drives them.
//!
//! Ticket 07's last requirement is that read, search, write and the refusal
//! cases are exercised **through the socket boundary**, and this file is that.
//! Its sibling `socket_filesystem.rs` covers the tree and the picker.
//!
//! The unit tests beside [`laplus_server::files`] cover the rules — which
//! paths are inside a project, what counts as binary, where the megabyte falls.
//! What can only be said here is that the four methods compose: a name from the
//! tree opens, an edit saves, and the search finds what the save created.

mod harness;

use std::path::Path;

use harness::workspace::{paths, symlink, Workspace};
use harness::{Outcome, SocketClient, TestServer};
use serde_json::json;

async fn read_file(client: &mut SocketClient, cwd: &Path, relative_path: &str) -> Outcome {
    client
        .call(
            "projects.readFile",
            json!({"cwd": cwd.to_string_lossy(), "relativePath": relative_path}),
        )
        .await
}

async fn write_file(
    client: &mut SocketClient,
    cwd: &Path,
    relative_path: &str,
    contents: &str,
) -> Outcome {
    client
        .call(
            "projects.writeFile",
            json!({
                "cwd": cwd.to_string_lossy(),
                "relativePath": relative_path,
                "contents": contents,
            }),
        )
        .await
}

async fn search(client: &mut SocketClient, cwd: &Path, query: &str) -> Outcome {
    client
        .call(
            "projects.searchEntries",
            json!({"cwd": cwd.to_string_lossy(), "query": query, "limit": 80}),
        )
        .await
}

/// The ticket's first line: a name from the tree opens and shows its contents.
///
/// Driven as the UI drives it — the path comes out of `projects.listEntries`
/// rather than being written by hand, because the two agreeing on how a path is
/// spelled is the part that could break.
#[tokio::test]
async fn a_file_named_by_the_tree_opens_and_shows_its_contents() {
    let workspace = Workspace::with(&[]);
    workspace.put("src/main.rs", "fn main() {}
");
    workspace.put("README.md", "# Hi
");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = client
        .call(
            "projects.listEntries",
            json!({"cwd": workspace.path().to_string_lossy()}),
        )
        .await
        .expect_success();
    let from_the_tree = paths(&listed)
        .into_iter()
        .find(|path| path.ends_with("main.rs"))
        .expect("the tree names the file")
        .to_string();

    let opened = read_file(&mut client, workspace.path(), &from_the_tree)
        .await
        .expect_success();

    assert_eq!(
        opened,
        json!({
            "relativePath": "src/main.rs",
            "contents": "fn main() {}\n",
            "byteLength": 13,
            "truncated": false,
        })
    );

    client.close().await;
    server.stop().await;
}

/// The ticket's third line, and the round trip that makes it worth having: an
/// edit is saved, and reading it back shows the edit rather than what was there
/// before.
#[tokio::test]
async fn an_edit_is_saved_and_reads_back() {
    let workspace = Workspace::with(&[]);
    workspace.put("notes.md", "before
");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let saved = write_file(&mut client, workspace.path(), "notes.md", "after\n")
        .await
        .expect_success();
    assert_eq!(saved, json!({"relativePath": "notes.md"}));

    assert_eq!(
        workspace.read("notes.md"),
        "after\n"
    );
    assert_eq!(
        read_file(&mut client, workspace.path(), "notes.md")
            .await
            .expect_success()["contents"],
        "after\n"
    );

    client.close().await;
    server.stop().await;
}

/// The ticket's second line. The composer sends a fragment and gets the paths
/// holding it, and the answer has to survive a save — a file the user has just
/// created is exactly the one they are about to mention.
#[tokio::test]
async fn searching_by_name_finds_files_including_ones_just_written() {
    let workspace =
        Workspace::with(&["packages/web/app.tsx", "packages/api/app.ts", "README.md"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let found = search(&mut client, workspace.path(), "app.ts")
        .await
        .expect_success();
    assert_eq!(
        paths(&found),
        ["packages/api/app.ts", "packages/web/app.tsx"]
    );
    assert_eq!(found["truncated"], json!(false));

    // Narrowing by a parent directory is how a user tells two files with one
    // name apart.
    let narrowed = search(&mut client, workspace.path(), "web/app")
        .await
        .expect_success();
    assert_eq!(paths(&narrowed), ["packages/web/app.tsx"]);

    write_file(&mut client, workspace.path(), "packages/web/brand-new.tsx", "x")
        .await
        .expect_success();

    let after = search(&mut client, workspace.path(), "brand-new")
        .await
        .expect_success();
    assert_eq!(
        paths(&after),
        ["packages/web/brand-new.tsx"],
        "the search did not see the file the save had just made"
    );

    client.close().await;
    server.stop().await;
}

/// Ticket 25, at the seam. A project's tree and its search both come from what
/// the repository says is in it, so neither offers what the repository ignores.
///
/// This is the behaviour the whole git-backed scan exists for: without it a
/// JavaScript project's tree fills with `node_modules` and the user's own
/// source is pushed past the entry limit.
#[tokio::test]
async fn a_repository_offers_neither_ignored_files_nor_its_own_git_directory() {
    let workspace =
        Workspace::with(&["src/app.ts", "node_modules/left-pad/app.ts", "dist/app.ts"]);
    workspace.put(".gitignore", "node_modules/
dist/
");
    workspace.init_repository();

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = client
        .call(
            "projects.listEntries",
            json!({"cwd": workspace.path().to_string_lossy()}),
        )
        .await
        .expect_success();
    assert_eq!(paths(&listed), [".gitignore", "src", "src/app.ts"]);

    // The search reads the same scan, so it hides the same files — otherwise
    // the composer would offer a mention the tree has no row for.
    let found = search(&mut client, workspace.path(), "app.ts")
        .await
        .expect_success();
    assert_eq!(paths(&found), ["src/app.ts"]);

    client.close().await;
    server.stop().await;
}

/// The ticket's fifth and sixth lines, which are about the UI not being handed
/// something it cannot draw.
#[tokio::test]
async fn a_binary_file_is_refused_and_a_huge_one_is_truncated() {
    let workspace = Workspace::with(&[]);
    std::fs::write(
        workspace.path().join("logo.png"),
        [0x89, b'P', b'N', b'G', 0x00, 0x1a],
    )
    .expect("writes the file");
    workspace.put("huge.log", &"x".repeat(1024 * 1024 + 512));

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = read_file(&mut client, workspace.path(), "logo.png")
        .await
        .expect_declared("ProjectReadFileError");
    assert_eq!(error["failure"], "binary_file");
    assert!(error["message"]
        .as_str()
        .expect("a message")
        .contains("binary"));

    // Too big is not a refusal: the pane shows the first megabyte with a
    // banner, and the banner needs the real size to say how much is missing.
    let opened = read_file(&mut client, workspace.path(), "huge.log")
        .await
        .expect_success();
    assert_eq!(opened["truncated"], json!(true));
    assert_eq!(opened["byteLength"], json!(1024 * 1024 + 512));
    assert_eq!(
        opened["contents"].as_str().expect("contents").len(),
        1024 * 1024
    );

    client.close().await;
    server.stop().await;
}

/// The ticket's seventh line, which is the security-shaped one. Neither reading
/// nor writing may leave the project, and the two ways out are different facts:
/// a path that says so, and a path that only says so once the filesystem is
/// asked where it really goes.
#[tokio::test]
async fn nothing_outside_the_project_can_be_read_or_written() {
    let workspace = Workspace::with(&["inside.txt"]);
    let elsewhere = tempfile::tempdir().expect("a second directory");
    let secret = elsewhere.path().join("id_rsa");
    std::fs::write(&secret, "PRIVATE KEY").expect("writes the file");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Said out loud in the path.
    let error = read_file(&mut client, workspace.path(), "../id_rsa")
        .await
        .expect_declared("ProjectReadFileError");
    assert_eq!(error["failure"], "workspace_path_outside_root");

    let error = read_file(&mut client, workspace.path(), &secret.to_string_lossy())
        .await
        .expect_declared("ProjectReadFileError");
    assert_eq!(error["failure"], "workspace_path_outside_root");

    // Hidden behind a link, and only findable by resolving it.
    let link = workspace.path().join("innocent.txt");
    if symlink(&secret, &link, false) {
        let error = read_file(&mut client, workspace.path(), "innocent.txt")
            .await
            .expect_declared("ProjectReadFileError");
        assert_eq!(error["failure"], "resolved_path_outside_root");
        assert!(error["resolvedPath"].is_string(), "{error}");
    } else {
        eprintln!("skipped the symlink half: this machine will not create file symlinks");
    }

    // And a write out of the project changes nothing on disk.
    let error = write_file(&mut client, workspace.path(), "../id_rsa", "clobbered")
        .await
        .expect_declared("ProjectWriteFileError");
    assert_eq!(error["failure"], "workspace_path_outside_root");
    assert_eq!(
        std::fs::read_to_string(&secret).expect("untouched"),
        "PRIVATE KEY"
    );

    // Four refusals cost four calls and nothing else.
    assert!(matches!(
        read_file(&mut client, workspace.path(), "inside.txt").await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// The ticket's eighth line. The save fails, says why, and what was on disk is
/// exactly what was on disk.
#[tokio::test]
async fn a_failed_write_reports_why_and_leaves_the_file_alone() {
    let workspace = Workspace::with(&[]);
    workspace.put("src/main.rs", "fn main() {}
");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = write_file(&mut client, workspace.path(), "src", "clobbered")
        .await
        .expect_declared("ProjectWriteFileError");

    assert_eq!(error["failure"], "path_not_file");
    assert!(error["message"]
        .as_str()
        .expect("a message")
        .contains("not a file"));
    assert_eq!(
        workspace.read("src/main.rs"),
        "fn main() {}\n"
    );

    client.close().await;
    server.stop().await;
}

/// The ticket's fourth line. The editor list is part of the configuration the
/// UI fetches before it can do anything, and the picker offers exactly what the
/// server said it could start — so an id the server does not know is a refusal
/// rather than a process.
#[tokio::test]
async fn the_editors_the_server_offers_are_the_ones_it_will_launch() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let offered: Vec<&str> = config["availableEditors"]
        .as_array()
        .expect("an array of editors")
        .iter()
        .map(|editor| editor.as_str().expect("an id"))
        .collect();

    // Whatever else this machine has, it has somewhere to show a folder.
    assert!(
        offered.contains(&"file-manager"),
        "no editor was advertised at all: {offered:?}"
    );

    let error = client
        .call(
            "shell.openInEditor",
            json!({"cwd": "/repo/src/main.rs", "editor": "emacs"}),
        )
        .await
        .expect_declared("ExternalLauncherUnknownEditorError");
    assert_eq!(error["editor"], "emacs");

    client.close().await;
    server.stop().await;
}

/// Every refusal in this family arrives as the method's own declared error, so
/// the client decodes it and shows the sentence. One that did not would cost
/// the connection rather than the call — see `DispatchError::to_error`.
#[tokio::test]
async fn a_call_for_a_file_that_is_not_there_fails_only_itself() {
    let workspace = Workspace::with(&["present.txt"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = read_file(&mut client, workspace.path(), "absent.txt")
        .await
        .expect_declared("ProjectReadFileError");
    assert_eq!(error["failure"], "operation_failed");
    assert_eq!(error["cwd"], json!(workspace.path().to_string_lossy()));
    assert_eq!(error["relativePath"], "absent.txt");

    let missing_project = workspace.path().join("not-a-project");
    let error = search(&mut client, &missing_project, "anything")
        .await
        .expect_declared("ProjectSearchEntriesError");
    assert_eq!(error["failure"], "workspace_root_not_found");

    assert!(matches!(
        read_file(&mut client, workspace.path(), "present.txt").await,
        Outcome::Success(_)
    ));
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}
