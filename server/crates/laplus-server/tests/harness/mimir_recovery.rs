//! Causal persistence and replay-boundary regressions at the public bridge seam.
use super::*;

pub(super) async fn start_manual(peer: &Peer, server: &TestServer) -> (SocketClient, String) {
    peer.shared.state.lock().unwrap().manual_turns = true;
    let mut client = server.connect().await;
    dispatch(
        &mut client,
        create_project("project-1", &peer.shared.workspace),
    )
    .await;
    let mut thread = create_thread("project-1", "thread-1");
    thread["modelSelection"] = json!({"instanceId":"mimirLocal","model":"test/org/model"});
    dispatch(&mut client, thread).await;
    let subscription = client.watch_conversation("thread-1").await;
    dispatch(&mut client, follow_up("thread-1", "message-a", "A")).await;
    wait_for_request(peer, "request-1").await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["session"]["status"] == "running"
        })
        .await;
    (client, subscription)
}
pub(super) async fn wait_for_request(peer: &Peer, id: &str) {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let ready = {
                let state = peer.shared.state.lock().unwrap();
                state.active.as_deref() == Some(id) && !state.sinks.is_empty()
            };
            if ready {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("accepted request and event subscriber");
}
async fn snapshot_with(server: &TestServer, text: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let snapshot = server
                .connect()
                .await
                .into_thread_snapshot("thread-1")
                .await;
            if snapshot["thread"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text"] == text)
            {
                break snapshot["thread"].clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("observed text reached Laplus")
}
pub(super) async fn resumed_turn(client: &mut SocketClient, subscription: &str) -> Vec<Value> {
    // An attachment can report idle readiness after Starting and before the
    // queued prompt actually starts. Observe Running, not that attach snapshot.
    let mut events = client
        .values_until(subscription, |item| {
            item["event"]["payload"]["session"]["status"] == "running"
        })
        .await;
    events.extend(client.events_through_the_turn(subscription).await);
    events
}

fn assert_retired_draft(peer: &Peer, thread: &Value) {
    assert_eq!(thread["session"]["status"], "error");
    let error = thread["session"]["lastError"].as_str().unwrap();
    assert!(error.contains("history is incomplete"));
    assert!(error.contains("Retry a queued message or send again"));
    assert!(thread.to_string().contains("provider.history-gap"));
    let b = thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "message-b")
        .unwrap();
    assert_eq!(b["text"], "B queued");
    assert_eq!(b["deliveryState"], "retryable");
    let state = peer.shared.state.lock().unwrap();
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|r| r["action"] == "prompt")
            .count(),
        1,
        "B must not be sent on the ambiguous source"
    );
    assert!(state.requests.iter().any(|r| r["action"] == "release"));
    assert_eq!(
        state.cursors.len(),
        1,
        "do not attempt to fence the SDK pump with bridge cursors"
    );
}

async fn partial_survives_restart(fail_stream: bool) {
    let peer = Peer::start().await;
    let database = peer.directory.path().join("partial.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, subscription) = start_manual(&peer, &server).await;
    {
        let mut state = peer.shared.state.lock().unwrap();
        state.observe(
            false,
            json!({"text_delta":{"index":0,"value":"First unfinished block"}}),
        );
        state.observe(
            false,
            json!({"text_delta":{"index":1,"value":"Second unfinished block"}}),
        );
        state.observe(
            true,
            json!({"text_delta":{"index":0,"value":"Child must stay separate"}}),
        );
    }
    let before = snapshot_with(&server, "Second unfinished block").await;
    let first_id = before["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["text"] == "First unfinished block")
        .unwrap()["id"]
        .clone();
    if fail_stream {
        for sink in &peer.shared.state.lock().unwrap().sinks {
            sink.send(format!(
                "event: closed\ndata: {}\n\n",
                json!({"version":2,"session_id":SESSION})
            ))
            .unwrap();
        }
        client.events_through_the_turn(&subscription).await;
    }
    client.close().await;
    server.stop().await;
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let messages = snapshot["thread"]["messages"].as_array().unwrap();
    for text in ["First unfinished block", "Second unfinished block"] {
        let matching: Vec<_> = messages
            .iter()
            .filter(|m| m["role"] == "assistant" && m["text"] == text)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "already-observed partial reply must survive restart: {text}"
        );
        assert_eq!(matching[0]["streaming"], false);
    }
    assert_eq!(
        messages
            .iter()
            .find(|m| m["text"] == "First unfinished block")
            .unwrap()["id"],
        first_id
    );
    assert!(!messages
        .iter()
        .any(|m| m["text"] == "Child must stay separate"));
    server.stop().await;
}
#[tokio::test]
async fn mimir_recovery_shutdown_persists_all_unfinished_root_blocks() {
    partial_survives_restart(false).await;
}
#[tokio::test]
async fn mimir_recovery_stream_failure_persists_all_unfinished_root_blocks() {
    partial_survives_restart(true).await;
}

#[tokio::test]
async fn mimir_recovery_gap_cannot_attribute_retained_a_text_or_question_to_queued_b() {
    let peer = Peer::start().await;
    let server = TestServer::start_with(peer.config()).await;
    let (mut client, subscription) = start_manual(&peer, &server).await;
    peer.shared.state.lock().unwrap().observe(
        false,
        json!({"text_delta":{"index":0,"value":"Observed A"}}),
    );
    let before = snapshot_with(&server, "Observed A").await;
    let a_turn = before["session"]["activeTurnId"].clone();
    dispatch(&mut client, follow_up("thread-1", "message-b", "B queued")).await;
    {
        let mut state = peer.shared.state.lock().unwrap();
        assert_eq!(state.active.as_deref(), Some("request-1"));
        state.active = None;
        state.completed.push("request-1".into());
        // Snapshot is terminal A, while the same stream still holds A's tail.
        // Engine run ids intentionally differ from SDK accepted request ids.
        let tail = [
            json!({"observation":{"source":{"run_id":"engine-run-a","agent_id":"root-1","parent_agent_id":null,"turn":1,"model_attempt":0},"event":{"text_delta":{"index":2,"value":"STALE A replay"}}}}),
            json!({"observation":{"source":{"run_id":"engine-run-a","agent_id":"root-1","parent_agent_id":null,"turn":1,"model_attempt":0},"event":{"user_request_ready":{"request":{"id":"stale-question","questions":[{"id":"stale-q","prompt":"Wrong turn?","allow_multiple":false,"options":[]}]}}}}}),
            json!({"completed":{"request_id":"request-1","stop_reason":"end_turn","error":null}}),
        ];
        let mut bytes = format!(
            "event: gap\ndata: {}\n\n",
            json!({"version":2,"reason":"replay_unavailable","cursor":"random-opaque-epoch:1"})
        );
        for event in tail {
            let frame = format!(
                "id: random-opaque-epoch:{}\nevent: session\ndata: {}\n\n",
                state.events.len() + 1,
                json!({"version":2,"event":event})
            );
            state.events.push(frame.clone());
            bytes.push_str(&frame);
        }
        for sink in &state.sinks {
            sink.send(bytes.clone()).unwrap();
        }
    }
    let mut events = client.events_through_the_turn(&subscription).await;
    let retired = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_retired_draft(&peer, &retired["thread"]);
    dispatch(&mut client, json!({"type":"thread.turn.retry","commandId":"retry-b","threadId":"thread-1","createdAt":"2026-09-06T00:00:00Z"})).await;
    wait_for_request(&peer, "request-2").await;
    {
        let mut state = peer.shared.state.lock().unwrap();
        state.observe(
            false,
            json!({"text_delta":{"index":0,"value":"Only B reply"}}),
        );
        state.observe(false, json!({"content_block_stop":{"index":0}}));
        state.finish();
    }
    events.extend(resumed_turn(&mut client, &subscription).await);
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let messages = snapshot["thread"]["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|m| m["text"] == "Observed A" && m["turnId"] == a_turn));
    assert!(messages
        .iter()
        .any(|m| m["text"] == "Only B reply" && m["turnId"] != a_turn));
    assert!(
        !messages
            .iter()
            .any(|m| m["text"] == "STALE A replay" && m["turnId"] != a_turn),
        "retained A must never become B text"
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("stale-question"),
        "stale A question must never become answerable on B"
    );
    assert!(snapshot.to_string().contains("provider.history-gap"));
    assert_eq!(snapshot["thread"]["session"]["status"], "ready");
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_recovery_gap_while_running_preserves_queued_work_for_deliberate_retry() {
    for sdk_resync in [false, true] {
        let peer = Peer::start().await;
        let server = TestServer::start_with(peer.config()).await;
        let (mut client, subscription) = start_manual(&peer, &server).await;
        dispatch(&mut client, follow_up("thread-1", "message-b", "B queued")).await;
        {
            let mut state = peer.shared.state.lock().unwrap();
            // A is still running when the source is lost. Retirement must
            // release it, not wait for completion or silently send queued B.
            assert_eq!(state.active.as_deref(), Some("request-1"));
            if sdk_resync {
                state.emit(json!("resync"));
            } else {
                for sink in &state.sinks {
                    sink.send(format!("event: gap\ndata: {}\n\n", json!({"version":2,"reason":"replay_unavailable","cursor":"random-opaque-epoch:0"}))).unwrap();
                }
            }
        }
        client.events_through_the_turn(&subscription).await;
        let retired = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_retired_draft(&peer, &retired["thread"]);
        // No turn is running now, but a retryable queue still exists. Native
        // commands must not start a bridge or merge into that pending work.
        assert!(retired["thread"]["session"]["activeTurnId"].is_null());
        let request_count = peer.shared.state.lock().unwrap().requests.len();
        for (index, command) in ["/goal", "/goal pause", "/goal clear", "/init"]
            .into_iter()
            .enumerate()
        {
            let reply = client
                .call(
                    "orchestration.dispatchCommand",
                    follow_up("thread-1", &format!("pending-command-{index}"), command),
                )
                .await;
            assert!(
                matches!(reply, harness::Outcome::Failure(_)),
                "command accepted with pending work: {reply:?}"
            );
            assert!(format!("{reply:?}").contains("Wait for Mimir to become idle"));
        }
        assert_eq!(
            peer.shared.state.lock().unwrap().requests.len(),
            request_count,
            "rejected commands must not reach the bridge"
        );
        let unchanged = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_eq!(
            unchanged["thread"]["messages"],
            retired["thread"]["messages"]
        );

        dispatch(&mut client, json!({"type":"thread.turn.retry","commandId":"retry-b","threadId":"thread-1","createdAt":"2026-09-06T00:00:00Z"})).await;
        wait_for_request(&peer, "request-2").await;
        peer.shared.state.lock().unwrap().finish();
        resumed_turn(&mut client, &subscription).await;
        let snapshot = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_eq!(snapshot["thread"]["session"]["status"], "ready");
        assert!(snapshot.to_string().contains("provider.history-gap"));
        client.close().await;
        server.stop().await;
    }
}

#[tokio::test]
async fn mimir_recovery_sdk_tail_after_ready_cannot_reach_queued_b() {
    for sdk_resync in [false, true] {
        let peer = Peer::start().await;
        // Keep attachments as well as SQLite across the real server restart.
        let data = peer.directory.path();
        let server = TestServer::start_persistent_with_config_in(data, peer.config()).await;
        let (mut client, subscription) = start_manual(&peer, &server).await;
        peer.shared.state.lock().unwrap().observe(
            false,
            json!({"text_delta":{"index":0,"value":"Observed A"}}),
        );
        let before = snapshot_with(&server, "Observed A").await;
        let a_turn = before["session"]["activeTurnId"].clone();
        dispatch(&mut client, follow_up("thread-1", "message-b", "B queued")).await;
        let mut c = follow_up("thread-1", "message-c", "C queued");
        c["message"]["attachments"] = json!([{"type":"image","name":"c.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="}]);
        dispatch(&mut client, c).await;
        {
            let mut state = peer.shared.state.lock().unwrap();
            state.active = None;
            state.completed.push("request-1".into());
            // Both recovery snapshots remain terminal A. These events are not
            // in the bridge replay buffer until AFTER the recovery ready frame.
            state.tail_after_next_ready = vec![
                json!({"observation":{"source":{"run_id":"engine-run-a","agent_id":"root-1","parent_agent_id":null,"turn":1,"model_attempt":0},"event":{"text_delta":{"index":2,"value":"LATE A tail"}}}}),
                json!({"observation":{"source":{"run_id":"engine-run-a","agent_id":"root-1","parent_agent_id":null,"turn":1,"model_attempt":0},"event":{"user_request_ready":{"request":{"id":"late-a-question","questions":[{"id":"late-q","prompt":"Wrong turn?","allow_multiple":false,"options":[]}]}}}}}),
                json!({"completed":{"request_id":"request-1","stop_reason":"end_turn","error":null}}),
            ];
            if sdk_resync {
                state.emit(json!("resync"));
            } else {
                for sink in &state.sinks {
                    sink.send(format!("event: gap\ndata: {}\n\n", json!({"version":2,"reason":"replay_unavailable","cursor":"random-opaque-epoch:1"}))).unwrap();
                }
            }
        }
        let admitted_b = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                {
                    let state = peer.shared.state.lock().unwrap();
                    if state.active.as_deref() == Some("request-2") {
                        break true;
                    }
                    if state.requests.iter().any(|r| r["action"] == "release") {
                        break false;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("gap must retire the source or expose old queue admission");
        if admitted_b {
            peer.shared.state.lock().unwrap().finish();
        }
        let mut events = client.events_through_the_turn(&subscription).await;
        if admitted_b {
            events.extend(client.events_through_the_turn(&subscription).await);
        }
        let snapshot = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert!(
            !snapshot["thread"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["text"] == "LATE A tail" && m["turnId"] != a_turn),
            "SDK A tail appended after recovery ready must never become B text"
        );
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("late-a-question"),
            "late SDK A question must never become answerable on B"
        );
        assert!(
            !admitted_b,
            "an ambiguous SDK source must not automatically admit B"
        );
        assert_retired_draft(&peer, &snapshot["thread"]);
        client.close().await;
        server.stop().await;

        // Teardown must make the already-seen text and unsent draft durable.
        let server = TestServer::start_persistent_with_config_in(data, peer.config()).await;
        let restored = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        let messages = restored["thread"]["messages"].as_array().unwrap();
        let a = messages.iter().find(|m| m["text"] == "Observed A").unwrap();
        assert_eq!(a["turnId"], a_turn);
        assert_eq!(a["streaming"], false);
        let b = messages.iter().find(|m| m["id"] == "message-b").unwrap();
        assert_eq!(b["deliveryState"], "retryable");
        assert_eq!(b["text"], "B queued");
        let c = messages.iter().find(|m| m["id"] == "message-c").unwrap();
        assert_eq!(c["deliveryState"], "retryable");
        assert_eq!(c["text"], "C queued");
        assert_eq!(c["turnId"], b["turnId"]);
        assert_eq!(c["attachments"][0]["name"], "c.png");
        let mut client = server.connect().await;
        let subscription = client.watch_conversation("thread-1").await;
        dispatch(&mut client, json!({"type":"thread.turn.retry","commandId":"retry-b","threadId":"thread-1","createdAt":"2026-09-06T00:00:00Z"})).await;
        wait_for_request(&peer, "request-2").await;
        {
            let mut state = peer.shared.state.lock().unwrap();
            assert!(state.requests.iter().any(|r| r["action"] == "open"));
            assert_eq!(state.tokens.len(), 2, "retry launches a fresh owned bridge");
            let prompts: Vec<_> = state
                .requests
                .iter()
                .filter(|r| r["action"] == "prompt")
                .collect();
            assert_eq!(
                prompts.len(),
                2,
                "the entire queued draft retries exactly once"
            );
            assert_eq!(prompts[1]["input"]["text"], "B queued\n\nC queued");
            assert_eq!(prompts[1]["input"]["images"][0]["data"], json!([104, 105]));
            state.observe(
                false,
                json!({"text_delta":{"index":0,"value":"Only B reply"}}),
            );
            state.observe(false, json!({"content_block_stop":{"index":0}}));
            state.finish();
        }
        let events = resumed_turn(&mut client, &subscription).await;
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("late-a-question"));
        let resumed = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_eq!(resumed["thread"]["session"]["status"], "ready");
        assert!(!resumed.to_string().contains("LATE A tail"));
        assert!(resumed["thread"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "Only B reply" && m["turnId"] != a_turn));
        client.close().await;
        server.stop().await;
    }
}

#[tokio::test]
async fn mimir_wire_latest_turn_omits_absent_source_plan() {
    let peer = Peer::start().await;
    let database = peer.directory.path().join("source-plan.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, _) = start_manual(&peer, &server).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert!(
        snapshot["thread"]["latestTurn"]
            .get("sourceProposedPlan")
            .is_none(),
        "optional sourceProposedPlan must be absent rather than JSON null"
    );
    let mut queued = follow_up("thread-1", "message-b", "B queued");
    let source = json!({"threadId":"source-thread","planId":"source-plan"});
    queued["sourceProposedPlan"] = source.clone();
    dispatch(&mut client, queued).await;
    client.close().await;
    tokio::time::timeout(Duration::from_secs(60), server.stop())
        .await
        .expect("shutdown must observe closed input even with a queued draft");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let restored = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(
        restored["thread"]["latestTurn"]["sourceProposedPlan"],
        source
    );
    server.stop().await;
}

#[tokio::test]
async fn mimir_wire_checkpoint_status_is_contract_valid_live_and_after_restart() {
    for error in [None, Some("model failed")] {
        let peer = Peer::start().await;
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&peer.shared.workspace)
            .status()
            .unwrap()
            .success());
        std::fs::write(peer.shared.workspace.join("README.md"), "fixture").unwrap();
        let database = peer.directory.path().join("checkpoint.sqlite");
        let server = TestServer::start_at_with_config(&database, peer.config()).await;
        let (mut client, subscription) = start_manual(&peer, &server).await;
        {
            let mut state = peer.shared.state.lock().unwrap();
            let id = state.active.take().unwrap();
            state.completed.push(id.clone());
            state.emit(
                json!({"completed":{"request_id":id,"stop_reason":"end_turn","error":error}}),
            );
        }
        let events = client.events_through_the_checkpoint(&subscription, 1).await;
        let expected = if error.is_some() { "error" } else { "ready" };
        let event = events
            .iter()
            .find(|e| e["event"]["type"] == "thread.turn-diff-completed")
            .unwrap();
        assert_eq!(event["event"]["payload"]["status"], expected);
        let snapshot = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_eq!(snapshot["thread"]["checkpoints"][0]["status"], expected);
        client.close().await;
        server.stop().await;
        let server = TestServer::start_at_with_config(&database, peer.config()).await;
        let restored = server
            .connect()
            .await
            .into_thread_snapshot("thread-1")
            .await;
        assert_eq!(restored["thread"]["checkpoints"][0]["status"], expected);
        server.stop().await;
    }
}
