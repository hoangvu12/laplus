//! Multi-turn continuity and transcript persistence, through the socket.
//!
//! Ticket 11, at the seam the spec calls primary. Everything here is driven the
//! way the real UI drives it — a dispatched command in, the events a client would
//! fold out — and the two halves of the ticket are deliberately tested against
//! *different* evidence, because they are different mechanisms:
//!
//! - **Continuity is the agent's.** What makes a follow-up a follow-up is that
//!   one `claude` session took both turns. So the evidence is the agent's own
//!   behaviour: `harness::agent::ScriptedAgent::per_turn` keeps its turn counter
//!   in the process, and a server that re-spawned per turn would reset it and get
//!   the first script back. Across a restart the evidence is the argv — a
//!   recorded `--resume <session-id>`, which is the whole of the mechanism.
//! - **The transcript is this server's.** What makes a conversation readable
//!   tomorrow is rows in SQLite. So the evidence is a second server started on the
//!   same file, which is a restart with no second process required.
//!
//! ## Two things are asserted by their absence
//!
//! A **delta is never written down**. The buffered message supersedes it and is
//! the authoritative one, so the disk is touched at message boundaries and not
//! per token — which is what "transcript writes do not block or stutter the live
//! stream" means concretely, and it is observable as a restored transcript
//! holding one reply rather than one row per token.
//!
//! A **restored conversation has no session**. A session is a running process and
//! after a restart there is none, so the thread comes back with `session: null`
//! and a turn that did not finish comes back as one that did not finish — never as
//! one still working, which nothing would be left to settle.

mod harness;

use harness::agent::{ScriptedAgent, REFUSAL};
use harness::conversation::{activity, assistant_sends, follow_up, last_session, start_turn};
use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};

/// The session the scripted agent announces on its `init` line. What the server
/// has to remember, write down, and hand back as `--resume`.
const SESSION: &str = "1f0d7a52-3c11-4a8e-9f60-6b2c7d4e5a90";

/// One turn of a healthy streamed conversation, replying `reply`.
///
/// `announces` is what the real CLI does exactly once per process: the `init`
/// line comes with the session, so a script for a *second* turn on the same child
/// does not carry one.
fn a_turn(reply: &str, announces: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if announces {
        lines.push(format!(
            r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Read"]}}"#
        ));
    }
    lines.push(r#"{"type":"stream_event","event":{"type":"message_start"}}"#.to_string());
    lines.push(format!(
        r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{}}}}}}}"#,
        json!(reply)
    ));
    lines.push(r#"{"type":"stream_event","event":{"type":"message_stop"}}"#.to_string());
    lines.push(format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":{}}}]}}}}"#,
        json!(reply)
    ));
    lines.push(
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":120,"total_cost_usd":0.0042}"#
            .to_string(),
    );
    lines
}

/// A whole conversation's worth of scripts: one per reply, announcing the session
/// on the first the way a long-lived child does.
fn a_conversation(replies: &[&str]) -> Vec<Vec<String>> {
    replies
        .iter()
        .enumerate()
        .map(|(index, reply)| a_turn(reply, index == 0))
        .collect()
}

/// The scripts as the agent takes them. Named rather than inlined because the
/// borrowed lines have to outlive the call that reads them.
fn lines(scripted: &[Vec<String>]) -> Vec<Vec<&str>> {
    scripted
        .iter()
        .map(|turn| turn.iter().map(String::as_str).collect())
        .collect()
}

/// The transcript as (role, text), which is what a developer reads.
fn transcript(snapshot: &Value) -> Vec<(String, String)> {
    snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            (
                message["role"].as_str().unwrap_or("").to_string(),
                message["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Continuity inside one run
// ---------------------------------------------------------------------------

/// A follow-up reaches the session that took the turn before it, and several
/// turns can be exchanged without that degrading.
///
/// The agent's turn counter is a variable inside its process, so which script a
/// turn is answered with is also the answer to "was this the same process". Three
/// distinct replies in order means one session took all three — which is what "a
/// follow-up retains prior context" rests on, because the context is the agent's
/// and the agent is the same one.
#[tokio::test]
async fn several_turns_in_a_row_reach_the_session_that_took_the_one_before() {
    let scripted = a_conversation(&["one", "two, after one", "three, after two"]);
    let agent = ScriptedAgent::per_turn(&lines(&scripted));
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    let mut replies = Vec::new();
    for (index, prompt) in ["first", "second", "third"].into_iter().enumerate() {
        let command = match index {
            0 => start_turn("thread-1", "message-1", prompt),
            _ => follow_up("thread-1", &format!("message-{}", index + 1), prompt),
        };
        client
            .call("orchestration.dispatchCommand", command)
            .await
            .expect_success();

        let events = client.events_through_the_turn(&subscription).await;
        assert_eq!(
            last_session(&events, prompt)["payload"]["session"]["status"],
            "ready",
            "turn {} did not settle cleanly",
            index + 1
        );
        let (reply, streaming) = assistant_sends(&events)
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("turn {} produced no reply", index + 1));
        assert!(!streaming, "a turn has to end with the buffered message");
        replies.push(reply);
    }

    assert_eq!(
        replies,
        vec!["one", "two, after one", "three, after two"],
        "a later turn was answered by a session that had not taken the earlier ones"
    );
    assert_eq!(
        agent.starts(),
        1,
        "the conversation was served by more than one process: {:?}",
        agent.arguments()
    );
    assert!(
        agent.resumed().is_empty(),
        "a live session was resumed instead of continued: {:?}",
        agent.resumed()
    );

    // And the whole exchange is one transcript, in order.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    assert_eq!(
        transcript(&snapshot),
        vec![
            ("user".to_string(), "first".to_string()),
            ("assistant".to_string(), "one".to_string()),
            ("user".to_string(), "second".to_string()),
            ("assistant".to_string(), "two, after one".to_string()),
            ("user".to_string(), "third".to_string()),
            ("assistant".to_string(), "three, after two".to_string()),
        ]
    );

    client.close().await;
    server.stop().await;
}

// ---------------------------------------------------------------------------
// Continuity across a restart
// ---------------------------------------------------------------------------

/// The conversation and its full transcript survive an app restart.
///
/// A second server on the same database file is a restart with no second process
/// required. What it has to produce is the conversation a developer left: in the
/// project list so they can find it, and with its messages, its work log and how
/// its last turn went so that opening it shows the same thing it showed
/// yesterday.
#[tokio::test]
async fn a_conversation_and_its_transcript_survive_a_restart() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/main.rs"]);
    let scripted = a_conversation(&["the answer"]);
    let agent = ScriptedAgent::per_turn(&lines(&scripted));

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "the question"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        // Ends the agents and *then* waits for the transcript queue, which is the
        // order that keeps the last message of a conversation.
        server.stop().await;
    }

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;

    // In the project list, which is where a developer goes looking for it.
    let shell = restarted.connect().await.into_shell_snapshot().await;
    let threads = shell["threads"].as_array().expect("threads");
    assert_eq!(threads.len(), 1, "{shell:#?}");
    assert_eq!(threads[0]["id"], "thread-1");
    assert_eq!(threads[0]["title"], "A conversation");
    assert_eq!(threads[0]["projectId"], "project-1");
    assert!(
        threads[0]["latestUserMessageAt"].is_string(),
        "the thread list sorts on this: {}",
        threads[0]
    );

    // And with the conversation in it.
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(
        transcript(&snapshot),
        vec![
            ("user".to_string(), "the question".to_string()),
            ("assistant".to_string(), "the answer".to_string()),
        ]
    );

    // The work log too: what the turn cost is what a developer scrolls back for.
    let kinds: Vec<&str> = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .filter_map(|activity| activity["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"turn.completed"), "{kinds:?}");

    // How the last turn went is kept; the session that ran it is not, because
    // there is no process behind it any more.
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "completed");
    assert!(snapshot["thread"]["latestTurn"]["completedAt"].is_string());
    assert_eq!(
        snapshot["thread"]["session"],
        Value::Null,
        "a restored conversation claimed to have a running agent behind it"
    );

    restarted.stop().await;
}

/// A restored conversation can be continued, not just read.
///
/// The continuation is one flag: `--resume <session-id>`, pointed at the session
/// the agent announced on the first run. That is the whole mechanism — the context
/// is in the agent's own store rather than in this server's transcript — so the
/// argv the second process was given is the honest place to observe it.
#[tokio::test]
async fn a_restored_conversation_is_continued_by_resuming_the_agents_own_session() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let scripted = a_conversation(&["before"]);
    let agent = ScriptedAgent::per_turn(&lines(&scripted));

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "before the restart"),
            )
            .await
            .expect_success();
        let _ = client.events_through_the_turn(&subscription).await;

        // The id the agent announced is not on the wire at all — nothing the
        // client renders needs it — so what this run can assert is only that it
        // did not itself resume anything. That the id was *kept* is what the
        // second run below proves, by being given it.
        assert!(
            agent.resumed().is_empty(),
            "the first session was started with a resume"
        );

        client.close().await;
        server.stop().await;
    }

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut client = restarted.connect().await;
    // Not a draft this time: the conversation is on disk, so its subscription
    // opens with it.
    let subscription = client.watch_conversation("thread-1").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "after the restart"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let continued = &last_session(&events, "the follow-up")["payload"]["session"];
    assert_eq!(
        continued["status"],
        "ready",
        "the continued turn did not settle: {events:#?}"
    );
    assert_eq!(continued["providerName"], "claudeAgent");
    assert_eq!(continued["providerInstanceId"], "claudeAgent");
    assert_eq!(
        agent.resumed(),
        vec![SESSION.to_string()],
        "the second run started a fresh conversation instead of resuming: {:?}",
        agent.arguments()
    );
    assert_eq!(
        agent.starts(),
        2,
        "one process per run, and no more: {:?}",
        agent.arguments()
    );

    // And the conversation is one conversation, not two.
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(
        transcript(&snapshot),
        vec![
            ("user".to_string(), "before the restart".to_string()),
            ("assistant".to_string(), "before".to_string()),
            ("user".to_string(), "after the restart".to_string()),
            ("assistant".to_string(), "before".to_string()),
        ],
        "the restored transcript and the new turn are not one transcript"
    );

    client.close().await;
    restarted.stop().await;
}

/// A session the agent no longer holds fails with an explanation, and the
/// transcript stays readable.
///
/// This is the failure with no NDJSON to it: the CLI writes its reason to stderr
/// and exits without a line, so the only account of it is the agent's own words.
/// What the developer is owed is a sentence saying the conversation can be read
/// and not continued — and the conversation itself, untouched, because the
/// transcript is this server's copy rather than the agent's.
#[tokio::test]
async fn a_resume_the_agent_will_not_honour_is_explained_and_leaves_the_transcript_readable() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let scripted = a_conversation(&["said once"]);
    let agent = ScriptedAgent::refusing_to_resume(&lines(&scripted));

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "while it still remembered"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        server.stop().await;
    }

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut client = restarted.connect().await;
    let subscription = client.watch_conversation("thread-1").await;

    // The turn is still accepted — the failure is not the client's — and the
    // refusal arrives in the conversation.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "and now?"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let failed = &activity(&events, "session.resume-failed")["payload"]["activity"];
    let explanation = failed["summary"].as_str().expect("a summary");
    assert_eq!(failed["tone"], "error");
    assert!(
        explanation.contains(SESSION),
        "the explanation has to name the session it could not resume: {explanation}"
    );
    assert!(
        explanation.contains("read") && explanation.contains("not continued"),
        "the explanation has to say what the developer can still do: {explanation}"
    );
    assert!(
        explanation.contains(REFUSAL),
        "the agent's own words are the useful part: {explanation}"
    );

    let ended = last_session(&events, "the refused resume");
    assert_eq!(ended["payload"]["session"]["status"], "error");
    assert!(ended["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|error| error.contains(SESSION)));

    // The conversation is readable, and the new prompt is in it to retry from.
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(
        transcript(&snapshot),
        vec![
            (
                "user".to_string(),
                "while it still remembered".to_string()
            ),
            ("assistant".to_string(), "said once".to_string()),
            ("user".to_string(), "and now?".to_string()),
        ]
    );

    client.close().await;
    restarted.stop().await;
}

// ---------------------------------------------------------------------------
// What persistence costs, and what it does not
// ---------------------------------------------------------------------------

/// Only whole messages are written down, which is what keeps the disk out of the
/// streaming path.
///
/// A turn's deltas are superseded by the buffered message a moment later, so
/// writing them would be an `fsync` per token of a reply that is not the
/// authoritative one anyway. Driven with deltas that *disagree* with the buffered
/// message, because that is the only way to tell "the reply was written once" from
/// "the reply was written per token and happened to end up the same".
#[tokio::test]
async fn a_stored_transcript_holds_whole_messages_rather_than_a_row_per_token() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"the "}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"beginning "}}}"#,
        r#"{"type":"stream_event","event":{"type":"message_stop"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the beginning and the end"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":90,"total_cost_usd":0.001}"#,
    ]);

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "finish the sentence"),
            )
            .await
            .expect_success();

        // Three streaming sends and one buffered one reached the client.
        let events = client.events_through_the_turn(&subscription).await;
        let sends = assistant_sends(&events);
        assert!(
            sends.iter().filter(|(_, streaming)| *streaming).count() >= 2,
            "the reply did not stream, so this proves nothing: {sends:?}"
        );

        client.close().await;
        server.stop().await;
    }

    let restarted = TestServer::start_at(&database).await;
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;

    assert_eq!(
        transcript(&snapshot),
        vec![
            ("user".to_string(), "finish the sentence".to_string()),
            (
                "assistant".to_string(),
                "the beginning and the end".to_string()
            ),
        ],
        "the streamed pieces were written down as well as the message that replaced them"
    );
    // And it comes back settled rather than mid-stream: only whole messages are
    // stored, so a stored message was never streaming.
    assert_eq!(
        snapshot["thread"]["messages"][1]["streaming"],
        json!(false)
    );

    restarted.stop().await;
}

/// A very long transcript comes back whole, in order, and without the
/// conversation's length costing the connection anything.
///
/// The snapshot is built on the subscription's own task (`subscriptions::pump`),
/// not on the socket's read loop, which is what keeps a long conversation from
/// stalling everything else a client is doing. The unrelated call is therefore
/// *sent before the snapshot is read* and collected afterwards: it is outstanding
/// for the whole time the pump spends building and sending several hundred
/// messages, so an answer to it is an answer from a read loop that was not waiting
/// on any of that. Asserting on a stopwatch instead would be asserting on how fast
/// this machine is.
#[tokio::test]
async fn a_long_transcript_comes_back_whole_and_leaves_the_connection_usable() {
    const SAID: usize = 400;

    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    // One turn, several hundred buffered messages. A provider legitimately sends
    // more than one message per turn — commentary between tool calls — so this is
    // a long conversation arriving quickly rather than a shape the CLI cannot
    // produce.
    let mut said = vec![
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#
            .to_string(),
    ];
    for index in 0..SAID {
        said.push(format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{index}"}}]}}}}"#
        ));
    }
    said.push(
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":900,"total_cost_usd":0.02}"#
            .to_string(),
    );
    let turn: Vec<&str> = said.iter().map(String::as_str).collect();
    let agent = ScriptedAgent::emitting(&turn);

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "say a lot"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        server.stop().await;
    }

    let restarted = TestServer::start_at(&database).await;
    let mut client = restarted.connect().await;

    // Outstanding across the whole of the snapshot's arrival, on the same socket.
    let unrelated = client.send_request("server.getConfig", json!({})).await;
    let subscription = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({"threadId": "thread-1"}),
        )
        .await;
    let opening = client.next_chunk(&subscription).await;
    let snapshot = opening
        .into_iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the conversation is on disk")["snapshot"]
        .clone();

    // And the read loop answered it rather than queueing behind the pump.
    client.await_outcome(&unrelated).await.expect_success();

    let messages = transcript(&snapshot);
    assert_eq!(messages.len(), SAID + 1, "the prompt and everything after it");
    assert_eq!(messages[0].1, "say a lot");
    assert!(
        messages[1..]
            .iter()
            .enumerate()
            .all(|(index, (role, text))| role == "assistant" && text == &index.to_string()),
        "the transcript came back in a different order than it was said"
    );

    client.interrupt(&subscription).await;
    client.close().await;
    restarted.stop().await;
}

/// Deleting a project takes its conversations with it, on the wire, on disk, and
/// in the process table.
///
/// The client's shell reducer answers `project-removed` by filtering the projects
/// and nothing else, so without a `thread-removed` beside it a conversation whose
/// project has gone would sit in the project list until the next restart and
/// vanish after it. The rows go in the same transaction, by the schema's own
/// cascade.
///
/// The delete happens while the conversation's agent is **still alive**, which is
/// the ordinary case: a child stays up between turns. So this also drives the
/// release of that agent, and the server's stop afterwards is what says the
/// driver was waited for rather than detached — a `claude` outliving the server is
/// the one leak this process can produce that survives the process.
///
/// It is also the case where the *registry* has to be the source of truth for
/// which conversations exist: the thread reached the database eventually, and a
/// delete a moment earlier would have found nothing stored to remove.
#[tokio::test]
async fn deleting_a_project_takes_its_conversations_with_it() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let scripted = a_conversation(&["said"]);
    let agent = ScriptedAgent::per_turn(&lines(&scripted));

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "hello"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;

        let shell = client
            .subscribe("orchestration.subscribeShell", json!({}))
            .await;
        client.next_chunk(&shell).await;
        client.ack(&shell).await;

        server.await_live_agents(1).await;
        client
            .call(
                "orchestration.dispatchCommand",
                json!({
                    "type": "project.delete",
                    "commandId": "test:delete:project-1",
                    "projectId": "project-1",
                }),
            )
            .await
            .expect_success();
        assert_eq!(
            server.live_agents(),
            0,
            "the deleted conversation's agent was left running"
        );

        // Both halves of the removal reach the project list.
        let removals = client
            .values_until(&shell, |item| item["kind"] == "thread-removed")
            .await;
        assert!(
            removals
                .iter()
                .any(|item| item["kind"] == "project-removed"
                    && item["projectId"] == "project-1"),
            "{removals:#?}"
        );
        assert!(
            removals
                .iter()
                .any(|item| item["kind"] == "thread-removed" && item["threadId"] == "thread-1"),
            "{removals:#?}"
        );

        client.close().await;
        server.stop().await;
    }

    // And the next run does not bring the conversation back.
    let restarted = TestServer::start_at(&database).await;
    let shell = restarted.connect().await.into_shell_snapshot().await;
    assert_eq!(shell["projects"], json!([]));
    assert_eq!(shell["threads"], json!([]));

    restarted.stop().await;
}

/// A turn the app closed in the middle of comes back as one that did not finish.
///
/// Two things are being pinned. The turn does not come back `running`: nothing is
/// alive to settle it, so a conversation that showed one working forever would be
/// a conversation the developer could only close. And the reply the agent had
/// begun comes back with it, finished rather than mid-flight.
///
/// That second half changed in ticket 15. Only whole messages are written down —
/// a delta owes the database nothing, which is what keeps the disk out of the
/// streaming path — so before it, a reply cut short had nothing on disk at all and
/// the conversation came back showing a prompt nobody had answered. The driver now
/// settles the message on its way down, which is the moment it knows no buffered
/// message is coming: the developer sees how far the agent got, and the message
/// stops claiming to still be arriving. What is still lost is the *hard*-kill
/// case, where the driver never runs at all.
///
/// The state is `error` rather than `interrupted` because that is what a graceful
/// close produces: the agent is told there will be no more turns and stops with
/// one still in flight, which the driver reports as the agent having stopped
/// early. `interrupted` is the *hard*-kill case, where nothing got to report
/// anything and the stored turn is still `running` — driven as a unit test on
/// `Thread::restored`, since a test cannot kill its own process.
#[tokio::test]
async fn a_turn_the_app_closed_during_does_not_come_back_running() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    // Deltas and then silence: the agent never sends its buffered message and
    // never reports a result, so the turn is still in flight when the server
    // stops.
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"halfway thr"}}}"#,
    ]);

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let subscription = client.open_conversation(&workspace, "thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-1", "start something"),
            )
            .await
            .expect_success();

        // Read as far as the first piece of the reply, so the turn is known to be
        // under way before the server is stopped.
        client
            .values_until(&subscription, |item| {
                item["event"]["type"] == "thread.message-sent"
                    && item["event"]["payload"]["role"] == "assistant"
            })
            .await;

        client.close().await;
        server.stop().await;
    }

    let restarted = TestServer::start_at(&database).await;
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;

    assert_eq!(
        transcript(&snapshot),
        vec![
            ("user".to_string(), "start something".to_string()),
            ("assistant".to_string(), "halfway thr".to_string()),
        ],
        "the reply the agent had begun was lost rather than settled"
    );
    assert_eq!(
        snapshot["thread"]["messages"][1]["streaming"],
        json!(false),
        "a reply nothing is left to finish came back claiming to still be arriving"
    );
    let turn = &snapshot["thread"]["latestTurn"];
    assert_ne!(
        turn["state"], "running",
        "a conversation came back with a turn nothing is left to finish: {turn}"
    );
    assert_eq!(turn["state"], "error", "{turn}");
    assert!(turn["completedAt"].is_string(), "{turn}");

    restarted.stop().await;
}
