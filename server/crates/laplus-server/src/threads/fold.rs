//! What a conversation is, and what one change does to it.
//!
//! The pure half of [`crate::threads`], cut out along the line `docs/adr/0002`
//! named and `docs/adr/0025` took. Everything here answers the same way every
//! time it is asked: a [`Thread`] and a [`Change`] go in, the thread as it now is
//! and the payload describing the move come out. Nothing in the path of [`fold`]
//! reads a clock, takes a lock, opens a channel or touches a child process — the
//! moment a change happened is decided by [`crate::threads::Threads::commit`] and
//! handed in as `at`, which is what lets a re-emitted settle report the stamp the
//! conversation already carried rather than this one.
//!
//! The fold mirrors `threadReducer.ts` deliberately and closely, for the reason
//! `docs/adr/0002` gives: upstream's own primary seam is a fold with no I/O, and
//! two implementations of one rule agree until they do not.
//!
//! ## The five that do read a clock
//!
//! [`Activity::info`], [`Activity::tool`], [`Activity::approval`],
//! [`Activity::failed`] and [`Adoption::now`] stamp the clock and mint
//! identifiers. They are not a hole in the paragraph above: none of them is
//! called by [`fold`]. They are called from `session`, `turn`, `worklog` and
//! `orchestration` to build the [`Change`] that is then handed to it, so the
//! clock enters *upstream* of the fold exactly as it did before this module
//! existed. Adding a sixth is a decision, not a tidy-up — `docs/adr/0025` is
//! where the four alternatives were weighed.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use crate::clock::now_iso;
use crate::settling::SessionStatus;
use crate::transcripts::Write;

// ---------------------------------------------------------------------------
// What a thread is
// ---------------------------------------------------------------------------

/// One conversation, as `OrchestrationThread` in the contract.
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// The provider instance this conversation was created under and its
    /// registered driver. Durable so a later model choice cannot silently
    /// change which agent owns the thread.
    pub provider: crate::provider::ProviderIdentity,
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
    /// Where this conversation sits in the developer's inbox — see
    /// [`Lifecycle`].
    pub lifecycle: Lifecycle,
}

/// **Inbox state**: whether a conversation belongs in the developer's working
/// list, and until when.
///
/// The six fields the contract declares on both renderings of a thread, in one
/// shape because they are emitted twice. Before ticket 01 of the
/// thread-lifecycle effort they were two independent lists of `null` literals on
/// the two `json!` blocks below, which is a shape that agrees until somebody adds
/// a seventh field to one of them.
///
/// **Not [`crate::settling`]**, despite `settled_override` and `settled_at`.
/// Settling is reading a session status as how a *turn* went; this is whether a
/// *thread* is in the inbox. The field names are the contract's and are not
/// negotiable — see the **Inbox state** entry in `CONTEXT.md`.
///
/// **This server does not classify.** `effectiveSettled`, `effectiveSnoozed`,
/// `threadRaisedHandWhileSnoozed` and the rest already exist in
/// `@t3tools/client-runtime`, with their own suite, and are what the developer
/// actually sees. A Rust copy would be a fourth copy of a rule this repository
/// already keeps three of — which is a thing this repository *could* do, since
/// ADR-0012 makes the client ordinary work, and does not. In particular a snooze
/// expires by being *read*: once `snoozed_until` is in the past the client stops
/// counting the thread as snoozed, so there is no timer here and nothing to
/// schedule.
///
/// Every field is `None` on a thread nothing has been done to, which is the same
/// answer a row written before the columns existed gives back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lifecycle {
    pub archived_at: Option<String>,
    /// The developer's standing answer to "is this finished?", overriding what
    /// the client would otherwise derive. One of the contract's two literals —
    /// see [`settled_override`] for why it is not a free string.
    pub settled_override: Option<&'static str>,
    pub settled_at: Option<String>,
    /// When the conversation comes back to the inbox on its own.
    pub snoozed_until: Option<String>,
    pub snoozed_at: Option<String>,
    /// Deleting is soft: the row, its transcript and its checkpoints stay, and
    /// this is the whole of what makes the thread deleted.
    pub deleted_at: Option<String>,
}

impl Lifecycle {
    /// Add the six keys to a rendering of a thread.
    ///
    /// Called from both [`Thread::to_detail_value`] and [`Thread::to_shell_value`]
    /// rather than written out in each, which is the point of the struct: a
    /// seventh field cannot reach one rendering and miss the other.
    ///
    /// The shell summary carries `deletedAt` too, though the contract's
    /// `OrchestrationThreadShell` does not declare it. A `Schema.Struct` ignores
    /// a key it does not name, so this costs the client nothing and saves this
    /// file a second, shorter shape whose only difference would be one omission.
    /// Since ticket 10 it is also always `null` there — a deleted conversation is
    /// on neither shelf ([`Shelf::holds`]), so no summary carrying a stamp ever
    /// reaches a client. The key stays for the reason above: one shape for both
    /// renderings is what stops a seventh field reaching one and missing the
    /// other.
    ///
    /// Panics on anything that is not an object, rather than returning and
    /// leaving the six keys off. Both callers pass a `json!({…})` literal, so
    /// there is no runtime path to it — and the failure it would otherwise
    /// produce is the expensive one this module keeps warning about: a snapshot
    /// missing a key the contract requires, which a client rejects whole.
    fn write_onto(&self, rendering: &mut Value) {
        let fields = rendering
            .as_object_mut()
            .expect("a rendering of a thread is an object");
        for (key, value) in [
            ("archivedAt", json!(self.archived_at)),
            ("settledOverride", json!(self.settled_override)),
            ("settledAt", json!(self.settled_at)),
            ("snoozedUntil", json!(self.snoozed_until)),
            ("snoozedAt", json!(self.snoozed_at)),
            ("deletedAt", json!(self.deleted_at)),
        ] {
            fields.insert(key.to_string(), value);
        }
    }

    /// Is this conversation already asleep until exactly this moment?
    ///
    /// One snooze asks it twice and from two directions, which is why it is a
    /// function rather than a comparison written out at each:
    /// [`Change::re_emitted_at`] asks it to decide whether the clock is read at
    /// all, and [`fold`] asks it to decide whether `snoozedAt` is restamped. A
    /// repeat has to answer the same in both, or a double-click would report an
    /// `updatedAt` from one snooze beside a `snoozedAt` from another.
    ///
    /// Keyed on the *time* rather than on being snoozed at all, because choosing
    /// a different one is a new decision rather than a repeat — see
    /// [`Change::Snoozed`], where what that costs is argued.
    fn asleep_until(&self, until: &str) -> bool {
        self.snoozed_until.as_deref() == Some(until)
    }

    /// Has the developer deleted this conversation?
    ///
    /// One reading of the field, because the question is asked from four
    /// directions and each of them would otherwise be a second answer to it:
    /// [`Shelf::holds`] keeps a deleted conversation off both lists,
    /// [`Change::on_the_list`] keeps its later changes off the project list's
    /// feed, `crate::threads::Threads::subscribe` refuses a stream over one, and
    /// `crate::orchestration::Shell::delete` refuses a second delete.
    ///
    /// The stamp *is* the deletion — deleting is soft, so there is no row
    /// missing and no other field to read.
    pub fn deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

/// Which of the developer's two lists a snapshot is describing.
///
/// The project list is the work in hand and stops carrying a conversation the
/// moment it is archived; `orchestration.getArchivedShellSnapshot` is the other
/// half, and is the only way back to one. Both are built by
/// [`crate::threads::Threads::shell_summaries`] from this — a second builder would let the world
/// a client draws depend on which of the two answered first, which is what the
/// shared builder exists to prevent.
///
/// An archived conversation is still *here*: it stays in the registry, keeps its
/// transcript and its checkpoints, and is one unarchive away from the list it
/// left. This decides only which snapshot names it.
///
/// **A deleted conversation is on neither.** That is not symmetry with archive —
/// it is what the client's own reducer needs. The archived section of the
/// settings panel takes `snapshot.threads` whole and groups it by project
/// (`SettingsPanels.tsx`, `archivedGroups`), filtering on neither `archivedAt`
/// nor `deletedAt`, so a conversation that was archived and then deleted would
/// be drawn there with an unarchive control on it unless this server leaves it
/// out of the answer. Ticket 10 of the thread-lifecycle effort checked that
/// against the reducer rather than choosing whichever seemed tidier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shelf {
    /// Everything not archived — the list the developer is working from.
    Working,
    /// What they put away.
    Archived,
}

impl Shelf {
    /// Is this conversation on this shelf already?
    ///
    /// The one place the question is answered, and it is asked from two very
    /// different directions: [`crate::threads::Threads::shell_summaries`] filters a snapshot with
    /// it, and `Shell::set_archived` refuses a move that would not move anything.
    /// A second reading of `archived_at` would be a second answer to one
    /// question.
    ///
    /// A deleted conversation is on neither shelf, whatever its `archivedAt`
    /// says — see the note on [`Shelf`] itself. The two commands that read this
    /// as "already there?" never see one, because a deleted conversation is
    /// refused every command before the world is asked
    /// (`crate::orchestration::Shell::dispatch`).
    pub fn holds(&self, thread: &Thread) -> bool {
        if thread.lifecycle.deleted() {
            return false;
        }
        match self {
            Shelf::Working => thread.lifecycle.archived_at.is_none(),
            Shelf::Archived => thread.lifecycle.archived_at.is_some(),
        }
    }

    /// The change that puts a conversation here.
    pub fn arrival(&self) -> Change {
        match self {
            Shelf::Working => Change::Unarchived,
            Shelf::Archived => Change::Archived,
        }
    }
}

/// What the project list's feed is sent about one change.
///
/// The two things `shellReducer.ts` can be told about a conversation, and the
/// reason this is an answer rather than a `bool`: a change is either the whole
/// summary, upserted by id, or the conversation leaving the list. See
/// [`Change::on_the_list`], which decides which, and
/// [`crate::threads::Threads::commit`], which renders it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Listing {
    /// The conversation's shell summary, whole. `thread-upserted`.
    Summary,
    /// The conversation is off this list. `thread-removed`.
    Removal,
}

/// The developer's standing answer that a conversation is finished.
pub const SETTLED: &str = "settled";

/// The developer's standing answer that it is not — the pin an unsettle leaves.
pub const ACTIVE: &str = "active";

/// The reason on a `thread.unsettled` the developer asked for.
///
/// One of the contract's two, and the only one a *command* can carry: the
/// neutral reset is the server's own and must not be forgeable, which is why
/// `ThreadUnsettleCommand.reason` is the single literal `user` while
/// `ThreadUnsettledPayload.reason` is a union of two.
pub const BY_THE_USER: &str = "user";

/// The other of the contract's two: the reason on a reset this server decided
/// for itself, because real work turned up in the conversation.
///
/// **No command can carry it**, and that is the property the asymmetry rests on
/// — see [`pinned_by`]. A user unsettle *pins* a conversation active; this one
/// returns it to no override at all, so it can settle itself again once the
/// burst of work goes stale. A client able to send it could pin a conversation
/// the developer had let go of, or clear a pin they had asked for.
///
/// The three places it is emitted from are [`Change::wakes`].
pub const BY_ACTIVITY: &str = "activity";

/// Why a lifecycle reset was not emitted — a sentence that reaches nobody.
///
/// [`Threads::woken_by`] asks [`Thread::wants_waking`] through the same
/// refusal the archive commands use, because that is the only guard in this
/// module decided under the fold's own lock. A refusal there is a sentence, and
/// this is the honest one; it is a constant rather than a `format!` naming the
/// conversation because this is the *ordinary* answer — most work happens in a
/// conversation nobody has settled — and a sentence nobody renders is not worth
/// building per work-log row.
pub(crate) const NOTHING_TO_WAKE: &str = "There is no inbox state to return this conversation to.";

/// [`NOTHING_TO_WAKE`]'s twin, for the other reset — the ordinary answer for a
/// conversation nobody put to sleep.
pub(crate) const NOTHING_TO_UNSNOOZE: &str = "There is no snooze on this conversation to spend.";

/// A stored settle override, back as one of the contract's two.
///
/// Same reasoning as [`tone`], with a sharper edge: `settledOverride` is a
/// closed set of two on the wire, so a literal the contract does not name fails
/// the client's decode of the *whole* conversation rather than drawing a wrong
/// badge — the argument `CONTEXT.md` makes for the runtime modes. `None` is the
/// answer for anything else, and it is the honest one: no override is what a
/// thread has before anybody settles it.
pub fn settled_override(stored: &str) -> Option<&'static str> {
    match stored {
        SETTLED => Some(SETTLED),
        ACTIVE => Some(ACTIVE),
        _ => None,
    }
}

/// What a `thread.unsettled` carrying this reason leaves behind.
///
/// **The two directions are not symmetrical**, and this is where that lives. A
/// *user* unsettle pins the conversation active, so it stays in the inbox until
/// real work moves it on; an *activity* unsettle returns it to no override at
/// all, so it can settle itself again once the burst of work goes stale.
/// `threadReducer.ts`, `case "thread.unsettled"`, mirrored — a server that
/// disagreed here would pin a conversation a client had let go of.
///
/// [`BY_ACTIVITY`] takes the second branch, and so would any third reason
/// somebody added without reading this: the reducer tests `=== "user"` and
/// treats everything else as neutral, so falling through to `None` is agreement
/// rather than laziness.
pub(crate) fn pinned_by(reason: &str) -> Option<&'static str> {
    match reason {
        BY_THE_USER => Some(ACTIVE),
        _ => None,
    }
}

/// How long a user message with no turn behind it is still a turn about to
/// start rather than stale data.
///
/// `QUEUED_TURN_START_GRACE_MS` in `client-runtime/src/state/threadSettled.ts`.
/// Session adoption takes seconds, so a message still unadopted after this is a
/// start that failed — and without the bound such a conversation could never be
/// settled at all.
const ADOPTION_GRACE_MILLIS: u64 = 2 * 60 * 1_000;

/// The window a queued turn start is believable in: the grace either side of
/// now.
///
/// **Two rendered stamps rather than a number of milliseconds**, because that is
/// the shape of everything it is compared against. Every timestamp on this wire
/// is [`crate::clock`]'s fixed-width UTC rendering — `YYYY-MM-DDTHH:MM:SS.mmmZ`,
/// one length, one zone — so it orders lexicographically, and drawing the window
/// once means the comparisons need no calendar and this module needs no parser.
///
/// Bounded on *both* sides, which is the client's own guard rather than a
/// tidiness: a message timestamp originates on whichever device sent it, so a
/// clock running ahead of this one would otherwise hold a conversation queued
/// for the whole of the skew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adoption {
    earliest: String,
    latest: String,
}

impl Adoption {
    pub fn now() -> Adoption {
        Adoption::around(crate::clock::now_epoch_millis())
    }

    /// The window around a chosen instant. Split out from [`Adoption::now`] for
    /// [`crate::clock::iso_from_epoch`]'s reason — a window that cannot be given
    /// a centre is a window that cannot be tested.
    fn around(millis: u64) -> Adoption {
        Adoption {
            earliest: crate::clock::iso_from_epoch_millis(
                millis.saturating_sub(ADOPTION_GRACE_MILLIS),
            ),
            latest: crate::clock::iso_from_epoch_millis(millis + ADOPTION_GRACE_MILLIS),
        }
    }

    fn covers(&self, at: &str) -> bool {
        self.earliest.as_str() <= at && at <= self.latest.as_str()
    }
}

/// What stands between a conversation and the developer letting it go.
///
/// `canSettle` in `client-runtime/src/state/threadSettled.ts`, where it is
/// authoritative rather than convenient: the client keeps a twin of this list so
/// the interface can refuse before a round trip, and the list is deliberately the
/// same one `effectiveSettled` refuses to *classify* with. Anything that will not
/// classify as settled must not be accepted as a settle target either, or the
/// developer would be told a conversation had left the inbox and then watch it
/// stay.
///
/// **The order is the client's order and it is load-bearing**, because it decides
/// the sentence a refusal shows: an agent that has asked for permission is also
/// running, and "waiting for your decision" is the more useful of the two things
/// to say about it.
///
/// Snooze refuses on a subset of these — a running session *is* snoozable,
/// because snooze governs attention and never the agent. [`Attention`] is which
/// subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// The agent asked for permission and has not been answered.
    Approval,
    /// The agent asked a question and has not been answered.
    Question,
    /// The process is starting or running.
    Session,
    /// The developer asked for a turn and no session has picked it up yet.
    QueuedTurn,
}

/// Which of the developer's two ways of putting a conversation out of sight the
/// blockers are being asked about.
///
/// `canSettle` and `canSnooze` are two readings of one list in
/// `client-runtime/src/state/threadSettled.ts`, and the difference is a single
/// entry: a working agent blocks a settle and does not block a snooze. So this
/// is a parameter on [`Thread::busy`] rather than a second function beside it —
/// two functions would be two copies of the order, and the order is what decides
/// which sentence a refusal shows.
///
/// **It is asked where the blocker is asked, not applied to the answer.** A
/// conversation can be both working and holding an unadopted turn, and `busy`
/// answers with the first blocker in the client's order — so filtering a
/// `Session` out of the answer would report nothing at all and let a snooze hide
/// the queued turn behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    /// Leaving the inbox as finished — `canSettle`, all four blockers.
    Settling,
    /// Leaving it until a time the developer chose — `canSnooze`, which is the
    /// same list without the live session.
    ///
    /// The three that remain are the ones a snooze would *hide*: a request the
    /// agent is blocked on the developer for, and a turn about to start. Work in
    /// progress is not hidden by a snooze, because the conversation comes back
    /// on its own.
    Snoozing,
}

impl Attention {
    /// Does a working agent stand in the way of this?
    ///
    /// The whole of the difference between the two lists, in the one place it can
    /// be read — so a third caller cannot invent a third answer to it.
    fn minds_a_working_agent(&self) -> bool {
        matches!(self, Attention::Settling)
    }
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
    pub provider: crate::provider::ProviderIdentity,
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
    pub lifecycle: Lifecycle,
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
    /// `None` until [`crate::threads::Threads::apply`] takes a number — an activity is built
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
        let mut detail = json!({
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
            "messages": self.messages.iter().map(Message::to_value).collect::<Vec<Value>>(),
            "proposedPlans": [],
            "activities": self.activities.iter().map(Activity::to_value).collect::<Vec<Value>>(),
            "checkpoints": self
                .checkpoints
                .iter()
                .map(Checkpoint::to_value)
                .collect::<Vec<Value>>(),
            "session": self.session.as_ref().map(|session| session.to_value(
                &self.id,
                &self.provider,
            )),
        });
        self.lifecycle.write_onto(&mut detail);
        detail
    }

    /// The `OrchestrationThreadShell` the project list carries — the same thread
    /// without its transcript, plus the three flags the inbox sorts on.
    ///
    /// The first two are real: a thread the agent has asked permission on — or
    /// asked a question of — raises its hand in the thread list, which is what
    /// makes a conversation waiting on the developer findable from another one.
    /// Both are folded out of the work log rather than counted beside it — see
    /// [`Thread::has_pending_approvals`], which is also what
    /// [`Thread::busy`] refuses a settle with, so the flag the list renders and
    /// the invariant the command enforces are one answer. The third stays `false`
    /// and is a later ticket's: a proposed
    /// plan needs `ExitPlanMode` answered rather than merely reported, and a
    /// `true` nothing could act on would put a badge on a thread with nothing
    /// behind it.
    pub fn to_shell_value(&self) -> Value {
        let mut summary = json!({
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
            "session": self.session.as_ref().map(|session| session.to_value(
                &self.id,
                &self.provider,
            )),
            "latestUserMessageAt": self.latest_user_message_at,
            // Linear in the work log, and only for the shell summary, which a
            // delta and an activity both skip ([`Change::reaches_the_shell`]).
            "hasPendingApprovals": self.has_pending_approvals(),
            "hasPendingUserInput": self.has_pending_user_input(),
            "hasActionableProposedPlan": false,
        });
        self.lifecycle.write_onto(&mut summary);
        summary
    }

    /// Is the agent waiting on a permission decision?
    ///
    /// **Derived from the work log rather than counted beside it**, because the
    /// client derives its own panel from the same rows — a counter kept here
    /// would be a second answer to one question, and the two would agree until
    /// they did not. Deriving it is also what makes it survive a restart: the
    /// activities are stored, and a count in memory would not be.
    ///
    /// One function rather than the fold written out at each call, because it is
    /// read from two places that must not be allowed to disagree: the flag
    /// [`Thread::to_shell_value`] publishes, and the guard [`Thread::busy`]
    /// refuses a settle with. That the published flag and the enforced invariant
    /// are one answer is the whole of why the client's twin of the invariant can
    /// be trusted to agree with this one.
    fn has_pending_approvals(&self) -> bool {
        !crate::worklog::unanswered(&self.activities).is_empty()
    }

    /// Is the agent waiting on an answer to a question?
    /// [`Thread::has_pending_approvals`]'s twin, and separate for the reason the
    /// two folds are separate in the client: a question that arrived as an
    /// approval is a bug, so a reader that accepted either would miss it.
    fn has_pending_user_input(&self) -> bool {
        !crate::worklog::unanswered_user_input(&self.activities).is_empty()
    }

    /// What stands between this conversation and the developer settling it, if
    /// anything does.
    ///
    /// The four checks of `canSettle`, in its order, over the same fields the
    /// shell summary publishes — which is what makes this and the client's twin
    /// one rule rather than two: both read the pending flags out of the work log,
    /// the session's status, and the latest turn against the latest user message.
    /// The first two are literally the flags the summary carries, because they
    /// come from the same two functions.
    ///
    /// `canSnooze` is the same list read once more with the session left out, and
    /// it is [`Attention`] that says so. The session check is *skipped* rather
    /// than its answer discarded: see [`Attention`], where the conversation that
    /// makes those two different is written down.
    pub fn busy(&self, adoption: &Adoption, about: Attention) -> Option<Busy> {
        if self.has_pending_approvals() {
            return Some(Busy::Approval);
        }
        if self.has_pending_user_input() {
            return Some(Busy::Question);
        }
        if about.minds_a_working_agent()
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.status.is_working())
        {
            return Some(Busy::Session);
        }
        self.has_queued_turn_start(adoption)
            .then_some(Busy::QueuedTurn)
    }

    /// Is there an inbox state here for real work to reset?
    ///
    /// The guard on all three of [`Change::wakes`]'s triggers, and both
    /// halves of it are about not moving something nobody asked to have moved:
    ///
    /// - **An override to clear.** A reset over a conversation with none lands in
    ///   [`Change::re_emitted_at`] as a repeat, which would publish a no-op event
    ///   at a stale `updatedAt` — a conversation reordered by work that changed
    ///   nothing about it, or not reordered by work that did.
    /// - **A conversation that is in the inbox at all.** [`Shelf::holds`] is
    ///   asked here as well as by both settle commands, so the filter and the
    ///   rule stay one rule: an archived conversation has no inbox to be returned
    ///   to, and clearing an override that `thread.unsettle` itself refuses to
    ///   touch would lose the developer's decision the moment they unarchived it.
    ///   A **deleted** conversation is on neither shelf either, and that answer
    ///   is reachable rather than theoretical: deleting does not stop a session,
    ///   so an agent still winding down behind one goes on producing exactly the
    ///   three triggers. Waking it would move a conversation that has left both
    ///   of the developer's lists, and no command could put the override back.
    ///
    /// Live work is never hidden either way, and that is what makes the archived
    /// half safe rather than a hole: the client's `effectiveSettled` checks its
    /// activity blockers — a pending approval or question, a session that is
    /// working, an unadopted turn — *before* it reads any override, so a
    /// conversation unarchived while its agent is busy does not classify as
    /// settled whatever these two fields say.
    pub(crate) fn wants_waking(&self) -> bool {
        self.lifecycle.settled_override.is_some() && Shelf::Working.holds(self)
    }

    /// Is there a return ticket here for a new message to spend?
    ///
    /// [`Thread::wants_waking`]'s twin over the other pair of fields, and both
    /// halves are the same two questions asked about a snooze: a conversation
    /// with no wake time set has nothing to clear and would land in
    /// [`Change::re_emitted_at`] as a repeat, and an archived one is refused for
    /// the reason [`Shelf::holds`] is asked at all — `thread.snooze` and
    /// `thread.unsnooze` both refuse an archived conversation, so a reset that
    /// did not would spend a snooze no command could have set or cleared. The
    /// same shelf reading covers a deleted conversation, and for the same reason
    /// spelled out in [`Thread::wants_waking`].
    pub(crate) fn wants_unsnoozing(&self) -> bool {
        self.lifecycle.snoozed_until.is_some() && Shelf::Working.holds(self)
    }

    /// A message the developer sent that no session has picked up yet.
    ///
    /// `hasQueuedTurnStart`, mirrored. The turn was asked for — the message is in
    /// the transcript — but no session adopted it, so the work is invisible to the
    /// status checks above: `session` is still `None` and nothing is running.
    /// Detected as a user message strictly newer than every stamp on the latest
    /// turn, because adoption gives the new turn a `requestedAt` at or after the
    /// message time and so clears the condition by itself.
    ///
    /// A session that already **failed** is not queued: the failure is the
    /// developer's answer about what happened to that message, and it is already
    /// visible on the conversation.
    ///
    /// The comparisons are string comparisons, which [`Adoption`] argues: every
    /// stamp here was rendered by [`crate::clock`] or by the registry's own
    /// `strftime` in the one fixed shape, so lexicographic order *is*
    /// chronological order.
    fn has_queued_turn_start(&self, adoption: &Adoption) -> bool {
        let Some(message_at) = self.latest_user_message_at.as_deref() else {
            return false;
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.status == SessionStatus::Error)
        {
            return false;
        }
        if !adoption.covers(message_at) {
            return false;
        }
        let Some(turn) = self.latest_turn.as_ref() else {
            return true;
        };
        [Some(turn.requested_at.as_str()), turn.started_at.as_deref(), turn.completed_at.as_deref()]
            .into_iter()
            .flatten()
            .all(|stamp| stamp < message_at)
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
            provider: self.provider.clone(),
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
            lifecycle: self.lifecycle.clone(),
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
            provider: row.provider,
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
            // Kept for the same reason the checkpoints are: an archived or
            // snoozed conversation that came back from a restart in the inbox
            // would undo the developer's curation every time the window opened.
            lifecycle: row.lifecycle,
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

    pub(crate) fn to_value(&self) -> Value {
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
    fn to_value(
        &self,
        thread_id: &str,
        provider: &crate::provider::ProviderIdentity,
    ) -> Value {
        json!({
            "threadId": thread_id,
            "status": self.status.as_str(),
            "providerName": provider.driver,
            "providerInstanceId": provider.instance_id,
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

/// A field that may be absent, may be `null`, and may carry a value.
///
/// Three states, which is one more than an `Option` holds — and the distinction
/// is load-bearing on `thread.meta.update`: the client sends only the fields it
/// means to move, so **absent has to mean "leave this alone"** while an explicit
/// `null` means "clear it". The composer relies on both at once, sending
/// `{branch, worktreePath: null}` to move a conversation onto a branch and out of
/// whatever worktree it was in (`ChatView.logic.ts`,
/// `resolveThreadMetadataUpdateForNextTurn`).
pub type Given<T> = Option<Option<T>>;

/// What a `thread.meta.update` asked to change about a conversation.
///
/// Every field is optional and only the present ones are applied, which mirrors
/// the client's own reducer (`threadReducer.ts`, `case "thread.meta-updated"`,
/// which spreads in each field only when it is not `undefined`). The mirroring is
/// the rule the whole fold follows: a client that watched every event and one that
/// arrives late and takes a snapshot have to see the same conversation.
///
/// The two nullable fields are [`Given`] rather than `Option`, because the client
/// clears them by sending `null` and a server that could not tell that from an
/// absent field would either never clear one or clear it on every write.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaUpdate {
    pub title: Option<String>,
    pub model_selection: Option<Value>,
    pub branch: Given<String>,
    pub worktree_path: Given<String>,
}

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
    /// The conversation's own description changed. `thread.meta-updated`.
    ///
    /// Four fields, each of which may be absent — see [`MetaUpdate`], where what
    /// "absent" has to mean is argued.
    MetaUpdated(MetaUpdate),
    /// The developer put a conversation away. `thread.archived`.
    ///
    /// **Archiving is not deleting**, and nothing here says otherwise: the only
    /// field it moves is [`Lifecycle::archived_at`], so the transcript, the work
    /// log and the checkpoints are exactly where they were. What changes is
    /// which snapshot names the conversation — see [`Shelf`].
    ///
    /// The stamp is the moment this server committed, which is also the
    /// `updatedAt` [`crate::threads::Threads::apply`] is about to put on the thread. The client's
    /// reducer reads both out of the payload (`threadReducer.ts`,
    /// `case "thread.archived"`), so sending one and not the other would leave a
    /// window that watched the archive disagreeing with one that reloaded after
    /// it about when the conversation last changed.
    Archived,
    /// The developer took one back out. `thread.unarchived`.
    ///
    /// [`Change::Archived`]'s twin, and it clears the stamp rather than writing a
    /// second one: the contract's `ThreadUnarchivedPayload` is the conversation
    /// and an `updatedAt`, with no stamp of its own, because there is no such
    /// thing as when a conversation stopped being archived — it either is or it
    /// is not.
    Unarchived,
    /// The developer decided a conversation is finished with. `thread.settled`.
    ///
    /// **Not [`crate::settling`]**, which reads a session status as how a *turn*
    /// went. This is whether a *thread* belongs in the inbox — see [`Lifecycle`]
    /// and the **Inbox state** entry in `CONTEXT.md`, where the collision and its
    /// seniority are written down.
    ///
    /// It moves two fields and tells nobody else: the agent, if one is there, is
    /// not spoken to, and the transcript, the work log and the checkpoints are
    /// exactly where they were. Settling is a decision about the developer's
    /// attention, in the same way archiving is about their list.
    ///
    /// **This server does not decide what the inbox shows.** `effectiveSettled`
    /// reads the two fields this writes alongside four other things, and it lives
    /// in the client. What is enforced here is which conversations may be
    /// *targeted* — see [`Busy`].
    Settled,
    /// The developer pinned a conversation back. `thread.unsettled`.
    ///
    /// The reason is one of the contract's two and decides what the conversation
    /// is left in — see [`pinned_by`], where the asymmetry is argued. A command
    /// sends only [`BY_THE_USER`]; the neutral [`BY_ACTIVITY`] reset is this
    /// server's own and is emitted from [`Change::wakes`].
    ///
    /// It clears `settledAt` rather than stamping a second time, for
    /// [`Change::Unarchived`]'s reason: there is no such thing as when a
    /// conversation stopped being settled.
    Unsettled { reason: &'static str },
    /// The developer put a conversation to sleep until a time they chose.
    /// `thread.snoozed`.
    ///
    /// **An overlay rather than a destination.** A snoozed conversation is still
    /// active in this data model — it is not archived, not settled and not
    /// deleted — and these two fields only suppress it from the inbox until the
    /// wake time passes. That is why snooze does not sit in the same vocabulary
    /// slot as [`Change::Archived`].
    ///
    /// **There is no scheduler behind it.** A snooze expires by being *read*:
    /// once `until` is in the past, `effectiveSnoozed` simply stops classifying
    /// the conversation as snoozed, so no event fires when a wake time passes and
    /// there is nothing here to schedule. The same is true of a raised hand — a
    /// snoozed conversation whose agent becomes blocked on the developer stops
    /// classifying without *spending* the snooze, and both derivations live in
    /// the bundled client runtime.
    ///
    /// **And it never touches the agent.** A running session is snoozable, which
    /// is the one thing that makes this different from
    /// [`Change::SessionStopRequested`]: a snooze is a decision about the
    /// developer's attention, not an interruption of the work.
    ///
    /// `until` is stored exactly as the developer's client sent it rather than
    /// re-rendered. A wake time this server cannot place on its own clock is
    /// refused at the parse — see [`crate::clock::epoch_millis_from_iso`] — so
    /// what reaches here is already one of this wire's renderings, and
    /// normalising it again would be a second spelling of a moment two readers
    /// would then have to agree about.
    Snoozed { until: String },
    /// The conversation came back. `thread.unsnoozed`.
    ///
    /// [`Change::Unsettled`]'s shape and its asymmetry read the other way round:
    /// the reason is one of the contract's two and *neither* leaves anything
    /// behind, because there is no such thing as a pinned-awake conversation —
    /// both directions clear both fields. What the reason distinguishes is who
    /// decided, which the client renders and this server must not let a client
    /// forge: [`BY_THE_USER`] is the developer saying "wake it now", and
    /// [`BY_ACTIVITY`] is this server spending the return ticket because they
    /// sent a new message.
    ///
    /// **A timer wake is not one of these.** Nothing is emitted when a wake time
    /// passes; the stored fields simply stop classifying. So an `Unsnoozed` is
    /// always somebody's decision, and that is what makes it worth an event.
    Unsnoozed { reason: &'static str },
    /// The developer deleted a conversation. `thread.deleted`.
    ///
    /// **Deleting is soft, and the stamp is the whole of it.** The row stays,
    /// and so do its transcript, its work log and its checkpoints. Three reasons,
    /// none of them squeamishness: the checkpoint refs a turn wrote are real git
    /// objects in the developer's own repository and a hard delete would orphan
    /// them; the threads table cascades, so removing the row would take the
    /// transcript and the work log with it in one statement; and the contract
    /// carries a deletion time on the thread, which is only meaningful if the
    /// thread survives to carry it.
    ///
    /// **It leaves both lists and takes no further commands.** [`Shelf::holds`]
    /// is what keeps it off the two snapshots, [`Change::on_the_list`] is what
    /// keeps its later changes off the project list's feed, and
    /// `crate::orchestration::Shell::dispatch` is what refuses the commands — a
    /// stale window must not go on driving a conversation the developer removed.
    ///
    /// The payload is the contract's `ThreadDeletedPayload`, which is a
    /// `threadId` and a `deletedAt` and — alone among the lifecycle events —
    /// **no `updatedAt`**. The client's reducer folds this to `{kind: "deleted"}`
    /// (`threadReducer.ts`) and keeps none of the thread, so there is no
    /// conversation left for a second stamp to describe.
    Deleted,
    /// The developer moved the conversation's runtime mode.
    /// `thread.runtime-mode-set`.
    ///
    /// The same field [`Change::TurnRequested`] writes when the composer sends a
    /// per-turn override, reached by its own command instead — which is the whole
    /// of what makes the picker mean something between turns rather than only at
    /// the start of one. It does not touch [`Session::runtime_mode`], and that is
    /// the point: the turn in flight stays under the rules it started with, and
    /// the mode reaches the child when the *next* turn is dispatched — see
    /// `crate::session::retune`, which pushes it on the driver and moves the
    /// session's own copy with it.
    RuntimeModeSet { runtime_mode: String },
    /// The developer moved the conversation's interaction mode.
    /// `thread.interaction-mode-set`. [`Change::RuntimeModeSet`]'s twin.
    InteractionModeSet { interaction_mode: String },
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
    /// The developer ended the session. `thread.session-stop-requested`.
    ///
    /// Published when the driver has been *told* rather than when the child has
    /// gone, which is [`Change::InterruptRequested`]'s shape and is why the
    /// contract declares an event for a command that could have been answered
    /// with a session change alone: the process takes a moment to go, and the
    /// developer's click is what should stop the session being drawn as alive.
    /// The `thread.session-set` carrying `stopped` follows once it really has —
    /// see [`crate::session`], where reaping precedes it.
    ///
    /// It folds what the client folds and no more (`threadReducer.ts`,
    /// `case "thread.session-stop-requested"`): the session goes to `stopped`
    /// with no active turn, and the **latest turn is left alone**. Settling the
    /// turn here would be a third copy of the settling rule
    /// ([`crate::settling`]) in a place the client has none, so a window that
    /// watched the stop and one that reloaded after it would disagree about how
    /// the last turn ended.
    ///
    /// A conversation with no session folds to nothing at all, which is why the
    /// command does not publish this when there was no agent to end.
    SessionStopRequested,
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
    /// The developer asked for the working tree to be put back to a turn
    /// boundary. `thread.checkpoint-revert-requested`.
    ///
    /// Published when the revert has been *accepted* rather than when it has
    /// happened, which is the same two-stage shape
    /// [`Change::InterruptRequested`] has and is why the contract declares two
    /// events for one command: restoring a tree touches a disk, and the socket's
    /// only reader must never wait on one. The client folds this as `unchanged`
    /// (`threadReducer.ts`) — it is the receipt, and [`Change::Reverted`] is the
    /// answer.
    RevertRequested { turn_count: u64 },
    /// The working tree has been put back. `thread.reverted`.
    ///
    /// Published *after* the tree has actually been written, never before — the
    /// same rule [`Change::Checkpointed`] follows, for the same reason: an event
    /// that ran ahead of the disk would tell the developer their project had
    /// been restored while it still held the turn they were undoing.
    ///
    /// **This does not move the conversation**, and that is the ticket's own
    /// criterion: a revert moves the working tree, and the thread, its
    /// transcript and its work log are left as they were. The client's reducer
    /// is more eager — it drops the messages, checkpoints and activities after
    /// the turn reverted to — so a window that watched the revert shows a
    /// shorter conversation than one that reloads afterwards. That divergence is
    /// deliberate here and is written down in
    /// `.scratch/thread-lifecycle/issues/05-…`; trimming a transcript is a
    /// deletion, and this ticket is about a tree.
    Reverted { turn_count: u64 },
}

impl Change {
    /// What the project list is told about this change, if anything.
    ///
    /// `thread` is the conversation **as this change has already left it**, which
    /// is what lets one answer cover all three cases below rather than the caller
    /// reading the lifecycle a second time.
    ///
    /// - **A deletion is a removal**, not a summary. `OrchestrationThreadShell`
    ///   does not declare `deletedAt` at all — a `Schema.Struct` drops a key it
    ///   does not name — so a client could not filter a deleted conversation out
    ///   of the list from its summary the way it filters an archived one on
    ///   `archivedAt`. `thread-removed` is the vocabulary the shell reducer
    ///   already has for a conversation leaving the list (`shellReducer.ts`), and
    ///   is what `project.delete` already publishes per conversation.
    /// - **Nothing else about a deleted conversation is news.** An agent may
    ///   still be running behind one — deleting does not stop a session — and
    ///   every `thread.session-set` it publishes would otherwise upsert the
    ///   conversation straight back onto the list the removal just took it off.
    /// - **Everything else is the summary, except a delta and an activity.** A
    ///   turn produces hundreds of deltas and none of them changes anything the
    ///   thread *list* renders — the title, the session state, the latest turn —
    ///   so republishing the summary per token would be the shell subscription
    ///   carrying a token stream it has no use for. A checkpoint is excluded for
    ///   a different reason: the shell summary does not carry `checkpoints` at
    ///   all, so nothing on the list would read one. The two halves of a revert
    ///   go with it on that same reading — the list renders a conversation's
    ///   title, session and latest turn, and a revert moves a working tree rather
    ///   than any of the three.
    pub(crate) fn on_the_list(&self, thread: &Thread) -> Option<Listing> {
        match self {
            Change::Deleted => Some(Listing::Removal),
            _ if thread.lifecycle.deleted() => None,
            Change::AssistantDelta { .. }
            | Change::Activity(_)
            | Change::Checkpointed(_)
            | Change::RevertRequested { .. }
            | Change::Reverted { .. } => None,
            _ => Some(Listing::Summary),
        }
    }

    /// The moment a repeat of this change reports, when the conversation is
    /// already where the change would put it.
    ///
    /// **Idempotence by re-emission**, which the spec asks for by name and only
    /// for the inbox-state commands. Settling something already settled is not
    /// refused — unlike a second archive, which is a click on a control that is no
    /// longer there ([`Shelf`]) — because both directions of a settle are a
    /// standing answer the developer gave rather than a move between two lists, so
    /// folding the event again lands on the same state either way.
    ///
    /// What a repeat must not do is *churn*: the client's thread list is ordered
    /// by when things changed, so a double-click that stamped the clock would move
    /// a conversation the developer did not touch. So the re-emission carries the
    /// conversation's existing `updatedAt` — and, through the fold, its existing
    /// `settledAt` — rather than the current time, which is what makes a duplicate
    /// neither rewind the thread nor reorder the list.
    ///
    /// Every other change stamps the clock unconditionally. [`crate::orchestration::Shell::set_mode`]
    /// argues why that is right for a write of a value the developer chose, and
    /// says this is where it would not be.
    ///
    /// **The cost is an event whose `occurredAt` is older than the one before
    /// it**, which is the one place on this feed where that can happen. It is
    /// safe because nothing orders by it: the client folds by `sequence`, and a
    /// re-emission takes a fresh one like every other change. `crate::clock`'s
    /// "two timestamps taken in order never go backwards" is still true of the
    /// clock — this reads a stamp rather than taking one.
    ///
    /// **A caller must still ask whether the change is worth making.** An
    /// `Unsettled { reason: "activity" }` over a conversation with no override
    /// lands here as a repeat and would publish a no-op event at a stale
    /// `updatedAt` — a conversation reordered, or not, by work that changed
    /// nothing about it. The answer is the guard at the call site, and the one
    /// caller that sends that reason has it: see [`Threads::woken_by`], which
    /// refuses the reset when there is no override to reset.
    pub(crate) fn re_emitted_at(&self, thread: &Thread) -> Option<String> {
        let already = match self {
            Change::Settled => thread.lifecycle.settled_override == Some(SETTLED),
            Change::Unsettled { reason } => thread.lifecycle.settled_override == pinned_by(reason),
            // Keyed on the wake time, because a snooze to a *different* moment is
            // a new decision and has to stamp the clock — see
            // [`Lifecycle::asleep_until`], which both this and [`fold`] ask so
            // that a repeat cannot be a repeat to one of them and not the other.
            Change::Snoozed { until } => thread.lifecycle.asleep_until(until),
            // Waking has one destination whoever asked for it, so any wake of a
            // conversation nobody snoozed is the repeat — there is no second
            // state for a reason to land in, which is where this parts company
            // with [`Change::Unsettled`].
            Change::Unsnoozed { .. } => thread.lifecycle.snoozed_until.is_none(),
            _ => return None,
        };
        already.then(|| thread.updated_at.clone())
    }

    /// Is this the real activity that owes the conversation a place in the inbox
    /// again?
    ///
    /// **Leaving the inbox must never hide something that needs the developer.**
    /// The invariants ([`Busy`]) refuse to create that state when the developer
    /// asks, and it must not be reachable a minute later either: a conversation
    /// settled while quiet whose agent then asks for permission would sit outside
    /// the inbox while blocked on a decision only the developer can make. These
    /// three triggers are what close that, [`Thread::wants_waking`] is the guard
    /// on all of them, and [`Threads::woken_by`] is where the reset is
    /// emitted.
    ///
    /// **It resets an override in either direction**, which is why the answer is
    /// not "does this un-settle it": a conversation the developer pinned *active*
    /// returns to neutral too, so it can settle itself again once the burst of
    /// work goes stale. Both go through [`BY_ACTIVITY`], which [`pinned_by`]
    /// reads as no override at all.
    ///
    /// Two of the three are narrow on purpose:
    ///
    /// - **A session only counts while it is working.** `ready`, `stopped` and
    ///   `error` are a status arriving *after* the fact, and one of those must
    ///   not fight the developer's explicit settle — a conversation whose agent
    ///   has just finished is the ordinary thing to settle, and a `ready` a moment
    ///   later would undo it. [`SessionStatus::is_working`] is the same reading [`Busy`]
    ///   refuses a settle with, deliberately: the settle an agent blocks is the
    ///   settle a starting agent undoes.
    /// - **Only a request that blocks on the developer counts.**
    ///   [`crate::worklog::blocks_on_the_developer`], not any work-log row — a
    ///   settled conversation would otherwise wake on every tool call of every
    ///   turn.
    ///
    /// A turn request is not narrowed, because there is nothing narrower about
    /// it: the developer typed something and pressed enter, which is the
    /// clearest possible statement that they are back in the conversation.
    ///
    /// **The two resets do not have the same triggers**, which is the whole
    /// reason this answers a list rather than a `bool` — see [`Woken`].
    pub(crate) fn wakes(&self) -> &'static [Woken] {
        match self {
            // The developer came back of their own accord, and that spends
            // both: the pin they left and the return ticket they set.
            Change::TurnRequested { .. } => &[Woken::Inbox, Woken::Snooze],
            Change::Session(session) if session.status.is_working() => &[Woken::Inbox],
            Change::Activity(appended) if crate::worklog::blocks_on_the_developer(appended) => {
                &[Woken::Inbox]
            }
            _ => &[],
        }
    }
}

/// A conversation the developer had put out of sight, and what real work in it
/// takes back.
///
/// Two resets rather than one, because they are not spent by the same work and
/// collapsing them would make each wrong in the other's direction:
///
/// - **The inbox override** answers "is this finished?", so an agent that starts
///   working or blocks on the developer contradicts it and it goes.
/// - **The snooze** is a return ticket the developer bought. A session starting
///   or failing does not spend one — the snooze never paused the agent, so work
///   happening is not the developer changing their mind — and a raised hand
///   already stops the conversation *classifying* as snoozed without spending
///   it, which is a derivation that ships in the client. Spending it there would
///   cost the developer the rest of their snooze the moment they dismissed the
///   request.
///
/// What both share is the guard's shape and its home: each is asked under the
/// lock the fold runs under, through the refusal [`crate::threads::Threads::commit`]
/// already takes, and each refuses when there is nothing to reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woken {
    /// The developer's standing answer about whether the conversation is
    /// finished. Ticket 08's three triggers.
    Inbox,
    /// The wake time they chose. A new message, and nothing else.
    Snooze,
}

impl Woken {
    /// The change that performs this reset.
    ///
    /// Both carry [`BY_ACTIVITY`], which is the reason no command can send: a
    /// client able to forge one could pin a conversation the developer had let
    /// go of, or wake one they had put to sleep.
    pub(crate) fn reset(&self) -> Change {
        match self {
            Woken::Inbox => Change::Unsettled {
                reason: BY_ACTIVITY,
            },
            Woken::Snooze => Change::Unsnoozed {
                reason: BY_ACTIVITY,
            },
        }
    }

    /// Why this reset was not emitted, or `None` when it should be.
    ///
    /// A refusal rather than a check before the call, so that "is there anything
    /// to reset?" is decided under the fold's own lock and before a sequence is
    /// taken — two triggers arriving at once cannot then both find something and
    /// both emit. The cost is a sentence no client reads, because nothing
    /// dispatches these.
    pub(crate) fn refusal(&self, thread: &Thread) -> Option<String> {
        let (wanted, why) = match self {
            Woken::Inbox => (thread.wants_waking(), NOTHING_TO_WAKE),
            Woken::Snooze => (thread.wants_unsnoozing(), NOTHING_TO_UNSNOOZE),
        };
        (!wanted).then(|| why.to_string())
    }
}

/// What one change did: the payload describing it, and anything the caller has
/// to act on that this module will not do itself.
///
/// A struct rather than a bare `Value` so that [`fold`] can stay a total
/// function. The one thing it has to report and cannot perform is
/// [`Reconciled`] — see `docs/adr/0025`, where passing the counters in was
/// weighed against this and rejected.
pub struct Rendered {
    /// The `OrchestrationThreadStreamItem` payload for the change, ready to be
    /// wrapped in an event envelope by
    /// [`crate::threads::Threads::commit`].
    pub payload: Value,
    /// Present only when a buffered assistant message replaced an accumulation
    /// of deltas, which is the one case there is anything to say.
    pub reconciled: Option<Reconciled>,
}

/// Whether the deltas had already built exactly the message that replaced them.
///
/// The reconciliation assumption, checked on every turn rather than in a test:
/// the deltas drive live rendering and the buffered message is authoritative, so
/// the two agreeing is the thing that makes the live rendering honest. Answered
/// here as a value and counted by [`crate::threads::Threads::commit`], because
/// counting is a shared atomic and this module has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    /// The buffered message and the accumulated deltas were the same string.
    Matched,
    /// They were not, and how far apart they were. The caller says so on
    /// stderr; nothing about the conversation changes either way, because the
    /// buffered message wins regardless.
    Replaced { streamed: usize, buffered: usize },
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
/// than read here because [`crate::threads::Threads::apply`] holds the log open
/// around both. `at` is the moment the change happened, decided by
/// [`crate::threads::Threads::commit`] for the reason [`Change::re_emitted_at`]
/// gives, so that nothing here has to read a clock.
pub fn fold(thread: &mut Thread, change: &Change, sequence: i64, at: &str) -> Rendered {
    let mut reconciled = None;
    let payload = match change {
        Change::UserMessage {
            message_id,
            text,
            turn_id,
        } => {
            thread.latest_user_message_at = Some(at.to_string());
            let (payload, verdict) =
                message_sent(thread, message_id, "user", text, Some(turn_id), false, at);
            reconciled = verdict;
            payload
        }
        // The client's reducer, mirrored twice over: each field is applied
        // only if it was sent, and the payload carries only the fields that
        // were — so a subscriber folding this event moves exactly what the
        // stored thread just moved, and nothing else. An event that named all
        // four every time would have a title-only rename claim to have set
        // the model and the branch as well, which is a different account of
        // what happened; the reducer distinguishes the two, so this does.
        Change::MetaUpdated(update) => {
            let mut payload = json!({"threadId": thread.id, "updatedAt": at});
            let described = payload
                .as_object_mut()
                .expect("the payload above is an object");
            if let Some(title) = &update.title {
                thread.title = title.clone();
                described.insert("title".to_string(), json!(title));
            }
            if let Some(selection) = &update.model_selection {
                thread.model_selection = selection.clone();
                described.insert("modelSelection".to_string(), selection.clone());
            }
            if let Some(branch) = &update.branch {
                thread.branch = branch.clone();
                described.insert("branch".to_string(), json!(branch));
            }
            if let Some(worktree_path) = &update.worktree_path {
                thread.worktree_path = worktree_path.clone();
                described.insert("worktreePath".to_string(), json!(worktree_path));
            }
            payload
        }
        // The client's reducer, mirrored: one field moves, and the payload
        // carries the same two values the reducer writes onto the thread
        // (`threadReducer.ts`, `case "thread.archived"`). Nothing else is
        // touched — archiving a conversation is not a way of ending it, so
        // the session, the latest turn and the transcript are all left as
        // they are.
        Change::Archived => {
            thread.lifecycle.archived_at = Some(at.to_string());
            json!({
                "threadId": thread.id,
                "archivedAt": at,
                "updatedAt": at,
            })
        }
        Change::Unarchived => {
            thread.lifecycle.archived_at = None;
            json!({
                "threadId": thread.id,
                "updatedAt": at,
            })
        }
        // The client's reducer, mirrored: the override goes to `settled` and
        // the payload's `settledAt` is written straight onto the thread
        // (`threadReducer.ts`, `case "thread.settled"`).
        //
        // A conversation already settled keeps the moment it settled at rather
        // than being stamped again — and `at` is its existing `updatedAt` on
        // such a repeat, so the whole re-emission reports where the
        // conversation already was. See [`Change::re_emitted_at`], which is
        // where that is decided; here it is one `unwrap_or_else` because the
        // stamp and the override are written together or not at all.
        Change::Settled => {
            let settled_at = thread
                .lifecycle
                .settled_at
                .clone()
                .unwrap_or_else(|| at.to_string());
            thread.lifecycle.settled_override = Some(SETTLED);
            thread.lifecycle.settled_at = Some(settled_at.clone());
            json!({
                "threadId": thread.id,
                "settledAt": settled_at,
                "updatedAt": at,
            })
        }
        Change::Unsettled { reason } => {
            thread.lifecycle.settled_override = pinned_by(reason);
            thread.lifecycle.settled_at = None;
            json!({
                "threadId": thread.id,
                "reason": reason,
                "updatedAt": at,
            })
        }
        // The client's reducer, mirrored: both fields move and the payload
        // carries both, because that is what the reducer writes onto the thread
        // (`threadReducer.ts`, `case "thread.snoozed"`).
        //
        // The moment the developer asked is kept only across a *repeat* — a
        // snooze to a wake time the conversation is already asleep until, where
        // `at` is its existing `updatedAt` and the whole re-emission reports
        // where it already was. Choosing a different time restamps it, and that
        // is the half worth being careful about: the client measures a raised
        // hand against `snoozedAt` (`threadRaisedHandWhileSnoozed`), so a second
        // snooze carrying the first one's stamp would be woken immediately by
        // the work the developer had just decided to sleep through.
        Change::Snoozed { until } => {
            let snoozed_at = match thread.lifecycle.asleep_until(until) {
                true => thread
                    .lifecycle
                    .snoozed_at
                    .clone()
                    .unwrap_or_else(|| at.to_string()),
                false => at.to_string(),
            };
            thread.lifecycle.snoozed_until = Some(until.clone());
            thread.lifecycle.snoozed_at = Some(snoozed_at.clone());
            json!({
                "threadId": thread.id,
                "snoozedUntil": until,
                "snoozedAt": snoozed_at,
                "updatedAt": at,
            })
        }
        // Both fields, never one. A `snoozedAt` left behind is a conversation
        // the client reads as never snoozed and this server reads as snoozed at
        // a moment it no longer is — and `threadWokeAt` renders exactly that
        // stamp into a "Woke" indicator for a wake the developer has already
        // dealt with.
        Change::Unsnoozed { reason } => {
            thread.lifecycle.snoozed_until = None;
            thread.lifecycle.snoozed_at = None;
            json!({
                "threadId": thread.id,
                "reason": reason,
                "updatedAt": at,
            })
        }
        // One field, and it is the only one this change touches: the
        // transcript, the work log, the checkpoints, the session and the other
        // five lifecycle fields are all left exactly as they were, because
        // deleting is soft and this stamp is the whole of it.
        //
        // The payload is the contract's two keys and no third. `updatedAt` is
        // absent from `ThreadDeletedPayload` — alone among the lifecycle
        // payloads — because the client's reducer keeps none of the thread
        // after folding this (`threadReducer.ts`, `case "thread.deleted"`), so
        // there is nothing left for a second stamp to describe. The thread's
        // own `updatedAt` still moves, in `Threads::commit` with every other
        // change's.
        Change::Deleted => {
            thread.lifecycle.deleted_at = Some(at.to_string());
            json!({
                "threadId": thread.id,
                "deletedAt": at,
            })
        }
        // The client's reducer, mirrored: one field and `updatedAt`, and
        // nothing else moves. `updatedAt` is the payload's own key here
        // rather than only the thread's, because that is what the reducer
        // reads it out of (`threadReducer.ts`,
        // `case "thread.runtime-mode-set"`).
        Change::RuntimeModeSet { runtime_mode } => {
            thread.runtime_mode = runtime_mode.clone();
            json!({
                "threadId": thread.id,
                "runtimeMode": thread.runtime_mode,
                "updatedAt": at,
            })
        }
        Change::InteractionModeSet { interaction_mode } => {
            thread.interaction_mode = interaction_mode.clone();
            json!({
                "threadId": thread.id,
                "interactionMode": thread.interaction_mode,
                "updatedAt": at,
            })
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
        } => {
            let (payload, verdict) = message_sent(
                thread,
                message_id,
                "assistant",
                text,
                Some(turn_id),
                true,
                at,
            );
            reconciled = verdict;
            payload
        }
        Change::AssistantMessage {
            message_id,
            turn_id,
            text,
        } => {
            let (payload, verdict) = message_sent(
                thread,
                message_id,
                "assistant",
                text,
                Some(turn_id),
                false,
                at,
            );
            reconciled = verdict;
            payload
        }
        Change::Session(session) => {
            thread.latest_turn = settle(thread.latest_turn.take(), session);
            thread.session = Some(session.clone());
            json!({
                "threadId": thread.id,
                "session": session.to_value(
                    &thread.id,
                    &thread.provider,
                ),
            })
        }
        // The client's reducer, mirrored down to what it leaves alone: the
        // session stops and gives up its turn, and the *latest turn* is not
        // settled here. See [`Change::SessionStopRequested`], where both
        // halves of that are argued.
        //
        // `createdAt` is the moment this server committed, like every other
        // stamp on this feed, and it is also what the reducer puts on the
        // session — so the two copies of the session agree on when it
        // stopped.
        Change::SessionStopRequested => {
            if let Some(session) = thread.session.as_mut() {
                session.status = SessionStatus::Stopped;
                session.active_turn_id = None;
                session.updated_at = at.to_string();
            }
            json!({
                "threadId": thread.id,
                "createdAt": at,
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
        // Neither half of a revert folds anything in, which is what the
        // ticket asks for: the conversation is left as it was and the
        // working tree is what moved. So both are payload only — the
        // `ThreadCheckpointRevertRequestedPayload` and `ThreadRevertedPayload`
        // the contract declares, which differ in exactly one field.
        Change::RevertRequested { turn_count } => json!({
            "threadId": thread.id,
            "turnCount": turn_count,
            "createdAt": at,
        }),
        Change::Reverted { turn_count } => json!({
            "threadId": thread.id,
            "turnCount": turn_count,
        }),
    };

    Rendered {
        payload,
        reconciled,
    }
}

/// Append or replace a message, and move the latest turn with it.
///
/// The two-line rule at the heart of the ticket: a streaming send appends,
/// a buffered one replaces. Everything else here is the turn bookkeeping
/// that has to stay in step with it.
/// The reconciliation verdict is returned rather than counted here, which is
/// the whole of why this module needs no `&self` — see `docs/adr/0025`.
#[allow(clippy::too_many_arguments)]
fn message_sent(
    thread: &mut Thread,
    message_id: &str,
    role: &str,
    text: &str,
    turn_id: Option<&String>,
    streaming: bool,
    at: &str,
) -> (Value, Option<Reconciled>) {
    let mut reconciled = None;
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
                    // The answer leaves as a value; the caller is what counts
                    // it and says so — see [`Reconciled`].
                    reconciled = Some(if existing.text == text {
                        Reconciled::Matched
                    } else {
                        Reconciled::Replaced {
                            streamed: existing.text.len(),
                            buffered: text.len(),
                        }
                    });
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

    let payload = json!({
        "threadId": thread.id,
        "messageId": message_id,
        "role": role,
        "text": text,
        "turnId": turn_id,
        "streaming": streaming,
        "createdAt": at,
        "updatedAt": at,
    });

    (payload, reconciled)
}

impl Change {
    pub(crate) fn event_type(&self) -> &'static str {
        match self {
            Change::UserMessage { .. }
            | Change::AssistantDelta { .. }
            | Change::AssistantMessage { .. } => "thread.message-sent",
            Change::MetaUpdated(_) => "thread.meta-updated",
            Change::Archived => "thread.archived",
            Change::Unarchived => "thread.unarchived",
            Change::Settled => "thread.settled",
            Change::Unsettled { .. } => "thread.unsettled",
            Change::Snoozed { .. } => "thread.snoozed",
            Change::Unsnoozed { .. } => "thread.unsnoozed",
            Change::Deleted => "thread.deleted",
            Change::RuntimeModeSet { .. } => "thread.runtime-mode-set",
            Change::InteractionModeSet { .. } => "thread.interaction-mode-set",
            Change::TurnRequested { .. } => "thread.turn-start-requested",
            Change::InterruptRequested { .. } => "thread.turn-interrupt-requested",
            Change::Session(_) => "thread.session-set",
            Change::SessionStopRequested => "thread.session-stop-requested",
            Change::Activity(_) => "thread.activity-appended",
            Change::Checkpointed(_) => "thread.turn-diff-completed",
            Change::RevertRequested { .. } => "thread.checkpoint-revert-requested",
            Change::Reverted { .. } => "thread.reverted",
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
pub(crate) fn durable(thread: &Thread, change: &Change) -> Vec<Write> {
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
// Identifiers
// ---------------------------------------------------------------------------

/// An identifier no other one in this process will equal.
///
/// The contract types every id as a trimmed non-empty string rather than a
/// UUID, so what these have to be is unique and readable — and readable is worth
/// something, because these ids appear in the transcript a developer is
/// debugging. The process stamp is what keeps a restart from re-issuing ids a
/// client has cached under a different meaning.
pub(crate) fn fresh_id(prefix: &str) -> String {
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
pub(crate) mod tests {
    use super::*;

    use serde_json::json;

    /// An agent working on a turn.
    pub(crate) fn running(turn_id: &str) -> Session {
        Session {
            status: SessionStatus::Running,
            runtime_mode: "full-access".to_string(),
            active_turn_id: Some(turn_id.to_string()),
            last_error: None,
            updated_at: now_iso(),
        }
    }

    /// The least conversation there can be. Shared with `crate::rpc`'s tests,
    /// which need one to subscribe to and have no other way to make one.
    pub(crate) fn a_thread(id: &str) -> Thread {
        Thread {
            id: id.to_string(),
            project_id: "project-1".to_string(),
            title: "A conversation".to_string(),
            provider: crate::provider::registration(crate::provider::CLAUDE_INSTANCE_ID)
                .expect("the Claude driver is registered")
                .identity(),
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
            lifecycle: Lifecycle::default(),
        }
    }

    /// A stored override that is not one of the contract's two is no override.
    /// Passing it through would fail the client's decode of the whole
    /// conversation — see [`settled_override`].
    #[test]
    fn a_settle_override_the_contract_does_not_name_is_no_override() {
        assert_eq!(settled_override("settled"), Some("settled"));
        assert_eq!(settled_override("active"), Some("active"));
        assert_eq!(settled_override("archived"), None);
        assert_eq!(settled_override(""), None);
    }

    // -- what may be settled -------------------------------------------------

    /// A moment inside the adoption window, and one outside it, as the two ends
    /// of the same clock: the tests below pick a `now` and then place a message
    /// relative to it, rather than reading the real clock and hoping.
    const NOW_MILLIS: u64 = 1_800_000_000_000;

    fn at(offset_millis: i64) -> String {
        crate::clock::iso_from_epoch_millis(
            (NOW_MILLIS as i64 + offset_millis).try_into().expect("an instant after the epoch"),
        )
    }

    /// A conversation with a message the developer sent a moment ago and nothing
    /// that has picked it up — the gap between asking for a turn and a session
    /// adopting one.
    fn with_a_queued_message(offset_millis: i64) -> Thread {
        Thread {
            latest_user_message_at: Some(at(offset_millis)),
            ..a_thread("thread-1")
        }
    }

    /// An unanswered permission request, as the work log records one.
    fn asked_for_permission(request_id: &str) -> Activity {
        crate::worklog::requested(
            &crate::protocol::Permission {
                request_id: request_id.to_string(),
                tool_name: "Write".to_string(),
                input: json!({"file_path": "note.txt"}),
                tool_use_id: Some("toolu_1".to_string()),
                description: None,
                suggestions: Vec::new(),
            },
            Some("turn-1".to_string()),
        )
    }

    /// A conversation with nothing happening is settleable, which is the case
    /// every other one below is a departure from.
    #[test]
    fn a_quiet_conversation_is_not_busy() {
        assert_eq!(a_thread("thread-1").busy(&Adoption::around(NOW_MILLIS), Attention::Settling), None);
    }

    /// The four blockers of `canSettle`, each on its own, and in the order the
    /// client checks them — which is the order the sentences depend on: an agent
    /// that has asked for permission is *also* running, and the request is the
    /// more useful of the two things to say.
    #[test]
    fn every_blocker_the_client_refuses_to_classify_with_is_refused_as_a_target() {
        let window = Adoption::around(NOW_MILLIS);

        let waiting = Thread {
            activities: vec![asked_for_permission("req-1")],
            session: Some(running("turn-1")),
            ..a_thread("thread-1")
        };
        assert_eq!(
            waiting.busy(&window, Attention::Settling),
            Some(Busy::Approval),
            "a request waiting on the developer outranks the session it came from"
        );

        let asked = Thread {
            activities: vec![crate::worklog::user_input_requested(
                &crate::protocol::Permission {
                    request_id: "req-2".to_string(),
                    tool_name: "AskUserQuestion".to_string(),
                    input: json!({}),
                    tool_use_id: None,
                    description: None,
                    suggestions: Vec::new(),
                },
                vec![json!({"id": "q", "question": "q", "options": []})],
                Some("turn-1".to_string()),
            )],
            ..a_thread("thread-1")
        };
        assert_eq!(asked.busy(&window, Attention::Settling), Some(Busy::Question));

        for status in [SessionStatus::Starting, SessionStatus::Running] {
            let working = Thread {
                session: Some(Session {
                    status,
                    ..running("turn-1")
                }),
                ..a_thread("thread-1")
            };
            assert_eq!(
                working.busy(&window, Attention::Settling),
                Some(Busy::Session),
                "{} is work in progress",
                status.as_str()
            );
        }

        assert_eq!(
            with_a_queued_message(-1_000).busy(&window, Attention::Settling),
            Some(Busy::QueuedTurn),
            "a turn asked for a second ago and not yet picked up"
        );
    }

    /// A snooze to a moment the conversation is already asleep until is a
    /// repeat; a snooze to any other moment is a new decision.
    ///
    /// Decided here, where no clock is involved, because that is the only place
    /// it *can* be decided honestly: two snoozes dispatched back to back can land
    /// in the same millisecond, so a test comparing their stamps would be
    /// asserting the clock's resolution rather than this rule. What follows from
    /// it — a new decision restamps `snoozedAt`, which is the baseline the client
    /// measures a raised hand against — is [`fold`]'s, and reads the same
    /// question through [`Lifecycle::asleep_until`].
    #[test]
    fn a_snooze_to_another_time_is_not_a_repeat_of_the_first() {
        let wake = "2026-07-31T09:00:00.000Z";
        let asleep = Thread {
            lifecycle: Lifecycle {
                snoozed_until: Some(wake.to_string()),
                snoozed_at: Some("2026-07-30T09:00:00.000Z".to_string()),
                ..Lifecycle::default()
            },
            ..a_thread("thread-1")
        };

        assert_eq!(
            Change::Snoozed {
                until: wake.to_string()
            }
            .re_emitted_at(&asleep),
            Some(asleep.updated_at.clone()),
            "the same wake time twice is the double-click, and must not churn"
        );
        assert_eq!(
            Change::Snoozed {
                until: "2026-08-01T09:00:00.000Z".to_string()
            }
            .re_emitted_at(&asleep),
            None,
            "a different wake time was folded as a repeat of the first"
        );

        // And waking: a conversation with a snooze on it is not where an
        // `Unsnoozed` would leave it, so that is no repeat either.
        assert_eq!(
            Change::Unsnoozed {
                reason: BY_THE_USER
            }
            .re_emitted_at(&asleep),
            None
        );
        assert_eq!(
            Change::Unsnoozed {
                reason: BY_THE_USER
            }
            .re_emitted_at(&a_thread("thread-1")),
            Some(a_thread("thread-1").updated_at),
            "waking a conversation nobody snoozed must not stamp the clock"
        );
    }

    /// A live agent stands between a conversation and being settled, and not
    /// between it and being snoozed — the one entry where `canSnooze` and
    /// `canSettle` differ.
    ///
    /// Snooze governs the developer's attention and never the agent: the work
    /// carries on, and only where the conversation is drawn changes. Settling
    /// says the conversation is *finished*, which is a claim a running agent
    /// contradicts.
    #[test]
    fn a_working_agent_blocks_a_settle_and_lets_a_snooze_through() {
        let window = Adoption::around(NOW_MILLIS);
        let working = Thread {
            session: Some(running("turn-1")),
            ..a_thread("thread-1")
        };

        assert_eq!(working.busy(&window, Attention::Settling), Some(Busy::Session));
        assert_eq!(
            working.busy(&window, Attention::Snoozing),
            None,
            "a live session is not a blocker for a snooze"
        );
    }

    /// Skipping the session does not skip what stands *behind* it.
    ///
    /// The case a filter over the settle answer would get wrong rather than
    /// merely differ on: a conversation with both a working agent and a message
    /// no turn has adopted answers `Session` for a settle, so dropping that
    /// answer would report nothing at all and let a snooze hide the queued turn.
    /// The blocker is skipped where it is *asked*, not where it is answered.
    #[test]
    fn a_queued_turn_behind_a_live_session_still_blocks_a_snooze() {
        let window = Adoption::around(NOW_MILLIS);
        let both = Thread {
            session: Some(running("turn-1")),
            ..with_a_queued_message(-1_000)
        };

        assert_eq!(both.busy(&window, Attention::Settling), Some(Busy::Session));
        assert_eq!(both.busy(&window, Attention::Snoozing), Some(Busy::QueuedTurn));
    }

    /// What blocks a snooze is what would be *hidden* by one: a request the agent
    /// is waiting on the developer to answer, and a turn about to start.
    ///
    /// `canSnooze` in `client-runtime/src/state/threadSettled.ts`, which is the
    /// list this mirrors and the reason the two must agree exactly — the client
    /// refuses these before a round trip and this refuses them authoritatively.
    #[test]
    fn a_snooze_is_refused_by_everything_that_is_waiting_on_the_developer() {
        let window = Adoption::around(NOW_MILLIS);

        let waiting = Thread {
            activities: vec![asked_for_permission("req-1")],
            session: Some(running("turn-1")),
            ..a_thread("thread-1")
        };
        assert_eq!(waiting.busy(&window, Attention::Snoozing), Some(Busy::Approval));

        assert_eq!(
            with_a_queued_message(-1_000).busy(&window, Attention::Snoozing),
            Some(Busy::QueuedTurn)
        );
        assert_eq!(
            a_thread("thread-1").busy(&window, Attention::Snoozing),
            None,
            "a quiet conversation is snoozable"
        );
    }

    /// A session that has *finished* — however it finished — is not work in
    /// progress. The four statuses the client lets through, kept honest here
    /// because `starting` and `running` are named rather than everything else
    /// being excluded.
    #[test]
    fn a_session_that_is_over_does_not_hold_a_conversation_open() {
        let window = Adoption::around(NOW_MILLIS);
        for status in [
            SessionStatus::Idle,
            SessionStatus::Ready,
            SessionStatus::Interrupted,
            SessionStatus::Stopped,
            SessionStatus::Error,
        ] {
            let finished = Thread {
                session: Some(Session {
                    status,
                    active_turn_id: None,
                    ..running("turn-1")
                }),
                ..a_thread("thread-1")
            };
            assert_eq!(finished.busy(&window, Attention::Settling), None, "{}", status.as_str());
        }
    }

    /// The adoption grace, from both sides. A message older than the window is a
    /// start that failed rather than one about to happen — without the bound such
    /// a conversation could never be settled at all — and one *newer* than the
    /// window came from a device whose clock is ahead, which must not hold the
    /// conversation open for the whole of the skew either.
    #[test]
    fn a_queued_turn_stops_being_queued_once_the_adoption_grace_has_passed() {
        let window = Adoption::around(NOW_MILLIS);
        assert_eq!(
            with_a_queued_message(-(ADOPTION_GRACE_MILLIS as i64) - 1_000).busy(&window, Attention::Settling),
            None,
            "a message this old is a start that never happened"
        );
        assert_eq!(
            with_a_queued_message(ADOPTION_GRACE_MILLIS as i64 + 1_000).busy(&window, Attention::Settling),
            None,
            "a message from a clock this far ahead is not pending work"
        );
    }

    /// A turn that *has* been adopted clears the queued state by itself, because
    /// adoption stamps the new turn at or after the message it picked up. This is
    /// the ordinary path — `Shell::start_turn` writes the message and then the
    /// turn — and it is why the guard almost never fires in practice.
    #[test]
    fn a_message_a_turn_has_picked_up_is_not_queued() {
        let window = Adoption::around(NOW_MILLIS);
        let adopted = Thread {
            latest_turn: Some(LatestTurn {
                turn_id: "turn-1".to_string(),
                state: TurnState::Running,
                requested_at: at(-500),
                started_at: None,
                completed_at: None,
                assistant_message_id: None,
            }),
            ..with_a_queued_message(-1_000)
        };
        assert_eq!(adopted.busy(&window, Attention::Settling), None);
    }

    /// A failed start is not pending work. The failure is already in front of the
    /// developer — a status edge and an error — so holding the conversation open
    /// for it would refuse the settle that is the reasonable response to it.
    #[test]
    fn a_message_whose_session_failed_is_not_queued() {
        let failed = Thread {
            session: Some(Session {
                status: SessionStatus::Error,
                active_turn_id: None,
                ..running("turn-1")
            }),
            ..with_a_queued_message(-1_000)
        };
        assert_eq!(failed.busy(&Adoption::around(NOW_MILLIS), Attention::Settling), None);
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
