//! What the composer sends, and how to read a conversation back off the wire.
//!
//! Shared by `socket_turn.rs` and `socket_continuity.rs`, and shared for one
//! reason: these payloads are the real UI's, verbatim in shape from
//! `apps/web/src/components/ChatView.tsx`. Two copies would be two chances for a
//! test to keep passing against a command the composer no longer sends.
//!
//! Nothing here reaches into the server. A turn goes in as a dispatched command
//! and comes back out as the events a client would fold, which is the seam the
//! spec calls primary.

#![allow(dead_code)]

use std::path::Path;

use serde_json::{json, Value};

use super::workspace::Workspace;
use super::SocketClient;

/// The captured `project.create` payload with a folder the test made.
pub fn create_project(id: &str, folder: &Path) -> Value {
    json!({
        "type": "project.create",
        "commandId": format!("test:create:{id}"),
        "projectId": id,
        "title": "",
        "workspaceRoot": folder.to_string_lossy(),
        "createWorkspaceRootIfMissing": true,
        "defaultModelSelection": Value::Null,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.create` the client-runtime sends when a conversation is started
/// somewhere other than the composer's draft — and what these tests use to get a
/// thread to watch.
///
/// The fields are `bootstrap.createThread`'s, so a thread created this way and a
/// thread bootstrapped by a first turn are the same thread. See
/// [`SocketClient::open_conversation_in`] for why the tests create it up front
/// rather than watching a draft.
pub fn create_thread(project_id: &str, thread_id: &str) -> Value {
    create_thread_at(project_id, thread_id, None)
}

/// The same, for a conversation the developer pointed at a worktree.
///
/// `worktreePath` is one of the fields the composer sends on the thread it asks
/// for. It is set by picking a ref that is current in a worktree — something a
/// developer reaches with `git worktree add` and no help from this server — and
/// what the server then does with it is the whole of
/// `orchestration::where_the_work_happens`. `branch` stays null either way: the
/// server carries it and acts on nothing, so no test here has a use for it.
pub fn create_thread_at(project_id: &str, thread_id: &str, worktree: Option<&Path>) -> Value {
    json!({
        "type": "thread.create",
        "commandId": format!("test:thread:{thread_id}"),
        "threadId": thread_id,
        "projectId": project_id,
        "title": "A conversation",
        "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
        "runtimeMode": "full-access",
        "interactionMode": "default",
        "branch": Value::Null,
        "worktreePath": match worktree {
            Some(path) => json!(path.to_string_lossy()),
            None => Value::Null,
        },
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.turn.start` the composer sends for the first message of a new
/// conversation.
///
/// A new conversation is a **client-side draft**: the composer subscribes to a
/// thread the server has never heard of, and the thread only reaches the server
/// when the first turn is dispatched — carrying, under `bootstrap.createThread`,
/// the thread it wants created. A server that implemented only `thread.create`
/// would answer the real UI's first message with "there is no such thread".
pub fn start_turn(thread_id: &str, message_id: &str, text: &str) -> Value {
    start_turn_in(thread_id, message_id, text, "full-access")
}

/// The same, with the composer's runtime-mode picker set to something else.
///
/// Every other test wants `full-access` because it wants the turn to run without
/// being asked about. Ticket 13's want `approval-required`, which is the mode
/// whose whole meaning is that the agent asks.
pub fn start_turn_in(thread_id: &str, message_id: &str, text: &str, runtime_mode: &str) -> Value {
    turn_start("project-1", thread_id, message_id, text, runtime_mode)
}

/// The same, for a conversation belonging to a project other than the first.
///
/// A thread is scoped to a project — `CONTEXT.md`'s *Thread* — and the composer
/// says which one under `bootstrap.createThread`. Every test before ticket 16 had
/// one project and could leave it implied; two conversations in two projects
/// cannot, because which project a thread is in is what decides whose delete
/// takes it away.
pub fn start_turn_for(project_id: &str, thread_id: &str, message_id: &str, text: &str) -> Value {
    turn_start(project_id, thread_id, message_id, text, "full-access")
}

fn turn_start(
    project_id: &str,
    thread_id: &str,
    message_id: &str,
    text: &str,
    runtime_mode: &str,
) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": thread_id,
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": text,
            "attachments": [],
        },
        "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
        "titleSeed": "A conversation",
        "runtimeMode": runtime_mode,
        "interactionMode": "default",
        "bootstrap": {
            "createThread": {
                "projectId": project_id,
                "title": "A conversation",
                "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
                "runtimeMode": runtime_mode,
                "interactionMode": "default",
                "branch": Value::Null,
                "worktreePath": Value::Null,
                "createdAt": "2026-07-26T00:23:04.909Z",
            },
        },
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.checkpoint.revert` `revertThreadCheckpoint` builds.
///
/// `createdAt` is sent because the contract requires it of the client. This
/// server ignores it — see `orchestration::RevertCheckpointPayload` — and no
/// test reads it back.
pub fn revert_checkpoint(thread_id: &str, turn_count: u64) -> Value {
    json!({
        "type": "thread.checkpoint.revert",
        "commandId": format!("test:revert:{thread_id}:{turn_count}"),
        "threadId": thread_id,
        "turnCount": turn_count,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// A follow-up, which asks for no thread to be created because there already is
/// one.
pub fn follow_up(thread_id: &str, message_id: &str, text: &str) -> Value {
    follow_up_in(thread_id, message_id, text, "full-access")
}

pub fn follow_up_in(
    thread_id: &str,
    message_id: &str,
    text: &str,
    runtime_mode: &str,
) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": thread_id,
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": text,
            "attachments": [],
        },
        "runtimeMode": runtime_mode,
        "interactionMode": "default",
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.turn.interrupt` the composer's stop button sends.
///
/// `turn_id` is optional in the contract and the UI means something by leaving
/// it out: `buildThreadTurnInterruptInput` (`ChatView.logic.ts`) sends it only
/// while the session is `running`, so `None` is the client saying "stop whatever
/// is going, if anything is".
pub fn interrupt_turn(thread_id: &str, turn_id: Option<&str>) -> Value {
    let mut command = json!({
        "type": "thread.turn.interrupt",
        "commandId": format!("test:interrupt:{thread_id}"),
        "threadId": thread_id,
        "createdAt": "2026-07-26T00:23:04.909Z",
    });
    if let Some(turn_id) = turn_id {
        command["turnId"] = json!(turn_id);
    }
    command
}

/// The `thread.approval.respond` the composer's approval buttons send.
///
/// `decision` is one of `accept`, `acceptForSession`, `decline`, `cancel` —
/// the four in `ComposerPendingApprovalActions.tsx`, in the client's spelling.
pub fn respond_to_approval(thread_id: &str, request_id: &str, decision: &str) -> Value {
    json!({
        "type": "thread.approval.respond",
        "commandId": format!("test:approval:{request_id}"),
        "threadId": thread_id,
        "requestId": request_id,
        "decision": decision,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.user-input.respond` the composer's question header sends.
///
/// `answers` is keyed by the *question text*, which is the composer's `id` and
/// the key the CLI looks an answer up by — see `crate::worklog::questions`.
pub fn respond_to_user_input(thread_id: &str, request_id: &str, answers: Value) -> Value {
    json!({
        "type": "thread.user-input.respond",
        "commandId": format!("test:user-input:{request_id}"),
        "threadId": thread_id,
        "requestId": request_id,
        "answers": answers,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `type` of each event, in order.
pub fn kinds(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|item| item["event"]["type"].as_str().unwrap_or("<not an event>"))
        .collect()
}

/// Every `thread.message-sent` for the assistant, as (text, streaming).
pub fn assistant_sends(events: &[Value]) -> Vec<(String, bool)> {
    events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| {
            event["type"] == "thread.message-sent" && event["payload"]["role"] == "assistant"
        })
        .map(|event| {
            (
                event["payload"]["text"].as_str().unwrap_or("").to_string(),
                event["payload"]["streaming"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

/// Every activity, in the order it was published — the work log as the UI folds
/// one.
pub fn activities(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .collect()
}

/// Every activity of these kinds, in order. What a test asserting the *shape* of
/// a turn wants: the session's own bookkeeping is not the subject, so it is left
/// out rather than asserted around.
pub fn activities_of<'a>(events: &'a [Value], kinds: &[&str]) -> Vec<&'a Value> {
    activities(events)
        .into_iter()
        .filter(|activity| {
            activity["kind"]
                .as_str()
                .is_some_and(|kind| kinds.contains(&kind))
        })
        .collect()
}

/// The first activity of this kind.
pub fn activity<'a>(events: &'a [Value], kind: &str) -> &'a Value {
    find_activity(events, kind)
        .unwrap_or_else(|| panic!("no {kind} activity in {:?}", kinds(events)))
}

/// The first activity of this kind, if there is one.
pub fn find_activity<'a>(events: &'a [Value], kind: &str) -> Option<&'a Value> {
    events
        .iter()
        .map(|item| &item["event"])
        .find(|event| {
            event["type"] == "thread.activity-appended"
                && event["payload"]["activity"]["kind"] == kind
        })
}

/// The last `thread.session-set` in a run of events — how the session ended up.
pub fn last_session<'a>(events: &'a [Value], events_of: &str) -> &'a Value {
    events
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .unwrap_or_else(|| panic!("the session never said anything about {events_of}"))
}

impl SocketClient {
    /// Register a project, create the conversation, and open its subscription.
    pub async fn open_conversation(&mut self, workspace: &Workspace, thread_id: &str) -> String {
        self.open_conversation_in(workspace, "project-1", thread_id)
            .await
    }

    /// The same, naming the project the folder is registered as.
    ///
    /// Ticket 16's, and the reason it exists is that a second project has to be a
    /// second *folder*: `project.create` refuses a root another project already
    /// holds, so "two conversations in different projects" is two workspaces as
    /// well as two ids.
    ///
    /// ## Why the thread is created before it is watched
    ///
    /// This used to subscribe to a *draft* — an id the server had never heard of
    /// — because that is what the composer does, and then let the first turn's
    /// `bootstrap.createThread` bring the thread into being under a subscription
    /// that was already open. The server allowed it. **The real client cannot
    /// use it**: `client-runtime/state/threads.ts` folds an event only into a
    /// thread it already holds, and only a snapshot gives it one, so every event
    /// of that first turn was discarded and the window spun forever. That is
    /// ticket 28, and [`laplus_server::threads::Threads::subscribe`] now
    /// refuses a thread that does not exist, exactly as the reference server
    /// does.
    ///
    /// So a draft is no longer something a subscription can be *held open*
    /// across, here or anywhere. The client's answer is to retry every 250ms
    /// until the thread exists; these tests take the deterministic road instead
    /// and create the thread first, which leaves every event of the turn — the
    /// subject of nearly every test here — arriving live on the subscription as
    /// before. Only `thread.created` itself moves out of the stream and into the
    /// opening snapshot.
    ///
    /// What that road does not cover is the composer's own path, where the first
    /// turn creates the thread. `socket_turn.rs` keeps one test on it, driven the
    /// way the real client drives it.
    pub async fn open_conversation_in(
        &mut self,
        workspace: &Workspace,
        project_id: &str,
        thread_id: &str,
    ) -> String {
        self.open_conversation_at(workspace, project_id, thread_id, None)
            .await
    }

    /// The same, for a conversation whose work happens in a worktree of the
    /// project rather than in the project's own folder.
    ///
    /// The project is still registered at its own root — that is the whole point
    /// of the case: the developer registered a repository and then pointed one
    /// conversation at a checkout of it somewhere else.
    pub async fn open_conversation_at(
        &mut self,
        workspace: &Workspace,
        project_id: &str,
        thread_id: &str,
        worktree: Option<&Path>,
    ) -> String {
        self.call(
            "orchestration.dispatchCommand",
            create_project(project_id, workspace.path()),
        )
        .await
        .expect_success();
        self.call(
            "orchestration.dispatchCommand",
            create_thread_at(project_id, thread_id, worktree),
        )
        .await
        .expect_success();

        self.watch_conversation(thread_id).await
    }

    /// Open the thread subscription for a conversation the server holds — a
    /// second window, or the same window after a restart.
    ///
    /// It opens with the conversation, always. A subscription that opened with
    /// anything else would be one the real client renders nothing from.
    pub async fn watch_conversation(&mut self, thread_id: &str) -> String {
        let subscription = self
            .subscribe(
                "orchestration.subscribeThread",
                json!({"threadId": thread_id, "requestCompletionMarker": true}),
            )
            .await;

        let opening = self.next_chunk(&subscription).await;
        self.ack(&subscription).await;
        assert_eq!(
            opening.first().map(|item| item["kind"].clone()),
            Some(json!("snapshot")),
            "a conversation the server holds must open with it: {opening:#?}"
        );

        subscription
    }

    /// Watch a conversation the way the composer does: keep asking until the
    /// thread exists.
    ///
    /// The client's own loop, at the client's own cadence
    /// (`subscribeDynamic`'s `retryExpectedFailureAfter: "250 millis"`), made
    /// faster because nothing here is waiting on a person. Used by the tests
    /// that drive the draft path end to end.
    pub async fn watch_draft(&mut self, thread_id: &str) -> String {
        for _ in 0..200 {
            let request = self
                .send_request(
                    "orchestration.subscribeThread",
                    json!({"threadId": thread_id, "requestCompletionMarker": true}),
                )
                .await;
            match self.first_chunk_or_failure(&request).await {
                Some(opening) => {
                    self.ack(&request).await;
                    assert_eq!(
                        opening.first().map(|item| item["kind"].clone()),
                        Some(json!("snapshot")),
                        "the retry that finds the thread opens with it: {opening:#?}"
                    );
                    return request;
                }
                None => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        panic!("the thread {thread_id} never came into existence");
    }

    /// The opening chunk, or `None` if the subscription was refused instead.
    ///
    /// A refusal ends the call, so the frame is the terminal `Exit` rather than
    /// a `Chunk` — which is how the client tells "not yet" from "here it is".
    async fn first_chunk_or_failure(&mut self, request_id: &str) -> Option<Vec<Value>> {
        let frame = self.next_frame_for(request_id).await;
        if frame["_tag"] != json!("Chunk") {
            assert_eq!(
                frame["exit"]["cause"][0]["error"]["_tag"],
                json!("OrchestrationGetSnapshotError"),
                "a thread that does not exist yet is refused by name: {frame}"
            );
            return None;
        }
        Some(
            frame["values"]
                .as_array()
                .unwrap_or_else(|| panic!("a chunk's values are an array: {frame}"))
                .clone(),
        )
    }

    /// Read the turn out of a thread subscription, up to and including the session
    /// going quiet again.
    ///
    /// A *snapshot* saying the session has gone quiet ends the read too, and that
    /// is not belt-and-braces: a turn that publishes hundreds of events outruns the
    /// subscription's backlog, and the pump answers that by discarding what it
    /// could not deliver and describing the world again. The terminal event is then
    /// one of the things discarded — so a reader that only watched for the event
    /// would wait for one that had already been superseded.
    pub async fn events_through_the_turn(&mut self, subscription: &str) -> Vec<Value> {
        let watching = settle_watch(true);
        self.values_until(subscription, move |item| watching(item)).await
    }

    /// The same, for a reader that is **not** already inside a turn.
    ///
    /// [`settle_watch`] says why the two differ: a conversation whose delegation
    /// tree outlives its root turn publishes two more sessions of its own, and a
    /// reader standing between turns would otherwise take the first of them for
    /// the settle it was waiting for.
    pub async fn events_through_the_next_turn(&mut self, subscription: &str) -> Vec<Value> {
        let watching = settle_watch(false);
        self.values_until(subscription, move |item| watching(item)).await
    }

    /// Read the turn out up to and including the moment its working tree has
    /// been recorded.
    ///
    /// One event later than [`SocketClient::events_through_the_turn`], and the
    /// gap between them is real rather than an artefact: the turn settles when
    /// the agent says it is done, and the checkpoint is written after that —
    /// off the driver's loop, because it is a `git add -A` over the project. A
    /// test that asked for a diff on the settle would be racing that.
    ///
    /// A *snapshot* carrying the checkpoint ends the read too, for the reason
    /// the settle's own reader gives: a turn that outruns the subscription's
    /// backlog is answered by describing the world again, and the event would
    /// then be one of the things discarded.
    pub async fn events_through_the_checkpoint(
        &mut self,
        subscription: &str,
        turn_count: u64,
    ) -> Vec<Value> {
        self.values_until(subscription, |item| match item["kind"].as_str() {
            Some("event") => {
                item["event"]["type"] == "thread.turn-diff-completed"
                    && item["event"]["payload"]["checkpointTurnCount"] == json!(turn_count)
            }
            Some("snapshot") => item["snapshot"]["thread"]["checkpoints"]
                .as_array()
                .is_some_and(|checkpoints| {
                    checkpoints
                        .iter()
                        .any(|checkpoint| checkpoint["checkpointTurnCount"] == json!(turn_count))
                }),
            _ => false,
        })
        .await
    }

    /// Read the turn out up to and including the moment the working tree has
    /// been put back.
    ///
    /// One event later than the *answer* to the revert, and the gap between them
    /// is the whole shape of that command: the dispatch records that a revert was
    /// asked for, and the restore happens off the read loop because it touches a
    /// disk. A test that looked at files on the answer would be racing a `git`.
    ///
    /// A *snapshot* does not end this read, unlike the checkpoint's own reader:
    /// the server leaves the conversation alone across a revert, so there is
    /// nothing in a snapshot that would say one had happened.
    pub async fn events_through_the_revert(
        &mut self,
        subscription: &str,
        turn_count: u64,
    ) -> Vec<Value> {
        self.values_until(subscription, |item| {
            item["kind"] == json!("event")
                && item["event"]["type"] == json!("thread.reverted")
                && item["event"]["payload"]["turnCount"] == json!(turn_count)
        })
        .await
    }

    /// The diff of one turn, as the panel asks for it: `max(0, n - 1)` to `n`.
    ///
    /// The echoed fields are asserted here rather than by each caller, because
    /// they are the same three every time and a panel that got them back wrong
    /// would render one turn's patch under another turn's heading.
    pub async fn turn_diff(&mut self, thread_id: &str, turn: u64) -> String {
        let answered = self
            .call(
                "orchestration.getTurnDiff",
                json!({
                    "threadId": thread_id,
                    "fromTurnCount": turn.saturating_sub(1),
                    "toTurnCount": turn,
                    "ignoreWhitespace": false,
                }),
            )
            .await
            .expect_success();
        assert_eq!(answered["threadId"], thread_id);
        assert_eq!(answered["fromTurnCount"], json!(turn.saturating_sub(1)));
        assert_eq!(answered["toTurnCount"], json!(turn));
        answered["diff"]
            .as_str()
            .unwrap_or_else(|| panic!("a diff is a string: {answered}"))
            .to_string()
    }

    /// Read the turn out up to and including the agent asking for permission,
    /// and hand back the id the answer has to name.
    ///
    /// The turn does not settle here — that is the whole point of a permission
    /// request — so [`SocketClient::events_through_the_turn`] would wait for an
    /// ending that only arrives once somebody has decided.
    pub async fn events_until_permission(&mut self, subscription: &str) -> (Vec<Value>, String) {
        let events = self
            .values_until(subscription, |item| {
                item["event"]["payload"]["activity"]["kind"] == "approval.requested"
            })
            .await;

        let request_id = activity(&events, "approval.requested")["payload"]["activity"]["payload"]
            ["requestId"]
            .as_str()
            .expect("a request the client can answer names an id")
            .to_string();
        (events, request_id)
    }

    /// Read the turn out up to and including the agent asking a *question*, and
    /// hand back the id the answers have to name.
    ///
    /// [`SocketClient::events_until_permission`]'s twin, and separate for the
    /// reason the two folds are separate in the client: a question that arrived
    /// as an `approval.requested` is the bug these tests exist to catch, so a
    /// reader that accepted either would pass on it.
    pub async fn events_until_user_input(&mut self, subscription: &str) -> (Vec<Value>, String) {
        let events = self
            .values_until(subscription, |item| {
                item["event"]["payload"]["activity"]["kind"] == "user-input.requested"
            })
            .await;

        let request_id = activity(&events, "user-input.requested")["payload"]["activity"]
            ["payload"]["requestId"]
            .as_str()
            .expect("a question the client can answer names an id")
            .to_string();
        (events, request_id)
    }

    /// Read the turn out up to and including the agent having said something.
    ///
    /// What a test that means to interrupt *mid-turn* needs: pressing stop before
    /// the agent has streamed anything would be a test of stopping a turn that
    /// had not started, and the partial reply is the thing the ticket asks to be
    /// kept — so there has to be one.
    pub async fn events_until_streaming(&mut self, subscription: &str) -> Vec<Value> {
        self.values_until(subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
                && item["event"]["payload"]["streaming"] == json!(true)
        })
        .await
    }

    /// Subscribe to a thread and take the snapshot it opens with.
    ///
    /// What a second window, or a client that arrived late, or the first window
    /// after a restart is handed. Used to check that the transcript the server
    /// holds is the one a client that watched every event would have folded — if
    /// the two ever differ, which conversation a developer sees depends on when
    /// they opened it.
    pub async fn into_thread_snapshot(mut self, thread_id: &str) -> Value {
        let subscription = self
            .subscribe(
                "orchestration.subscribeThread",
                json!({"threadId": thread_id}),
            )
            .await;
        let opening = self.next_chunk(&subscription).await;
        let snapshot = opening
            .into_iter()
            .find(|item| item["kind"] == "snapshot")
            .unwrap_or_else(|| panic!("no snapshot for {thread_id}"));
        self.close().await;
        snapshot["snapshot"].clone()
    }

    /// The project list as it opens, which is where a restored conversation has to
    /// appear for the developer to be able to click on it.
    pub async fn into_shell_snapshot(mut self) -> Value {
        let subscription = self
            .subscribe("orchestration.subscribeShell", json!({}))
            .await;
        let opening = self.next_chunk(&subscription).await;
        let snapshot = opening
            .into_iter()
            .find(|item| item["kind"] == "snapshot")
            .expect("the shell describes itself");
        self.close().await;
        snapshot["snapshot"].clone()
    }
}

/// "This item is the end of a turn", as a predicate with a memory.
///
/// A turn settles when the session leaves `running` **having been running on a
/// turn**, and the second half of that is load-bearing rather than pedantry: a
/// conversation whose delegation tree is still working goes on reporting
/// `running` after its root turn has settled, and returns to `ready` when the
/// last descendant finishes (`Threads::follow_delegation`). Neither of those two
/// events names a turn, so a reader that took any quiet status for a settle
/// would stop a test one turn short of what it came to watch.
///
/// `on_a_turn_already` is what a caller knows and this cannot see: a reader that
/// dispatched a turn and only then subscribed has already missed the session
/// that named it, while a reader standing between turns has not.
///
/// A *snapshot* saying the session has gone quiet ends the read too, and that is
/// not belt-and-braces: a turn that publishes hundreds of events outruns the
/// subscription's backlog, and the pump answers that by discarding what it could
/// not deliver and describing the world again. The terminal event is then one of
/// the things discarded — so a reader that only watched for the event would wait
/// for one that had already been superseded.
pub fn settle_watch(on_a_turn_already: bool) -> impl Fn(&Value) -> bool {
    let on_a_turn = std::cell::Cell::new(on_a_turn_already);
    move |item: &Value| {
        let settled = |status: Option<&str>| {
            matches!(
                status,
                Some("ready") | Some("error") | Some("stopped") | Some("interrupted")
            )
        };
        match item["kind"].as_str() {
            Some("event") if item["event"]["type"] == "thread.session-set" => {
                let session = &item["event"]["payload"]["session"];
                if session["activeTurnId"].is_string() {
                    on_a_turn.set(true);
                }
                settled(session["status"].as_str()) && on_a_turn.replace(false)
            }
            Some("snapshot") => {
                let session = &item["snapshot"]["thread"]["session"];
                if session["activeTurnId"].is_string() {
                    on_a_turn.set(true);
                }
                settled(session["status"].as_str())
            }
            _ => false,
        }
    }
}
