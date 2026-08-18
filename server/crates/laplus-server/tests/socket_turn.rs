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

use harness::agent::{ScriptedAgent, AWAIT_QUESTION, PAUSE, WORKING_DIRECTORY_MARKER};
use harness::conversation::{
    activity, assistant_sends, create_project, follow_up, interrupt_turn, kinds, last_session,
    settle_watch_between_turns, start_turn,
};
use harness::subagents::{child_row, child_stream};
use harness::workspace::Workspace;
use harness::TestServer;
use laplus_server::config::ServerConfig;
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

#[tokio::test]
async fn claude_receives_text_and_images_as_one_streaming_user_message() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    let mut command = start_turn("thread-1", "message-1", "compare these");
    command["message"]["attachments"] = json!([
        {"type":"image","name":"first.png","mimeType":"image/png","sizeBytes":1,"dataUrl":"data:image/png;base64,YQ=="},
        {"type":"image","name":"second.jpg","mimeType":"image/jpeg","sizeBytes":1,"dataUrl":"data:image/jpeg;base64,Yg=="},
        {"type":"image","name":"third.gif","mimeType":"image/gif","sizeBytes":2,"dataUrl":"data:image/gif;base64,aGk="},
        {"type":"image","name":"fourth.webp","mimeType":"image/webp","sizeBytes":3,"dataUrl":"data:image/webp;base64,Ynll"}
    ]);
    client.call("orchestration.dispatchCommand", command).await.expect_success();
    let subscription = client.watch_conversation("thread-1").await;
    client.events_through_the_turn(&subscription).await;
    assert_eq!(agent.prompts().len(), 1);
    assert_eq!(serde_json::from_str::<Value>(&agent.prompts()[0]).unwrap(), json!({"type":"user","session_id":"","parent_tool_use_id":null,"message":{"role":"user","content":[
        {"type":"text","text":"compare these"},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"YQ=="}},
        {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"Yg=="}},
        {"type":"image","source":{"type":"base64","media_type":"image/gif","data":"aGk="}},
        {"type":"image","source":{"type":"base64","media_type":"image/webp","data":"Ynll"}}
    ]}}));
    client.close().await; server.stop().await;
}

#[tokio::test]
async fn claude_receives_an_image_only_turn_without_an_empty_text_block() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    let mut command = start_turn("thread-1", "message-1", "");
    command["message"]["attachments"] = json!([{"type":"image","name":"screen.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="}]);
    client.call("orchestration.dispatchCommand", command).await.expect_success();
    let subscription = client.watch_conversation("thread-1").await;
    client.events_through_the_turn(&subscription).await;
    let prompt: Value = serde_json::from_str(&agent.prompts()[0]).unwrap();
    assert_eq!(prompt["message"]["content"], json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGk="}}]));
    client.close().await; server.stop().await;
}

#[tokio::test]
async fn claude_refuses_a_missing_stored_image_before_dispatch() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    let mut command = start_turn("thread-1", "message-1", "do not send this alone");
    command["message"]["attachments"] = json!([{"type":"image","id":"missing-image","name":"screen.png","mimeType":"image/png","sizeBytes":2}]);
    let refusal = client.call("orchestration.dispatchCommand", command).await.expect_declared("OrchestrationDispatchCommandError");
    assert!(refusal["message"].as_str().unwrap().contains("could not be resolved"), "{refusal:#}");
    assert!(agent.prompts().is_empty());
    client.close().await; server.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn claude_reports_an_unreadable_stored_image_without_sending_partial_input() {
    use std::os::unix::fs::PermissionsExt;

    let agent = ScriptedAgent::per_turn(&[
        vec![r#"{"type":"system","subtype":"init","session_id":"session-images"}"#, r#"{"type":"result","subtype":"success","is_error":false}"#],
        vec![r#"{"type":"result","subtype":"success","is_error":false}"#],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let preferences = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent.binary_path = agent.configured();
    let server = TestServer::start_persistent_with_config_in(preferences.path(), config).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    let mut first = start_turn("thread-1", "message-1", "remember this");
    first["message"]["attachments"] = json!([{"type":"image","name":"screen.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="}]);
    client.call("orchestration.dispatchCommand", first).await.expect_success();
    client.events_through_the_turn(&subscription).await;

    let stored = preferences.path().join("attachments/message-1-0.png");
    std::fs::set_permissions(&stored, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut second = follow_up("thread-1", "message-2", "do not send this alone");
    second["message"]["attachments"] = json!([{"type":"image","id":"message-1-0","name":"screen.png","mimeType":"image/png","sizeBytes":2}]);
    client.call("orchestration.dispatchCommand", second).await.expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let failed = &activity(&events, "session.failed")["payload"]["activity"];
    assert!(failed["summary"].as_str().unwrap().contains("could not be sent to the agent"), "{failed:#}");
    assert_eq!(agent.prompts().len(), 1, "Claude received an incomplete second prompt");

    std::fs::set_permissions(&stored, std::fs::Permissions::from_mode(0o600)).unwrap();
    client.close().await; server.stop().await;
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

/// A prompt sent while the agent is working goes in the queue **quietly**: the
/// conversation is still running the turn the developer is watching, and nothing
/// about accepting the next one is allowed to say otherwise.
///
/// The bug this pins was one event. The fresh-turn path published
/// `Session { status: Starting, activeTurnId: <the queued turn> }`
/// unconditionally, and the window then showed three wrong things at once from
/// that single publish:
///
/// - **"connecting"**, because `starting` is what `derivePhase` maps to it
///   (`apps/web/src/session-logic.ts`) — a conversation mid-turn reporting that
///   it has no agent yet.
/// - the running turn losing its working row, because `MessagesTimeline` takes
///   `runningTurnId` from `status === "running" ? activeTurnId : null`
///   (`apps/web/src/components/ChatView.tsx`).
/// - the running turn **settling mid-turn** at its next buffered message,
///   because the client's guard against exactly that is
///   `status === "running" && activeTurnId === turnId`
///   (`packages/client-runtime/src/state/threadReducer.ts`), and this event
///   falsified both halves of it.
///
/// Upstream guards the same publish the same way —
/// `ProviderCommandReactor.ts`'s
/// `if (options?.pendingTurnStart === true && thread.session?.status !== "running")`.
/// `.scratch/prompt-queueing/upstream-research.md` has the reading.
///
/// Driven against an agent that stops mid-turn, because that is the only way the
/// prompt can arrive while the first turn is genuinely in flight — the same
/// device `the_reply_is_readable_before_the_turn_has_finished` above uses, and
/// for the same reason.
#[tokio::test]
async fn a_prompt_sent_while_the_agent_is_working_is_queued_without_leaving_running() {
    let agent = ScriptedAgent::per_turn(&[
        vec![
            r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"working on it"}}}"#,
            PAUSE,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working on it, and done"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":1100,"total_cost_usd":0.01}"#,
        ],
        vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"and the second thing too"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":900,"total_cost_usd":0.01}"#,
        ],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "the first thing"),
        )
        .await
        .expect_success();

    // Far enough in that the agent has streamed something and is now sitting in
    // its pause. Everything the session has said so far — the `starting` this
    // turn is owed and the `running` that followed it — is consumed here, so
    // what the next read holds is only what the *follow-up* caused.
    let opening = client.events_until_streaming(&subscription).await;
    let first_turn = last_session(&opening, "the first turn")["payload"]["session"]
        ["activeTurnId"]
        .as_str()
        .expect("the first turn was named")
        .to_string();

    let mut follow_up = follow_up("thread-1", "message-2", "the second thing");
    follow_up["message"]["attachments"] = json!([{
        "type":"image", "name":"queued.png", "mimeType":"image/png",
        "sizeBytes":2, "dataUrl":"data:image/png;base64,aGk="
    }]);
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up,
        )
        .await
        .expect_success();

    // The rest of the *first* turn, up to and including the buffered message it
    // ends with. That message is the assertion's whole point: it is the event
    // the client's `turnStillRunning` guard is consulted on, so the window
    // between the queued prompt being accepted and this arriving is exactly
    // where a wrong session event does its damage.
    let queued = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
                && item["event"]["payload"]["streaming"] == json!(false)
        })
        .await;
    let said: Vec<(&str, Option<&str>)> = queued
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.session-set")
        .map(|event| {
            (
                event["payload"]["session"]["status"].as_str().unwrap_or(""),
                event["payload"]["session"]["activeTurnId"].as_str(),
            )
        })
        .collect();
    assert!(
        !said.iter().any(|(status, _)| *status == "starting"),
        "the queued prompt took the conversation out of `running`, so the pane \
         said `connecting` while the agent was working: {said:?}"
    );
    assert!(
        said.iter()
            .all(|(_, turn)| *turn == Some(first_turn.as_str()) || turn.is_none()),
        "a session event named the queued turn while the first one was still \
         running, which is what settles the running turn mid-turn: {said:?}"
    );
    assert_eq!(
        assistant_sends(&queued).last(),
        Some(&("working on it, and done".to_string(), false)),
        "the first turn's reply is not what this window ended on: {said:?}"
    );

    // The developer's second message is in the transcript straight away — it is
    // *queued*, not refused — and it carries a turn of its own, because laplus
    // queues where OpenCode steers.
    let queued_message = queued
        .iter()
        .map(|item| &item["event"])
        .find(|event| {
            event["type"] == "thread.message-sent"
                && event["payload"]["messageId"] == json!("message-2")
        })
        .expect("the queued prompt reached the transcript");
    assert_ne!(
        queued_message["payload"]["turnId"].as_str(),
        Some(first_turn.as_str()),
        "the queued prompt joined the running turn, which is steering rather \
         than queueing"
    );

    // And it then runs, on the same agent, as its own turn.
    let second = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        assistant_sends(&second).last(),
        Some(&("and the second thing too".to_string(), false))
    );
    assert_eq!(agent.starts(), 1, "the queued turn reached a second process");
    let sent: Value = serde_json::from_str(&agent.prompts()[1]).unwrap();
    assert_eq!(sent["message"]["content"], json!([
        {"type":"text","text":"the second thing"},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGk="}}
    ]));

    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let transcript: Vec<&str> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        transcript,
        vec![
            "the first thing",
            "working on it, and done",
            "the second thing",
            "and the second thing too",
        ]
    );
    assert_eq!(snapshot["thread"]["messages"][2]["attachments"], json!([{
        "type":"image","id":"message-2-0","name":"queued.png","mimeType":"image/png","sizeBytes":2
    }]));

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

/// How full the context window is, which the composer draws its meter from
/// (`apps/web/src/components/chat/ContextWindowMeter.tsx`, fed by
/// `deriveLatestContextWindowSnapshot`).
///
/// Replays the two-tool-call capture rather than a plain turn, because the whole
/// difficulty is in that capture: its `usage` reports 52,763 tokens across the
/// turn and the conversation is carrying 26,441. A meter reading the first would
/// show a 200k window twice as full as it is, and would climb further with every
/// tool call — so this asserts the smaller number, and asserts the larger one is
/// carried separately rather than dropped.
///
/// The client reads this from the thread snapshot as well as from the live
/// event, so both are checked: a developer opening a thread they have not
/// touched this session gets the meter from the snapshot alone.
#[tokio::test]
async fn a_turn_reports_how_full_the_context_window_is() {
    let agent = ScriptedAgent::replaying("06-several-tool-calls");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "read both files"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let rows: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| {
            event["type"] == "thread.activity-appended"
                && event["payload"]["activity"]["kind"] == "context-window.updated"
        })
        .map(|event| &event["payload"]["activity"])
        .collect();
    let readings: Vec<&Value> = rows.iter().map(|row| &row["payload"]).collect();

    // The climb. This capture carries ten sets of counts and reports five: five
    // assistant messages of which three repeat the one before, two
    // `message_delta`s, two `message_start`s this build does not read, and the
    // `result`. The meter moves when the conversation does and stays still when
    // it does not.
    //
    // 26322 and the first 26441 are the streaming readings — they arrive while
    // the message they belong to is still being written, which is the whole
    // difference between a meter that moves during a turn and one that moves
    // twice in it.
    assert_eq!(
        readings
            .iter()
            .map(|reading| reading["usedTokens"].clone())
            .collect::<Vec<Value>>(),
        vec![
            json!(26_089),
            json!(26_322),
            json!(26_402),
            json!(26_441),
            json!(26_441),
        ],
    );

    // Mid-turn there is no window to measure against — `modelUsage` arrives only
    // on the `result` — so the client draws a token count and no percentage
    // until the turn ends. On the turns after this one the window is remembered
    // and the percentage is there from the first reading.
    assert_eq!(readings[0]["maxTokens"], json!(null));
    assert_eq!(readings[0]["totalProcessedTokens"], json!(null));

    // The last two readings agree on the count and differ on everything else:
    // the `result` repeats what the final `message_delta` already said and adds
    // the window it is measured against. That is why an unchanged `usedTokens`
    // is not on its own a reason to stay silent.
    assert_eq!(readings[3]["usedTokens"], readings[4]["usedTokens"]);
    assert_eq!(readings[3]["maxTokens"], json!(null));

    // The turn's last word, and the reading the meter settles on.
    let settled = readings.last().expect("a settled reading");
    assert_eq!(settled["maxTokens"], json!(200_000));
    assert_eq!(settled["totalProcessedTokens"], json!(52_763));
    assert_eq!(settled["inputTokens"], json!(26_400));
    assert_eq!(settled["outputTokens"], json!(41));

    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let activities = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities");
    // Backwards, which is the direction the client reads them in
    // (`deriveLatestContextWindowSnapshot`): the newest row is the reading.
    let stored = activities
        .iter()
        .rev()
        .find(|activity| activity["kind"] == "context-window.updated")
        .expect("the meter's row survives into the snapshot");
    assert_eq!(stored["payload"]["usedTokens"], json!(26_441));
    assert_eq!(stored["turnId"], rows.last().expect("a row")["turnId"]);

    client.close().await;
    server.stop().await;
}

/// The meter fills from what the CLI *says* rather than from what this server
/// works out, which is ticket 76.
///
/// Replays `19-context-usage`, the capture of the exchange: the server asks
/// `get_context_usage` when the session announces itself and again when the turn
/// ends, and the CLI answers both. What it settles that the counts cannot is
/// three things, one per moment:
///
/// - **The opening turn has a percentage.** The window is in the first answer,
///   before any `result` has arrived — inference has nothing to measure against
///   until a turn has finished.
/// - **The tooltip's sentence has a source.** `isAutoCompactEnabled` appears
///   nowhere in the event stream, so before this the line the client renders from
///   it could never appear.
/// - **The answer beats the inference.** Both are available when the turn ends
///   and they disagree by 22 tokens; the CLI's own count is the one that stands.
#[tokio::test]
async fn the_meter_is_filled_from_what_the_agent_says_about_its_own_window() {
    let agent = ScriptedAgent::replaying("19-context-usage");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write a note about bicycles"),
        )
        .await
        .expect_success();

    // Read past the settle rather than up to it. The last answer arrives *after*
    // the turn has ended — it is a reply to a question asked on the `result` —
    // so `events_through_the_turn` would stop one row short of the reading the
    // meter comes to rest on.
    let events = client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "context-window.updated"
                && item["event"]["payload"]["activity"]["payload"]["usedTokens"] == json!(26_937)
        })
        .await;

    let readings: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| {
            event["type"] == "thread.activity-appended"
                && event["payload"]["activity"]["kind"] == "context-window.updated"
        })
        .map(|event| &event["payload"]["activity"]["payload"])
        .collect();

    // The first reading of the session is the CLI's answer, and it is a *whole*
    // one. Under ticket 40 alone the first row of a session carried a token count
    // and `maxTokens: null`, so the composer drew a figure with no percentage and
    // no bar until the turn ended.
    let opening = readings.first().expect("a reading before the turn ended");
    assert_eq!(opening["usedTokens"], json!(26_789));
    assert_eq!(opening["maxTokens"], json!(200_000));
    assert_eq!(opening["compactsAutomatically"], json!(true));

    // The one field no amount of inference reaches, on *every* row rather than
    // only on the two the answers produced. The client reads the newest row and
    // does not merge it with the ones before it, so a row that dropped this would
    // take the sentence out of the tooltip until the next answer arrived —
    // several times a turn, since every assistant message moves the meter.
    assert!(
        readings
            .iter()
            .all(|reading| reading["compactsAutomatically"] == json!(true)),
        "the sentence must not blink out between answers: {readings:#?}"
    );

    // The turn's last word. The `result` inferred 26,959 from its own counts and
    // the CLI answered 26,937 about the same conversation; the answer is what the
    // meter settles on, which is the precedence `completeTurn` uses upstream.
    let settled = readings.last().expect("a settled reading");
    assert_eq!(settled["usedTokens"], json!(26_937));
    assert_eq!(settled["maxTokens"], json!(200_000));
    assert!(
        readings
            .iter()
            .any(|reading| reading["usedTokens"] == json!(26_959)),
        "the inferred reading is still taken — it is what a CLI that refuses \
         leaves the meter on: {readings:#?}"
    );
    // Carried onto the answer rather than lost with the line it arrived on: the
    // total belongs to the turn, and the reading that follows the turn is still
    // about the same conversation.
    assert_eq!(settled["totalProcessedTokens"], json!(54_082));
    // The window has no input or output side to report, and says so rather than
    // claiming zero.
    assert_eq!(settled["inputTokens"], json!(null));
    assert_eq!(settled["outputTokens"], json!(null));

    // What the agent was actually asked, which no capture can contain because it
    // travelled on stdin. Two questions, at the two moments the driver asks.
    let asked: Vec<Value> = agent
        .answers()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] == "control_request")
        .collect();
    assert_eq!(asked.len(), 2, "{:?}", agent.answers());
    for question in &asked {
        assert_eq!(question["request"], json!({"subtype": "get_context_usage"}));
    }
    assert_ne!(
        asked[0]["request_id"], asked[1]["request_id"],
        "each question gets an id of its own"
    );

    // And the meter survives into the snapshot, which is where a developer
    // opening the thread tomorrow reads it from.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let stored = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .rev()
        .find(|activity| activity["kind"] == "context-window.updated")
        .expect("the meter's row survives into the snapshot");
    assert_eq!(stored["payload"]["usedTokens"], json!(26_937));
    assert_eq!(stored["payload"]["compactsAutomatically"], json!(true));

    client.close().await;
    server.stop().await;
}

/// A CLI that will not answer the question keeps the meter ticket 40 built.
///
/// The acceptance the whole fallback rests on, and the case this project may
/// actually meet: `get_context_usage` is an SDK control request, and a `claude`
/// that predates it answers with an error naming a callback it never registered.
/// Written rather than recorded, because the installed CLI implements the
/// request and cannot be asked to stop.
#[tokio::test]
async fn an_agent_that_will_not_say_leaves_the_inferred_meter_alone() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"session-1","model":"claude-opus-5","cwd":".","permissionMode":"default","tools":["Read"]}"#,
        // The server's question goes here — asked on `init`, refused on the next
        // line. Everything after it is a turn that never learns it was refused.
        AWAIT_QUESTION,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"context-1","error":"get_context_usage is not supported in this context (onGetContextUsage callback not registered)"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Refusing to introspect."}],"usage":{"input_tokens":30000,"output_tokens":40}}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":11,"usage":{"input_tokens":30000,"output_tokens":40,"iterations":[{"input_tokens":30000,"output_tokens":40}]},"modelUsage":{"claude-opus-5":{"contextWindow":200000}}}"#,
        // The question asked on the `result`, refused the same way. Nothing after
        // it, which is the point: the turn has already settled on the inferred
        // reading and nothing arrives to disturb it.
        AWAIT_QUESTION,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"context-2","error":"get_context_usage is not supported in this context (onGetContextUsage callback not registered)"}}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "introspect"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let readings: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| {
            event["type"] == "thread.activity-appended"
                && event["payload"]["activity"]["kind"] == "context-window.updated"
        })
        .map(|event| &event["payload"]["activity"]["payload"])
        .collect();

    // The meter is exactly what ticket 40 would have made of this turn.
    let settled = readings.last().expect("the counts still fill the meter");
    assert_eq!(settled["usedTokens"], json!(30_040));
    assert_eq!(settled["maxTokens"], json!(200_000));
    // Never said, so never claimed. The client renders the absence by leaving the
    // sentence out — which is what it did before this request existed.
    assert_eq!(settled["compactsAutomatically"], json!(null));

    // **Nothing reaches the conversation.** A refusal here is not the developer's
    // problem: the number they are looking at is already on screen, and a row
    // about how it got there would be the first thing they saw.
    let complaints: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .filter(|activity| activity["tone"] == "error")
        .collect();
    assert!(complaints.is_empty(), "{complaints:#?}");

    // The turn still ends the way any other does.
    let session = events
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .expect("the session settles");
    assert_eq!(session["payload"]["session"]["status"], "ready");

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

/// Driven against `fixtures/claude-cli/22-background-subagent.ndjson`, a
/// recording of a background subagent — the case where a developer could see
/// nothing at all, and where what they *did* see was the subagent's words
/// attributed to the agent they were talking to.
///
/// Three things at once, because they are one turn: the subagent gets a row that
/// runs and then finishes, its own messages stay out of the transcript, and the
/// two trailing `result` lines end the turn once.
#[tokio::test]
async fn a_background_subagent_gets_its_own_row_and_stays_out_of_the_transcript() {
    let agent = ScriptedAgent::replaying("22-background-subagent");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count to three in the background"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let activities: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .collect();

    // The subagent's row, keyed on the subagent rather than on the `Agent` call
    // that launched it — that call completes immediately, and sharing its key
    // would tick the subagent off at the moment it started.
    let subagent_rows: Vec<&Value> = activities
        .iter()
        .copied()
        .filter(|activity| {
            activity["payload"]["itemType"] == "collab_agent_tool_call"
                && activity["payload"]["data"]["taskId"].is_string()
        })
        .collect();
    assert!(
        !subagent_rows.is_empty(),
        "a running subagent was invisible: {activities:#?}"
    );
    let statuses: Vec<&str> = subagent_rows
        .iter()
        .filter_map(|row| row["payload"]["status"].as_str())
        .collect();
    assert!(
        statuses.contains(&"running"),
        "the subagent never showed as working: {statuses:?}"
    );
    assert_eq!(
        statuses.last(),
        Some(&"completed"),
        "the subagent never finished: {statuses:?}"
    );
    // Its final report reached the row, which is the thing the main agent then
    // answered from. `detail` is the bounded preview the log renders and `data`
    // is the record, so the whole answer is looked for in the record — this
    // subagent's report is long enough that its last line does not survive the
    // preview's truncation.
    let finished = subagent_rows.last().expect("a terminal subagent row");
    assert!(
        finished["payload"]["data"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("1 2 3 — done")),
        "the subagent's answer is missing: {finished:#?}"
    );
    assert!(
        finished["payload"]["detail"]
            .as_str()
            .is_some_and(|detail| !detail.trim().is_empty()),
        "the row previews nothing: {finished:#?}"
    );

    // The turn ended once, though the recording carries two `result` lines.
    let endings = activities
        .iter()
        .filter(|activity| activity["kind"] == "turn.completed")
        .count();
    assert_eq!(endings, 1, "the turn ended {endings} times");

    // And the agent was started able to say what its subagents are doing. Not a
    // setting: watching an agent work is what this application is for, so the
    // flag is pinned here rather than left to a configuration nobody sets.
    let argv = agent.arguments().join(" ");
    assert!(
        argv.contains("--forward-subagent-text"),
        "a subagent's own words never reach the row: {argv}"
    );

    // And the conversation is the developer's, not the subagent's. The subagent
    // counted "1", "2", "3" and reported on being refused permission; none of
    // that is something anyone in this conversation said.
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let said: Vec<String> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["text"].as_str())
        .map(str::to_string)
        .collect();
    assert!(
        said.iter().any(|text| text.contains("in the background")),
        "the agent's own answer is missing: {said:#?}"
    );
    assert!(
        !said.iter().any(|text| text.trim() == "1"),
        "the subagent's counting reached the transcript: {said:#?}"
    );

    client.close().await;
    server.stop().await;
}

/// One child's work stream as a client holds it, and everything the wire carried
/// to get it there.
///
/// The fold is the client's own: entries are upserted by id onto the snapshot
/// the subscription opened with, and ordered by sequence. The second half of the
/// answer is what makes a claim about loss or duplication mean anything —
/// **every entry id the wire carried, in the order it first appeared** — because
/// the fold alone would be just as satisfied by a server that sent the same
/// entry twice, an upsert applied twice landing on the state it already held.
fn folded_child_stream<'a>(
    snapshot: &Value,
    frames: impl Iterator<Item = &'a Value>,
) -> (Vec<Value>, Vec<String>) {
    let mut folded: Vec<Value> = snapshot["entries"].as_array().expect("entries").clone();
    let mut seen_ids: Vec<String> = folded
        .iter()
        .map(|entry| entry["id"].as_str().expect("an entry id").to_string())
        .collect();
    for item in frames {
        let Some(entry) = item.get("entry") else {
            continue;
        };
        let id = entry["id"].as_str().expect("an entry id").to_string();
        if !seen_ids.contains(&id) {
            seen_ids.push(id);
        }
        match folded.iter().position(|held| held["id"] == entry["id"]) {
            Some(index) => folded[index] = entry.clone(),
            None => folded.push(entry.clone()),
        }
    }
    folded.sort_by_key(|entry| entry["sequence"].as_i64().unwrap_or_default());
    (folded, seen_ids)
}

/// Open the child of `fixtures/claude-cli/22-background-subagent.ndjson`, drive
/// it, and read its work back through the socket.
///
/// The same seam the OpenCode tracer uses and everything is asserted the way a
/// client would learn it: the compact row a developer clicks, the subscription
/// that row addresses, and the snapshot a reload takes. Nothing here reaches
/// into the driver or the database.
///
/// The recording is paused just before the child's closing message, so the
/// stream is opened while the child is genuinely working. That is the
/// replay/live boundary — a client that lost an entry there, or was handed one
/// twice, or saw them out of order, would be indistinguishable from one that
/// never had the entry at all once the child was finished.
#[tokio::test]
async fn a_claude_child_work_stream_replays_and_then_continues_live() {
    let agent = ScriptedAgent::replaying_paused_before("22-background-subagent", "I counted to 3");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count to three in the background"),
        )
        .await
        .expect_success();

    // The launcher: one compact row, naming the child and carrying the reference
    // its work stream is addressed by. `taskId` is the CLI's own id for the
    // subagent — nothing here is minted by laplus.
    let child = "ab80091070230889d";
    let opening = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == child
        }),
    )
    .await
    .expect("the compact child row reaches the socket while the child is working");
    let row = opening
        .iter()
        .filter_map(|item| item["event"]["payload"].get("activity"))
        .find(|activity| activity["payload"]["data"]["childId"] == child)
        .expect("a row carrying the stream reference");
    assert_eq!(row["payload"]["itemType"], "collab_agent_tool_call");
    assert_eq!(row["payload"]["title"], "Subagent general-purpose");
    assert_eq!(row["payload"]["data"]["taskId"], child);

    // A second window, opening the child while it is still working. Its own
    // connection, because a developer inspecting a child has not closed the
    // conversation they are inspecting it from.
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "thread-1", "childId": child}),
        )
        .await;
    let replayed = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a child stream opens with itself")["snapshot"]
        .clone();
    assert_eq!(snapshot["stream"]["childId"], child);
    assert_eq!(snapshot["stream"]["name"], "general-purpose");
    assert_eq!(
        snapshot["stream"]["assignment"], "Count to three slowly",
        "the assignment is what the child was asked for, not what it is doing now"
    );
    assert_eq!(
        snapshot["stream"]["state"], "working",
        "a child that is still working must not read as finished: {snapshot:#?}"
    );
    assert_eq!(snapshot["stream"]["outcome"], Value::Null);
    assert_eq!(
        snapshot["stream"]["parentChildId"],
        Value::Null,
        "Claude proves no hierarchy here, so none may be drawn"
    );
    assert!(
        !snapshot["entries"]
            .as_array()
            .expect("entries")
            .is_empty(),
        "the child had already worked, and the replay lost it: {snapshot:#?}"
    );

    // Now let the recording finish, and fold what arrives the way a client does:
    // upsert by entry id, order by sequence.
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        // The *outcome*, not merely the state. A subagent ends twice on this
        // wire and the bare `task_updated` comes first, so the head reads
        // `completed` a moment before the report that says what it completed
        // with — waiting on the state alone would stop reading in that gap.
        inspector.values_until(&stream, |item| {
            item["kind"] == "stream-updated" && item["stream"]["outcome"]["kind"] == "completed"
        }),
    )
    .await
    .expect("the child's conclusion reaches its stream");

    let (folded, seen_ids) = folded_child_stream(&snapshot, replayed.iter().chain(live.iter()));

    // Everything the child did, in the order it did it: its prose, the three
    // commands it tried and what each of them came back with, its closing
    // message, and its report. The `detail` of a command is what that command
    // *returned*, which for this recording is the CLI refusing it three times.
    let read: Vec<(i64, &str, &str, &str)> = folded
        .iter()
        .map(|entry| {
            let payload = &entry["payload"];
            (
                entry["sequence"].as_i64().expect("a sequence"),
                entry["kind"].as_str().expect("a kind"),
                payload["command"].as_str().unwrap_or_default(),
                payload["status"]
                    .as_str()
                    .or_else(|| payload["kind"].as_str())
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "message", "", ""),
            (2, "command", "python3 -c \"import time; time.sleep(3)\"", "failed"),
            (
                3,
                "command",
                "python3 -c \"import time; time.sleep(3); print('pause 1 done')\"",
                "failed"
            ),
            (
                4,
                "command",
                "python3 -c \"import time; time.sleep(3)\"; echo two",
                "failed"
            ),
            (5, "message", "", ""),
            (6, "outcome", "", "completed"),
        ],
        "the child's stream lost, repeated or reordered its work across the \
         replay/live boundary: {folded:#?}"
    );
    assert_eq!(folded[0]["payload"]["text"], "1", "the child counting");
    assert!(
        folded[4]["payload"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("I counted to 3")),
        "the child's closing message: {:#?}",
        folded[4]
    );
    assert!(
        folded[5]["payload"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("1 2 3 — done")),
        "the child's report is the stream's terminal entry: {:#?}",
        folded[5]
    );
    assert_eq!(
        folded[1]["payload"]["detail"], "This command requires approval",
        "what the command came back with is its own error"
    );

    // And each of those was one entry rather than several. A `tool_use` and the
    // `tool_result` that claims it share the CLI's own call id, so a command
    // finishing moves the row it was already drawn on.
    assert_eq!(
        seen_ids,
        vec![
            format!("{child}:n:1"),
            format!("{child}:k:toolu_018P4BWzpkPTZVxuaZVhq1TL"),
            format!("{child}:k:toolu_012uWvvv3ZAJrL6Y3aXxRU1a"),
            format!("{child}:k:toolu_011MtuZgZ8pv9MccD3h1meMb"),
            format!("{child}:n:5"),
            format!("{child}:k:outcome"),
        ],
        "the child's stream carried an entry nothing asked for"
    );

    let concluded = live
        .iter()
        .rfind(|item| item["kind"] == "stream-updated")
        .expect("the child settles")["stream"]
        .clone();
    assert_eq!(concluded["state"], "completed");
    assert_eq!(concluded["outcome"]["kind"], "completed");

    // The root turn ended **once**, though a subagent started, worked, ran three
    // commands, spoke twice and finished inside it — and though the recording
    // ends with two `result` lines. A child boundary is not a turn boundary.
    let events = client.events_through_the_turn(&subscription).await;
    let conversation: Vec<&Value> = opening
        .iter()
        .chain(events.iter())
        .map(|item| &item["event"])
        .collect();
    let endings = conversation
        .iter()
        .filter(|event| event["type"] == "thread.activity-appended")
        .filter(|event| event["payload"]["activity"]["kind"] == "turn.completed")
        .count();
    assert_eq!(endings, 1, "the turn ended {endings} times");

    // **The compact row is the summary as well as the launcher**, and the
    // recording is what proves the two halves of it. While the child worked the
    // row showed what it was up to — the CLI's own account of it, and the
    // child's own words — and when the child reported, that replaced them
    // rather than being appended beside them.
    let details: Vec<&str> = conversation
        .iter()
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .filter(|activity| activity["payload"]["data"]["childId"] == child)
        .filter_map(|activity| activity["payload"]["detail"].as_str())
        .collect();
    assert!(
        details.contains(&"Running Pause 3 seconds"),
        "the row never said what the child was doing: {details:#?}"
    );
    assert!(
        details.contains(&"1"),
        "the row never said what the child itself said: {details:#?}"
    );
    let last = details.last().expect("a terminal row");
    assert!(
        last.contains("I counted to 3"),
        "the row ended on stale activity rather than on what came back: {details:#?}"
    );

    // And the child's own words arrived **after the root had gone quiet**. In
    // this recording the agent answers, stops, and only then does eleven lines
    // of subagent reach the wire — which is the ordering a background child is
    // for, read off the capture rather than scripted.
    let said_first = conversation
        .iter()
        .position(|event| {
            event["type"] == "thread.message-sent"
                && event["payload"]["role"] == "assistant"
                && event["payload"]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("off counting to 3 in the background"))
        })
        .expect("the root's own reply");
    let child_spoke = conversation
        .iter()
        .position(|event| {
            event["type"] == "thread.activity-appended"
                && event["payload"]["activity"]["payload"]["data"]["childId"] == child
                && event["payload"]["activity"]["payload"]["detail"] == "1"
        })
        .expect("the child's first forwarded words");
    assert!(
        said_first < child_spoke,
        "the child's work did not outlive the root's own reply: {said_first} then {child_spoke}"
    );

    inspector.close().await;
    client.close().await;

    // A reload: a connection that watched none of it replays the same stream,
    // and the conversation it belongs to still carries only the compact row.
    let mut reloaded = server.connect().await;
    let stream = reloaded
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "thread-1", "childId": child}),
        )
        .await;
    let snapshot = reloaded
        .next_chunk(&stream)
        .await
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a completed child replays")["snapshot"]
        .clone();
    let replayed_ids: Vec<&str> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(replayed_ids, seen_ids, "the replay is not the same stream");
    assert!(snapshot["stream"]["outcome"]["text"]
        .as_str()
        .is_some_and(|text| text.contains("1 2 3 — done")));
    reloaded.close().await;

    let thread = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    let said: Vec<&str> = thread["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["text"].as_str())
        .collect();
    assert!(
        !said.iter().any(|text| text.trim() == "1"),
        "the child's prose reached the parent transcript: {said:#?}"
    );
    let carried: Vec<&Value> = thread["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .filter(|activity| activity["payload"]["data"]["childId"] == child)
        .collect();
    assert!(!carried.is_empty(), "the snapshot lost the compact child row");
    assert!(
        carried
            .iter()
            .all(|activity| activity["payload"]["data"]["entries"].is_null()),
        "an ordinary thread snapshot carried the child's whole history: {carried:#?}"
    );

    server.stop().await;
}

/// The other recording, and the case it is the evidence for: a child that
/// answers in one word.
///
/// `fixtures/claude-cli/23-forwarded-subagent-text.ndjson` carries the shape
/// `22` does not — the **prompt** the child was handed, forwarded as a `user`
/// message on the child's own wire. It is the parent's words, so it is not one
/// of the child's messages, and a stream that recorded it would put the
/// assignment in the child's voice.
#[tokio::test]
async fn a_childs_prompt_is_not_one_of_the_things_the_child_said() {
    let agent = ScriptedAgent::replaying("23-forwarded-subagent-text");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "what is 2+2"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client.close().await;

    let child = "ae572c8d808b48d78";
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "thread-1", "childId": child}),
        )
        .await;
    let snapshot = inspector
        .next_chunk(&stream)
        .await
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child replays")["snapshot"]
        .clone();

    assert_eq!(snapshot["stream"]["name"], "general-purpose");
    assert_eq!(snapshot["stream"]["assignment"], "Compute 2+2");
    assert_eq!(snapshot["stream"]["state"], "completed");
    assert_eq!(snapshot["stream"]["outcome"]["kind"], "completed");
    assert_eq!(snapshot["stream"]["outcome"]["text"], "4");

    let read: Vec<(&str, &str)> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![("message", "4"), ("outcome", "4")],
        "the prompt the child was handed became something the child said: \
         {snapshot:#?}"
    );

    inspector.close().await;
    server.stop().await;
}

/// A Claude child's work stream comes back after the application is restarted.
///
/// Two processes over one database file, which is the only way to drive a real
/// restart from a test — `socket_opencode_turn`'s
/// `a_child_work_stream_replays_after_the_server_restarts` established the shape
/// and `socket_continuity.rs` the rest of it.
///
/// **Driven for Claude rather than argued from OpenCode.** Storage is
/// provider-neutral, and that is exactly the reasoning that lets a
/// provider-specific persistence bug through: what a child stream is made *of*
/// is this adapter's, and a work entry has more of it than the prose OpenCode's
/// restart test carries — a command line, an output, a status, a call id that
/// has to come back as the same entry rather than as a second one. So this
/// asserts the whole stream, ids included.
#[tokio::test]
async fn a_claude_child_work_stream_replays_after_the_server_restarts() {
    let agent = ScriptedAgent::replaying("22-background-subagent");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let child = "ab80091070230889d";

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "count to three in the background"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        // Ends the agents and *then* waits for the transcript queue, which is
        // what puts the child's last entry on the disk before the file is
        // handed to the next process.
        server.stop().await;
    }

    // A second process, which watched none of it.
    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut reopened = restarted.connect().await;
    let stream = reopened
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "thread-1", "childId": child}),
        )
        .await;
    let snapshot = reopened
        .next_chunk(&stream)
        .await
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a restored child replays")["snapshot"]
        .clone();

    assert_eq!(snapshot["stream"]["name"], "general-purpose");
    assert_eq!(snapshot["stream"]["assignment"], "Count to three slowly");
    assert_eq!(snapshot["stream"]["state"], "completed");
    assert_eq!(snapshot["stream"]["outcome"]["kind"], "completed");
    assert!(snapshot["stream"]["outcome"]["text"]
        .as_str()
        .is_some_and(|text| text.contains("1 2 3 — done")));

    let read: Vec<(i64, &str, &str)> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["sequence"].as_i64().expect("a sequence"),
                entry["kind"].as_str().expect("a kind"),
                entry["id"].as_str().expect("an id"),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "message", format!("{child}:n:1").as_str()),
            (
                2,
                "command",
                format!("{child}:k:toolu_018P4BWzpkPTZVxuaZVhq1TL").as_str()
            ),
            (
                3,
                "command",
                format!("{child}:k:toolu_012uWvvv3ZAJrL6Y3aXxRU1a").as_str()
            ),
            (
                4,
                "command",
                format!("{child}:k:toolu_011MtuZgZ8pv9MccD3h1meMb").as_str()
            ),
            (5, "message", format!("{child}:n:5").as_str()),
            (6, "outcome", format!("{child}:k:outcome").as_str()),
        ],
        "the restart did not bring the child's work back in order: {snapshot:#?}"
    );

    // And a work entry is the whole of what it was, rather than a row that
    // survived with its prose and lost what it did.
    let command = &snapshot["entries"][1]["payload"];
    assert_eq!(command["title"], "Bash");
    assert_eq!(command["status"], "failed");
    assert_eq!(command["command"], "python3 -c \"import time; time.sleep(3)\"");
    assert_eq!(command["detail"], "This command requires approval");

    // The conversation it belongs to came back beside it, still carrying the
    // compact row that launches this stream and none of the child's prose.
    let thread = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert!(
        thread["activities"]
            .as_array()
            .expect("activities")
            .iter()
            .any(|activity| activity["payload"]["data"]["childId"] == child),
        "the restored conversation lost the launcher: {thread:#?}"
    );
    assert!(
        !thread["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .any(|message| message["text"].as_str() == Some("1")),
        "the child's prose reached the restored transcript: {thread:#?}"
    );

    reopened.close().await;
    restarted.stop().await;
}

/// The case `22-background-subagent` does not contain, and the one a developer
/// actually hit.
///
/// In that recording the subagent finishes before any `result`, so the agent's
/// report lands in the turn that is still running. When the subagent takes longer
/// than the reply — which is the *point* of `run_in_background` — the order
/// inverts: the turn settles, and the report arrives at a session with nothing in
/// flight. Every word of it used to be discarded, so the developer watched the
/// work log tick along and was then told nothing, and asking again was the only
/// way to get the answer out.
///
/// Written rather than recorded because the timing is the whole scenario and a
/// capture cannot be made to have it on demand. Each line is a shape taken from
/// the recording: the `task_updated`/`task_notification` pair is lines 49 and 50
/// of it, bare one first.
#[tokio::test]
async fn a_subagent_that_finishes_after_the_turn_still_gets_its_report_to_the_developer() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"Count the variants","subagent_type":"Explore"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Launched it in the background."}]}}"#,
        // The turn ends here, with the subagent still working.
        r#"{"type":"result","subtype":"success","duration_ms":5600,"num_turns":1}"#,
        r#"{"type":"system","subtype":"task_updated","task_id":"t1","patch":{"status":"completed"}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_1","status":"completed","summary":"eleven variants"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The subagent found eleven variants."}]}}"#,
        r#"{"type":"result","subtype":"success","duration_ms":66000,"num_turns":1}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count the variants in the background"),
        )
        .await
        .expect_success();

    // Read past the *first* settle, which is where a reader of one turn stops.
    // The second is the turn the report opened, and its arrival is half of what
    // this test is about: a turn that opened and never settled would leave the
    // composer working forever, which is a worse bug than the one being fixed.
    let settles = std::cell::Cell::new(0usize);
    let watching = settle_watch_between_turns();
    let events = client
        .values_until(&subscription, move |item| {
            if watching(item) {
                settles.set(settles.get() + 1);
            }
            settles.get() >= 2
        })
        .await;

    let activities: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .collect();
    let endings = activities
        .iter()
        .filter(|activity| activity["kind"] == "turn.completed")
        .count();
    assert_eq!(
        endings, 2,
        "two turns ran — the one the developer asked for and the one the report \
         opened: {activities:#?}"
    );

    // The thing the developer was owed and was not given.
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let said: Vec<String> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["text"].as_str())
        .map(str::to_string)
        .collect();
    assert!(
        said.iter()
            .any(|text| text.contains("The subagent found eleven variants")),
        "the report never reached the conversation: {said:#?}"
    );

    // And it is attributed to a turn of its own rather than to the settled one,
    // so the transcript reads in the order it happened.
    let turns: Vec<&str> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| {
            message["text"]
                .as_str()
                .is_some_and(|text| text.contains("eleven variants") || text.contains("Launched it"))
        })
        .filter_map(|message| message["turnId"].as_str())
        .collect();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_ne!(turns[0], turns[1], "both replies landed in the same turn");

    client.close().await;
    server.stop().await;
}

/// A background child goes on working after the turn that spawned it has
/// settled — and its own boundaries settle nothing.
///
/// The half of the background case a recording cannot hold: in
/// `22-background-subagent` the child finishes inside its turn, and the whole
/// point of `run_in_background` is that it need not. So the shapes are the
/// recording's, taken line for line, and only the *timing* is written — the
/// `result` lands with the child still going, which is what puts everything
/// after it in a session with nothing in flight.
///
/// Three things at that seam, and they are one behaviour: the child's stream is
/// still open after the root settled, it goes on gaining entries and reaches its
/// conclusion there, and none of that opens or ends a turn. The turn the
/// developer sees afterwards is the one the *agent's own report* opens, which is
/// what `a_subagent_that_finishes_after_the_turn_still_gets_its_report_to_the_developer`
/// is about.
#[tokio::test]
async fn a_background_child_keeps_working_after_its_turn_has_settled() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"Count the variants","subagent_type":"Explore"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Launched it in the background."}]}}"#,
        // The turn ends here, with the subagent still working.
        r#"{"type":"result","subtype":"success","duration_ms":5600,"num_turns":1}"#,
        PAUSE,
        PAUSE,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"role":"assistant","content":[{"type":"text","text":"looking through the variants"}]}}"#,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_grep","name":"Grep","input":{"pattern":"variant","path":"src"}}]}}"#,
        r#"{"type":"user","parent_tool_use_id":"toolu_1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_grep","content":"eleven matches"}]}}"#,
        r#"{"type":"system","subtype":"task_updated","task_id":"t1","patch":{"status":"completed"}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_1","status":"completed","summary":"eleven variants"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The subagent found eleven variants."}]}}"#,
        r#"{"type":"result","subtype":"success","duration_ms":66000,"num_turns":1}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count the variants in the background"),
        )
        .await
        .expect_success();

    // Read to the moment the developer's turn is over. Everything after this
    // point is a child that nobody is waiting for.
    let first = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        first
            .iter()
            .map(|item| &item["event"])
            .filter(|event| event["payload"]["activity"]["kind"] == "turn.completed")
            .count(),
        1,
        "the developer's turn settled once: {first:#?}"
    );

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "thread-1", "childId": "t1"}),
        )
        .await;
    let replayed = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child is still there after its turn ended")["snapshot"]
        .clone();
    assert_eq!(
        snapshot["stream"]["state"], "working",
        "a settled turn ended the child with it: {snapshot:#?}"
    );
    assert_eq!(snapshot["stream"]["assignment"], "Count the variants");

    // And it goes on recording, with no turn in flight to record it into.
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        inspector.values_until(&stream, |item| {
            item["kind"] == "stream-updated" && item["stream"]["outcome"]["kind"] == "completed"
        }),
    )
    .await
    .expect("the child concludes after the turn that spawned it");

    let mut folded: Vec<Value> = snapshot["entries"].as_array().expect("entries").clone();
    for item in replayed.iter().chain(live.iter()) {
        let Some(entry) = item.get("entry") else {
            continue;
        };
        match folded.iter().position(|held| held["id"] == entry["id"]) {
            Some(index) => folded[index] = entry.clone(),
            None => folded.push(entry.clone()),
        }
    }
    folded.sort_by_key(|entry| entry["sequence"].as_i64().unwrap_or_default());
    let read: Vec<(&str, &str, &str)> = folded
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"]
                    .as_str()
                    .or_else(|| entry["payload"]["title"].as_str())
                    .unwrap_or_default(),
                entry["payload"]["status"]
                    .as_str()
                    .or_else(|| entry["payload"]["kind"].as_str())
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            ("message", "looking through the variants", ""),
            ("read", "Grep", "completed"),
            ("outcome", "eleven variants", "completed"),
        ],
        "the child's work after the settle: {folded:#?}"
    );
    assert_eq!(folded[1]["payload"]["query"], "variant");
    assert_eq!(folded[1]["payload"]["detail"], "eleven matches");

    // And none of it opened or ended a turn of its own. The second ending is the
    // agent's own report — a turn with a message in it — rather than anything a
    // child boundary published.
    let rest = client.events_through_the_next_turn(&subscription).await;
    let endings: Vec<&Value> = rest
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["payload"]["activity"]["kind"] == "turn.completed")
        .collect();
    assert_eq!(
        endings.len(),
        1,
        "a child boundary settled a turn of its own: {endings:#?}"
    );

    inspector.close().await;
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

/// **Ticket 06.** The conversation goes on reporting itself as working while a
/// background child does, and stops only when the child is terminal.
///
/// The case the spec's "a quiet or settled root does not make the thread idle"
/// is about, and the one no other provider can be made to produce on demand: the
/// developer's turn is **over** — it settled, and nothing is in flight — while
/// the child it launched is still counting. The sidebar draws *Working* from one
/// thing, a session whose status is `running` (`Sidebar.logic.ts`,
/// `resolveThreadStatusPill`), so that is what is asserted here, at the two
/// moments it must be true and false.
///
/// Note what the settle itself still does: the turn ends when the agent says it
/// has, carrying the state it reached. The session that follows it says
/// something else — that the conversation is not finished — and it names no
/// turn, which is why the client's reducer folds it without disturbing the turn
/// it has already settled.
#[tokio::test]
async fn the_conversation_stays_working_while_a_background_child_does() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"Count the variants","subagent_type":"Explore"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Launched it in the background."}]}}"#,
        // The turn ends here, with the subagent still working.
        r#"{"type":"result","subtype":"success","duration_ms":5600,"num_turns":1}"#,
        PAUSE,
        PAUSE,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"role":"assistant","content":[{"type":"text","text":"looking through the variants"}]}}"#,
        r#"{"type":"system","subtype":"task_updated","task_id":"t1","patch":{"status":"completed"}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_1","status":"completed","summary":"eleven variants"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The subagent found eleven variants."}]}}"#,
        r#"{"type":"result","subtype":"success","duration_ms":66000,"num_turns":1}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count the variants in the background"),
        )
        .await
        .expect_success();
    let settled = client.events_through_the_turn(&subscription).await;
    let last = settled
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("the turn settled")["event"]["payload"]["session"]
        .clone();
    assert_eq!(last["status"], "ready");
    assert_eq!(last["activeTurnId"], Value::Null);

    // And the very next thing the conversation says about itself is that it is
    // working — with no turn in flight to be working on.
    let held = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.session-set"
        })
        .await;
    let session = held
        .last()
        .expect("a session")["event"]["payload"]["session"]
        .clone();
    assert_eq!(
        session["status"], "running",
        "the conversation went idle with a child still working: {session:#?}"
    );
    assert_eq!(
        session["activeTurnId"],
        Value::Null,
        "a delegation tree is not a turn: {session:#?}"
    );
    let working = child_stream(&server, "thread-1", "t1").await;
    assert!(
        matches!(
            working["stream"]["state"].as_str(),
            Some("pending" | "working" | "blocked")
        ),
        "nothing was actually working: {working:#?}"
    );

    // It leaves Working with the last descendant, and not before.
    client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.session-set"
                && item["event"]["payload"]["session"]["activeTurnId"] == Value::Null
                && matches!(
                    item["event"]["payload"]["session"]["status"].as_str(),
                    Some("ready" | "idle")
                )
        })
        .await;
    let done = child_stream(&server, "thread-1", "t1").await;
    assert_eq!(
        done["stream"]["state"], "completed",
        "the conversation stopped working before its child did: {done:#?}"
    );
    assert_eq!(done["stream"]["outcome"]["text"], "eleven variants");

    client.close().await;
    server.stop().await;
}

/// **Ticket 06.** Stopping the parent stops the delegation tree, on the third of
/// the three providers — and on the case Claude is the only one that produces:
/// a conversation whose **only** remaining work is a background child.
///
/// The developer's own turn is already over when they press stop, so there is no
/// turn to interrupt and nothing but the tree to act on. That is exactly what
/// the composer sends in this state (`buildThreadTurnInterruptInput` omits the
/// turn id once the session is not running), and it is the reading
/// `Shell::interrupt_turn` has always taken of a stop with nothing to stop:
/// succeed. What must not also happen is that the child goes on.
#[tokio::test]
async fn stopping_a_claude_parent_stops_the_child_that_outlived_its_turn() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"Count the variants","subagent_type":"Explore"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Launched it in the background."}]}}"#,
        // The turn ends here, with the subagent still working.
        r#"{"type":"result","subtype":"success","duration_ms":5600,"num_turns":1}"#,
        PAUSE,
        PAUSE,
        // Everything past the stop is a provider still narrating a child the
        // developer has already ended.
        r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"role":"assistant","content":[{"type":"text","text":"looking through the variants"}]}}"#,
        r#"{"type":"system","subtype":"task_updated","task_id":"t1","patch":{"status":"completed"}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_1","status":"completed","summary":"eleven variants"}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count the variants in the background"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    let working = child_stream(&server, "thread-1", "t1").await;
    assert_eq!(
        working["stream"]["state"], "working",
        "there was nothing left to stop: {working:#?}"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", None),
        )
        .await
        .expect_success();

    let stopped = child_stream(&server, "thread-1", "t1").await;
    assert_eq!(stopped["stream"]["state"], "interrupted");
    assert_eq!(stopped["stream"]["outcome"]["kind"], "interrupted");
    assert_eq!(
        stopped["entries"]
            .as_array()
            .expect("entries")
            .last()
            .expect("a terminal entry")["kind"],
        "outcome"
    );
    // The CLI goes on regardless: more prose, its own completion, and a report.
    // None of it may reopen the child or replace the ending the developer asked
    // for — so this waits for the stream to move and fails if it ever does.
    // Written as a wait rather than a single later read because the lines it is
    // about arrive after the script's two pauses, and a read taken before them
    // would pass against a server that accepted every one.
    let moved = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = child_stream(&server, "thread-1", "t1").await;
            if snapshot["entries"] != stopped["entries"]
                || snapshot["stream"]["outcome"] != stopped["stream"]["outcome"]
            {
                return snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await;
    if let Ok(after) = moved {
        panic!("an interrupted child took live work after its ending: {after:#?}");
    }

    client.close().await;
    server.stop().await;
}

/// **The compact row and the child's stream must not contradict each other after
/// a Stop.**
///
/// The row is the surface the developer is actually looking at — the child's tab
/// may never have been opened — and it used to be the one surface a Stop could
/// not reach. `Shell::stop_the_delegation_tree` (`orchestration.rs`) called only
/// `Streams::interrupt` and `follow_delegation`, while every compact-row emitter
/// lives in a provider fold path (`turn.rs::fold`, `opencode.rs`, `codex.rs`),
/// so a Stop drew no row and nothing refused one afterwards. Two things followed
/// and this asserts against both:
///
/// 1. Immediately after the Stop the row still read `running` with its pre-stop
///    detail while the stream read `interrupted`. Now the developer's own
///    command draws the ending, because nothing else ever will: the CLI is not
///    told that a subagent it was running has been abandoned.
/// 2. The CLI goes on narrating a child the developer ended.
///    `Streams::record` refused all of it for the stream, but the row did not,
///    so the row settled on `tool.completed` / `status: "completed"` /
///    `detail: "eleven variants"` — the answer the developer had already
///    declined to wait for, presented as the child's ending. `session::spend`
///    now refuses it for the row too.
///
/// **`stopped`, not `interrupted`.** This test was first written asserting
/// `payload.status == "interrupted"`, and that was wrong: `status` is the
/// client's `WorkLogToolLifecycleStatus`, whose five literals are `inProgress`,
/// `completed`, `failed`, `declined` and `stopped`
/// (`session-logic.ts::extractWorkLogToolLifecycleStatus`). A word outside them
/// is read as *no status*, and a `tool.completed` with no status defaults to
/// `completed` — so the original assertion would have been satisfied by a row
/// the developer still saw as finished. `stopped` is the mapping the Codex
/// driver has always made for the same state.
#[tokio::test]
async fn a_stopped_claude_child_row_agrees_with_the_stream_it_belongs_to() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"toolu_1","description":"Count the variants","subagent_type":"Explore"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Launched it in the background."}]}}"#,
        r#"{"type":"result","subtype":"success","duration_ms":5600,"num_turns":1}"#,
        PAUSE,
        PAUSE,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_1","message":{"role":"assistant","content":[{"type":"text","text":"looking through the variants"}]}}"#,
        r#"{"type":"system","subtype":"task_updated","task_id":"t1","patch":{"status":"completed"}}"#,
        r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_1","status":"completed","summary":"eleven variants"}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "count the variants in the background"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", None),
        )
        .await
        .expect_success();

    let stopped = child_stream(&server, "thread-1", "t1").await;
    assert_eq!(stopped["stream"]["state"], "interrupted");

    // One: the ending is on the row already, before the CLI has said anything
    // at all — which is the only moment it can be, because the CLI will never
    // mention it.
    let ended = child_row(&server, "thread-1", "t1").await;
    assert_eq!(
        ended["payload"]["status"], "stopped",
        "the compact row disagreed with the stopped child's stream: {ended:#?}"
    );
    assert_eq!(ended["kind"], "tool.completed", "{ended:#?}");
    assert_eq!(
        ended["payload"]["data"]["toolCallId"], "subagent:t1",
        "the ending landed beside the child's row instead of on it: {ended:#?}"
    );
    assert_eq!(
        ended["payload"]["detail"], "Interrupted",
        "the row kept the line the child was on when it was stopped: {ended:#?}"
    );

    // Two: long enough for the three lines past the script's two pauses to be
    // folded, which is the window the second half of the defect lived in. The
    // row does not move — not to `completed`, and not onto "eleven variants".
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let after = child_row(&server, "thread-1", "t1").await;
    assert_eq!(
        after, ended,
        "narration after a Stop moved the stopped child's row: {after:#?}"
    );

    // And the stream it belongs to still says the same thing it did.
    let stream = child_stream(&server, "thread-1", "t1").await;
    assert_eq!(stream["stream"]["state"], "interrupted");
    assert_eq!(stream["stream"]["outcome"]["kind"], "interrupted");

    client.close().await;
    server.stop().await;
}
