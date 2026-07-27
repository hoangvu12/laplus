//! Stopping the agent mid-turn, driven the way the UI drives it.
//!
//! Ticket 14 at the seam the spec calls primary. The feature is small on the
//! wire — one command, one line on the agent's stdin — and almost all of it is
//! about what the conversation looks like *afterwards*: partial output kept and
//! marked, a correction accepted immediately, and the same child taking it.
//!
//! ## The scripts are recordings, and they had to be
//!
//! `fixtures/claude-cli/11`–`15` are real `claude` output, captured for this
//! ticket by interrupting the binary five different ways. It matters here for
//! the same reason it mattered for permissions: the thing under test is a round
//! trip, and what the CLI does with an interrupt *is* the feature. Two of the
//! findings would have been guessed wrong:
//!
//! - **A stopped turn is reported as a failed one.** `"is_error": true`, subtype
//!   `error_during_execution`. A server that read the wire and believed it would
//!   show the developer an error for work they cancelled.
//! - **The partial reply arrives whole, after the acknowledgement.** The CLI
//!   flushes what it had buffered rather than dropping it, so "output produced
//!   before the interrupt is retained" needs nothing special from this server —
//!   which is worth knowing, because a server that *had* invented something
//!   would have invented a second, worse copy of the reply.
//!
//! ## What the assertions are made of
//!
//! Two things, deliberately different in kind — the same split
//! `socket_permissions.rs` makes:
//!
//! - **What the client would render**: the `turn.interrupted` row, the message
//!   left in the transcript, the session status and the latest turn's state.
//! - **What the agent was actually told**: `ScriptedAgent::answers`, the lines
//!   the server wrote to the child's stdin. A server that published a stop and
//!   sent nothing would satisfy every assertion of the first kind.

mod harness;

use harness::agent::{ScriptedAgent, AWAIT_ANSWER, PAUSE};
use harness::conversation::{
    activities_of, activity, find_activity, follow_up, interrupt_turn, last_session,
    respond_to_approval, start_turn_in, start_turn,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// The mode whose whole meaning is that the agent asks first — ticket 13's, and
/// needed here for the one interrupt this server does not send itself.
const ASKS_FIRST: &str = "approval-required";

/// What the agent says on the turn after the one that was stopped.
///
/// A script of its own rather than the recording again, for the reason ticket 13
/// gives: replaying `11` twice would have the agent produce a second essay to
/// stop, which is another question rather than an answer to "did the
/// conversation survive the first one". The reply exists only here, so hearing it
/// is proof that the *same* child took both turns — the turn counter lives inside
/// the process.
const THE_CORRECTION: [&str; 2] = [
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Stopped. What would you like instead?"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":11,"total_cost_usd":0.002}"#,
];

/// A conversation with a turn under way and something already on screen.
///
/// Everything outlives the read, the way ticket 13's `Asked` does and for the
/// same reason: the turn is not over — that is the point — so the socket, the
/// subscription and the workspace all have to still be there when the stop is
/// pressed.
struct Running {
    server: TestServer,
    client: SocketClient,
    subscription: String,
    /// The turn the client is looking at, which is what its stop button names.
    turn_id: String,
    /// Everything published up to the agent having said something.
    events: Vec<Value>,
    _workspace: Workspace,
}

/// Register a project, open the conversation, send a turn, and read until the
/// agent has streamed something.
async fn running(agent: &ScriptedAgent) -> Running {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "write about bicycles"),
        )
        .await
        .expect_success();

    let events = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&events, "the running turn")["payload"]["session"]["activeTurnId"]
        .as_str()
        .expect("a running session names the turn it is working on")
        .to_string();

    Running {
        server,
        client,
        subscription,
        turn_id,
        events,
        _workspace: workspace,
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

/// Every line the server wrote to the agent, parsed.
fn written(agent: &ScriptedAgent) -> Vec<Value> {
    agent
        .answers()
        .iter()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("the agent was written non-json: {error}: {line}"))
        })
        .collect()
}

/// The assistant's message as the transcript holds it, which is what a client
/// arriving after the fact is handed.
fn transcript_reply(snapshot: &Value) -> Value {
    snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .rfind(|message| message["role"] == "assistant")
        .cloned()
        .unwrap_or_else(|| panic!("nothing the agent said survived: {}", snapshot["thread"]))
}

/// The first four criteria, which are one story: stop it, keep what it said, say
/// that is what happened, and let the developer carry on.
///
/// The partial reply is asserted against the *transcript* rather than against the
/// events, because that is what a client which arrives after the interrupt reads
/// — and the criterion is about what is retained, not about what streamed past.
#[tokio::test]
async fn stopping_a_turn_keeps_what_the_agent_said_and_marks_it_interrupted() {
    let agent = ScriptedAgent::replaying("11-interrupted-turn");
    let mut running = running(&agent).await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some(&running.turn_id)),
        )
        .await
        .expect_success();
    let rest = running
        .client
        .events_through_the_turn(&running.subscription)
        .await;

    // **What the agent was told.** The one request this server makes of it, in
    // the CLI's own vocabulary. A server that published the row below and wrote
    // nothing here would pass every other assertion in this test.
    let written = written(&agent);
    assert_eq!(written.len(), 1, "{written:#?}");
    assert_eq!(written[0]["type"], "control_request");
    assert_eq!(written[0]["request"]["subtype"], "interrupt");
    assert!(
        !text(&written[0]["request_id"]).is_empty(),
        "the CLI answers by naming the id, so there has to be one: {}",
        written[0]
    );

    // **What settles the turn.** `thread.turn-interrupt-requested` is the
    // contract's own event and the client's reducer moves the latest turn to
    // `interrupted` on it *immediately* (`threadReducer.ts`) — so the developer's
    // click is what stops the turn being reported as running, rather than a round
    // trip to the agent. The reducer folds it as `unchanged` without a `turnId`,
    // so an event that carried none would do nothing at all.
    let asked_to_stop = rest
        .iter()
        .map(|item| &item["event"])
        .find(|event| event["type"] == "thread.turn-interrupt-requested")
        .unwrap_or_else(|| panic!("nothing settled the turn: {:?}", harness::conversation::kinds(&rest)));
    assert_eq!(asked_to_stop["payload"]["turnId"], running.turn_id);
    assert_eq!(asked_to_stop["payload"]["threadId"], "thread-1");

    // **What the developer sees.** A row saying they stopped it, attributed to
    // the turn they stopped.
    let stopped = activity(&rest, "turn.interrupted")["payload"]["activity"].clone();
    assert_eq!(stopped["turnId"], running.turn_id);
    assert_eq!(stopped["payload"]["turnId"], running.turn_id);
    assert!(
        !text(&stopped["payload"]["detail"]).is_empty(),
        "a row with no detail renders as a heading with nothing under it"
    );

    // The turn ends as stopped rather than as failed. The recording's own
    // `result` is `"is_error": true` — this is the assertion that the server does
    // not simply believe it.
    let completed = activity(&rest, "turn.completed")["payload"]["activity"].clone();
    assert_eq!(completed["tone"], "info", "{completed}");
    assert_eq!(completed["payload"]["interrupted"], json!(true));
    assert_eq!(completed["payload"]["isError"], json!(false));
    assert!(
        text(&completed["summary"]).starts_with("Turn stopped by the developer"),
        "{}",
        completed["summary"]
    );

    let ended = last_session(&rest, "the stopped turn");
    assert_eq!(ended["payload"]["session"]["status"], "interrupted");
    assert_eq!(
        ended["payload"]["session"]["lastError"],
        Value::Null,
        "a turn the developer stopped is not an error to show them"
    );

    // **What is kept.** The partial reply is in the transcript, settled rather
    // than left streaming, and the turn beside it says how it ended.
    let snapshot = running
        .server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let reply = transcript_reply(&snapshot);
    assert!(
        text(&reply["text"]).starts_with("# The Evolution of the Bicycle"),
        "the partial reply was not kept: {reply}"
    );
    assert!(
        text(&reply["text"]).ends_with("long skirts, found"),
        "the reply is not the one that was cut off: {}",
        text(&reply["text"])
    );
    assert_eq!(
        reply["streaming"],
        json!(false),
        "a message left streaming stays streaming for the life of the thread"
    );
    assert_eq!(
        snapshot["thread"]["latestTurn"]["state"], "interrupted",
        "{}",
        snapshot["thread"]["latestTurn"]
    );
    assert!(
        !snapshot["thread"]["latestTurn"]["completedAt"].is_null(),
        "an interrupted turn with no completedAt reads as one still needing attention"
    );

    running.client.close().await;
    running.server.stop().await;
}

/// A correction can be sent immediately afterwards, and the same agent takes it.
///
/// The fourth and seventh criteria together, because they are only worth
/// anything together: a follow-up that reached a *new* process would be a
/// conversation that had forgotten what it was correcting.
#[tokio::test]
async fn a_correction_sent_straight_afterwards_reaches_the_same_agent() {
    let agent = ScriptedAgent::replaying_then("11-interrupted-turn", &THE_CORRECTION);
    let mut running = running(&agent).await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some(&running.turn_id)),
        )
        .await
        .expect_success();
    running
        .client
        .events_through_the_turn(&running.subscription)
        .await;
    assert_eq!(running.server.live_agents(), 1, "the interrupt took the agent with it");

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "never mind, do the other thing"),
        )
        .await
        .expect_success();
    let after = running
        .client
        .events_through_the_turn(&running.subscription)
        .await;

    assert_eq!(
        harness::conversation::assistant_sends(&after)
            .last()
            .map(|(text, _)| text.clone()),
        Some("Stopped. What would you like instead?".to_string()),
        "the conversation could not take a turn after the interrupt"
    );
    assert_eq!(
        last_session(&after, "the correction")["payload"]["session"]["status"],
        "ready"
    );
    assert_eq!(
        agent.starts(),
        1,
        "the correction went to a second process, so the interrupt killed the session"
    );

    // And the conversation reads as one conversation: the stopped reply, then the
    // correction, then the answer to it.
    let snapshot = running
        .server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let roles: Vec<String> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| text(&message["role"]))
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "completed");

    running.client.close().await;
    running.server.stop().await;
}

/// A script that counts what the server writes, without needing the agent to
/// stop for it.
///
/// **This is how a no-op is proved rather than assumed.** Asserting "nothing was
/// written" straight after dispatching is a race — the command answers before the
/// driver has looked at the signal — and `ScriptedAgent::answers` only records at
/// a stop, of which a no-op test has none. So the counting is done by the agent's
/// own turn loop: it reads one line per turn, so a line the server *should not
/// have written* is consumed as a turn and answered with the next script. Three
/// scripts, and hearing the third is the failure.
fn counting_lines() -> ScriptedAgent {
    ScriptedAgent::per_turn(&[
        vec![
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5","cwd":"/tmp","permissionMode":"bypassPermissions","tools":["Read"]}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}}"#,
            PAUSE,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"first"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
        ],
        vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
        ],
        vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"third"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
        ],
    ])
}

/// The last thing the agent said in this run of events.
fn last_reply(events: &[Value]) -> String {
    harness::conversation::assistant_sends(events)
        .last()
        .map(|(text, _)| text.clone())
        .unwrap_or_else(|| panic!("the agent said nothing"))
}

/// A correction sent *while the stopped turn is still winding down* is not
/// reported as finished the moment the old one is.
///
/// The race the interrupt creates and nothing else does. Stopping the agent is
/// what re-enables the composer, so the developer can dispatch the next turn
/// before the CLI has finished aborting the last one — and by then the session
/// describes the new turn. A driver that settled the old turn's session anyway
/// would announce the new one as over before the agent had been handed it.
///
/// Driven with a script that pauses *after* being written to, which is what makes
/// the window a second wide rather than a race the test would lose.
#[tokio::test]
async fn a_correction_sent_while_the_old_turn_winds_down_is_not_settled_with_it() {
    let agent = ScriptedAgent::per_turn(&[
        vec![
            r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5","cwd":"/tmp","permissionMode":"bypassPermissions","tools":["Read"]}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half a "}}}"#,
            AWAIT_ANSWER,
            // The agent has been told to stop and takes a moment to wind down,
            // which is the whole window this test is about.
            PAUSE,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"interrupt-1","response":{"still_queued":[]}}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"half a "}]}}"#,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":900,"total_cost_usd":0.0}"#,
        ],
        vec![
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
        ],
    ]);
    let mut running = running(&agent).await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some(&running.turn_id)),
        )
        .await
        .expect_success();
    // The row goes up as soon as the request has been written, so seeing it means
    // the agent is in its wind-down rather than still waiting to be told.
    running
        .client
        .values_until(&running.subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "turn.interrupted"
        })
        .await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "do the other thing"),
        )
        .await
        .expect_success();
    let after = running
        .client
        .events_through_the_turn(&running.subscription)
        .await;

    // Every session the client was told about, after the correction was
    // dispatched. The old turn's ending must not be among them: the correction is
    // `starting`, then `running`, then `ready` — never `interrupted`.
    let sessions: Vec<(String, Value)> = after
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.session-set")
        .map(|event| {
            (
                text(&event["payload"]["session"]["status"]),
                event["payload"]["session"]["activeTurnId"].clone(),
            )
        })
        .collect();
    assert!(
        !sessions.iter().any(|(status, _)| status == "interrupted"),
        "the stopped turn settled the session out from under the correction: {sessions:?}"
    );
    assert_eq!(
        sessions.last().map(|(status, _)| status.as_str()),
        Some("ready")
    );
    assert_eq!(last_reply(&after), "second");

    let snapshot = running
        .server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(
        snapshot["thread"]["latestTurn"]["state"], "completed",
        "the correction was reported as ending the way the turn before it did: {}",
        snapshot["thread"]["latestTurn"]
    );

    running.client.close().await;
    running.server.stop().await;
}

/// Stopping when nothing is in flight is a no-op rather than an error.
///
/// Two shapes of it, and the client sends both: no `turnId` at all, which is what
/// it sends when it does not believe a turn is running, and the id of one that
/// has finished. The command succeeds either way, because telling a developer
/// that stopping an agent which is not running went wrong would be this server
/// inventing a problem.
#[tokio::test]
async fn stopping_when_nothing_is_running_does_nothing_and_says_so_by_succeeding() {
    let agent = counting_lines();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;
    let settled = last_session(&turn, "the finished turn");
    assert_eq!(settled["payload"]["session"]["status"], "ready");
    let finished = settled["payload"]["session"]["activeTurnId"].clone();
    assert_eq!(finished, Value::Null, "the turn did not settle");

    for named in [None, Some("turn-that-never-was")] {
        client
            .call(
                "orchestration.dispatchCommand",
                interrupt_turn("thread-1", named),
            )
            .await
            .expect_success();
    }

    // The follow-up gets the *second* script. A stop that had been written to the
    // agent would have been read as a turn, and this would be the third.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "carry on"),
        )
        .await
        .expect_success();
    let after = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        last_reply(&after),
        "second",
        "a no-op interrupt was written to the agent and read as a turn"
    );
    assert!(
        find_activity(&after, "turn.interrupted").is_none(),
        "a no-op interrupt left a row behind: {:?}",
        activities_of(&after, &["turn.interrupted"])
    );
    assert_eq!(
        last_session(&after, "the turn after the no-ops")["payload"]["session"]["status"],
        "ready",
        "the conversation did not survive being stopped for no reason"
    );

    client.close().await;
    server.stop().await;
}

/// Naming a turn that is no longer the one running is a no-op too — including
/// while a *different* turn is running, which is the case that could do harm.
///
/// The race the client cannot avoid: `buildThreadTurnInterruptInput` names the
/// turn the developer is looking at, and by the time the command lands that can
/// be the previous one. Stopping the turn after it would be stopping work the
/// developer never saw start.
#[tokio::test]
async fn naming_a_turn_that_is_no_longer_running_stops_nothing() {
    let agent = counting_lines();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();

    // Sent while the first turn is genuinely in flight — the script pauses for a
    // second — and naming a turn that is not it.
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some("turn-from-a-minute-ago")),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;

    // The turn ran to its own end rather than being stopped.
    assert_eq!(last_reply(&turn), "first");
    assert_eq!(
        last_session(&turn, "the turn that was not stopped")["payload"]["session"]["status"],
        "ready"
    );
    assert!(
        find_activity(&turn, "turn.interrupted").is_none(),
        "a stale turn id stopped the turn in flight"
    );

    // And nothing was written to the agent, proved the same way as above.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "carry on"),
        )
        .await
        .expect_success();
    let after = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        last_reply(&after),
        "second",
        "a stale turn id was written to the agent and read as a turn"
    );

    client.close().await;
    server.stop().await;
}

/// Stopping during a run of tool calls leaves the work log honest and the child
/// alive.
///
/// The fifth criterion. The orphan the criterion is about is the tool's own
/// child process, and the CLI is what owns that — `13-interrupt-during-tool-use`
/// is the recording of it aborting one, six seconds into a run that would have
/// taken far longer. What this server owes is the other half: the `claude`
/// process itself is still there afterwards, and is reaped when the session
/// really does end.
#[tokio::test]
async fn stopping_during_tool_use_leaves_the_agent_alive_and_the_work_log_honest() {
    let agent = ScriptedAgent::replaying("13-interrupt-during-tool-use");
    let mut running = running(&agent).await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", None),
        )
        .await
        .expect_success();
    let rest = running
        .client
        .events_through_the_turn(&running.subscription)
        .await;

    // The call that finished before the stop is in the work log, paired. The one
    // the agent was opening when it was stopped is not — the CLI announces a call
    // on the buffered message that closes the block, and that block never closed.
    let calls = activities_of(&running.events, &["tool.updated", "tool.completed"]);
    let after = activities_of(&rest, &["tool.updated", "tool.completed"]);
    let kinds: Vec<String> = calls
        .into_iter()
        .chain(after)
        .map(|activity| text(&activity["kind"]))
        .collect();
    assert_eq!(
        kinds,
        vec!["tool.updated", "tool.completed"],
        "a call was announced with no result, or a result with no call"
    );

    assert_eq!(
        last_session(&rest, "the stopped tool run")["payload"]["session"]["status"],
        "interrupted"
    );
    assert_eq!(
        running.server.live_agents(),
        1,
        "stopping the turn took the agent with it"
    );
    assert_eq!(agent.starts(), 1);

    // …and it is reaped when the session does end. Ended by deleting the project
    // rather than by stopping the server, because `TestServer::stop` consumes it
    // and the gauge has to be readable afterwards.
    running
        .client
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
    running.server.await_live_agents(0).await;

    running.client.close().await;
    running.server.stop().await;
}

/// Cancelling a permission is an interrupt this server did not send but did
/// cause, and the turn has to end the same way.
///
/// Ticket 13 sent `cancel` correctly — a denial carrying `interrupt: true` — and
/// named this as the half it was leaving undone: what the CLI *does* with that
/// flag was untested. `15-permission-cancelled.ndjson` is the recording of it,
/// and it ends exactly as an interrupt does: `"is_error": true` with
/// `[Request interrupted by user]` in front of it. So a server that read the wire
/// alone would tell the developer that cancelling had gone wrong.
#[tokio::test]
async fn cancelling_a_permission_ends_the_turn_as_stopped_rather_than_as_failed() {
    let agent = ScriptedAgent::replaying("15-permission-cancelled");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn_in("thread-1", "message-1", "write note.txt", ASKS_FIRST),
        )
        .await
        .expect_success();
    let (_, request_id) = client.events_until_permission(&subscription).await;

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &request_id, "cancel"),
        )
        .await
        .expect_success();
    let rest = client.events_through_the_turn(&subscription).await;

    // The agent was told to stop, on the same line as the refusal rather than on
    // a second one — a cancel is one decision, not a decline followed by a stop.
    let written = written(&agent);
    assert_eq!(written.len(), 1, "{written:#?}");
    assert_eq!(written[0]["response"]["response"]["behavior"], "deny");
    assert_eq!(written[0]["response"]["response"]["interrupt"], json!(true));

    let completed = activity(&rest, "turn.completed")["payload"]["activity"].clone();
    assert_eq!(completed["payload"]["interrupted"], json!(true));
    assert_eq!(completed["tone"], "info");
    let ended = last_session(&rest, "the cancelled turn");
    assert_eq!(ended["payload"]["session"]["status"], "interrupted");
    assert_eq!(ended["payload"]["session"]["lastError"], Value::Null);

    client.close().await;
    server.stop().await;
}

/// An agent that will not stop says so, and the turn is not reported as stopped.
///
/// No recording contains this — a healthy CLI acknowledges every interrupt — so
/// the script is hand-written, which is exactly what
/// `fixtures/claude-cli/03-synthetic-drift.ndjson` exists for as a precedent. The
/// case matters because the alternative is silent: a stop button that reports
/// success and a turn that carries on to say something the developer thought they
/// had prevented.
#[tokio::test]
async fn an_agent_that_refuses_to_stop_says_so_and_the_turn_ends_as_it_was_going_to() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5","cwd":"/tmp","permissionMode":"bypassPermissions","tools":["Read"]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"working"}}}"#,
        AWAIT_ANSWER,
        r#"{"type":"control_response","response":{"subtype":"error","request_id":"interrupt-1","error":"No active turn to interrupt"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working, and finished"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
    ]);
    let mut running = running(&agent).await;

    running
        .client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some(&running.turn_id)),
        )
        .await
        .expect_success();
    let rest = running
        .client
        .events_through_the_turn(&running.subscription)
        .await;

    let refused = activity(&rest, "turn.interrupt-failed")["payload"]["activity"].clone();
    assert_eq!(refused["tone"], "error");
    assert!(
        text(&refused["payload"]["detail"]).contains("No active turn to interrupt"),
        "the CLI's own words are the useful part: {}",
        refused["payload"]["detail"]
    );

    // The turn finished the way it was going to, and is not reported as one the
    // developer stopped — because it was not.
    let completed = activity(&rest, "turn.completed")["payload"]["activity"].clone();
    assert_eq!(completed["payload"]["interrupted"], json!(false));
    assert_eq!(
        last_session(&rest, "the turn that would not stop")["payload"]["session"]["status"],
        "ready"
    );

    running.client.close().await;
    running.server.stop().await;
}

/// A conversation this server has never heard of is refused by name.
///
/// The one thing that is not a race: a command naming a thread that does not
/// exist is a client bug, and the message says which thread so a developer can
/// tell which one it meant.
#[tokio::test]
async fn stopping_a_conversation_that_does_not_exist_is_refused_by_name() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;
    client.open_conversation(&workspace, "thread-1").await;

    let refusal = client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-2", None),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    assert!(text(&refusal["message"]).contains("thread-2"), "{refusal}");

    client.close().await;
    server.stop().await;
}
