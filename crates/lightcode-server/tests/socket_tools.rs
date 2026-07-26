//! Tool-use round-trips, driven the way the UI drives them.
//!
//! Ticket 12 at the seam the spec calls primary. What a developer can *see* the
//! agent doing is read out of the events a client would fold — the work log is
//! built from `thread.activity-appended` (`apps/web/src/session-logic.ts`,
//! `deriveWorkLogEntries`), so those events are the subject, and nothing here
//! reaches into the server to find them.
//!
//! ## Three of the scripts are recordings
//!
//! `fixtures/claude-cli/04-tool-use.ndjson`, `05-tool-failure.ndjson` and
//! `06-several-tool-calls.ndjson` are real `claude` output, captured for this
//! ticket and held to by `tests/protocol_golden.rs` as well. So the three cases
//! the ticket names — one tool call, several, and one that failed — are driven
//! against what the CLI actually said rather than against this project's idea of
//! it. The two purpose-built scripts here are for what a recording could not be
//! made to contain: a reply that narrates *before* it calls a tool, and a tool
//! whose input and output are larger than a row can show.
//!
//! ## What "visually associated" means at this seam
//!
//! The UI collapses an invocation into its result when the two carry the same
//! collapse key, and prefers `payload.data.toolCallId` for it
//! (`deriveToolLifecycleCollapseKey`). So the assertion that a result is
//! associated with its invocation is that the pair shares that id and the row's
//! heading — which is what the client needs to merge them, and is checkable
//! without a browser.

mod harness;

use harness::agent::ScriptedAgent;
use harness::conversation::{activities, activities_of, assistant_sends, start_turn};
use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};

/// The kinds a tool call's two halves are published as.
const CALL_ROWS: &[&str] = &["tool.updated", "tool.completed"];

/// One tool row, as the work log will read it.
///
/// A struct rather than a tuple because the assertions below are about *which*
/// field: four strings addressed by position is the shape `agent::Launch`'s own
/// documentation rejects for getting wrong silently.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    kind: String,
    title: String,
    status: String,
    call_id: String,
    /// The number the client sorts the work log by. `None` would mean the client
    /// falls back to a millisecond timestamp.
    sequence: Option<i64>,
}

/// Register a project, open the conversation, send one prompt, and read the turn
/// out — the whole of what every test here does before it asserts anything.
async fn turn(agent: &ScriptedAgent, prompt: &str) -> (TestServer, Vec<Value>) {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", prompt),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    client.close().await;
    (server, events)
}

/// Every tool row, **in the order the client will put them in** — by sequence,
/// which is what `compareActivitiesByOrder` uses when it is present.
///
/// Sorting here rather than trusting the publish order is the point: asserting the
/// order events arrived in would pass even if the client re-derived a different one
/// from the millisecond timestamps, which is exactly the failure the sequence
/// exists to prevent.
fn rows(events: &[Value]) -> Vec<Row> {
    let mut rows: Vec<Row> = activities_of(events, CALL_ROWS)
        .into_iter()
        .map(|activity| {
            let payload = &activity["payload"];
            Row {
                kind: text(&activity["kind"]),
                title: text(&payload["title"]),
                status: text(&payload["status"]),
                call_id: text(&payload["data"]["toolCallId"]),
                sequence: activity["sequence"].as_i64(),
            }
        })
        .collect();
    rows.sort_by_key(|row| row.sequence);
    rows
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

/// The whole ticket in one test, against a recording of a real tool call: the
/// invocation appears naming the tool and what it was given, the result appears
/// carrying the tool's answer, and the two are the pair the UI will collapse into
/// one row.
#[tokio::test]
async fn a_tool_call_appears_with_its_input_and_then_with_its_result() {
    let agent = ScriptedAgent::replaying("04-tool-use");
    let (server, events) = turn(&agent, "read note.txt").await;

    let calls = activities_of(&events, CALL_ROWS);
    assert_eq!(calls.len(), 2, "{:#?}", rows(&events));
    let (invoked, returned) = (calls[0], calls[1]);

    // The invocation names the tool and its input, and says it is under way.
    assert_eq!(invoked["kind"], "tool.updated");
    assert_eq!(invoked["tone"], "tool");
    assert_eq!(invoked["payload"]["status"], "inProgress");
    let detail = text(&invoked["payload"]["detail"]);
    assert!(detail.starts_with("Read: "), "{detail}");
    assert!(detail.contains("note.txt"), "{detail}");
    assert_eq!(invoked["payload"]["data"]["toolName"], "Read");
    assert!(
        text(&invoked["payload"]["data"]["input"]["file_path"]).ends_with("note.txt"),
        "{}",
        invoked["payload"]["data"]["input"]
    );

    // The result carries what the tool answered, and says the step worked.
    assert_eq!(returned["kind"], "tool.completed");
    assert_eq!(returned["payload"]["status"], "completed");
    let output = text(&returned["payload"]["detail"]);
    assert!(output.contains("the answer is 42"), "{output}");
    assert!(
        text(&returned["payload"]["data"]["result"]).contains("the answer is 42"),
        "the record of what the tool returned is missing"
    );

    // And the two are one row's worth of work: the same call id and the same
    // heading are what the client collapses them by.
    assert_eq!(
        invoked["payload"]["data"]["toolCallId"],
        returned["payload"]["data"]["toolCallId"]
    );
    assert!(
        invoked["payload"]["data"]["toolCallId"].is_string(),
        "an unpaired row is an invocation the result cannot be attached to"
    );
    assert_eq!(invoked["payload"]["title"], returned["payload"]["title"]);
    assert_eq!(invoked["turnId"], returned["turnId"]);
    assert!(invoked["turnId"].is_string(), "{invoked}");

    server.stop().await;
}

/// The criterion a developer needs most: whether the step worked, at a glance.
///
/// Against a recording of a real failure — a `Read` of a file that is not there —
/// because the CLI's own `is_error` is the whole mechanism and a hand-written
/// script would be this project asserting its own assumption about where that flag
/// lives.
#[tokio::test]
async fn a_failed_tool_call_says_it_failed() {
    let agent = ScriptedAgent::replaying("05-tool-failure");
    let (server, events) = turn(&agent, "read missing.txt").await;

    let calls = activities_of(&events, CALL_ROWS);
    assert_eq!(calls.len(), 2, "{:#?}", rows(&events));
    let returned = calls[1];

    assert_eq!(returned["kind"], "tool.completed");
    assert_eq!(
        returned["payload"]["status"], "failed",
        "the UI reads the status; without it a failure is inferred from prose"
    );
    assert!(
        text(&returned["payload"]["detail"]).contains("does not exist"),
        "{}",
        returned["payload"]["detail"]
    );

    // A step that failed, not a server that did. The two are styled differently,
    // and reporting this as a server error would misattribute the failure.
    assert_eq!(returned["tone"], "tool");

    // The turn itself went fine — the agent handled the failure and answered.
    assert_eq!(
        assistant_sends(&events).last().map(|(text, _)| text.clone()),
        Some("gone".to_string())
    );
    let ended = harness::conversation::last_session(&events, "the turn");
    assert_eq!(ended["payload"]["session"]["status"], "ready");

    server.stop().await;
}

/// Several calls in one turn, in order and correctly paired.
///
/// The pairing is adjacency in the UI, so the order matters as much as the ids:
/// this asserts the sequence is `A` invoked, `A` returned, `B` invoked, `B`
/// returned rather than both invocations followed by both results. That the CLI
/// interleaves them this way even for calls the model made in parallel is what
/// `06-several-tool-calls.ndjson` records.
#[tokio::test]
async fn several_tool_calls_in_one_turn_arrive_in_order_and_in_pairs() {
    let agent = ScriptedAgent::replaying("06-several-tool-calls");
    let (server, events) = turn(&agent, "read a.txt and b.txt").await;

    let rows = rows(&events);
    assert_eq!(rows.len(), 4, "{rows:#?}");

    let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec![
            "tool.updated",
            "tool.completed",
            "tool.updated",
            "tool.completed"
        ],
        "the calls are not adjacent pairs once ordered as the client orders them: {rows:#?}"
    );

    let (first, second) = (&rows[0].call_id, &rows[2].call_id);
    assert_eq!(
        &rows[1].call_id, first,
        "the first result answered the wrong call"
    );
    assert_eq!(
        &rows[3].call_id, second,
        "the second result answered the wrong call"
    );
    assert_ne!(first, second, "two calls shared one id: {rows:#?}");

    // Every row is numbered, and strictly increasing. Without this the client
    // falls back to `createdAt` — a millisecond — and then to a rank that puts
    // every `.updated` before every `.completed`, which would gather the two
    // invocations at the front and leave both showing as still running.
    let sequences: Vec<Option<i64>> = rows.iter().map(|row| row.sequence).collect();
    assert!(
        sequences.iter().all(Option::is_some),
        "an unnumbered row is one the client re-orders off a millisecond clock: {rows:#?}"
    );
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "the numbers do not order the rows: {sequences:?}"
    );

    // Each call's own output, on its own row — the pairing is only worth anything
    // if what it pairs is right.
    let outputs: Vec<String> = activities_of(&events, &["tool.completed"])
        .into_iter()
        .map(|activity| text(&activity["payload"]["detail"]))
        .collect();
    assert!(outputs[0].contains("alpha"), "{outputs:?}");
    assert!(outputs[1].contains("beta"), "{outputs:?}");

    assert_eq!(
        assistant_sends(&events).last().map(|(text, _)| text.clone()),
        Some("alpha-beta".to_string())
    );

    server.stop().await;
}

/// Thinking is its own kind of thing, not text in the reply.
///
/// Two halves, and the second is the one that used to be false: the reasoning is
/// published as a work-log row *and* is absent from the message the developer
/// reads. Before this ticket the buffered message flattened a thinking block to
/// the literal string `[thinking]`, which went into the chat bubble as prose.
#[tokio::test]
async fn thinking_is_published_as_thinking_and_not_as_the_reply() {
    let agent = ScriptedAgent::replaying("04-tool-use");
    let (server, events) = turn(&agent, "read note.txt").await;

    let thoughts = activities_of(&events, &["task.progress"]);
    assert!(!thoughts.is_empty(), "{:#?}", activities(&events));
    let first = thoughts[0];
    assert_eq!(
        first["kind"], "task.progress",
        "the UI renders only this kind with its thinking affordance"
    );
    assert_eq!(first["payload"]["summary"], "Thinking");
    // The reasoning is on the record and out of `detail`, which is the field the
    // client scans for failure-shaped prose — see `worklog::thinking`.
    assert!(
        text(&first["payload"]["thinking"]).contains("read"),
        "{}",
        first["payload"]
    );
    assert!(
        first["payload"].get("detail").is_none(),
        "{}",
        first["payload"]
    );

    // And none of it is in the conversation. The recording's reply is "42"; its
    // reasoning is several sentences about reading a file.
    let sends = assistant_sends(&events);
    assert_eq!(sends.last().map(|(text, _)| text.clone()), Some("42".to_string()));
    for (text, _) in &sends {
        assert!(
            !text.contains("[thinking]") && !text.contains("wants me to"),
            "the model's reasoning reached the transcript as the reply: {text}"
        );
    }

    // Nor is there an empty bubble where a thought or a tool call was: the CLI
    // buffers one message per block, and most of a tool turn's blocks are neither.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["text"], "42");

    server.stop().await;
}

/// A turn that says something, uses a tool, and then says something else. Both
/// kinds of thing, in the order they happened.
///
/// Purpose-built, because whether the model narrates before a tool call is the
/// model's choice and no recording can be made to contain one reliably. What is
/// being checked is this server's ordering, and the events carry it: the two
/// halves of the conversation and the tool between them, in the order the lines
/// arrived.
#[tokio::test]
async fn a_turn_mixing_text_and_tool_use_publishes_both_in_order() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Bash"]}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Let me look."}]}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_a","name":"Bash","input":{"command":"git status"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_a","content":"nothing to commit"}]}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The tree is clean."}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":90,"total_cost_usd":0.001}"#,
    ]);
    let (server, events) = turn(&agent, "is the tree clean").await;

    // Read the two kinds off one list, so what is asserted is their interleaving
    // rather than each in isolation.
    let happened: Vec<String> = events
        .iter()
        .map(|item| &item["event"])
        .filter_map(|event| match event["type"].as_str() {
            Some("thread.message-sent") if event["payload"]["role"] == "assistant" => {
                Some(format!("said {}", text(&event["payload"]["text"])))
            }
            Some("thread.activity-appended") => {
                let activity = &event["payload"]["activity"];
                let kind = text(&activity["kind"]);
                CALL_ROWS.contains(&kind.as_str())
                    .then(|| format!("{kind} {}", text(&activity["payload"]["detail"])))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        happened,
        vec![
            "said Let me look.".to_string(),
            "tool.updated Bash: git status".to_string(),
            "tool.completed nothing to commit".to_string(),
            "said The tree is clean.".to_string(),
        ],
        "the turn was not published in the order it happened"
    );

    // The transcript keeps them as two messages rather than one run-on reply: a
    // buffered message per block means each gets its own id.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert_eq!(messages[1]["text"], "Let me look.");
    assert_eq!(messages[2]["text"], "The tree is clean.");
    assert_ne!(messages[1]["id"], messages[2]["id"]);

    // A command execution's row shows the command itself, which is the field the
    // client looks for before it falls back to parsing the detail.
    let invoked = activities_of(&events, &["tool.updated"])[0];
    assert_eq!(invoked["payload"]["title"], "Command run");
    assert_eq!(invoked["payload"]["data"]["command"], "git status");

    server.stop().await;
}

/// A reply that streamed and then buffered nothing still gets closed.
///
/// The skip that keeps a tool-only message out of the transcript must not swallow
/// this: the client stops appending to a message when a non-streaming send
/// replaces it, so a reply left `streaming` would sit under a cursor for the life
/// of the thread. No recording contains an empty buffered message — the CLI's
/// buffered text is authoritative and arrives whole — which is why this is written
/// rather than replayed.
#[tokio::test]
async fn a_reply_that_streamed_and_buffered_nothing_is_still_settled() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"all there is"}}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":40,"total_cost_usd":0.001}"#,
    ]);
    let (server, events) = turn(&agent, "say something").await;

    assert_eq!(
        assistant_sends(&events),
        vec![
            ("all there is".to_string(), true),
            (String::new(), false),
        ],
        "the streamed message was never closed"
    );

    // The accumulation stands, because an empty buffered message has nothing
    // authoritative in it to replace with — the client's reducer makes the same
    // exception — and it is no longer streaming.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let messages = snapshot["thread"]["messages"].as_array().expect("messages");
    assert_eq!(messages[1]["text"], "all there is");
    assert_eq!(messages[1]["streaming"], json!(false));

    server.stop().await;
}

/// A result for a call this driver never saw the invocation of.
///
/// Unreachable against a healthy CLI: the buffered `assistant` message carrying a
/// `tool_use` always precedes the `user` message carrying its result. What it
/// guards is the alternative — dropping the result on the floor, which would leave
/// the developer looking at a tool that never came back. So the row appears, says
/// what the tool returned, and is honest about not knowing what the tool was.
#[tokio::test]
async fn a_result_for_an_unseen_call_is_still_shown() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Read"]}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_orphan","content":"it happened anyway","is_error":false}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":40,"total_cost_usd":0.001}"#,
    ]);
    let (server, events) = turn(&agent, "do the thing").await;

    let calls = activities_of(&events, CALL_ROWS);
    assert_eq!(calls.len(), 1, "{:#?}", rows(&events));
    let returned = calls[0];
    assert_eq!(returned["kind"], "tool.completed");
    assert_eq!(returned["payload"]["status"], "completed");
    assert_eq!(returned["payload"]["detail"], "it happened anyway");
    assert_eq!(
        returned["payload"]["data"]["toolCallId"], "toolu_orphan",
        "the id the agent used is still the row's key"
    );

    server.stop().await;
}

/// A tool given a great deal and returning a great deal. The row shows an
/// abbreviation and the transcript keeps the whole thing, because those are two
/// different jobs: one is a line in a log, the other is the record of what the
/// agent did to the developer's code.
///
/// Purpose-built for the size. Making a real capture large enough would mean
/// asking the model to read a large file, which is a slow and expensive way to
/// pin an argument about string lengths.
#[tokio::test]
async fn a_large_tool_input_and_output_are_shortened_for_the_row_but_kept_whole() {
    let command = "echo ".to_string() + &"argument ".repeat(200);
    let output = "line of output\n".repeat(500);
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Bash"]}"#,
        &format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_a","name":"Bash","input":{{"command":{}}}}}]}}}}"#,
            json!(command)
        ),
        &format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_a","content":{}}}]}}}}"#,
            json!(output)
        ),
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":90,"total_cost_usd":0.001}"#,
    ]);
    let (server, events) = turn(&agent, "run it").await;

    let calls = activities_of(&events, CALL_ROWS);
    assert_eq!(calls.len(), 2, "{:#?}", rows(&events));
    let (invoked, returned) = (calls[0], calls[1]);

    for shown in [&invoked["payload"]["detail"], &returned["payload"]["detail"]] {
        let shown = text(shown);
        assert!(
            shown.chars().count() <= 180 && shown.ends_with("..."),
            "a row carried {} characters: {shown:.80}",
            shown.chars().count()
        );
    }

    // The record, whole, on both halves of the pair. `data.input` is the record —
    // the block's input verbatim — and `data.command` beside it is for the row, so
    // it is the same command with the trailing space off.
    assert_eq!(invoked["payload"]["data"]["input"]["command"], json!(command));
    assert_eq!(
        invoked["payload"]["data"]["command"],
        json!(command.trim_end())
    );
    assert_eq!(returned["payload"]["data"]["result"], json!(output));

    // Including after a restart's worth of round trip through the snapshot a late
    // client is handed, which is where a truncation applied in the wrong place
    // would show up.
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    let stored = snapshot["thread"]["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .find(|activity| activity["kind"] == "tool.completed")
        .expect("the completed call is in the transcript");
    assert_eq!(stored["payload"]["data"]["result"], json!(output));

    server.stop().await;
}
