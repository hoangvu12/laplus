//! Browsing and listing, driven the way the UI drives them.
//!
//! Ticket 06's last requirement is that browse and listing are exercised
//! **through the socket boundary**, and this file is that. Nothing here calls
//! [`lightcode_server::filesystem`] — a folder is picked by sending the request
//! the command palette sends while a user types, and a tree is read by sending
//! the one request `FileBrowserPanel` makes when a project opens.
//!
//! The unit tests next to the module cover the rules — which names match a
//! prefix, where a walk stops, what a symlink does. What can only be said here
//! is what the *connection* does while that work is going on, which is the
//! ticket's "opens without the UI becoming unresponsive" and the reason
//! `Answer::Deferred` exists at all.

mod harness;

use std::path::Path;
use std::time::Duration;

use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// A tree written out from a list of paths. A path ending in `/` is an empty
/// directory; anything else is a file.
fn tree(root: &Path, paths: &[&str]) {
    for path in paths {
        let full = root.join(path.trim_end_matches('/'));
        if path.ends_with('/') {
            std::fs::create_dir_all(&full).expect("creates the directory");
        } else {
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("creates the parents");
            std::fs::write(&full, "contents").expect("writes the file");
        }
    }
}

/// A path with the platform's separator on the end — what the picker sends once
/// the user has finished typing a directory name.
fn inside(path: &Path) -> String {
    format!("{}{}", path.to_string_lossy(), std::path::MAIN_SEPARATOR)
}

async fn browse(client: &mut SocketClient, partial_path: &str) -> Outcome {
    client
        .call("filesystem.browse", json!({"partialPath": partial_path}))
        .await
}

async fn list_entries(client: &mut SocketClient, cwd: &Path) -> Outcome {
    client
        .call(
            "projects.listEntries",
            json!({"cwd": cwd.to_string_lossy()}),
        )
        .await
}

fn names(browsed: &Value) -> Vec<&str> {
    browsed["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of entries: {browsed}"))
        .iter()
        .map(|entry| entry["name"].as_str().expect("a name"))
        .collect()
}

fn paths(listed: &Value) -> Vec<&str> {
    listed["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of entries: {listed}"))
        .iter()
        .map(|entry| entry["path"].as_str().expect("a path"))
        .collect()
}

/// The ticket's first line, at the seam the user meets it: the palette walks the
/// filesystem one directory per keystroke, offering folders and nothing else.
#[tokio::test]
async fn the_filesystem_can_be_browsed_to_pick_a_folder_for_a_new_project() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(
        directory.path(),
        &[
            "projects/lightcode/src/",
            "projects/lighthouse/",
            "projects/notes.md",
            "photos/",
        ],
    );

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // "…\" — the user has finished a directory name and wants what is in it.
    let listed = browse(&mut client, &inside(directory.path()))
        .await
        .expect_success();
    assert_eq!(listed["parentPath"], json!(directory.path().to_string_lossy()));
    assert_eq!(names(&listed), ["photos", "projects"]);

    // "…\projects\light" — part way through the next name. Folders only: the
    // markdown file beside them is not somewhere a project can live.
    let projects = directory.path().join("projects");
    let filtered = browse(&mut client, &format!("{}light", inside(&projects)))
        .await
        .expect_success();
    assert_eq!(filtered["parentPath"], json!(projects.to_string_lossy()));
    assert_eq!(names(&filtered), ["lightcode", "lighthouse"]);
    assert_eq!(
        filtered["entries"][0]["fullPath"],
        json!(projects.join("lightcode").to_string_lossy())
    );

    // And the folder that came back is one the registry will take, which is the
    // whole point of picking it.
    let created = client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "project.create",
                "commandId": "test:create:p1",
                "projectId": "p1",
                "title": "",
                "workspaceRoot": filtered["entries"][0]["fullPath"],
                "createWorkspaceRootIfMissing": true,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }),
        )
        .await
        .expect_success();
    assert_eq!(created["sequence"], json!(1));

    client.close().await;
    server.stop().await;
}

/// The ticket's second and third lines. One request describes the project, and
/// the tree the UI draws from it expands a directory at a time on the client —
/// `initialExpansion: 1` in `FileBrowserPanel.tsx`. What the server owes is that
/// the answer is complete, correctly kinded, and named the way the tree splits
/// paths.
#[tokio::test]
async fn an_open_project_lists_its_tree_in_one_answer() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(
        directory.path(),
        &[
            "src/main.rs",
            "src/lib/util.rs",
            "README.md",
            ".git/objects/ab/cdef",
        ],
    );

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = list_entries(&mut client, directory.path())
        .await
        .expect_success();

    assert_eq!(
        paths(&listed),
        ["README.md", "src", "src/lib", "src/lib/util.rs", "src/main.rs"]
    );
    assert_eq!(listed["truncated"], json!(false));
    assert_eq!(listed["entries"][1], json!({"path": "src", "kind": "directory"}));
    assert_eq!(
        listed["entries"][4],
        json!({"path": "src/main.rs", "kind": "file"})
    );

    client.close().await;
    server.stop().await;
}

/// The ticket's last-but-one line. Names with spaces and non-ASCII characters
/// are ordinary, and every step between the directory entry and the JSON string
/// is somewhere one could be mangled — so this is checked at the far end of the
/// socket rather than in the walk.
#[tokio::test]
async fn listings_are_correct_for_spaces_and_non_ascii_names() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(
        directory.path(),
        &["my documents/note book.txt", "café/naïve.txt", "日本語/ファイル.txt"],
    );

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = list_entries(&mut client, directory.path())
        .await
        .expect_success();
    let paths = paths(&listed);
    for expected in [
        "my documents",
        "my documents/note book.txt",
        "café",
        "café/naïve.txt",
        "日本語",
        "日本語/ファイル.txt",
    ] {
        assert!(paths.contains(&expected), "{expected} is missing: {paths:?}");
    }

    // And the picker offers them back under the same names.
    let browsed = browse(&mut client, &inside(directory.path()))
        .await
        .expect_success();
    assert_eq!(names(&browsed), ["café", "my documents", "日本語"]);

    client.close().await;
    server.stop().await;
}

/// The ticket's "opens without the UI becoming unresponsive", said precisely.
///
/// A tree large enough to take real time is listed, and while that is happening
/// the connection has to keep working: a `Ping` sent immediately after the
/// request is answered *before* the listing is, and a subscription opened
/// beforehand still delivers. Answering the listing on the read loop would make
/// both impossible — every frame behind it would wait for the disk.
#[tokio::test]
async fn a_large_repository_does_not_stall_the_connection_while_it_is_listed() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    for folder in 0..40 {
        let path = directory.path().join(format!("package-{folder:02}")).join("src");
        std::fs::create_dir_all(&path).expect("creates the folder");
        for file in 0..50 {
            std::fs::write(path.join(format!("module-{file:02}.ts")), "export {};\n")
                .expect("writes the file");
        }
    }

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // A subscription open across the listing, so "the rest of the app" is
    // represented by something the server is actively pumping.
    let shell = client
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    client.next_chunk(&shell).await;
    client.ack(&shell).await;

    let listing = client
        .send_request(
            "projects.listEntries",
            json!({"cwd": directory.path().to_string_lossy()}),
        )
        .await;
    client.send(json!({"_tag": "Ping"})).await;

    // Raw, so the ordering assertion is about what actually crossed the wire.
    let first = client.recv().await;
    assert_eq!(
        first["_tag"], "Pong",
        "the listing held the read loop; nothing else could be answered: {first}"
    );

    let listed = client.await_outcome(&listing).await.expect_success();
    assert_eq!(listed["entries"].as_array().expect("entries").len(), 40 * 52);
    assert_eq!(listed["truncated"], json!(false));

    client.interrupt(&shell).await;
    client.close().await;
    server.stop().await;
}

/// Two calls the UI makes routinely that cannot be answered: a path the user is
/// still typing, and a project folder that has been moved or deleted since it
/// was registered. Each fails its own call with the error its method declares,
/// and the connection carries on.
#[tokio::test]
async fn a_path_that_cannot_be_read_fails_only_its_own_call() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let missing = directory.path().join("not-there");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = browse(&mut client, &inside(&missing.join("deeper")))
        .await
        .expect_declared("FilesystemBrowseError");
    assert_eq!(error["failure"], "read_directory_failed");
    assert_eq!(error["partialPath"], json!(inside(&missing.join("deeper"))));

    // An explicitly relative path with no project open. The palette sends this
    // whenever a user types `./` before choosing a project, so it is a refusal
    // the server owes a sentence rather than a stack trace.
    let error = browse(&mut client, "./src")
        .await
        .expect_declared("FilesystemBrowseError");
    assert_eq!(error["failure"], "current_project_required");
    assert!(error["message"].as_str().expect("a message").contains("./src"));

    let error = list_entries(&mut client, &missing)
        .await
        .expect_declared("ProjectListEntriesError");
    assert_eq!(error["failure"], "workspace_root_not_found");
    assert_eq!(error["cwd"], json!(missing.to_string_lossy()));
    assert!(error["message"]
        .as_str()
        .expect("a message")
        .contains(&*missing.to_string_lossy()));

    // Three refusals cost three calls and nothing else.
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));
    assert!(matches!(
        list_entries(&mut client, directory.path()).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// A deferred answer belongs to its `requestId` and to nothing else. Two
/// listings of different trees are sent back to back without waiting, and each
/// answer has to find its own caller — which is the property that makes running
/// them off the read loop safe in the first place.
#[tokio::test]
async fn concurrent_listings_are_correlated_by_request_id() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(directory.path(), &["first/only-here.txt", "second/and-here.txt"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client
        .send_request(
            "projects.listEntries",
            json!({"cwd": directory.path().join("first").to_string_lossy()}),
        )
        .await;
    let second = client
        .send_request(
            "projects.listEntries",
            json!({"cwd": directory.path().join("second").to_string_lossy()}),
        )
        .await;

    // Awaited in the opposite order to the one they were sent in, because the
    // harness correlates rather than assuming arrival order — and so does the
    // real client.
    let listed = client.await_outcome(&second).await.expect_success();
    assert_eq!(paths(&listed), ["and-here.txt"]);
    let listed = client.await_outcome(&first).await.expect_success();
    assert_eq!(paths(&listed), ["only-here.txt"]);

    client.close().await;
    server.stop().await;
}

/// A client that vanishes mid-listing must not leave the server holding
/// anything. The work finishes into a queue nobody is draining, the connection
/// is accounted for as gone, and the next client is served normally.
#[tokio::test]
async fn a_client_that_leaves_mid_listing_leaves_nothing_behind() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(directory.path(), &["src/main.rs"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;
    client
        .send_request(
            "projects.listEntries",
            json!({"cwd": directory.path().to_string_lossy()}),
        )
        .await;
    client.abandon();

    server.await_live_connections(0).await;
    assert_eq!(server.live_subscriptions(), 0);

    let mut client = server.connect().await;
    let listed = list_entries(&mut client, directory.path())
        .await
        .expect_success();
    assert_eq!(paths(&listed), ["src", "src/main.rs"]);

    // Nothing about the abandoned call was mistaken for a protocol problem.
    assert_eq!(server.unparseable_frames(), 0);
    assert_eq!(server.unrecognized_messages(), 0);

    client.close().await;
    server.stop().await;
}

/// The listing is bounded, and the UI is told when it hit the bound — it draws
/// a "· partial" badge from this flag. Twenty-five thousand entries is too many
/// to write out in a test, so what is checked here is that the flag exists,
/// decodes as a boolean, and is `false` for a workspace that fits.
#[tokio::test]
async fn a_listing_that_fits_is_not_reported_as_partial() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(directory.path(), &["one.txt", "two.txt"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = list_entries(&mut client, directory.path())
        .await
        .expect_success();
    assert_eq!(listed["truncated"], json!(false));

    client.close().await;
    server.stop().await;
}

/// Neither method is a subscription, so neither may leave one behind. Worth
/// asserting once: these are the first methods answered from somewhere other
/// than the read loop, and "answered elsewhere" is exactly what a subscription
/// also is.
#[tokio::test]
async fn reading_the_disk_opens_no_subscription() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    tree(directory.path(), &["src/main.rs"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    browse(&mut client, &inside(directory.path()))
        .await
        .expect_success();
    list_entries(&mut client, directory.path())
        .await
        .expect_success();

    assert_eq!(server.live_subscriptions(), 0);
    client.expect_silence(Duration::from_millis(50)).await;

    client.close().await;
    server.stop().await;
}
