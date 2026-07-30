//! Reviewing the agent's work as a diff, driven the way the UI drives one.
//!
//! Ticket 20 at the seam the spec calls primary: a real socket, a real
//! repository in a temporary folder, a real turn, and the two calls the diff
//! panel makes. Nothing here reaches into the server — every checkpoint is one
//! the driver took because a turn ended, and every diff is asked for by method
//! tag with the payload the panel sends.
//!
//! ## The agent edits files here, and that is new
//!
//! Every ticket before this one could drive a turn against an agent that only
//! *talked*, because what was being tested was the conversation. A diff of a
//! turn that only talked is empty by definition, so `harness::agent` grew two
//! markers — [`writes`] and [`deletes`] — and the scripted agent now changes the
//! project in the middle of a turn, from the working directory the server
//! started it in. That is as close to a real edit as a stand-in gets: the server
//! is not told, and finds out the same way it would find out about the real
//! thing.
//!
//! ## What is deliberately driven twice
//!
//! Several tests assert both the *summary* on the thread's `checkpoints` and the
//! *patch* the diff call returns. They are read from git two different ways —
//! `--numstat`/`--name-status` for one, `--patch` for the other — so a test that
//! only checked one of them would pass while the panel showed a file list that
//! did not match the diff under it.

mod harness;

use harness::agent::{deletes, writes, ScriptedAgent, WORKING_DIRECTORY_MARKER};
use harness::conversation::{follow_up, start_turn};
use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};

/// The lines a scripted turn is made of, either side of whatever it does to the
/// project. Written out rather than replayed from a capture because no
/// recording contains a turn that edited *these* files.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Write"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// A repository holding `files` and one committed file of its own, which is what
/// a turn's diff is measured against.
///
/// It also ignores the marker the scripted agent drops into whatever directory
/// it was started in — that is how `socket_turn.rs` observes the agent's working
/// directory, and without ignoring it every diff here would carry a file no test
/// is about, and the turn that is supposed to change *nothing* would change one
/// thing. Ignoring it also happens to be the plainest check that a checkpoint
/// obeys `.gitignore`, which is what keeps `node_modules` out of a review.
fn a_repository(files: &[(&str, &str)]) -> Workspace {
    let workspace = Workspace::with(&[]);
    workspace.put("kept.txt", "one\ntwo\n");
    workspace.put(".gitignore", &format!("{WORKING_DIRECTORY_MARKER}\n"));
    for (path, contents) in files {
        workspace.put(path, contents);
    }
    workspace.init_repository().commit("the beginning");
    workspace
}

/// The whole conversation as one change.
async fn thread_diff(client: &mut harness::SocketClient, thread: &str, through: u64) -> String {
    let answered = client
        .call(
            "orchestration.getFullThreadDiff",
            json!({
                "threadId": thread,
                "toTurnCount": through,
                "ignoreWhitespace": false,
            }),
        )
        .await
        .expect_success();
    assert_eq!(
        answered["fromTurnCount"],
        json!(0),
        "a full-thread diff runs from the baseline: {answered}"
    );
    answered["diff"]
        .as_str()
        .unwrap_or_else(|| panic!("a diff is a string: {answered}"))
        .to_string()
}

/// The checkpoints on the thread, as a client that watched the whole
/// conversation would hold them.
async fn checkpoints(server: &TestServer, thread: &str) -> Vec<Value> {
    let client = server.connect().await;
    let snapshot = client.into_thread_snapshot(thread).await;
    snapshot["thread"]["checkpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("checkpoints are an array: {snapshot}"))
        .clone()
}

/// The whole ticket's first half: a turn happens, and what it did can be read
/// back as a patch.
///
/// Two files, because the two are read out of git by different routes and both
/// have to be there — a file it already tracked and a file it has never seen.
/// The untracked one is the interesting case and is a criterion of its own: a
/// brand new file is what an agent mostly produces, and `git diff` on a working
/// tree has nothing to say about one.
#[tokio::test]
async fn a_turn_is_reviewable_as_the_diff_of_what_it_changed() {
    let modified = writes("kept.txt", "one\ntwo\nthree\n");
    let created = writes("src/new.txt", "brand new\n");
    let agent = ScriptedAgent::emitting(&[
        INIT,
        modified.as_str(),
        created.as_str(),
        SAID,
        DONE,
    ]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "make a change"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(diff.contains("+three"), "the modified file is not in {diff}");
    assert!(
        diff.contains("src/new.txt") && diff.contains("+brand new"),
        "the file the agent created is not in {diff}"
    );
    assert!(
        diff.contains("new file mode"),
        "an untracked file has to arrive as an addition rather than as context: {diff}"
    );

    // …and the same two files, read out of git the other way, so the list the
    // panel shows above the patch cannot disagree with the patch.
    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(recorded.len(), 1, "{recorded:#?}");
    assert_eq!(recorded[0]["checkpointTurnCount"], json!(1));
    assert_eq!(recorded[0]["status"], "ready");
    assert_eq!(
        recorded[0]["files"]
            .as_array()
            .expect("a file list")
            .iter()
            .map(|file| (
                file["path"].as_str().unwrap_or_default(),
                file["kind"].as_str().unwrap_or_default(),
                file["additions"].as_u64().unwrap_or_default(),
            ))
            .collect::<Vec<(&str, &str, u64)>>(),
        vec![("kept.txt", "modified", 1), ("src/new.txt", "added", 1)]
    );

    server.stop().await;
}

/// The second half: two turns, and the conversation reads as one change as well
/// as two.
///
/// The discriminator is the *first* turn's file. It is in the cumulative diff
/// and not in the second turn's, which is the whole difference between the two
/// views and the reason both exist.
#[tokio::test]
async fn a_conversation_is_reviewable_both_step_by_step_and_as_one_change() {
    let first = writes("first.txt", "from turn one\n");
    let second = writes("second.txt", "from turn two\n");
    let agent = ScriptedAgent::per_turn(&[
        vec![INIT, first.as_str(), SAID, DONE],
        vec![second.as_str(), SAID, DONE],
    ]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "second"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 2)
        .await;

    let one = client.turn_diff("thread-1", 1).await;
    assert!(one.contains("first.txt"), "{one}");
    assert!(!one.contains("second.txt"), "turn one predates it: {one}");

    let two = client.turn_diff("thread-1", 2).await;
    assert!(two.contains("second.txt"), "{two}");
    assert!(
        !two.contains("first.txt"),
        "a single turn's diff must not carry the turn before it: {two}"
    );

    let whole = thread_diff(&mut client, "thread-1", 2).await;
    assert!(
        whole.contains("first.txt") && whole.contains("second.txt"),
        "the conversation is both turns at once: {whole}"
    );

    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(
        recorded
            .iter()
            .map(|checkpoint| checkpoint["checkpointTurnCount"].as_u64().unwrap_or_default())
            .collect::<Vec<u64>>(),
        vec![1, 2],
        "a conversation offers one turn per turn it has finished"
    );

    server.stop().await;
}

/// Every kind of change git can report about a file, in one turn.
///
/// The rename is the one worth spelling out: the agent deletes a file and writes
/// its contents back under another name, which is exactly what an editing agent
/// does, and git's own rename detection is what turns those two events back into
/// one. Nothing here asks for that — it is on by default and this is the test
/// that says so out loud.
#[tokio::test]
async fn a_diff_covers_files_that_were_added_modified_deleted_and_renamed() {
    let workspace = a_repository(&[
        ("doomed.txt", "goes away\n"),
        ("old-name.txt", "unchanged contents\n"),
    ]);

    let added = writes("added.txt", "new\n");
    let modified = writes("kept.txt", "one\ntwo\nthree\n");
    let removed = deletes("doomed.txt");
    let gone = deletes("old-name.txt");
    let renamed = writes("new-name.txt", "unchanged contents\n");
    let agent = ScriptedAgent::emitting(&[
        INIT,
        added.as_str(),
        modified.as_str(),
        removed.as_str(),
        gone.as_str(),
        renamed.as_str(),
        SAID,
        DONE,
    ]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "rearrange"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(diff.contains("new file mode"), "nothing was added: {diff}");
    assert!(diff.contains("deleted file mode"), "nothing was deleted: {diff}");
    assert!(diff.contains("+three"), "nothing was modified: {diff}");
    assert!(
        diff.contains("rename from old-name.txt") && diff.contains("rename to new-name.txt"),
        "the rename arrived as a delete and an add: {diff}"
    );

    let recorded = checkpoints(&server, "thread-1").await;
    let mut kinds: Vec<(&str, &str)> = recorded[0]["files"]
        .as_array()
        .expect("a file list")
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap_or_default(),
                file["kind"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            ("added.txt", "added"),
            ("doomed.txt", "deleted"),
            ("kept.txt", "modified"),
            ("new-name.txt", "renamed"),
        ]
    );

    server.stop().await;
}

/// A turn that only talked. There is nothing to show and that is not a failure:
/// the panel asks for the diff of whatever turn the developer clicked, and a
/// refusal would put an error where "this turn touched nothing" belongs.
#[tokio::test]
async fn a_turn_that_changed_nothing_is_an_empty_diff_rather_than_an_error() {
    let agent = ScriptedAgent::emitting(&[INIT, SAID, DONE]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "just talk"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    assert_eq!(client.turn_diff("thread-1", 1).await, "");
    assert_eq!(thread_diff(&mut client, "thread-1", 1).await, "");

    // The turn is still *offered*, with an empty file list. A turn missing from
    // the list would be a step of the conversation the developer cannot click on
    // to find out that it changed nothing.
    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(recorded.len(), 1, "{recorded:#?}");
    assert_eq!(recorded[0]["files"], json!([]));

    server.stop().await;
}

/// A binary file is named and not rendered. Nothing here asks for that either —
/// no `--binary` is passed, so git says so itself — and this is the test that
/// keeps it true.
#[tokio::test]
async fn a_binary_file_is_reported_without_its_contents() {
    // A NUL in the first stretch is what git itself reads as "binary".
    let binary: String = "\u{0}\u{1}\u{2}\u{3}".repeat(64);
    let written = writes("logo.bin", &binary);
    let agent = ScriptedAgent::emitting(&[INIT, written.as_str(), SAID, DONE]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "add a logo"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("logo.bin") && diff.contains("Binary files"),
        "a binary file has to be named rather than rendered: {diff}"
    );
    assert!(
        !diff.contains("GIT binary patch"),
        "no content should have been attempted: {diff}"
    );

    // A binary file has no lines, which is not the same as no lines changed. The
    // summary says zero either way and the patch above is what says which.
    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(recorded[0]["files"][0]["path"], "logo.bin");
    assert_eq!(recorded[0]["files"][0]["additions"], json!(0));

    server.stop().await;
}

/// A hand edit between turns belongs to the turn it happened in.
///
/// The developer changes a file themselves while the agent is not running, and
/// the next turn's diff carries it. That is the honest answer for a model built
/// on photographs of the working tree — see ADR-0008 — and it is the answer the
/// panel needs, because the question it asks is "what is different now", not
/// "who typed it".
#[tokio::test]
async fn a_hand_edit_between_turns_belongs_to_the_turn_it_happened_in() {
    let first = writes("agent.txt", "from the agent\n");
    let second = writes("agent.txt", "from the agent, twice\n");
    let agent = ScriptedAgent::per_turn(&[
        vec![INIT, first.as_str(), SAID, DONE],
        vec![second.as_str(), SAID, DONE],
    ]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    // The developer, between turns, in their own editor.
    workspace.put("by-hand.txt", "the developer wrote this\n");

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "second"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 2)
        .await;

    let one = client.turn_diff("thread-1", 1).await;
    assert!(
        !one.contains("by-hand.txt"),
        "the turn that finished before the edit must not claim it: {one}"
    );

    let two = client.turn_diff("thread-1", 2).await;
    assert!(
        two.contains("by-hand.txt") && two.contains("the developer wrote this"),
        "an edit made between two turns falls in the second of them: {two}"
    );
    assert!(
        two.contains("twice"),
        "and the agent's own change to the same turn is still there: {two}"
    );

    server.stop().await;
}

/// A diff past the far end of a conversation is refused with a sentence rather
/// than answered empty.
///
/// An empty diff means "this turn changed nothing", so answering one here would
/// tell the developer something false about a turn that does not exist. The
/// error is the method's own, or the client cannot decode it.
#[tokio::test]
async fn a_turn_the_conversation_has_not_reached_is_refused_by_name() {
    let agent = ScriptedAgent::emitting(&[INIT, SAID, DONE]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "one turn only"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let error = client
        .call(
            "orchestration.getTurnDiff",
            json!({"threadId": "thread-1", "fromTurnCount": 4, "toTurnCount": 5}),
        )
        .await
        .expect_declared("OrchestrationGetTurnDiffError");
    assert!(
        error["message"]
            .as_str()
            .expect("a sentence the panel can show")
            .contains("1 recorded turn"),
        "{error}"
    );

    let error = client
        .call(
            "orchestration.getFullThreadDiff",
            json!({"threadId": "thread-1", "toTurnCount": 5}),
        )
        .await
        .expect_declared("OrchestrationGetFullThreadDiffError");
    assert!(error["message"].is_string(), "{error}");

    server.stop().await;
}

/// A project with no repository has nowhere to keep a checkpoint. That is not a
/// failure of the turn and not a row in the work log — `vcs.init` is the door
/// out, and until the developer walks through it the conversation simply has no
/// turns to review.
///
/// **Two turns, and only the first one is asserted about.** There is no event to
/// wait for when a checkpoint is never taken, so what makes this deterministic
/// is the driver's own loop: the capture is awaited before the next prompt is
/// read, so a row the first turn was going to produce has certainly been
/// produced by the time the second turn settles.
#[tokio::test]
async fn a_project_that_is_not_a_repository_offers_no_turns_and_says_nothing_about_it() {
    let written = writes("made.txt", "no repository here\n");
    let agent = ScriptedAgent::per_turn(&[
        vec![INIT, written.as_str(), SAID, DONE],
        vec![SAID, DONE],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "make a change"),
        )
        .await
        .expect_success();
    let mut events = client.events_through_the_turn(&subscription).await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "and again"),
        )
        .await
        .expect_success();
    events.extend(client.events_through_the_turn(&subscription).await);

    assert!(
        harness::conversation::find_activity(&events, "checkpoint.failed").is_none(),
        "a project with no repository is not something that went wrong: {events:#?}"
    );
    assert!(
        harness::conversation::kinds(&events)
            .iter()
            .all(|kind| *kind != "thread.turn-diff-completed"),
        "there is nowhere to keep a checkpoint, so no turn may be offered: {events:#?}"
    );
    assert_eq!(checkpoints(&server, "thread-1").await, Vec::<Value>::new());

    let error = client
        .call(
            "orchestration.getTurnDiff",
            json!({"threadId": "thread-1", "fromTurnCount": 0, "toTurnCount": 1}),
        )
        .await
        .expect_declared("OrchestrationGetTurnDiffError");
    assert!(error["message"].is_string(), "{error}");

    server.stop().await;
}

/// A conversation that came back from a restart can still be reviewed.
///
/// Two halves have to survive and they survive in different places: the *tree*
/// is a ref in the developer's own repository, and the *turn list* is a row in
/// this server's database. A build that stored neither would still answer a
/// diff by luck, because the refs are derived from the thread id — so what this
/// pins is the list, which is the only way the panel knows the turn is there.
#[tokio::test]
async fn a_restored_conversation_still_offers_its_turns() {
    let written = writes("before.txt", "written before the restart\n");
    let agent = ScriptedAgent::emitting(&[INIT, written.as_str(), SAID, DONE]);
    let workspace = a_repository(&[]);
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("registry.sqlite");

    let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut client = server.connect().await;
    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "before"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;
    client.close().await;
    server.stop().await;

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut client = restarted.connect().await;

    let recorded = checkpoints(&restarted, "thread-1").await;
    assert_eq!(recorded.len(), 1, "{recorded:#?}");
    assert_eq!(recorded[0]["checkpointTurnCount"], json!(1));
    assert_eq!(recorded[0]["files"][0]["path"], "before.txt");

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(diff.contains("written before the restart"), "{diff}");

    restarted.stop().await;
}

/// A turn that failed is recorded as one.
///
/// `status` is not a fact about the capture — the client reads it straight back
/// into `latestTurn.state` (`threadReducer.ts`, `checkpointStatusToTurnState`),
/// so a failed turn carrying `ready` would arrive at the panel as a clean one
/// and quietly undo the settle the session had just published.
#[tokio::test]
async fn a_turn_that_failed_is_recorded_as_a_failure_rather_than_a_clean_one() {
    let written = writes("half-done.txt", "got this far\n");
    let agent = ScriptedAgent::emitting(&[
        INIT,
        written.as_str(),
        SAID,
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"duration_ms":10}"#,
    ]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "try something"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(recorded[0]["status"], "error", "{recorded:#?}");

    // The work is still reviewable. A turn that went wrong is the one a
    // developer most wants to look at.
    let diff = client.turn_diff("thread-1", 1).await;
    assert!(diff.contains("got this far"), "{diff}");

    server.stop().await;
}

/// A turn the developer stopped gets no checkpoint at all, and its changes fall
/// into the turn that follows.
///
/// The contract has three checkpoint statuses and the client maps two of them to
/// `completed` and one to `error`; **none of them means "interrupted"**. So a row
/// for a stopped turn — whatever it said — would relabel the turn as finished and
/// undo what ticket 14 settled. Not recording one is the cost, and this is what
/// it buys.
#[tokio::test]
async fn a_turn_the_developer_stopped_is_not_offered_for_review_on_its_own() {
    let abandoned = writes("half-written.txt", "the agent got this far\n");
    let corrected = writes("finished.txt", "and then this\n");
    let agent = ScriptedAgent::per_turn(&[
        vec![
            INIT,
            abandoned.as_str(),
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"working"}}}"#,
            // Held here until the server writes the interrupt it is about to
            // acknowledge — the same stop the recording in
            // `fixtures/claude-cli/11-interrupted-turn.ndjson` has.
            harness::agent::AWAIT_ANSWER,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"interrupt-1","response":{"still_queued":[]}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working"}]}}"#,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"num_turns":1,"duration_ms":10}"#,
        ],
        vec![corrected.as_str(), SAID, DONE],
    ]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "start something"),
        )
        .await
        .expect_success();
    let started = client.events_until_streaming(&subscription).await;
    let turn_id = harness::conversation::last_session(&started, "the running turn")["payload"]
        ["session"]["activeTurnId"]
        .as_str()
        .expect("a running session names its turn")
        .to_string();

    client
        .call(
            "orchestration.dispatchCommand",
            harness::conversation::interrupt_turn("thread-1", Some(&turn_id)),
        )
        .await
        .expect_success();
    let stopped = client.events_through_the_turn(&subscription).await;

    assert!(
        harness::conversation::kinds(&stopped)
            .iter()
            .all(|kind| *kind != "thread.turn-diff-completed"),
        "a stopped turn must not be offered, because no status could describe it: {:?}",
        harness::conversation::kinds(&stopped)
    );

    // The correction, which is the turn that *does* get recorded — and it
    // carries both halves, because nothing recorded a boundary between them.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "carry on"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let recorded = checkpoints(&server, "thread-1").await;
    assert_eq!(
        recorded
            .iter()
            .map(|checkpoint| checkpoint["checkpointTurnCount"].as_u64().unwrap_or_default())
            .collect::<Vec<u64>>(),
        vec![1],
        "two turns ran and only the one that finished is reviewable: {recorded:#?}"
    );

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("half-written.txt") && diff.contains("finished.txt"),
        "the stopped turn's work is reviewed as part of the turn that followed it: {diff}"
    );

    server.stop().await;
}

/// A diff too large to send is cut, and the cut is visible.
///
/// `ThreadTurnDiff` carries the patch as one string, so there is no flag to say
/// it was shortened — the notice has to be in the patch, which is what makes
/// this observable at all. Twelve megabytes because the ceiling is ten; a
/// generated file is the ordinary way a turn produces one.
#[tokio::test]
async fn a_diff_too_large_to_send_is_cut_and_says_so() {
    let enormous = "a line that is long enough to be worth counting\n".repeat(260_000);
    assert!(enormous.len() > 10_000_000, "the fixture is under the cap");

    let written = writes("generated.txt", &enormous);
    let agent = ScriptedAgent::emitting(&[INIT, written.as_str(), SAID, DONE]);
    let workspace = a_repository(&[]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "generate"),
        )
        .await
        .expect_success();
    client
        .events_through_the_checkpoint(&subscription, 1)
        .await;

    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.len() <= 10_000_000 + 200,
        "the patch was not bounded: {} bytes",
        diff.len()
    );
    assert!(
        diff.contains("truncated by laplus"),
        "a cut diff has to say it was cut, or a short patch reads as a small change"
    );
    assert!(
        diff.ends_with('\n'),
        "the notice is the last line rather than something a reader runs into mid-hunk"
    );

    server.stop().await;
}
