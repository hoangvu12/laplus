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
//! ## What is deliberately not here
//!
//! - **Persistence.** Threads live for as long as the server does. Ticket 11 owns
//!   multi-turn continuity and persistence together, and they are one job: what
//!   makes a conversation survive a restart is the CLI's own `--session-id` and
//!   `--resume`, not a table of messages the agent has forgotten about.
//! - **Tool use, approvals, checkpoints, proposed plans.** `activities`,
//!   `checkpoints` and `proposedPlans` are present and empty except for the two
//!   activities a turn produces here; tickets 12, 13 and 20 fill them.
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
use crate::store::Sequences;
use crate::subscriptions::{EventSource, BACKLOG};

/// The subscription that *is* one conversation.
pub const SUBSCRIBE_THREAD: &str = "orchestration.subscribeThread";

/// How many user turns may be waiting for an agent that has not read them yet.
///
/// A person types one prompt at a time and the agent is normally already
/// listening; this is only the window between a turn being dispatched and the
/// child existing. Bounded rather than unbounded so a session whose agent never
/// started cannot absorb prompts forever.
const PROMPT_QUEUE: usize = 8;

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
    pub session: Option<Session>,
    pub latest_turn: Option<LatestTurn>,
    /// When the developer last said something. On the shell summary rather than
    /// derived by the client, so the thread list can sort without the messages.
    pub latest_user_message_at: Option<String>,
}

/// One message in the transcript.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct Activity {
    pub id: String,
    pub tone: &'static str,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub turn_id: Option<String>,
    pub created_at: String,
}

/// The agent process behind a thread, as the client sees it.
#[derive(Debug, Clone)]
pub struct Session {
    pub status: &'static str,
    pub runtime_mode: String,
    pub active_turn_id: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

/// The most recent turn and how far it got.
#[derive(Debug, Clone)]
pub struct LatestTurn {
    pub turn_id: String,
    pub state: &'static str,
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
            "checkpoints": [],
            "session": self.session.as_ref().map(|session| session.to_value(&self.id)),
        })
    }

    /// The `OrchestrationThreadShell` the project list carries — the same thread
    /// without its transcript, plus the three flags the inbox sorts on.
    ///
    /// All three are `false` here and each is a later ticket's: approvals are
    /// ticket 13, the user-input questions that go with them are the same, and a
    /// proposed plan needs plan mode, which is ticket 12's tool round-trips at
    /// the earliest. A `true` any of them could not be acted on would put a badge
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
            "hasPendingApprovals": false,
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
            created_at: now_iso(),
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
            created_at: now_iso(),
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "tone": self.tone,
            "kind": self.kind,
            "summary": self.summary,
            "payload": self.payload,
            "turnId": self.turn_id,
            "createdAt": self.created_at,
        })
    }
}

impl Session {
    /// `threadId` is a field of the session in the contract and is the key the
    /// client re-attaches it by, so it comes from the thread rather than being
    /// stored twice.
    fn to_value(&self, thread_id: &str) -> Value {
        json!({
            "threadId": thread_id,
            "status": self.status,
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
    fn to_value(&self) -> Value {
        json!({
            "turnId": self.turn_id,
            "state": self.state,
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
}

impl Change {
    /// Does the project list need to hear about this?
    ///
    /// Everything except a delta and an activity. A turn produces hundreds of
    /// deltas and none of them changes anything the thread *list* renders — the
    /// title, the session state, the latest turn — so republishing the summary
    /// per token would be the shell subscription carrying a token stream it has
    /// no use for.
    fn reaches_the_shell(&self) -> bool {
        !matches!(self, Change::AssistantDelta { .. } | Change::Activity(_))
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

impl Threads {
    pub fn new(sequences: Sequences, shell: broadcast::Sender<Value>) -> Threads {
        Threads {
            inner: Arc::new(Inner {
                open: Mutex::new(HashMap::new()),
                sequences,
                shell,
                live_agents: AtomicUsize::new(0),
                reconciled_messages: AtomicUsize::new(0),
                messages_matching_deltas: AtomicUsize::new(0),
            }),
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
        let payload = self.fold(thread, &change, &occurred_at);
        thread.updated_at = occurred_at.clone();

        let event = thread_event(sequence, thread_id, change.event_type(), payload, &occurred_at);
        let summary = change.reaches_the_shell().then(|| thread.to_shell_value());
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
    fn fold(&self, thread: &mut Thread, change: &Change, at: &str) -> Value {
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
                    state: "running",
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
                thread.activities.push(activity.clone());
                json!({
                    "threadId": thread.id,
                    "activity": activity.to_value(),
                })
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

    /// Is a thread here?
    pub fn contains(&self, thread_id: &str) -> bool {
        self.get(thread_id).is_some()
    }

    /// A copy of the thread, for a caller that needs to know what to start an
    /// agent with.
    pub fn get(&self, thread_id: &str) -> Option<Thread> {
        lock(&self.find(thread_id)?.state).clone()
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
        start: impl FnOnce(mpsc::Receiver<Prompt>) -> JoinHandle<()>,
    ) -> mpsc::Sender<Prompt> {
        let entry = self.entry(thread_id);
        let mut live = lock(&entry.live);
        if let Some(running) = live.as_ref() {
            return running.prompts.clone();
        }

        let (prompts, incoming) = mpsc::channel(PROMPT_QUEUE);
        self.inner.live_agents.fetch_add(1, Ordering::Relaxed);
        *live = Some(Live {
            prompts: prompts.clone(),
            task: start(incoming),
        });
        prompts
    }

    /// Called by the driver when its agent has gone, so the next turn starts a
    /// new one rather than writing into a closed pipe.
    ///
    /// Does not wait for the task: this *is* the task, at the end of itself.
    pub fn detach(&self, thread_id: &str) {
        let entry = self.entry(thread_id);
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
        let running: Vec<JoinHandle<()>> = {
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
            Change::Session(_) => "thread.session-set",
            Change::Activity(_) => "thread.activity-appended",
        }
    }
}

/// Move the latest turn on when an assistant message lands.
///
/// `threadReducer.ts`'s rule, and the subtle half of it is why a completed
/// assistant message does *not* end the turn: a provider may send several of
/// them in one turn — commentary between tool calls — so the turn stays running
/// until the session says otherwise. Ticket 12's tool round-trips are exactly
/// the case that breaks if this settles early.
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
        session.status == "running" && session.active_turn_id.as_ref() == Some(turn_id)
    });
    let settles = !streaming && !still_running;
    let previous = latest.as_ref();

    Some(LatestTurn {
        turn_id: turn_id.clone(),
        state: match settles {
            false => "running",
            true => match previous.map(|turn| turn.state) {
                Some("interrupted") => "interrupted",
                Some("error") => "error",
                _ => "completed",
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
    if session.status == "running" {
        if let Some(active) = &session.active_turn_id {
            let previous = latest.as_ref().filter(|turn| &turn.turn_id == active);
            return Some(LatestTurn {
                turn_id: active.clone(),
                state: "running",
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

    let settled = match session.status {
        "idle" | "ready" => "completed",
        "error" => "error",
        "interrupted" => "interrupted",
        // `starting` and `stopped` say nothing about how the turn went, so a
        // running turn stays running rather than being called completed.
        _ => return latest,
    };

    match latest {
        Some(turn) if turn.state == "running" => Some(LatestTurn {
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
        (Threads::new(Sequences::from(0), shell), watching)
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
            session: None,
            latest_turn: None,
            latest_user_message_at: None,
        }
    }

    fn running(turn_id: &str) -> Session {
        Session {
            status: "running",
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

    /// A turn does not end when the assistant stops talking — a provider sends
    /// several messages in one turn — it ends when the session leaves `running`.
    /// Getting this wrong settles a turn in the middle of itself, which is the
    /// failure ticket 12's tool round-trips would hit first.
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
        assert_eq!(turn.as_ref().map(|turn| turn.state), Some("running"));
        assert_eq!(
            turn.as_ref().and_then(|turn| turn.assistant_message_id.clone()),
            Some("assistant-1".to_string())
        );

        threads.apply(
            "thread-1",
            Change::Session(Session {
                status: "ready",
                active_turn_id: None,
                ..running("turn-1")
            }),
        );
        let turn = threads
            .get("thread-1")
            .expect("the thread")
            .latest_turn
            .expect("a turn");
        assert_eq!(turn.state, "completed");
        assert!(turn.completed_at.is_some(), "a completed turn has an end");
        assert!(turn.started_at.is_some(), "and a beginning to measure from");
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
