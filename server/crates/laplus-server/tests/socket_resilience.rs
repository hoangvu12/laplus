//! Things going wrong, driven the way the UI would meet them.
//!
//! This is ticket 15 at the seam the spec calls primary. Every failure below is
//! driven through a real socket against a scripted stand-in `claude`, and every
//! assertion is on what a client would fold — an activity, a session status, a
//! transcript — rather than on anything inside the server.
//!
//! ## The shape of the ticket
//!
//! Four kinds of thing go wrong, and they are different in kind rather than in
//! severity:
//!
//! - **The agent reports a failed turn.** The conversation says so and the
//!   session is still there to retry in. This is the ordinary case and it must
//!   not end anything.
//! - **The agent stops being a process.** Nobody asked, the turn will never
//!   finish, and the only account of why is whatever it managed to say on
//!   stderr. The window survives, the transcript survives, and the next turn
//!   starts a replacement.
//! - **The CLI says something this build cannot read.** Counted, not fatal —
//!   and the count is put where a developer will see it, because a number
//!   nobody renders is not an early-warning system.
//! - **The session runs long.** Compaction and rate limits, neither of which is
//!   a failure and both of which change what the next turn can do.
//!
//! ## Why the drift assertions are on a sentence
//!
//! `turn.completed` carried `unknownEvents` and `parseErrors` in its payload
//! before this ticket, and the UI's work log renders a row's `summary` and its
//! `detail` — not arbitrary payload keys. So the counters were, in the only
//! sense that matters, not exposed. The assertions here are on the summary for
//! that reason, with the session's running totals checked in the payload beside
//! it.

mod harness;

use harness::agent::{ScriptedAgent, DIES, LAST_WORDS};
use harness::conversation::{
    activities_of, activity, assistant_sends, find_activity, follow_up, last_session, start_turn,
};
use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};

/// The `system`/`init` line every session opens with.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"session-15","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Read"]}"#;

/// The `result` a turn that went well ends with.
const WENT_WELL: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":300,"total_cost_usd":0.002}"#;

/// A healthy turn that says one thing, for the scripts that need one after
/// something has gone wrong.
///
/// Written out per test rather than formatted from the text, because these lines
/// are the CLI's own JSON and a script built with `format!` is a test asserting
/// against something no capture contains.
fn a_good_turn(delta: &'static str, buffered: &'static str) -> Vec<&'static str> {
    vec![
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        delta,
        buffered,
        WENT_WELL,
    ]
}

/// The agent says the turn failed. The conversation says so, in the agent's own
/// words, and the session is still there to try again in.
///
/// Driven against `fixtures/claude-cli/16-error-result.ndjson`, whose `result`
/// carries an `errors` array — which is where the CLI actually puts the reason,
/// and without which the developer is told only that something failed.
#[tokio::test]
async fn an_agent_error_is_reported_in_the_conversation_and_the_turn_can_be_retried() {
    let agent = ScriptedAgent::replaying_then(
        "16-error-result",
        &a_good_turn(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"worked this time"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"worked this time"}]}}"#,
        ),
    );
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "read the file"),
        )
        .await
        .expect_success();
    let failed = client.events_through_the_turn(&subscription).await;

    // The row a developer reads. Styled as a failure, and carrying what the
    // agent said rather than only that something went wrong.
    let completed = &activity(&failed, "turn.completed")["payload"]["activity"];
    assert_eq!(completed["tone"], "error");
    let summary = completed["summary"].as_str().expect("a summary");
    assert!(summary.starts_with("Turn failed"), "{summary}");
    assert!(summary.contains("Internal server error"), "{summary}");
    assert_eq!(completed["payload"]["isError"], json!(true));

    // The session says the same thing, in the field the client renders as the
    // error banner.
    let ended = last_session(&failed, "the failed turn");
    assert_eq!(ended["payload"]["session"]["status"], "error");
    let last_error = ended["payload"]["session"]["lastError"]
        .as_str()
        .expect("a session that failed says why");
    assert!(last_error.contains("Internal server error"), "{last_error}");

    // And the whole point: the conversation is still usable. The retry is sent
    // the way the composer sends one, and is answered by the *same process* —
    // an error that had killed the session would show up here as a second start.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "try again"),
        )
        .await
        .expect_success();
    let retried = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        last_session(&retried, "the retry")["payload"]["session"]["status"],
        "ready"
    );
    assert_eq!(
        assistant_sends(&retried).last(),
        Some(&("worked this time".to_string(), false))
    );
    assert_eq!(
        agent.starts(),
        1,
        "the error ended the session instead of the turn"
    );

    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "completed");

    client.close().await;
    server.stop().await;
}

/// The agent stops being a process in the middle of a turn. The window survives,
/// the conversation says what happened, and what had already streamed is still
/// on screen.
///
/// No recording contains this and none can — see `harness::agent::DIES`, which
/// is why. The script dies where a recording would simply stop.
#[tokio::test]
async fn an_agent_that_dies_mid_turn_is_reported_without_taking_the_server_with_it() {
    let agent = ScriptedAgent::emitting(&[
        INIT,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half a sen"}}}"#,
        DIES,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "keep talking"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    // Reported in the conversation, not only on the session — the developer is
    // reading the conversation — and quoting the agent's own last words, which
    // are the only account of why it went.
    let died = &activity(&events, "session.failed")["payload"]["activity"];
    assert_eq!(died["tone"], "error");
    let summary = died["summary"].as_str().expect("a summary");
    assert!(
        summary.contains("stopped before the turn finished"),
        "{summary}"
    );
    assert!(summary.contains(LAST_WORDS), "{summary}");

    // `error` rather than `stopped`, which is ticket 15's own decision and is
    // recorded in ADR-0004: `error` is the only status the contract lets carry
    // `lastError`, and that sentence is the whole of what the developer is told.
    let ended = last_session(&events, "the dead agent");
    assert_eq!(ended["payload"]["session"]["status"], "error");
    assert!(ended["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|why| why.contains(LAST_WORDS)));

    // The child is gone and nothing is left holding a slot for it.
    server.await_live_agents(0).await;

    // The socket is still open and the server still answers on it. "Does not
    // crash the window" is not observable from here, but "does not crash the
    // server" is, and this is it.
    client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(server.live_connections(), 1);

    // What had already streamed is still in the transcript. A partial reply is
    // the developer's evidence of how far the agent got, and losing it would
    // make a crash look like a turn that never started.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["text"], "keep talking");
    assert_eq!(messages[1]["text"], "half a sen");
    // And it is *finished* rather than left mid-flight: a message still marked
    // `streaming` is a reply the UI renders as growing for the life of the
    // thread, on a turn nothing is ever going to add to.
    assert_eq!(messages[1]["streaming"], json!(false));
    // Nor was a reconciliation recorded for a buffered message that never came.
    assert_eq!(server.reconciliation().reconciled, 0);
    // And the turn is settled rather than left running for the life of the
    // thread — `error`, because nobody asked for this.
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "error");

    client.close().await;
    server.stop().await;
}

/// A session whose process died is restarted by the next turn, and the
/// conversation it belongs to is still there.
///
/// Two things have to be true and they are separate claims: a *replacement*
/// process is started, and it is asked to continue the same `claude`
/// conversation rather than begin a new one. The transcript is this server's own
/// and survives regardless; continuity is the agent's, and `--resume` is the
/// whole of it.
#[tokio::test]
async fn a_session_whose_agent_died_is_restarted_without_losing_the_transcript() {
    let agent = ScriptedAgent::resuming_after_a_death(&[
        vec![
            INIT,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"halfway through"}}}"#,
            DIES,
        ],
        a_good_turn(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"carrying on where we left off"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"carrying on where we left off"}]}}"#,
        ),
    ]);
    let workspace = Workspace::with(&["src/"]);
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
    client.events_through_the_turn(&subscription).await;
    server.await_live_agents(0).await;

    // The next turn starts a replacement, which is the restart.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "and now finish it"),
        )
        .await
        .expect_success();
    let restarted = client.events_through_the_turn(&subscription).await;

    assert_eq!(agent.starts(), 2, "the dead session was never replaced");
    assert_eq!(
        agent.resumed(),
        vec!["session-15".to_string()],
        "the replacement began a new conversation instead of continuing this one"
    );
    assert_eq!(
        last_session(&restarted, "the restarted session")["payload"]["session"]["status"],
        "ready"
    );

    // And nothing from before the death was lost: the developer's two prompts,
    // the partial reply the dead process managed, and the new one.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let said: Vec<&str> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        said,
        vec![
            "start something",
            "halfway through",
            "and now finish it",
            "carrying on where we left off",
        ]
    );

    client.close().await;
    server.stop().await;
}

/// An event type this build has never seen, and a line that is not JSON at all.
/// Neither ends the session, both are counted, and the count reaches the
/// sentence a developer actually reads.
///
/// Three turns, because the claim has three parts. The first turn drifts and
/// says so. The second reports a line that arrived **between** the two — the CLI
/// talks when no turn is running, and drift there belongs to somebody or it is
/// reported by nobody. The third is clean and says nothing, because a clause on
/// every turn is noise that trains the developer to skip the turn where it
/// mattered.
#[tokio::test]
async fn drift_is_counted_the_session_carries_on_and_the_count_is_visible() {
    let agent = ScriptedAgent::per_turn(&[
        vec![
            INIT,
            // From a CLI newer than this build.
            r#"{"type":"telemetry_event","subtype":"heartbeat"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"citation_delta","index":0}}"#,
            // Truncated, the way a line that lost its tail arrives.
            r#"{"type":"assistant","message":{"role":"assis"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"still here"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"still here"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":410,"total_cost_usd":0.003}"#,
            // After the turn ended and before the next one starts, which is a
            // real place for the CLI to talk — a rate-limit notice arrives here.
            r#"{"type":"holograph_event","from":"a later CLI"}"#,
        ],
        a_good_turn(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"and nothing odd this time"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"and nothing odd this time"}]}}"#,
        ),
        a_good_turn(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"nor this time"}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"nor this time"}]}}"#,
        ),
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say something"),
        )
        .await
        .expect_success();
    let drifted = client.events_through_the_turn(&subscription).await;

    // The session carried on: the reply after the unreadable lines still landed.
    assert_eq!(
        assistant_sends(&drifted).last(),
        Some(&("still here".to_string(), false))
    );
    assert_eq!(
        last_session(&drifted, "the drifting turn")["payload"]["session"]["status"],
        "ready"
    );

    // The count, in the sentence the work log renders. Two unrecognised events —
    // the telemetry line and the stream event inside it — and one line that did
    // not parse.
    let completed = &activity(&drifted, "turn.completed")["payload"]["activity"];
    let summary = completed["summary"].as_str().expect("a summary");
    assert!(
        summary.contains("2 unrecognised events and 1 unreadable line"),
        "the drift counters are not where a developer would see them: {summary}"
    );
    // And the session's running totals beside it, which is the number to look at
    // when asking how far this build has fallen behind the CLI.
    assert_eq!(completed["payload"]["unknownEvents"], json!(2));
    assert_eq!(completed["payload"]["parseErrors"], json!(1));

    // The next turn reports the line that arrived after the last one ended.
    // Anchoring the report to the start of a turn would have dropped it, and the
    // CLI's between-turn traffic — rate limits, compaction boundaries — is
    // exactly where a format change would first show.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "again"),
        )
        .await
        .expect_success();
    let between = client.events_through_the_turn(&subscription).await;
    let completed = &activity(&between, "turn.completed")["payload"]["activity"];
    let summary = completed["summary"].as_str().expect("a summary");
    assert!(
        summary.contains("1 unrecognised event"),
        "a line the CLI sent between turns was reported by nobody: {summary}"
    );
    assert_eq!(completed["payload"]["unknownEvents"], json!(3));

    // And a clean turn says nothing about drift.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-3", "once more"),
        )
        .await
        .expect_success();
    let clean = client.events_through_the_turn(&subscription).await;
    let completed = &activity(&clean, "turn.completed")["payload"]["activity"];
    let summary = completed["summary"].as_str().expect("a summary");
    assert!(
        !summary.contains("unrecognised"),
        "a clean turn repeated drift already reported: {summary}"
    );
    // The session's totals are still the session's, and have not gone backwards.
    assert_eq!(completed["payload"]["unknownEvents"], json!(3));

    client.close().await;
    server.stop().await;
}

/// The agent summarises its own conversation to make room. The developer is told,
/// and nothing they were reading goes away.
///
/// Both halves are asserted, because a server that mirrored the agent's
/// housekeeping would pass the first and fail the second.
#[tokio::test]
async fn compaction_is_reported_and_leaves_the_visible_transcript_intact() {
    let agent = ScriptedAgent::per_turn(&[
        vec![
            INIT,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"said long before the context filled up"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":900,"total_cost_usd":0.002}"#,
        ],
        vec![
            r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":154000,"post_tokens":21000}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"said afterwards"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":2,"duration_ms":1100,"total_cost_usd":0.004}"#,
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
    let first = client.events_through_the_turn(&subscription).await;
    assert!(
        find_activity(&first, "session.compacted").is_none(),
        "nothing was compacted yet"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "the second thing"),
        )
        .await
        .expect_success();
    let second = client.events_through_the_turn(&subscription).await;

    let compacted = &activity(&second, "session.compacted")["payload"]["activity"];
    let summary = compacted["summary"].as_str().expect("a summary");
    assert!(summary.contains("automatically"), "{summary}");
    assert!(summary.contains("154,000 tokens → 21,000"), "{summary}");
    assert_eq!(compacted["payload"]["preTokens"], json!(154_000));
    assert_eq!(compacted["payload"]["postTokens"], json!(21_000));
    assert!(
        compacted["payload"]["detail"].is_string(),
        "a row with no detail renders as a heading with nothing under it"
    );

    // And the transcript is untouched — every message, in order, including the
    // one said before the agent forgot it.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let said: Vec<&str> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        said,
        vec![
            "the first thing",
            "said long before the context filled up",
            "the second thing",
            "said afterwards",
        ]
    );
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "completed");

    client.close().await;
    server.stop().await;
}

/// The account running out of room is the difference between a slow turn and one
/// that is not going to happen, so it is reported rather than swallowed.
///
/// Three notices are scripted and two are reported: the CLI emits one whenever
/// its view of the account moves, which includes moving back to fine, and a row
/// saying nothing is wrong is noise on a schedule nobody chose.
#[tokio::test]
async fn a_rate_limit_is_surfaced_rather_than_silently_swallowed() {
    let agent = ScriptedAgent::per_turn(&[
        vec![
            INIT,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","resetsAt":1764547200}}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"five_hour","resetsAt":1764547200,"utilization":0.93}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"answered, but only just"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":800,"total_cost_usd":0.004}"#,
        ],
        vec![
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1764547200}}"#,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":40,"total_cost_usd":0,"result":"Claude AI usage limit reached"}"#,
        ],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "one more"),
        )
        .await
        .expect_success();
    let warned = client.events_through_the_turn(&subscription).await;

    let notices = activities_of(&warned, &["session.rate-limited"]);
    assert_eq!(
        notices.len(),
        1,
        "the notice saying nothing is wrong was reported too: {notices:#?}"
    );
    let summary = notices[0]["summary"].as_str().expect("a summary");
    assert!(summary.contains("close to its usage limit"), "{summary}");
    assert!(summary.contains("five_hour"), "{summary}");
    // The reset time is the whole reason the row is worth having, and it is
    // rendered as the timestamp the rest of this wire speaks.
    assert!(summary.contains("2025-12-01T00:00:00.000Z"), "{summary}");
    assert_eq!(
        notices[0]["tone"], "info",
        "being close to a limit is not yet a failure"
    );

    // And a refusal, which is one.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "and another"),
        )
        .await
        .expect_success();
    let refused = client.events_through_the_turn(&subscription).await;

    let notice = &activity(&refused, "session.rate-limited")["payload"]["activity"];
    assert_eq!(notice["tone"], "error");
    assert!(notice["summary"]
        .as_str()
        .is_some_and(|said| said.contains("usage limit has been reached")));

    // The turn failed, and says why in the agent's own words rather than only
    // that it failed.
    let ended = last_session(&refused, "the refused turn");
    assert_eq!(ended["payload"]["session"]["status"], "error");
    assert!(ended["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|why| why.contains("Claude AI usage limit reached")));

    client.close().await;
    server.stop().await;
}

/// Every failure above leaves the thread readable by a client that arrives
/// afterwards, which is the claim that matters after a restart: what the
/// developer comes back to is the stored conversation, not the events they
/// happened to be connected for.
#[tokio::test]
async fn a_conversation_that_went_wrong_is_still_readable_from_a_cold_start() {
    let database = tempfile::tempdir().expect("a temporary directory");
    let path = database.path().join("threads.sqlite3");
    let agent = ScriptedAgent::emitting(&[
        INIT,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"as far as I got"}}}"#,
        DIES,
    ]);
    let workspace = Workspace::with(&["src/"]);

    {
        let server = TestServer::start_at_with_agent(&path, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "go on then"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        server.stop().await;
    }

    let server = TestServer::start_at_with_agent(&path, &agent.configured()).await;
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;

    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["text"], "go on then");
    assert_eq!(messages[1]["text"], "as far as I got");
    // A session is a running process and there is none after a restart, so the
    // failure is read off the work log rather than off a session that is gone.
    assert_eq!(snapshot["thread"]["session"], Value::Null);
    let rows: Vec<&Value> = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .filter(|activity| activity["kind"] == "session.failed")
        .collect();
    assert_eq!(rows.len(), 1, "{:#?}", snapshot["thread"]["activities"]);
    assert!(rows[0]["summary"]
        .as_str()
        .is_some_and(|said| said.contains("stopped before the turn finished")));

    server.stop().await;
}
