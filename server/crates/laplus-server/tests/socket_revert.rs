//! Undoing a turn, driven the way the UI drives it.
//!
//! Ticket 05 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, a real repository in a temporary folder, a real turn that
//! edits it, and the command `client-runtime/src/operations/commands.ts` builds.
//! Nothing here reaches into the server, and every checkpoint is one the driver
//! took because a turn ended.
//!
//! **The revert control dispatched a command this server refused**, which is the
//! gap that prompted the whole audit: a developer could see exactly what a turn
//! had changed, in a panel built for the purpose, and had no way to put it back.
//!
//! ## What is asserted, and where
//!
//! The command is answered in two stages, so the tests are too. A dispatch
//! answers with a sequence and publishes `thread.checkpoint-revert-requested`;
//! the restore runs off the read loop and `thread.reverted` follows it. So
//! **every test that looks at files waits for the completion first** — a test
//! that read the tree on the answer would be racing a `git` and would fail on a
//! loaded machine rather than on a broken server.
//!
//! The files themselves are read from the workspace rather than from the server,
//! because a working tree is the one thing on this wire that no event describes.
//! `git status` is read alongside them for a second reason: it says the
//! developer's own index was never written, which is the property that makes a
//! revert safe to offer at all.

mod harness;

use harness::agent::{deletes, writes, ScriptedAgent, WORKING_DIRECTORY_MARKER};
use harness::conversation::{activity, create_project, create_thread, follow_up, start_turn};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// The lines a scripted turn is made of, either side of whatever it does to the
/// project. The same three `socket_diffs.rs` uses, and for the same reason: no
/// recording contains a turn that edited *these* files.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Write"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// The `thread.checkpoint.revert` `revertThreadCheckpoint` builds.
///
/// `createdAt` is sent because the contract requires it of the client. This
/// server ignores it — see `orchestration::RevertCheckpointPayload` — and
/// nothing below reads it back.
fn revert(thread_id: &str, turn_count: u64) -> Value {
    json!({
        "type": "thread.checkpoint.revert",
        "commandId": format!("test:revert:{thread_id}:{turn_count}"),
        "threadId": thread_id,
        "turnCount": turn_count,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// A repository with one committed file and one the developer left untracked.
///
/// The untracked file is a criterion of its own. A checkpoint photographs the
/// whole tree, tracked or not, so a revert has to put an untracked file back the
/// way the photograph recorded it — and an untracked file is exactly the thing a
/// `git checkout` would leave alone, which makes it the case a revert built out
/// of ordinary git commands would silently get wrong.
///
/// The marker the scripted agent drops into whatever directory it was started in
/// is ignored, as it is in `socket_diffs.rs`: without that every checkpoint here
/// would carry a file no test is about. It doubles as the plainest check that a
/// revert obeys `.gitignore` — nothing below ever puts it back, because nothing
/// ever takes it away.
fn a_repository() -> Workspace {
    let workspace = Workspace::with(&[]);
    workspace.put("kept.txt", "one\ntwo\n");
    workspace.put("doomed.txt", "still here\n");
    workspace.put(".gitignore", &format!("{WORKING_DIRECTORY_MARKER}\n"));
    workspace.init_repository().commit("the beginning");
    // After the commit, so the checkpoint is the only thing that ever records
    // it. A file git has never been told about is what an agent mostly produces.
    workspace.put("untracked.txt", "before the turn\n");
    workspace
}

/// Read a thread subscription up to and including the moment the working tree
/// has been put back.
///
/// One event later than the answer, and the gap between them is the whole shape
/// of this command: the dispatch records that a revert was asked for, and the
/// restore happens off the read loop because it touches a disk. A test that
/// looked at files on the answer would be racing it.
///
/// A *snapshot* does not end this read, unlike the checkpoint's own reader: the
/// server leaves the conversation alone across a revert, so there is nothing in
/// a snapshot that would say one had happened.
async fn events_through_the_revert(
    client: &mut SocketClient,
    subscription: &str,
    turn_count: u64,
) -> Vec<Value> {
    client
        .values_until(subscription, |item| {
            item["kind"] == json!("event")
                && item["event"]["type"] == json!("thread.reverted")
                && item["event"]["payload"]["turnCount"] == json!(turn_count)
        })
        .await
}

/// The event types a subscriber saw, in order.
///
/// Deliberately not `harness::conversation::kinds`, which renders anything that
/// is not an event as `<not an event>`: a subscription that fell behind is
/// re-described with a snapshot, and a test asserting the *order of two events*
/// would then fail on a busy machine for a reason it is not about. What is
/// asserted here is that the request precedes the completion, and a snapshot
/// between them says nothing either way.
fn event_types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|item| item["event"]["type"].as_str())
        .collect()
}

/// The whole ticket in one turn: every kind of change a turn can make, put back.
///
/// Four files and four different reasons a naive revert would leave one of them
/// wrong — a file the turn modified, one it created, one it deleted, and one git
/// had never been told about. They are asserted together rather than in four
/// tests because they are one restore, and a revert that got three of them right
/// is not three quarters of a working undo.
#[tokio::test]
async fn a_turn_is_undone_and_the_project_is_put_back_to_how_it_looked() {
    let modified = writes("kept.txt", "one\ntwo\nthree\n");
    let created = writes("src/new.txt", "brand new\n");
    let removed = deletes("doomed.txt");
    let clobbered = writes("untracked.txt", "after the turn\n");
    let agent = ScriptedAgent::emitting(&[
        INIT,
        modified.as_str(),
        created.as_str(),
        removed.as_str(),
        clobbered.as_str(),
        SAID,
        DONE,
    ]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "make a mess"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // Turn zero is the tree before the first turn ran, which is what the panel
    // names when the developer reverts the conversation's first message:
    // `max(0, n - 1)` in `ChatView.tsx`.
    client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();
    events_through_the_revert(&mut client, &subscription, 0).await;

    assert_eq!(
        workspace.read("kept.txt"),
        "one\ntwo\n",
        "a file the turn modified was not put back"
    );
    assert_eq!(
        workspace.read("doomed.txt"),
        "still here\n",
        "a file the turn deleted was not restored"
    );
    assert!(
        !workspace.path().join("src/new.txt").exists(),
        "a file the turn created is still there"
    );
    assert!(
        !workspace.path().join("src").exists(),
        "the directory the turn's file was alone in is still there"
    );
    assert_eq!(
        workspace.read("untracked.txt"),
        "before the turn\n",
        "an untracked file was not put back the way the checkpoint recorded it"
    );

    // Still untracked, which says the restore went through a scratch index and
    // never wrote the developer's own — a revert that staged their work would be
    // this server signing its name to their next commit.
    let status = workspace.git(&["status", "--porcelain"]);
    assert!(
        status.contains("?? untracked.txt"),
        "the developer's index was written: {status}"
    );
    assert!(
        !status.contains("kept.txt") && !status.contains("doomed.txt"),
        "the tracked files did not come back clean: {status}"
    );

    server.stop().await;
}

/// The command answers before the tree has been written, and says so afterwards.
///
/// Two events for one command, in order, with the completion carrying a later
/// sequence than the answer — which is the observable half of "the restore runs
/// off the read loop". The other half is what it buys, and there is no way to
/// assert a wait that did not happen; what can be asserted is that the client
/// was answered without one, and that the tree was not written until the second
/// event.
///
/// A second connection is driven alongside, because an event that reaches the
/// subscription that asked for it proves only that it was broadcast to the
/// caller.
#[tokio::test]
async fn a_revert_answers_first_and_publishes_its_completion_after_the_tree_is_written() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // A second window on the same conversation, opened before the revert.
    let mut watcher = server.connect().await;
    let watching = watcher.watch_conversation("thread-1").await;

    let answered = client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();
    let sequence = answered["sequence"]
        .as_i64()
        .unwrap_or_else(|| panic!("a revert answers with a sequence: {answered}"));

    let seen = events_through_the_revert(&mut client, &subscription, 0).await;
    assert_eq!(
        event_types(&seen),
        vec!["thread.checkpoint-revert-requested", "thread.reverted"],
        "a revert is a receipt and then an answer: {seen:#?}"
    );
    assert_eq!(
        seen[0]["event"]["sequence"], json!(sequence),
        "the answer names the sequence the request was committed at: {seen:#?}"
    );
    assert_eq!(seen[0]["event"]["payload"]["turnCount"], json!(0));
    assert!(
        seen[1]["event"]["sequence"]
            .as_i64()
            .is_some_and(|completion| completion > sequence),
        "the completion has to be committed after the request it completes: {seen:#?}"
    );

    // The tree is written by the time the completion is published, never before.
    assert!(
        !workspace.path().join("only.txt").exists(),
        "the completion outran the restore"
    );

    // And the other window heard the same two things.
    let elsewhere = events_through_the_revert(&mut watcher, &watching, 0).await;
    assert_eq!(
        event_types(&elsewhere),
        vec!["thread.checkpoint-revert-requested", "thread.reverted"],
        "a second window saw a different revert: {elsewhere:#?}"
    );

    server.stop().await;
}

/// A conversation of two turns, put back to the middle of itself.
///
/// The discriminator is the *first* turn's file: it survives, and the second
/// turn's does not. Without it a revert that simply emptied the project would
/// pass every other assertion in this file.
#[tokio::test]
async fn a_revert_names_a_turn_and_keeps_everything_before_it() {
    let first = writes("first.txt", "from turn one\n");
    let second = writes("second.txt", "from turn two\n");
    let agent = ScriptedAgent::per_turn(&[
        vec![INIT, first.as_str(), SAID, DONE],
        vec![second.as_str(), SAID, DONE],
    ]);
    let workspace = a_repository();
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
    client.events_through_the_checkpoint(&subscription, 1).await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "second"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 2).await;

    client
        .call("orchestration.dispatchCommand", revert("thread-1", 1))
        .await
        .expect_success();
    events_through_the_revert(&mut client, &subscription, 1).await;

    assert_eq!(
        workspace.read("first.txt"),
        "from turn one\n",
        "the turn before the one reverted to was undone as well"
    );
    assert!(
        !workspace.path().join("second.txt").exists(),
        "the turn reverted away is still in the tree"
    );

    server.stop().await;
}

/// A project that is one package inside a repository reverts that package, and
/// leaves the rest of the repository alone.
///
/// The developer commits somewhere else in the same repository between the turn
/// and the revert, which is the case that decides how the restore is seeded. A
/// revert seeded from `HEAD` would see that commit as a difference to resolve
/// and `read-tree -m` would abort the whole thing rather than pick a side —
/// seeded from the photograph, everything above the project's own folder starts
/// out agreeing and there is nothing to resolve. See `crate::checkpoints`.
///
/// Both halves are asserted, because either alone would pass for the wrong
/// reason: the package has to be put back, *and* the sibling has to still hold
/// what the developer committed.
#[tokio::test]
async fn a_project_inside_a_repository_reverts_itself_and_nothing_above_it() {
    let workspace = Workspace::with(&[]);
    workspace.put("pkg/kept.txt", "one\n");
    workspace.put("pkg/.gitignore", &format!("{WORKING_DIRECTORY_MARKER}\n"));
    workspace.put("elsewhere/sibling.txt", "before the commit\n");
    workspace.init_repository().commit("the beginning");

    let created = writes("new.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    // The project is the package, not the repository — which is the whole point
    // of the test and is why `open_conversation` is spelled out here.
    let package = workspace.path().join("pkg");
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", &package),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread("project-1", "thread-1"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("thread-1").await;

    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // The developer, meanwhile, commits in a part of the repository this
    // conversation has nothing to do with. `HEAD` moves.
    workspace.put("elsewhere/sibling.txt", "committed since\n");
    workspace.git(&["add", "elsewhere/sibling.txt"]);
    workspace.git(&["commit", "-m", "work outside the project"]);

    client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();
    events_through_the_revert(&mut client, &subscription, 0).await;

    assert!(
        !package.join("new.txt").exists(),
        "the package was not reverted"
    );
    assert_eq!(workspace.read("pkg/kept.txt"), "one\n");
    assert_eq!(
        workspace.read("elsewhere/sibling.txt"),
        "committed since\n",
        "a revert reached outside the project and undid the developer's own commit"
    );

    server.stop().await;
}

/// A revert does not reach the project list.
///
/// The spec asks every new change to declare whether it does, and this one says
/// no: the list renders a conversation's title, its session and its latest turn,
/// and a revert moves a working tree rather than any of the three.
///
/// Asserted by driving a change that *does* reach the list afterwards and
/// showing it is the next thing the list hears, rather than by waiting out a
/// silence — a test that asserted on elapsed time would be measuring the machine.
#[tokio::test]
async fn a_revert_does_not_reach_the_project_list() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // On its own connection, so what the list hears is not mixed with the
    // thread's own feed.
    let mut lists = server.connect().await;
    let shell = lists.subscribe("orchestration.subscribeShell", json!({})).await;
    lists.next_chunk(&shell).await;
    lists.ack(&shell).await;

    client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();
    events_through_the_revert(&mut client, &subscription, 0).await;

    // A rename is the nearest change that does reach the list, so it is what
    // proves the two revert events were not simply still in flight.
    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.meta.update",
                "commandId": "test:rename-after-revert",
                "threadId": "thread-1",
                "title": "renamed after the revert",
            }),
        )
        .await
        .expect_success();

    let heard = lists.next_chunk(&shell).await;
    lists.ack(&shell).await;
    assert_eq!(
        heard
            .iter()
            .map(|item| item["thread"]["title"].as_str().unwrap_or("<no thread>"))
            .collect::<Vec<&str>>(),
        vec!["renamed after the revert"],
        "the project list heard about a revert: {heard:#?}"
    );

    server.stop().await;
}

/// Reverting to the turn the tree already matches changes nothing and is not an
/// error.
///
/// A double-click, and the state a developer is in immediately after a turn ends
/// — the control is on the message, and nothing stops them pressing it twice.
#[tokio::test]
async fn reverting_to_the_turn_the_tree_already_matches_is_harmless() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    for _ in 0..2 {
        client
            .call("orchestration.dispatchCommand", revert("thread-1", 1))
            .await
            .expect_success();
        events_through_the_revert(&mut client, &subscription, 1).await;

        assert_eq!(
            workspace.read("only.txt"),
            "from the turn\n",
            "reverting to the tree that is already there moved it"
        );
        assert_eq!(workspace.read("kept.txt"), "one\ntwo\n");
        assert_eq!(workspace.read("untracked.txt"), "before the turn\n");
    }

    server.stop().await;
}

/// A revert moves the working tree and not the conversation.
///
/// The transcript, the work log and the list of turns a diff can be opened for
/// are all read back from a **fresh** subscriber, which is what proves they were
/// left alone rather than merely not re-broadcast.
#[tokio::test]
async fn a_revert_leaves_the_conversation_where_it_was() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    let before = server.connect().await.into_thread_snapshot("thread-1").await;

    client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();
    events_through_the_revert(&mut client, &subscription, 0).await;

    let after = server.connect().await.into_thread_snapshot("thread-1").await;
    for kept in ["messages", "activities", "checkpoints"] {
        assert_eq!(
            after["thread"][kept], before["thread"][kept],
            "a revert moved the conversation's {kept}"
        );
    }
    assert_eq!(after["thread"]["latestTurn"], before["thread"]["latestTurn"]);

    // The tree did move, which is what says the comparison above is about a
    // revert that happened rather than one that did nothing.
    assert!(!workspace.path().join("only.txt").exists());

    server.stop().await;
}

/// A restore that fails is said in the conversation, and is never published as a
/// completion.
///
/// The distinction is the point: `thread.reverted` is what the client folds as
/// "your project has been put back", so a failed revert that published one would
/// leave the developer believing a tree that still holds the turn they were
/// undoing.
///
/// Driven by removing the checkpoint refs behind the server's back, which is the
/// real shape of this failure rather than a contrivance — the refs live in the
/// developer's own repository, nothing here owns them, and a `git gc` or a hand
/// `update-ref -d` is all it takes. The registry still says the turn was
/// recorded, so the command is accepted and only the disk can refuse it, which
/// is exactly the case this arm exists for.
#[tokio::test]
async fn a_restore_that_fails_is_reported_as_a_failure_rather_than_a_completion() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    for reference in workspace
        .git(&["for-each-ref", "--format=%(refname)", "refs/laplus"])
        .lines()
        .map(str::to_string)
        .collect::<Vec<String>>()
    {
        workspace.git(&["update-ref", "-d", &reference]);
    }

    client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_success();

    let seen = client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == json!("revert.failed")
        })
        .await;
    assert!(
        !event_types(&seen).contains(&"thread.reverted"),
        "a failed revert was published as a finished one: {seen:#?}"
    );

    // `activity` hands back the whole event, so the row itself is one step in.
    let failure = &activity(&seen, "revert.failed")["payload"]["activity"];
    assert_eq!(failure["tone"], "error");
    assert!(
        failure["summary"]
            .as_str()
            .is_some_and(|said| said.contains("turn 0")),
        "the failure has to name the turn that was not restored: {failure}"
    );

    // And the tree is as the turn left it, which is what the sentence claims.
    assert_eq!(workspace.read("only.txt"), "from the turn\n");

    server.stop().await;
}

/// A revert with nothing behind it is refused, and the sentence names the turn.
///
/// The contract's dispatch error carries a message and nothing else
/// machine-readable, so the sentence *is* the diagnostic — and a refusal that
/// said only "no checkpoint" would leave a developer with two windows open
/// unable to tell which undo had failed.
#[tokio::test]
async fn a_revert_of_a_turn_with_no_checkpoint_is_refused_by_name() {
    let created = writes("only.txt", "from the turn\n");
    let agent = ScriptedAgent::emitting(&[INIT, created.as_str(), SAID, DONE]);
    let workspace = a_repository();
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;

    // Before any turn has finished, every turn is one with nothing behind it.
    let refusal = client
        .call("orchestration.dispatchCommand", revert("thread-1", 0))
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    let message = refusal["message"].as_str().unwrap_or_default();
    assert!(message.contains("turn 0"), "{message}");

    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a file"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // One turn recorded, so turn two is still past the end of the conversation.
    let refusal = client
        .call("orchestration.dispatchCommand", revert("thread-1", 2))
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    let message = refusal["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("turn 2") && message.contains("1 recorded turn"),
        "{message}"
    );

    // A conversation this server has never heard of is refused by name, not by
    // turn: there is no conversation to have a turn.
    let refusal = client
        .call("orchestration.dispatchCommand", revert("thread-9", 0))
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    let message = refusal["message"].as_str().unwrap_or_default();
    assert!(message.contains("thread-9"), "{message}");

    // Nothing was attempted: the file the turn wrote is still there.
    assert_eq!(workspace.read("only.txt"), "from the turn\n");

    server.stop().await;
}
