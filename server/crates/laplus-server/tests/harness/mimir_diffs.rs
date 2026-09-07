//! Mimir's real turn boundaries through the sidebar's existing Git diff RPCs.
//! The peer edits disk after prompt acceptance; it never injects checkpoints.
use super::*;
use recovery_tests::{start_manual, wait_for_request};

fn git(peer: &Peer, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(&peer.shared.workspace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repository(peer: &Peer) {
    git(peer, &["init", "--quiet"]);
    git(peer, &["config", "user.name", "Fixture"]);
    git(peer, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(peer.shared.workspace.join("tracked.txt"), "before\n").unwrap();
    git(peer, &["add", "."]);
    git(peer, &["commit", "--quiet", "-m", "before Mimir"]);
}

fn edit_files(peer: &Peer) {
    std::fs::write(peer.shared.workspace.join("tracked.txt"), "after Mimir\n").unwrap();
    std::fs::write(
        peer.shared.workspace.join("created.txt"),
        "new from Mimir\n",
    )
    .unwrap();
}

fn reply(peer: &Peer) {
    let mut state = peer.shared.state.lock().unwrap();
    state.observe(
        false,
        json!({"text_delta":{"index":0,"value":"Done editing"}}),
    );
    state.observe(false, json!({"content_block_stop":{"index":0}}));
}

fn changed_files(checkpoint: &Value) {
    let files = checkpoint["files"]
        .as_array()
        .expect("sidebar file summary");
    assert_eq!(files.len(), 2, "{checkpoint}");
    assert_eq!(files[0]["path"], "created.txt");
    assert_eq!(files[0]["kind"], "added");
    assert_eq!(files[0]["additions"], 1);
    assert_eq!(files[0]["deletions"], 0);
    assert_eq!(files[1]["path"], "tracked.txt");
    assert_eq!(files[1]["kind"], "modified");
    assert_eq!(files[1]["additions"], 1);
    assert_eq!(files[1]["deletions"], 1);
}

async fn full_diff(client: &mut SocketClient, through: u64) -> String {
    let response = client
        .call(
            "orchestration.getFullThreadDiff",
            json!({
                "threadId":"thread-1", "toTurnCount":through, "ignoreWhitespace":false
            }),
        )
        .await
        .expect_success();
    assert_eq!(response["fromTurnCount"], 0);
    response["diff"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn mimir_created_and_edited_files_reach_sidebar_summary_and_both_diff_rpcs_after_restart() {
    let peer = Peer::start().await;
    repository(&peer);
    let head = git(&peer, &["rev-parse", "HEAD"]);
    let database = peer.directory.path().join("diffs.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, subscription) = start_manual(&peer, &server).await;
    // The working-tree sidebar is live independently of completed-turn diffs.
    // No provider tool row, refresh RPC or completion triggers this update.
    let mut vcs = server.connect().await;
    let watch = vcs
        .subscribe("subscribeVcsStatus", json!({"cwd":peer.shared.workspace}))
        .await;
    let clean = vcs
        .values_until(&watch, |event| event["local"]["isRepo"] == true)
        .await;
    assert_eq!(
        clean.last().unwrap()["local"]["workingTree"]["files"],
        json!([])
    );
    server.await_watched_workspaces(1).await;
    edit_files(&peer);
    let changes = vcs
        .values_until(&watch, |event| {
            event["local"]["workingTree"]["files"]
                .as_array()
                .is_some_and(|files| files.len() == 2)
        })
        .await;
    let files = changes.last().unwrap()["local"]["workingTree"]["files"]
        .as_array()
        .unwrap();
    for path in ["created.txt", "tracked.txt"] {
        let file = files
            .iter()
            .find(|file| file["path"] == path)
            .expect("live changed path");
        assert_eq!(file["insertions"], 1);
        assert_eq!(file["deletions"], if path == "tracked.txt" { 1 } else { 0 });
    }
    let active = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(active["thread"]["session"]["status"], "running");
    assert_eq!(active["thread"]["checkpoints"], json!([]));
    let preview = vcs
        .call(
            "review.getDiffPreview",
            json!({"cwd":peer.shared.workspace, "ignoreWhitespace":false}),
        )
        .await
        .expect_success();
    let live_diff = preview["sources"].as_array().unwrap().iter()
        .find(|source| source["kind"] == "working-tree")
        .expect("the actual Working tree sidebar source")["diff"]
        .as_str().unwrap();
    assert!(
        live_diff.contains("+after Mimir") && live_diff.contains("+new from Mimir"),
        "{live_diff}"
    );
    vcs.close().await;
    reply(&peer);
    peer.shared.state.lock().unwrap().finish();
    let events = client.events_through_the_checkpoint(&subscription, 1).await;
    let checkpoint = &events
        .iter()
        .find(|e| e["event"]["type"] == "thread.turn-diff-completed")
        .expect("live sidebar invalidation")["event"]["payload"];
    changed_files(checkpoint);
    assert_eq!(checkpoint["status"], "ready");
    assert!(
        checkpoint["assistantMessageId"].is_string(),
        "the edit diff is linked to the reply: {checkpoint}"
    );
    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("-before") && diff.contains("+after Mimir"),
        "{diff}"
    );
    assert!(
        diff.contains("new file mode") && diff.contains("+new from Mimir"),
        "{diff}"
    );
    assert_eq!(full_diff(&mut client, 1).await, diff);
    assert_eq!(
        git(&peer, &["rev-parse", "HEAD"]),
        head,
        "checkpoints must not commit on the user's branch"
    );
    assert_eq!(
        git(&peer, &["diff", "--cached"]),
        "",
        "checkpoints must not stage the user's edits"
    );

    // A read-only follow-up uses the same checkpoint chain, but invents no edit.
    dispatch(
        &mut client,
        follow_up("thread-1", "read-only", "Read the file"),
    )
    .await;
    wait_for_request(&peer, "request-2").await;
    assert_eq!(
        std::fs::read_to_string(peer.shared.workspace.join("tracked.txt")).unwrap(),
        "after Mimir\n"
    );
    reply(&peer);
    peer.shared.state.lock().unwrap().finish();
    let events = client.events_through_the_checkpoint(&subscription, 2).await;
    let checkpoint = &events
        .iter()
        .find(|e| e["event"]["type"] == "thread.turn-diff-completed")
        .unwrap()["event"]["payload"];
    assert_eq!(checkpoint["files"], json!([]));
    assert_eq!(client.turn_diff("thread-1", 2).await, "");
    assert_eq!(full_diff(&mut client, 2).await, diff);
    client.close().await;
    server.stop().await;

    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let checkpoints = snapshot["thread"]["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 2);
    changed_files(&checkpoints[0]);
    assert_eq!(checkpoints[0]["status"], "ready");
    assert_eq!(checkpoints[1]["files"], json!([]));
    let mut client = server.connect().await;
    assert_eq!(client.turn_diff("thread-1", 1).await, diff);
    assert_eq!(full_diff(&mut client, 2).await, diff);
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_failed_turn_still_has_reviewable_edits_with_error_checkpoint() {
    let peer = Peer::start().await;
    repository(&peer);
    let database = peer.directory.path().join("failed-diff.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, subscription) = start_manual(&peer, &server).await;
    edit_files(&peer);
    reply(&peer);
    {
        let mut state = peer.shared.state.lock().unwrap();
        let id = state.active.take().unwrap();
        state.completed.push(id.clone());
        state.emit(json!({"completed":{"request_id":id,"stop_reason":"error","error":"model failed after editing"}}));
    }
    let events = client.events_through_the_checkpoint(&subscription, 1).await;
    let checkpoint = &events
        .iter()
        .find(|e| e["event"]["type"] == "thread.turn-diff-completed")
        .unwrap()["event"]["payload"];
    changed_files(checkpoint);
    assert_eq!(checkpoint["status"], "error");
    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("+after Mimir") && diff.contains("+new from Mimir"),
        "{diff}"
    );
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(snapshot["thread"]["session"]["status"], "error");
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_stopped_edits_stay_on_disk_without_relabelling_the_interrupted_turn() {
    let peer = Peer::start().await;
    repository(&peer);
    let database = peer.directory.path().join("stopped-diff.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, subscription) = start_manual(&peer, &server).await;
    edit_files(&peer);
    reply(&peer);
    dispatch(
        &mut client,
        harness::conversation::interrupt_turn("thread-1", None),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "interrupted");
    assert_eq!(snapshot["thread"]["checkpoints"], json!([]));
    assert_eq!(
        std::fs::read_to_string(peer.shared.workspace.join("created.txt")).unwrap(),
        "new from Mimir\n"
    );

    // Shared checkpoint semantics: interrupted changes remain in the next
    // completed turn's review, not a fabricated ready/error checkpoint for A.
    dispatch(
        &mut client,
        follow_up("thread-1", "continue", "Read what remains"),
    )
    .await;
    wait_for_request(&peer, "request-2").await;
    reply(&peer);
    peer.shared.state.lock().unwrap().finish();
    client.events_through_the_checkpoint(&subscription, 1).await;
    let resumed = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let checkpoint = &resumed["thread"]["checkpoints"][0];
    assert_ne!(
        checkpoint["turnId"],
        snapshot["thread"]["latestTurn"]["turnId"]
    );
    assert_eq!(checkpoint["status"], "ready");
    changed_files(checkpoint);
    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("+after Mimir") && diff.contains("+new from Mimir"),
        "{diff}"
    );
    client.close().await;
    server.stop().await;
}
