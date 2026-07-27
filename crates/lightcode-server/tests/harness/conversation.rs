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

/// A follow-up, which asks for no thread to be created because there already is
/// one.
pub fn follow_up(thread_id: &str, message_id: &str, text: &str) -> Value {
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
        "runtimeMode": "full-access",
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
    /// Register a project and open the thread subscription, as the UI does before
    /// the developer has typed anything.
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
    pub async fn open_conversation_in(
        &mut self,
        workspace: &Workspace,
        project_id: &str,
        thread_id: &str,
    ) -> String {
        self.call(
            "orchestration.dispatchCommand",
            create_project(project_id, workspace.path()),
        )
        .await
        .expect_success();

        self.watch_conversation(thread_id, true).await
    }

    /// Open the thread subscription for a conversation whose project is already
    /// registered — a second window, or the same window after a restart.
    ///
    /// `expect_draft` says which opening chunk this is: a conversation the server
    /// has never heard of describes itself as *nothing*, because an empty snapshot
    /// would be a positive claim that the conversation is empty and would wipe
    /// what the composer is optimistically showing. A restored one opens with its
    /// transcript.
    pub async fn watch_conversation(&mut self, thread_id: &str, expect_draft: bool) -> String {
        let subscription = self
            .subscribe(
                "orchestration.subscribeThread",
                json!({"threadId": thread_id, "requestCompletionMarker": true}),
            )
            .await;

        let opening = self.next_chunk(&subscription).await;
        self.ack(&subscription).await;
        match expect_draft {
            true => assert_eq!(
                opening,
                vec![json!({"kind": "synchronized"})],
                "a subscription to a draft must open without claiming the thread is empty"
            ),
            false => assert_eq!(
                opening.first().map(|item| item["kind"].clone()),
                Some(json!("snapshot")),
                "a conversation the server holds must open with it: {opening:#?}"
            ),
        }

        subscription
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
        self.values_until(subscription, |item| {
            let settled = |status: Option<&str>| {
                matches!(
                    status,
                    Some("ready") | Some("error") | Some("stopped") | Some("interrupted")
                )
            };
            match item["kind"].as_str() {
                Some("event") => {
                    item["event"]["type"] == "thread.session-set"
                        && settled(item["event"]["payload"]["session"]["status"].as_str())
                }
                Some("snapshot") => {
                    settled(item["snapshot"]["thread"]["session"]["status"].as_str())
                }
                _ => false,
            }
        })
        .await
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
