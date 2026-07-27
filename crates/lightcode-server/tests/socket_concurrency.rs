//! More than one conversation at a time, driven the way the UI drives them.
//!
//! Ticket 16, and it is a testing ticket: the isolation it asks for is a
//! consequence of how the registry was built rather than a feature added for it.
//! `crate::threads` keys everything per thread — an `Entry` holds one
//! conversation's state, its own event feed and its own `Live` — and nothing in
//! the crate is process-global. So the job here is to *demonstrate* that from
//! outside, one case per criterion, rather than to take a single "two
//! conversations stream" test as evidence for eight things at once.
//!
//! ## What the assertions are made of
//!
//! Three kinds of observable, and each answers a criterion the others cannot:
//!
//! - **Each subscription's own events.** A thread subscription is per thread, so
//!   an event carrying another thread's `aggregateId` is bleed-through by
//!   definition. This is what "output never crosses" means on the wire.
//! - **The sequence numbers on those events.** `Sequences` is deliberately shared
//!   — one total order across the whole feed, which the client relies on to drop
//!   rather than reorder — and that makes it the honest way to prove *simultaneity*
//!   without asserting on a clock: if one conversation's streamed deltas take
//!   sequences either side of the other's, both agents were producing output at
//!   once.
//! - **Each project's own working-directory marker, and the live-agent gauge.**
//!   `WORKING_DIRECTORY_MARKER` lands wherever the child was started. Both
//!   children write the same name, so a marker in each folder is only worth
//!   something if the turns are taken one at a time — which is how that case is
//!   driven. The gauge is how "one session's child was released and the other's
//!   was not" is read from outside.
//!
//! ## Two constraints the harness imposes, and what they cost
//!
//! **`binaryPath` is one server-wide setting**, so both conversations spawn the
//! *same* [`ScriptedAgent`]. Its `starts()`, `arguments()` and `answers()` logs
//! are files beside the script that two concurrent processes would interleave or
//! lock, so nothing here asserts on them — the per-project observables above are
//! used instead. It also means both children replay the same lines, and a script
//! stop that only one of them is released from would hang the other: the
//! interrupt case works around that by putting the stop on a turn only one
//! conversation ever reaches.
//!
//! **One socket connection, two subscriptions.** What the real app does — one
//! window, one WebSocket — and safe here because `values_until` buffers frames
//! that do not match rather than dropping them, so draining one subscription
//! never loses the other's.
//!
//! ## What "independent" does not promise
//!
//! Within one project, isolation is *server-side*: transcripts, events, sessions
//! and subprocesses never cross. Both agents run in the same folder at the same
//! time and may edit the same files; last write wins. Upstream isolates
//! same-project threads with git worktrees, and this project's spec excludes
//! worktrees by name, so conflict-freedom is not on offer and this file does not
//! pretend it is. See `docs/adr/0003-independent-conversations-are-not-conflict-free-ones.md`.

mod harness;

use harness::agent::{ScriptedAgent, AWAIT_ANSWER, PAUSE, WORKING_DIRECTORY_MARKER};
use harness::conversation::{
    activity, assistant_sends, create_thread, find_activity, follow_up, interrupt_turn,
    last_session, start_turn_for,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};
use std::path::PathBuf;

/// The `system/init` the CLI opens a process with. On the first turn only —
/// later turns are the same child answering again, and it does not re-announce
/// itself.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5","cwd":"/tmp","permissionMode":"bypassPermissions","tools":["Read"]}"#;

/// A first turn that answers at once and streams nothing.
///
/// **Its job is to get the child started**, and that is why the cases about
/// simultaneity take it before the turn they measure. Spawning a `claude.cmd`
/// through `cmd.exe` costs a good fraction of a second, and more when the rest of
/// the suite is doing the same thing beside it — so two conversations whose
/// *first* turns were dispatched together would still reach the agent a spawn
/// apart, and whether their replies overlapped would be a fact about the machine.
/// Once both children exist, the only gap left between the two dispatches is the
/// microseconds it takes to write a second frame.
///
/// It streams nothing on purpose too: a turn with no deltas has no accumulation
/// for the buffered message to replace, so it leaves the reconciliation gauge
/// alone and the turns that are the subject can be counted exactly.
const A_WARM_UP_TURN: [&str; 3] = [
    INIT,
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ready when you are"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":8,"total_cost_usd":0.001}"#,
];

/// The turn the concurrency cases are actually about: about a second spent
/// saying two words.
///
/// The pause is the whole point. An agent that answers instantaneously would let
/// one conversation finish before the other had begun, and "they streamed
/// simultaneously" would be a claim the wire could not support. With it — and
/// with both children already warm — each is still mid-reply while the other is
/// producing its own.
const A_STREAMED_TURN: [&str; 6] = [
    r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"one "}}}"#,
    PAUSE,
    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"moment"}}}"#,
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"one moment"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":1200,"total_cost_usd":0.001}"#,
];

/// What the agent says on a second turn. Distinct wording because it is the
/// discriminator: hearing it means the conversation that asked was still the one
/// being answered.
const A_SECOND_TURN: [&str; 2] = [
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"still here"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":12,"total_cost_usd":0.001}"#,
];

/// A turn that streams and then waits to be stopped.
///
/// **Deliberately the third turn of a process**, which is the one constraint the
/// shared binary imposes on this file. [`AWAIT_ANSWER`] blocks the child until
/// the server writes it a line, and the server only writes to the child it is
/// interrupting — so an undisturbed conversation that reached this script would
/// hang there forever. Putting it on a turn that only the interrupted
/// conversation ever takes is what keeps the other one usable.
const A_TURN_TO_STOP: [&str; 6] = [
    r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half a "}}}"#,
    AWAIT_ANSWER,
    r#"{"type":"control_response","response":{"subtype":"success","request_id":"interrupt-1","response":{"still_queued":[]}}}"#,
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"half a "}]}}"#,
    r#"{"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":900,"total_cost_usd":0.0}"#,
];

/// Two projects on disk, two conversations open on one connection.
///
/// Everything outlives the read, the way `socket_interrupt.rs`'s `Running` does:
/// the workspaces are where the agents' working-directory markers land and the
/// subscriptions are what the turns are read out of, so neither can be dropped
/// while the test is still asserting.
struct Pair {
    server: TestServer,
    client: SocketClient,
    /// One per project: two when the conversations are in different projects,
    /// one when they share it.
    workspaces: Vec<Workspace>,
    subscriptions: [String; 2],
}

impl Pair {
    /// A conversation in each of two projects, neither having said anything yet.
    async fn across_two_projects(agent: &ScriptedAgent) -> Pair {
        let workspaces = vec![Workspace::with(&["src/"]), Workspace::with(&["docs/"])];
        let server = TestServer::start_with_agent(&agent.configured()).await;
        let mut client = server.connect().await;

        let first = client
            .open_conversation_in(&workspaces[0], "project-1", "thread-1")
            .await;
        let second = client
            .open_conversation_in(&workspaces[1], "project-2", "thread-2")
            .await;

        Pair {
            server,
            client,
            workspaces,
            subscriptions: [first, second],
        }
    }

    /// Two conversations in the *same* project — one folder, one registration,
    /// two threads. There is one workspace because the two conversations really
    /// do share it, which is the limitation ADR-0003 records.
    async fn within_one_project(agent: &ScriptedAgent) -> Pair {
        let workspace = Workspace::with(&["src/"]);
        let server = TestServer::start_with_agent(&agent.configured()).await;
        let mut client = server.connect().await;

        let first = client.open_conversation(&workspace, "thread-1").await;
        // The project is already registered; a second conversation in it is a
        // second thread and a second subscription, and nothing else.
        client
            .call(
                "orchestration.dispatchCommand",
                create_thread("project-1", "thread-2"),
            )
            .await
            .expect_success();
        let second = client.watch_conversation("thread-2").await;

        Pair {
            server,
            workspaces: vec![workspace],
            client,
            subscriptions: [first, second],
        }
    }

    /// Both conversations take a turn that starts their agent and nothing more.
    ///
    /// Neither is read before both have been dispatched — a helper that drove one
    /// to its end first would be starting two conversations in sequence, which is
    /// not the claim any of these tests makes. See [`A_WARM_UP_TURN`] for why the
    /// cases about simultaneity begin here rather than with the turn they
    /// measure.
    async fn warm_up(&mut self, projects: [&str; 2]) -> [Vec<Value>; 2] {
        for (index, (project, text)) in projects
            .iter()
            .zip(["are you there", "and you"])
            .enumerate()
        {
            self.client
                .call(
                    "orchestration.dispatchCommand",
                    start_turn_for(
                        project,
                        &format!("thread-{}", index + 1),
                        &format!("message-{}", index + 1),
                        text,
                    ),
                )
                .await
                .expect_success();
        }
        self.both_turns().await
    }

    /// A follow-up on both conversations, dispatched back to back and read out
    /// afterwards — which is what puts the two turns in flight together.
    async fn both_follow_up(&mut self, texts: [&str; 2]) -> [Vec<Value>; 2] {
        for (index, text) in texts.into_iter().enumerate() {
            self.client
                .call(
                    "orchestration.dispatchCommand",
                    follow_up(
                        &format!("thread-{}", index + 1),
                        &format!("message-{}", index + 3),
                        text,
                    ),
                )
                .await
                .expect_success();
        }
        self.both_turns().await
    }

    /// Both turns read out to their ends, first conversation then second.
    ///
    /// Sequential here and concurrent on the server: the subscription that is not
    /// being read keeps producing into its own backlog, and the sequence numbers
    /// on what it produced are what say so.
    async fn both_turns(&mut self) -> [Vec<Value>; 2] {
        let [first, second] = self.subscriptions.clone();
        let first = self.client.events_through_the_turn(&first).await;
        let second = self.client.events_through_the_turn(&second).await;
        [first, second]
    }

    /// The subscription for one of the two conversations, by number.
    fn subscription(&self, whose: usize) -> String {
        self.subscriptions[whose - 1].clone()
    }

    async fn stop(self) {
        self.client.close().await;
        self.server.stop().await;
    }
}

/// The `project.delete` the UI sends.
fn delete_project(id: &str) -> Value {
    json!({
        "type": "project.delete",
        "commandId": format!("test:delete:{id}"),
        "projectId": id,
    })
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

/// The sequence of every event in a run, in the order it arrived.
fn sequences(events: &[Value]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|item| item["event"]["sequence"].as_i64())
        .collect()
}

/// The sequences of the events the agent produced *while it was talking* — the
/// streamed assistant messages.
///
/// These exist only between the first delta and the buffered message that
/// replaces it, so the span they cover is the span the turn was genuinely in
/// flight. Two spans that overlap are two agents producing output at once, which
/// is what "simultaneously" has to mean if it is to be checked rather than timed.
fn streaming_sequences(events: &[Value]) -> Vec<i64> {
    events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| {
            event["type"] == "thread.message-sent"
                && event["payload"]["role"] == "assistant"
                && event["payload"]["streaming"] == json!(true)
        })
        .filter_map(|event| event["sequence"].as_i64())
        .collect()
}

/// The threads an event stream is about, as the contract names them. One entry
/// per event, deduplicated — a stream carrying two is bleed-through.
fn aggregates(events: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = events
        .iter()
        .filter(|item| item["kind"] == "event")
        .map(|item| text(&item["event"]["aggregateId"]))
        .collect();
    seen.sort();
    seen.dedup();
    seen
}

/// Every message in a thread's stored transcript, as (role, text).
fn transcript(snapshot: &Value) -> Vec<(String, String)> {
    snapshot["thread"]["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("a thread has messages: {}", snapshot["thread"]))
        .iter()
        .map(|message| (text(&message["role"]), text(&message["text"])))
        .collect()
}

/// Open a thread subscription and take the snapshot it opens with, leaving the
/// subscription open.
///
/// How the state of a conversation nobody is currently reading is observed:
/// a snapshot is what any client arriving now would be handed, so it is the
/// honest answer to "what does the server say about that conversation right
/// this moment". Used where the other conversation is deliberately mid-turn and
/// therefore has no new events to wait for.
async fn snapshot_of(client: &mut SocketClient, thread_id: &str) -> Value {
    let subscription = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({"threadId": thread_id}),
        )
        .await;
    let opening = client.next_chunk(&subscription).await;
    client.ack(&subscription).await;
    let snapshot = opening
        .into_iter()
        .find(|item| item["kind"] == "snapshot")
        .unwrap_or_else(|| panic!("no snapshot for {thread_id}"));
    client.interrupt(&subscription).await;
    snapshot["snapshot"].clone()
}

/// The project list as it stands, read on the connection that is already open.
async fn shell_snapshot(client: &mut SocketClient) -> Value {
    let subscription = client
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    let opening = client.next_chunk(&subscription).await;
    client.ack(&subscription).await;
    let snapshot = opening
        .into_iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the shell describes itself");
    client.interrupt(&subscription).await;
    snapshot["snapshot"].clone()
}

/// The first criterion. Two conversations in two projects, both streaming at the
/// same moment.
///
/// The proof is the shared sequence counter rather than a clock. Each turn's
/// streamed deltas occupy a span of sequences, and the two spans overlap: the
/// second conversation published a delta before the first had published its
/// last, and the other way round. There is no ordering of "one turn, then the
/// other" that produces that.
#[tokio::test]
async fn two_conversations_in_different_projects_stream_at_the_same_time() {
    let agent = ScriptedAgent::per_turn(&[A_WARM_UP_TURN.to_vec(), A_STREAMED_TURN.to_vec()]);
    let mut pair = Pair::across_two_projects(&agent).await;

    pair.warm_up(["project-1", "project-2"]).await;
    let [first, second] = pair
        .both_follow_up(["what is in src", "what is in docs"])
        .await;

    let one = streaming_sequences(&first);
    let two = streaming_sequences(&second);
    assert!(
        one.len() > 1 && two.len() > 1,
        "each turn has to stream more than once for its span to mean anything: {one:?} {two:?}"
    );
    assert!(
        one.first() < two.last() && two.first() < one.last(),
        "the turns ran one after the other rather than together: {one:?} then {two:?}"
    );

    // And both of them finished, which is the other half of "can stream
    // simultaneously" — sharing the server did not cost either one its ending.
    for (events, whose) in [(&first, "thread-1"), (&second, "thread-2")] {
        assert_eq!(
            assistant_sends(events).last().map(|(said, _)| said.clone()),
            Some("one moment".to_string()),
            "{whose} did not finish saying its piece"
        );
        assert_eq!(
            last_session(events, whose)["payload"]["session"]["status"],
            "ready"
        );
    }
    assert_eq!(
        pair.server.live_agents(),
        2,
        "two conversations were served by fewer than two agents"
    );

    pair.stop().await;
}

/// The second criterion. Nothing one conversation produced appears in the other.
///
/// Three separate ways of crossing, checked separately because a server could
/// get any two of them right: the live event feeds, the stored transcripts, and
/// the session each thread reports.
#[tokio::test]
async fn neither_conversation_can_see_the_other() {
    let agent = ScriptedAgent::per_turn(&[A_WARM_UP_TURN.to_vec(), A_STREAMED_TURN.to_vec()]);
    let mut pair = Pair::across_two_projects(&agent).await;

    pair.warm_up(["project-1", "project-2"]).await;
    let [first, second] = pair
        .both_follow_up(["ask about bicycles", "ask about bridges"])
        .await;

    // **The feeds.** A thread subscription is per thread, so an event naming
    // another one is bleed-through by definition.
    assert_eq!(aggregates(&first), vec!["thread-1".to_string()]);
    assert_eq!(aggregates(&second), vec!["thread-2".to_string()]);

    // **The transcripts.** Each holds its own developer's words and one reply —
    // not the other's, and not two.
    let one = snapshot_of(&mut pair.client, "thread-1").await;
    let two = snapshot_of(&mut pair.client, "thread-2").await;
    assert_eq!(
        transcript(&one),
        vec![
            ("user".to_string(), "are you there".to_string()),
            ("assistant".to_string(), "ready when you are".to_string()),
            ("user".to_string(), "ask about bicycles".to_string()),
            ("assistant".to_string(), "one moment".to_string()),
        ]
    );
    assert_eq!(
        transcript(&two),
        vec![
            ("user".to_string(), "and you".to_string()),
            ("assistant".to_string(), "ready when you are".to_string()),
            ("user".to_string(), "ask about bridges".to_string()),
            ("assistant".to_string(), "one moment".to_string()),
        ]
    );

    // **The session state.** Each thread is in its own project and each turn has
    // its own id — a shared latest-turn would show up as one of these matching.
    assert_eq!(one["thread"]["projectId"], "project-1");
    assert_eq!(two["thread"]["projectId"], "project-2");
    assert_ne!(
        one["thread"]["latestTurn"]["turnId"], two["thread"]["latestTurn"]["turnId"],
        "both conversations report the same turn: {}",
        one["thread"]["latestTurn"]
    );
    assert_eq!(one["thread"]["session"]["threadId"], "thread-1");
    assert_eq!(two["thread"]["session"]["threadId"], "thread-2");

    pair.stop().await;
}

/// The third criterion. Each child ran in its own project's folder.
///
/// The marker is written by the agent script itself, to a relative path, on
/// every turn — so it lands wherever the child was started.
///
/// **The conversations take their turns one at a time, and that is the whole
/// design of this test.** Both children write a file of the same name, because
/// both replay the same script, so "a marker in each folder" is also what a
/// server that started each child in the *other* project's folder would produce.
/// Driving one conversation to its end before the other has said anything makes
/// the marker attributable: after the first turn, exactly one folder has one, and
/// it is the first conversation's.
#[tokio::test]
async fn each_agent_runs_in_its_own_projects_working_directory() {
    let agent = ScriptedAgent::emitting(&A_WARM_UP_TURN);
    let mut pair = Pair::across_two_projects(&agent).await;
    let folders: Vec<PathBuf> = pair
        .workspaces
        .iter()
        .map(|workspace| workspace.path().join(WORKING_DIRECTORY_MARKER))
        .collect();

    for (whose, project) in ["project-1", "project-2"].into_iter().enumerate() {
        let subscription = pair.subscription(whose + 1);
        pair.client
            .call(
                "orchestration.dispatchCommand",
                start_turn_for(
                    project,
                    &format!("thread-{}", whose + 1),
                    &format!("message-{}", whose + 1),
                    "where are you",
                ),
            )
            .await
            .expect_success();
        pair.client.events_through_the_turn(&subscription).await;

        for (index, folder) in folders.iter().enumerate() {
            assert_eq!(
                folder.exists(),
                index <= whose,
                "after {project}'s turn, {} {}",
                folder.display(),
                match index <= whose {
                    true => "has no marker",
                    false => "has one it could only have got from another project's agent",
                }
            );
        }
    }

    pair.stop().await;
}

/// The fourth criterion, and one of the two worth the effort: stopping one
/// conversation leaves the other alone.
///
/// Interrupting is where `Live`, the signal channel and the driver's own
/// wind-down all meet, and every one of those paths had only ever run with a
/// single agent in the process. What could go wrong is not subtle — a signal
/// reaching the wrong child, or a stop settling every session — and both would
/// be invisible to a test that only ever stopped the only thing running.
///
/// The stop is put on the third turn of a process for the reason
/// [`A_TURN_TO_STOP`] gives: the two conversations share a binary, and a script
/// that waits to be written to would hang whichever of them was not interrupted.
#[tokio::test]
async fn stopping_one_conversation_leaves_the_other_running() {
    let agent = ScriptedAgent::per_turn(&[
        A_WARM_UP_TURN.to_vec(),
        A_SECOND_TURN.to_vec(),
        A_TURN_TO_STOP.to_vec(),
    ]);
    let mut pair = Pair::across_two_projects(&agent).await;

    pair.warm_up(["project-1", "project-2"]).await;
    let (stopping, untouched) = (pair.subscription(1), pair.subscription(2));

    // One follow-up walks the interrupted conversation to its second turn, so the
    // *next* one is the third and reaches the script that waits to be stopped.
    // The other conversation stays on its first, which is what keeps it off that
    // script and therefore still able to answer.
    pair.client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-3", "carry on"),
        )
        .await
        .expect_success();
    pair.client.events_through_the_turn(&stopping).await;

    pair.client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-4", "and again"),
        )
        .await
        .expect_success();
    let streaming = pair.client.events_until_streaming(&stopping).await;
    let turn_id = last_session(&streaming, "the running turn")["payload"]["session"]["activeTurnId"]
        .as_str()
        .expect("a running session names the turn it is working on")
        .to_string();
    pair.client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("thread-1", Some(&turn_id)),
        )
        .await
        .expect_success();
    let stopped = pair.client.events_through_the_turn(&stopping).await;

    // The conversation that was stopped is stopped.
    assert_eq!(
        last_session(&stopped, "the stopped turn")["payload"]["session"]["status"],
        "interrupted"
    );
    assert_eq!(
        activity(&stopped, "turn.interrupted")["payload"]["activity"]["turnId"],
        turn_id
    );

    // The other one heard nothing about it, and its own child is still there.
    let other = snapshot_of(&mut pair.client, "thread-2").await;
    assert_eq!(
        other["thread"]["session"]["status"], "ready",
        "stopping one conversation moved the other's session: {}",
        other["thread"]["session"]
    );
    assert_eq!(other["thread"]["latestTurn"]["state"], "completed");
    assert_eq!(
        pair.server.live_agents(),
        2,
        "stopping one conversation took an agent with it"
    );

    // …and it can still take a turn. The reply exists only in the second script,
    // so hearing it means the child that answered is the one that was already
    // there — the interrupt next door did not restart it.
    pair.client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-2", "message-5", "still there?"),
        )
        .await
        .expect_success();
    let after = pair.client.events_through_the_turn(&untouched).await;
    assert_eq!(
        assistant_sends(&after).last().map(|(said, _)| said.clone()),
        Some("still here".to_string())
    );
    assert_eq!(aggregates(&after), vec!["thread-2".to_string()]);
    assert!(
        find_activity(&after, "turn.interrupted").is_none(),
        "the other conversation was marked as stopped too"
    );

    pair.stop().await;
}

/// The fifth criterion. Two conversations in the *same* project are as separate
/// as two in different ones.
///
/// Separate on the server, which is what the ticket asks for and the whole of
/// what it asks for: two children, two transcripts, two sessions. They share the
/// folder — both write the same working-directory marker to it, and there is
/// only one — and that is the documented limitation rather than a defect. See
/// this file's header and ADR-0003.
#[tokio::test]
async fn two_conversations_in_one_project_stay_independent() {
    let agent = ScriptedAgent::per_turn(&[A_WARM_UP_TURN.to_vec(), A_STREAMED_TURN.to_vec()]);
    let mut pair = Pair::within_one_project(&agent).await;

    pair.warm_up(["project-1", "project-1"]).await;
    let [first, second] = pair
        .both_follow_up(["the first question", "the second question"])
        .await;

    assert_eq!(aggregates(&first), vec!["thread-1".to_string()]);
    assert_eq!(aggregates(&second), vec!["thread-2".to_string()]);

    let one = streaming_sequences(&first);
    let two = streaming_sequences(&second);
    assert!(
        one.len() > 1 && two.len() > 1,
        "each turn has to stream more than once for its span to mean anything: {one:?} {two:?}"
    );
    assert!(
        one.first() < two.last() && two.first() < one.last(),
        "the two turns ran one after the other: {one:?} then {two:?}"
    );

    let one = snapshot_of(&mut pair.client, "thread-1").await;
    let two = snapshot_of(&mut pair.client, "thread-2").await;
    assert_eq!(
        transcript(&one),
        vec![
            ("user".to_string(), "are you there".to_string()),
            ("assistant".to_string(), "ready when you are".to_string()),
            ("user".to_string(), "the first question".to_string()),
            ("assistant".to_string(), "one moment".to_string()),
        ]
    );
    assert_eq!(
        transcript(&two),
        vec![
            ("user".to_string(), "and you".to_string()),
            ("assistant".to_string(), "ready when you are".to_string()),
            ("user".to_string(), "the second question".to_string()),
            ("assistant".to_string(), "one moment".to_string()),
        ]
    );
    assert_eq!(one["thread"]["projectId"], "project-1");
    assert_eq!(two["thread"]["projectId"], "project-1");
    assert_eq!(one["thread"]["session"]["threadId"], "thread-1");
    assert_eq!(two["thread"]["session"]["threadId"], "thread-2");
    assert_ne!(
        one["thread"]["latestTurn"]["turnId"], two["thread"]["latestTurn"]["turnId"],
        "one project got one turn for two conversations: {}",
        one["thread"]["latestTurn"]
    );
    assert_eq!(
        pair.server.live_agents(),
        2,
        "two conversations in one project shared a child"
    );

    pair.stop().await;
}

/// The sixth criterion, and the other one worth the effort: ending one session
/// releases that session's subprocess and no other.
///
/// Deleting a project is where `Threads::forget`, `Inner::winding_down` and
/// `shutdown` interact, and like the interrupt path they had only ever run with
/// one agent in the process. `forget` takes the deleted project's entries out of
/// the registry and drops each one's prompt channel; a version that dropped
/// every entry's would leave the surviving conversation with a child that never
/// answers again, which is what the follow-up below rules out.
///
/// **What the gauge does and does not say.** `forget` decrements `live_agents`
/// itself, in the same call that removes the entry, so the gauge falling to
/// exactly one says the tracking is *per session* — one conversation's child was
/// released and the other's was not. It does not, on its own, say the released
/// child was reaped: `forget` parks the driver's handle on `winding_down` rather
/// than waiting for it, on purpose, because a project delete answers the client
/// immediately. What makes reaping true is `shutdown` awaiting that handle, and
/// the nearest a socket test can come to observing it is that `Pair::stop` at the
/// end of this test returns rather than hanging — which it would if the driver
/// had never been told there would be no more turns.
///
/// Whether a handle that had been *dropped* instead of parked — a detached task
/// that shutdown would not wait for — could be distinguished from outside is a
/// question this seam cannot answer. It is `threads.rs`'s own `#[cfg(test)]`
/// units that would have to.
#[tokio::test]
async fn deleting_one_project_releases_its_agent_and_leaves_the_others_alone() {
    let agent = ScriptedAgent::per_turn(&[A_WARM_UP_TURN.to_vec(), A_SECOND_TURN.to_vec()]);
    let mut pair = Pair::across_two_projects(&agent).await;

    pair.warm_up(["project-1", "project-2"]).await;
    let surviving = pair.subscription(2);
    assert_eq!(pair.server.live_agents(), 2);

    pair.client
        .call("orchestration.dispatchCommand", delete_project("project-1"))
        .await
        .expect_success();

    // Exactly one session's child was released. Waited for rather than read
    // outright because the gauge is not this call's to settle — see the note
    // above — and a test that pinned *when* it moved would be pinning the shape
    // of `forget` rather than what a client can see.
    pair.server.await_live_agents(1).await;

    // The surviving conversation is untouched and still usable. `still here` is
    // in the second script only, so hearing it means the same child took this
    // turn as took the first.
    pair.client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-2", "message-3", "are you still there"),
        )
        .await
        .expect_success();
    let after = pair.client.events_through_the_turn(&surviving).await;
    assert_eq!(
        assistant_sends(&after).last().map(|(said, _)| said.clone()),
        Some("still here".to_string()),
        "the surviving conversation lost its agent when the other project was deleted"
    );
    assert_eq!(aggregates(&after), vec!["thread-2".to_string()]);

    // And the project list says exactly one project and one conversation went.
    let shell = shell_snapshot(&mut pair.client).await;
    let projects: Vec<String> = shell["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| text(&project["id"]))
        .collect();
    let threads: Vec<String> = shell["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .map(|thread| text(&thread["id"]))
        .collect();
    assert_eq!(projects, vec!["project-2".to_string()]);
    assert_eq!(threads, vec!["thread-2".to_string()]);

    pair.stop().await;
}

/// The seventh criterion. What the two conversations share, they share on
/// purpose.
///
/// There is no process-global mutable state in the crate — no `static mut`, no
/// `OnceLock` singleton, no `set_var`, no ambient `current_dir` on the agent
/// path — and everything a conversation has hangs off its own `Entry`. Three
/// things genuinely are shared, and this is the test that each of them is
/// *aggregation* rather than bleed-through:
///
/// - **`Sequences`**, one total order across the whole feed. Shared because the
///   client relies on it: it drops anything at or below the sequence it holds
///   rather than reordering, so two counters would make one conversation's
///   changes invisible. What it must never do is hand the same number to two
///   changes.
/// - **The `shell` broadcast**, the project list. Both conversations appear in
///   it, each carrying its own project and its own turn.
/// - **The observability gauges.** They count across the server by design.
///   Reconciliation is the sharpest of them: two streamed turns must arrive as
///   two reconciled messages that both agreed with their deltas, not one, and
///   not two that disagreed because one turn's deltas were folded into the
///   other's buffered message.
#[tokio::test]
async fn what_the_two_conversations_share_is_shared_on_purpose() {
    let agent = ScriptedAgent::per_turn(&[A_WARM_UP_TURN.to_vec(), A_STREAMED_TURN.to_vec()]);
    let mut pair = Pair::across_two_projects(&agent).await;

    pair.warm_up(["project-1", "project-2"]).await;
    let [first, second] = pair.both_follow_up(["question one", "question two"]).await;

    // **One order, no collisions.** Every sequence either conversation was given
    // is distinct, and each conversation saw its own strictly increasing.
    for (events, whose) in [(&first, "thread-1"), (&second, "thread-2")] {
        let seen = sequences(events);
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        assert_eq!(seen, sorted, "{whose} was given its events out of order");
    }
    let mut every: Vec<i64> = sequences(&first).into_iter().chain(sequences(&second)).collect();
    let count = every.len();
    every.sort_unstable();
    every.dedup();
    assert_eq!(
        every.len(),
        count,
        "two changes were announced under one sequence, so a client would drop one of them"
    );

    // **The project list carries both**, each with its own project and its own
    // turn — the shared feed aggregating rather than merging.
    let shell = shell_snapshot(&mut pair.client).await;
    let threads: Vec<(String, String, String)> = shell["threads"]
        .as_array()
        .expect("threads")
        .iter()
        .map(|thread| {
            (
                text(&thread["id"]),
                text(&thread["projectId"]),
                text(&thread["latestTurn"]["turnId"]),
            )
        })
        .collect();
    assert_eq!(threads.len(), 2, "{threads:?}");
    assert_eq!(threads[0].0, "thread-1");
    assert_eq!(threads[0].1, "project-1");
    assert_eq!(threads[1].0, "thread-2");
    assert_eq!(threads[1].1, "project-2");
    assert_ne!(
        threads[0].2, threads[1].2,
        "the project list gave both conversations the same latest turn"
    );

    // **The gauges count both**, and reconciliation agreed on both. A server that
    // let one turn's deltas accumulate into the other's message would report two
    // reconciled and fewer than two agreed.
    assert_eq!(pair.server.live_agents(), 2);
    let reconciliation = pair.server.reconciliation();
    assert_eq!(
        (reconciliation.reconciled, reconciliation.agreed),
        (2, 2),
        "two streamed turns did not reconcile as two: {reconciliation:?}"
    );

    pair.stop().await;
}
