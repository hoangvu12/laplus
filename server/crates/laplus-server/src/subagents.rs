//! Subagent work streams: what a delegated child did, as its own replayable
//! session.
//!
//! A **subagent** is a delegated child agent; a **subagent work stream** is its
//! ordered, replayable conversation and work. Both words are the product's, and
//! this module is the whole of the second one — see the **Subagent** and
//! **Subagent work stream** entries in `CONTEXT.md`.
//!
//! ```text
//! C>S Request  orchestration.subscribeSubagent {"threadId":"…","childId":"…"}
//! S>C Chunk    {"kind":"snapshot","snapshot":{"stream":{…},"entries":[…]}}
//! S>C Chunk    {"kind":"entry-upserted","entry":{"id":"…","sequence":3,…}}
//! S>C Chunk    {"kind":"stream-updated","stream":{…,"state":"completed"}}
//! ```
//!
//! ## Provider-neutral by construction
//!
//! Nothing here knows what OpenCode, Claude or Codex is. An adapter decides an
//! [`Update`] — a child's identity and whatever that one provider event knew —
//! and this module is what turns a sequence of those into a stream a client can
//! replay. That is the seam the three drivers share: OpenCode is merely the
//! first to fill it, and a field a protocol does not expose stays `None` rather
//! than being invented.
//!
//! ## Three decisions worth stating
//!
//! **A child stream is stored beside the parent transcript, not in it.** The
//! conversation keeps one compact row per child — [`crate::worklog::subagent`],
//! carrying the `childId` this module is keyed by — and nothing else. A thread
//! snapshot therefore stays the size of the conversation however much its
//! children did, and opening an old thread does not hydrate every child it ever
//! ran. The row is the launcher; this is what it launches.
//!
//! **An entry is upserted by its own id, and ordered by its own sequence.** A
//! provider that revises a part it already sent — OpenCode resends a text part
//! with the text so far — must move that entry rather than append a second copy
//! of the same prose. It also makes replay and live continuation meet without
//! arithmetic: a subscription opens with the whole stream and then streams
//! upserts, so an event a client sees twice lands on the state it already held.
//! There is no cursor to get wrong, which is why this subscription does not
//! take one.
//!
//! **A stream lives exactly as long as its parent thread.** Deleting the
//! conversation removes its children here and on disk ([`Streams::forget`]),
//! because a historical inline row that opened an expired view would be worse
//! than one that opens nothing. That is the one place this parts company with
//! the thread's own soft delete, and it is deliberate: the reasons a thread row
//! survives a delete — git refs, a `deletedAt` the contract carries — are not
//! true of a child's prose.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::clock::now_iso;
use crate::subscriptions::{EventSource, BACKLOG};
use crate::transcripts::{Transcripts, Write};

/// The subscription that *is* one subagent's work stream.
pub const SUBSCRIBE_SUBAGENT: &str = "orchestration.subscribeSubagent";

/// Where a child is in its life, as the compact row and the stream both report
/// it.
///
/// Six rather than "running or not", because the inline row has to answer
/// "should I be waiting for this?" and the three terminal answers are not the
/// same news. [`State::Blocked`] is the one that answers "it is waiting on
/// *you*": a child that stopped for a permission or a question, recorded by the
/// adapter that received the request and cleared when the developer answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Delegated, and not yet doing anything this server has seen.
    Pending,
    Working,
    /// Waiting on the developer. Recorded by the adapter that can prove it.
    Blocked,
    Completed,
    Interrupted,
    Failed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Pending => "pending",
            State::Working => "working",
            State::Blocked => "blocked",
            State::Completed => "completed",
            State::Interrupted => "interrupted",
            State::Failed => "failed",
        }
    }

    fn from_str(value: &str) -> Option<State> {
        Some(match value {
            "pending" => State::Pending,
            "working" => State::Working,
            "blocked" => State::Blocked,
            "completed" => State::Completed,
            "interrupted" => State::Interrupted,
            "failed" => State::Failed,
            _ => return None,
        })
    }

    pub fn terminal(self) -> bool {
        matches!(self, State::Completed | State::Interrupted | State::Failed)
    }
}

/// How a child's work ended, and what came back.
///
/// Four answers rather than three: a child that finished and said nothing is
/// **not** a child whose result is missing, and a row that showed the last thing
/// it happened to say would present stale activity as a conclusion. That is the
/// distinction [`OutcomeKind::Empty`] exists for, and
/// [`Outcome::completed`] is what makes it impossible to record the wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Completed,
    /// Completed, with no textual result. A conclusion, not a gap.
    Empty,
    Failed,
    Interrupted,
}

impl OutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeKind::Completed => "completed",
            OutcomeKind::Empty => "empty",
            OutcomeKind::Failed => "failed",
            OutcomeKind::Interrupted => "interrupted",
        }
    }

    fn from_str(value: &str) -> Option<OutcomeKind> {
        Some(match value {
            "completed" => OutcomeKind::Completed,
            "empty" => OutcomeKind::Empty,
            "failed" => OutcomeKind::Failed,
            "interrupted" => OutcomeKind::Interrupted,
            _ => return None,
        })
    }

    fn state(self) -> State {
        match self {
            OutcomeKind::Completed | OutcomeKind::Empty => State::Completed,
            OutcomeKind::Failed => State::Failed,
            OutcomeKind::Interrupted => State::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kind: OutcomeKind,
    pub text: Option<String>,
}

impl Outcome {
    /// A child that finished. Blank text is [`OutcomeKind::Empty`] rather than a
    /// completed result nobody can read.
    pub fn completed(text: Option<String>) -> Outcome {
        match text.filter(|text| !text.trim().is_empty()) {
            Some(text) => Outcome {
                kind: OutcomeKind::Completed,
                text: Some(text),
            },
            None => Outcome {
                kind: OutcomeKind::Empty,
                text: None,
            },
        }
    }

    pub fn failed(text: Option<String>) -> Outcome {
        Outcome {
            kind: OutcomeKind::Failed,
            text: text.filter(|text| !text.trim().is_empty()),
        }
    }

    pub fn interrupted(text: Option<String>) -> Outcome {
        Outcome {
            kind: OutcomeKind::Interrupted,
            text: text.filter(|text| !text.trim().is_empty()),
        }
    }

    fn to_value(&self) -> Value {
        json!({"kind": self.kind.as_str(), "text": self.text})
    }
}

/// What one entry in a child stream *is*.
///
/// Eight members covering the spec's shared kinds: child prose, commands and
/// output, reads and searches, edits and diffs, other tool calls and results,
/// warnings and errors, approvals or questions, and the terminal outcome. They
/// are one vocabulary rather than one per provider, because a developer reading
/// a child's work should not have to learn which agent produced it — and a
/// provider that does not expose one of them simply never records it.
///
/// **A kind is a rendering decision, not a taxonomy.** [`EntryKind::Command`]
/// exists because the main agent's work log draws a command differently from a
/// file change, and the child's tab reuses those same rows; a provider whose
/// tool laplus cannot place lands on [`EntryKind::Tool`], which is what the
/// generic row already draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The child's own prose.
    Message,
    /// A command the child ran, with whatever the provider said about how it
    /// went. Payload: [`Work`], carrying `command`.
    Command,
    /// A file the child read, or a search it made — the evidence it examined.
    /// Payload: [`Work`], carrying `paths` and/or `query`.
    Read,
    /// A file the child changed. Payload: [`Work`], carrying `paths`, which is
    /// what the child's tab offers file and diff navigation from.
    Edit,
    /// Any other tool call and its result. Payload: [`Work`].
    Tool,
    /// A warning or an error, in its place in the work rather than lifted out of
    /// it. Payload: [`Notice`].
    Notice,
    /// A permission or a question the child stopped for, and — on the same
    /// entry, under the same key — how it was resolved. Payload: [`Blocker`].
    Blocker,
    /// The terminal entry: its result, failure, interruption, or empty answer.
    Outcome,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Message => "message",
            EntryKind::Command => "command",
            EntryKind::Read => "read",
            EntryKind::Edit => "edit",
            EntryKind::Tool => "tool",
            EntryKind::Notice => "notice",
            EntryKind::Blocker => "blocker",
            EntryKind::Outcome => "outcome",
        }
    }

    fn from_str(value: &str) -> Option<EntryKind> {
        Some(match value {
            "message" => EntryKind::Message,
            "command" => EntryKind::Command,
            "read" => EntryKind::Read,
            "edit" => EntryKind::Edit,
            "tool" => EntryKind::Tool,
            "notice" => EntryKind::Notice,
            "blocker" => EntryKind::Blocker,
            "outcome" => EntryKind::Outcome,
            _ => return None,
        })
    }
}

/// One piece of a child's work, in the vocabulary the main agent's work rows
/// already speak.
///
/// The status strings are the client's `toolLifecycleStatus` literals rather
/// than any provider's, for the reason [`crate::worklog::subagent`] builds the
/// same row the Claude driver does: the child's tab renders through the *same*
/// components as the parent transcript, so a translation done here is a
/// translation the client does not have to do twice.
///
/// Every field but `title` and `status` is optional, and an absent one crosses
/// the wire as `null` rather than as a guess. A provider that does not report
/// which file a search covered, or what a command printed, leaves laplus with
/// nothing to say about it — and a client must read that `null` as absence
/// rather than as emptiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    /// What to call it — the tool's own name or the title it reported.
    pub title: String,
    pub status: Progress,
    /// The output, the error, or the one-line summary the provider gave.
    pub detail: Option<String>,
    /// The command line, for [`EntryKind::Command`].
    pub command: Option<String>,
    /// The files read or changed. What file and diff navigation is offered from,
    /// so only paths that name a file belong here — a search's *directory* does
    /// not, because there is no file surface to open it in.
    pub paths: Vec<String>,
    /// The pattern, for a search.
    pub query: Option<String>,
}

/// How a piece of child work is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    InProgress,
    Completed,
    Failed,
}

impl Progress {
    pub fn as_str(self) -> &'static str {
        match self {
            Progress::InProgress => "inProgress",
            Progress::Completed => "completed",
            Progress::Failed => "failed",
        }
    }
}

impl Work {
    fn to_value(&self) -> Value {
        json!({
            "title": self.title,
            "status": self.status.as_str(),
            "detail": self.detail,
            "command": self.command,
            "paths": self.paths,
            "query": self.query,
        })
    }
}

/// A warning or an error the child hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warning,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }
}

/// A permission or a question a child stopped for.
///
/// **One entry for the whole blocker**, asked and answered: the resolution is
/// written back under the same key rather than appended as a second row, so a
/// child's history reads "it waited for this, and this is what it was told"
/// instead of leaving the developer to pair two entries by an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    /// The provider's own request id — the identity a response is routed by.
    pub request_id: String,
    pub kind: BlockerKind,
    pub title: String,
    pub detail: Option<String>,
    /// How it ended, once it has. `None` is still waiting.
    pub resolution: Option<Resolution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockerKind {
    Permission,
    Question,
}

impl BlockerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockerKind::Permission => "permission",
            BlockerKind::Question => "question",
        }
    }
}

/// How a blocker ended.
///
/// An identity rather than a sentence. The wording belongs to the client, which
/// already owns the wording of every other closed literal it is handed — see
/// `OUTCOME_LABELS` in `SubagentStreamPanel.tsx`. Carrying prose here would put
/// presentation in the contract and would make a resolution something a later
/// build could only recognise by matching a string it did not write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Approved,
    ApprovedForSession,
    Declined,
    Cancelled,
    /// A question the developer answered.
    Answered,
    /// A question the developer refused to answer.
    Rejected,
    /// The developer decided, and the decision could not be delivered.
    ///
    /// Its own member rather than an absent resolution, because the two are
    /// different news: nobody has answered yet, against nobody *can* now. The
    /// child stays [`State::Blocked`] either way — it is, and no answer reached
    /// it — while the conversation records the decision and the failure to send
    /// it. See [`crate::session`]'s `answer`.
    Undelivered,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Resolution::Approved => "approved",
            Resolution::ApprovedForSession => "approvedForSession",
            Resolution::Declined => "declined",
            Resolution::Cancelled => "cancelled",
            Resolution::Answered => "answered",
            Resolution::Rejected => "rejected",
            Resolution::Undelivered => "undelivered",
        }
    }
}

impl Blocker {
    /// What a child stopped for, read off the request itself.
    ///
    /// `None` when the request is not a child's — which is the root agent, and
    /// every provider that does not attribute a request to one. Reading it from
    /// the request rather than from the adapter's own bookkeeping is what stops
    /// the question of "is this a permission or a question" being answered twice
    /// and differently: [`crate::approval::ApprovalRequest`] already knows, and
    /// it is the same value that reaches the conversation's row.
    pub fn waiting_on(request: &crate::approval::ApprovalRequest) -> Option<(String, Blocker)> {
        let waiting = request.subagent.as_ref()?;
        Some((
            waiting.child_id.clone(),
            Blocker {
                request_id: request.request_id.clone(),
                kind: match request.tool_name.as_str() {
                    crate::worklog::ASK_USER_QUESTION => BlockerKind::Question,
                    _ => BlockerKind::Permission,
                },
                title: request.tool_name.clone(),
                detail: request.description.clone(),
                resolution: None,
            },
        ))
    }

    pub fn resolved(mut self, resolution: Resolution) -> Blocker {
        self.resolution = Some(resolution);
        self
    }
}

/// One thing that happened in a child's session.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable for the life of the stream. An entry that arrives again under the
    /// same id **replaces** the one already there and keeps its position.
    pub id: String,
    /// Where it sits. Assigned once, when the entry is first seen, and never
    /// moved.
    pub sequence: i64,
    pub kind: EntryKind,
    pub payload: Value,
    pub created_at: String,
}

impl Entry {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "sequence": self.sequence,
            "kind": self.kind.as_str(),
            "payload": self.payload,
            "createdAt": self.created_at,
        })
    }
}

/// A stream without its work: who the child is, what it was asked for, where it
/// is, and how it ended.
///
/// A type of its own rather than eight fields on [`Stream`], because it is
/// exactly the part that travels alone. It is what the durable row holds
/// ([`crate::transcripts::Write::SubagentStream`]) and what crosses the wire on
/// every update, and both are for [`crate::threads::ThreadRow`]'s reason: a
/// child that says one more thing must not cost a copy of everything it has
/// already said.
#[derive(Debug, Clone, PartialEq)]
pub struct Head {
    /// Stable for the child's whole life, and the address of its right-panel
    /// surface. Minted by the adapter from whatever the provider makes durable.
    pub child_id: String,
    /// The child that delegated this one, when the provider proves it.
    ///
    /// `None` is "this is a direct child of the conversation" *and* "the
    /// provider did not say" — which is honest, because a hierarchy laplus
    /// cannot prove is one it must not draw. Nothing reads it yet; ticket 06
    /// places a nested launcher with it.
    pub parent_child_id: Option<String>,
    /// The child's semantic name or type — `explore`, `general`, a project's
    /// own.
    pub name: Option<String>,
    /// What the parent asked it for.
    pub assignment: Option<String>,
    pub state: State,
    pub outcome: Option<Outcome>,
    pub created_at: String,
    pub updated_at: String,
}

impl Head {
    /// The contract's `OrchestrationSubagentStream`.
    ///
    /// `entry_count` is passed rather than held, because it is the one thing on
    /// this object that is a fact about the work rather than about the child —
    /// storing it here would be a second copy of `entries.len()` to keep in
    /// step.
    fn to_value(&self, entry_count: usize) -> Value {
        json!({
            "childId": self.child_id,
            "parentChildId": self.parent_child_id,
            "name": self.name,
            "assignment": self.assignment,
            "state": self.state.as_str(),
            "outcome": self.outcome.as_ref().map(Outcome::to_value),
            "entryCount": entry_count,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

/// One delegated child: who it is, what it was asked for, and everything it did.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub head: Head,
    pub entries: Vec<Entry>,
}

impl Stream {
    fn new(child_id: String, created_at: String) -> Stream {
        Stream {
            head: Head {
                child_id,
                parent_child_id: None,
                name: None,
                assignment: None,
                state: State::Pending,
                outcome: None,
                created_at: created_at.clone(),
                updated_at: created_at,
            },
            entries: Vec::new(),
        }
    }

    /// The stream without its entries, on the wire.
    ///
    /// What a client needs to label a tab and what the parent's compact row
    /// already says, and deliberately *not* the work — see the module note on
    /// why the two travel separately.
    fn head_value(&self) -> Value {
        self.head.to_value(self.entries.len())
    }

    fn snapshot(&self) -> Value {
        json!({
            "stream": self.head_value(),
            "entries": self.entries.iter().map(Entry::to_value).collect::<Vec<_>>(),
        })
    }
}

/// One entry an adapter decided, before it has a place in a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    /// The provider's own name for this entry, when it has one.
    ///
    /// Load-bearing rather than cosmetic: OpenCode resends a text part carrying
    /// the prose *so far*, so a key is what makes the second one an edit of the
    /// first instead of the same sentence twice. `None` mints a fresh id and
    /// always appends.
    pub key: Option<String>,
    pub kind: EntryKind,
    pub payload: Value,
}

impl NewEntry {
    /// The child's own prose, as one part of its reply.
    pub fn said(key: Option<String>, text: &str) -> NewEntry {
        NewEntry {
            key,
            kind: EntryKind::Message,
            payload: json!({"text": text}),
        }
    }

    /// Something the child *did*: a command, a read or search, an edit, or any
    /// other tool call.
    ///
    /// The key is the provider's own name for the call, so the same call
    /// arriving again as it progresses — announced, running, finished — moves
    /// one entry through its statuses rather than leaving three rows saying the
    /// same thing at different times. That is [`NewEntry::key`]'s whole
    /// property, applied to work rather than to prose.
    pub fn worked(key: Option<String>, kind: EntryKind, work: &Work) -> NewEntry {
        NewEntry {
            key,
            kind,
            payload: work.to_value(),
        }
    }

    /// A warning or an error, kept in the order it happened.
    pub fn noticed(key: Option<String>, notice: &Notice) -> NewEntry {
        NewEntry {
            key,
            kind: EntryKind::Notice,
            payload: json!({"level": notice.level.as_str(), "text": notice.text}),
        }
    }

    /// A permission or question the child stopped for — and, under the same key,
    /// what it was eventually told. See [`Blocker`].
    pub fn blocked(blocker: &Blocker) -> NewEntry {
        NewEntry {
            key: Some(format!("blocker:{}", blocker.request_id)),
            kind: EntryKind::Blocker,
            payload: json!({
                "requestId": blocker.request_id,
                "blocker": blocker.kind.as_str(),
                "title": blocker.title,
                "detail": blocker.detail,
                "resolution": blocker.resolution.map(Resolution::as_str),
            }),
        }
    }

    /// The terminal entry. The conclusion stays connected to the work that
    /// produced it by living in the same stream rather than replacing it.
    pub fn concluded(outcome: &Outcome) -> NewEntry {
        NewEntry {
            key: Some("outcome".to_string()),
            kind: EntryKind::Outcome,
            payload: outcome.to_value(),
        }
    }
}

/// What one provider event knew about one child.
///
/// Every field but the identity is optional and an absent one means "unchanged",
/// which is what lets an adapter forward exactly what its protocol exposed
/// without first re-deriving everything it did not.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub child_id: String,
    pub parent_child_id: Option<String>,
    pub name: Option<String>,
    pub assignment: Option<String>,
    pub state: Option<State>,
    /// Recording an outcome also settles the state and appends the terminal
    /// entry — one decision rather than three an adapter could make
    /// inconsistently.
    pub outcome: Option<Outcome>,
    pub entries: Vec<NewEntry>,
}

impl Update {
    pub fn for_child(child_id: impl Into<String>) -> Update {
        Update {
            child_id: child_id.into(),
            parent_child_id: None,
            name: None,
            assignment: None,
            state: None,
            outcome: None,
            entries: Vec::new(),
        }
    }

    pub fn named(mut self, name: Option<String>) -> Update {
        self.name = name;
        self
    }

    pub fn assigned(mut self, assignment: Option<String>) -> Update {
        self.assignment = assignment;
        self
    }

    pub fn in_state(mut self, state: State) -> Update {
        self.state = Some(state);
        self
    }

    pub fn concluded(mut self, outcome: Outcome) -> Update {
        self.outcome = Some(outcome);
        self
    }

    pub fn with(mut self, entry: NewEntry) -> Update {
        self.entries.push(entry);
        self
    }
}

/// A validated `orchestration.subscribeSubagent` call.
#[derive(Debug, Clone)]
pub struct Watch {
    pub thread_id: String,
    pub child_id: String,
}

impl Watch {
    pub fn read(payload: &Value) -> Result<Watch, Value> {
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        match (field("threadId"), field("childId")) {
            (Some(thread_id), Some(child_id)) => Ok(Watch {
                thread_id,
                child_id,
            }),
            _ => Err(crate::rpc::declared(
                "OrchestrationGetSnapshotError",
                format_args!("orchestration.subscribeSubagent needs a threadId and a childId"),
            )),
        }
    }
}

/// Every child stream this server holds, by the conversation that owns it.
///
/// Cheap to clone and every clone is the same registry, like
/// [`crate::threads::Threads`], which holds one of these: a subscription
/// outlives the call that opened it.
#[derive(Debug, Clone)]
pub struct Streams {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    open: Mutex<HashMap<String, Arc<Slot>>>,
    /// Conversations whose children have been removed. A delete does not stop
    /// the agent behind a conversation — see [`crate::orchestration::Shell`] —
    /// so a child still running would otherwise write its stream back moments
    /// after the developer removed it.
    forgotten: Mutex<std::collections::HashSet<String>>,
    transcripts: Transcripts,
}

/// One child's slot: what it is, and who is watching.
#[derive(Debug)]
struct Slot {
    stream: Mutex<Stream>,
    events: broadcast::Sender<Value>,
}

/// The key a slot is held under. A child id is the provider's and is only
/// unique within its conversation.
fn slot_key(thread_id: &str, child_id: &str) -> String {
    format!("{thread_id}\u{1f}{child_id}")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Streams {
    pub fn new(transcripts: Transcripts) -> Streams {
        Streams {
            inner: Arc::new(Inner {
                open: Mutex::new(HashMap::new()),
                forgotten: Mutex::new(std::collections::HashSet::new()),
                transcripts,
            }),
        }
    }

    /// Put back the child streams the last run left behind.
    ///
    /// Silent, like [`crate::threads::Threads::restore`] and for its reason:
    /// these are not changes, they are the world as the first client will find
    /// it.
    pub fn restore(&self, stored: Vec<(String, Stream)>) {
        let mut open = lock(&self.inner.open);
        for (thread_id, mut stream) in stored {
            stream.entries.sort_by_key(|entry| entry.sequence);
            open.entry(slot_key(&thread_id, &stream.head.child_id))
                .or_insert_with(|| {
                    Arc::new(Slot {
                        stream: Mutex::new(stream),
                        events: broadcast::channel(BACKLOG).0,
                    })
                });
        }
    }

    /// Fold what an adapter decided into a child's stream, publish it, and queue
    /// it for the disk.
    ///
    /// The three happen together for [`crate::threads::fold::Change`]'s reason:
    /// a change has to update the stream *and* describe itself to subscribers
    /// *and* be written down, and the three must not be possible to do
    /// inconsistently.
    pub fn record(&self, thread_id: &str, update: Update) {
        if lock(&self.inner.forgotten).contains(thread_id) {
            return;
        }
        let slot = {
            let mut open = lock(&self.inner.open);
            Arc::clone(
                open.entry(slot_key(thread_id, &update.child_id))
                    .or_insert_with(|| {
                        Arc::new(Slot {
                            stream: Mutex::new(Stream::new(update.child_id.clone(), now_iso())),
                            events: broadcast::channel(BACKLOG).0,
                        })
                    }),
            )
        };

        let at = now_iso();
        let mut published = Vec::new();
        let mut writes = Vec::new();
        {
            let mut stream = lock(&slot.stream);
            let mut head_moved = false;
            if update.parent_child_id.is_some()
                && stream.head.parent_child_id != update.parent_child_id
            {
                stream.head.parent_child_id = update.parent_child_id;
                head_moved = true;
            }
            if update.name.is_some() && stream.head.name != update.name {
                stream.head.name = update.name;
                head_moved = true;
            }
            if update.assignment.is_some() && stream.head.assignment != update.assignment {
                stream.head.assignment = update.assignment;
                head_moved = true;
            }
            // A terminal state is final. A provider that goes on narrating a
            // child it has already reported on must not reopen it, which is the
            // same rule the compact row has always followed.
            if let Some(state) = update.state {
                if !stream.head.state.terminal() && stream.head.state != state {
                    stream.head.state = state;
                    head_moved = true;
                }
            }

            let mut entries = update.entries;
            if let Some(outcome) = &update.outcome {
                if stream.head.outcome.is_none() {
                    entries.push(NewEntry::concluded(outcome));
                    stream.head.outcome = Some(outcome.clone());
                    stream.head.state = outcome.kind.state();
                    head_moved = true;
                }
            }

            for new in entries {
                // Two namespaces so a provider whose key happens to be a number
                // cannot collide with an unkeyed entry's position.
                let next = stream
                    .entries
                    .last()
                    .map(|entry| entry.sequence + 1)
                    .unwrap_or(1);
                let id = match &new.key {
                    Some(key) => format!("{}:k:{key}", stream.head.child_id),
                    None => format!("{}:n:{next}", stream.head.child_id),
                };
                let existing = stream.entries.iter().position(|entry| entry.id == id);
                let entry = match existing {
                    Some(index) => {
                        let entry = Entry {
                            id,
                            sequence: stream.entries[index].sequence,
                            kind: new.kind,
                            payload: new.payload,
                            created_at: stream.entries[index].created_at.clone(),
                        };
                        if stream.entries[index] == entry {
                            continue;
                        }
                        stream.entries[index] = entry.clone();
                        entry
                    }
                    None => {
                        let entry = Entry {
                            id,
                            sequence: next,
                            kind: new.kind,
                            payload: new.payload,
                            created_at: at.clone(),
                        };
                        stream.entries.push(entry.clone());
                        entry
                    }
                };
                head_moved = true;
                writes.push(Write::SubagentEntry {
                    thread_id: thread_id.to_string(),
                    child_id: stream.head.child_id.clone(),
                    entry: Box::new(entry.clone()),
                });
                published.push(json!({"kind": "entry-upserted", "entry": entry.to_value()}));
            }

            if !head_moved {
                return;
            }
            stream.head.updated_at = at;
            published.push(json!({"kind": "stream-updated", "stream": stream.head_value()}));
            writes.insert(
                0,
                Write::SubagentStream {
                    thread_id: thread_id.to_string(),
                    head: Box::new(stream.head.clone()),
                },
            );
        }

        for write in writes {
            self.inner.transcripts.queue(write);
        }
        for event in published {
            let _ = slot.events.send(event);
        }
    }

    /// Open an `orchestration.subscribeSubagent` subscription: the whole stream,
    /// then every change to it.
    ///
    /// **Replay and live continuation are one operation**, which is why there is
    /// no cursor here and no gap to lose an entry in: the snapshot is taken
    /// after the receiver is subscribed, so anything published in between
    /// arrives twice rather than not at all — and an upsert applied twice lands
    /// on the state it already held.
    ///
    /// A child this server does not hold is **refused**, on
    /// [`crate::threads::Threads::subscribe`]'s footing and for its reason: a
    /// stream that opened on nothing and then narrated a child's arrival is a
    /// stream a client silently discards. Ticket 05 turns this refusal into the
    /// explicit unavailable surface a restored tab needs.
    pub fn subscribe(&self, call: &Watch) -> Result<EventSource, Value> {
        let slot = lock(&self.inner.open)
            .get(&slot_key(&call.thread_id, &call.child_id))
            .map(Arc::clone)
            .ok_or_else(|| {
                crate::rpc::declared(
                    "OrchestrationGetSnapshotError",
                    format_args!(
                        "Subagent {} was not found in thread {}",
                        call.child_id, call.thread_id
                    ),
                )
            })?;
        let updates = slot.events.subscribe();
        Ok(EventSource::new(
            move || {
                vec![json!({
                    "kind": "snapshot",
                    "snapshot": lock(&slot.stream).snapshot(),
                })]
            },
            updates,
        ))
    }

    /// One child's stream as it stands, for a caller that wants the value rather
    /// than a subscription.
    pub fn get(&self, thread_id: &str, child_id: &str) -> Option<Stream> {
        lock(&self.inner.open)
            .get(&slot_key(thread_id, child_id))
            .map(|slot| lock(&slot.stream).clone())
    }

    /// The children of one conversation, oldest first.
    pub fn of_thread(&self, thread_id: &str) -> Vec<Stream> {
        let prefix = format!("{thread_id}\u{1f}");
        let mut found: Vec<Stream> = lock(&self.inner.open)
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, slot)| lock(&slot.stream).clone())
            .collect();
        found.sort_by(|left, right| {
            (&left.head.created_at, &left.head.child_id)
                .cmp(&(&right.head.created_at, &right.head.child_id))
        });
        found
    }

    /// Forget every child of these conversations, here and on disk.
    ///
    /// Called when the developer deletes a thread or its project. The spec's own
    /// instruction — "removing the parent removes its child index and streams" —
    /// and the module note says why this is a real deletion where the thread's
    /// own is a stamp.
    pub fn forget(&self, thread_ids: &[String]) {
        {
            let mut open = lock(&self.inner.open);
            let mut forgotten = lock(&self.inner.forgotten);
            for thread_id in thread_ids {
                forgotten.insert(thread_id.clone());
                let prefix = format!("{thread_id}\u{1f}");
                let going: Vec<String> = open
                    .keys()
                    .filter(|key| key.starts_with(&prefix))
                    .cloned()
                    .collect();
                for key in going {
                    open.remove(&key);
                }
            }
        }
        // Queued whether or not anything was in memory: a restart followed by a
        // delete has nothing here and rows on disk, and that is precisely the
        // case the stored copy must not survive.
        for thread_id in thread_ids {
            self.inner.transcripts.queue(Write::ForgetSubagents {
                thread_id: thread_id.clone(),
            });
        }
    }
}

impl From<Head> for Stream {
    /// A stored head, back as the stream it heads. Its entries are dealt in
    /// afterwards — see [`crate::store::Database::child_streams`], which walks
    /// two ordered lists the way the transcript reader walks three.
    fn from(head: Head) -> Stream {
        Stream {
            head,
            entries: Vec::new(),
        }
    }
}

/// Read a state back off a stored row, defaulting an unknown one to
/// [`State::Pending`] rather than refusing the conversation it belongs to.
pub fn state_from_stored(value: &str) -> State {
    State::from_str(value).unwrap_or(State::Pending)
}

pub fn outcome_from_stored(kind: Option<&str>, text: Option<String>) -> Option<Outcome> {
    Some(Outcome {
        kind: OutcomeKind::from_str(kind?)?,
        text,
    })
}

/// Read an entry back, dropping one whose kind this build does not know.
///
/// Forward compatibility rather than tidiness: a stream written by a later build
/// that learned a richer entry vocabulary must not stop an earlier one opening
/// the conversation at all — the same drift policy the provider adapters follow.
pub fn entry_from_stored(
    id: String,
    sequence: i64,
    kind: &str,
    payload: Value,
    created_at: String,
) -> Option<Entry> {
    Some(Entry {
        id,
        sequence,
        kind: EntryKind::from_str(kind)?,
        payload,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streams() -> Streams {
        Streams::new(Transcripts::nowhere())
    }

    fn entries(stream: &Stream) -> Vec<(i64, &str)> {
        stream
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.sequence,
                    entry.payload["text"].as_str().unwrap_or_default(),
                )
            })
            .collect()
    }

    /// A provider that revises a part it already sent is editing one entry, not
    /// adding a second. OpenCode resends a text part carrying the prose so far,
    /// so without the key a child that streamed three tokens would read as three
    /// paragraphs each a prefix of the next.
    #[test]
    fn an_entry_resent_under_its_key_moves_rather_than_repeats() {
        let streams = streams();
        streams.record(
            "thread-1",
            Update::for_child("child-1").with(NewEntry::said(Some("part-1".into()), "look")),
        );
        streams.record(
            "thread-1",
            Update::for_child("child-1")
                .with(NewEntry::said(Some("part-1".into()), "looking through it")),
        );
        streams.record(
            "thread-1",
            Update::for_child("child-1").with(NewEntry::said(Some("part-2".into()), "and again")),
        );

        let stream = streams.get("thread-1", "child-1").expect("the stream");
        assert_eq!(
            entries(&stream),
            vec![(1, "looking through it"), (2, "and again")],
            "a revised part took a second position"
        );
    }

    /// The conclusion is the last entry of the same stream rather than a
    /// replacement for it, and it settles the state with it.
    #[test]
    fn a_conclusion_is_the_streams_terminal_entry() {
        let streams = streams();
        streams.record(
            "thread-1",
            Update::for_child("child-1")
                .in_state(State::Working)
                .with(NewEntry::said(Some("part-1".into()), "looking")),
        );
        streams.record(
            "thread-1",
            Update::for_child("child-1").concluded(Outcome::completed(Some("eleven".into()))),
        );

        let stream = streams.get("thread-1", "child-1").expect("the stream");
        assert_eq!(stream.head.state, State::Completed);
        assert_eq!(stream.entries.len(), 2);
        let last = stream.entries.last().expect("the terminal entry");
        assert_eq!(last.kind, EntryKind::Outcome);
        assert_eq!(last.payload["kind"], "completed");
        assert_eq!(last.payload["text"], "eleven");
    }

    /// A child that finished without saying anything has *concluded*, and the
    /// stream says so rather than leaving the row to guess from silence.
    #[test]
    fn a_silent_completion_is_an_empty_outcome_rather_than_a_missing_one() {
        let streams = streams();
        streams.record(
            "thread-1",
            Update::for_child("child-1").concluded(Outcome::completed(Some("   ".into()))),
        );
        let stream = streams.get("thread-1", "child-1").expect("the stream");
        assert_eq!(stream.head.outcome.expect("an outcome").kind, OutcomeKind::Empty);
        assert_eq!(stream.head.state, State::Completed);
    }

    /// Anything a provider goes on saying after it has reported must not reopen
    /// the child — the answer is the answer.
    #[test]
    fn a_reported_child_does_not_reopen() {
        let streams = streams();
        streams.record(
            "thread-1",
            Update::for_child("child-1").concluded(Outcome::completed(Some("eleven".into()))),
        );
        streams.record(
            "thread-1",
            Update::for_child("child-1")
                .in_state(State::Working)
                .concluded(Outcome::failed(Some("no".into()))),
        );
        let stream = streams.get("thread-1", "child-1").expect("the stream");
        assert_eq!(stream.head.state, State::Completed);
        assert_eq!(
            stream.head.outcome.expect("an outcome").text.as_deref(),
            Some("eleven")
        );
    }

    /// Deleting the conversation takes its children with it, which is the whole
    /// of what "retained for as long as its parent thread" means.
    #[test]
    fn forgetting_a_thread_forgets_its_children() {
        let streams = streams();
        streams.record("thread-1", Update::for_child("child-1"));
        streams.record("thread-2", Update::for_child("child-1"));
        streams.forget(&["thread-1".to_string()]);
        assert!(streams.get("thread-1", "child-1").is_none());
        assert!(
            streams.get("thread-2", "child-1").is_some(),
            "a child id is only unique within its conversation"
        );
    }

    /// A subscription to a child nobody delegated is refused rather than opened
    /// on nothing, so a restored tab has something explicit to render.
    #[test]
    fn subscribing_to_an_unknown_child_is_refused() {
        let streams = streams();
        let refusal = streams
            .subscribe(&Watch {
                thread_id: "thread-1".into(),
                child_id: "child-1".into(),
            })
            .expect_err("a refusal");
        assert_eq!(refusal["_tag"], "OrchestrationGetSnapshotError");
    }
}
