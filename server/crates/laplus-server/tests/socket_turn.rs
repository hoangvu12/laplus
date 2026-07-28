//! One complete agent turn, driven the way the UI drives one.
//!
//! This is ticket 10 at the seam the spec calls primary: a real socket, the
//! commands the real composer sends, and the subscription the real thread view
//! opens. Nothing here reaches into the server — the transcript is read out of
//! the events a client would fold, not out of a struct.
//!
//! **No test invokes the real API.** The agent is a scripted stand-in
//! (`harness::agent::ScriptedAgent`) injected through
//! `settings.providers.claudeAgent.binaryPath` — the same setting a developer
//! configures for real use, so the whole production path runs: resolution, the
//! child process, its stdio, the fold, the events, the socket. What differs is
//! the program on the other end of the pipe.
//!
//! Two of the scripts are *recordings*: `fixtures/claude-cli/*.ndjson`, the same
//! files `tests/protocol_golden.rs` holds the reducer to. A turn driven against
//! one of those is a turn against what `claude` actually said. The rest are
//! written for the occasion, for the cases a healthy CLI does not produce —
//! deltas that disagree with the buffered message, a failed result, a binary
//! that is not there.
//!
//! ## What the UI sends, and why the bootstrap matters
//!
//! A new conversation is a **client-side draft**. The composer subscribes to a
//! thread the server has never heard of, and the thread only reaches the server
//! when the first turn is dispatched — carrying, under `bootstrap.createThread`,
//! the thread it wants created (`apps/web/src/components/ChatView.tsx`, where
//! `isLocalDraftThread` decides it). A server that implemented only
//! `thread.create` would answer the real UI's first message with "there is no
//! such thread", so every turn here goes through the bootstrap the way the UI's
//! own does.
//!
//! What these tests do **not** do is watch the draft across its creation, and
//! that is ticket 28: a subscription to a thread the server does not have is
//! refused, because a client that is never sent a snapshot renders nothing from
//! the events that follow one. So the thread is created before it is watched —
//! see `SocketClient::open_conversation_in` — and
//! `a_draft_becomes_the_conversation_the_composer_is_watching` below is the one
//! test that drives the composer's path in full, refusal and retry included.
//!
//! Those payloads, and the helpers that read a conversation back off the wire,
//! live in `harness::conversation` — shared with `socket_continuity.rs`, because
//! two copies of "what the real UI sends" would be two chances for a test to keep
//! passing against a command that no longer exists.

mod harness;

use harness::agent::{ScriptedAgent, PAUSE, WORKING_DIRECTORY_MARKER};
use harness::conversation::{
    activity, assistant_sends, create_project, follow_up, kinds, start_turn,
};
use harness::workspace::Workspace;
use harness::TestServer;
use laplus_server::threads::Reconciliation;
use serde_json::{json, Value};

/// **Ticket 28.** The composer's own path, driven the way the real client drives
/// it: a conversation that does not exist is watched, refused, and watched again
/// until the first turn brings it into being.
///
/// The bug this pins was not that the events were wrong. Every event of the turn
/// was correct, in order, and on the subscription the composer had open — and
/// the window rendered none of it and sat on `Working for 3m 22s` against a turn
/// that finished in 5.4 seconds. `client-runtime/state/threads.ts` folds an
/// event only into a thread it already holds, and the only thing that gives it
/// one is a **snapshot**. A subscription that opened on a thread the server did
/// not have never sent one, so every event was dropped on arrival.
///
/// So what has to be true is not "the events arrive" — they always did — but
/// that the first thing the composer's subscription ever carries is a snapshot.
/// The refusal is what makes that happen: the client retries an expected failure
/// every 250ms (`subscribeDynamic`), and the retry that lands after the turn
/// created the thread opens with the conversation in it.
#[tokio::test]
async fn a_draft_becomes_the_conversation_the_composer_is_watching() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();

    // The composer, before the developer has typed anything: a thread id it
    // minted itself, which this server has never heard of.
    // Refused as a *declared* error: the client reads one of those as "ask again
    // in 250ms", and a defect as "this whole socket is broken".
    let refused = client
        .call(
            "orchestration.subscribeThread",
            json!({"threadId": "thread-1"}),
        )
        .await
        .expect_declared("OrchestrationGetSnapshotError");
    assert!(
        refused["message"]
            .as_str()
            .expect("a message")
            .contains("thread-1"),
        "the refusal names the thread: {refused}"
    );

    // The developer types. The thread reaches the server for the first time as
    // the turn that wants it created.
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();

    // And the retry finds it. `watch_draft` asserts the opening is a snapshot,
    // which is the whole of the fix: without one the client has nothing to fold
    // the rest of the turn into.
    let subscription = client.watch_draft("thread-1").await;
    let events = client.events_through_the_turn(&subscription).await;

    // The turn finishes on the subscription the composer is holding, so the
    // spinner has something to stop for.
    let settled = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("the session settled");
    assert_eq!(settled["event"]["payload"]["session"]["status"], "ready");

    // And the reply is there to be drawn, whether it arrived in the snapshot or
    // as events after it — which is the difference between the two the client
    // cannot see, and must not have to.
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let messages = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages");
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert_eq!(messages[0]["text"], "say ok");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["text"], "ok");

    client.close().await;
    server.stop().await;
}

/// The other half of the same rule: a client that says it already holds the
/// conversation is **not** refused, even for a thread this server has never had.
///
/// A browser keeps its cache in `localStorage` and this server keeps its
/// conversations in a registry, and the two can be reset independently — a fresh
/// database under a window that still has the thread is exactly the case. The
/// refusal is there to make a client with nothing to draw ask again; this one has
/// something to draw, so it gets the feed and is left to draw it.
///
/// It is also what `fixtures/socket-wire/01-browser-session.ndjson` captured the
/// reference server doing: request `3` carries `afterSequence` and is answered
/// with `synchronized` and no snapshot.
#[tokio::test]
async fn a_resume_of_a_thread_this_server_does_not_have_is_answered_not_refused() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({
                "threadId": "a-thread-only-the-client-remembers",
                "afterSequence": 2,
                "requestCompletionMarker": true,
            }),
        )
        .await;

    assert_eq!(
        client.next_chunk(&subscription).await,
        vec![json!({"kind": "synchronized"})],
        "an empty snapshot would be a claim that the client's own copy is wrong"
    );

    client.close().await;
    server.stop().await;
}

/// The whole ticket in one test: a prompt goes in, the reply comes back token by
/// token, and the transcript ends holding what the agent actually said.
///
/// Driven against `fixtures/claude-cli/02-streamed-turn.ndjson`, a recording of
/// a real streamed turn, so what is asserted is behaviour against the CLI's own
/// output rather than against this project's idea of it.
#[tokio::test]
async fn a_prompt_streams_a_reply_and_ends_with_the_buffered_message() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;

    // Acknowledged immediately: the answer is a log position, and it arrives
    // without anything having waited for a process to exist.
    let accepted = client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();
    assert!(
        accepted["sequence"].as_i64().expect("a sequence") > 0,
        "{accepted}"
    );

    let events = client.events_through_the_turn(&subscription).await;

    // The whole shape of a turn, in order. The client folds these into the
    // conversation, so their order *is* what the developer sees happen.
    //
    // It begins at the prompt rather than at `thread.created`, because the
    // thread exists before it is watched — see
    // [`SocketClient::open_conversation_in`], and
    // `a_draft_becomes_the_conversation_the_composer_is_watching` below for the
    // composer's own path, where the turn creates it.
    let seen = kinds(&events);
    assert_eq!(seen[0], "thread.message-sent", "{seen:?}");
    assert_eq!(seen[1], "thread.turn-start-requested", "{seen:?}");
    assert_eq!(*seen.last().expect("an end"), "thread.session-set");

    // The developer's own prompt is in the transcript before anything the agent
    // said about it.
    let prompt = &events[0]["event"]["payload"];
    assert_eq!(prompt["role"], "user");
    assert_eq!(prompt["text"], "say ok");
    assert_eq!(prompt["streaming"], json!(false));

    // The reply arrives in pieces and is then replaced by the buffered message.
    // The recording's reply is "ok", delivered as one delta that agrees with it.
    let sends = assistant_sends(&events);
    assert!(
        sends.len() >= 2,
        "the reply arrived in one jump rather than incrementally: {sends:?}"
    );
    assert!(
        sends[..sends.len() - 1].iter().all(|(_, streaming)| *streaming),
        "everything before the buffered message must be a streaming send: {sends:?}"
    );
    let (final_text, streaming) = sends.last().expect("a buffered message").clone();
    assert!(!streaming, "the turn must end with a non-streaming send");
    assert_eq!(final_text, "ok");
    assert_eq!(
        sends[..sends.len() - 1]
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<String>(),
        final_text,
        "the deltas and the buffered message disagreed on this recording"
    );

    // And the same reply is in the snapshot a client arriving late would take.
    let late = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = late["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["text"], "ok");
    assert_eq!(messages[1]["streaming"], json!(false));

    client.close().await;
    server.stop().await;
}

/// The criterion the reconciliation rule exists for: deltas are best-effort, and
/// when they are shed the transcript still has to say what the agent said.
///
/// No recording contains this, because a healthy CLI's deltas agree with its
/// buffered message — which is exactly why the rule needs a purpose-built script
/// to be tested at all.
#[tokio::test]
async fn the_transcript_holds_the_buffered_message_even_when_deltas_were_shed() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Read"]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        // Two of the five words the agent will turn out to have said.
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"the beginning "}}}"#,
        r#"{"type":"stream_event","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the beginning and the end"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":120,"total_cost_usd":0.0042}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "finish the sentence"),
        )
        .await
        .expect_success();

    let events = client.events_through_the_turn(&subscription).await;
    let sends = assistant_sends(&events);

    assert_eq!(
        sends,
        vec![
            ("the beginning ".to_string(), true),
            ("the beginning and the end".to_string(), false),
        ],
        "the buffered message did not replace the accumulation"
    );

    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(
        messages[1]["text"], "the beginning and the end",
        "the transcript was silently truncated to what streamed"
    );

    // Whether the two agreed is recorded, which is the continuous check on the
    // assumption that makes streaming safe: one message reconciled, none of them
    // built exactly by its deltas.
    assert_eq!(
        server.reconciliation(),
        Reconciliation {
            reconciled: 1,
            agreed: 0
        }
    );

    client.close().await;
    server.stop().await;
}

/// The other half of the same counter. A turn whose deltas *did* build the
/// buffered message has to be counted as agreeing, or the ratio is not a check
/// on anything.
#[tokio::test]
async fn deltas_that_agreed_with_the_buffered_message_are_recorded_as_agreeing() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert_eq!(
        server.reconciliation(),
        Reconciliation {
            reconciled: 1,
            agreed: 1
        }
    );

    client.close().await;
    server.stop().await;
}

/// "Not in one jump at the end", which is only observable against an agent that
/// takes its time: the deltas have to be readable while the turn is still
/// running, not merely earlier in the list once it is over.
#[tokio::test]
async fn the_reply_is_readable_before_the_turn_has_finished() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"thinking out loud"}}}"#,
        PAUSE,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"thinking out loud, and done"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":1100,"total_cost_usd":0.01}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "take your time"),
        )
        .await
        .expect_success();

    // Read only as far as the first piece of assistant text. The agent is still
    // mid-turn at this point — it has not sent its buffered message and will not
    // for about a second — so what this proves is that the developer has text on
    // screen before the turn is over.
    let so_far = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
        })
        .await;

    let sends = assistant_sends(&so_far);
    assert_eq!(sends, vec![("thinking out loud".to_string(), true)]);
    let session = so_far
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .expect("the session announced itself");
    assert_eq!(
        session["payload"]["session"]["status"], "running",
        "the turn had already finished, so this proves nothing about streaming"
    );

    // And then the rest of it arrives.
    let rest = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        assistant_sends(&rest).last(),
        Some(&("thinking out loud, and done".to_string(), false))
    );

    client.close().await;
    server.stop().await;
}

/// The three things a turn has to report about itself: which model it ran on,
/// how much latitude the agent had, and what the turn cost in time and money.
///
/// All three are read out of activities, which is the contract's own mechanism
/// for "something happened in this thread worth showing" — and which the UI's
/// work log renders for any kind it does not specifically suppress
/// (`apps/web/src/session-logic.ts`, `deriveWorkLogEntries`).
#[tokio::test]
async fn a_turn_reports_its_model_its_permission_mode_and_what_it_cost() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    // The session announcing itself publishes nothing — see `crate::turn`,
    // `Folded::Initialized` — so the recording's `init` line leaves no row, and
    // this asserts that rather than leaving the absence unremarked.
    assert!(
        harness::conversation::find_activity(&events, "session.init").is_none(),
        "the agent's own announcement is not a row: {:?}",
        harness::conversation::kinds(&events),
    );

    // What the turn cost, from the recording's `result` line.
    let completed = &activity(&events, "turn.completed")["payload"]["activity"];
    let summary = completed["summary"].as_str().expect("a summary");
    assert!(summary.contains("2.0s"), "{summary}");
    assert!(summary.contains("$0.0795"), "{summary}");
    assert_eq!(completed["payload"]["durationMs"], json!(2008));
    assert_eq!(completed["payload"]["isError"], json!(false));

    // The turn is settled with a beginning and an end, so its duration is also
    // structurally available rather than only in a sentence.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let turn = &snapshot["thread"]["latestTurn"];
    assert_eq!(turn["state"], "completed");
    assert!(turn["startedAt"].is_string(), "{turn}");
    assert!(turn["completedAt"].is_string(), "{turn}");

    // The model in force is on the thread, which is where the composer reads it.
    assert_eq!(
        snapshot["thread"]["modelSelection"]["model"],
        "claude-opus-5"
    );
    assert_eq!(snapshot["thread"]["runtimeMode"], "full-access");

    client.close().await;
    server.stop().await;
}

/// The agent's working directory is the project's folder. Observed by having the
/// agent write a file where it stands, because a relative path in a transcript
/// only means what the developer thinks it means if this is true.
#[tokio::test]
async fn the_agent_runs_in_the_projects_directory() {
    let agent = ScriptedAgent::replaying("01-buffered-turn");
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "where are you"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert!(
        workspace.path().join(WORKING_DIRECTORY_MARKER).exists(),
        "the agent ran somewhere other than {}",
        workspace.path().display()
    );

    client.close().await;
    server.stop().await;
}

/// One child for the whole conversation, and no child left behind.
///
/// Two turns, because "spawned once rather than per-request" is a claim about
/// the second one — and the claim is checked by counting *starts*, not by
/// reading the live-agent gauge. The gauge reads 1 either way: a re-spawn is a
/// decrement and an increment between two looks at it.
#[tokio::test]
async fn one_subprocess_serves_the_conversation_and_is_reaped_when_the_server_stops() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    assert_eq!(server.live_agents(), 0, "nothing runs before a turn");
    assert_eq!(agent.starts(), 0);

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    server.await_live_agents(1).await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "again"),
        )
        .await
        .expect_success();
    let second = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        agent.starts(),
        1,
        "the second turn started a second process instead of reusing the session"
    );
    assert_eq!(server.live_agents(), 1);

    // And the second turn ran as a turn rather than as a session that had never
    // begun: the session had to be `running` for it, which the agent's `init`
    // line — printed once per process, before the first turn — cannot supply.
    let ran = second
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.session-set")
        .map(|event| event["payload"]["session"]["status"].as_str().unwrap_or(""))
        .collect::<Vec<&str>>();
    assert!(
        ran.contains(&"running"),
        "the second turn never entered `running`: {ran:?}"
    );

    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    assert_eq!(
        snapshot["thread"]["latestTurn"]["state"], "completed",
        "the second turn did not settle"
    );

    client.close().await;
    // Stopping the server ends every session and waits for the children. A
    // `claude` that outlived the server would hold the project's files open and
    // keep talking to an API on the developer's account.
    server.stop().await;
}

/// An agent that cannot be started is reported in the conversation rather than
/// swallowed. The turn is still acknowledged — the failure is not the client's —
/// and the session says what went wrong in a sentence naming the file.
///
/// Driven with a configured path that **exists and is not a program**, which is
/// the one unusable case that does not fall back to `PATH` — see
/// `provider::resolve`, where the asymmetry is deliberate. A path that is simply
/// missing would fall through and find the developer's own install, which is
/// correct behaviour and would make this a test that starts a real agent.
#[tokio::test]
async fn an_agent_that_cannot_be_started_is_reported_in_the_conversation() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let configured = directory.path().join("claude-notes.txt");
    std::fs::write(&configured, "not a program").expect("writes the file");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&configured.to_string_lossy()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();

    let events = client.events_through_the_turn(&subscription).await;
    let failed = &activity(&events, "session.failed")["payload"]["activity"];
    let detail = failed["summary"].as_str().expect("a summary");
    assert!(
        detail.contains("claude-notes.txt"),
        "the diagnostic has to name the file it refused: {detail}"
    );
    assert_eq!(failed["tone"], "error");

    let ended = events
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .expect("the session said how it ended");
    assert_eq!(ended["payload"]["session"]["status"], "error");
    assert!(ended["payload"]["session"]["lastError"].is_string());

    // And the developer's prompt is still there to retry from.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    assert_eq!(snapshot["thread"]["messages"][0]["text"], "hello");

    client.close().await;
    server.stop().await;
}

/// The project list hears that a conversation exists and how it is getting on,
/// without carrying the conversation itself. A shell subscription that streamed
/// a token at a time would be the thread list re-rendering per character.
#[tokio::test]
async fn the_project_list_hears_about_the_thread_but_not_about_every_token() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let shell = client
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    let opening = client.next_chunk(&shell).await;
    client.ack(&shell).await;
    assert_eq!(opening[0]["snapshot"]["threads"], json!([]));

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say ok"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    // Read the shell until the turn has settled there too.
    let shell_events = client
        .values_until(&shell, |item| {
            item["thread"]["latestTurn"]["state"] == "completed"
        })
        .await;

    let upserts: Vec<&Value> = shell_events
        .iter()
        .filter(|item| item["kind"] == "thread-upserted")
        .collect();
    assert!(!upserts.is_empty(), "{shell_events:#?}");
    assert!(
        upserts.iter().all(|item| item["thread"]["id"] == "thread-1"
            && item["thread"].get("messages").is_none()),
        "the project list must carry summaries, not transcripts"
    );

    let deltas = assistant_sends(&events_of(&shell_events)).len();
    assert_eq!(deltas, 0, "a token delta reached the project list");

    // The summary is still the thread the detail subscription described.
    let latest = upserts.last().expect("an upsert");
    assert_eq!(latest["thread"]["latestTurn"]["state"], "completed");
    assert_eq!(latest["thread"]["session"]["status"], "ready");
    assert!(latest["thread"]["latestUserMessageAt"].is_string());

    client.close().await;
    server.stop().await;
}

/// Shell items are not thread events, so reading them as one has to yield
/// nothing rather than half-match.
fn events_of(items: &[Value]) -> Vec<Value> {
    items
        .iter()
        .filter(|item| item["kind"] == "event")
        .cloned()
        .collect()
}

/// A turn is refused when there is nothing to run it in. The message names the
/// project, because "not registered" without a name is not something a developer
/// can act on.
#[tokio::test]
async fn a_turn_for_a_project_this_server_does_not_know_is_refused_by_name() {
    let agent = ScriptedAgent::replaying("01-buffered-turn");
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let refusal = client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");

    let message = refusal["message"].as_str().expect("a message");
    assert!(message.contains("project-1"), "{message}");
    assert_eq!(server.live_agents(), 0, "a refused turn started an agent");

    client.close().await;
    server.stop().await;
}

/// Nothing here may reach the developer's own agent, and the way one could is a
/// test that forgot to configure a stand-in: the default `binaryPath` is a bare
/// name, which resolves on `PATH` to whatever is installed.
///
/// So this pins the default and pins that an unconfigured server starts nothing
/// on its own. Every test above configures a stand-in, and the one that drives a
/// *failure* is careful to drive the one failure that does not fall back to
/// `PATH` — see it for why.
#[tokio::test]
async fn an_unconfigured_server_starts_no_agent_of_its_own() {
    let server = TestServer::start().await;

    assert_eq!(
        server.config()["settings"]["providers"]["claudeAgent"]["binaryPath"],
        "claude",
        "a test that wants an agent has to configure one"
    );
    assert_eq!(server.live_agents(), 0);

    server.stop().await;
}
