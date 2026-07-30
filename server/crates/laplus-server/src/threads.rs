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
//! S>C Exit     Failure  OrchestrationGetSnapshotError — no such thread, yet
//! C>S Request  orchestration.dispatchCommand  {"type":"thread.turn.start",…}
//! S>C Exit     Success {"sequence":5}
//! C>S Request  orchestration.subscribeThread  {"threadId":"…"}   (the retry)
//! S>C Chunk    {"kind":"snapshot","snapshot":{"snapshotSequence":5,"thread":{…}}}
//! S>C Chunk    {"kind":"event","event":{"sequence":6,"type":"thread.message-sent",…}}
//! S>C Chunk    {"kind":"event","event":{"sequence":7,"type":"thread.message-sent",…}}
//! ```
//!
//! The refusal at the top is not a failure mode; it is the first half of how a
//! new conversation begins, and [`Threads::subscribe`] is where it is explained.
//! A **snapshot is what makes a subscription mean anything**: the client folds an
//! event only into a thread it already holds, so a stream that never opened with
//! one is a stream it silently discards. Ticket 28.
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
//! Nothing about this is a laplus invention; the client was already written
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
//! - **Worktrees.** `branch` is carried and never acted on. `worktreePath` is
//!   acted on in exactly one way: it says which folder the conversation's work
//!   happens in — the agent's, the checkpoints' and a revert's, all through
//!   [`crate::orchestration`]'s `where_the_work_happens`. That is *obeying* a
//!   worktree the developer made, not *giving* a thread one: a
//!   `thread.turn.start` that asks for one to be prepared is still refused by
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
use crate::transcripts::{Transcripts, Write};

/// The subscription that *is* one conversation.
pub const SUBSCRIBE_THREAD: &str = "orchestration.subscribeThread";

/// A validated `orchestration.subscribeThread` call.
///
/// Read by hand rather than deserialized, because there are three fields and
/// only one of them can make the call unanswerable here: a subscription to a
/// blank thread id would open a stream against a conversation nothing can name.
/// Whether the *named* thread can be answered for is [`Threads::subscribe`]'s
/// question, not this one's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    thread_id: String,
    /// Whether the client asked to be told when it has the whole conversation.
    wants_marker: bool,
    /// Whether the client says it already holds this conversation.
    ///
    /// `afterSequence` is the client's cursor: "I have it up to here, send me
    /// what came after". Its *presence* is what this server reads, and what it
    /// means is the thing ticket 28 turned on — a client that sends one has a
    /// thread to fold events into, and a client that does not has nothing, and
    /// must be sent a snapshot or shown nothing at all.
    ///
    /// So it decides whether an absent thread is a refusal. Refusing a resume
    /// would take a conversation the client can still draw from its own cache
    /// and replace it with an error, and the reference server does not: with a
    /// cursor it replays from the log and never asks whether the thread is there
    /// (`apps/server/src/ws.ts`), which is why the boot capture's
    /// `synchronized`-only opening — the one this server used to give *every*
    /// subscription — is an answer to a resume and to nothing else. See
    /// `fixtures/socket-wire/01-browser-session.ndjson`, request `3`.
    ///
    /// The cursor's *value* decides one thing and not the other. It cannot say
    /// which events to replay — this server keeps no log to replay them from —
    /// but a cursor that is still [`Sequences::caught_up`] asks for a replay of
    /// nothing, and that this server can answer exactly, by opening with no
    /// snapshot. Every other value is answered with the whole conversation,
    /// which is correct because a snapshot replaces what the client holds rather
    /// than being folded into it. See ADR-0016.
    ///
    /// For a thread this server does not hold there is nothing to send whatever
    /// the value is, and the opening carries no snapshot at all — which is the
    /// one case where saying nothing is the whole point, since an empty snapshot
    /// would be a claim that the client's own copy is wrong.
    after: Option<i64>,
}

impl Watch {
    /// Read the payload, or refuse with the error the method declares.
    pub fn read(payload: &Value) -> Result<Watch, Value> {
        let thread_id = payload
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(Watch {
            thread_id: crate::rpc::non_blank(
                thread_id,
                "OrchestrationGetSnapshotError",
                "thread id",
            )?,
            wants_marker: payload
                .get("requestCompletionMarker")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // `null` is not a cursor, and neither is a value the contract's
            // `NonNegativeInt` would have refused. Both leave this a client that
            // holds nothing, which is the answer that sends a snapshot.
            after: crate::rpc::resume_cursor(payload),
        })
    }

    /// Whether the client says it already holds this conversation. The presence
    /// of the cursor, which is the half of it ticket 28 turned on.
    fn resuming(&self) -> bool {
        self.after.is_some()
    }
}

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

pub mod fold;

// What a conversation is and what one change does to it, re-exported so that
// the rest of the crate keeps naming this module for the concept. The seam is
// `docs/adr/0025`; the paths callers use are deliberately unchanged by it.
pub use fold::{
    checkpoint_status, fresh_activity_id, fresh_message_id, fresh_turn_id, settled_override, tone,
    Activity, Adoption, Attention, Busy, Change, Checkpoint, Conversation, Given, LatestTurn,
    Lifecycle, Woken,
    Message, MetaUpdate, Reconciled, Rendered, Session, Shelf, Thread, ThreadRow, TurnState, ACTIVE,
    BY_ACTIVITY, BY_THE_USER, SETTLED,
};
use fold::{durable, fresh_id, Listing};

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
    /// A slot can exist before the thread does, because [`Threads::entry`]
    /// allocates one for any id it is asked to write against and the id arrives
    /// before the thread: a new conversation is a client-side draft, and the
    /// server hears about it only when the first turn is dispatched with a
    /// `bootstrap.createThread`.
    ///
    /// A *subscription* used to be one of the things that allocates one and
    /// mostly no longer is — see [`Threads::subscribe`], which refuses an absent
    /// thread rather than opening on the empty slot it had just made, unless the
    /// client says it holds the conversation already.
    state: Mutex<Option<Thread>>,
    events: broadcast::Sender<Value>,
    /// The running agent's end of the conversation, while there is one.
    live: Mutex<Option<Live>>,
    /// How many sessions this conversation has had. What [`Live::epoch`] is
    /// counted by.
    sessions: AtomicU64,
}

/// A handle on the task driving one session.
#[derive(Debug)]
struct Live {
    /// Which session of this conversation's this is.
    ///
    /// Carried so that a driver ending can tell "my slot" from "the slot the
    /// session after mine is in". [`Threads::stop_session`] frees the slot at
    /// once — that is what lets the next turn start a new session rather than
    /// queueing prompts into a channel nobody is reading — so a driver winding
    /// down afterwards would otherwise [`Threads::detach`] the session that
    /// replaced it, taking a live agent out of the registry and off the gauge
    /// while its child went on running.
    epoch: u64,
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
    /// What the conversation said the agent should be running under when *this*
    /// turn was dispatched. See [`Retune`].
    pub wanted: Retune,
}

/// What the conversation says the agent should be running under, as of the turn
/// this travels with.
///
/// **It travels with the prompt, and the pairing is the whole point.** A mode
/// belongs to one turn — it is the mode that turn was requested under and the
/// mode it has to be answered under — so it cannot live in a slot beside the
/// queue: two turns queued behind a running one, with the picker moved between
/// them, would collapse onto whichever arrived last and the first would be
/// answered under the second's rules.
///
/// Two fields rather than two messages because they are one question asked of
/// one child at one moment, and because the driver has to compare both against
/// its own capture before it says anything: a push for a value that has not
/// moved is a request the CLI has to answer for nothing.
///
/// The model is optional because a selection may name none, and there is no
/// request that means "go back to the default model" — so `None` leaves the
/// child on whatever it has, which is honest about what can be asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retune {
    pub runtime_mode: String,
    pub model: Option<String>,
}

/// Something owed to the turn the agent is working on right now.
///
/// One channel for every kind rather than one each, and the ordering is the
/// reason: a developer who approves a tool and then immediately presses stop
/// means those two things *in that order*, and two channels would leave a
/// `select!` free to take them in either. One channel is also one place where
/// the rule "a signal is never queued behind a prompt" has to be got right.
#[derive(Debug, Clone)]
pub enum Signal {
    /// The developer answered a permission request.
    Answer(Answered),
    /// The developer answered the agent's questions.
    ///
    /// A separate variant rather than a second shape of [`Answered`], because
    /// the two are answers to different questions that happen to travel the same
    /// wire: a permission is one of four decisions this server knows the meaning
    /// of, and this is a map of the developer's own words that it passes through
    /// without reading. See [`crate::worklog::answers_for`].
    AnswerUserInput(UserInputAnswered),
    /// The developer stopped the agent.
    ///
    /// The turn is carried because the client names the one it is looking at,
    /// and a moment is all it takes for that to stop being the one in flight —
    /// the turn it asked about may have finished while the click was travelling.
    /// `None` is the client saying "whatever is running", which is what it sends
    /// when it does not believe anything is.
    Interrupt { turn_id: Option<String> },
    /// The developer ended the session.
    ///
    /// **Not an interrupt, and travelling this channel for the same reason one
    /// does**: it is owed to the turn in flight rather than queued behind it. An
    /// interrupt asks a turn to stop and leaves the child running; this ends the
    /// session, and the driver answers it by leaving its loop — which closes the
    /// agent's stdin, waits for the child, and kills it if waiting was not
    /// enough ([`crate::agent::Agent::stop`]). That bound is the point: the case
    /// this exists for is an agent that is wedged, and closing a pipe at
    /// something wedged is a hope rather than a guarantee.
    ///
    /// It carries no turn. The developer is ending the process, so *which* turn
    /// it was in the middle of does not change what happens to it — unlike an
    /// interrupt, where naming the wrong turn would stop work the developer
    /// never saw start.
    Stop,
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

/// One set of answers on its way to the agent that asked for them.
///
/// The answers are carried as the client sent them and are never inspected here:
/// the contract types them as an open record, the keys are the agent's own
/// question text, and the CLI is what reads them. A server that normalised them
/// would be editing the developer's answer on the way past.
#[derive(Debug, Clone)]
pub struct UserInputAnswered {
    pub request_id: String,
    pub answers: Value,
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
    /// Mostly for callers that are about to *put something in it* — a turn that
    /// is about to create the thread. A question about a thread uses
    /// [`Threads::find`], which does not: a query that quietly allocated would
    /// let any id a client mentions leak a slot.
    ///
    /// Ticket 28 moved subscriptions across that line, and left one behind.
    /// A plain [`Threads::subscribe`] is now a question and asks; a **resume**
    /// still allocates, because it is owed the events an absent conversation
    /// would produce and they arrive on this slot's channel. So a composer
    /// sitting on a draft it never sends costs nothing, where before it left a
    /// slot behind for the life of the process — and the real client
    /// re-subscribes to that draft four times a second.
    fn entry(&self, thread_id: &str) -> Arc<Entry> {
        let mut open = self.lock();
        Arc::clone(open.entry(thread_id.to_string()).or_insert_with(|| {
            Arc::new(Entry {
                state: Mutex::new(None),
                events: broadcast::channel(BACKLOG).0,
                live: Mutex::new(None),
                sessions: AtomicU64::new(0),
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
    ///
    /// **One change can be two events.** Real work in a conversation the
    /// developer had settled returns it to the inbox, and the answer is then the
    /// reset's number rather than the change's — see [`Threads::woken_by`]. Every
    /// caller here is already right for that, because the answer is the log
    /// position reached either way.
    pub fn apply(&self, thread_id: &str, change: Change) -> Option<i64> {
        self.apply_unless(thread_id, |_| None, change)?.ok()
    }

    /// [`Threads::apply`], with the world's own refusal decided under the lock
    /// the fold runs under.
    ///
    /// The two archive commands are why this exists. Whether a conversation is
    /// already archived is a question about the very field the change is about to
    /// move, so asking it under one lock and answering under another would let
    /// two windows both be told they archived one conversation — and the second
    /// of them would have published a `thread.archived` over a thread that was
    /// already put away. `refused` is asked *before* a sequence is taken, so a
    /// refusal costs the log nothing.
    ///
    /// `None` for a conversation this server does not hold, which is
    /// [`Threads::apply`]'s own answer to one; `Some(Err)` for one it holds and
    /// will not move, carrying the sentence a client is shown.
    pub fn apply_unless(
        &self,
        thread_id: &str,
        refused: impl FnOnce(&Thread) -> Option<String>,
        change: Change,
    ) -> Option<Result<i64, String>> {
        let entry = self.find(thread_id)?;
        let sequence = match self.commit(&entry, refused, &change)? {
            Ok(sequence) => sequence,
            Err(why) => return Some(Err(why)),
        };
        // *After* the change that caused it, and after the refusal: a reset
        // published beside a change that was turned away would be a conversation
        // woken by work that did not happen. The answer is the later of the two
        // numbers, which is the shape a turn already answers with when it commits
        // several events — the log position reached, with everything before it
        // already published.
        Some(Ok(self.woken_by(&entry, &change).unwrap_or(sequence)))
    }

    /// Bring a conversation the developer had put out of sight back, because
    /// real work turned up in it.
    ///
    /// The trigger points are [`Change::wakes`], and every one of them is a path
    /// this server already owned — a turn request, a session change, a work-log
    /// append — so these are guarded emissions *beside* events that already fire
    /// rather than a new mechanism.
    ///
    /// [`Woken`] is what says which resets a change spends and what stands in the
    /// way of each. Both guards travel through the refusal [`Threads::commit`]
    /// already takes for the archive commands rather than being asked before the
    /// call: that is what puts them under the lock the fold runs under and before
    /// a sequence is taken, so two triggers arriving at once cannot both find
    /// something to reset and both emit.
    ///
    /// **The answer is the last number reached**, so a message that spends both
    /// answers with the second of them. Each reset is its own event and its own
    /// commit — a client folding one and not the other would hold half a
    /// conversation's lifecycle — and a reset that was refused leaves the number
    /// where it was.
    fn woken_by(&self, entry: &Arc<Entry>, change: &Change) -> Option<i64> {
        let mut reached = None;
        for woken in change.wakes() {
            if let Some(Ok(sequence)) =
                self.commit(entry, |thread| woken.refusal(thread), &woken.reset())
            {
                reached = Some(sequence);
            }
        }
        reached
    }

    /// Fold one change into the conversation this entry holds and publish it.
    ///
    /// [`Threads::apply_unless`] without the wake, so that the wake can be one of
    /// its callers: the reset is a second change against the same conversation,
    /// and going back through `apply_unless` would ask whether *it* wakes
    /// anything.
    fn commit(
        &self,
        entry: &Arc<Entry>,
        refused: impl FnOnce(&Thread) -> Option<String>,
        change: &Change,
    ) -> Option<Result<i64, String>> {
        let mut state = lock(&entry.state);
        let thread = state.as_mut()?;
        if let Some(why) = refused(thread) {
            return Some(Err(why));
        }

        // Under the entry's lock, so two changes to one thread are numbered in
        // the order they are applied; and holding the log until both
        // announcements are out is what keeps them published in that order. A
        // client drops anything at or below the sequence it holds, so a pair
        // that inverted would lose the earlier one permanently.
        let commit = self.inner.sequences.commit();
        let sequence = commit.sequence();
        // The clock, unless the change is one the conversation has already had —
        // see [`Change::re_emitted_at`], where re-emitting rather than refusing is
        // argued, and why such a repeat must report the moment the conversation
        // already carried rather than this one.
        let occurred_at = change.re_emitted_at(thread).unwrap_or_else(now_iso);
        let Rendered {
            payload,
            reconciled,
        } = fold::fold(thread, change, sequence, &occurred_at);
        thread.updated_at = occurred_at.clone();
        // The one thing the fold reports rather than performs: it has no
        // counters and no stderr, so the answer comes back as a value and is
        // spent here. See `docs/adr/0025`.
        if let Some(reconciled) = reconciled {
            self.inner
                .reconciled_messages
                .fetch_add(1, Ordering::Relaxed);
            match reconciled {
                Reconciled::Matched => {
                    self.inner
                        .messages_matching_deltas
                        .fetch_add(1, Ordering::Relaxed);
                }
                Reconciled::Replaced { streamed, buffered } => eprintln!(
                    "laplus: thread {}: the buffered message replaced {streamed} streamed \
                     characters with {buffered}",
                    thread.id,
                ),
            }
        }

        let event = thread_event(sequence, &thread.id, change.event_type(), payload, &occurred_at);
        // Rendered against the conversation the fold has just left behind, so a
        // deletion is a removal and everything after one is nothing at all — see
        // [`Change::on_the_list`], which is where the three cases are argued.
        let listed = change.on_the_list(thread).map(|listing| match listing {
            Listing::Summary => json!({
                "kind": "thread-upserted",
                "sequence": sequence,
                "thread": thread.to_shell_value(),
            }),
            Listing::Removal => json!({
                "kind": "thread-removed",
                "sequence": sequence,
                "threadId": thread.id,
            }),
        });
        // Under the same lock as the fold, so what is written down is what was
        // just folded in and not whatever a later change left behind.
        for write in durable(thread, change) {
            self.inner.transcripts.queue(write);
        }
        drop(state);

        // `send` on a broadcast channel never blocks — it drops the oldest value
        // when the buffer is full and a lagging subscriber is resent a snapshot
        // instead — so publishing here cannot stall the caller.
        let _ = entry.events.send(event);
        if let Some(listed) = listed {
            let _ = self.inner.shell.send(listed);
        }
        Some(Ok(sequence))
    }

    /// Open an `orchestration.subscribeThread` subscription: the thread now,
    /// then every change to it.
    ///
    /// **A thread this server has never heard of is refused, not opened.** The
    /// UI subscribes to a draft before the first prompt has created it, so this
    /// is the common case rather than an edge one — and it is the whole of
    /// ticket 28. A stream that opens on an absent thread and then narrates its
    /// creation is *silently discarded by the real client*: `threads.ts`
    /// (`applyItem`) drops every event while it holds no thread, because it has
    /// no state to fold one into, and only a `snapshot` can give it that state.
    /// So the pane sat on `Working for 3m 22s` while the server streamed it a
    /// correct and complete account of a turn it had already finished.
    ///
    /// Refusing is what the reference server does (`apps/server/src/ws.ts`,
    /// `Thread ${input.threadId} was not found`) and what the client is written
    /// for: `subscribeDynamic` is given `retryExpectedFailureAfter: "250 millis"`
    /// for this subscription, so a refusal is not an error to a draft pane but a
    /// *poll* — the retry that lands after the first prompt creates the thread
    /// opens with a snapshot, and the conversation appears.
    ///
    /// The refusal must therefore be a **declared** error. A defect would fail
    /// every other subscription on the socket rather than this one — see
    /// [`crate::rpc::DispatchError::error_value`].
    ///
    /// A [resume](Watch::after) is the exception, and the reason it exists is
    /// the same rule read the other way: a client that sent a cursor already
    /// holds the conversation, so it has somewhere to put an event and nothing
    /// to be refused for.
    ///
    /// **A conversation the developer deleted is refused on the same footing as
    /// one that was never created**, and the resume exception is unchanged by
    /// that: a client holding the conversation is owed the `thread.deleted` it
    /// has not folded yet, and refusing it would leave that client drawing a
    /// conversation it will never be told is gone. What a fresh subscriber is
    /// spared is a snapshot of something the developer removed — which is the
    /// same sentence a stale window's *commands* are refused with, in
    /// [`crate::orchestration::Shell::dispatch`].
    pub fn subscribe(&self, call: &Watch) -> Result<EventSource, Value> {
        let thread_id = call.thread_id.as_str();
        // Three standings, and the two refusals are one answer in two sentences:
        // there is nothing here a fresh subscriber could draw, and *why* is the
        // whole of what the message can carry. Asked through [`Threads::deleted`]
        // rather than read here, so that the guard on the commands and the guard
        // on the subscriptions are one reading of one field.
        let deleted = self.deleted(thread_id);
        // Asked for rather than allocated, so that a draft nobody ever sends
        // does not leak a slot on the strength of having been looked at.
        let held = self
            .find(thread_id)
            .filter(|entry| !deleted && lock(&entry.state).is_some());
        let entry = match held {
            Some(entry) => entry,
            // A resume does allocate, and it is the one caller that allocates
            // without putting anything in the slot — see [`Threads::entry`]. It
            // has to: the client is owed the events this conversation produces
            // if it turns up, and they will arrive on this slot's channel or
            // nowhere. That leaves a slot behind for a resume of a thread that
            // never exists, which is a few hundred bytes and needs a client that
            // holds a conversation this server does not.
            None if call.resuming() => self.entry(thread_id),
            // Deleted, and said as deleted: a client polling for a draft that is
            // about to exist and one reopening a conversation the developer
            // removed are the same retry loop, and only the sentence can tell
            // the two apart in a log.
            None if deleted => {
                return Err(crate::rpc::declared(
                    "OrchestrationGetSnapshotError",
                    format_args!("Thread {thread_id} was deleted"),
                ))
            }
            // Nothing to describe and nobody who can draw it: say so, and be
            // asked again.
            None => {
                return Err(crate::rpc::declared(
                    "OrchestrationGetSnapshotError",
                    format_args!("Thread {thread_id} was not found"),
                ))
            }
        };
        // Subscribed to before the description closure is handed over, so a
        // change landing between here and the pump's first read arrives as an
        // event rather than falling into the gap — the same ordering
        // [`crate::orchestration::Shell::subscribe`] keeps, and absorbed the
        // same way, by a client that drops anything at or below what it holds.
        let updates = entry.events.subscribe();
        let sequences = self.inner.sequences.clone();
        let marker_owed = AtomicBool::new(call.wants_marker);
        let cursor = call.after;

        Ok(EventSource::new(
            move || {
                let mut items = Vec::new();
                // Conditional twice over, for two unrelated reasons. A client
                // whose cursor is still current is owed no snapshot at all. And
                // the thread existed when the subscription opened — unless this
                // is a resume, which is allowed not to — but this closure runs
                // again whenever a subscriber falls a whole backlog behind, by
                // which point the thread may also have been deleted.
                if !sequences.caught_up(cursor) {
                    if let Some(thread) = lock(&entry.state).as_ref() {
                        items.push(json!({
                            "kind": "snapshot",
                            "snapshot": detail_snapshot(thread, &sequences),
                        }));
                    }
                }
                if marker_owed.swap(false, Ordering::Relaxed) {
                    items.push(json!({"kind": "synchronized"}));
                }
                items
            },
            updates,
        ))
    }

    /// One conversation as `GET /api/orchestration/threads/{threadId}` answers
    /// with it, or `None` for a thread this server does not hold.
    ///
    /// The HTTP half of ticket 31, and the same value the subscription above
    /// opens with — the builder is shared, so the two cannot drift. `None` is
    /// the route's typed `thread_not_found`, and it is the common case rather
    /// than an edge one: a "New thread" pane asks for a draft's snapshot four
    /// times a second before the first prompt brings the thread into being.
    pub fn detail_snapshot(&self, thread_id: &str) -> Option<Value> {
        let entry = self.find(thread_id)?;
        let state = lock(&entry.state);
        Some(detail_snapshot(state.as_ref()?, &self.inner.sequences))
    }

    /// The threads on one of the developer's two shelves, as a snapshot carries
    /// them.
    ///
    /// One builder for both, which is ticket 06's own instruction: the project
    /// list and the archived snapshot are the same object filtered two ways, and
    /// a second builder would let the conversation a client draws depend on which
    /// of them answered.
    pub fn shell_summaries(&self, shelf: Shelf) -> Vec<Value> {
        let entries: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
        let mut summaries: Vec<((String, String), Value)> = entries
            .iter()
            .filter_map(|entry| {
                let state = lock(&entry.state);
                let thread = state.as_ref().filter(|thread| shelf.holds(thread))?;
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

    /// The most recent moment a thread on this shelf changed, for the shell
    /// snapshot's own timestamp.
    ///
    /// Filtered by the same shelf the snapshot's conversations are, because the
    /// field describes *that* snapshot: an archived answer whose `updatedAt` came
    /// from a conversation still on the project list would be reporting a change
    /// to something it does not carry — and would be non-null with nothing
    /// archived at all.
    pub fn latest_change(&self, shelf: Shelf) -> Option<String> {
        let entries: Vec<Arc<Entry>> = self.lock().values().map(Arc::clone).collect();
        entries
            .iter()
            .filter_map(|entry| {
                lock(&entry.state)
                    .as_ref()
                    .filter(|thread| shelf.holds(thread))
                    .map(|thread| thread.updated_at.clone())
            })
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

    /// Has the developer deleted this conversation?
    ///
    /// `false` for one this server does not hold, and that is not an evasion:
    /// "there is no such conversation" is a different sentence with a different
    /// author — the command's own, naming the thread it could not find — and
    /// answering `true` here would put the deletion sentence on a thread nobody
    /// ever created.
    ///
    /// Reads the field rather than cloning the conversation, which is why it is
    /// not [`Threads::get`] with a question after it: this is asked for every
    /// command a client dispatches, and a copy of a long transcript per keystroke
    /// of an agent's turn is not what the answer costs.
    pub fn deleted(&self, thread_id: &str) -> bool {
        self.find(thread_id).is_some_and(|entry| {
            lock(&entry.state)
                .as_ref()
                .is_some_and(|thread| thread.lifecycle.deleted())
        })
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
    /// receives the prompt channel's other end, the signal channel's, and the
    /// number of the session it is about to drive, and is expected to spawn the
    /// task that owns the agent. That number comes back on the way out — see
    /// [`Live::epoch`].
    ///
    /// Synchronous on purpose: the call that dispatches a turn has to answer the
    /// client immediately, so nothing on this path may wait for a process to
    /// exist. Starting the agent happens inside the spawned task, and the first
    /// prompt waits in the channel until it does.
    pub fn attach(
        &self,
        thread_id: &str,
        start: impl FnOnce(mpsc::Receiver<Prompt>, mpsc::Receiver<Signal>, u64) -> JoinHandle<()>,
    ) -> mpsc::Sender<Prompt> {
        let entry = self.entry(thread_id);
        let mut live = lock(&entry.live);
        if let Some(running) = live.as_ref() {
            return running.prompts.clone();
        }

        let (prompts, incoming) = mpsc::channel(PROMPT_QUEUE);
        let (signals, signalled) = mpsc::channel(SIGNAL_QUEUE);
        let epoch = entry.sessions.fetch_add(1, Ordering::Relaxed);
        self.inner.live_agents.fetch_add(1, Ordering::Relaxed);
        *live = Some(Live {
            epoch,
            prompts: prompts.clone(),
            signals,
            task: start(incoming, signalled, epoch),
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

    /// Hand the developer's answers to the agent that asked the questions.
    ///
    /// [`Threads::answer`] beside it, for the other half of what arrives as a
    /// permission request — same channel, same refusals, and the same division of
    /// labour with the driver: this knows whether a session is listening, and the
    /// driver knows whether *this request* is one it asked about.
    ///
    /// The sentences differ because the developer's screen does: someone who has
    /// just typed an answer to a question is not helped by being told a
    /// permission decision could not be delivered.
    pub fn answer_user_input(
        &self,
        thread_id: &str,
        answered: UserInputAnswered,
    ) -> Result<(), String> {
        let Some(running) = self.live(thread_id)? else {
            return Err(
                "No agent is running for this conversation, so there is no question left to \
                 answer."
                    .to_string(),
            );
        };

        running
            .try_send(Signal::AnswerUserInput(answered))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    "The agent has not read what has already been sent to it, so these answers \
                     were not queued."
                        .to_string()
                }
                mpsc::error::TrySendError::Closed(_) => {
                    "The agent session has ended and could not be given these answers.".to_string()
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

    /// End this conversation's session, keeping the conversation.
    ///
    /// Answers whether there was one to end. `false` is not a failure: the
    /// developer asked for no agent to be running and there is none, which is
    /// the state they asked for — the same reading [`Threads::interrupt`] takes
    /// of a turn that is not there. `Err` is reserved for a conversation this
    /// server has never heard of, which is a client naming something that does
    /// not exist.
    ///
    /// **Nothing here is about the turn.** Interrupting asks a running turn to
    /// stop and leaves the child alive; this ends the process, which is a
    /// different act with a different subject — see [`Signal::Stop`], and
    /// `.scratch/thread-lifecycle/issues/04-…`, where the distinction is the
    /// ticket.
    ///
    /// Three things happen, in this order, and each is load-bearing:
    ///
    /// 1. **The slot is freed**, so a turn dispatched a moment later starts a
    ///    *new* session. Leaving it would mean the next turn queued a prompt into
    ///    a channel whose driver had stopped reading, which is a conversation
    ///    waiting forever on a turn nothing was ever handed. The real client does
    ///    exactly this: the branch toolbar stops the session and moves the
    ///    conversation's worktree in the same breath.
    /// 2. **The driver is told**, on the channel it is already listening to. Said
    ///    before the channels are dropped, and it arrives anyway: a closed
    ///    [`mpsc`] still yields what was queued on it before the sender went.
    /// 3. **The handle is parked**, because dropping a [`JoinHandle`] detaches
    ///    the task and a detached driver is one [`Threads::shutdown`] would not
    ///    wait for. The same reasoning, and the same list, as
    ///    [`Threads::forget`]; nothing *waits* here either, because the developer
    ///    is owed an answer rather than a reaping.
    ///
    /// If the signal cannot be queued the dropped prompt channel is still the
    /// older way of saying the same thing — the driver reads it as "no more
    /// turns", closes the agent's stdin and drains what is left. That is a
    /// gentler ending than the signal asks for and it is the right fallback: a
    /// driver with a full signal queue is one that is not reading them.
    pub fn stop_session(&self, thread_id: &str) -> Result<bool, String> {
        let entry = self.find(thread_id).ok_or_else(unknown(thread_id))?;
        // The guard is released with the statement, before the parking list is
        // taken — the lock order [`Threads::forget`] uses, and there is no reason
        // for this to be the one path that inverts it.
        let Some(live) = lock(&entry.live).take() else {
            return Ok(false);
        };
        self.inner.live_agents.fetch_sub(1, Ordering::Relaxed);

        let _ = live.signals.try_send(Signal::Stop);
        drop(live.prompts);
        drop(live.signals);

        let mut winding_down = lock(&self.inner.winding_down);
        winding_down.retain(|driver| !driver.is_finished());
        winding_down.push(live.task);
        Ok(true)
    }

    /// The running session's signal channel, or `None` when nothing is running.
    ///
    /// `Err` is reserved for a conversation this server has never heard of,
    /// which is a client naming something that does not exist rather than
    /// anything about a session.
    fn live(&self, thread_id: &str) -> Result<Option<mpsc::Sender<Signal>>, String> {
        let entry = self.find(thread_id).ok_or_else(unknown(thread_id))?;
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
    ///
    /// `epoch` is the session the caller was driving, and the slot is given up
    /// only if it is still holding *that* one. A driver can also outlive its own
    /// slot: [`Threads::stop_session`] frees it while the child is still being
    /// reaped, and the next turn fills it with a session of its own. See
    /// [`Live::epoch`].
    ///
    /// Answers whether the conversation is **still this session's to describe**,
    /// which is the question a driver has to ask before publishing its ending and
    /// the only place it can ask it — see [`crate::turn`].
    ///
    /// That is not quite "was the slot mine". An *empty* slot is still this
    /// session's to describe: a stop or a shutdown frees it and neither of them
    /// puts another session in the conversation, so the ending is the last true
    /// thing anybody can say about it. `false` is reserved for the two cases where
    /// something else is now the truth — a *newer* session in the slot, and a
    /// conversation that has been forgotten with its project.
    pub fn detach(&self, thread_id: &str, epoch: u64) -> bool {
        let Some(entry) = self.find(thread_id) else {
            return false;
        };
        let mut live = lock(&entry.live);
        match live.as_ref().map(|running| running.epoch) {
            Some(held) if held == epoch => {
                *live = None;
                self.inner.live_agents.fetch_sub(1, Ordering::Relaxed);
                true
            }
            Some(_) => false,
            None => true,
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

/// A command naming a conversation this server has never heard of.
///
/// Said in one place because two of the session commands refuse for it, and the
/// sentence names the thread: a client that asked about something that does not
/// exist has a bug, and which id it sent is the whole of what would find it.
fn unknown(thread_id: &str) -> impl FnOnce() -> String + '_ {
    move || format!("There is no conversation '{thread_id}' on this server.")
}

/// A poisoned lock means a previous holder panicked mid-change. What is behind
/// it is a plain value with no invariant a panic could have broken halfway, so
/// refusing to use it would turn one panic into a dead conversation.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// One conversation and the log position it was read at — the contract's
/// `OrchestrationThreadDetailSnapshot`.
///
/// Lifted out of [`Threads::subscribe`]'s description closure by ticket 31,
/// because the HTTP route and the socket answer with the same object and the
/// client uses whichever of the two it gets. The sequence is read here rather
/// than passed in so that it is taken at the same moment as the thread, which
/// is what makes it safe for the client to fold events from.
fn detail_snapshot(thread: &Thread, sequences: &Sequences) -> Value {
    json!({
        "snapshotSequence": sequences.current(),
        "thread": thread.to_detail_value(),
    })
}

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
/// laplus does not have; inventing ids for them would be describing a
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::settling::SessionStatus;
    use crate::threads::fold::pinned_by;

    /// The conversation fixtures live beside the code that renders them. Re-
    /// exported rather than re-declared: `crate::rpc`'s tests reach `a_thread`
    /// through this path, and one fixture is the point of sharing it.
    pub(crate) use crate::threads::fold::tests::{a_thread, running};

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

    /// A composer opening a conversation: it holds nothing and expects to be
    /// sent everything.
    fn a_watch(id: &str) -> Watch {
        Watch {
            thread_id: id.to_string(),
            wants_marker: true,
            after: None,
        }
    }

    /// A client that already holds the conversation and wants what came after.
    ///
    /// The cursor is one this fixture's log has not reached, which is the case
    /// the tests using this helper are about: a browser that kept its cache
    /// across a server that did not keep its threads. A cursor that *is*
    /// current is a different rule and has its own tests.
    fn a_resume(id: &str) -> Watch {
        Watch {
            after: Some(7),
            ..a_watch(id)
        }
    }

    /// A client that holds everything this server has: the boot case, where an
    /// HTTP snapshot was read a moment ago and nothing has happened since.
    fn a_caught_up_resume(threads: &Threads, id: &str) -> Watch {
        Watch {
            after: Some(threads.inner.sequences.current()),
            ..a_watch(id)
        }
    }

    /// The six keys [`Lifecycle`] writes, as the contract spells them.
    const LIFECYCLE_KEYS: [&str; 6] = [
        "archivedAt",
        "settledOverride",
        "settledAt",
        "snoozedUntil",
        "snoozedAt",
        "deletedAt",
    ];

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

    /// A thread the server has never heard of is a draft the UI is showing, and
    /// its subscription is **refused** — the whole of ticket 28.
    ///
    /// This test used to assert the opposite: that the subscription opened and
    /// stayed silent until a turn created the thread. It did open, and the
    /// events did arrive, and the window rendered none of them, because a client
    /// that never received a snapshot has nothing to fold an event into. The
    /// refusal is what makes the client ask again.
    #[test]
    fn a_subscription_to_a_thread_that_does_not_exist_is_refused() {
        let (threads, _shell) = threads();

        let refusal = threads
            .subscribe(&a_watch("thread-1"))
            .expect_err("a draft has no conversation to describe");
        assert_eq!(refusal["_tag"], "OrchestrationGetSnapshotError");
        assert!(
            refusal["message"]
                .as_str()
                .expect("a message")
                .contains("thread-1"),
            "the refusal names the thread: {refusal}"
        );

        // And the retry that lands after the first prompt created it opens with
        // the conversation, which is what the client renders.
        threads.create(a_thread("thread-1")).expect("created");
        let described = threads
            .subscribe(&a_watch("thread-1"))
            .expect("the thread exists now")
            .describe();
        assert_eq!(described[0]["kind"], "snapshot");
        assert_eq!(described[0]["snapshot"]["thread"]["id"], "thread-1");
        assert!(described[0]["snapshot"]["snapshotSequence"].is_i64());
        // The conversation first and the marker after it: the client is told it
        // is synchronized once there is something to be synchronized with.
        assert_eq!(
            described.iter().map(|item| &item["kind"]).collect::<Vec<_>>(),
            vec!["snapshot", "synchronized"],
            "{described:#?}"
        );
    }

    /// A client resuming a conversation *it* still holds is not refused, even
    /// though this server no longer has one — a registry that was reset under a
    /// browser that kept its cache.
    ///
    /// The refusal exists to make a client that has nothing ask again. This one
    /// has something, so it is given the feed and left to draw what it holds,
    /// which is what the reference server does with a cursor in hand.
    #[test]
    fn a_resume_is_opened_even_for_a_thread_this_server_does_not_have() {
        let (threads, _shell) = threads();

        let opening = threads
            .subscribe(&a_resume("thread-1"))
            .expect("a resume is not refused")
            .describe();
        assert_eq!(
            opening,
            vec![json!({"kind": "synchronized"})],
            "there is nothing to describe, and saying so falsely would wipe the client's copy"
        );
    }

    /// The case ADR-0016 is about: the client read this conversation over HTTP
    /// a moment ago and nothing has happened since, so the replay it asked for
    /// is a replay of nothing and the socket carries no second copy.
    ///
    /// This is the whole of the saving. Everything else about the subscription
    /// is unchanged, which is why the marker still arrives — the client is owed
    /// "you are up to date" precisely when it is.
    #[test]
    fn a_cursor_that_is_still_current_opens_without_a_snapshot() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let opening = threads
            .subscribe(&a_caught_up_resume(&threads, "thread-1"))
            .expect("the thread exists")
            .describe();
        assert_eq!(
            opening,
            vec![json!({"kind": "synchronized"})],
            "the client already holds this conversation: {opening:#?}"
        );
    }

    /// The two ways a cursor can fail to be current, and they are answered the
    /// same way because this server can do nothing else: it keeps no log to
    /// replay from, and a snapshot replaces whatever the client holds.
    ///
    /// *Behind* is an ordinary client that fell out of date between reading the
    /// snapshot and opening the socket. *Ahead* is not an early client but one
    /// holding a number from a previous run — the counter resumes from the last
    /// durable write, so every number issued after it is handed out again. That
    /// second case is the one upstream guards with `replayGap < 0`
    /// (`apps/server/src/ws.ts`), and it is the reason this comparison is
    /// equality rather than "at least".
    #[test]
    fn a_cursor_this_server_cannot_replay_from_is_answered_with_the_conversation() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        let current = threads.inner.sequences.current();

        for (cursor, case) in [(current - 1, "behind"), (current + 1, "ahead")] {
            let opening = threads
                .subscribe(&Watch {
                    after: Some(cursor),
                    ..a_watch("thread-1")
                })
                .expect("the thread exists")
                .describe();
            assert_eq!(
                opening.iter().map(|item| &item["kind"]).collect::<Vec<_>>(),
                vec!["snapshot", "synchronized"],
                "a cursor {case} of {current}: {opening:#?}"
            );
        }
    }

    /// The invariant that lets the cursor be re-read rather than remembered.
    ///
    /// `describe` runs again whenever a subscriber falls a whole backlog behind,
    /// and that second description must be a snapshot even though the first one
    /// was skipped. Nothing tracks which call this is: every event carries a
    /// number taken from `Sequences`, so falling behind is *itself* what makes
    /// the cursor stale. This test is what would fail if an event were ever
    /// published without taking one.
    #[test]
    fn a_subscription_that_opened_caught_up_is_re_described_once_it_has_not() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let source = threads
            .subscribe(&a_caught_up_resume(&threads, "thread-1"))
            .expect("the thread exists");
        assert_eq!(source.describe(), vec![json!({"kind": "synchronized"})]);

        threads
            .apply(
                "thread-1",
                Change::UserMessage {
                    message_id: "message-1".to_string(),
                    text: "hello".to_string(),
                    turn_id: "turn-1".to_string(),
                },
            )
            .expect("applied");

        let again = source.describe();
        assert_eq!(again[0]["kind"], "snapshot", "{again:#?}");
        assert_eq!(again[0]["snapshot"]["thread"]["id"], "thread-1");
    }

    /// Ticket 31's HTTP route and the subscription describe a conversation with
    /// the same builder, so the client cannot be shown two versions of it
    /// depending on which transport answered first.
    ///
    /// The absent cases are the ones worth naming. A thread nobody has
    /// mentioned has no slot; a thread a *resume* mentioned has an empty one —
    /// and both have to read as "not here", because a snapshot of an empty slot
    /// would be this server claiming a conversation the client still holds is
    /// gone.
    #[test]
    fn the_route_and_the_subscription_describe_a_thread_identically() {
        let (threads, _shell) = threads();

        assert_eq!(threads.detail_snapshot("thread-1"), None);
        threads
            .subscribe(&a_resume("thread-1"))
            .expect("a resume allocates the slot without filling it");
        assert_eq!(
            threads.detail_snapshot("thread-1"),
            None,
            "an empty slot is not a conversation"
        );

        threads.create(a_thread("thread-1")).expect("created");
        let over_socket = threads
            .subscribe(&a_watch("thread-1"))
            .expect("the thread exists")
            .describe();
        assert_eq!(
            threads.detail_snapshot("thread-1"),
            Some(over_socket[0]["snapshot"].clone())
        );
    }

    /// The snapshot is taken at the highest number handed out, so every event
    /// issued after it is strictly newer. A snapshot numbered above its
    /// successors would have the client drop them.
    #[test]
    fn a_snapshot_is_older_than_every_event_that_follows_it() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        let snapshot = threads
            .subscribe(&a_watch("thread-1"))
            .expect("the thread exists")
            .describe();
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
        assert!(threads.shell_summaries(Shelf::Working).is_empty());
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
            "snoozedUntil",
            "snoozedAt",
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
            "snoozedUntil",
            "snoozedAt",
            // `deletedAt` is deliberately not here: the contract's
            // `OrchestrationThreadShell` does not declare it, and this test is
            // about the keys it does. That the summary carries it anyway is
            // argued on `Lifecycle::write_onto` and pinned by
            // [`both_renderings_read_the_lifecycle_from_one_shape`].
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

    /// Ticket 01 of the thread-lifecycle effort: the six fields are one shape
    /// emitted twice rather than two lists of literals that happen to agree.
    ///
    /// Asserted as "the two renderings say the same thing" rather than by
    /// reading each value twice, because the failure this guards is a field
    /// reaching one of them and not the other.
    #[test]
    fn both_renderings_read_the_lifecycle_from_one_shape() {
        let (threads, _shell) = threads();
        let mut thread = a_thread("thread-1");
        thread.lifecycle = Lifecycle {
            archived_at: Some("2026-07-30T09:00:00.000Z".to_string()),
            settled_override: Some("settled"),
            settled_at: Some("2026-07-30T09:01:00.000Z".to_string()),
            snoozed_until: Some("2026-07-31T09:00:00.000Z".to_string()),
            snoozed_at: Some("2026-07-30T09:02:00.000Z".to_string()),
            deleted_at: Some("2026-07-30T09:03:00.000Z".to_string()),
        };
        threads.create(thread).expect("created");
        let thread = threads.get("thread-1").expect("the thread");

        let detail = thread.to_detail_value();
        let summary = thread.to_shell_value();
        for key in LIFECYCLE_KEYS {
            assert!(!detail[key].is_null(), "the detail dropped {key}: {detail}");
            assert_eq!(detail[key], summary[key], "{key} disagrees");
        }
        assert_eq!(detail["settledOverride"], "settled");
        assert_eq!(detail["snoozedUntil"], "2026-07-31T09:00:00.000Z");
    }

    /// A thread nothing has been done to reports all six as `null` — the same
    /// answer a row written before the columns existed gives back, which is what
    /// makes the two indistinguishable.
    #[test]
    fn a_thread_nothing_has_been_done_to_is_null_on_all_six() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        let thread = threads.get("thread-1").expect("the thread");

        for key in LIFECYCLE_KEYS {
            assert_eq!(thread.to_detail_value()[key], Value::Null, "{key}");
            assert_eq!(thread.to_shell_value()[key], Value::Null, "{key}");
        }
    }

    // -- idempotence by re-emission ------------------------------------------

    /// Settling twice lands on the same state and does not move the conversation
    /// in a list ordered by when things changed. The second command is answered
    /// rather than refused — both directions are a standing answer rather than a
    /// move between two lists — but it reports the moment the conversation
    /// already carried.
    #[test]
    fn a_repeated_settle_re_emits_without_moving_anything() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply("thread-1", Change::Settled)
            .expect("settled");
        let settled = threads.get("thread-1").expect("the thread");

        threads
            .apply("thread-1", Change::Settled)
            .expect("settled again");
        let again = threads.get("thread-1").expect("the thread");

        assert_eq!(again.lifecycle, settled.lifecycle, "the state moved");
        assert_eq!(
            again.updated_at, settled.updated_at,
            "a double-click reordered the developer's list"
        );
    }

    /// The unsettle side of the same rule, and the asymmetry beside it: a *user*
    /// unsettle pins the conversation active rather than clearing the override to
    /// neutral, so the client's own auto-settle stays suppressed until real work
    /// moves it on.
    #[test]
    fn a_user_unsettle_pins_the_conversation_active_and_repeats_harmlessly() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply("thread-1", Change::Unsettled { reason: BY_THE_USER })
            .expect("unsettled");
        let pinned = threads.get("thread-1").expect("the thread");
        assert_eq!(pinned.lifecycle.settled_override, Some(ACTIVE));
        assert_eq!(pinned.lifecycle.settled_at, None);

        threads
            .apply("thread-1", Change::Unsettled { reason: BY_THE_USER })
            .expect("unsettled again");
        let again = threads.get("thread-1").expect("the thread");
        assert_eq!(again.lifecycle, pinned.lifecycle);
        assert_eq!(again.updated_at, pinned.updated_at);
    }

    /// A settle after something else touched the conversation keeps the moment it
    /// first settled, and moves neither stamp. The case a repeat that re-read the
    /// clock would get wrong in two places at once.
    #[test]
    fn a_settle_repeated_after_an_unrelated_change_keeps_both_stamps() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply("thread-1", Change::Settled)
            .expect("settled");
        let settled_at = threads
            .get("thread-1")
            .expect("the thread")
            .lifecycle
            .settled_at
            .expect("a settle says when");

        threads
            .apply(
                "thread-1",
                Change::MetaUpdated(MetaUpdate {
                    title: Some("Renamed".to_string()),
                    model_selection: None,
                    branch: None,
                    worktree_path: None,
                }),
            )
            .expect("renamed");
        let renamed_at = threads.get("thread-1").expect("the thread").updated_at;

        threads
            .apply("thread-1", Change::Settled)
            .expect("settled again");
        let again = threads.get("thread-1").expect("the thread");
        assert_eq!(
            again.lifecycle.settled_at,
            Some(settled_at),
            "the repeat restamped when the conversation settled"
        );
        assert_eq!(
            again.updated_at, renamed_at,
            "the repeat moved the conversation in a list it had not changed in"
        );
    }

    /// A wake time, in the one shape this wire renders — the shape
    /// `Date.toISOString()` produces, which is what the client sends.
    const WAKE: &str = "2026-07-31T09:00:00.000Z";

    /// The same rule for snooze, and it is keyed on the *wake time* rather than
    /// on being snoozed at all: a second snooze to the moment the conversation is
    /// already asleep until is the double-click, and it must not reorder a list.
    #[test]
    fn a_repeated_snooze_re_emits_without_moving_anything() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        let snoozed = threads.get("thread-1").expect("the thread");
        assert_eq!(snoozed.lifecycle.snoozed_until, Some(WAKE.to_string()));
        assert!(snoozed.lifecycle.snoozed_at.is_some(), "a snooze says when");

        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed again");
        let again = threads.get("thread-1").expect("the thread");
        assert_eq!(again.lifecycle, snoozed.lifecycle, "the state moved");
        assert_eq!(
            again.updated_at, snoozed.updated_at,
            "a double-click reordered the developer's list"
        );
    }

    /// Choosing a *different* time is a new decision, not a repeat: both stamps
    /// move.
    ///
    /// `snoozedAt` moving is the half that matters and it is not tidiness. The
    /// client measures a raised hand against it — a session that failed or a turn
    /// that completed *after* the snooze wakes the conversation early
    /// (`threadRaisedHandWhileSnoozed`) — so a second snooze that kept the first
    /// one's stamp would be woken immediately by the work the developer had just
    /// decided to sleep through.
    #[test]
    fn snoozing_to_a_new_time_is_a_new_decision_and_restamps_both() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        let first = threads.get("thread-1").expect("the thread");

        let later = "2026-08-01T09:00:00.000Z";
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: later.to_string(),
                },
            )
            .expect("snoozed later");
        let second = threads.get("thread-1").expect("the thread");

        assert_eq!(second.lifecycle.snoozed_until, Some(later.to_string()));
        // The two stamps are one moment, which is what a snooze that took the
        // clock looks like. It is not the whole of the pin — two calls this
        // close together can land in the same millisecond, so *comparing* the
        // two snoozes would assert the clock's resolution rather than this
        // rule. `a_snooze_to_another_time_is_not_a_repeat_of_the_first` is
        // where it is decided, and it needs no clock at all.
        assert_eq!(
            second.lifecycle.snoozed_at,
            Some(second.updated_at.clone()),
            "the second snooze's two stamps came from different moments"
        );
        assert!(second.updated_at >= first.updated_at);
    }

    /// Waking a conversation nobody snoozed lands on the state it is already in,
    /// and reports the moment it already carried — the unsettle half of the rule,
    /// asked about the other pair of fields.
    #[test]
    fn waking_a_conversation_that_is_not_snoozed_moves_nothing() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        let before = threads.get("thread-1").expect("the thread");

        threads
            .apply(
                "thread-1",
                Change::Unsnoozed {
                    reason: BY_THE_USER,
                },
            )
            .expect("woken");
        let after = threads.get("thread-1").expect("the thread");

        assert_eq!(after.lifecycle, before.lifecycle);
        assert_eq!(after.updated_at, before.updated_at);
    }

    /// Waking a snoozed conversation clears both fields rather than one.
    ///
    /// A `snoozedAt` left behind would be a conversation the client reads as
    /// never having been snoozed and this server reads as snoozed at a moment it
    /// no longer is — and `threadWokeAt` renders that stamp into a "Woke"
    /// indicator the developer already dealt with.
    #[test]
    fn waking_by_hand_clears_both_stamps() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");

        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        threads
            .apply(
                "thread-1",
                Change::Unsnoozed {
                    reason: BY_THE_USER,
                },
            )
            .expect("woken");

        let woken = threads.get("thread-1").expect("the thread");
        assert_eq!(woken.lifecycle.snoozed_until, None);
        assert_eq!(woken.lifecycle.snoozed_at, None);
    }

    // -- deleting is soft ----------------------------------------------------

    /// A deletion stamps one field and leaves the conversation where it is —
    /// which is the whole of what makes it recoverable, and what keeps the git
    /// refs its turns wrote from being orphaned.
    #[test]
    fn deleting_stamps_the_field_and_moves_nothing_else() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply(
                "thread-1",
                Change::UserMessage {
                    message_id: "message-1".to_string(),
                    text: "hello".to_string(),
                    turn_id: "turn-1".to_string(),
                },
            )
            .expect("said");
        let before = threads.get("thread-1").expect("the thread");

        threads.apply("thread-1", Change::Deleted).expect("deleted");
        let after = threads.get("thread-1").expect("the thread");

        assert_eq!(
            after.lifecycle.deleted_at,
            Some(after.updated_at.clone()),
            "the deletion is stamped at the moment the change committed"
        );
        assert_eq!(
            after.lifecycle,
            Lifecycle {
                deleted_at: after.lifecycle.deleted_at.clone(),
                ..before.lifecycle
            },
            "a delete moved a lifecycle field that is not its own"
        );
        assert_eq!(
            after.messages.len(),
            before.messages.len(),
            "the transcript went with the deletion"
        );
    }

    /// A deleted conversation is on neither of the developer's lists.
    ///
    /// Both, and the archived one is the half that had to be checked against the
    /// client rather than assumed: the settings panel takes the archived snapshot
    /// whole and groups it by project, filtering on neither field, so a
    /// conversation archived and then deleted would be drawn there with an
    /// unarchive control on it.
    #[test]
    fn a_deleted_conversation_is_on_neither_shelf() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply("thread-1", Change::Archived)
            .expect("archived");
        assert_eq!(threads.shell_summaries(Shelf::Archived).len(), 1);

        threads.apply("thread-1", Change::Deleted).expect("deleted");

        assert!(threads.shell_summaries(Shelf::Working).is_empty());
        assert!(threads.shell_summaries(Shelf::Archived).is_empty());
        assert_eq!(
            threads.latest_change(Shelf::Archived),
            None,
            "the archived snapshot is timestamped by a conversation it does not carry"
        );
    }

    /// The project list is told the conversation has *gone*, and is told nothing
    /// about it afterwards.
    ///
    /// A summary would not do: `OrchestrationThreadShell` does not declare
    /// `deletedAt`, so a client cannot filter a deleted conversation out of the
    /// list the way it filters an archived one. And a session change arriving
    /// afterwards — an agent deleting does not stop — would upsert the
    /// conversation straight back onto the list the removal just took it off.
    #[test]
    fn the_project_list_is_told_a_removal_and_then_nothing() {
        let (threads, mut shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        let _ = last_upsert(&mut shell);

        let sequence = threads.apply("thread-1", Change::Deleted).expect("deleted");

        let removal = last_upsert(&mut shell).expect("the list was told");
        assert_eq!(removal["kind"], "thread-removed");
        assert_eq!(removal["sequence"], sequence);
        assert_eq!(removal["threadId"], "thread-1");
        assert!(
            removal.get("thread").is_none(),
            "a removal carries an id and no summary: {removal}"
        );

        threads.apply("thread-1", Change::Session(running("turn-1")));
        assert!(
            last_upsert(&mut shell).is_none(),
            "an agent still winding down put the conversation back on the list"
        );
    }

    /// The conversation's own feed still carries the deletion, because the client
    /// that is looking at it is exactly the one that has to be told.
    ///
    /// The payload is the contract's two keys: a `deletedAt` and no `updatedAt`,
    /// alone among the lifecycle events, because the reducer keeps none of the
    /// thread after folding it.
    #[test]
    fn the_deletion_is_published_on_the_conversations_own_feed() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        threads.create(a_thread("thread-1")).expect("created");
        let mut watching = entry.events.subscribe();

        threads.apply("thread-1", Change::Deleted).expect("deleted");

        let published = events(&mut watching);
        assert_eq!(published.len(), 1, "{published:#?}");
        let event = &published[0]["event"];
        assert_eq!(event["type"], "thread.deleted");
        assert_eq!(event["payload"]["threadId"], "thread-1");
        assert_eq!(
            event["payload"]["deletedAt"],
            json!(threads.get("thread-1").expect("the thread").updated_at)
        );
        assert!(
            event["payload"].get("updatedAt").is_none(),
            "ThreadDeletedPayload carries two keys: {event}"
        );
    }

    /// Work in a deleted conversation does not bring it back.
    ///
    /// Reachable rather than theoretical: deleting does not stop a session, so an
    /// agent still winding down behind a deleted conversation goes on producing
    /// exactly the changes that reset an override. `Shelf::holds` is what refuses
    /// both resets, which is the same reading that keeps the conversation off
    /// both lists — a wake here would move a conversation the developer can no
    /// longer see, and no command could put the override back.
    #[test]
    fn work_in_a_deleted_conversation_does_not_wake_it() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply("thread-1", Change::Settled)
            .expect("settled");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        threads.apply("thread-1", Change::Deleted).expect("deleted");
        let mut watching = entry.events.subscribe();

        // The agent that was already running says it is working, and then asks
        // the developer something — two of the three triggers, and the two that
        // can still arrive once the commands are refused.
        threads.apply("thread-1", Change::Session(running("turn-1")));
        threads.apply(
            "thread-1",
            Change::Activity(crate::worklog::requested(&a_permission(), None)),
        );

        assert_eq!(
            published(&mut watching),
            vec!["thread.session-set", "thread.activity-appended"],
            "work in a deleted conversation published a lifecycle reset"
        );
        let deleted = threads.get("thread-1").expect("the thread");
        assert_eq!(deleted.lifecycle.settled_override, Some(SETTLED));
        assert_eq!(deleted.lifecycle.snoozed_until, Some(WAKE.to_string()));
    }

    /// A fresh subscription to a deleted conversation is refused; one that says
    /// it already holds the conversation is not.
    ///
    /// The resume rule is unchanged by the deletion, and that is the point: a
    /// client holding the conversation is owed the `thread.deleted` it has not
    /// folded yet, and refusing it would leave that client drawing a conversation
    /// it will never be told is gone.
    #[test]
    fn a_subscription_to_a_deleted_conversation_is_refused_unless_it_is_a_resume() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Deleted).expect("deleted");

        let refusal = threads
            .subscribe(&a_watch("thread-1"))
            .expect_err("a deleted conversation is not described");
        assert_eq!(refusal["_tag"], "OrchestrationGetSnapshotError");
        assert!(
            refusal["message"]
                .as_str()
                .expect("a message")
                .contains("deleted"),
            "{refusal}"
        );

        assert!(
            threads.subscribe(&a_resume("thread-1")).is_ok(),
            "a client that holds the conversation was refused its own events"
        );
    }

    // -- lifecycle resets on real activity -----------------------------------
    //
    // Leaving the inbox must never hide something that needs the developer.
    // [`Busy`] refuses to create that state when the developer asks; these are
    // what stop it being reachable a minute later. They live here rather than at
    // the socket because two of the three cannot be reached from there: a turn
    // request resets the override before the session or the work log gets a chance
    // to, so through this server's own dispatch the later two triggers always find
    // nothing to reset. `tests/socket_activity_resets.rs` drives the first.

    /// A permission request as the module that writes them writes it — so the
    /// wiring under test is the same constant [`crate::worklog`] reads.
    fn a_permission() -> crate::protocol::Permission {
        crate::protocol::Permission {
            request_id: "req-1".to_string(),
            tool_name: "Bash".to_string(),
            input: json!({"command": "ls"}),
            tool_use_id: Some("toolu_1".to_string()),
            description: None,
            suggestions: Vec::new(),
        }
    }

    /// Everything published on a conversation's own feed since this was last read.
    fn events(watching: &mut broadcast::Receiver<Value>) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(item) = watching.try_recv() {
            seen.push(item);
        }
        seen
    }

    /// The same, as the event types alone.
    fn published(watching: &mut broadcast::Receiver<Value>) -> Vec<String> {
        events(watching)
            .iter()
            .map(|item| {
                item["event"]["type"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The last summary the project list was sent.
    fn last_upsert(shell: &mut broadcast::Receiver<Value>) -> Option<Value> {
        let mut last = None;
        while let Ok(item) = shell.try_recv() {
            last = Some(item);
        }
        last
    }

    fn a_turn_request() -> Change {
        Change::TurnRequested {
            turn_id: "turn-1".to_string(),
            message_id: "message-1".to_string(),
            model_selection: None,
            runtime_mode: None,
            interaction_mode: None,
        }
    }

    fn override_on(threads: &Threads, id: &str) -> Option<&'static str> {
        threads
            .get(id)
            .expect("the thread")
            .lifecycle
            .settled_override
    }

    /// A turn the developer asked for resets either override, and the reset is
    /// published *after* the change that caused it, at its own number.
    ///
    /// Both directions through one event: a settled conversation comes back, and
    /// one the developer pinned *active* returns to neutral so it can settle
    /// itself again once the burst of work goes stale.
    #[test]
    fn a_turn_request_resets_either_override_to_neutral() {
        let (threads, mut shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");

        for standing in [Change::Settled, Change::Unsettled { reason: BY_THE_USER }] {
            threads.apply("thread-1", standing).expect("an override");
            assert!(override_on(&threads, "thread-1").is_some());

            let _ = published(&mut watching);
            while shell.try_recv().is_ok() {}

            let answered = threads
                .apply("thread-1", a_turn_request())
                .expect("the turn was requested");
            assert_eq!(override_on(&threads, "thread-1"), None);

            // The reset follows the turn rather than preceding it, and the answer
            // is the later of the two numbers — the log position reached, which is
            // the shape a turn already answers with when it commits several
            // events.
            let seen = published(&mut watching);
            assert_eq!(
                seen,
                vec!["thread.turn-start-requested", "thread.unsettled"],
                "the reset did not accompany the turn it was caused by"
            );
            let reset = last_upsert(&mut shell).expect("the project list heard the reset");
            assert_eq!(reset["kind"], "thread-upserted");
            assert_eq!(reset["sequence"], json!(answered));
            assert_eq!(reset["thread"]["settledOverride"], Value::Null);
            assert_eq!(reset["thread"]["settledAt"], Value::Null);
        }
    }

    /// A conversation with no override is left alone, which is the guard
    /// [`Change::re_emitted_at`] asks for: a reset over a conversation with
    /// nothing to reset would land there as a repeat and publish a no-op event at
    /// a stale `updatedAt`.
    #[test]
    fn work_in_a_conversation_with_no_override_publishes_no_reset() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        let _ = published(&mut watching);

        for work in [
            a_turn_request(),
            Change::Session(running("turn-1")),
            Change::Activity(crate::worklog::requested(&a_permission(), None)),
        ] {
            threads.apply("thread-1", work).expect("applied");
        }

        let seen = published(&mut watching);
        assert!(
            !seen.iter().any(|kind| kind == "thread.unsettled"),
            "a conversation nobody had settled was woken anyway: {seen:?}"
        );
    }

    /// A session wakes a settled conversation only while the agent is *working*.
    ///
    /// `ready`, `stopped` and `error` are a status arriving after the fact, and
    /// one of those must not fight the developer's explicit settle — settling a
    /// conversation whose agent has just finished is the ordinary case, and a
    /// `ready` a moment later would undo it.
    #[test]
    fn only_a_session_that_is_working_wakes_a_settled_conversation() {
        for after_the_fact in [
            SessionStatus::Ready,
            SessionStatus::Stopped,
            SessionStatus::Error,
            SessionStatus::Idle,
            SessionStatus::Interrupted,
        ] {
            let (threads, _shell) = threads();
            threads.create(a_thread("thread-1")).expect("created");
            threads.apply("thread-1", Change::Settled).expect("settled");

            threads
                .apply(
                    "thread-1",
                    Change::Session(Session {
                        status: after_the_fact,
                        active_turn_id: None,
                        ..running("turn-1")
                    }),
                )
                .expect("the session changed");
            assert_eq!(
                override_on(&threads, "thread-1"),
                Some(SETTLED),
                "{} undid the developer's settle",
                after_the_fact.as_str()
            );
        }

        for coming_alive in [SessionStatus::Starting, SessionStatus::Running] {
            let (threads, _shell) = threads();
            threads.create(a_thread("thread-1")).expect("created");
            threads.apply("thread-1", Change::Settled).expect("settled");

            threads
                .apply(
                    "thread-1",
                    Change::Session(Session {
                        status: coming_alive,
                        ..running("turn-1")
                    }),
                )
                .expect("the session changed");
            assert_eq!(
                override_on(&threads, "thread-1"),
                None,
                "{} left the conversation out of the inbox with an agent working in it",
                coming_alive.as_str()
            );
        }
    }

    /// A request that blocks on the developer wakes a settled conversation; an
    /// ordinary work-log row does not.
    ///
    /// This is the hole ticket 07 shipped knowingly: a conversation settled while
    /// quiet whose agent then asks for permission would sit outside the inbox
    /// while blocked on a decision only the developer can make. The negative half
    /// matters as much — a turn produces dozens of tool rows, and a conversation
    /// woken by each is one the developer cannot let go of.
    #[test]
    fn a_request_that_blocks_on_the_developer_wakes_it_and_a_tool_call_does_not() {
        let asked = crate::worklog::user_input_requested(
            &a_permission(),
            vec![json!({"id": "Which database?", "question": "Which database?"})],
            None,
        );

        for blocking in [crate::worklog::requested(&a_permission(), None), asked] {
            let (threads, mut shell) = threads();
            threads.create(a_thread("thread-1")).expect("created");
            threads.apply("thread-1", Change::Settled).expect("settled");

            threads
                .apply(
                    "thread-1",
                    Change::Activity(Activity::tool("tool.invoked", "Read", json!({}), None)),
                )
                .expect("a tool row");
            assert_eq!(
                override_on(&threads, "thread-1"),
                Some(SETTLED),
                "ordinary work woke a conversation the developer had finished with"
            );

            let kind = blocking.kind.clone();
            let _ = last_upsert(&mut shell);
            let answered = threads
                .apply("thread-1", Change::Activity(blocking))
                .expect("the request");
            assert_eq!(
                override_on(&threads, "thread-1"),
                None,
                "{kind} was left waiting outside the inbox"
            );

            // The list hears the reset even though it does not hear the row that
            // caused it — an activity does not reach the shell
            // ([`Change::reaches_the_shell`]) and this is how the conversation
            // reappears, carrying the raised hand the same summary now reports.
            let listed = last_upsert(&mut shell).expect("the project list heard the reset");
            assert_eq!(listed["sequence"], json!(answered));
            assert_eq!(listed["thread"]["settledOverride"], Value::Null);
        }
    }

    /// An archived conversation is not woken, however much work turns up in it.
    ///
    /// [`Shelf::holds`] is asked here as well as by both settle commands, so the
    /// filter and the rule stay one rule: there is no inbox to return an archived
    /// conversation to, and clearing an override `thread.unsettle` itself refuses
    /// to touch would lose the developer's decision the moment they unarchived it.
    /// The safety this ticket is about is not weakened — the client's
    /// `effectiveSettled` checks its activity blockers before it reads either
    /// field, so a conversation unarchived while its agent is busy does not
    /// classify as settled regardless.
    #[test]
    fn an_archived_conversation_keeps_its_inbox_state_through_real_work() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Settled).expect("settled");
        threads
            .apply("thread-1", Change::Archived)
            .expect("archived");

        for work in [
            a_turn_request(),
            Change::Session(running("turn-1")),
            Change::Activity(crate::worklog::requested(&a_permission(), None)),
        ] {
            threads.apply("thread-1", work).expect("applied");
            assert_eq!(
                override_on(&threads, "thread-1"),
                Some(SETTLED),
                "work in an archived conversation cleared a decision no command could"
            );
        }

        // And it comes back as the developer left it.
        threads
            .apply("thread-1", Change::Unarchived)
            .expect("unarchived");
        assert_eq!(override_on(&threads, "thread-1"), Some(SETTLED));
    }

    /// The reset carries the server's own reason and never the user's, which is
    /// the whole of why it cannot be forged: `user` *pins* a conversation active,
    /// and a reset that carried it would hold in the inbox a conversation nobody
    /// had asked to hold there.
    #[test]
    fn a_reset_carries_the_neutral_reason() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Settled).expect("settled");
        while watching.try_recv().is_ok() {}

        threads.apply("thread-1", a_turn_request()).expect("a turn");

        let reasons: Vec<Value> = events(&mut watching)
            .iter()
            .filter(|item| item["event"]["type"] == "thread.unsettled")
            .map(|item| item["event"]["payload"]["reason"].clone())
            .collect();
        assert_eq!(reasons, vec![json!(BY_ACTIVITY)]);
        assert_eq!(
            pinned_by(BY_ACTIVITY),
            None,
            "the neutral reason pinned the conversation instead of releasing it"
        );
    }

    /// Snoozing a conversation and then sending it a message spends the return
    /// ticket: the developer came back of their own accord, so there is nothing
    /// left to bring them back to.
    #[test]
    fn a_new_message_spends_the_return_ticket() {
        let (threads, mut shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        let _ = published(&mut watching);
        while shell.try_recv().is_ok() {}

        let answered = threads
            .apply("thread-1", a_turn_request())
            .expect("the turn was requested");

        let lifecycle = threads.get("thread-1").expect("the thread").lifecycle;
        assert_eq!(lifecycle.snoozed_until, None);
        assert_eq!(lifecycle.snoozed_at, None);
        assert_eq!(
            published(&mut watching),
            vec!["thread.turn-start-requested", "thread.unsnoozed"],
            "the wake did not accompany the message that caused it"
        );
        let listed = last_upsert(&mut shell).expect("the project list heard the wake");
        assert_eq!(listed["sequence"], json!(answered));
        assert_eq!(listed["thread"]["snoozedUntil"], Value::Null);
        assert_eq!(listed["thread"]["snoozedAt"], Value::Null);
    }

    /// A session coming alive and an agent raising its hand leave the snooze
    /// exactly where it was.
    ///
    /// This is the difference between the two resets rather than an omission. A
    /// snooze never paused the agent, so an agent starting is not the developer
    /// changing their mind — and a raised hand already stops the conversation
    /// *classifying* as snoozed without spending it, which is a derivation that
    /// ships in the client. Clearing the fields here would spend a return ticket
    /// the developer might still want: dismiss the request and the conversation
    /// would stay in the inbox rather than going back to sleep.
    #[test]
    fn a_session_or_a_raised_hand_does_not_spend_the_snooze() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        let asleep = threads.get("thread-1").expect("the thread").lifecycle;
        let _ = published(&mut watching);

        for work in [
            Change::Session(running("turn-1")),
            Change::Session(Session {
                status: SessionStatus::Error,
                ..running("turn-1")
            }),
            Change::Activity(crate::worklog::requested(&a_permission(), None)),
        ] {
            threads.apply("thread-1", work).expect("applied");
        }

        assert_eq!(
            threads.get("thread-1").expect("the thread").lifecycle,
            asleep,
            "work the developer did not do spent their snooze"
        );
        assert!(
            !published(&mut watching)
                .iter()
                .any(|kind| kind == "thread.unsnoozed"),
            "a wake was announced for a snooze nothing had spent"
        );
    }

    /// One message can spend both, and each is announced as its own event.
    ///
    /// The two resets are separate decisions about separate fields, so a
    /// conversation the developer settled *and* snoozed comes back from both —
    /// and a client that folded one event would otherwise be left holding half a
    /// conversation's lifecycle.
    #[test]
    fn a_message_can_spend_both_the_pin_and_the_return_ticket() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        threads.apply("thread-1", Change::Settled).expect("settled");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        let _ = published(&mut watching);

        threads.apply("thread-1", a_turn_request()).expect("a turn");

        assert_eq!(
            published(&mut watching),
            vec![
                "thread.turn-start-requested",
                "thread.unsettled",
                "thread.unsnoozed"
            ]
        );
        let lifecycle = threads.get("thread-1").expect("the thread").lifecycle;
        assert_eq!(lifecycle.settled_override, None);
        assert_eq!(lifecycle.snoozed_until, None);
    }

    /// The wake carries the server's own reason, for the reason a reset does: a
    /// client cannot forge it, and the developer's `user` wake is a different
    /// account of what happened.
    #[test]
    fn a_spent_return_ticket_carries_the_neutral_reason() {
        let (threads, _shell) = threads();
        let entry = threads.entry("thread-1");
        let mut watching = entry.events.subscribe();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        while watching.try_recv().is_ok() {}

        threads.apply("thread-1", a_turn_request()).expect("a turn");

        let reasons: Vec<Value> = events(&mut watching)
            .iter()
            .filter(|item| item["event"]["type"] == "thread.unsnoozed")
            .map(|item| item["event"]["payload"]["reason"].clone())
            .collect();
        assert_eq!(reasons, vec![json!(BY_ACTIVITY)]);
    }

    /// An archived conversation keeps its snooze through real work, which is
    /// [`Thread::wants_waking`]'s archived reading asked about the other pair of
    /// fields — and it has to be the same reading, because `thread.snooze` and
    /// `thread.unsnooze` both refuse an archived conversation too.
    #[test]
    fn an_archived_conversation_keeps_its_snooze_through_real_work() {
        let (threads, _shell) = threads();
        threads.create(a_thread("thread-1")).expect("created");
        threads
            .apply(
                "thread-1",
                Change::Snoozed {
                    until: WAKE.to_string(),
                },
            )
            .expect("snoozed");
        threads
            .apply("thread-1", Change::Archived)
            .expect("archived");

        threads
            .apply("thread-1", a_turn_request())
            .expect("the turn was requested");

        assert_eq!(
            threads
                .get("thread-1")
                .expect("the thread")
                .lifecycle
                .snoozed_until,
            Some(WAKE.to_string()),
            "work in an archived conversation spent a snooze no command could"
        );
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
}
