//! Threads: a conversation with the agent, as the UI reads one.
//!
//! [`crate::orchestration`] is the wire half of the *project* registry and this
//! is the wire half of the conversation, sharing that module's two mechanisms —
//! a command writes and a subscription reads, joined by a sequence number.
//! Threads are the second aggregate in the contract's orchestration model and
//! the one the product exists for.
//!
//! ```text
//! C>S Request  orchestration.subscribeThread  {"threadId":"…"}
//! S>C Chunk    {"kind":"snapshot","snapshot":{"snapshotSequence":4,"thread":{…}}}
//! C>S Request  orchestration.dispatchCommand  {"type":"thread.turn.start",…}
//! S>C Exit     Success {"sequence":5}
//! S>C Chunk    {"kind":"event","event":{"sequence":6,"type":"thread.message-sent",…}}
//! S>C Chunk    {"kind":"event","event":{"sequence":7,"type":"thread.message-sent",…}}
//! ```
//!
//! ## Streaming is two kinds of `thread.message-sent`, and the client knows which
//!
//! The contract has no separate "delta" event. What it has is a `streaming` flag
//! on `thread.message-sent`, and the client's reducer reads it as the whole
//! difference (`threadReducer.ts`, `case "thread.message-sent"`):
//!
//! - `streaming: true` — **append** `text` to the message already under this id.
//! - `streaming: false` — **replace** it, unless `text` is empty.
//!
//! That is the accumulate-and-reconcile rule the spike settled, expressed in the
//! vocabulary the UI already speaks. A token delta is a streaming send carrying
//! the delta; the buffered `assistant` message the CLI produces at the end of the
//! turn is a non-streaming send carrying the whole text, and it replaces whatever
//! the deltas built — which is exactly what has to happen when a delta was shed.
//! Nothing about this is a lightcode invention; the client was already written
//! for it.
//!
//! The server keeps its own copy of each message and folds it the same way, so
//! that a client which arrives mid-turn and takes a snapshot sees what a client
//! that watched every event sees.
//!
//! ## Persistence is a projection, not a second model
//!
//! Ticket 11 made conversations durable, and the shape it chose is: this module
//! stays the live one and [`crate::transcripts`] mirrors it to disk behind the
//! stream. Two things follow, and both are decisions rather than details.
//!
//! **A durable write is a [`ThreadRow`], not a [`Thread`].** The row is
//! everything about a conversation except what is in it. A change to a title or
//! a latest turn must not cost a clone of every message the thread holds, or a
//! long conversation would pay for its own length on every change to it.
//!
//! **Deltas are not persisted.** A token delta is superseded by the buffered
//! message a moment later, and the buffered message is the authoritative one —
//! the same rule that governs the transcript governs the table. So a write
//! happens at a *message boundary*, not per token, which is what keeps the disk
//! out of the streaming path entirely. See [`crate::transcripts`] for what that
//! costs, which is the tail of a reply the app was killed in the middle of.
//!
//! ## What is deliberately not here
//!
//! - **A live session, stored.** [`Session`] describes a running process and
//!   [`LatestTurn`] describes a turn in flight; neither is true after a restart.
//!   The row carries the latest turn — a restored conversation showing how its
//!   last turn went is worth having — and [`Thread::restored`] moves a turn that
//!   was still `running` to `interrupted`, because the app stopped in the middle
//!   of it and nothing is going to finish it.
//! - **Proposed plans.** `proposedPlans` is present and empty; a later ticket
//!   fills it. `activities` holds a turn's own bookkeeping and, since ticket 12,
//!   its tool calls and the reasoning between them — see [`crate::worklog`],
//!   which owns what those rows look like, and `checkpoints` holds the turns
//!   that can be reviewed as a diff — see [`crate::checkpoints`], which owns
//!   what is behind one.
//! - **Worktrees.** `branch` and `worktreePath` are carried and never acted on.
//!   A `thread.turn.start` that asks for a worktree to be prepared is refused by
//!   name rather than silently run in the project root — see
//!   [`crate::orchestration`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::clock::now_iso;
use crate::settling::SessionStatus;
use crate::store::Sequences;
use crate::subscriptions::{EventSource, BACKLOG};
use crate::transcripts::{Transcripts, Write};

/// The subscription that *is* one conversation.
pub const SUBSCRIBE_THREAD: &str = "orchestration.subscribeThread";

/// How many user turns may be waiting for an agent that has not read them yet.
///
/// A person types one prompt at a time and the agent is normally already
/// listening; this is only the window between a turn being dispatched and the
/// child existing. Bounded rather than unbounded so a session whose agent never
/// started cannot absorb prompts forever.
const PROMPT_QUEUE: usize = 8;

/// How many signals — decisions and interrupts — may be waiting for a driver
/// that has not read them yet.
///
/// A separate channel from the prompts rather than a second kind of message on
/// one, and that is the whole reason it exists: a turn is queued *behind* the
/// turn in flight, and a signal is owed *to* the turn in flight. Sharing a
/// channel would put the signal behind a prompt the driver is deliberately not
/// reading yet, which is a conversation waiting on a decision that has already
/// been made — or an agent still running after the developer pressed stop.
const SIGNAL_QUEUE: usize = 8;

// ---------------------------------------------------------------------------
// What a thread is
// ---------------------------------------------------------------------------

/// One conversation, as `OrchestrationThread` in the contract.
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// `{instanceId, model}` — what the agent is asked for with `--model`, and
    /// what the UI shows in the composer's picker.
    pub model_selection: Value,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub messages: Vec<Message>,
    pub activities: Vec<Activity>,
    /// One per turn that has finished, in the order the turns happened. What
    /// makes a turn a point in time the working tree can be diffed against —
    /// see [`crate::checkpoints`].
    pub checkpoints: Vec<Checkpoint>,
    pub session: Option<Session>,
    pub latest_turn: Option<LatestTurn>,
    /// When the developer last said something. On the shell summary rather than
    /// derived by the client, so the thread list can sort without the messages.
    pub latest_user_message_at: Option<String>,
    /// The `claude` session this conversation is being held in, as the agent
    /// itself reported it on its `init` line.
    ///
    /// The one field here that is neither in the contract nor derived from it.
    /// It is what `--resume` is given, and it is therefore the whole of how a
    /// conversation survives a restart: the context is in the agent's own store,
    /// not in this server's transcript, and this id is the handle on it. `None`
    /// until an agent has announced itself for this thread.
    pub agent_session_id: Option<String>,
}

/// A conversation's own row: everything about a thread except what is in it.
///
/// The unit of a durable write, and the reason [`crate::transcripts`] can keep
/// up with a conversation of any length — see this module's documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub model_selection: Value,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub agent_session_id: Option<String>,
    pub latest_turn: Option<LatestTurn>,
    pub latest_user_message_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A thread as the database gives it back: its row, and everything in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub thread: ThreadRow,
    pub messages: Vec<Message>,
    pub activities: Vec<Activity>,
    pub checkpoints: Vec<Checkpoint>,
}

/// What the working tree looked like when one turn finished, as
/// `OrchestrationCheckpointSummary`.
///
/// The row itself carries no diff. It is a *name* — the ref
/// [`crate::checkpoints`] wrote the tree under, and the turn count that names
/// the range a diff runs over — plus enough of a summary for the panel to list
/// the turn before anyone asks for its patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub turn_id: String,
    /// How many turns this conversation had recorded once this one finished.
    /// Turn one's checkpoint is 1, and the baseline taken before it is 0 — so a
    /// turn's diff runs from `turn_count - 1` to `turn_count`.
    pub turn_count: u64,
    /// Where the tree is, in the project's own repository.
    pub reference: String,
    /// **How the turn went**, not whether the capture worked — `ready` for one
    /// that finished, `error` for one that failed.
    ///
    /// That reading is the client's, and it is not a nuance: the reducer sets
    /// `latestTurn.state` from this on every checkpoint it folds
    /// (`threadReducer.ts`, `checkpointStatusToTurnState`). So a status that
    /// disagreed with how the turn actually ended would relabel the turn, and
    /// this server and the client would then be showing two different
    /// conversations. [`crate::turn::Ending::checkpoint_status`] is where the
    /// mapping lives, and where the third case — a turn the developer stopped,
    /// which gets no checkpoint at all — is argued.
    ///
    /// The contract's `missing` is therefore never sent. It means `completed`
    /// to the client, so the only turn it could describe is one this server
    /// would rather not describe at all.
    pub status: &'static str,
    /// What the turn changed, for the row the panel shows before the developer
    /// opens the patch. Empty when the turn changed nothing, and empty when the
    /// summary could not be read — the patch is authoritative either way.
    ///
    /// Counted **without** ignoring whitespace, while the patch a developer
    /// opens ignores it by default. The two can therefore disagree on a
    /// reformatting turn: a file listed as `+40 −40` whose patch is empty. That
    /// is upstream's behaviour and it is unavoidable rather than chosen — the
    /// summary is computed once when the turn ends and the flag is chosen per
    /// request, so there is no single count that could be right for both.
    pub files: Vec<crate::checkpoints::Changed>,
    pub assistant_message_id: Option<String>,
    pub completed_at: String,
}

impl Checkpoint {
    fn to_value(&self) -> Value {
        json!({
            "turnId": self.turn_id,
            "checkpointTurnCount": self.turn_count,
            "checkpointRef": self.reference,
            "status": self.status,
            "files": self.files
                .iter()
                .map(crate::checkpoints::Changed::to_value)
                .collect::<Vec<Value>>(),
            "assistantMessageId": self.assistant_message_id,
            "completedAt": self.completed_at,
        })
    }
}

/// A stored tone, back as one of the three this server produces.
///
/// [`Activity::tone`] is a `&'static str` because every value it can hold is a
/// literal in this file — `info` from [`Activity::info`], `error` from
/// [`Activity::failed`] and `tool` from [`Activity::tool`]. So the round trip is
/// lossless for everything that can be written, and anything else is a row this
/// build did not put there.
pub fn tone(stored: &str) -> &'static str {
    match stored {
        "error" => "error",
        "tool" => "tool",
        _ => "info",
    }
}

/// A stored checkpoint status, back as one of the contract's three.
///
/// Same reasoning as [`tone`]: every value this build can write is a literal in
/// this file, so the round trip is lossless for anything it put there and
/// anything else is a row it did not. A status it cannot read is `error`, which
/// is the one of the three that promises the developer nothing.
pub fn checkpoint_status(stored: &str) -> &'static str {
    match stored {
        "ready" => "ready",
        "missing" => "missing",
        _ => "error",
    }
}

/// A stored turn state, back as one of the contract's four.
///
/// Same reasoning as [`tone`]. Lives in [`crate::settling`] now, because the
/// four states are half of one vocabulary and reading a stored one is the same
/// question as reading a session's.
pub use crate::settling::TurnState;

/// One message in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub text: String,
    pub turn_id: Option<String>,
    /// True while more text is expected under this id.
    pub streaming: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Something worth showing that is not a message. The UI's work log is built
/// from these (`session-logic.ts`, `deriveWorkLogEntries`), which renders any
/// kind it does not specifically suppress.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub id: String,
    pub tone: &'static str,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub turn_id: Option<String>,
    /// Where this row sits in the work log, from the same counter that numbers
    /// the event announcing it.
    ///
    /// `None` until [`Threads::apply`] takes a number — an activity is built
    /// before it is published, and there is nothing honest to put here until then.
    ///
    /// It is what the client *sorts the work log by* when it is present
    /// (`compareActivitiesByOrder`), and the whole reason it has to be present is
    /// that the fallback is `createdAt`, which is a millisecond. Two rows inside
    /// one millisecond fall through to a rank that puts every `.updated` before
    /// every `.completed` — so a turn whose tool calls landed close together would
    /// have its invocations gathered at the front, away from the results they pair
    /// with, and the work log collapses a pair only when the two are *adjacent*.
    /// The same argument [`crate::store`] makes for storing a message's `ordinal`
    /// rather than trusting its timestamp.
    pub sequence: Option<i64>,
    pub created_at: String,
}

/// The agent process behind a thread, as the client sees it.
#[derive(Debug, Clone)]
pub struct Session {
    pub status: SessionStatus,
    pub runtime_mode: String,
    pub active_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

/// The most recent turn and how far it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestTurn {
    pub turn_id: String,
    pub state: TurnState,
    pub requested_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub assistant_message_id: Option<String>,
}

impl Thread {
    /// The `OrchestrationThread` a `subscribeThread` snapshot carries.
    ///
    /// Built by hand rather than derived, for the same reason
    /// [`crate::projects::Project::to_value`] is: a third of it is constants the
    /// contract requires and a `Serialize` impl would hide which.
    pub fn to_detail_value(&self) -> Value {
        json!({
            "id": self.id,
            "projectId": self.project_id,
            "title": self.title,
            "modelSelection": self.model_selection,
            "runtimeMode": self.runtime_mode,
            "interactionMode": self.interaction_mode,
            "branch": self.branch,
            "worktreePath": self.worktree_path,
            "latestTurn": self.latest_turn.as_ref().map(LatestTurn::to_value),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "archivedAt": Value::Null,
            "settledOverride": Value::Null,
            "settledAt": Value::Null,
            "deletedAt": Value::Null,
            "messages": self.messages.iter().map(Message::to_value).collect::<Vec<Value>>(),
            "proposedPlans": [],
            "activities": self.activities.iter().map(Activity::to_value).collect::<Vec<Value>>(),
            "checkpoints": self
                .checkpoints
                .iter()
                .map(Checkpoint::to_value)
                .collect::<Vec<Value>>(),
            "session": self.session.as_ref().map(|session| session.to_value(&self.id)),
        })
    }

    /// The `OrchestrationThreadShell` the project list carries — the same thread
    /// without its transcript, plus the three flags the inbox sorts on.
    ///
    /// The first is now real: a thread the agent has asked permission on raises
    /// its hand in the thread list, which is what makes a conversation waiting on
    /// the developer findable from another one. The other two stay `false` and
    /// each is a later ticket's — the user-input questions an `AskUserQuestion`
    /// raises, and a proposed plan, which needs `ExitPlanMode` answered rather
    /// than merely reported. A `true` neither could be acted on would put a badge
    /// on a thread with nothing behind it.
    pub fn to_shell_value(&self) -> Value {
        json!({
            "id": self.id,
            "projectId": self.project_id,
            "title": self.title,
            "modelSelection": self.model_selection,
            "runtimeMode": self.runtime_mode,
            "interactionMode": self.interaction_mode,
            "branch": self.branch,
            "worktreePath": self.worktree_path,
            "latestTurn": self.latest_turn.as_ref().map(LatestTurn::to_value),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "archivedAt": Value::Null,
            "settledOverride": Value::Null,
            "settledAt": Value::Null,
            "session": self.session.as_ref().map(|session| session.to_value(&self.id)),
            "latestUserMessageAt": self.latest_user_message_at,
            // Derived from the work log rather than counted beside it, because
            // the client derives its own panel from the same rows — a counter
            // kept here would be a second answer to one question, and the two
            // would agree until they did not. Linear in the work log, and only
            // for the shell summary, which a delta and an activity both skip
            // ([`Change::reaches_the_shell`]).
            "hasPendingApprovals": !crate::worklog::unanswered(&self.activities).is_empty(),
            "hasPendingUserInput": false,
            "hasActionableProposedPlan": false,
        })
    }

    /// The model slug to start the agent with, if the selection names one.
    pub fn model(&self) -> Option<String> {
        self.model_selection
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// This conversation without its transcript — what a durable write carries.
    pub fn row(&self) -> ThreadRow {
        ThreadRow {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            title: self.title.clone(),
            model_selection: self.model_selection.clone(),
            runtime_mode: self.runtime_mode.clone(),
            interaction_mode: self.interaction_mode.clone(),
            branch: self.branch.clone(),
            worktree_path: self.worktree_path.clone(),
            agent_session_id: self.agent_session_id.clone(),
            latest_turn: self.latest_turn.clone(),
            latest_user_message_at: self.latest_user_message_at.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    /// A conversation as it comes back from a restart.
    ///
    /// Two things are deliberately not what was stored:
    ///
    /// - **There is no session.** A session is a running process, and after a
    ///   restart there is none. The first turn on this thread starts one, with
    ///   `--resume` pointed at [`Thread::agent_session_id`].
    /// - **A turn that was still `running` becomes `interrupted`.** The app
    ///   stopped in the middle of it and nothing is going to finish it, so
    ///   leaving it `running` would show a conversation working forever. The
    ///   turn's `completedAt` is the last moment the thread is known to have
    ///   changed, which is the closest true answer available.
    pub fn restored(stored: Conversation) -> Thread {
        let row = stored.thread;
        let latest_turn = row.latest_turn.map(|turn| match turn.state {
            TurnState::Running => LatestTurn {
                state: TurnState::Interrupted,
                completed_at: Some(row.updated_at.clone()),
                ..turn
            },
            _ => turn,
        });

        Thread {
            id: row.id,
            project_id: row.project_id,
            title: row.title,
            model_selection: row.model_selection,
            runtime_mode: row.runtime_mode,
            interaction_mode: row.interaction_mode,
            branch: row.branch,
            worktree_path: row.worktree_path,
            created_at: row.created_at,
            updated_at: row.updated_at,
            messages: stored.messages,
            activities: stored.activities,
            // Kept, unlike the session, and for the opposite reason: a
            // checkpoint is a ref in the developer's repository, so it is still
            // there after a restart and the diff it names still opens. A
            // restored conversation the developer cannot review would be a
            // conversation they have to re-run to see.
            checkpoints: stored.checkpoints,
            session: None,
            latest_turn,
            latest_user_message_at: row.latest_user_message_at,
            agent_session_id: row.agent_session_id,
        }
    }
}

impl Message {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "role": self.role,
            "text": self.text,
            "turnId": self.turn_id,
            "streaming": self.streaming,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

impl Activity {
    /// Something that happened and is worth showing.
    pub fn info(
        kind: &str,
        summary: &str,
        payload: Value,
        turn_id: Option<String>,
    ) -> Activity {
        Activity {
            id: fresh_activity_id(),
            tone: "info",
            kind: kind.to_string(),
            summary: summary.to_string(),
            payload,
            turn_id,
            sequence: None,
            created_at: now_iso(),
        }
    }

    /// A step the agent took, rather than something the server has to report.
    ///
    /// `tool` is the contract's own tone for this and the UI reads it as one:
    /// a row with the tool's affordances, and — the part that matters for a call
    /// that *failed* — styled as a step that did not work rather than as an error
    /// in this server (`session-logic.ts`, `showDestructiveRowStyle`). Which of
    /// the two it was is in the payload's `status`; see [`crate::worklog`].
    pub fn tool(kind: &str, summary: &str, payload: Value, turn_id: Option<String>) -> Activity {
        Activity {
            tone: "tool",
            ..Activity::info(kind, summary, payload, turn_id)
        }
    }

    /// The agent asking to be allowed to do something, or the answer to one.
    ///
    /// `approval` is one of the contract's four tones and the only one this
    /// server does not otherwise use. The UI renders the *row* as an ordinary
    /// info row (`session-logic.ts`, `toDerivedWorkLogEntry`), so the tone earns
    /// its place elsewhere: it is what says a row is about a decision rather than
    /// about work, which is what the tone vocabulary is for and what a later
    /// reader of the transcript needs.
    ///
    /// The panel itself is driven by the `kind` and by `payload.requestId` — see
    /// [`crate::worklog`], where both are built.
    pub fn approval(kind: &str, summary: &str, payload: Value, turn_id: Option<String>) -> Activity {
        Activity {
            tone: "approval",
            ..Activity::info(kind, summary, payload, turn_id)
        }
    }

    /// Something that went wrong, said in the conversation rather than only to a
    /// log — the developer is looking at the conversation.
    pub fn failed(kind: &str, summary: &str) -> Activity {
        Activity {
            id: fresh_activity_id(),
            tone: "error",
            kind: kind.to_string(),
            summary: summary.to_string(),
            // Repeated under `detail` because that is the key the UI's work log
            // reads a body out of (`session-logic.ts`, `extractToolDetail`);
            // `summary` alone renders as a heading with nothing under it.
            payload: json!({"detail": summary}),
            turn_id: None,
            sequence: None,
            created_at: now_iso(),
        }
    }

    fn to_value(&self) -> Value {
        let mut activity = json!({
            "id": self.id,
            "tone": self.tone,
            "kind": self.kind,
            "summary": self.summary,
            "payload": self.payload,
            "turnId": self.turn_id,
            "createdAt": self.created_at,
        });
        // `Schema.optional`, not `Schema.NullOr` — so an absent sequence is an
        // absent *key*. A `null` would fail the client's decode of the whole
        // activity, and the ordering rule beside it tests for `!== undefined`.
        if let Some(sequence) = self.sequence {
            activity["sequence"] = json!(sequence);
        }
        activity
    }
}

impl Session {
    /// `threadId` is a field of the session in the contract and is the key the
    /// client re-attaches it by, so it comes from the thread rather than being
    /// stored twice.
    fn to_value(&self, thread_id: &str) -> Value {
        json!({
            "threadId": thread_id,
            "status": self.status.as_str(),
            // The driver slug, which is what upstream puts here and what the UI
            // renders beside the session state.
            "providerName": crate::provider::INSTANCE_ID,
            "providerInstanceId": crate::provider::INSTANCE_ID,
            "runtimeMode": self.runtime_mode,
            "activeTurnId": self.active_turn_id,
            "lastError": self.last_error,
            "updatedAt": self.updated_at,
        })
    }
}

impl LatestTurn {
    /// The contract's `OrchestrationLatestTurn`. Also the stored form — see the
    /// `threads` table in [`crate::store`], which keeps this shape verbatim
    /// rather than spreading it over six columns nothing queries.
    pub fn to_value(&self) -> Value {
        json!({
            "turnId": self.turn_id,
            "state": self.state.as_str(),
            "requestedAt": self.requested_at,
            "startedAt": self.started_at,
            "completedAt": self.completed_at,
            "assistantMessageId": self.assistant_message_id,
        })
    }
}

// ---------------------------------------------------------------------------
// What can happen to one
// ---------------------------------------------------------------------------

/// Everything that changes a thread after it exists.
///
/// A closed vocabulary rather than a setter per field, for the reason
/// [`crate::config_store::ConfigChange`] is one: a change has to update the
/// stored thread *and* describe itself to subscribers, and the two must not be
/// possible to do inconsistently. Each member is one `OrchestrationEvent` type.
#[derive(Debug, Clone)]
pub enum Change {
    /// The developer's prompt, in the transcript. `thread.message-sent`.
    UserMessage {
        message_id: String,
        text: String,
        turn_id: String,
    },
    /// A turn was asked for. `thread.turn-start-requested`.
    ///
    /// The three optional fields are what the composer had selected when the
    /// developer pressed enter, and they are per-turn in the contract: a model
    /// picked mid-conversation arrives here rather than as a separate edit. Each
    /// is applied to the thread when present and left alone when absent, because
    /// absent means "unchanged" and defaulting one would silently move a
    /// conversation back to `full-access` every turn.
    TurnRequested {
        turn_id: String,
        message_id: String,
        model_selection: Option<Value>,
        runtime_mode: Option<String>,
        interaction_mode: Option<String>,
    },
    /// The developer stopped a turn. `thread.turn-interrupt-requested`.
    ///
    /// Published when the agent has been *asked* to stop rather than when it
    /// has, and that is what the event is for: the client's reducer moves the
    /// latest turn to `interrupted` on it immediately (`threadReducer.ts`,
    /// `case "thread.turn-interrupt-requested"`), so the developer's own click
    /// settles the turn instead of a round trip to the agent doing it. The
    /// session follows a moment later, when the agent says it has stopped.
    ///
    /// The turn is named because the reducer requires it — an event without one
    /// is folded as `unchanged`, and so is one naming a turn that is not the
    /// latest.
    InterruptRequested { turn_id: String },
    /// Live assistant text. `thread.message-sent` with `streaming: true`, which
    /// the client appends.
    AssistantDelta {
        message_id: String,
        turn_id: String,
        text: String,
    },
    /// The buffered assistant message, which is authoritative.
    /// `thread.message-sent` with `streaming: false`, which the client replaces
    /// with.
    AssistantMessage {
        message_id: String,
        turn_id: String,
        text: String,
    },
    /// The agent process changed state. `thread.session-set`.
    Session(Session),
    /// Something happened worth showing. `thread.activity-appended`.
    Activity(Activity),
    /// A turn finished and what the working tree looked like was recorded.
    /// `thread.turn-diff-completed`.
    ///
    /// The event the client's reducer appends to `thread.checkpoints`, which is
    /// the list the diff panel offers turns from. Published *after* the tree has
    /// actually been written, never before: a row naming a ref that is not
    /// there is a turn the developer can select and cannot open.
    Checkpointed(Box<Checkpoint>),
}

impl Change {
    /// Does the project list need to hear about this?
    ///
    /// Everything except a delta and an activity. A turn produces hundreds of
    /// deltas and none of them changes anything the thread *list* renders — the
    /// title, the session state, the latest turn — so republishing the summary
    /// per token would be the shell subscription carrying a token stream it has
    /// no use for.
    ///
    /// A checkpoint is in the same position for a different reason: the shell
    /// summary does not carry `checkpoints` at all, so nothing on the list would
    /// read one.
    fn reaches_the_shell(&self) -> bool {
        !matches!(
            self,
            Change::AssistantDelta { .. } | Change::Activity(_) | Change::Checkpointed(_)
        )
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Every thread this server knows, and the agents running behind them.
///
/// Cheap to clone and every clone is the same registry, like
/// [`crate::config_store::ConfigStore`] and [`crate::orchestration::Shell`]: a
/// subscription outlives the call that opened it, and so does the task driving a
/// turn.
#[derive(Debug, Clone)]
pub struct Threads {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    open: Mutex<HashMap<String, Arc<Entry>>>,
    sequences: Sequences,
    /// The project list's feed. A thread that changes publishes its summary here
    /// as well as its event on its own feed — the two subscriptions are read by
    /// different parts of the UI and neither can be derived from the other.
    shell: broadcast::Sender<Value>,
    /// Where a change goes to be written down. Queueing only — see
    /// [`crate::transcripts`], whose whole purpose is that publishing a change
    /// and storing it are never the same wait.
    transcripts: Transcripts,
    /// Drivers whose thread has been forgotten and which are still winding down.
    ///
    /// [`Threads::forget`] cannot wait for them — a project delete answers the
    /// client immediately — but it must not *drop* them either: dropping a
    /// `JoinHandle` detaches the task, and a detached driver is one
    /// [`Threads::shutdown`] would not wait for. That is the single leak this
    /// process can produce that outlives the process, so the handle is parked here
    /// instead and shutdown waits for it with the rest.
    winding_down: Mutex<Vec<JoinHandle<()>>>,
    live_agents: AtomicUsize,
    reconciled_messages: AtomicUsize,
    messages_matching_deltas: AtomicUsize,
}

/// One thread's slot: what it is, who is watching, and what is running.
#[derive(Debug)]
struct Entry {
    /// `None` until the thread is created.
    ///
    /// A slot exists before the thread does because the UI subscribes first: a
    /// new conversation is a client-side draft, and the server hears about it
    /// only when the first turn is dispatched with a `bootstrap.createThread`.
    /// A subscription opened against a draft therefore has to be a subscription
    /// to something that is not there yet, rather than a refusal.
    state: Mutex<Option<Thread>>,
    events: broadcast::Sender<Value>,
    /// The running agent's end of the conversation, while there is one.
    live: Mutex<Option<Live>>,
}

/// A handle on the task driving one session.
#[derive(Debug)]
struct Live {
    prompts: mpsc::Sender<Prompt>,
    signals: mpsc::Sender<Signal>,
    task: JoinHandle<()>,
}

/// The continuous check on the assumption that makes streaming safe.
///
/// `reconciled` counts the buffered assistant messages that landed after deltas
/// had already been published — every completed streamed turn. `agreed` counts
/// how many of those the deltas had built exactly.
///
/// The two travel together because either alone means nothing. It is a *ratio*,
/// not an error count: a turn that used a tool, or thought before answering,
/// legitimately ends with deltas that do not equal the buffered text, because
/// the buffered message flattens blocks the deltas never carried. What would be
/// alarming is `agreed` going to zero on plain turns, which is the case the
/// suite drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    pub reconciled: usize,
    pub agreed: usize,
}

/// One user turn on its way to the agent.
///
/// The turn's own identity travels with the text because the driver needs it
/// for everything it publishes afterwards, and it is minted by the dispatch that
/// answered the client — which is what makes the sequence the client was given
/// and the events that follow describe the same turn.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub turn_id: String,
    pub text: String,
}

/// Something owed to the turn the agent is working on right now.
///
/// One channel for both kinds rather than one each, and the ordering is the
/// reason: a developer who approves a tool and then immediately presses stop
/// means those two things *in that order*, and two channels would leave a
/// `select!` free to take them in either. One channel is also one place where
/// the rule "a signal is never queued behind a prompt" has to be got right.
#[derive(Debug, Clone)]
pub enum Signal {
    /// The developer answered a permission request.
    Answer(Answered),
    /// The developer stopped the agent.
    ///
    /// The turn is carried because the client names the one it is looking at,
    /// and a moment is all it takes for that to stop being the one in flight —
    /// the turn it asked about may have finished while the click was travelling.
    /// `None` is the client saying "whatever is running", which is what it sends
    /// when it does not believe anything is.
    Interrupt { turn_id: Option<String> },
}

/// One permission decision on its way to the agent waiting for it.
///
/// The id rather than the request: the driver holds what was asked, because it
/// is the only thing that saw the question, and the client answers by naming the
/// id it was shown.
#[derive(Debug, Clone)]
pub struct Answered {
    pub request_id: String,
    pub decision: crate::worklog::Decision,
}

impl Threads {
    pub fn new(
        sequences: Sequences,
        shell: broadcast::Sender<Value>,
        transcripts: Transcripts,
    ) -> Threads {
        Threads {
            inner: Arc::new(Inner {
                open: Mutex::new(HashMap::new()),
                sequences,
                shell,
                transcripts,
                winding_down: Mutex::new(Vec::new()),
                live_agents: AtomicUsize::new(0),
                reconciled_messages: AtomicUsize::new(0),
                messages_matching_deltas: AtomicUsize::new(0),
            }),
        }
    }

    /// Put back the conversations the last run left behind.
    ///
    /// Silent on purpose: nothing is announced and no sequence is taken. These
    /// are not changes — they are the world as the first client will find it, and
    /// an event for each would number a restart as though a hundred
    /// conversations had just been created.
    ///
    /// Anything already open under the same id is left alone. Nothing calls this
    /// twice today; refusing to overwrite is what makes that harmless rather than
    /// a way to lose a live conversation.
    pub fn restore(&self, stored: Vec<Conversation>) {
        for conversation in stored {
            let entry = self.entry(&conversation.thread.id);
            let mut state = lock(&entry.state);
            if state.is_none() {
                *state = Some(Thread::restored(conversation));
            }
        }
    }

    /// Agent processes currently running. The gauge that makes "the subprocess
    /// is terminated and reaped when the session ends" observable from outside,
    /// without a test reaching into the registry to look — the same accounting
    /// [`crate::server::ServerState`] keeps for connections, subscriptions and
    /// watched workspaces.
    pub fn live_agents(&self) -> usize {
        self.inner.live_agents.load(Ordering::Relaxed)
    }

    /// How often the buffered message and the deltas before it agreed.
    pub fn reconciliation(&self) -> Reconciliation {
        Reconciliation {
            reconciled: self.inner.reconciled_messages.load(Ordering::Relaxed),
            agreed: self.inner.messages_matching_deltas.load(Ordering::Relaxed),
        }
    }

    /// The slot for this thread, making one if the thread has not been created
    /// yet.
    ///
    /// Only for callers that are about to *put something in it* — a subscription
    /// that will need somewhere to hear from, or a turn that is about to create
    /// the thread. A question about a thread uses [`Threads::find`], which does
    /// not: a query that quietly allocated would let any id a client mentions
    /// leak a slot.
    ///
    /// What is left unreaped is the slot for a draft that was subscribed to and
    /// never sent, which is a few hundred bytes per abandoned draft for the life
    /// of the process.
    fn entry(&self, thread_id: &str) -> Arc<Entry> {
        let mut open = self.lock();
        Arc::clone(open.entry(thread_id.to_string()).or_insert_with(|| {
            Arc::new(Entry {
                state: Mutex::new(None),
                events: broadcast::channel(BACKLOG).0,
                live: Mutex::new(None),
            })
        }))
    }

    /// The slot for this thread, if there is one. Asks rather than allocates.
    fn find(&self, thread_id: &str) -> Option<Arc<Entry>> {
        self.lock().get(thread_id).map(Arc::clone)
    }

    /// Bring a thread into being. Refuses an id that already names one, which is
    /// the only way this can fail.
    pub fn create(&self, thread: Thread) -> Result<i64, String> {
        let entry = self.entry(&thread.id);
        let mut state = lock(&entry.state);
        if let Some(existing) = state.as_ref() {
            return Err(format!(
                "Thread '{}' already exists in project '{}'.",
                existing.id, existing.project_id
            ));
        }

        // Held until both announcements are out — see [`Sequences::commit`]. The
        // project list folds threads and projects against one cursor and drops
        // anything at or below what it holds, so an event that published out of
        // numeric order would be dropped rather than reordered.
        let commit = self.inner.sequences.commit();
        let sequence = commit.sequence();
        let event = created_event(sequence, &thread);
        let summary = thread.to_shell_value();
        self.inner.transcripts.queue(Write::Thread(Box::new(thread.row())));
        *state = Some(thread);
        drop(state);

        let _ = entry.events.send(event);
        let _ = self.inner.shell.send(json!({
            "kind": "thread-upserted",
            "sequence": sequence,
            "thread": summary,
        }));
        Ok(sequence)
    }

    /// Fold one change in and tell everyone watching.
    ///
    /// `None` when the thread does not exist — a change published for a thread
    /// nothing has created would describe a conversation no client could open.
    pub fn apply(&self, thread_id: &str, change: Change) -> Option<i64> {
        let entry = self.find(thread_id)?;
        let mut state = lock(&entry.state);
        let thread = state.as_mut()?;

        // Under the entry's lock, so two changes to one thread are numbered in
        // the order they are applied; and holding the log until both
        // announcements are out is what keeps them published in that order. A
        // client drops anything at or below the sequence it holds, so a pair
        // that inverted would lose the earlier one permanently.
        let commit = self.inner.sequences.commit();
        let sequence = commit.sequence();
        let occurred_at = now_iso();
        let payload = self.fold(thread, &change, sequence, &occurred_at);
        thread.updated_at = occurred_at.clone();

        let event = thread_event(sequence, thread_id, change.event_type(), payload, &occurred_at);
        let summary = change.reaches_the_shell().then(|| thread.to_shell_value());
        // Under the same lock as the fold, so what is written down is what was
        // just folded in and not whatever a later change left behind.
        for write in durable(thread, &change) {
            self.inner.transcripts.queue(write);
        }
        drop(state);

        // `send` on a broadcast channel never blocks — it drops the oldest value
        // when the buffer is full and a lagging subscriber is resent a snapshot
        // instead — so publishing here cannot stall the caller.
        let _ = entry.events.send(event);
        if let Some(summary) = summary {
            let _ = self.inner.shell.send(json!({
                "kind": "thread-upserted",
                "sequence": sequence,
                "thread": summary,
            }));
        }
        Some(sequence)
    }

    /// Apply one change to the stored thread and return the event payload that
    /// describes it.
    ///
    /// The fold mirrors `threadReducer.ts` deliberately and closely. A server
    /// whose stored thread disagreed with the client's fold would show one
    /// transcript to a client that watched the whole turn and a different one to
    /// a client that arrived late and took a snapshot.
    ///
    /// `sequence` is the number the change is being announced under. Only an
    /// activity keeps it — see [`Activity::sequence`] — and it is passed in rather
    /// than read here because [`Threads::apply`] holds the log open around both.
    fn fold(&self, thread: &mut Thread, change: &Change, sequence: i64, at: &str) -> Value {
        match change {
            Change::UserMessage {
                message_id,
                text,
                turn_id,
            } => {
                thread.latest_user_message_at = Some(at.to_string());
                self.message_sent(thread, message_id, "user", text, Some(turn_id), false, at)
            }
            Change::TurnRequested {
                turn_id,
                message_id,
                model_selection,
                runtime_mode,
                interaction_mode,
            } => {
                if let Some(selection) = model_selection {
                    thread.model_selection = selection.clone();
                }
                if let Some(mode) = runtime_mode {
                    thread.runtime_mode = mode.clone();
                }
                if let Some(mode) = interaction_mode {
                    thread.interaction_mode = mode.clone();
                }
                thread.latest_turn = Some(LatestTurn {
                    turn_id: turn_id.clone(),
                    state: TurnState::Running,
                    requested_at: at.to_string(),
                    started_at: None,
                    completed_at: None,
                    assistant_message_id: None,
                });
                json!({
                    "threadId": thread.id,
                    "messageId": message_id,
                    "modelSelection": thread.model_selection,
                    "runtimeMode": thread.runtime_mode,
                    "interactionMode": thread.interaction_mode,
                    "createdAt": at,
                })
            }
            // The client's reducer, mirrored: the latest turn moves to
            // `interrupted` and keeps whatever `completedAt` it already had, and
            // an event naming some other turn changes nothing. The mirroring is
            // the same rule the rest of this fold follows — a client that watched
            // every event and one that arrives late and takes a snapshot have to
            // see the same conversation.
            Change::InterruptRequested { turn_id } => {
                if let Some(latest) = thread
                    .latest_turn
                    .as_mut()
                    .filter(|latest| &latest.turn_id == turn_id)
                {
                    latest.state = TurnState::Interrupted;
                    latest.started_at.get_or_insert_with(|| at.to_string());
                    latest.completed_at.get_or_insert_with(|| at.to_string());
                }
                json!({
                    "threadId": thread.id,
                    "turnId": turn_id,
                    "createdAt": at,
                })
            }
            Change::AssistantDelta {
                message_id,
                turn_id,
                text,
            } => self.message_sent(
                thread,
                message_id,
                "assistant",
                text,
                Some(turn_id),
                true,
                at,
            ),
            Change::AssistantMessage {
                message_id,
                turn_id,
                text,
            } => self.message_sent(
                thread,
                message_id,
                "assistant",
                text,
                Some(turn_id),
                false,
                at,
            ),
            Change::Session(session) => {
                thread.latest_turn = settle(thread.latest_turn.take(), session);
                thread.session = Some(session.clone());
                json!({
                    "threadId": thread.id,
                    "session": session.to_value(&thread.id),
                })
            }
            Change::Activity(activity) => {
                // Numbered as it is folded in, so the row the client sorts by
                // sequence and the row a late client finds in the snapshot are the
                // same row. Set here rather than at construction because a caller
                // building an activity has no number to give it.
                let appended = Activity {
                    sequence: Some(sequence),
                    ..activity.clone()
                };
                let described = appended.to_value();
                thread.activities.push(appended);
                json!({
                    "threadId": thread.id,
                    "activity": described,
                })
            }
            // Keyed by the **turn**, which is the client's own key
            // (`threadReducer.ts`, `case "thread.turn-diff-completed"`, which
            // filters on `entry.turnId !== checkpoint.turnId`). Keying on
            // anything else would let a second capture of one turn be one row
            // here and two in the panel, which is the kind of disagreement a
            // client that took a snapshot and a client that watched every event
            // would show differently.
            Change::Checkpointed(recorded) => {
                // Filled in here rather than by the driver, because this is
                // where the transcript is: the reply a turn is remembered by is
                // its last assistant message, and the driver has already
                // forgotten which that was — it clears the id as each message
                // completes, so that a turn which said several things gives each
                // its own. Upstream's reactor resolves it the same way and from
                // the same end of the list.
                let mut captured = (**recorded).clone();
                if captured.assistant_message_id.is_none() {
                    captured.assistant_message_id = thread
                        .messages
                        .iter()
                        .rev()
                        .find(|message| {
                            message.role == "assistant"
                                && message.turn_id.as_deref() == Some(captured.turn_id.as_str())
                        })
                        .map(|message| message.id.clone());
                }
                match thread
                    .checkpoints
                    .iter_mut()
                    .find(|held| held.turn_id == captured.turn_id)
                {
                    Some(held) => *held = captured.clone(),
                    None => thread.checkpoints.push(captured.clone()),
                }
                let mut payload = captured.to_value();
                payload["threadId"] = json!(thread.id);
                payload
            }
        }
    }

    /// Append or replace a message, and move the latest turn with it.
    ///
    /// The two-line rule at the heart of the ticket: a streaming send appends,
    /// a buffered one replaces. Everything else here is the turn bookkeeping
    /// that has to stay in step with it.
    #[allow(clippy::too_many_arguments)]
    fn message_sent(
        &self,
        thread: &mut Thread,
        message_id: &str,
        role: &str,
        text: &str,
        turn_id: Option<&String>,
        streaming: bool,
        at: &str,
    ) -> Value {
        match thread
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
        {
            Some(existing) => {
                if streaming {
                    existing.text.push_str(text);
                } else {
                    // The buffered message wins — unless it is empty, in which
                    // case there is nothing authoritative in it to win with and
                    // the accumulation stands. The client's reducer makes the
                    // same exception.
                    if !text.is_empty() {
                        // The one place the reconciliation assumption is
                        // checked, and it is checked on every turn rather than
                        // in a test: did the deltas already build exactly this?
                        self.inner.reconciled_messages.fetch_add(1, Ordering::Relaxed);
                        if existing.text == text {
                            self.inner
                                .messages_matching_deltas
                                .fetch_add(1, Ordering::Relaxed);
                        } else {
                            eprintln!(
                                "lightcode: thread {}: the buffered message replaced {} streamed \
                                 characters with {}",
                                thread.id,
                                existing.text.len(),
                                text.len()
                            );
                        }
                        existing.text = text.to_string();
                    }
                    existing.updated_at = at.to_string();
                }
                existing.streaming = streaming;
            }
            None => thread.messages.push(Message {
                id: message_id.to_string(),
                role: role.to_string(),
                text: text.to_string(),
                turn_id: turn_id.cloned(),
                streaming,
                created_at: at.to_string(),
                updated_at: at.to_string(),
            }),
        }

        if role == "assistant" {
            thread.latest_turn = bind_assistant_message(
                thread.latest_turn.take(),
                thread.session.as_ref(),
                turn_id,
                message_id,
                streaming,
                at,
            );
        }

        json!({
            "threadId": thread.id,
            "messageId": message_id,
            "role": role,
            "text": text,
            "turnId": turn_id,
            "streaming": streaming,
            "createdAt": at,
            "updatedAt": at,
        })
    }

    /// Open an `orchestration.subscribeThread` subscription: the thread now,
    /// then every change to it.
    pub fn subscribe(&self, thread_id: &str, wants_marker: bool) -> EventSource {
        let entry = self.entry(thread_id);
        // Subscribed to before the description closure is handed over, so a
        // change landing between here and the pump's first read arrives as an
        // event rather than falling into the gap — the same ordering
        // [`crate::orchestration::Shell::subscribe`] keeps, and absorbed the
        // same way, by a client that drops anything at or below what it holds.
        let updates = entry.events.subscribe();
        let sequences = self.inner.sequences.clone();
        let marker_owed = AtomicBool::new(wants_marker);

        EventSource::new(
            move || {
                let mut items = Vec::new();
                // A thread that does not exist yet describes itself as nothing
                // rather than as an empty thread. The UI subscribes to a draft
                // before the server has heard of it, and an empty snapshot would
                // be a claim that the conversation is empty — which would wipe
                // the messages the composer is optimistically showing.
                if let Some(thread) = lock(&entry.state).as_ref() {
                    items.push(json!({
                        "kind": "snapshot",
                        "snapshot": {
                            "snapshotSequence": sequences.current(),
                            "thread": thread.to_detail_value(),
                        },
                    }));
                }
                if marker_owed.swap(false, Ordering::Relaxed) {
                    items.push(json!({"kind": "synchronized"}));
                }
                items
            },
            updates,
        )
    }

    /// Every thread, as the project list carries them.
    pub fn shell_summaries(&self) -> Vec<Value> {
        let entries: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
        let mut summaries: Vec<((String, String), Value)> = entries
            .iter()
            .filter_map(|entry| {
                let state = lock(&entry.state);
                let thread = state.as_ref()?;
                Some((
                    (thread.created_at.clone(), thread.id.clone()),
                    thread.to_shell_value(),
                ))
            })
            .collect();
        // A total order, because a map has none and a snapshot that reshuffled
        // between reads would make the thread list jump. The id breaks the tie:
        // timestamps here are milliseconds, and two threads created inside one
        // would otherwise keep whatever order the map happened to yield.
        summaries.sort_by(|left, right| left.0.cmp(&right.0));
        summaries.into_iter().map(|(_, thread)| thread).collect()
    }

    /// The most recent moment any thread changed, for the shell snapshot's own
    /// timestamp.
    pub fn latest_change(&self) -> Option<String> {
        let entries: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
        entries
            .iter()
            .filter_map(|entry| lock(&entry.state).as_ref().map(|thread| thread.updated_at.clone()))
            .max()
    }

    /// Remember the `claude` session the agent announced for this thread.
    ///
    /// Nothing is published, and that is not an omission: no event in the
    /// contract describes this and no client renders it. It is the server's own
    /// handle on the agent's memory — what `--resume` will be given — so what it
    /// owes is a durable write and nothing else. The `session.init` activity
    /// beside it is where the same id becomes visible.
    ///
    /// An id the thread already holds is dropped rather than rewritten. A
    /// resumed session announces one on every start, and a row that says what it
    /// already said is a disk touch for nothing.
    pub fn remember_agent_session(&self, thread_id: &str, session_id: &str) {
        let Some(entry) = self.find(thread_id) else {
            return;
        };
        let mut state = lock(&entry.state);
        let Some(thread) = state.as_mut() else { return };
        if thread.agent_session_id.as_deref() == Some(session_id) {
            return;
        }

        thread.agent_session_id = Some(session_id.to_string());
        self.inner.transcripts.queue(Write::AgentSession {
            thread_id: thread.id.clone(),
            session_id: session_id.to_string(),
        });
    }

    /// The conversations in a project, oldest first.
    ///
    /// Asked of this registry rather than of the database, and that is the whole
    /// point of the method: a thread reaches the database *eventually* — see
    /// [`crate::transcripts`] — so the stored rows are a subset of what exists. A
    /// project deleted seconds after a conversation started would leave that
    /// conversation behind if the database were the source of truth for which
    /// conversations there were.
    pub fn of_project(&self, project_id: &str) -> Vec<String> {
        let entries: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
        let mut found: Vec<(String, String)> = entries
            .iter()
            .filter_map(|entry| {
                let state = lock(&entry.state);
                let thread = state.as_ref().filter(|thread| thread.project_id == project_id)?;
                Some((thread.created_at.clone(), thread.id.clone()))
            })
            .collect();
        // The same total order the snapshot uses, so the events that remove these
        // arrive in the order the thread list has them.
        found.sort();
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// Forget these conversations.
    ///
    /// Called when their project is deleted. Doing it here as well as in the
    /// database is not tidying: a thread that outlived its project in this registry
    /// would be listed in every snapshot until the next restart and gone after it,
    /// which is the worst of both answers.
    ///
    /// An agent still running for one of them is told there will be no more turns,
    /// by the same mechanism a shutdown uses — dropping the prompt channel. Nothing
    /// *waits* for it, because a project delete answers the client immediately; the
    /// driver's handle is parked on [`Inner::winding_down`] so that a shutdown a
    /// moment later still reaps the child rather than finding the task detached.
    ///
    /// The queue is told as well, so the writes already in flight for these
    /// conversations are dropped rather than refused by a foreign key whose project
    /// is now gone.
    pub fn forget(&self, thread_ids: &[String]) {
        // The entries are taken out from under the registry lock and their drivers
        // released afterwards, which is the order [`Threads::shutdown`] uses. One
        // path that held both locks in the other order would be enough for a
        // deadlock, and there is no reason for this to be that path.
        let going: Vec<Arc<Entry>> = {
            let mut open = self.lock();
            thread_ids
                .iter()
                .filter_map(|thread_id| open.remove(thread_id))
                .collect()
        };

        let mut winding_down = lock(&self.inner.winding_down);
        // Whatever has already finished is dropped here rather than accumulating
        // for the life of the process.
        winding_down.retain(|driver| !driver.is_finished());
        for entry in going {
            if let Some(live) = lock(&entry.live).take() {
                self.inner.live_agents.fetch_sub(1, Ordering::Relaxed);
                // Dropping the sender is the driver's signal that there are no
                // more turns; keeping the handle is what lets shutdown wait.
                drop(live.prompts);
                winding_down.push(live.task);
            }
        }
        drop(winding_down);

        self.inner.transcripts.discard(thread_ids);
    }

    /// Is a thread here?
    pub fn contains(&self, thread_id: &str) -> bool {
        self.get(thread_id).is_some()
    }

    /// A copy of the thread, for a caller that needs to know what to start an
    /// agent with.
    pub fn get(&self, thread_id: &str) -> Option<Thread> {
        lock(&self.find(thread_id)?.state).clone()
    }

    /// How many turns of this conversation have had their working tree
    /// recorded.
    ///
    /// The number a checkpoint is taken *at*: the next capture is this plus one,
    /// and the baseline the next turn is diffed against is this. Zero for a
    /// conversation that has never finished a turn — and zero is also the
    /// baseline's own count, so the two read the same and mean the same thing.
    ///
    /// The **maximum** rather than the length, because a conversation whose
    /// turns were recorded out of order — a capture that failed and a later one
    /// that did not — must not have the gap counted as a turn that never
    /// happened.
    pub fn checkpoint_count(&self, thread_id: &str) -> u64 {
        self.get(thread_id)
            .map(|thread| {
                thread
                    .checkpoints
                    .iter()
                    .map(|checkpoint| checkpoint.turn_count)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    // -- the running agent --------------------------------------------------

    /// Give this thread a driver, unless it already has one.
    ///
    /// Returns the channel to feed prompts into either way, so a caller does not
    /// have to know whether it was the one that started the session. `start`
    /// receives the prompt channel's other end and is expected to spawn the task
    /// that owns the agent.
    ///
    /// Synchronous on purpose: the call that dispatches a turn has to answer the
    /// client immediately, so nothing on this path may wait for a process to
    /// exist. Starting the agent happens inside the spawned task, and the first
    /// prompt waits in the channel until it does.
    pub fn attach(
        &self,
        thread_id: &str,
        start: impl FnOnce(mpsc::Receiver<Prompt>, mpsc::Receiver<Signal>) -> JoinHandle<()>,
    ) -> mpsc::Sender<Prompt> {
        let entry = self.entry(thread_id);
        let mut live = lock(&entry.live);
        if let Some(running) = live.as_ref() {
            return running.prompts.clone();
        }

        let (prompts, incoming) = mpsc::channel(PROMPT_QUEUE);
        let (signals, signalled) = mpsc::channel(SIGNAL_QUEUE);
        self.inner.live_agents.fetch_add(1, Ordering::Relaxed);
        *live = Some(Live {
            prompts: prompts.clone(),
            signals,
            task: start(incoming, signalled),
        });
        prompts
    }

    /// Hand a permission decision to the agent that is waiting on it.
    ///
    /// Refused, with a sentence, when there is nothing waiting: a decision with
    /// no session behind it is one the developer is about to be told did not
    /// land, which is a great deal better than an approval that quietly reached
    /// nothing. Whether the *request* is one this session knows is the driver's
    /// question, because the driver is what saw it asked.
    ///
    /// Synchronous, like [`Threads::attach`] and for the same reason: it is
    /// called from the socket's read loop, which has to stay free for the next
    /// frame.
    pub fn answer(&self, thread_id: &str, answered: Answered) -> Result<(), String> {
        let Some(running) = self.live(thread_id)? else {
            return Err(
                "No agent is running for this conversation, so there is no permission request \
                 left to answer."
                    .to_string(),
            );
        };

        running.try_send(Signal::Answer(answered)).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                "The agent has not read the decisions already sent to it, so this one was not \
                 queued."
                    .to_string()
            }
            mpsc::error::TrySendError::Closed(_) => {
                "The agent session has ended and could not be given this decision.".to_string()
            }
        })
    }

    /// Stop whatever the agent is doing for this conversation.
    ///
    /// **A session that is not there is not an error**, and that is the whole
    /// difference between this and [`Threads::answer`] beside it. A decision with
    /// nothing to route it to is a developer about to be told their click did not
    /// land; an interrupt with nothing to route it to is a developer who got what
    /// they asked for — the agent is not running. The ticket asks for exactly
    /// that: "interrupting when no turn is in flight is a no-op rather than an
    /// error".
    ///
    /// Whether the turn *named* is the turn in flight is the driver's question,
    /// for the same reason the request id is: the driver is the only thing that
    /// knows what it is currently working on.
    ///
    /// The one failure left is a driver that has stopped reading, which is worth
    /// saying because the stop button will otherwise appear to have worked.
    pub fn interrupt(&self, thread_id: &str, turn_id: Option<String>) -> Result<(), String> {
        let Some(running) = self.live(thread_id)? else {
            return Ok(());
        };

        match running.try_send(Signal::Interrupt { turn_id }) {
            Ok(()) => Ok(()),
            // The session ended between the lookup and the send, which is the
            // no-op case arriving a moment later than it might have.
            Err(mpsc::error::TrySendError::Closed(_)) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(
                "The agent has not read the signals already sent to it, so it was not asked to \
                 stop."
                    .to_string(),
            ),
        }
    }

    /// The running session's signal channel, or `None` when nothing is running.
    ///
    /// `Err` is reserved for a conversation this server has never heard of,
    /// which is a client naming something that does not exist rather than
    /// anything about a session.
    fn live(&self, thread_id: &str) -> Result<Option<mpsc::Sender<Signal>>, String> {
        let entry = self
            .find(thread_id)
            .ok_or_else(|| format!("There is no conversation '{thread_id}' on this server."))?;
        let live = lock(&entry.live);
        Ok(live.as_ref().map(|running| running.signals.clone()))
    }

    /// The turn this conversation's session says it is working on.
    ///
    /// Asked by the driver before it settles a turn, and the reason it has to be
    /// asked rather than assumed: after an interrupt the developer can send the
    /// next turn while the agent is still winding the old one down, and a session
    /// change published for the finished turn would describe the one that just
    /// started. Reads one field rather than cloning the conversation, because it
    /// is on the path of every completed turn.
    pub fn active_turn(&self, thread_id: &str) -> Option<String> {
        let entry = self.find(thread_id)?;
        let state = lock(&entry.state);
        state
            .as_ref()?
            .session
            .as_ref()?
            .active_turn_id
            .clone()
    }

    /// Called by the driver when its agent has gone, so the next turn starts a
    /// new one rather than writing into a closed pipe.
    ///
    /// Does not wait for the task: this *is* the task, at the end of itself.
    ///
    /// Asks for the slot rather than making one. A driver can outlive the thread
    /// it was driving — [`Threads::forget`] removes a deleted project's
    /// conversations while their agents are still winding down — and allocating
    /// here would put an empty slot back for every one of them.
    pub fn detach(&self, thread_id: &str) {
        let Some(entry) = self.find(thread_id) else {
            return;
        };
        if lock(&entry.live).take().is_some() {
            self.inner.live_agents.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// End every session and wait for the agents to be reaped.
    ///
    /// What the server calls on its way down. Dropping the prompt channel is the
    /// signal — the driver reads it as "no more turns", closes the agent's stdin
    /// and lets it exit — and awaiting the task is what makes "reaped" true
    /// rather than merely asked for.
    pub async fn shutdown(&self) {
        let mut running: Vec<JoinHandle<()>> = {
            let open: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
            open.iter()
                .filter_map(|entry| lock(&entry.live).take())
                .map(|live| {
                    self.inner.live_agents.fetch_sub(1, Ordering::Relaxed);
                    // Dropping `live` here drops the sender with it, which is
                    // what the driver is waiting on.
                    live.task
                })
                .collect()
        };
        // The drivers of conversations whose project was deleted. They were already
        // told there would be no more turns; what is owed them is the wait.
        running.append(&mut lock(&self.inner.winding_down));

        for task in running {
            let _ = task.await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Entry>>> {
        self.inner
            .open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A poisoned lock means a previous holder panicked mid-change. What is behind
/// it is a plain value with no invariant a panic could have broken halfway, so
/// refusing to use it would turn one panic into a dead conversation.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Change {
    fn event_type(&self) -> &'static str {
        match self {
            Change::UserMessage { .. }
            | Change::AssistantDelta { .. }
            | Change::AssistantMessage { .. } => "thread.message-sent",
            Change::TurnRequested { .. } => "thread.turn-start-requested",
            Change::InterruptRequested { .. } => "thread.turn-interrupt-requested",
            Change::Session(_) => "thread.session-set",
            Change::Activity(_) => "thread.activity-appended",
            Change::Checkpointed(_) => "thread.turn-diff-completed",
        }
    }
}

/// What a change owes the database, once it has been folded in.
///
/// Three rules, and each is a decision this module's documentation argues:
///
/// - **A delta owes nothing.** The buffered message supersedes it within the
///   turn, and a row per token would put the disk in the streaming path.
/// - **Everything else writes the row.** Every change moves `updatedAt` and most
///   of them move the latest turn with it. The row is a dozen small fields and
///   the writes are batched, so writing it unconditionally costs less than
///   working out whether it was needed.
/// - **A message or an activity writes itself as well, at the position the fold
///   gave it.** The position is stored because a millisecond timestamp is not a
///   total order, and a transcript that reordered itself across a restart would
///   be a different conversation.
fn durable(thread: &Thread, change: &Change) -> Vec<Write> {
    let message_id = match change {
        Change::AssistantDelta { .. } => return Vec::new(),
        Change::UserMessage { message_id, .. } | Change::AssistantMessage { message_id, .. } => {
            Some(message_id)
        }
        _ => None,
    };

    let mut writes = vec![Write::Thread(Box::new(thread.row()))];

    // Both positions are *found* rather than assumed to be the end. For a message
    // that is load-bearing — a buffered message replaces one the deltas already put
    // in the transcript, and that one is not at the end once a turn has said
    // several things — and for an activity it is merely honest: a position that
    // came from a length would have to invent an answer for a log the fold had not
    // appended to, and there is no honest one.
    if let Some(message_id) = message_id {
        if let Some((ordinal, message)) = at(&thread.messages, |message| &message.id == message_id) {
            writes.push(Write::Message {
                thread_id: thread.id.clone(),
                ordinal,
                message: message.clone(),
            });
        }
    }

    if let Change::Activity(appended) = change {
        if let Some((ordinal, activity)) = at(&thread.activities, |activity| {
            activity.id == appended.id
        }) {
            writes.push(Write::Activity {
                thread_id: thread.id.clone(),
                ordinal,
                activity: Box::new(activity.clone()),
            });
        }
    }

    // Keyed by the turn count rather than positioned like a message or an
    // activity, because that is what a checkpoint *is* — one row per turn, and
    // a second capture of the same turn replaces the first. So there is no
    // ordinal to find and nothing that can be written out of order.
    if let Change::Checkpointed(recorded) = change {
        writes.push(Write::Checkpoint {
            thread_id: thread.id.clone(),
            checkpoint: recorded.clone(),
        });
    }

    writes
}

/// The first item matching `wanted`, and where it is.
fn at<T>(items: &[T], wanted: impl Fn(&T) -> bool) -> Option<(usize, &T)> {
    items.iter().enumerate().find(|(_, item)| wanted(item))
}

/// Move the latest turn on when an assistant message lands.
///
/// `threadReducer.ts`'s rule, and the subtle half of it is why a completed
/// assistant message does *not* end the turn: a provider may send several of
/// them in one turn — commentary between tool calls — so the turn stays running
/// until the session says otherwise. A turn that uses a tool is exactly the case
/// that breaks if this settles early, and the CLI emits one buffered message per
/// content block, so such a turn produces several of them as a matter of course.
fn bind_assistant_message(
    latest: Option<LatestTurn>,
    session: Option<&Session>,
    turn_id: Option<&String>,
    message_id: &str,
    streaming: bool,
    at: &str,
) -> Option<LatestTurn> {
    let turn_id = turn_id?;
    if latest.as_ref().is_some_and(|turn| &turn.turn_id != turn_id) {
        return latest;
    }

    let still_running = session.is_some_and(|session| {
        session.status == SessionStatus::Running
            && session.active_turn_id.as_ref() == Some(turn_id)
    });
    let settles = !streaming && !still_running;
    let previous = latest.as_ref();

    Some(LatestTurn {
        turn_id: turn_id.clone(),
        state: match settles {
            false => TurnState::Running,
            true => match previous.map(|turn| turn.state) {
                Some(TurnState::Interrupted) => TurnState::Interrupted,
                Some(TurnState::Error) => TurnState::Error,
                _ => TurnState::Completed,
            },
        },
        requested_at: previous
            .map(|turn| turn.requested_at.clone())
            .unwrap_or_else(|| at.to_string()),
        started_at: previous
            .and_then(|turn| turn.started_at.clone())
            .or_else(|| Some(at.to_string())),
        completed_at: match settles {
            true => Some(at.to_string()),
            false => previous.and_then(|turn| turn.completed_at.clone()),
        },
        assistant_message_id: Some(message_id.to_string()),
    })
}

/// Move the latest turn on when the session changes state.
///
/// Leaving `running` is the authoritative end of a turn — the point the client's
/// reducer settles on, and the reason a turn's duration covers the whole turn
/// rather than stopping at the last assistant message.
fn settle(latest: Option<LatestTurn>, session: &Session) -> Option<LatestTurn> {
    if session.status == SessionStatus::Running {
        if let Some(active) = &session.active_turn_id {
            let previous = latest.as_ref().filter(|turn| &turn.turn_id == active);
            return Some(LatestTurn {
                turn_id: active.clone(),
                state: TurnState::Running,
                requested_at: previous
                    .map(|turn| turn.requested_at.clone())
                    .unwrap_or_else(|| session.updated_at.clone()),
                started_at: previous
                    .and_then(|turn| turn.started_at.clone())
                    .or_else(|| Some(session.updated_at.clone())),
                completed_at: None,
                assistant_message_id: previous.and_then(|turn| turn.assistant_message_id.clone()),
            });
        }
    }

    // `starting` and `running` say nothing about how the turn went, so a running
    // turn stays running rather than being called completed. Every other status
    // settles it — including `stopped`, which is the process going away
    // underneath a turn: nobody asked for that, but the turn did not finish, and
    // upstream reads it as `interrupted` in both of its copies of this rule.
    let Some(settled) = session.status.settles_turn_as() else {
        return latest;
    };

    match latest {
        Some(turn) if turn.state == TurnState::Running => Some(LatestTurn {
            state: settled,
            completed_at: Some(session.updated_at.clone()),
            ..turn
        }),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

fn created_event(sequence: i64, thread: &Thread) -> Value {
    thread_event(
        sequence,
        &thread.id,
        "thread.created",
        json!({
            "threadId": thread.id,
            "projectId": thread.project_id,
            "title": thread.title,
            "modelSelection": thread.model_selection,
            "runtimeMode": thread.runtime_mode,
            "interactionMode": thread.interaction_mode,
            "branch": thread.branch,
            "worktreePath": thread.worktree_path,
            "createdAt": thread.created_at,
            "updatedAt": thread.updated_at,
        }),
        &thread.updated_at,
    )
}

/// One `OrchestrationThreadStreamItem` of kind `event`.
///
/// The four correlation fields are `null` and `metadata` is empty, which the
/// contract requires as keys and permits as values. They exist upstream so an
/// event can be traced to the command that caused it through an event store
/// lightcode does not have; inventing ids for them would be describing a
/// causation chain nothing recorded.
fn thread_event(
    sequence: i64,
    thread_id: &str,
    event_type: &str,
    payload: Value,
    at: &str,
) -> Value {
    json!({
        "kind": "event",
        "event": {
            "sequence": sequence,
            "eventId": fresh_id("event"),
            "aggregateKind": "thread",
            "aggregateId": thread_id,
            "occurredAt": at,
            "commandId": Value::Null,
            "causationEventId": Value::Null,
            "correlationId": Value::Null,
            "metadata": Map::new(),
            "type": event_type,
            "payload": payload,
        },
    })
}

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// An identifier no other one in this process will equal.
///
/// The contract types every id as a trimmed non-empty string rather than a
/// UUID, so what these have to be is unique and readable — and readable is worth
/// something, because these ids appear in the transcript a developer is
/// debugging. The process stamp is what keeps a restart from re-issuing ids a
/// client has cached under a different meaning.
fn fresh_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ordinal = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{ordinal:x}", process_stamp())
}

pub fn fresh_turn_id() -> String {
    fresh_id("turn")
}

pub fn fresh_message_id() -> String {
    fresh_id("assistant")
}

pub fn fresh_activity_id() -> String {
    fresh_id("activity")
}

fn process_stamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    static STAMP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *STAMP.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn threads() -> (Threads, broadcast::Receiver<Value>) {
        let (shell, watching) = broadcast::channel(BACKLOG);
        // Nothing here is about the stored conversation — `crate::transcripts`
        // drives its own writes and `tests/socket_continuity.rs` drives a
        // restart — so these threads are written down nowhere.
        (
            Threads::new(Sequences::from(0), shell, Transcripts::nowhere()),
            watching,
        )
    }

    fn a_thread(id: &str) -> Thread {
        Thread {
            id: id.to_string(),
            project_id: "project-1".to_string(),
            title: "A conversation".to_string(),
            model_selection: json!({"instanceId": "claudeAgent", "model": "claude-opus-5"}),
            runtime_mode: "full-access".to_string(),
            interaction_mode: "default".to_string(),
            branch: None,
            worktree_path: None,
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
            messages: Vec::new(),
            activities: Vec::new(),
            checkpoints: Vec::new(),
            session: None,
            latest_turn: None,
            latest_user_message_at: None,
            agent_session_id: None,
        }
    }

    fn running(turn_id: &str) -> Session {
        Session {
            status: SessionStatus::Running,
            runtime_mode: "full-access".to_string(),
            active_turn_id: Some(turn_id.to_string()),
            last_error: None,
            updated_at: now_iso(),
        }
    }

    /// The transcript as the server holds it, which is what a client arriving
    /// mid-turn is handed.
    fn transcript(threads: &Threads, id: &str) -> Vec<(String, String, bool)> {
        threads
            .get(id)
            .expect("the thread exists")
            .messages
            .into_iter()
            .map(|message| (message.role, message.text, message.streaming))
            .collect()
    }

    /// The rule the whole ticket turns on, at the seam where it is implemented:
    /// a streaming send appends and a buffered one replaces. Driven with deltas
    /// that were *shed*, because that is the case the rule exists for — the
    /// transcript has to end up saying what the agent said, not what arrived.
    #[test]
    fn the_buffered_message_replaces_what_the_deltas_built() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Session(running("turn-1")));

        for piece in ["hel", "l"] {
            threads.apply(
                "thread-1",
                Change::AssistantDelta {
                    message_id: "assistant-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    text: piece.to_string(),
                },
            );
        }
        assert_eq!(
            transcript(&threads, "thread-1"),
            vec![("assistant".to_string(), "hell".to_string(), true)]
        );

        threads.apply(
            "thread-1",
            Change::AssistantMessage {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: "hello there".to_string(),
            },
        );

        assert_eq!(
            transcript(&threads, "thread-1"),
            vec![(
                "assistant".to_string(),
                "hello there".to_string(),
                false
            )],
            "the buffered message did not win"
        );
        assert_eq!(
            threads.reconciliation(),
            Reconciliation {
                reconciled: 1,
                agreed: 0
            },
            "the deltas did not build this text, and the count has to say so"
        );
    }

    /// The same turn with nothing shed. The counters are a ratio, so both halves
    /// have to be driven or the ratio means nothing.
    #[test]
    fn deltas_that_agreed_with_the_buffered_message_are_counted_as_agreeing() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Session(running("turn-1")));

        for piece in ["hello", " there"] {
            threads.apply(
                "thread-1",
                Change::AssistantDelta {
                    message_id: "assistant-1".to_string(),
                    turn_id: "turn-1".to_string(),
                    text: piece.to_string(),
                },
            );
        }
        threads.apply(
            "thread-1",
            Change::AssistantMessage {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: "hello there".to_string(),
            },
        );

        assert_eq!(
            threads.reconciliation(),
            Reconciliation {
                reconciled: 1,
                agreed: 1
            }
        );
    }

    /// An empty buffered message is the one case where the accumulation stands.
    /// The client's reducer makes the same exception, and a server that did not
    /// would blank a reply the user watched arrive.
    #[test]
    fn an_empty_buffered_message_does_not_erase_the_accumulation() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply(
            "thread-1",
            Change::AssistantDelta {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: "everything".to_string(),
            },
        );
        threads.apply(
            "thread-1",
            Change::AssistantMessage {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: String::new(),
            },
        );

        assert_eq!(
            transcript(&threads, "thread-1"),
            vec![("assistant".to_string(), "everything".to_string(), false)]
        );
        assert_eq!(
            threads.reconciliation().reconciled,
            0,
            "nothing was reconciled"
        );
    }

    /// Every change is one event, numbered, in the order it was applied. The
    /// client drops anything at or below what it holds, so an inversion here
    /// would lose an event permanently.
    #[test]
    fn every_change_is_published_as_one_numbered_event() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();

        threads.create(a_thread("thread-1")).expect("created");
        threads.apply(
            "thread-1",
            Change::UserMessage {
                message_id: "message-1".to_string(),
                text: "hello".to_string(),
                turn_id: "turn-1".to_string(),
            },
        );
        threads.apply(
            "thread-1",
            Change::TurnRequested {
                turn_id: "turn-1".to_string(),
                message_id: "message-1".to_string(),
                model_selection: None,
                runtime_mode: None,
                interaction_mode: None,
            },
        );

        let mut seen = Vec::new();
        while let Ok(item) = watching.try_recv() {
            assert_eq!(item["kind"], "event");
            assert_eq!(item["event"]["aggregateKind"], "thread");
            assert_eq!(item["event"]["aggregateId"], "thread-1");
            seen.push((
                item["event"]["type"].as_str().expect("a type").to_string(),
                item["event"]["sequence"].as_i64().expect("a sequence"),
            ));
        }

        assert_eq!(
            seen.iter().map(|(kind, _)| kind.as_str()).collect::<Vec<&str>>(),
            vec![
                "thread.created",
                "thread.message-sent",
                "thread.turn-start-requested"
            ]
        );
        assert!(
            seen.windows(2).all(|pair| pair[0].1 < pair[1].1),
            "sequences must strictly increase: {seen:?}"
        );
    }

    /// A delta is the one change the project list does not need. A turn produces
    /// hundreds, and the thread *summary* is identical across all of them.
    #[test]
    fn the_project_list_hears_about_everything_except_deltas_and_activities() {
        let (threads, mut shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        assert_eq!(
            shell.try_recv().expect("the thread was announced")["kind"],
            "thread-upserted"
        );

        threads.apply(
            "thread-1",
            Change::AssistantDelta {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: "a".to_string(),
            },
        );
        threads.apply(
            "thread-1",
            Change::Activity(Activity {
                id: "activity-1".to_string(),
                tone: "info",
                kind: "session.init".to_string(),
                summary: "started".to_string(),
                payload: json!({}),
                turn_id: None,
                sequence: None,
                created_at: now_iso(),
            }),
        );
        assert!(
            shell.try_recv().is_err(),
            "a token delta must not republish the thread summary"
        );

        threads.apply("thread-1", Change::Session(running("turn-1")));
        assert_eq!(
            shell.try_recv().expect("a session change is a summary change")["kind"],
            "thread-upserted"
        );
    }

    /// An activity is numbered with the sequence its own event was announced
    /// under, and the number reaches the client on the activity itself.
    ///
    /// The client sorts the work log by it and falls back to `createdAt` — a
    /// millisecond — when it is absent, so an unnumbered pair of rows landing inside
    /// one millisecond is re-ordered by a rank that puts every `.updated` before
    /// every `.completed`. That separates a tool invocation from its result, and the
    /// work log only collapses the two when they are *adjacent*.
    #[test]
    fn an_activity_is_numbered_with_the_sequence_it_was_announced_under() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let appended = |kind: &str| {
            Change::Activity(Activity::tool(kind, "Tool call", json!({}), None))
        };
        let first = threads
            .apply("thread-1", appended("tool.updated"))
            .expect("applied");
        let second = threads
            .apply("thread-1", appended("tool.completed"))
            .expect("applied");
        assert!(first < second, "{first} then {second}");

        // The number the change was announced under is the number on the row a
        // client arriving late is handed. The two have to agree, or which order a
        // developer sees depends on when they opened the conversation —
        // `socket_tools.rs` drives the same number off the events themselves.
        let thread = threads.get("thread-1").expect("the thread");
        assert_eq!(
            thread
                .activities
                .iter()
                .map(|activity| activity.sequence)
                .collect::<Vec<Option<i64>>>(),
            vec![Some(first), Some(second)]
        );
        assert_eq!(
            thread.activities[0].to_value()["sequence"],
            json!(first),
            "the number has to reach the client, not merely be held here"
        );
    }

    /// A turn does not end when the assistant stops talking — a provider sends
    /// several messages in one turn — it ends when the session leaves `running`.
    /// Getting this wrong settles a turn in the middle of itself, which is what a
    /// turn that narrates before it calls a tool would hit first.
    #[test]
    fn a_turn_ends_when_the_session_does_rather_than_at_the_last_message() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Session(running("turn-1")));

        threads.apply(
            "thread-1",
            Change::AssistantMessage {
                message_id: "assistant-1".to_string(),
                turn_id: "turn-1".to_string(),
                text: "half way".to_string(),
            },
        );
        let turn = threads.get("thread-1").expect("the thread").latest_turn;
        assert_eq!(turn.as_ref().map(|turn| turn.state), Some(TurnState::Running));
        assert_eq!(
            turn.as_ref().and_then(|turn| turn.assistant_message_id.clone()),
            Some("assistant-1".to_string())
        );

        threads.apply(
            "thread-1",
            Change::Session(Session {
                status: SessionStatus::Ready,
                active_turn_id: None,
                ..running("turn-1")
            }),
        );
        let turn = threads
            .get("thread-1")
            .expect("the thread")
            .latest_turn
            .expect("a turn");
        assert_eq!(turn.state, TurnState::Completed);
        assert!(turn.completed_at.is_some(), "a completed turn has an end");
        assert!(turn.started_at.is_some(), "and a beginning to measure from");
    }

    /// The agent process going away under a running turn settles that turn, and
    /// settles it as interrupted rather than leaving it running forever.
    ///
    /// Nobody asked for it — that is what makes the session `stopped` rather
    /// than `interrupted` — but the turn did not finish, and the two copies of
    /// this rule upstream keeps (`ProjectionPipeline.ts:78`,
    /// `threadReducer.ts:539`) both read it that way. The client folds the same
    /// events with its own copy, so a turn left `running` here would be a
    /// conversation this server reports as working and the UI does not.
    ///
    /// Ticket 15 is what makes this reachable: `crate::turn` currently reports an
    /// unfinished turn as `error` and keeps `stopped` for the case where none was
    /// running. This is the ground that choice will stand on.
    #[test]
    fn a_turn_the_process_died_under_settles_as_interrupted() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Session(running("turn-1")));

        threads.apply(
            "thread-1",
            Change::Session(Session {
                status: SessionStatus::Stopped,
                active_turn_id: None,
                ..running("turn-1")
            }),
        );

        let turn = threads
            .get("thread-1")
            .expect("the thread")
            .latest_turn
            .expect("a turn");
        assert_eq!(turn.state, TurnState::Interrupted);
        assert!(
            turn.completed_at.is_some(),
            "a turn nothing is going to finish has ended"
        );
    }

    /// A thread the server has never heard of is a draft the UI is showing. Its
    /// subscription has to open and stay silent rather than fail, and it has to
    /// be the *same* subscription that carries the thread once a turn creates it.
    #[test]
    fn a_subscription_opened_before_the_thread_exists_carries_it_when_it_does() {
        let (threads, _shell) = threads();
        let source = threads.subscribe("thread-1", true);

        let opening = source.describe();
        assert_eq!(
            opening,
            vec![json!({"kind": "synchronized"})],
            "a draft describes itself as nothing, not as an empty conversation"
        );

        threads.create(a_thread("thread-1")).expect("created");
        let described = threads.subscribe("thread-1", false).describe();
        assert_eq!(described.len(), 1);
        assert_eq!(described[0]["kind"], "snapshot");
        assert_eq!(described[0]["snapshot"]["thread"]["id"], "thread-1");
        assert!(described[0]["snapshot"]["snapshotSequence"].is_i64());
    }

    /// The snapshot is taken at the highest number handed out, so every event
    /// issued after it is strictly newer. A snapshot numbered above its
    /// successors would have the client drop them.
    #[test]
    fn a_snapshot_is_older_than_every_event_that_follows_it() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let snapshot = threads.subscribe("thread-1", false).describe();
        let taken_at = snapshot[0]["snapshot"]["snapshotSequence"]
            .as_i64()
            .expect("a sequence");

        let next = threads
            .apply(
                "thread-1",
                Change::UserMessage {
                    message_id: "message-1".to_string(),
                    text: "hello".to_string(),
                    turn_id: "turn-1".to_string(),
                },
            )
            .expect("applied");

        assert!(next > taken_at, "{next} is not newer than {taken_at}");
    }

    /// Two threads with one id is a conversation that could be silently
    /// replaced. The refusal names both the thread and its project, because the
    /// client renders the sentence and nothing else.
    #[test]
    fn a_thread_cannot_be_created_twice() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let refusal = threads
            .create(a_thread("thread-1"))
            .expect_err("the id is taken");
        assert!(refusal.contains("thread-1"), "{refusal}");
        assert!(refusal.contains("project-1"), "{refusal}");
    }

    /// A change for a thread nothing created describes a conversation no client
    /// could open, so it is dropped rather than published.
    #[test]
    fn a_change_to_a_thread_that_does_not_exist_is_not_published() {
        let (threads, mut shell) = threads();

        assert_eq!(
            threads.apply(
                "never-created",
                Change::UserMessage {
                    message_id: "message-1".to_string(),
                    text: "hello".to_string(),
                    turn_id: "turn-1".to_string(),
                },
            ),
            None
        );
        assert!(shell.try_recv().is_err());
        assert!(threads.shell_summaries().is_empty());
    }

    /// The two renderings of one thread carry the keys the contract declares.
    /// A client rejects the whole snapshot on a missing one and then shows no
    /// conversation at all, which is a worse failure than a slightly wrong one.
    #[test]
    fn both_renderings_carry_every_key_the_contract_declares() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Session(running("turn-1")));
        let thread = threads.get("thread-1").expect("the thread");

        let detail = thread.to_detail_value();
        for key in [
            "id",
            "projectId",
            "title",
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "branch",
            "worktreePath",
            "latestTurn",
            "createdAt",
            "updatedAt",
            "archivedAt",
            "settledOverride",
            "settledAt",
            "deletedAt",
            "messages",
            "proposedPlans",
            "activities",
            "checkpoints",
            "session",
        ] {
            assert!(detail.get(key).is_some(), "the detail is missing {key}");
        }

        let summary = thread.to_shell_value();
        for key in [
            "id",
            "projectId",
            "title",
            "modelSelection",
            "runtimeMode",
            "interactionMode",
            "branch",
            "worktreePath",
            "latestTurn",
            "createdAt",
            "updatedAt",
            "archivedAt",
            "settledOverride",
            "settledAt",
            "session",
            "latestUserMessageAt",
            "hasPendingApprovals",
            "hasPendingUserInput",
            "hasActionableProposedPlan",
        ] {
            assert!(summary.get(key).is_some(), "the summary is missing {key}");
        }
        assert!(
            summary.get("messages").is_none(),
            "the project list must not carry a transcript"
        );

        let session = &summary["session"];
        assert_eq!(session["threadId"], "thread-1");
        assert_eq!(session["status"], "running");
        assert_eq!(session["providerName"], crate::provider::INSTANCE_ID);
    }

    // -- coming back from a restart -----------------------------------------

    /// A stored conversation comes back with its transcript and without a
    /// session. Nothing is running after a restart, and a thread claiming an
    /// agent behind it would have the composer offering to interrupt one.
    #[test]
    fn a_restored_conversation_has_its_transcript_and_no_session() {
        let (threads, mut shell) = threads();
        threads.restore(vec![Conversation {
            thread: a_thread("thread-1").row(),
            messages: vec![Message {
                id: "message-1".to_string(),
                role: "user".to_string(),
                text: "yesterday".to_string(),
                turn_id: Some("turn-1".to_string()),
                streaming: false,
                created_at: now_iso(),
                updated_at: now_iso(),
            }],
            activities: Vec::new(),
            checkpoints: Vec::new(),
        }]);

        let restored = threads.get("thread-1").expect("the conversation is back");
        assert_eq!(restored.messages.len(), 1);
        assert_eq!(restored.messages[0].text, "yesterday");
        assert!(restored.session.is_none());
        assert!(
            shell.try_recv().is_err(),
            "a restart is not a hundred conversations being created"
        );
        assert_eq!(
            threads.inner.sequences.current(),
            0,
            "restoring took a sequence number"
        );
    }

    /// The hard-kill case: the app went while a turn was in flight, so the stored
    /// turn is still `running` and nothing ever got to say otherwise. It has to
    /// come back as interrupted, or the conversation shows an agent working with
    /// nothing left alive to settle it.
    #[test]
    fn a_turn_stored_while_it_was_still_running_comes_back_interrupted() {
        let (threads, _shell) = threads();
        let mut row = a_thread("thread-1").row();
        row.latest_turn = Some(LatestTurn {
            turn_id: "turn-1".to_string(),
            state: TurnState::Running,
            requested_at: "2026-07-26T00:23:04.909Z".to_string(),
            started_at: Some("2026-07-26T00:23:04.909Z".to_string()),
            completed_at: None,
            assistant_message_id: None,
        });
        row.updated_at = "2026-07-26T00:23:09.000Z".to_string();

        threads.restore(vec![Conversation {
            thread: row,
            messages: Vec::new(),
            activities: Vec::new(),
            checkpoints: Vec::new(),
        }]);

        let turn = threads
            .get("thread-1")
            .expect("the conversation")
            .latest_turn
            .expect("a turn");
        assert_eq!(turn.state, TurnState::Interrupted);
        assert_eq!(
            turn.completed_at,
            Some("2026-07-26T00:23:09.000Z".to_string()),
            "the last moment the thread is known to have changed is the closest \
             true answer available"
        );

        // A turn that had already settled is left exactly as it was.
        let mut finished = a_thread("thread-2").row();
        finished.latest_turn = Some(LatestTurn {
            state: TurnState::Completed,
            completed_at: Some("2026-07-26T00:23:07.000Z".to_string()),
            ..turn
        });
        threads.restore(vec![Conversation {
            thread: finished,
            messages: Vec::new(),
            activities: Vec::new(),
            checkpoints: Vec::new(),
        }]);
        assert_eq!(
            threads
                .get("thread-2")
                .expect("the conversation")
                .latest_turn
                .map(|turn| turn.state),
            Some(TurnState::Completed)
        );
    }

    /// The agent's session id is the server's own bookkeeping: remembered and
    /// written down, and published to nobody, because no event in the contract
    /// describes it and no client renders it.
    #[test]
    fn the_agents_session_is_remembered_without_being_announced() {
        let (threads, mut shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        shell.try_recv().expect("the thread was announced");

        threads.remember_agent_session("thread-1", "session-alpha");
        assert_eq!(
            threads.get("thread-1").expect("the thread").agent_session_id,
            Some("session-alpha".to_string())
        );
        assert!(
            shell.try_recv().is_err(),
            "remembering the agent's session republished the thread"
        );

        // A resumed session announces one on every start, and the newest is the
        // one the next `--resume` has to be given.
        threads.remember_agent_session("thread-1", "session-beta");
        assert_eq!(
            threads.get("thread-1").expect("the thread").agent_session_id,
            Some("session-beta".to_string())
        );
    }

    /// A delta owes the database nothing — the buffered message supersedes it —
    /// and everything else writes the row it changed. This is the rule that keeps
    /// the disk out of the streaming path, checked where it is decided.
    #[test]
    fn a_delta_is_the_one_change_that_is_not_written_down() {
        let mut thread = a_thread("thread-1");
        thread.messages.push(Message {
            id: "assistant-1".to_string(),
            role: "assistant".to_string(),
            text: "hell".to_string(),
            turn_id: Some("turn-1".to_string()),
            streaming: true,
            created_at: now_iso(),
            updated_at: now_iso(),
        });

        let delta = Change::AssistantDelta {
            message_id: "assistant-1".to_string(),
            turn_id: "turn-1".to_string(),
            text: "o".to_string(),
        };
        assert!(durable(&thread, &delta).is_empty());

        let buffered = Change::AssistantMessage {
            message_id: "assistant-1".to_string(),
            turn_id: "turn-1".to_string(),
            text: "hello".to_string(),
        };
        let writes = durable(&thread, &buffered);
        assert_eq!(writes.len(), 2, "{writes:#?}");
        assert!(matches!(writes[0], Write::Thread(_)));
        // At the position the fold gave it, which is not necessarily the end: a
        // buffered message replaces one the deltas already put in the transcript.
        assert!(matches!(
            &writes[1],
            Write::Message { ordinal: 0, message, .. } if message.id == "assistant-1"
        ));
    }

    /// Identifiers appear in a transcript a developer is reading, and two the
    /// same would join two things the client keeps apart.
    #[test]
    fn every_minted_identifier_is_distinct() {
        let minted: Vec<String> = (0..64)
            .map(|_| fresh_turn_id())
            .chain((0..64).map(|_| fresh_message_id()))
            .collect();

        let unique: std::collections::HashSet<&String> = minted.iter().collect();
        assert_eq!(unique.len(), minted.len());
        assert!(minted.iter().all(|id| !id.trim().is_empty()));
    }
}
