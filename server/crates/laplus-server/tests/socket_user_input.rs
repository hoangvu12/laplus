//! The agent asking the developer a question, driven the way the UI drives it.
//!
//! `AskUserQuestion` reaches this server as a permission request and reaches the
//! developer as neither an allow nor a deny: it is a multiple-choice question,
//! and the client has had a composer header for one since long before this fork
//! (`derivePendingUserInputs`, `apps/web/src/pendingUserInput.ts`). What was
//! missing was a server that said so. Answered "Allow" it ran with no answers;
//! answered "Deny" the agent was told its question was refused.
//!
//! ## Why the scripts here are written rather than recorded
//!
//! Every other permission capture in `fixtures/claude-cli/` is real `claude`
//! output, and the reasoning in `socket_permissions.rs` for insisting on that
//! holds: a round trip asserted against a hand-written script is this project
//! agreeing with itself. It cannot be had here. A question is asked when the
//! *model* decides to ask one, so there is no prompt that reliably produces one
//! to record — the same problem `16`–`18` have, and the same answer: write the
//! script, and take the shapes from somewhere that is not a guess.
//!
//! Both halves come from one:
//!
//! - **The envelope** is `07-permission-approved.ndjson`'s own `control_request`,
//!   with the tool and its input swapped. That file is a recording, and its
//!   `init` line lists `AskUserQuestion` among the tools — so a request naming
//!   that tool is a thing the recorded CLI could have sent down the recorded
//!   channel.
//! - **The input** is the shape upstream parses (`ClaudeAdapter.ts`,
//!   `handleAskUserQuestion`): `questions[]` of `question`, `header`,
//!   `multiSelect` and `options[]` of `label` and `description`. Upstream reads
//!   it off the real SDK, so it is a recording at one remove.
//!
//! What that leaves unproven is stated rather than papered over: **what the real
//! CLI does with the answers this server sends**. The assertions below are about
//! the answer laplus writes to stdin, not about the tool result the model
//! eventually reads. `answers_for`'s doc carries the same warning at the source.

mod harness;

use harness::agent::{ScriptedAgent, AWAIT_ANSWER, DIES};
use harness::conversation::{
    activities_of, activity, find_activity, respond_to_approval, respond_to_user_input, start_turn,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// A session that has said hello, listing the tool that asks questions — which
/// the recorded `init` in `07-permission-approved.ndjson` does too.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"default","tools":["Read","AskUserQuestion"]}"#;

/// The question, as the agent asks it. One of each kind of field the client
/// requires, and a second option because a question with one choice is not one.
const ASKS: &str = r#"{"type":"control_request","request_id":"req-question-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database should this use?","header":"Database","multiSelect":false,"options":[{"label":"Postgres","description":"Relational, and the one the team already runs."},{"label":"SQLite","description":"One file, and nothing to operate."}]}]},"tool_use_id":"toolu_question_1"}}"#;

/// The same tool asking something this build cannot render as a question: the
/// options are gone, and the client drops a question that has none.
const ASKS_UNREADABLY: &str = r#"{"type":"control_request","request_id":"req-question-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database should this use?","header":"Database"}]},"tool_use_id":"toolu_question_1"}}"#;

/// What the agent does once it has been answered.
const CARRIES_ON: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Postgres it is."}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":90,"total_cost_usd":0.001}"#;

/// The two rows a permission request can become. Asserting that one of them is
/// absent is worth a message saying which were present.
const BOTH_KINDS: &[&str] = &["approval.requested", "user-input.requested"];

/// The developer's answer, keyed the way the composer keys it: by the question's
/// own text, which is what `crate::worklog::questions` sets as the `id` and what
/// the CLI looks an answer up by.
fn chose_postgres() -> Value {
    json!({ "Which database should this use?": "Postgres" })
}

/// A conversation stopped at a question, with everything still open.
struct Questioned {
    server: TestServer,
    client: SocketClient,
    subscription: String,
    request_id: String,
    events: Vec<Value>,
    _workspace: Workspace,
}

/// Open a conversation, send a turn, and read up to the agent asking.
async fn question(agent: &ScriptedAgent) -> Questioned {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "pick a database"),
        )
        .await
        .expect_success();

    let (events, request_id) = client.events_until_user_input(&subscription).await;
    Questioned {
        server,
        client,
        subscription,
        request_id,
        events,
        _workspace: workspace,
    }
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

/// The decision inside a `control_response`, which is the part the CLI acts on.
fn decision(answer: &Value) -> &Value {
    &answer["response"]["response"]
}

/// The first criterion: a question reaches the developer as a question.
///
/// Every field asserted here is one `parseUserInputQuestions`
/// (`apps/web/src/session-logic.ts`) requires by name, and it discards the whole
/// activity if any is missing or malformed — which would leave the composer with
/// nothing to show beside an agent that has stopped. So these are not a
/// description of the payload; they are the payload's reason for existing.
#[tokio::test]
async fn a_question_reaches_the_developer_as_a_question() {
    let agent = ScriptedAgent::emitting(&[INIT, ASKS, AWAIT_ANSWER, CARRIES_ON, DONE]);
    let questioned = question(&agent).await;

    let asked = activity(&questioned.events, "user-input.requested");
    let payload = &asked["payload"]["activity"]["payload"];
    assert_eq!(payload["requestId"], questioned.request_id);

    let questions = payload["questions"]
        .as_array()
        .unwrap_or_else(|| panic!("a question carries questions: {payload}"));
    assert_eq!(questions.len(), 1, "{payload}");
    let question = &questions[0];
    assert_eq!(question["question"], "Which database should this use?");
    assert_eq!(question["header"], "Database");
    assert_eq!(question["multiSelect"], json!(false));
    assert_eq!(
        question["options"],
        json!([
            {"label": "Postgres", "description": "Relational, and the one the team already runs."},
            {"label": "SQLite", "description": "One file, and nothing to operate."},
        ])
    );

    // The id is the question's own text. It looks redundant beside `question`
    // and it is the thing that makes an answer findable: the composer keys its
    // draft by `id`, and the CLI looks the answer up by question text.
    assert_eq!(question["id"], question["question"]);

    // And it is *not* also a permission request. Both rows would be two panels
    // over one agent, and answering the wrong one sends the wrong shape.
    assert!(
        find_activity(&questioned.events, "approval.requested").is_none(),
        "a question was also published as a permission request: {:?}",
        activities_of(&questioned.events, BOTH_KINDS),
    );

    // Nothing has been answered, so nothing has been sent.
    assert!(agent.answers().is_empty(), "{:?}", agent.answers());

    // The thread list raises its hand, so a conversation waiting on the
    // developer is findable from another one.
    let shell = questioned
        .server
        .connect()
        .await
        .into_shell_snapshot()
        .await;
    assert_eq!(
        shell["threads"][0]["hasPendingUserInput"],
        json!(true),
        "{}",
        shell["threads"][0]
    );

    questioned.client.close().await;
    questioned.server.stop().await;
}

/// Answering it sends the answers to the agent and lets the turn finish.
///
/// The half this ticket is really about. An `allow` whose `updatedInput` carries
/// the questions *and* the answers is the only shape the CLI turns into a tool
/// result; an `allow` carrying the request's own input — which is what every
/// other approval in this server sends, correctly — is the agent being told to
/// go ahead and ask, again.
#[tokio::test]
async fn answering_a_question_sends_the_answers_and_the_turn_finishes() {
    let agent = ScriptedAgent::emitting(&[INIT, ASKS, AWAIT_ANSWER, CARRIES_ON, DONE]);
    let mut questioned = question(&agent).await;

    questioned
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_user_input("thread-1", &questioned.request_id, chose_postgres()),
        )
        .await
        .expect_success();
    let rest = questioned
        .client
        .events_through_the_turn(&questioned.subscription)
        .await;

    // What the agent was told.
    let written = answers(&agent);
    assert_eq!(written.len(), 1, "{written:#?}");
    assert_eq!(written[0]["response"]["request_id"], questioned.request_id);
    let sent = decision(&written[0]);
    assert_eq!(sent["behavior"], "allow");
    assert_eq!(sent["updatedInput"]["answers"], chose_postgres());
    // The questions travel back beside the answers, because that is the shape
    // the CLI reads. Verbatim: these are the agent's own, handed back.
    assert_eq!(
        sent["updatedInput"]["questions"][0]["question"],
        "Which database should this use?"
    );
    assert_eq!(
        sent["updatedInput"]["questions"][0]["options"][0]["label"],
        "Postgres"
    );

    // What the developer sees. The resolution names the request, which is what
    // closes the composer's header.
    let resolved = activity(&rest, "user-input.resolved")["payload"]["activity"].clone();
    assert_eq!(resolved["payload"]["requestId"], questioned.request_id);
    assert_eq!(resolved["payload"]["answers"], chose_postgres());

    // And the turn carries on and ends normally.
    let session = harness::conversation::last_session(&rest, "the answered question");
    assert_eq!(session["payload"]["session"]["status"], "ready");

    let shell = questioned
        .server
        .connect()
        .await
        .into_shell_snapshot()
        .await;
    assert_eq!(
        shell["threads"][0]["hasPendingUserInput"],
        json!(false),
        "the question is answered and the thread is still raising its hand: {}",
        shell["threads"][0]
    );

    questioned.client.close().await;
    questioned.server.stop().await;
}

/// A question this build cannot read is still a permission the developer can
/// answer.
///
/// The fallback, and the reason `crate::worklog::questions` returns an `Option`
/// rather than doing its best. The client drops a question payload it cannot
/// parse — silently — so a server that published one anyway would leave an agent
/// stopped for an answer, a developer with no question on screen, and no
/// approval row either. Rendering it as what it is on the wire is worse UI and a
/// working conversation.
#[tokio::test]
async fn a_question_this_build_cannot_read_is_still_answerable() {
    let agent = ScriptedAgent::emitting(&[INIT, ASKS_UNREADABLY, AWAIT_ANSWER, CARRIES_ON, DONE]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;
    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "pick a database"),
        )
        .await
        .expect_success();

    let (events, request_id) = client.events_until_permission(&subscription).await;
    let asked = activity(&events, "approval.requested")["payload"]["activity"].clone();
    assert_eq!(asked["payload"]["data"]["toolName"], "AskUserQuestion");
    assert!(
        find_activity(&events, "user-input.requested").is_none(),
        "an unparseable question was published as one anyway: {:?}",
        activities_of(&events, BOTH_KINDS),
    );

    // And it answers like any other permission, which is the whole point of
    // falling back to one.
    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &request_id, "accept"),
        )
        .await
        .expect_success();
    let rest = client.events_through_the_turn(&subscription).await;
    assert!(find_activity(&rest, "approval.resolved").is_some());

    client.close().await;
    server.stop().await;
}

/// A session that ends holding a question closes it.
///
/// The stuck-composer case, and it has its own test because the two folds are
/// separate: the loop that closes leftovers publishes `approval.resolved`, which
/// closes an approval panel and does *nothing* to a question header. Left that
/// way the composer is unusable for the life of the conversation and across
/// every restart after it, because the header is folded out of stored
/// activities.
#[tokio::test]
async fn a_session_that_ends_holding_a_question_closes_it() {
    // The agent asks and then stops being a process, which is how a session ends
    // while the developer is still reading the question. Running the script out
    // would not do it: a scripted agent that reaches the end of a turn waits for
    // the next one, exactly as the real CLI does.
    let agent = ScriptedAgent::emitting(&[INIT, ASKS, DIES]);
    let questioned = question(&agent).await;
    let mut client = questioned.client;

    let rest = client.events_through_the_turn(&questioned.subscription).await;
    let closed = activity(&rest, "provider.user-input.respond.failed")["payload"]["activity"]
        .clone();
    assert_eq!(closed["payload"]["requestId"], questioned.request_id);
    // The wording is the mechanism: `isStalePendingRequestFailureDetail` matches
    // this phrase by substring, and a reworded sentence would make the header
    // permanent instead of clearing it.
    assert!(
        closed["payload"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("Unknown pending permission request"),
        "{}",
        closed["payload"]["detail"]
    );

    client.close().await;
    questioned.server.stop().await;
}
