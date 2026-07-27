//! Permission prompts, driven the way the UI drives them.
//!
//! Ticket 13 at the seam the spec calls primary. This is the developer's control
//! surface over what runs against their code, so both paths are driven here and
//! neither is the happy one: an approval has to reach the agent, and a rejection
//! has to reach it *and* leave the conversation usable afterwards.
//!
//! ## All four scripts are recordings
//!
//! `fixtures/claude-cli/07-permission-approved.ndjson` through `10-` are real
//! `claude` output, captured for this ticket by answering the binary's own
//! control protocol three different ways and, once, not at all. That matters more
//! here than anywhere else in the suite, because the thing under test is a
//! *round trip*: a hand-written script would be this project asserting its own
//! belief about what the CLI does with an answer, and what the CLI does with an
//! answer is the entire feature.
//!
//! Each recording is one decision. `07` is what happened when the request was
//! approved and `08` is what happened when it was declined, so a test that
//! approved against `08` would be replaying a refusal it did not send — which is
//! why the two are separate files rather than one script with a branch.
//!
//! ## What the assertions are made of
//!
//! Two things, and they are deliberately different in kind:
//!
//! - **What the client would render** — `approval.requested` and
//!   `approval.resolved` activities, which is exactly and only what
//!   `derivePendingApprovals` (`apps/web/src/session-logic.ts`) folds its pending
//!   approval panel out of.
//! - **What the agent was actually told** — `ScriptedAgent::answers`, the lines
//!   the server wrote to the child's stdin. A server that published a decline and
//!   sent an allow would satisfy every assertion of the first kind.

mod harness;

use harness::agent::ScriptedAgent;
use harness::conversation::{
    activities_of, activity, create_project, find_activity, follow_up, respond_to_approval,
    start_turn_in,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// The mode whose whole meaning is that the agent asks first.
const ASKS_FIRST: &str = "approval-required";

/// A conversation stopped at a permission request, with everything still open.
///
/// Ticket 12's equivalent closes the client and hands back the events, because a
/// tool call is over by the time it can be asserted on. A permission request is
/// *not* over — the point of it is that nothing more happens until the developer
/// answers — so the socket, the subscription and the workspace all have to
/// outlive the read.
struct Asked {
    server: TestServer,
    client: SocketClient,
    subscription: String,
    /// The id the answer has to name.
    request_id: String,
    /// Everything published up to and including the request.
    events: Vec<Value>,
    /// Held so the project's folder outlives the conversation in it.
    _workspace: Workspace,
}

/// Register a project, open the conversation, send a turn in the mode that asks,
/// and read up to the agent asking.
async fn ask(agent: &ScriptedAgent) -> Asked {
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

    let (events, request_id) = client.events_until_permission(&subscription).await;
    Asked {
        server,
        client,
        subscription,
        request_id,
        events,
        _workspace: workspace,
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

/// Every line the server wrote to the agent, parsed.
fn answers(agent: &ScriptedAgent) -> Vec<Value> {
    agent
        .answers()
        .iter()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("the agent was written non-json: {error}: {line}"))
        })
        .collect()
}

/// The permission decision inside a `control_response`, which is the part the
/// CLI acts on.
fn decision(answer: &Value) -> &Value {
    &answer["response"]["response"]
}

/// The first criterion: the developer is told what is being asked, in terms they
/// can act on.
///
/// Every field asserted here is one the client reads by name, and the request is
/// worthless without any of them — no `requestId` and there is nothing to answer
/// with, no `requestKind` and `derivePendingApprovals` drops the request from the
/// panel outright, no `detail` and the developer is asked to approve something
/// unnamed.
#[tokio::test]
async fn a_permission_request_appears_saying_what_is_being_asked() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let asked = ask(&agent).await;

    let request = activity(&asked.events, "approval.requested");
    let payload = &request["payload"]["activity"]["payload"];
    assert_eq!(request["payload"]["activity"]["tone"], "approval");
    assert_eq!(payload["requestId"], asked.request_id);
    assert_eq!(payload["requestKind"], "file-change");
    assert!(
        text(&payload["detail"]).starts_with("Write: "),
        "{}",
        payload["detail"]
    );
    assert_eq!(payload["data"]["toolName"], "Write");

    // And it is about a call the developer can already see: the invocation row
    // goes up before the request does, and the two carry the same id.
    let invoked = activity(&asked.events, "tool.updated");
    assert_eq!(
        payload["data"]["toolCallId"],
        invoked["payload"]["activity"]["payload"]["data"]["toolCallId"],
        "the request is not visibly about the tool call beside it"
    );

    // Nothing has been decided, so nothing has been sent.
    assert!(agent.answers().is_empty(), "{:?}", agent.answers());

    // The turn is still running — which is true, and is also the only thing that
    // can be said: `OrchestrationSessionStatus` has no `waiting`, and a status
    // outside that union fails the client's decode of the whole session.
    let session = harness::conversation::last_session(&asked.events, "the request");
    assert_eq!(session["payload"]["session"]["status"], "running");

    // The thread list raises its hand, so a conversation waiting on the developer
    // is findable from another one.
    let shell = asked.server.connect().await.into_shell_snapshot().await;
    assert_eq!(
        shell["threads"][0]["hasPendingApprovals"],
        json!(true),
        "{}",
        shell["threads"][0]
    );

    asked.client.close().await;
    asked.server.stop().await;
}

/// Approving lets the action proceed and the turn continue.
///
/// The round trip end to end: the decision reaches the agent as an `allow`, the
/// recording's tool goes on to succeed, the panel closes, and the turn settles.
#[tokio::test]
async fn approving_lets_the_action_proceed_and_the_turn_finish() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let mut asked = ask(&agent).await;

    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &asked.request_id, "accept"),
        )
        .await
        .expect_success();
    let rest = asked
        .client
        .events_through_the_turn(&asked.subscription)
        .await;

    // What the agent was told. The input goes back unedited, because approving a
    // call the developer did not read would be the opposite of the point.
    let answers = answers(&agent);
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert_eq!(decision(&answers[0])["behavior"], "allow");
    assert_eq!(answers[0]["response"]["request_id"], asked.request_id);
    assert!(
        text(&decision(&answers[0])["updatedInput"]["file_path"]).ends_with("note.txt"),
        "{}",
        decision(&answers[0])["updatedInput"]
    );

    // What the developer sees. The resolution names the request, which is what
    // closes the panel.
    let resolved = activity(&rest, "approval.resolved")["payload"]["activity"].clone();
    assert_eq!(resolved["payload"]["requestId"], asked.request_id);
    assert_eq!(resolved["payload"]["decision"], "accept");
    assert_eq!(resolved["summary"], "Approved: Write");

    // The action proceeded: the tool returned, and it returned successfully.
    let returned = activity(&rest, "tool.completed")["payload"]["activity"].clone();
    assert_eq!(returned["payload"]["status"], "completed");
    assert!(
        text(&returned["payload"]["detail"]).contains("File created successfully"),
        "{}",
        returned["payload"]["detail"]
    );

    // And the turn finished.
    let ended = harness::conversation::last_session(&rest, "the approved turn");
    assert_eq!(ended["payload"]["session"]["status"], "ready");

    // **A known cost, pinned rather than described.** The four rows arrive in
    // this order because the CLI announces the tool call before it asks about it,
    // and the client collapses an invocation into its result only when the two
    // are *adjacent* in the work log (`collapseDerivedWorkLogEntries` merges with
    // the row before it, and only for `tool.*` kinds). So a permissioned call
    // renders as two rows where an unpermissioned one renders as one. Nothing on
    // this side can fix it: renaming the approval rows to `tool.*` is what would
    // make them adjacent, and it is also what would take them out of the panel.
    let order: Vec<String> = harness::conversation::activities(&asked.events)
        .into_iter()
        .chain(harness::conversation::activities(&rest))
        .map(|activity| text(&activity["kind"]))
        .filter(|kind| kind.starts_with("tool.") || kind.starts_with("approval."))
        .collect();
    assert_eq!(
        order,
        vec![
            "tool.updated",
            "approval.requested",
            "approval.resolved",
            "tool.completed",
        ],
        "the order changed — check whether the pair collapses now"
    );

    // The panel is closed on the record as well as on the wire, which is what a
    // client arriving after the fact reads.
    let snapshot = asked
        .server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(snapshot["thread"]["session"]["status"], "ready");

    asked.client.close().await;
    asked.server.stop().await;
}

/// Rejecting returns control to the agent cleanly, and the session stays usable.
///
/// The criterion the ticket is most emphatic about: a declined action must never
/// kill the conversation. Four things have to be true, and the last two are the
/// ones a naive implementation gets wrong — the agent has to be *told*, in a
/// message it can act on; the turn has to finish rather than hang; and the
/// conversation has to take another turn afterwards.
#[tokio::test]
async fn rejecting_returns_control_to_the_agent_and_the_session_stays_usable() {
    // A second turn of its own, because replaying the recording again would have
    // the agent ask permission a second time — a new question rather than an
    // answer to "did the conversation survive the first one".
    let agent = ScriptedAgent::replaying_then(
        "08-permission-declined",
        &[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"All right."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":10,"total_cost_usd":0.001}"#,
        ],
    );
    let mut asked = ask(&agent).await;

    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &asked.request_id, "decline"),
        )
        .await
        .expect_success();
    let rest = asked
        .client
        .events_through_the_turn(&asked.subscription)
        .await;

    // The agent was told, in a sentence rather than a code — the message reaches
    // the *model* as the tool's result, which is how it knows what happened.
    let answers = answers(&agent);
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert_eq!(decision(&answers[0])["behavior"], "deny");
    assert!(
        text(&decision(&answers[0])["message"]).contains("declined"),
        "{}",
        decision(&answers[0])["message"]
    );
    assert!(
        decision(&answers[0])["interrupt"].is_null(),
        "declining one tool must not stop the turn: {}",
        decision(&answers[0])
    );

    let resolved = activity(&rest, "approval.resolved")["payload"]["activity"].clone();
    assert_eq!(resolved["payload"]["decision"], "decline");
    assert_eq!(resolved["summary"], "Declined: Write");

    // Control came back: the tool reports the refusal, the agent answered anyway,
    // and the turn ended normally rather than in error.
    let returned = activity(&rest, "tool.completed")["payload"]["activity"].clone();
    assert!(
        text(&returned["payload"]["detail"]).contains("declined"),
        "{}",
        returned["payload"]["detail"]
    );
    let ended = harness::conversation::last_session(&rest, "the declined turn");
    assert_eq!(
        ended["payload"]["session"]["status"], "ready",
        "a declined action ended the session: {}",
        ended["payload"]["session"]
    );
    assert_eq!(ended["payload"]["session"]["lastError"], Value::Null);

    // And the conversation continues — the whole of what "the session remains
    // usable" means. The second turn's reply exists only in the second script, so
    // hearing it is proof that the *same* child took both turns: the turn counter
    // lives inside the process, and a re-spawn would have answered with the first
    // script again.
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "never mind"),
        )
        .await
        .expect_success();
    let after = asked
        .client
        .events_through_the_turn(&asked.subscription)
        .await;
    assert_eq!(
        harness::conversation::assistant_sends(&after)
            .last()
            .map(|(text, _)| text.clone()),
        Some("All right.".to_string()),
        "the conversation could not take a turn after the rejection"
    );
    assert_eq!(
        harness::conversation::last_session(&after, "the follow-up")["payload"]["session"]
            ["status"],
        "ready"
    );
    assert_eq!(
        agent.starts(),
        1,
        "the follow-up went to a second process, so the rejection killed the session"
    );

    asked.client.close().await;
    asked.server.stop().await;
}

/// A request nobody answers deadlocks nothing and leaks nothing.
///
/// The third path, and the one with no button behind it: the developer walks
/// away. Two things have to hold here — the server keeps working while the agent
/// waits, and the child is reaped when the session ends. The third, that the
/// request is *closed* rather than left behind, is the test after this one.
#[tokio::test]
async fn a_request_nobody_answers_deadlocks_nothing_and_leaks_nothing() {
    let agent = ScriptedAgent::replaying("09-permission-unanswered");
    let directory = tempfile::tempdir().expect("a temporary directory");
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
    let (events, _) = client.events_until_permission(&subscription).await;
    assert_eq!(server.live_agents(), 1);

    // The server is still answering while the agent waits. A driver that blocked
    // its read loop on a decision would fail here rather than at the end.
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-2", directory.path()),
        )
        .await
        .expect_success();
    assert_eq!(
        harness::conversation::last_session(&events, "the unanswered request")["payload"]
            ["session"]["status"],
        "running"
    );

    // Nothing answers, and the session ends anyway. Closing the agent's input
    // closes the permission stream with it, so the CLI abandons the request and
    // finishes the turn — which is what the recording from here on *is*.
    //
    // Ended by deleting the project rather than by stopping the server, and that
    // is the whole reason this is a test: the gauge has to be readable
    // *afterwards*. `TestServer::stop` consumes the server, so an assertion after
    // it could only be made against a different one — where the count is zero
    // because nothing ever ran there, which is not an assertion at all.
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
    server.await_live_agents(0).await;

    client.close().await;
    server.stop().await;
}

/// …and it does not leave a pending approval behind, on this run or the next.
///
/// The separate half, and it needs a restart to be worth anything. The client's
/// panel is folded out of `approval.requested` minus `approval.resolved`, and
/// those activities are *stored* — so a request left open is a composer the
/// developer cannot type into, still, after closing and reopening the app.
#[tokio::test]
async fn a_request_nobody_answered_is_not_still_pending_after_a_restart() {
    let agent = ScriptedAgent::replaying("09-permission-unanswered");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("registry.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
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

    client.close().await;
    server.stop().await;

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let snapshot = restarted
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let stored: Vec<&Value> = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .filter(|activity| {
            activity["kind"] == "approval.requested" || activity["kind"] == "approval.resolved"
        })
        .collect();
    assert_eq!(stored.len(), 2, "{stored:#?}");
    assert_eq!(stored[0]["kind"], "approval.requested");
    assert_eq!(stored[1]["kind"], "approval.resolved");
    assert_eq!(stored[1]["payload"]["requestId"], request_id);
    assert_eq!(
        stored[1]["payload"]["decision"], "cancel",
        "the request has to be closed as what it was — nobody approved it"
    );

    let shell = restarted.connect().await.into_shell_snapshot().await;
    let thread = shell["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .find(|thread| thread["id"] == "thread-1")
        .expect("the conversation survived the restart");
    assert_eq!(
        thread["hasPendingApprovals"],
        json!(false),
        "a request nobody answered is still pending after a restart"
    );

    restarted.stop().await;
}

/// "Always allow this session" hands the CLI back its own suggestion.
///
/// The one decision that changes what happens *next*, and the recording proves it
/// does: `10-permission-for-the-session.ndjson` contains two `Write` calls and
/// exactly one request, because the CLI applied the permission update and stopped
/// asking. This server does not compose a rule of its own — it agrees to the one
/// offered, which is the difference between granting the latitude the developer
/// chose and granting some other latitude.
#[tokio::test]
async fn approving_for_the_session_hands_the_cli_back_its_own_suggestion() {
    let agent = ScriptedAgent::replaying("10-permission-for-the-session");
    let mut asked = ask(&agent).await;

    // The request carried the suggestion, so there is something to agree to.
    let request = activity(&asked.events, "approval.requested");
    assert_eq!(request["payload"]["activity"]["payload"]["requestKind"], "file-change");

    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &asked.request_id, "acceptForSession"),
        )
        .await
        .expect_success();
    let rest = asked
        .client
        .events_through_the_turn(&asked.subscription)
        .await;

    let answers = answers(&agent);
    assert_eq!(answers.len(), 1, "{answers:#?}");
    assert_eq!(decision(&answers[0])["behavior"], "allow");
    assert_eq!(
        decision(&answers[0])["updatedPermissions"],
        json!([{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]),
        "the CLI's own suggestion is what stops it asking again"
    );

    let resolved = activity(&rest, "approval.resolved")["payload"]["activity"].clone();
    assert_eq!(resolved["summary"], "Approved for this session: Write");

    // Two tool calls, one request. That is the recording, and it is the whole
    // evidence that "for the session" means anything.
    assert_eq!(activities_of(&rest, &["tool.completed"]).len(), 2);
    assert!(
        find_activity(&rest, "approval.requested").is_none(),
        "the second call asked again"
    );

    asked.client.close().await;
    asked.server.stop().await;
}

/// The permission mode in effect is visible, so the developer knows how much
/// latitude the agent has.
///
/// Both halves, because they can differ and each answers a different question:
/// the session says what the *conversation* is set to, which is what the
/// composer's mode picker renders; the `session.init` activity says what the
/// **agent** reported, which is the mode actually in force after the CLI has
/// applied the user's own settings file over what it was asked for.
#[tokio::test]
async fn the_permission_mode_in_effect_is_visible() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let asked = ask(&agent).await;

    let session = harness::conversation::last_session(&asked.events, "the mode");
    assert_eq!(session["payload"]["session"]["runtimeMode"], ASKS_FIRST);

    let init = activity(&asked.events, "session.init")["payload"]["activity"].clone();
    assert_eq!(init["payload"]["permissionMode"], "default");
    assert!(
        text(&init["summary"]).contains("permission mode default"),
        "{}",
        init["summary"]
    );

    // And the agent was started the way that mode means. `approval-required` is
    // expressed by *omitting* `--permission-mode`, because the CLI's own default
    // is to ask; what makes the asking reach this server is the prompt tool.
    let argv = agent.arguments().join(" ");
    assert!(
        argv.contains("--permission-prompt-tool stdio"),
        "the agent was started with no way to ask: {argv}"
    );
    assert!(
        !argv.contains("--permission-mode"),
        "approval-required must not override the CLI's own asking default: {argv}"
    );

    asked.client.close().await;
    asked.server.stop().await;
}

/// A decision for a request nothing is waiting on is refused, and says so in the
/// words the client recognises.
///
/// Two shapes of the same mistake, and the second is the one that matters: a
/// panel left behind by a session that died without settling would otherwise be
/// permanent, because nothing else ever clears it. `derivePendingApprovals` drops
/// a request when a `provider.approval.respond.failed` says it is unknown, so
/// answering a stale panel is what unsticks it.
#[tokio::test]
async fn a_decision_nothing_is_waiting_for_is_refused_rather_than_swallowed() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let mut asked = ask(&agent).await;

    // An id this session never asked about. The command succeeds — the session is
    // there and took the decision — and the conversation says it went nowhere.
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", "req-that-never-was", "accept"),
        )
        .await
        .expect_success();
    let refusal = asked
        .client
        .values_until(&asked.subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "provider.approval.respond.failed"
        })
        .await;
    let failed =
        activity(&refusal, "provider.approval.respond.failed")["payload"]["activity"].clone();
    assert_eq!(
        failed["payload"]["requestId"], "req-that-never-was",
        "without the id the client cannot tell which panel to clear: {failed}"
    );
    assert!(
        text(&failed["payload"]["detail"])
            .to_lowercase()
            .contains("unknown pending permission request"),
        "the client clears a stale panel by matching this wording: {}",
        failed["payload"]["detail"]
    );
    assert!(
        agent.answers().is_empty(),
        "a decision for an unknown request reached the agent: {:?}",
        agent.answers()
    );

    // A decision this server cannot read is refused outright rather than rounded
    // to the nearest one — the nearest one might be the one that runs it.
    let refused = asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &asked.request_id, "allow"),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    assert!(
        text(&refused["message"]).contains("allow"),
        "{}",
        refused["message"]
    );
    assert!(agent.answers().is_empty(), "{:?}", agent.answers());

    asked.client.close().await;
    asked.server.stop().await;
}

/// A decision for a conversation with **no session at all** clears the panel too,
/// and that is the case the escape hatch really exists for.
///
/// The test above is a request the *driver* does not recognise. This one never
/// reaches a driver: after a restart there is no session, so the decision is
/// refused before it can be routed anywhere. That is the shape a hard kill
/// leaves — a conversation back from disk with an `approval.requested` in its
/// stored work log and nothing alive to settle it — and it is the one shape the
/// driver's own settle cannot reach. If the refusal were only a typed command
/// error, the composer would be disabled for the life of that conversation.
#[tokio::test]
async fn a_decision_for_a_conversation_with_no_session_still_clears_the_panel() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("registry.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
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
    client.close().await;
    server.stop().await;

    let restarted = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let mut client = restarted.connect().await;
    let subscription = client.watch_conversation("thread-1").await;

    let refused = client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &request_id, "accept"),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    assert!(!text(&refused["message"]).is_empty());

    // The command failed *and* the conversation says the request is gone. The
    // second is what unsticks the composer; without it the developer would see an
    // error beside a panel that never clears.
    let refusal = client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "provider.approval.respond.failed"
        })
        .await;
    let failed =
        activity(&refusal, "provider.approval.respond.failed")["payload"]["activity"].clone();
    assert_eq!(
        failed["payload"]["requestId"], request_id,
        "without the id the client cannot tell which panel to clear: {failed}"
    );
    assert!(
        text(&failed["payload"]["detail"])
            .to_lowercase()
            .contains("unknown pending permission request"),
        "{}",
        failed["payload"]["detail"]
    );
    assert!(agent.answers().is_empty(), "{:?}", agent.answers());

    // A conversation this server has never heard of is refused too, and quietly:
    // there is no work log to clear.
    let unknown = client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-2", "req-1", "accept"),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    assert!(text(&unknown["message"]).contains("thread-2"), "{unknown}");

    client.close().await;
    restarted.stop().await;
}
