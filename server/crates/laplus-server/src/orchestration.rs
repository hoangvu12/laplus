//! The orchestration shell: the project registry as the UI actually reaches it.
//!
//! There is a trap in the contract worth naming up front. `WS_METHODS` in
//! `t3code/packages/contracts/src/rpc.ts` declares `projects.list`,
//! `projects.add` and `projects.remove` under a comment reading
//! `// Project registry methods`. **They are dead strings.** No `Rpc.make`
//! defines them, the RPC group does not register them, and nothing in the
//! upstream server or UI sends or answers one. Implementing them would produce
//! a server no client ever calls.
//!
//! The registry the UI really uses is this one, and
//! `fixtures/socket-wire/05-orchestration-and-backpressure.ndjson` captures the
//! whole of it:
//!
//! ```text
//! C>S Request  orchestration.subscribeShell  {"requestCompletionMarker":true}
//! S>C Chunk    {"kind":"snapshot","snapshot":{"snapshotSequence":4,"projects":[…],"threads":[…]}}
//! S>C Chunk    {"kind":"synchronized"}
//! C>S Request  orchestration.dispatchCommand {"type":"project.create",…}
//! S>C Exit     Success {"sequence":5}
//! S>C Chunk    {"kind":"project-upserted","sequence":5,"project":{…}}
//! C>S Request  orchestration.dispatchCommand {"type":"project.delete",…}
//! S>C Exit     Success {"sequence":6}
//! S>C Chunk    {"kind":"project-removed","sequence":6,"projectId":"…"}
//! ```
//!
//! So "add a project" is a command, "the project list" is a subscription, and
//! the two are joined by a **sequence**: a command answers with the log
//! position it committed at, and the event describing it carries the same
//! number. The client orders and de-duplicates by that number, which is why
//! [`crate::store`] persists it rather than counting from zero at each boot.
//!
//! This module is the wire half only. [`crate::projects`] says what a project
//! is and which folders qualify, [`crate::store`] keeps them; nothing here
//! knows any SQL and nothing there knows any JSON.
//!
//! Ticket 10 added the second aggregate. A thread is dispatched to and
//! subscribed to through exactly the same two mechanisms, so what lives here is
//! the routing and the parsing; what a thread *is* lives in [`crate::threads`],
//! which stands to this module as [`crate::projects`] does.
//!
//! ## What this ticket does not do
//!
//! - **`afterSequence` is answered at its two ends and not in between.** This is
//!   the cursor the two *subscriptions* carry, and it is all that is left of
//!   resumption here: `orchestration.replayEvents` was the method that asked for
//!   a log outright, and it left the contract rather than gaining one — for the
//!   same reason this bullet gives. The contract lets a client with a cached
//!   snapshot ask for a replay from a sequence. A cursor that is still
//!   [`Sequences::current`] is a replay of no events, and that is answered
//!   exactly: the opening carries no snapshot, and for the real client — which
//!   asks for no completion marker — no chunk at all. Any other cursor is
//!   answered with the whole snapshot, because replaying from a position needs a
//!   log of events and this server keeps none. See ADR-0016 for why it keeps
//!   none and why the two ends are enough.
//! - **`commandId` is not remembered.** Upstream uses it to recognise a command
//!   it has already run. laplus keeps no log of ids, so a re-dispatched
//!   `project.create` is *refused* ("already exists") rather than answered with
//!   the sequence the first one committed at. That is not idempotence and the
//!   difference is visible to a client that retries; what it is, is safe —
//!   neither command can be applied twice, which is the property the registry
//!   needs. Making a retry answer identically is work for the ticket that has a
//!   client which retries.
//!
//! ## The declared divergence: a missing folder is refused, not created
//!
//! `project.create` carries `createWorkspaceRootIfMissing`, and the upstream UI
//! sends it as `true` on every add
//! (`t3code/packages/client-runtime/src/operations/projects.ts`,
//! `buildProjectCreateCommand`). The reference server obeys, so upstream turns
//! a mistyped path into a new empty directory and reports success.
//!
//! **laplus ignores the flag and refuses a path that is not there**, naming
//! it in the message. Two reasons, and the first is the ticket's:
//!
//! 1. Ticket 05 asks for exactly this — "adding a path that does not exist, is
//!    not a directory, or is not readable fails with a message naming the
//!    problem". A typo that silently creates a directory is not a diagnostic.
//! 2. v1's socket authentication is permissive by design — loopback is the
//!    boundary and no credential is verified. Honouring the flag would mean any
//!    local process that can open the socket can make the server create
//!    directories at paths of its choosing. Refusing costs a user one clear
//!    error message; obeying spends a capability the server has no reason to
//!    hand out.
//!
//! The user-visible consequence is bounded: the folder picker (ticket 06) only
//! offers folders that exist, so this is reachable by typing a path by hand,
//! and the answer to it is a sentence saying which path was not found.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::clock::now_iso;
use crate::config::ServerConfig;
use crate::filesystem::Index;
use crate::projects::{Project, WorkspaceRoot};
use crate::store::{
    Conflict, Database, Insert, Registry, Removal, Rename, Sequences, StorageError,
};
use crate::subscriptions::{EventSource, BACKLOG};
use crate::settling::SessionStatus;
use crate::text_generation::{Operation as TextOperation, ResultText, Service as TextGeneration};
use crate::threads::{
    self, Adoption, Attention, Busy, Change, Given, MetaUpdate, Session, Shelf, Thread,
    TitleRegeneration,
    Threads,
};
use crate::transcripts::Transcripts;

/// The tag that carries every write to the registry.
pub const DISPATCH_COMMAND: &str = "orchestration.dispatchCommand";

/// The subscription that *is* the project list.
pub const SUBSCRIBE_SHELL: &str = "orchestration.subscribeShell";

/// The other half of the project list: what the developer archived.
///
/// A call rather than a subscription, because it is read by a settings panel a
/// developer opens rather than by the sidebar they live in — see
/// [`Shell::archived_shell_snapshot`].
pub const GET_ARCHIVED_SHELL_SNAPSHOT: &str = "orchestration.getArchivedShellSnapshot";

/// The contract's default when a client sends no runtime mode
/// (`DEFAULT_RUNTIME_MODE` in `orchestration.ts`). Repeated here rather than
/// inferred, because it decides how much latitude the agent is given.
pub(crate) const DEFAULT_RUNTIME_MODE: &str = "full-access";

/// The contract's `DEFAULT_PROVIDER_INTERACTION_MODE`.
pub(crate) const DEFAULT_PROVIDER_INTERACTION_MODE: &str = "default";

/// Every runtime mode the contract names (`RuntimeMode` in `orchestration.ts`),
/// in its order.
///
/// Written down here rather than inferred from [`crate::agent::permission_mode_for`],
/// which is a different question with a colliding shape: that table maps a mode
/// to the CLI's `--permission-mode` and answers `None` for `approval-required`
/// *and* for a mode nobody named, because upstream expresses the first by passing
/// no flag. A validator built on it would accept anything.
pub(crate) const RUNTIME_MODES: [&str; 4] = [
    "approval-required",
    "auto-accept-edits",
    "auto",
    "full-access",
];

/// Every interaction mode the contract names (`ProviderInteractionMode`).
///
/// Nothing in this server reads one — it is carried on the thread, published,
/// and never reaches the CLI — so this list is the whole of what the value is
/// checked against, and being a closed set is the only thing that keeps it from
/// becoming a free-text field the picker cannot render.
pub(crate) const INTERACTION_MODES: [&str; 2] = ["default", "plan"];

/// Every reason a client may give for unsettling a conversation, which is one.
///
/// The *event* carries two — `user` and `activity` — and the command carries only
/// the first, because the neutral reset belongs to the server. A one-element
/// closed set rather than an equality check, so it is refused by the same helper
/// and with the same sentence shape as a mode the contract does not name, and so
/// that a second reason is a line here rather than a new rule.
const UNSETTLE_REASONS: [&str; 1] = [threads::BY_THE_USER];

/// The same one reason for `thread.unsnooze`, and its own constant rather than
/// [`UNSETTLE_REASONS`] read twice.
///
/// The two are equal today and are not the same rule: they are two declarations
/// in the contract about two commands, and sharing one array here would mean a
/// reason added to either widened both. The sentence names which command it was
/// refused for, which is the diagnostic a developer with a stale client needs.
const UNSNOOZE_REASONS: [&str; 1] = [threads::BY_THE_USER];

/// The registry, live: what is in it, what changes it, and who is watching.
///
/// Cheap to clone, and every clone is the same shell — a subscription outlives
/// the call that opened it and has to be able to describe the world again long
/// afterwards. The same reasoning as [`crate::config_store::ConfigStore`],
/// which this deliberately mirrors.
#[derive(Debug, Clone)]
pub struct Shell {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// Behind an [`Arc`] because the transcript writer holds it too — see
    /// [`crate::transcripts`], where the whole point is that a conversation is
    /// written down on a thread of its own rather than on whoever changed it.
    database: Arc<Database>,
    updates: broadcast::Sender<Value>,
    /// Fires when the codes this machine has handed out change.
    ///
    /// Separate from `updates` rather than folded into it, because the two
    /// carry different vocabularies to different subscriptions: `updates` is
    /// the shell's projects and conversations, ordered by [`Sequences`], and a
    /// client folds it against one cursor. An access snapshot is a wholesale
    /// replacement with a `revision` of its own and no ordering relationship to
    /// a project being renamed. Sharing the channel would put each aggregate's
    /// events in the other's stream for both to filter back out.
    access_updates: broadcast::Sender<Value>,
    /// The number every change on this wire is ordered by, shared with
    /// [`Threads`] because both aggregates travel on the same subscription and
    /// a client folds them against one cursor.
    sequences: Sequences,
    threads: Threads,
    transcripts: Transcripts,
    mcp: Arc<dyn crate::mcp::Platform>,
}

/// A command this server understands, once its payload has been read.
///
/// Parsing to this is where a malformed or unimplemented command is turned
/// away, so by the time [`Shell::dispatch`] has one it is only the *world* that
/// can still refuse it.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    CreateProject(CreateProject),
    RenameProject {
        project_id: String,
        title: String,
    },
    DeleteProject {
        project_id: String,
    },
    CreateThread(CreateThread),
    UpdateThreadMeta(UpdateThreadMetaPayload),
    Archive {
        thread_id: String,
    },
    Unarchive {
        thread_id: String,
    },
    Pin {
        thread_id: String,
        order_key: Option<String>,
    },
    Unpin {
        thread_id: String,
    },
    ReorderPin {
        thread_id: String,
        order_key: String,
    },
    Settle {
        thread_id: String,
    },
    Unsettle {
        thread_id: String,
    },
    Snooze {
        thread_id: String,
        until: String,
    },
    Unsnooze {
        thread_id: String,
    },
    Delete {
        thread_id: String,
    },
    StartTurn(Box<StartTurn>),
    InterruptTurn(InterruptTurn),
    RespondToApproval(RespondToApproval),
    RespondToUserInput(RespondToUserInput),
    SetRuntimeMode(SetRuntimeModePayload),
    SetInteractionMode(SetInteractionModePayload),
    RevertCheckpoint(RevertCheckpointPayload),
    StopSession {
        thread_id: String,
    },
}

/// Everything a diff of a conversation needs that is not about git.
///
/// See [`Shell::reviewing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewing {
    /// The folder the agent was working in, and therefore the repository the
    /// checkpoints were written into.
    pub workspace_root: String,
    /// The highest turn count this conversation has a checkpoint for. Zero when
    /// it has none, which is also the baseline's own count — so "nothing has
    /// been recorded" and "only the baseline has" read the same, and both mean
    /// there is no turn to diff.
    pub checkpoints: u64,
}

/// Where a conversation's work happens: its worktree when it has one, the
/// project's folder otherwise.
///
/// **One rule, stated once, read by both sides.** A conversation is pointed at a
/// worktree by picking a ref that is current in one — no method here makes a
/// worktree, and none has to: one made by hand at a terminal is enough to reach
/// this. Two paths then have to agree about which folder that is, because they
/// are two views of one turn: the turn path, which is both the folder the agent
/// is started in and the folder [`crate::checkpoints`] photographs at each turn
/// boundary ([`Shell::start_turn`], through [`crate::session::Start`]); and the
/// review path, which is the folder the diff and the revert are run in
/// ([`Shell::reviewing`], read by [`Shell::revert_checkpoint`] and by
/// [`crate::checkpoints::Diff`]).
///
/// They did not agree. The review path resolved the worktree; the turn path used
/// the project's folder unconditionally. **What that cost, precisely**, because
/// it is easy to get wrong in both directions:
///
/// - **The diff panel was right by accident.** A checkpoint is a ref, and a
///   patch is `git diff` between two of them — it never reads a tree. Refs are
///   shared with a linked worktree, so a patch run in the worktree resolved
///   checkpoints captured from the project's folder and showed the agent's own
///   changes after all.
/// - **A revert was not.** [`crate::checkpoints::restore`] does write a tree, and
///   it wrote the tree recorded from the project's folder into the *worktree* —
///   over a checkout the agent had never touched, and quite possibly over work
///   the developer had. That is the damage, and nothing in the UI said so.
///
/// Writing the rule in one path and describing it in a comment on the other is
/// what let them drift, so it is a function rather than a comment now.
///
/// The client already resolves it this way for the terminal it opens
/// (`packages/shared/src/projectScripts.ts`, `projectScriptCwd`), which is why a
/// terminal lands beside the agent rather than needing a rule of its own here.
fn where_the_work_happens(thread: &Thread, project: &Project) -> String {
    thread
        .worktree_path
        .clone()
        .unwrap_or_else(|| project.workspace_root.clone())
}

/// A command that was not carried out, as the client will read it.
///
/// The contract's `OrchestrationDispatchCommandError` carries a message and
/// nothing else machine-readable — no failure code, no field name. So the
/// sentence *is* the diagnostic, and the upstream UI renders it verbatim under
/// "Failed to add project". Every message built here therefore names both what
/// went wrong and the thing it went wrong about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    message: String,
}

impl CommandError {
    fn new(message: impl Into<String>) -> CommandError {
        CommandError {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// The typed error to put in an `Exit`/`Failure`'s `Fail` cause. It is one
    /// of the two errors `orchestration.dispatchCommand` declares, so the
    /// client decodes it into a failed call rather than a broken connection.
    pub fn to_error(&self) -> Value {
        json!({
            "_tag": "OrchestrationDispatchCommandError",
            "message": self.message,
        })
    }
}

impl Shell {
    /// Open the registry and put back what the last run left in it.
    ///
    /// The conversations are read here rather than lazily, and that is a
    /// decision: a thread's *summary* is what the project list opens with, so a
    /// server that had not read them yet would have to answer its first
    /// `subscribeShell` with a claim that the user has no conversations. One pass
    /// over three tables is the cost, and it is the size of the history rather
    /// than a query per conversation — see [`Database::conversations`].
    pub fn new(database: Database) -> Shell {
        Self::new_with_mcp(database, Arc::new(crate::mcp::Host::new()))
    }

    pub fn new_with_mcp(database: Database, mcp: Arc<dyn crate::mcp::Platform>) -> Shell {
        let sequences = Sequences::resuming(&database).unwrap_or_else(|error| {
            // Unreachable in practice: everything else this server does with the
            // registry would already have failed. Loud rather than silent,
            // because carrying on from zero is the one thing that could re-issue
            // a number a committed change has already used.
            eprintln!("laplus: cannot read the registry's log position, resuming from zero: {error}");
            Sequences::from(0)
        });
        let stored = database.conversations().unwrap_or_else(|error| {
            // Same shape and the same reasoning as above, and the same
            // unreachability: this read is a `SELECT` over tables the migration
            // just created. Carrying on means the conversations are not shown,
            // and they are still on disk to be shown by the next run.
            eprintln!("laplus: cannot read the stored conversations: {error}");
            Vec::new()
        });

        let database = Arc::new(database);
        let updates = broadcast::channel(BACKLOG).0;
        let transcripts = Transcripts::writing_to(Arc::clone(&database));
        let threads = Threads::new(sequences.clone(), updates.clone(), transcripts.clone());
        threads.restore(stored);

        Shell {
            inner: Arc::new(Inner {
                database,
                threads,
                sequences,
                updates,
                access_updates: broadcast::channel(BACKLOG).0,
                transcripts,
                mcp,
            }),
        }
    }

    /// `subscribeAuthAccess` — the codes this machine has handed out, and who
    /// is holding one.
    ///
    /// **Snapshots only.** The contract's `AuthAccessStreamEvent` is a union of
    /// five, four of them deltas, and the Settings panel that is the only
    /// reader keeps just one: `if (event?.type !== "snapshot") return []`
    /// (`ConnectionsSettings.tsx:1602`). Emitting deltas would be writing four
    /// event shapes nothing decodes, so a change republishes the whole list —
    /// which is what the client folds anyway, and what every other subscription
    /// on this wire does.
    ///
    /// `clientSessions` is always empty, and that is ticket 73's decision
    /// rather than an omission here: it puts `auth.clients` out of scope and
    /// says to leave the UI refusing. A pairing link is what has to be visible,
    /// because a code the user cannot read off the screen is a code they cannot
    /// carry to their phone.
    pub fn subscribe_auth_access(&self) -> EventSource {
        // Subscribed before the description is handed over, for the reason
        // `ConfigStore::subscribe` gives: a change landing in between arrives
        // as an update rather than falling into the gap, and being seen twice
        // costs nothing when both are whole replacements.
        let updates = self.inner.access_updates.subscribe();
        let database = Arc::clone(&self.inner.database);
        EventSource::new(move || vec![access_snapshot_event(&database)], updates)
    }

    /// Say that the codes changed, so every open Settings panel re-reads them.
    ///
    /// Called by the two routes that change them. A failure to read the list
    /// back is not this function's to report — [`access_snapshot_event`] answers
    /// with an empty list and complains to the log, which is the same posture
    /// the route itself takes.
    pub fn auth_access_changed(&self) {
        // `send` errors only when nothing is subscribed, which is the ordinary
        // case: nobody has Settings open.
        let _ = self
            .inner
            .access_updates
            .send(access_snapshot_event(&self.inner.database));
    }

    /// The conversations this shell carries. One registry per server, shared the
    /// way the shell itself is.
    pub fn threads(&self) -> &Threads {
        &self.inner.threads
    }

    /// The database, for the one caller that wants it and is not a
    /// conversation: ticket 73's pairing routes.
    ///
    /// Pairing is not the shell's business and this accessor is not pretending
    /// it is. It is here because the shell is what *holds* the database — one
    /// per server, opened at bind — and threading a second handle through
    /// [`crate::server::Services`] would mean two owners of one connection for
    /// the sake of a tidier name.
    pub fn database(&self) -> &Database {
        &self.inner.database
    }

    /// Where a conversation's checkpoints are, and how far they go.
    ///
    /// The one thing [`crate::checkpoints`] cannot work out for itself: a diff
    /// is asked for by thread and has to be run in a *folder*, and the folder is
    /// the project's, which only the registry knows. Both halves are read here
    /// so the answer cannot be assembled from two moments — a thread whose
    /// project was deleted between them would be a diff run somewhere else.
    ///
    /// Answers from memory. The conversations are already open and the projects
    /// are a single row, so this is cheap enough to be called from the deferred
    /// work rather than needing a moment of its own.
    pub fn reviewing(&self, thread_id: &str) -> Result<Reviewing, CommandError> {
        let thread = self.open_thread(thread_id)?;
        let project = self.project(&thread.project_id)?;
        Ok(Reviewing {
            // [`where_the_work_happens`], the same call [`Shell::start_turn`]
            // makes — and it has to be, because the tree a checkpoint recorded
            // is the tree the agent was working in.
            workspace_root: where_the_work_happens(&thread, &project),
            // Asked of the registry rather than folded out of the copy above,
            // so that "how far has this conversation been recorded" has one
            // answer — the same one the driver uses to decide what the next
            // checkpoint is called.
            checkpoints: self.inner.threads.checkpoint_count(thread_id),
        })
    }

    /// Wait until every change made so far is on disk.
    ///
    /// What shutdown calls, after the agents have been reaped so that whatever
    /// they published on their way down is in the queue this waits for. Nothing
    /// else calls it: the queue is drained continuously and a caller that waited
    /// for the disk mid-conversation would be the stutter
    /// [`crate::transcripts`] exists to avoid.
    pub async fn flush(&self) {
        self.inner.transcripts.flush().await;
    }

    /// Carry out one `orchestration.dispatchCommand`, answering with the
    /// sequence it committed at.
    ///
    /// The index arrives as an argument rather than as a field because it is
    /// wanted by exactly one command — a deleted project releases the scan and
    /// the filesystem watcher held for it (ticket 08). Making it a field would
    /// say the registry depends on the index, which is not true of anything else
    /// it does. `config` is here for the same reason and for one command only:
    /// starting a turn needs to know which binary to look for.
    ///
    /// **One command, several events.** A project command commits once and
    /// answers with that number. A turn does not: it puts the developer's
    /// message in the transcript, records that a turn was asked for, and marks
    /// the session as starting — three events, three numbers. The answer is the
    /// last of them, which is the position the log reached, and every one of
    /// them has already been published by the time the client reads it.
    ///
    /// **A conversation the developer deleted takes no further commands**, and
    /// that is decided here rather than command by command: a stale window — one
    /// that has not folded the `thread.deleted`, or a second one that never
    /// heard it — must not go on driving a conversation the developer removed,
    /// and nineteen separate guards would be nineteen places to forget it. See
    /// [`Command::over_a_living_thread`], which is where the one command that is
    /// deliberately not covered is argued.
    pub fn dispatch(
        &self,
        payload: &Value,
        index: &Index,
        config: &ServerConfig,
    ) -> Result<Value, CommandError> {
        let command = Command::parse(payload)?;
        if let Some(thread_id) = command.over_a_living_thread() {
            if self.inner.threads.deleted(thread_id) {
                return Err(CommandError::new(format!(
                    "Conversation '{thread_id}' was deleted, so it takes no further commands."
                )));
            }
        }

        let sequence = match command {
            Command::CreateProject(create) => self.create_project(&create)?,
            Command::RenameProject { project_id, title } => {
                self.rename_project(&project_id, &title)?
            }
            Command::DeleteProject { project_id } => self.delete_project(&project_id, index)?,
            Command::CreateThread(create) => self.create_thread(&create, config)?,
            Command::UpdateThreadMeta(update) => self.update_thread_meta(update, config)?,
            Command::Archive { thread_id } => self.set_archived(&thread_id, Shelf::Archived)?,
            Command::Unarchive { thread_id } => self.set_archived(&thread_id, Shelf::Working)?,
            Command::Pin {
                thread_id,
                order_key,
            } => self.pin(&thread_id, order_key)?,
            Command::Unpin { thread_id } => self.unpin(&thread_id)?,
            Command::ReorderPin {
                thread_id,
                order_key,
            } => self.reorder_pin(&thread_id, order_key)?,
            Command::Settle { thread_id } => self.settle(&thread_id)?,
            Command::Unsettle { thread_id } => self.unsettle(&thread_id)?,
            Command::Snooze { thread_id, until } => self.snooze(&thread_id, until)?,
            Command::Unsnooze { thread_id } => self.unsnooze(&thread_id)?,
            Command::Delete { thread_id } => self.delete(&thread_id)?,
            Command::StartTurn(start) => self.start_turn(&start, config)?,
            Command::InterruptTurn(interrupt) => self.interrupt_turn(&interrupt)?,
            Command::RespondToApproval(respond) => self.respond_to_approval(&respond)?,
            Command::RespondToUserInput(respond) => self.respond_to_user_input(&respond)?,
            Command::SetRuntimeMode(set) => self.set_mode(
                &set.thread_id,
                Change::RuntimeModeSet {
                    runtime_mode: set.runtime_mode,
                },
            )?,
            Command::SetInteractionMode(set) => self.set_mode(
                &set.thread_id,
                Change::InteractionModeSet {
                    interaction_mode: set.interaction_mode,
                },
            )?,
            Command::RevertCheckpoint(revert) => self.revert_checkpoint(revert, index, config)?,
            Command::StopSession { thread_id } => self.stop_session(&thread_id)?,
        };

        Ok(json!({ "sequence": sequence }))
    }

    /// Move one of the conversation's two modes, for the turns after this one.
    ///
    /// **The mode applies to the next turn, not the one already running.** That
    /// falls out of where each value is read rather than being enforced here:
    /// this writes the *thread*, which is what the composer's pickers read
    /// (`ChatView.tsx`'s `activeThread?.runtimeMode`), and touches neither the
    /// session nor the agent. So the turn in flight stays under the rules it
    /// started with, which is the point: the rules an agent is working under must
    /// not move under its feet.
    ///
    /// **What carries it to the child is the next turn's dispatch.**
    /// [`crate::session::send`] reads the thread and pushes what has moved on the
    /// agent's control channel before the prompt is written, so a mode changed
    /// here reaches the process already serving the conversation without
    /// replacing it — ticket 11 of `.scratch/thread-lifecycle/`, which also
    /// closed the same hole in the per-turn override and in the model. Nothing
    /// about that belongs in this function, and that is the division: this says
    /// what the conversation wants, and the dispatch says it to the agent.
    ///
    /// **A repeat is answered rather than refused.** Both commands are a write
    /// of one field, and folding the event a second time lands on the same
    /// state, so a double-click or a stale client costs a sequence and nothing
    /// else. The value being new is not something either side promised.
    ///
    /// It is *not* the spec's "idempotence by re-emission", which carries the
    /// existing `updatedAt` so a duplicate cannot churn a list ordered by when
    /// things changed: [`Threads::apply`] stamps the clock for every change, and
    /// that rule was written for settle and snooze. The cost here is a repeat
    /// moving the thread's `updatedAt`, which the client's own list does not sort
    /// on — it prefers `latestUserMessageAt` (`threadSort.ts`). Worth knowing
    /// before the settle commands reuse this path, where it would matter.
    ///
    /// An unknown thread is refused by [`Threads::apply`]'s own `None` rather
    /// than by a lookup before it. Deliberately not the shape
    /// [`Shell::interrupt_turn`] uses: [`Shell::open_thread`] *clones the whole
    /// conversation* — every message and every activity — and a pre-check would
    /// copy a long transcript to ask a question the write answers anyway, with
    /// the same sentence.
    fn set_mode(&self, thread_id: &str, change: Change) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply(thread_id, change)
            .ok_or_else(|| self.not_open(thread_id))
    }

    /// Put the developer's working tree back to how it looked before a turn ran.
    ///
    /// **Answered in two stages**, which is the shape the contract declares and
    /// the reason it declares two events for one command. This records that a
    /// revert was asked for and answers with the sequence it committed at; the
    /// restore itself runs on a blocking thread, because it is a `git` over a
    /// repository whose size is the developer's and the socket's only reader must
    /// never wait on a disk. Completion is [`Change::Reverted`], published when
    /// the tree has actually been written.
    ///
    /// Everything the world can refuse is decided *before* the answer, from
    /// memory:
    ///
    /// - an unknown conversation, by [`Shell::reviewing`], with
    ///   [`Shell::not_open`]'s sentence;
    /// - a turn this conversation has no photograph of, which is a revert with
    ///   nothing behind it. Refused rather than attempted, for the reason a
    ///   checkpoint row is never published before its tree has been written: an
    ///   undo offered over nothing is worse than no undo.
    ///
    /// The count is compared against [`Reviewing::checkpoints`] rather than
    /// against the ref on disk, which is the same authority
    /// [`crate::checkpoints::Diff::run`] asks and is the point of asking it here:
    /// it is a question about the registry, and answering it needs no git. A
    /// conversation with no recorded turns has no baseline either — turn zero is
    /// the tree *before the first turn*, and until one has finished nothing has
    /// promised to have written it — so it is refused with the rest.
    ///
    /// **Turn zero is otherwise an ordinary target**, and is the common one: the
    /// panel reverts a user message by asking for `max(0, n - 1)`
    /// (`ChatView.tsx`), so undoing the first turn of a conversation names the
    /// baseline. A range check that started at one would refuse exactly the
    /// revert the control is most often used for.
    ///
    /// Whether the project folder is still there is *not* decided here. It is a
    /// disk, this is the read loop, and a folder that has been moved is reported
    /// the same way a `git` that refused is: as a failure on the conversation
    /// rather than as a refusal of a command that had already been accepted.
    fn revert_checkpoint(
        &self,
        revert: RevertCheckpointPayload,
        index: &Index,
        config: &ServerConfig,
    ) -> Result<i64, CommandError> {
        let RevertCheckpointPayload {
            thread_id,
            turn_count,
        } = revert;
        let reviewing = self.reviewing(&thread_id)?;
        let thread = self.open_thread(&thread_id)?;
        if reviewing.checkpoints == 0 || turn_count > reviewing.checkpoints {
            return Err(CommandError::new(format!(
                "The state of this project at turn {turn_count} was not recorded, so there is \
                 nothing to put it back to. This conversation has {} recorded turn{}.",
                reviewing.checkpoints,
                match reviewing.checkpoints {
                    1 => "",
                    _ => "s",
                }
            )));
        }

        let reference = crate::checkpoints::reference(&thread_id, turn_count);
        let sequence = self
            .inner
            .threads
            .apply(&thread_id, Change::RevertRequested { turn_count })
            .ok_or_else(|| self.not_open(&thread_id))?;

        // Off the read loop, like every other git in this server. The registry is
        // cloned into the task rather than borrowed: the connection that asked may
        // be gone by the time the tree has been written, and the conversation is
        // owed the answer either way.
        let threads = self.inner.threads.clone();
        let workspace_root = reviewing.workspace_root;
        let latest = reviewing.checkpoints;
        let index = index.clone();
        let settings = config.settings.clone();
        let mcp = Arc::clone(&self.inner.mcp);
        tokio::spawn(async move {
            let checked = WorkspaceRoot::check(&workspace_root)
                .map_err(|rejection| rejection.message());
            let root = match checked {
                Ok(root) => root,
                Err(why) => {
                    publish_revert_failure(&threads, &thread_id, turn_count, &why, false);
                    return;
                }
            };
            let root_path = root.path().to_path_buf();
            let restored = tokio::task::spawn_blocking({
                let root_path = root_path.clone();
                move || crate::checkpoints::restore(&root_path, &reference).map_err(|why| why.detail())
            })
            .await
            .map_err(|why| format!("the filesystem restore task stopped: {why}"))
            .and_then(|restored| restored);

            if let Err(why) = restored {
                publish_revert_failure(&threads, &thread_id, turn_count, &why, false);
                return;
            }

            // The restore bypasses the watcher on some platforms. Refresh now,
            // before the provider is touched, so the file picker already
            // describes the restored tree even when rollback fails afterwards.
            let refreshed = tokio::task::spawn_blocking({
                let index = index.clone();
                let workspace_root = workspace_root.clone();
                move || {
                    crate::filesystem::ListEntries::read(&json!({"cwd": workspace_root}))
                        .and_then(|call| call.run(&index))
                        .map(|_| ())
                        .map_err(|error| {
                            error.get("message").and_then(Value::as_str)
                                .unwrap_or("the workspace index could not be refreshed")
                                .to_string()
                        })
                }
            })
            .await
            .map_err(|why| format!("the workspace refresh task stopped: {why}"))
            .and_then(|refreshed| refreshed);
            if let Err(why) = refreshed {
                publish_revert_failure(&threads, &thread_id, turn_count, &why, true);
                return;
            }

            // Provider history is an OpenCode capability. Preserve the shared
            // filesystem-only behavior for the other drivers.
            if thread.provider.driver != crate::provider::OPENCODE_DRIVER {
                threads.apply(&thread_id, Change::Reverted { turn_count });
                return;
            }

            let prepared = match crate::session::prepare(&thread, &settings, mcp) {
                Ok(prepared) => prepared,
                Err(why) => {
                    publish_revert_failure(&threads, &thread_id, turn_count, &why, true);
                    return;
                }
            };
            let starting = crate::session::starting(&thread, &workspace_root, prepared);
            if let Err(why) = crate::opencode::rollback(&starting, latest - turn_count).await {
                publish_revert_failure(&threads, &thread_id, turn_count, &why, true);
                return;
            }

            let pruning_thread_id = thread_id.clone();
            let pruned = tokio::task::spawn_blocking(move || {
                crate::checkpoints::prune_after(&root_path, &pruning_thread_id, turn_count, latest)
                    .map_err(|why| why.detail())
            })
            .await
            .map_err(|why| format!("the checkpoint cleanup task stopped: {why}"))
            .and_then(|pruned| pruned);

            // **A failure is never published as a completion**, which is the
            // criterion this arm exists for: a client that folded
            // `thread.reverted` would show the developer a conversation put back
            // to a turn while their files still held the one they were undoing.
            // Said in the conversation rather than only to a log, the way a
            // checkpoint that could not be taken is — the developer is looking at
            // the conversation, and this is the only place left to tell them.
            match pruned {
                Ok(()) => {
                    threads.apply(&thread_id, Change::Reverted { turn_count });
                }
                Err(why) => {
                    threads.apply(
                        &thread_id,
                        Change::Activity(crate::threads::Activity::failed(
                            "revert.failed",
                            &format!(
                                "The project and OpenCode history were put back to turn \
                                 {turn_count}, but later checkpoint references could not all be \
                                 removed, so completion was not published: {why}"
                            ),
                        )),
                    );
                }
            }
        });

        Ok(sequence)
    }

    /// Rename a conversation — and, on the same command, move the three other
    /// things the client keeps beside its title.
    ///
    /// **This is not only the rename control**, and that is the thing about
    /// `thread.meta.update` worth knowing before touching it. The composer sends
    /// it on *every* send whose model or branch differs from the thread's, from
    /// `ChatView.tsx`'s `persistThreadSettingsForNextTurn`, and it sends it
    /// *first* — the two mode commands and the turn itself are behind an
    /// `if (failure === null)`. So while this command was refused by name, picking
    /// a runtime mode and pressing enter dispatched this, was refused, and never
    /// sent the message. Ticket 03 of the thread-lifecycle tracker records the
    /// run that found it.
    ///
    /// A blank title never reaches here — [`Command::parse`] refuses one, because
    /// a conversation called "" is a row the developer cannot pick out of a list.
    /// What does reach here is a rename to the title already held, which is
    /// answered rather than refused for [`Shell::set_mode`]'s reason: folding the
    /// event a second time lands on the same state.
    ///
    /// Such a repeat does move the thread's `updatedAt`, because [`Threads::apply`]
    /// stamps the clock for every change — and it is *not* the spec's "idempotence
    /// by re-emission", which carries the existing timestamp so a duplicate cannot
    /// churn a list ordered by when things changed. That rule was written for
    /// settle and snooze, and [`Shell::set_mode`] carries the whole argument: the
    /// client's own thread list prefers `latestUserMessageAt` (`threadSort.ts`), so
    /// nothing the developer looks at reorders. Worth knowing before the settle
    /// commands reuse this path, where it would matter.
    ///
    /// An unknown thread is refused by [`Threads::apply`]'s own `None`, again as
    /// [`Shell::set_mode`] does it and for the same reason — a pre-check would
    /// clone a whole transcript to ask a question the write answers anyway.
    fn update_thread_meta(
        &self,
        update: UpdateThreadMetaPayload,
        config: &ServerConfig,
    ) -> Result<i64, CommandError> {
        let UpdateThreadMetaPayload {
            thread_id,
            title,
            regenerate_title,
            command_id,
            model_selection,
            branch,
            worktree_path,
        } = update;
        if let Some(selection) = &model_selection {
            let thread = self.open_thread(&thread_id)?;
            selection_for(&thread, selection)?;
        }
        if regenerate_title {
            return self.regenerate_title(&thread_id, &command_id, config);
        }
        let title_regeneration = title.as_ref().map(|_| None);
        self.inner
            .threads
            .apply(
                &thread_id,
                Change::MetaUpdated(MetaUpdate {
                    title,
                    title_regeneration,
                    regenerate_title: false,
                    previous_title: None,
                    model_selection,
                    branch,
                    worktree_path,
                }),
            )
            .ok_or_else(|| self.not_open(&thread_id))
    }

    fn regenerate_title(
        &self,
        thread_id: &str,
        request_id: &str,
        config: &ServerConfig,
    ) -> Result<i64, CommandError> {
        let thread = self.open_thread(thread_id)?;
        let project = self.project(&thread.project_id)?;
        let selection = &config.settings.text_generation_model_selection;
        let instance_id = selection
            .get("instanceId")
            .and_then(Value::as_str)
            .ok_or_else(|| CommandError::new("No text-generation provider is configured."))?;
        let instance = crate::provider::resolve_instance(&config.settings, instance_id, None)
            .map_err(|_| CommandError::new(format!(
                "Text-generation provider '{instance_id}' is unavailable."
            )))?;
        let model = selection.get("model").and_then(Value::as_str).map(str::to_string);
        let context = format!(
            "Current title: {}\n\nConversation:\n{}",
            thread.title,
            thread
                .messages
                .iter()
                .filter(|message| !message.text.trim().is_empty())
                .map(|message| format!("{}: {}", message.role, message.text))
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        let previous_title = thread.title.clone();
        let directory = where_the_work_happens(&thread, &project);
        let request_id = request_id.to_string();
        let sequence = self
            .inner
            .threads
            .apply(
                thread_id,
                Change::MetaUpdated(MetaUpdate {
                    title: None,
                    title_regeneration: Some(Some(TitleRegeneration {
                        request_id: request_id.clone(),
                        started_at: now_iso(),
                    })),
                    regenerate_title: true,
                    previous_title: Some(previous_title.clone()),
                    model_selection: None,
                    branch: None,
                    worktree_path: None,
                }),
            )
            .ok_or_else(|| self.not_open(thread_id))?;

        let shell = self.clone();
        let thread_id = thread_id.to_string();
        tokio::spawn(async move {
            let generated = TextGeneration::new()
                .generate(
                    &instance,
                    &directory,
                    model.as_deref(),
                    TextOperation::ThreadTitle { context },
                )
                .await;
            let (title, failure) = match generated {
                Ok(ResultText::ThreadTitle(title)) if !title.trim().is_empty() => {
                    (Some(title.trim().to_string()), None)
                }
                Ok(_) => (None, Some("The title generator returned a blank title.".to_string())),
                Err(error) => (None, Some(error.to_string())),
            };
            let completion = shell.inner.threads.apply_unless(
                &thread_id,
                |thread| {
                    (thread.title_regeneration.as_ref().map(|pending| pending.request_id.as_str())
                        != Some(request_id.as_str())
                        || thread.title != previous_title)
                        .then(|| "the title generation was superseded".to_string())
                },
                Change::MetaUpdated(MetaUpdate {
                    title,
                    title_regeneration: Some(None),
                    regenerate_title: false,
                    previous_title: None,
                    model_selection: None,
                    branch: None,
                    worktree_path: None,
                }),
            );
            if completion.as_ref().is_some_and(|result| result.is_ok()) {
                if let Some(failure) = failure {
                    let _ = shell.inner.threads.apply(
                        &thread_id,
                        Change::Activity(crate::threads::Activity::failed(
                            "thread.title-regeneration.failed",
                            &format!("Failed to regenerate title: {failure}"),
                        )),
                    );
                }
            }
        });
        Ok(sequence)
    }

    /// Put a finished conversation away, or take it back out.
    ///
    /// The first thing in this server that lets the inbox be *cleared*: the
    /// project list has carried every conversation ever started, so the one that
    /// needs attention has been buried among the ones that do not.
    ///
    /// **Archiving is not deleting.** The thread, its transcript, its work log
    /// and its checkpoints all stay exactly as they were, and the agent — if one
    /// is running — is not told anything. The only thing that changes is which
    /// snapshot names the conversation: the project list stops
    /// ([`crate::threads::Shelf`]), and [`Shell::archived_shell_snapshot`]
    /// starts. That is what makes unarchiving give the whole conversation back
    /// rather than a husk of it.
    ///
    /// **A repeat is refused rather than answered**, which is where these two
    /// part company with [`Shell::set_mode`] and [`Shell::update_thread_meta`].
    /// Those write a field to a value the developer chose, so writing it twice
    /// lands on what they asked for either way. This is a move between two lists,
    /// and a second archive is a click on a control that is no longer there — a
    /// stale window, or a second one that has not caught up. A sentence saying
    /// which list the conversation is already on is more use to the developer
    /// than a sequence for a move that did not happen, and it is what upstream
    /// answers. It is also not the spec's "idempotence by re-emission", which is
    /// explicitly about settle and snooze.
    ///
    /// The refusal is decided under the same lock the change is folded under —
    /// see [`Threads::apply_unless`] — so two windows archiving at once cannot
    /// both be told they did it.
    fn set_archived(&self, thread_id: &str, to: Shelf) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                // The shelf answers whether it already holds this conversation,
                // which is the same question [`Threads::shell_summaries`] asks it
                // — so a move that would move nothing is refused by the predicate
                // that decides which list the conversation is on, rather than by
                // a second reading of the field.
                |thread| {
                    to.holds(thread).then(|| match to {
                        Shelf::Archived => format!(
                            "Conversation '{thread_id}' is already archived, so it was left where \
                             it is."
                        ),
                        Shelf::Working => format!(
                            "Conversation '{thread_id}' is not archived, so there is nothing to \
                             bring back."
                        ),
                    })
                },
                to.arrival(),
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Let a finished conversation leave the inbox.
    ///
    /// **This is not [`crate::settling`]**, which reads a session status as how a
    /// *turn* went, and the collision is the one thing to know before reading
    /// further: the contract spells these fields `settledOverride` and
    /// `settledAt`, and the word means something else three lines away in this
    /// same crate. `docs/adr/0024` and the **Inbox state** entry in `CONTEXT.md`
    /// are where that is settled — the turn meaning has seniority, the field names
    /// belong to the contract and do not move, and it is the prose and the Rust
    /// identifiers that disambiguate.
    ///
    /// **The server does not classify.** Which conversations *count* as settled is
    /// `effectiveSettled` in the bundled client runtime, which ships unmodified
    /// (ADR-0012) and reads these two fields alongside four other things. This
    /// stores the override, enforces the invariants and emits the event; what the
    /// inbox shows is not its decision.
    ///
    /// **The invariants are, though.** The client keeps a twin of them so the
    /// interface can refuse before a round trip, and this is the authoritative
    /// copy — see [`Busy`], which is the same list `effectiveSettled` refuses to
    /// classify with, because a conversation that will not classify as settled
    /// must not be accepted as a settle target either. An archived conversation is
    /// refused on top of those four: it is not in the inbox to leave it, and
    /// [`Shelf::holds`] is asked rather than the field read a second time.
    ///
    /// **A repeat re-emits rather than being refused**, which is where this parts
    /// company with [`Shell::set_archived`] — see [`Change::re_emitted_at`] for
    /// the whole of that argument.
    ///
    /// **And a settle is not permanent.** The invariants above refuse to hide
    /// live work at the moment the developer asks, and
    /// `crate::threads::Threads::woken_by` is what stops that being reachable a
    /// minute later: real activity in the conversation returns it to the inbox by
    /// itself, without this command being sent again.
    fn settle(&self, thread_id: &str) -> Result<i64, CommandError> {
        // Drawn before the lock is taken rather than inside the guard, so the
        // window a queued turn is measured against is one instant for the whole
        // command instead of a clock read per comparison.
        let adoption = Adoption::now();
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    if Shelf::Archived.holds(thread) {
                        return Some(format!(
                            "Conversation '{thread_id}' is archived, so it is already out of the \
                             inbox and there is nothing to settle."
                        ));
                    }
                    thread
                        .busy(&adoption, Attention::Settling)
                        .map(|busy| would_hide(thread_id, busy, Attention::Settling))
                },
                Change::Settled,
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Pin a conversation back into the inbox.
    ///
    /// [`Shell::settle`]'s twin, and deliberately **not its mirror image**. The
    /// reason is `user`, which pins the conversation *active* rather than clearing
    /// the override to neutral, so the client's own auto-settle stays suppressed
    /// until real work moves it on — see [`crate::threads::pinned_by`]. The
    /// neutral reset is the server's own — `crate::threads::Threads::woken_by`,
    /// which real work triggers — and the contract lets a client send only this
    /// reason, so it cannot be forged.
    ///
    /// "Until real work moves it on" is the pin's whole lifetime and it is not
    /// enforced here: the same three triggers that wake a *settled* conversation
    /// return a pinned one to neutral, so nothing has to remember that the
    /// developer pinned it.
    ///
    /// **The invariants are not this command's.** Pinning something back can never
    /// hide work — it is the direction that makes work visible — so the four
    /// blockers do not apply and nothing here asks about a session or the work
    /// log. Archived is refused for [`Shell::settle`]'s reason, which is about the
    /// inbox rather than about attention: there is no inbox to pin an archived
    /// conversation back to.
    fn unsettle(&self, thread_id: &str) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    Shelf::Archived.holds(thread).then(|| {
                        format!(
                            "Conversation '{thread_id}' is archived, so there is no inbox to pin \
                             it back to. Unarchive it first."
                        )
                    })
                },
                Change::Unsettled {
                    reason: threads::BY_THE_USER,
                },
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    fn pin(&self, thread_id: &str, order_key: Option<String>) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| Shelf::Archived.holds(thread).then(|| format!("Conversation '{thread_id}' is archived, so it cannot be pinned.")),
                Change::Pinned { order_key },
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    fn unpin(&self, thread_id: &str) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| Shelf::Archived.holds(thread).then(|| format!("Conversation '{thread_id}' is archived, so it cannot be unpinned.")),
                Change::Unpinned,
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    fn reorder_pin(&self, thread_id: &str, order_key: String) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    if Shelf::Archived.holds(thread) {
                        return Some(format!("Conversation '{thread_id}' is archived, so its pin cannot be reordered."));
                    }
                    thread.lifecycle.pinned_at.is_none().then(|| {
                        format!(
                            "Conversation '{thread_id}' is not pinned, so it cannot be reordered."
                        )
                    })
                },
                Change::PinReordered { order_key },
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Put a conversation to sleep until a time the developer chose.
    ///
    /// **An overlay rather than a destination.** A snoozed conversation stays
    /// active in this data model — not archived, not settled, not deleted — and
    /// the two fields this writes only suppress it from the inbox until the wake
    /// time passes. Which is why it is not in the same vocabulary slot as
    /// [`Shell::set_archived`]'s two.
    ///
    /// **And there is no scheduler.** A snooze expires by being *read*: once the
    /// wake time is in the past, `effectiveSnoozed` stops classifying the
    /// conversation as snoozed and no event fires. Nothing here starts a timer,
    /// registers a task, or has anything to cancel — see [`Change::Snoozed`],
    /// which is where the whole of that is written down.
    ///
    /// **The invariants are `canSnooze`'s**, which is `canSettle` minus the live
    /// session: a running agent is snoozable, because snooze is a decision about
    /// the developer's attention and never an interruption of the work. What is
    /// refused is what a snooze would *hide* — a request the agent is blocked on
    /// them for, and a turn about to start. [`Attention`] is that difference, and
    /// an archived conversation is refused on top for [`Shell::settle`]'s reason:
    /// it is not in the inbox for a snooze to take it out of.
    ///
    /// **A repeat re-emits rather than being refused**, and it is keyed on the
    /// wake time — a second snooze to a *different* moment is a new decision and
    /// stamps the clock. See [`Change::re_emitted_at`] for the first half and
    /// [`Change::Snoozed`] for why the second is not tidiness.
    ///
    /// The wake time itself was judged at the parse ([`a_moment_still_ahead`]),
    /// because it needs the clock and no conversation at all.
    fn snooze(&self, thread_id: &str, until: String) -> Result<i64, CommandError> {
        // [`Shell::settle`]'s reason: one instant for the whole command rather
        // than a clock read per comparison.
        let adoption = Adoption::now();
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    if Shelf::Archived.holds(thread) {
                        return Some(format!(
                            "Conversation '{thread_id}' is archived, so it is already out of the \
                             inbox and there is nothing to snooze."
                        ));
                    }
                    thread
                        .busy(&adoption, Attention::Snoozing)
                        .map(|busy| would_hide(thread_id, busy, Attention::Snoozing))
                },
                Change::Snoozed { until },
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Wake a conversation now, because the developer chose the time badly.
    ///
    /// [`Shell::snooze`]'s twin, and — unlike [`Shell::unsettle`] — a true mirror
    /// image: there is no such thing as a conversation pinned *awake*, so both
    /// reasons clear both fields and the only thing the reason distinguishes is
    /// who decided. A command carries [`threads::BY_THE_USER`] and can carry
    /// nothing else; the neutral wake is this server's own, emitted when the
    /// developer sends a new message and spends the return ticket
    /// (`crate::threads::Threads::woken_by`).
    ///
    /// **The invariants are not this command's**, for [`Shell::unsettle`]'s
    /// reason: waking is the direction that makes work visible, so nothing it can
    /// do could hide a request. Archived is refused all the same, and that is one
    /// rule with the snooze half rather than symmetry for its own sake — there is
    /// no inbox for an archived conversation to come back to, and the activity
    /// wake reads the same shelf ([`Thread::wants_unsnoozing`]).
    ///
    /// **Waking one that is not asleep is answered rather than refused.** It
    /// lands on the state it is already in, so it re-emits at the moment the
    /// conversation already carried — see [`Change::re_emitted_at`].
    fn unsnooze(&self, thread_id: &str) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    Shelf::Archived.holds(thread).then(|| {
                        format!(
                            "Conversation '{thread_id}' is archived, so there is no inbox to wake \
                             it into. Unarchive it first."
                        )
                    })
                },
                Change::Unsnoozed {
                    reason: threads::BY_THE_USER,
                },
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Take a conversation the developer started by mistake off their list.
    ///
    /// **Deleting is soft, and none of the three reasons is squeamishness.** The
    /// checkpoint refs a turn wrote are real git objects in the developer's own
    /// repository, and a hard delete would orphan them; the threads table
    /// cascades, so removing the row would take the transcript and the work log
    /// with it in one statement; and the contract carries a deletion time on the
    /// thread, which is only meaningful if the thread survives to carry it. So
    /// this stamps [`crate::threads::Lifecycle::deleted_at`] and moves nothing
    /// else — see [`Change::Deleted`].
    ///
    /// **What the developer sees is a conversation that is gone.** It leaves both
    /// snapshots ([`crate::threads::Shelf`]), the project list is told so as a
    /// `thread-removed` rather than as a summary carrying a field the contract
    /// does not declare on it ([`crate::threads::Change::on_the_list`]), a fresh
    /// subscription to it is refused, and every later command against it is
    /// refused by [`Shell::dispatch`]. The row underneath all that is a recovery
    /// path rather than a state a client can reach.
    ///
    /// **A repeat is refused rather than answered**, as the archive pair is and
    /// for the same reason: this is not a standing answer the developer gave that
    /// folding twice lands on either way, it is a conversation leaving a list, and
    /// a second delete is a click on a control that is no longer there. Refused
    /// under the same lock the change is folded under — [`Threads::apply_unless`]
    /// — so two windows deleting at once cannot both be told they did it, which
    /// is why the general guard in [`Shell::dispatch`] deliberately leaves this
    /// one command to answer for itself.
    ///
    /// **The agent is not spoken to**, which is the archive commands' rule again:
    /// a session still running behind a deleted conversation goes on writing to a
    /// transcript nobody is watching until it ends by itself. Ending it is
    /// `thread.session.stop`'s job, and a delete that stopped a turn mid-flight
    /// would be a deletion that also interrupted work — see ticket 10's comments,
    /// where that is left where upstream leaves it.
    fn delete(&self, thread_id: &str) -> Result<i64, CommandError> {
        self.inner
            .threads
            .apply_unless(
                thread_id,
                |thread| {
                    thread.lifecycle.deleted().then(|| {
                        format!(
                            "Conversation '{thread_id}' was already deleted, so it was left as it \
                             is."
                        )
                    })
                },
                Change::Deleted,
            )
            .ok_or_else(|| self.not_open(thread_id))?
            .map_err(CommandError::new)
    }

    /// Give a project the developer's own name for it.
    ///
    /// The sidebar's rename dialog, and the whole of what this server reads from
    /// `project.meta.update` — the command's other three fields are refused at the
    /// parse rather than accepted and dropped, because this registry stores none
    /// of them. See [`Command::parse`].
    ///
    /// The number is taken and held until the change has been announced, which is
    /// [`Shell::create_project`]'s shape and is not optional: two aggregates
    /// publish onto the one feed the project list folds, and a client drops
    /// anything at or below the sequence it holds.
    ///
    /// A rename publishes `project-upserted` — the same event a creation does,
    /// carrying the whole project. The client's shell reducer upserts by id
    /// (`shellReducer.ts`), so one event shape serves both and there is no
    /// separate "renamed" the list would have to learn.
    ///
    /// A repeat is answered rather than refused, as a thread's is. It moves the
    /// row's `updated_at` and moves nothing the developer sees: the registry is
    /// read `ORDER BY created_at ASC, id ASC` ([`Database::registry`]), so the
    /// project list cannot be reordered by renaming something twice.
    fn rename_project(&self, project_id: &str, title: &str) -> Result<i64, CommandError> {
        let commit = self.inner.sequences.commit();
        let renamed = self
            .inner
            .database
            .rename_project(project_id, title, commit.sequence())
            .map_err(unavailable("rename the project"))?;

        match renamed {
            Rename::Committed { sequence, project } => {
                self.announce(project_upserted(sequence, &project));
                Ok(sequence)
            }
            Rename::Absent => Err(self.not_registered(project_id)),
        }
    }

    /// Hand the developer's permission decision to the agent waiting on it.
    ///
    /// **Answers with the log position rather than with a number of its own**, and
    /// that is the one thing about this command worth arguing. Every other command
    /// here commits something and answers with the number it committed at; this one
    /// commits nothing, because the events it causes — the resolution row, and
    /// whatever the agent does next — are published by the driver once the decision
    /// has actually reached the child. Taking a sequence here would number a change
    /// that had not happened, and the client drops anything at or below the number
    /// it holds, so the row that *did* happen could be dropped as stale.
    ///
    /// What the client needs from the answer is whether the decision landed, and
    /// that it gets: a decision with no session behind it is a typed failure with a
    /// sentence, which the composer shows.
    fn respond_to_approval(&self, respond: &RespondToApproval) -> Result<i64, CommandError> {
        // Refused here rather than in the driver, because this is where there is
        // still a client listening. A decision this server cannot read is not
        // rounded to the nearest one it can: the nearest one might be the one that
        // runs something.
        let decision = crate::worklog::Decision::parse(&respond.decision).ok_or_else(|| {
            CommandError::new(format!(
                "'{}' is not a permission decision this server understands.",
                respond.decision
            ))
        })?;

        if let Err(why) = self.inner.threads.answer(
            &respond.thread_id,
            threads::Answered {
                request_id: respond.request_id.clone(),
                decision,
            },
        ) {
            // There is no session, so there is nothing to settle the request the
            // developer is looking at — and the panel is folded out of *stored*
            // activities, so without this it comes back with the conversation
            // every time and the composer stays disabled forever. Saying so is
            // the only thing that can close it. The command still fails, because
            // it did.
            self.inner.threads.apply(
                &respond.thread_id,
                Change::Activity(crate::worklog::unanswerable(&respond.request_id)),
            );
            return Err(CommandError::new(why));
        }

        Ok(self.inner.sequences.current())
    }

    /// Hand the developer's answers to the agent that asked for them.
    ///
    /// [`Shell::respond_to_approval`] above, for questions, and everything
    /// argued there applies unchanged: it answers with the log position because
    /// it commits nothing, and a failure leaves a row that closes the question
    /// header so the composer cannot be stuck on a session that is gone.
    ///
    /// It has no equivalent of that method's first act. A permission decision is
    /// one of four verbs and an unreadable one is refused here rather than
    /// rounded to the nearest; answers are the developer's own words, and the
    /// only thing that can be wrong with them — not being an object at all — is
    /// refused at the parse.
    fn respond_to_user_input(&self, respond: &RespondToUserInput) -> Result<i64, CommandError> {
        if let Err(why) = self.inner.threads.answer_user_input(
            &respond.thread_id,
            threads::UserInputAnswered {
                request_id: respond.request_id.clone(),
                answers: respond.answers.clone(),
                rejected: respond.rejected,
            },
        ) {
            self.inner.threads.apply(
                &respond.thread_id,
                Change::Activity(crate::worklog::unanswerable_user_input(&respond.request_id)),
            );
            return Err(CommandError::new(why));
        }

        Ok(self.inner.sequences.current())
    }

    /// Stop the turn the agent is working on.
    ///
    /// **Answers with the log position rather than with a number of its own**,
    /// for the same reason `thread.approval.respond` does: it commits nothing.
    /// What it causes — the `turn.interrupted` row, the partial reply the agent
    /// had buffered, the turn settling — is published by the driver once the
    /// request has actually reached the child, and numbering a change here that
    /// had not happened would let the client drop the row that did.
    ///
    /// **Succeeding when there is nothing to stop is the behaviour, not a
    /// shortcut.** The client sends this command with no `turnId` precisely when
    /// it does not believe a turn is running, and the turn it *does* name can
    /// finish while the click is in flight. A failure in either case would be
    /// this server telling the developer that stopping an agent which is not
    /// running went wrong. The thread still has to exist: a command naming a
    /// conversation this server has never heard of is a client bug rather than a
    /// race, and is worth saying so.
    fn interrupt_turn(&self, interrupt: &InterruptTurn) -> Result<i64, CommandError> {
        self.open_thread(&interrupt.thread_id)?;
        self.inner
            .threads
            .interrupt(&interrupt.thread_id, interrupt.turn_id.clone())
            .map_err(CommandError::new)?;
        Ok(self.inner.sequences.current())
    }

    /// End the agent process behind a conversation, and keep the conversation.
    ///
    /// **Not [`Shell::interrupt_turn`] under another name.** An interrupt asks a
    /// running turn to stop and leaves the child alive, which is what makes the
    /// correction typed a moment later a correction. This ends the session, and
    /// the case it exists for — an agent that is idle or wedged, holding a
    /// process — has no turn to interrupt at all. The client reaches for it in
    /// two places, neither of them a stop button: before deleting a thread, and
    /// when moving a conversation to a different worktree
    /// (`useThreadActions.ts`, `BranchToolbarBranchSelector.tsx`).
    ///
    /// **The conversation survives, and so does the agent's own handle on it.**
    /// Nothing here touches the transcript, the work log, or
    /// [`Thread::provider_resume_cursor`] — which is how continuation
    /// outlives a process, because the next turn is started with `--resume` and
    /// the context is in the agent's store rather than in this server's
    /// transcript. So the next turn is a new session continuing the same
    /// conversation.
    ///
    /// **Stopping a conversation with no session is answered rather than
    /// refused**, which is [`Threads::interrupt`]'s reading of an absent turn
    /// applied to an absent process: the developer asked for no agent to be
    /// running and there is none. It answers with the log position, because
    /// nothing was committed — the same shape [`Shell::respond_to_approval`]
    /// argues, and for the same reason: numbering a change that did not happen
    /// would let the client drop one that did.
    ///
    /// When there *was* one, the receipt is published after the driver has been
    /// told and answered with. [`Change::SessionStopRequested`] carries the rest
    /// of that argument, including why the session's own `stopped` follows
    /// separately rather than being the only event.
    fn stop_session(&self, thread_id: &str) -> Result<i64, CommandError> {
        let stopped = self
            .inner
            .threads
            .stop_session(thread_id)
            .map_err(CommandError::new)?;
        if !stopped {
            return Ok(self.inner.sequences.current());
        }

        self.inner
            .threads
            .apply(thread_id, Change::SessionStopRequested)
            .ok_or_else(|| self.not_open(thread_id))
    }

    /// Register a conversation.
    ///
    /// The project has to be one this server knows, because the thread's whole
    /// purpose is to run an agent in that project's folder — a thread pointing
    /// at nothing would be a conversation that could never take a turn.
    fn create_thread(&self, create: &CreateThread, config: &ServerConfig) -> Result<i64, CommandError> {
        let project = self.project(&create.thread.project_id)?;
        self.inner
            .threads
            .create(create.to_thread(&project, &config.settings)?)
            .map_err(CommandError::new)
    }

    /// Send a turn: put the prompt in the transcript and hand it to the agent.
    ///
    /// Returns as soon as the prompt is queued. Nothing here waits for a process
    /// to start, let alone for the agent to answer — the developer has just
    /// pressed enter and what they are owed first is an acknowledgement.
    fn start_turn(&self, start: &StartTurn, config: &ServerConfig) -> Result<i64, CommandError> {
        if start.prepares_a_worktree() {
            return Err(CommandError::new(
                "This server cannot prepare a git worktree for a thread, so the turn was not \
                 started. Run the conversation in the project's own checkout instead.",
            ));
        }

        // Bootstrapping is how the UI's composer starts a *new* conversation.
        // Build the candidate now, but do not publish it until every part of its
        // first turn has passed the same preflight as an existing thread. A
        // refused first turn must leave a draft as what it was: absent here.
        let pending = if !self.inner.threads.contains(&start.thread_id) {
            let Some(create) = start.bootstrap_thread() else {
                return Err(CommandError::new(format!(
                    "There is no thread '{}' on this server, and the turn did not ask for one to \
                     be created.",
                    start.thread_id
                )));
            };
            let project = self.project(&create.thread.project_id)?;
            Some(create.to_thread(&project, &config.settings)?)
        } else {
            None
        };

        // Everything that can still refuse the turn happens before anything is
        // published. A refusal that had already created the draft or put the
        // prompt in its transcript would leave a conversation with no agent
        // alive to settle it.
        let thread = match &pending {
            Some(thread) => thread.clone(),
            None => self.open_thread(&start.thread_id)?,
        };
        let project = self.project(&thread.project_id)?;
        if let Some(selection) = &start.model_selection {
            selection_for(&thread, selection)?;
        }
        let prepared = crate::session::prepare(
            &thread,
            &config.settings,
            Arc::clone(&self.inner.mcp),
        ).map_err(CommandError::new)?;
        let attachments = crate::attachments::resolve_all(&start.message.attachments, &start.message.message_id);

        // OpenCode's native busy-session prompt is a steer, not a queued new
        // turn. Keep the active turn id at both public boundaries: the user
        // message in the transcript and the prompt handed to the live driver.
        // Other drivers continue through the ordinary fresh-turn path below.
        if thread.provider.driver == "opencode" {
            if let Some(turn_id) = self.inner.threads.active_turn(&start.thread_id) {
                let sequence = self.inner.threads.apply(
                    &start.thread_id,
                    Change::UserMessage {
                        message_id: start.message.message_id.clone(),
                        text: start.message.text.clone(),
                        turn_id: turn_id.clone(),
                    },
                ).ok_or_else(|| self.not_open(&start.thread_id))?;
                let starting = crate::session::starting(
                    &thread,
                    &where_the_work_happens(&thread, &project),
                    prepared,
                );
                crate::session::send(
                    &self.inner.threads,
                    &starting,
                    turn_id,
                    start.message.text.clone(),
                    attachments,
                ).map_err(CommandError::new)?;
                return Ok(sequence);
            }
        }

        if let Some(thread) = pending {
            self.inner
                .threads
                .create(thread)
                .map_err(CommandError::new)?;
        }

        let turn_id = threads::fresh_turn_id();
        // The developer's own message first, so it is in the transcript before
        // anything the agent says about it can be.
        let (_, after_message) = self.inner
            .threads
            .apply_and_read(
                &start.thread_id,
                Change::UserMessage {
                    message_id: start.message.message_id.clone(),
                    text: start.message.text.clone(),
                    turn_id: turn_id.clone(),
                },
            )
            .ok_or_else(|| self.not_open(&start.thread_id))?;
        let is_first_user_turn = after_message
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count()
            == 1;
        let (requested, thread) = self
            .inner
            .threads
            .apply_and_read(
                &start.thread_id,
                Change::TurnRequested {
                    turn_id: turn_id.clone(),
                    message_id: start.message.message_id.clone(),
                    model_selection: start.model_selection.clone(),
                    runtime_mode: start.runtime_mode.clone(),
                    interaction_mode: start.interaction_mode.clone(),
                },
            )
            .ok_or_else(|| self.not_open(&start.thread_id))?;

        // Read by the same commit that folded the request, so another window's
        // next metadata change cannot retune this turn before it starts.
        // Driver availability was already checked above and provider identity
        // cannot be changed by a thread event, so this step is infallible.
        let starting = crate::session::starting(
            &thread,
            &where_the_work_happens(&thread, &project),
            prepared,
        );

        // **Only when nothing is already working.** This publish exists for the
        // *first* turn — it is what lets the composer answer before the baseline
        // `git add -A` below, and the reasoning for that is at
        // `session::run`'s `running(…)` call. A turn queued behind a running one
        // needs none of it, and publishing it anyway breaks the pane three ways,
        // all from this one event:
        //
        // - `Starting` is drawn as **"connecting"** (`session-logic.ts`,
        //   `derivePhase`), so a conversation that is working says it is not.
        // - `activeTurnId` names the *queued* turn, so `MessagesTimeline`'s
        //   `runningTurnId` goes null and the running turn loses the row that
        //   says it is running (`ChatView.tsx`).
        // - The client's `turnStillRunning` guard reads
        //   `status === "running" && activeTurnId === turnId`
        //   (`threadReducer.ts`) — both false now, so the next buffered
        //   assistant message **settles the running turn mid-turn**. That is
        //   precisely the settle the `running(…)` comment says the reducer
        //   exists to avoid, arriving from the other side.
        //
        // Upstream guards the same publish the same way, and additionally never
        // names a turn on it at all: `ProviderCommandReactor.ts`'s
        // `if (options?.pendingTurnStart === true && thread.session?.status !== "running")`,
        // with `activeTurnId: null`. `starting` upstream means the *process* is
        // coming up — it is produced from a provider session that is
        // `connecting` — which is what the client's word for it says.
        // `.scratch/prompt-queueing/upstream-research.md` has the citations.
        //
        // [`SessionStatus::is_working`] rather than `== Running`, because two
        // prompts sent in quick succession leave the second one arriving while
        // the session is still `Starting` for the first, and a turn queued
        // behind a turn that has not begun is no more this event's business
        // than one queued behind a turn that has.
        let working = thread
            .session
            .as_ref()
            .is_some_and(|session| session.status.is_working() && session.active_turn_id.is_some());
        let sequence = if working {
            requested
        } else {
            self.inner
                .threads
                .apply(
                    &start.thread_id,
                    Change::Session(Session {
                        status: SessionStatus::Starting,
                        runtime_mode: starting.runtime_mode.clone(),
                        active_turn_id: Some(turn_id.clone()),
                        last_error: None,
                        updated_at: now_iso(),
                    }),
                )
                .ok_or_else(|| self.not_open(&start.thread_id))?
        };

        if let Err(why) = crate::session::send(
            &self.inner.threads,
            &starting,
            turn_id,
            start.message.text.clone(),
            attachments,
        ) {
            // The prompt is already in the transcript and the turn is already
            // marked running, so the refusal has to end them as well as being
            // returned — otherwise the conversation sits waiting for a turn that
            // was never handed to anything.
            self.inner.threads.apply(
                &start.thread_id,
                Change::Session(Session {
                    status: SessionStatus::Error,
                    runtime_mode: thread.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: Some(why.clone()),
                    updated_at: now_iso(),
                }),
            );
            return Err(CommandError::new(why));
        }

        if is_first_user_turn {
            let title_context = if start.message.text.trim().is_empty()
                && !start.message.attachments.is_empty()
            {
                format!("Attachments:\n{}", Value::Array(start.message.attachments.clone()))
            } else {
                start.message.text.clone()
            };
            self.generate_first_turn_title(
                &start.thread_id,
                &thread.title,
                &title_context,
                &where_the_work_happens(&thread, &project),
                config,
            );
        }

        Ok(sequence)
    }

    /// Improve a new conversation's provisional seed without joining its fate
    /// to the agent turn. The compare and write happen under the thread fold's
    /// lock, so a manual or provider-native rename that lands while generation
    /// is running owns the title thereafter.
    fn generate_first_turn_title(
        &self,
        thread_id: &str,
        provisional_title: &str,
        message: &str,
        directory: &str,
        config: &ServerConfig,
    ) {
        let selection = &config.settings.text_generation_model_selection;
        let Some(instance_id) = selection.get("instanceId").and_then(Value::as_str) else {
            return;
        };
        let Ok(instance) = crate::provider::resolve_instance(&config.settings, instance_id, None)
        else {
            return;
        };
        let model = selection.get("model").and_then(Value::as_str).map(str::to_string);
        let shell = self.clone();
        let thread_id = thread_id.to_string();
        let expected = provisional_title.to_string();
        let context = message.to_string();
        let directory = directory.to_string();
        tokio::spawn(async move {
            let generated = TextGeneration::new()
                .generate(
                    &instance,
                    &directory,
                    model.as_deref(),
                    TextOperation::ThreadTitle { context },
                )
                .await;
            let Ok(ResultText::ThreadTitle(title)) = generated else {
                return;
            };
            let title = title.trim();
            if title.is_empty() {
                return;
            }
            let _ = shell.inner.threads.apply_unless(
                &thread_id,
                |thread| {
                    (thread.title != expected || thread.title_regeneration.is_some())
                        .then(|| "the provisional title was superseded".to_string())
                },
                Change::MetaUpdated(MetaUpdate {
                    title: Some(title.to_string()),
                    title_regeneration: None,
                    regenerate_title: false,
                    previous_title: None,
                    model_selection: None,
                    branch: None,
                    worktree_path: None,
                }),
            );
        });
    }

    /// A thread that was there a statement ago and is not now. Unreachable while
    /// nothing removes threads, and cheaper to say than to reason about.
    fn not_open(&self, thread_id: &str) -> CommandError {
        CommandError::new(format!("Thread '{thread_id}' is not open."))
    }

    fn open_thread(&self, thread_id: &str) -> Result<Thread, CommandError> {
        self.inner
            .threads
            .get(thread_id)
            .ok_or_else(|| self.not_open(thread_id))
    }

    fn project(&self, project_id: &str) -> Result<Project, CommandError> {
        self.inner
            .database
            .project(project_id)
            .map_err(unavailable("look up the project"))?
            .ok_or_else(|| self.not_registered(project_id))
    }

    /// A command about a project this server has never registered. Said in one
    /// place because two commands refuse for it — a thread cannot be created in a
    /// project that is not there, and a project that is not there cannot be
    /// renamed — and the developer should not have to work out that two different
    /// sentences mean the same thing.
    fn not_registered(&self, project_id: &str) -> CommandError {
        CommandError::new(format!(
            "Project '{project_id}' is not registered with this server."
        ))
    }

    fn create_project(&self, create: &CreateProject) -> Result<i64, CommandError> {
        // Outside the commit lock: checking a folder touches the filesystem and
        // can be slow, and nothing about it depends on what else is committing.
        let root = WorkspaceRoot::check(&create.workspace_root)
            .map_err(|rejection| CommandError::new(rejection.message()))?;
        let title = match create.title.trim() {
            "" => root.inferred_title(),
            given => given.to_string(),
        };

        // Taking the number holds the log open until the change it numbers has
        // been announced. Two aggregates publish onto one feed now, so nothing
        // weaker orders them — see `store::Sequences`.
        let commit = self.inner.sequences.commit();
        let insert = self
            .inner
            .database
            .insert_project(
                &create.project_id,
                &title,
                &root,
                create.created_at.as_deref(),
                commit.sequence(),
            )
            .map_err(unavailable("register the project"))?;

        match insert {
            // The project comes back from the write itself rather than from a
            // second read. That is what makes the event a subscriber sees
            // identical to the row a restart will find — and it means there is
            // no case where the registry committed but the client was told the
            // command failed.
            Insert::Committed { sequence, project } => {
                self.announce(project_upserted(sequence, &project));
                Ok(sequence)
            }
            Insert::Occupied { existing, conflict } => Err(CommandError::new(match conflict {
                Conflict::Id => format!(
                    "Project '{}' already exists and cannot be created twice.",
                    existing.id
                ),
                Conflict::WorkspaceRoot => format!(
                    "Project '{}' already exists for workspace root '{}'.",
                    existing.title, existing.workspace_root
                ),
            })),
        }
    }

    /// Take a project off the registry, and its conversations with it.
    ///
    /// **Several events, and therefore several numbers.** The client's shell
    /// reducer answers `project-removed` by filtering the projects and nothing
    /// else (`shellReducer.ts`), so a conversation whose project has gone stays
    /// in its snapshot until a `thread-removed` says otherwise — and the rows
    /// have already gone, by the schema's own cascade. One number for all of them
    /// would not do: a client ignores anything at or below the sequence it holds,
    /// so every event after the first would be dropped. The answer is the last
    /// number taken, which is the position the log reached.
    fn delete_project(&self, project_id: &str, index: &Index) -> Result<i64, CommandError> {
        // Which conversations there *are* is a question for the registry, not the
        // database: a thread reaches the database eventually (see
        // [`crate::transcripts`]), so a project deleted seconds after a
        // conversation started would leave that conversation behind if the stored
        // rows were the source of truth. Read before the delete either way, because
        // the delete is what makes them unreadable.
        let thread_ids = self.inner.threads.of_project(project_id);

        // The guard is released at the end of this block, before any further
        // number is taken — it is not reentrant, and each announcement only has
        // to be ordered against the writers it races.
        let removal = {
            let commit = self.inner.sequences.commit();
            let removal = self
                .inner
                .database
                .remove_project(project_id, commit.sequence())
                .map_err(unavailable("remove the project"))?;

            if let Removal::Committed {
                sequence,
                canonical_root,
            } = &removal
            {
                // The only moment on this wire that means "this project is
                // closed", and so the only moment the server can give back what
                // it was holding to keep the project's file tree fresh. Before
                // the announcement, so a client that reacts to `project-removed`
                // by asking about something else does not race a release.
                index.release(canonical_root);
                self.announce(project_removed(*sequence, project_id));
            }
            removal
        };

        let mut sequence = removal.sequence();
        if matches!(removal, Removal::Committed { .. }) {
            // Forgotten here as well as in the database, and before the
            // announcements: a thread that outlived its project in this registry
            // would be in every snapshot until the next restart and gone after
            // it, which is the worst of both answers.
            self.inner.threads.forget(&thread_ids);
            for thread_id in &thread_ids {
                let commit = self.inner.sequences.commit();
                sequence = commit.sequence();
                self.announce(thread_removed(sequence, thread_id));
            }
        }

        Ok(sequence)
    }

    /// Open an `orchestration.subscribeShell` subscription: the registry now,
    /// then every change to it.
    ///
    /// **A client whose cursor is still current is sent no snapshot.** See
    /// [`Sequences::caught_up`] for the rule and ADR-0016 for why it is the
    /// only part of `afterSequence` this server can answer.
    pub fn subscribe(&self, payload: &Value) -> EventSource {
        let wants_marker = payload
            .get("requestCompletionMarker")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let cursor = crate::rpc::resume_cursor(payload);

        // Subscribed to *before* the description closure is handed over, so a
        // change landing between here and the pump's first read arrives as an
        // event rather than falling into the gap. The cost is that such a
        // change can be seen twice, once folded into the snapshot and once as
        // an event — which is exactly what the contract's "overlapping events
        // are deduped by sequence on the client" is there to absorb.
        let updates = self.inner.updates.subscribe();
        let shell = self.clone();
        // The marker means "the initial catch-up is over", so it is owed once
        // however many times the world is re-described afterwards.
        let marker_owed = AtomicBool::new(wants_marker);

        EventSource::new(
            move || {
                let mut items = Vec::new();
                if !shell.inner.sequences.caught_up(cursor) {
                    match shell.snapshot() {
                        Ok(snapshot) => items.push(snapshot),
                        // Nothing rather than an empty registry, and the marker
                        // stays owed. An empty snapshot would be a claim that
                        // the user has no projects, which is a worse answer than
                        // silence — and the marker would be a claim that a
                        // catch-up succeeded when it did not.
                        Err(error) => {
                            eprintln!("laplus: cannot describe the project registry: {error}");
                            return Vec::new();
                        }
                    }
                }
                if marker_owed.swap(false, Ordering::Relaxed) {
                    items.push(json!({"kind": "synchronized"}));
                }
                items
            },
            updates,
        )
    }

    fn snapshot(&self) -> Result<Value, StorageError> {
        Ok(json!({
            "kind": "snapshot",
            "snapshot": self.shell_snapshot()?,
        }))
    }

    /// The `OrchestrationShellSnapshot` on its own, with no stream envelope
    /// around it.
    ///
    /// The body of `GET /api/orchestration/shell` (ticket 31), and the same
    /// object the subscription's opening chunk carries under `snapshot`. One
    /// builder for both, because the client takes whichever of the two it can
    /// get and two builders would let the shell a developer sees depend on
    /// which transport won.
    pub fn shell_snapshot(&self) -> Result<Value, StorageError> {
        self.snapshot_of(Shelf::Working)
    }

    /// `orchestration.getArchivedShellSnapshot` — the same object, filtered to
    /// the conversations the developer put away.
    ///
    /// The project list excludes archived threads, so this is the only way back
    /// to one: it is what the archived section of the settings panel is drawn
    /// from, and the unarchive control there is the only one that exists. It
    /// carries **every** project rather than only those with something archived
    /// in them, because the panel groups the threads by project and looks each
    /// one up in this list (`SettingsPanels.tsx`) — a filtered project list would
    /// silently drop the threads whose project had nothing else archived.
    ///
    /// Built by [`Shell::shell_snapshot`]'s own builder, which the ticket asks
    /// for by name: two builders would let the world the client draws depend on
    /// which transport answered first.
    pub fn archived_shell_snapshot(&self) -> Result<Value, StorageError> {
        self.snapshot_of(Shelf::Archived)
    }

    fn snapshot_of(&self, shelf: Shelf) -> Result<Value, StorageError> {
        Ok(shell_snapshot(
            &self.inner.database.registry()?,
            &self.inner.threads,
            self.inner.sequences.current(),
            shelf,
        ))
    }

    /// The folder of every registered project.
    ///
    /// For [`crate::catalogue`], which scans each one for the skills a developer
    /// keeps beside the code they apply to. Read from the registry rather than
    /// held anywhere, because a project added a moment ago has to be scanned by
    /// the next refresh and a cached list is one more thing to invalidate.
    ///
    /// An unreadable registry is an empty list: the caller's answer to it is a
    /// picker with the user's own skills and none of the projects', which is a
    /// smaller failure than a provider that would not describe itself.
    pub fn workspace_roots(&self) -> Vec<PathBuf> {
        self.inner
            .database
            .registry()
            .map(|registry| {
                registry
                    .projects
                    .iter()
                    .map(|project| PathBuf::from(&project.workspace_root))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn announce(&self, event: Value) {
        // `send` on a broadcast channel never blocks — it drops the oldest
        // value when the buffer is full, and a subscriber that lags is resent a
        // snapshot instead. So this cannot deadlock under the commit lock.
        let _ = self.inner.updates.send(event);
    }

}

/// A storage failure, said in a sentence a user can act on.
///
/// The SQLite detail is kept — it is the only thing that distinguishes a full
/// disk from a deleted file — but it is prefixed with what the server was
/// trying to do, because the client shows this string and nothing else.
fn unavailable(attempting: &'static str) -> impl Fn(StorageError) -> CommandError {
    move |error| CommandError::new(format!("Could not {attempting}: {error}"))
}

/// Every project and every thread: what a shell subscription opens with, and
/// what `GET /api/orchestration/shell` answers with.
///
/// `snapshotSequence` is the log position rather than the registry's stored one.
/// They differ as soon as a conversation is under way — a turn advances the log
/// without writing a row — and a snapshot that reported the *stored* number
/// would be older than events the client had already folded, so the client would
/// re-apply them.
///
/// `updatedAt` is the later of the registry's own timestamp and the newest
/// thread's, because the field describes the shell rather than either half of it.
/// `subscribeAuthAccess`'s only event: the whole access list, wrapped.
///
/// `version` is a literal `1` in the contract and the client refuses anything
/// else. `revision` is required and typed `Schema.Number` with no ordering
/// contract attached to it — the reader keeps the latest snapshot and nothing
/// compares two — so it is the wall clock rather than a counter this server
/// would have to persist to keep monotonic across a restart.
///
/// A list that cannot be read becomes an empty one and a line in the log. The
/// alternative is `AuthAccessStreamError`, which would tear down the
/// subscription and leave Settings with no list *and* no way to mint into it;
/// the panel already renders "No pairing links or client sessions" for the
/// empty case, and a code minted afterwards republishes.
fn access_snapshot_event(database: &Database) -> Value {
    let links = database.active_pairing_links().unwrap_or_else(|error| {
        eprintln!("laplus: cannot read the pairing links to publish them: {error}");
        Vec::new()
    });
    json!({
        "version": 1,
        "revision": crate::clock::now_epoch_millis(),
        "type": "snapshot",
        "payload": {
            "pairingLinks": crate::http::pairing_links(&links),
            "clientSessions": [],
        },
    })
}

fn shell_snapshot(
    registry: &Registry,
    threads: &Threads,
    sequence: i64,
    shelf: Shelf,
) -> Value {
    let updated_at = threads
        .latest_change(shelf)
        .filter(|latest| latest > &registry.updated_at)
        .unwrap_or_else(|| registry.updated_at.clone());

    json!({
        "snapshotSequence": sequence,
        "projects": registry
            .projects
            .iter()
            .map(Project::to_value)
            .collect::<Vec<Value>>(),
        "threads": threads.shell_summaries(shelf),
        "updatedAt": updated_at,
    })
}

fn project_upserted(sequence: i64, project: &Project) -> Value {
    json!({
        "kind": "project-upserted",
        "sequence": sequence,
        "project": project.to_value(),
    })
}

fn project_removed(sequence: i64, project_id: &str) -> Value {
    json!({
        "kind": "project-removed",
        "sequence": sequence,
        "projectId": project_id,
    })
}

fn thread_removed(sequence: i64, thread_id: &str) -> Value {
    json!({
        "kind": "thread-removed",
        "sequence": sequence,
        "threadId": thread_id,
    })
}

/// The fields of `project.create` this server reads. `commandId` and
/// `defaultModelSelection` are in the contract and deliberately absent here —
/// see the module documentation for why neither is kept.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProject {
    project_id: String,
    #[serde(default)]
    title: String,
    workspace_root: String,
    /// `None` once [`Command::parse`] has been through it means "the server
    /// should stamp this" — see [`usable_timestamp`].
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteProjectPayload {
    project_id: String,
}

/// The fields that describe a conversation, wherever they arrive from.
///
/// They arrive from two places and are identical in both: `thread.create` sends
/// them beside a `threadId`, and `thread.turn.start` sends them under
/// `bootstrap.createThread` with the id on the command. One struct, because two
/// would be two places to get `runtimeMode`'s default wrong.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadFields {
    project_id: String,
    #[serde(default)]
    title: String,
    /// `{instanceId, model}`. Required by the contract in both places it
    /// arrives, and it is what the agent is started with — so a thread without
    /// one would be a conversation nothing could choose a model for.
    model_selection: Value,
    #[serde(default = "default_runtime_mode")]
    runtime_mode: String,
    #[serde(default = "default_interaction_mode")]
    interaction_mode: String,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    worktree_path: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

impl ThreadFields {
    /// Both modes checked against the contract's own vocabularies, or a refusal
    /// naming the conversation that was not created.
    ///
    /// Here rather than in either parse arm for the reason the struct is one
    /// struct: the fields arrive through two doors and are identical in both, so
    /// a check in one arm would leave the other open — which is exactly the
    /// shape of the hole ticket 12 was written about.
    ///
    /// The modes are the only two fields with a closed vocabulary. `title` is
    /// free text, `modelSelection` is the client's own, `branch` is carried and
    /// never acted on, and `worktreePath` is a path this server does not
    /// second-guess — see [`where_the_work_happens`], which is the whole of
    /// what it does with one.
    fn modes_the_contract_names(&self, thread_id: &str) -> Result<(), CommandError> {
        named_by_the_contract(&self.runtime_mode, &RUNTIME_MODES, "runtime mode", thread_id)?;
        named_by_the_contract(
            &self.interaction_mode,
            &INTERACTION_MODES,
            "interaction mode",
            thread_id,
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
struct CreateThread {
    thread_id: String,
    thread: ThreadFields,
}

impl CreateThread {
    /// The thread as it will be stored.
    ///
    /// The project supplies the title when the client did not, the same way it
    /// does for a project with a blank name — the composer normally sends one,
    /// and a conversation called "" would be unreachable in the thread list.
    fn to_thread(&self, project: &Project, settings: &crate::config::Settings) -> Result<Thread, CommandError> {
        let created_at = self
            .thread
            .created_at
            .clone()
            .and_then(usable_timestamp)
            .unwrap_or_else(now_iso);
        let title = match self.thread.title.trim() {
            "" => project.title.clone(),
            given => given.to_string(),
        };

        let instance_id = provider_instance(&self.thread.model_selection, &self.thread_id)?;
        let identity = crate::provider::resolve_instance(settings, instance_id, None)
            .map(|instance| instance.identity().clone())
            .map_err(|unavailable| CommandError::new(match unavailable {
                crate::provider::InstanceUnavailable::Unknown => format!(
                    "Provider instance '{instance_id}' is not registered, so thread '{}' was not \
                     created.", self.thread_id
                ),
                crate::provider::InstanceUnavailable::Disabled => format!(
                    "Provider instance '{instance_id}' is disabled, so thread '{}' was not \
                     created.", self.thread_id
                ),
                crate::provider::InstanceUnavailable::Mismatched { .. } => {
                    unreachable!("thread creation records the configured driver")
                }
            }))?;

        Ok(Thread {
            id: self.thread_id.clone(),
            project_id: self.thread.project_id.clone(),
            title,
            title_regeneration: None,
            provider: identity,
            model_selection: self.thread.model_selection.clone(),
            runtime_mode: self.thread.runtime_mode.clone(),
            interaction_mode: self.thread.interaction_mode.clone(),
            branch: self.thread.branch.clone(),
            worktree_path: self.thread.worktree_path.clone(),
            updated_at: created_at.clone(),
            created_at,
            messages: Vec::new(),
            activities: Vec::new(),
            checkpoints: Vec::new(),
            session: None,
            latest_turn: None,
            latest_user_message_at: None,
            // Nothing has run yet, so there is no provider continuation to resume.
            provider_resume_cursor: None,
            // A new conversation is in the inbox, which is what all six being
            // absent means. Nothing creates a thread already archived, settled,
            // snoozed or deleted.
            lifecycle: crate::threads::Lifecycle::default(),
        })
    }
}

fn provider_instance<'a>(selection: &'a Value, thread_id: &str) -> Result<&'a str, CommandError> {
    selection
        .get("instanceId")
        .and_then(Value::as_str)
        .filter(|instance_id| !instance_id.trim().is_empty())
        .ok_or_else(|| {
            CommandError::new(format!(
                "Thread '{thread_id}' needs a model selection naming a provider instance."
            ))
        })
}

fn selection_for(thread: &Thread, selection: &Value) -> Result<(), CommandError> {
    let selected = provider_instance(selection, &thread.id)?;
    if selected == thread.provider.instance_id {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "Thread '{}' belongs to provider instance '{}', so its model selection cannot name \
         provider instance '{selected}'. Start a new conversation to use another provider.",
        thread.id, thread.provider.instance_id
    )))
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadPayload {
    thread_id: String,
    #[serde(flatten)]
    thread: ThreadFields,
}

/// `thread.turn.start` — the command the whole ticket exists to answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartTurn {
    thread_id: String,
    message: TurnMessage,
    /// What the composer had selected when the developer pressed enter. Absent
    /// means "whatever the thread already had", which is why none of these three
    /// has a default — a default would move a conversation back to the default
    /// every turn rather than leaving it where the developer put it.
    #[serde(default)]
    model_selection: Option<Value>,
    #[serde(default)]
    runtime_mode: Option<String>,
    #[serde(default)]
    interaction_mode: Option<String>,
    #[serde(default)]
    bootstrap: Option<Bootstrap>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnMessage {
    message_id: String,
    /// The contract types this as a plain string, so an empty prompt decodes.
    /// It is not refused here: the CLI is free to treat one however it likes,
    /// and a server that second-guessed it would be inventing a rule.
    #[serde(default)]
    text: String,
    /// Images pasted into the composer. Carried so a client that sends them is
    /// not refused, and dropped on the way to the agent — attachments need the
    /// asset service the spec puts out of scope.
    #[serde(default)]
    attachments: Vec<Value>,
}

/// The work a turn asks to have done before it starts.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    #[serde(default)]
    create_thread: Option<ThreadFields>,
    /// Present when the UI wants the turn run in a fresh git worktree. Refused
    /// by name rather than ignored — see [`Shell::start_turn`], where running in
    /// the project root instead would silently put the agent's changes somewhere
    /// the developer did not ask for.
    #[serde(default)]
    prepare_worktree: Option<Value>,
}

/// `thread.turn.interrupt` — the developer stopping the agent.
///
/// `turnId` is optional in the contract and the UI means something by leaving it
/// out: `buildThreadTurnInterruptInput` sends it only while the session is
/// `running`, so an absent one is the client saying "stop whatever is going, if
/// anything is". Carried as an `Option` all the way to the driver rather than
/// resolved here, because the only thing that knows what is running is the thing
/// running it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InterruptTurn {
    thread_id: String,
    #[serde(default)]
    turn_id: Option<String>,
}

/// `thread.approval.respond` — the developer answering the agent.
///
/// `decision` is a string rather than an enum here so that an unreadable one is
/// refused with a message naming it, instead of failing the whole command's
/// deserialization with serde's own account of which variants it knows.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondToApproval {
    thread_id: String,
    request_id: String,
    decision: String,
}

/// `thread.user-input.respond` — the developer answering the agent's questions.
///
/// `answers` is an open record in the contract (`ProviderUserInputAnswers`) and
/// stays a `Value` here: its keys are the agent's own question text and its
/// values are labels the agent wrote, so there is nothing in it this server is
/// entitled to have an opinion about. It is read by the CLI, and the one thing
/// that has to be true of it — that it is an object rather than a list or a
/// number — is checked in [`Command::parse`] rather than by a type, so that a
/// malformed one is refused with a sentence about answers instead of serde's
/// account of a shape nobody wrote down.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondToUserInput {
    thread_id: String,
    request_id: String,
    #[serde(default)]
    answers: Value,
    #[serde(default)]
    rejected: bool,
}

/// `project.meta.update` — the sidebar's rename dialog.
///
/// The command's other three fields — `workspaceRoot`, `defaultModelSelection`
/// and `scripts` — are absent here on purpose and are refused rather than
/// dropped; [`Command::parse`] is where that is argued.
///
/// The title is optional in the contract and therefore optional here, so that a
/// command carrying one of those three and no title is refused for the field this
/// server cannot keep rather than for a missing title it never needed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProjectMetaPayload {
    project_id: String,
    #[serde(default)]
    title: Option<String>,
}

/// The fields of `project.meta.update` this registry has nowhere to put.
///
/// A workspace root is a project's *identity* — `canonical_root` is derived from
/// it and is what "the same project" is answered by — so moving one is a different
/// act from renaming, with a duplicate check and a filesystem check of its own.
/// The other two are constants on the wire, each a later ticket's; see
/// [`crate::projects`].
const UNSTORED_PROJECT_FIELDS: [&str; 3] = ["workspaceRoot", "defaultModelSelection", "scripts"];

/// `thread.meta.update` — the conversation's own description, as the client keeps
/// it.
///
/// The four durable metadata fields are optional and only the ones that arrived
/// are applied — see [`MetaUpdate`], which is the same shape one step further
/// in. `regenerateTitle` is a fifth, transient intent: it starts background work
/// and publishes [`TitleRegeneration`] instead of becoming a database column.
///
/// `commandId` supplies the regeneration request identity, so overlapping work
/// can be settled by newest request rather than completion order. It remains
/// otherwise unread, as described in the module documentation. `expectedBranch`
/// is a compare-and-swap on the branch that **nothing in this
/// repository sends**: no call site in `apps/web` or `packages/client-runtime`
/// builds one. Honouring it would mean inventing the semantics of a guard no
/// client asks for, and refusing a payload that carried one would refuse a client
/// merely more careful than ours. So it is ignored, and this comment is the record
/// that it was a decision.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateThreadMetaPayload {
    thread_id: String,
    #[serde(default)]
    command_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    regenerate_title: bool,
    #[serde(default)]
    model_selection: Option<Value>,
    #[serde(default, deserialize_with = "given")]
    branch: Given<String>,
    #[serde(default, deserialize_with = "given")]
    worktree_path: Given<String>,
}

/// Tell an absent field from one sent as `null`.
///
/// serde's own `Option` cannot: it reads `null` as `None`, which is the same
/// answer it gives for a field that was not there — and on this command the two
/// mean opposite things, "leave it alone" against "clear it". Wrapping the read in
/// a second `Some` makes the outer layer mean *presence* and the inner one mean
/// *value*, which is what [`Given`] is.
fn given<'de, D, T>(deserializer: D) -> Result<Given<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// `thread.runtime-mode.set` — the composer's runtime picker, moved
/// mid-conversation.
///
/// The mode is a plain `String` rather than an enum for the reason
/// [`RespondToApproval::decision`] is one: an unreadable value is refused with a
/// sentence naming it and the thread, instead of failing the whole command's
/// deserialization with serde's own account of which variants it knows.
///
/// `createdAt` is in the contract and deliberately absent here. It is the moment
/// the *client* built the command, and what the thread's `updatedAt` and the
/// event's have to be is the moment this server committed — every other change
/// on this feed is stamped by [`crate::clock::now_iso`], and one that was not
/// would let a client with a skewed clock reorder the thread list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRuntimeModePayload {
    thread_id: String,
    runtime_mode: String,
}

/// `thread.interaction-mode.set` — the composer's plan-mode toggle, moved
/// mid-conversation. [`SetRuntimeModePayload`]'s twin, and everything argued there
/// applies unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetInteractionModePayload {
    thread_id: String,
    interaction_mode: String,
}

/// `thread.checkpoint.revert` — the working tree, put back to a turn boundary.
///
/// The count is a `u64` because the contract types it `NonNegativeInt`, and
/// reading it as one is the whole of the check: a negative number, a fraction or
/// a string fails the deserialization and is refused as a malformed payload
/// naming the conversation. Which turns this conversation actually has is a
/// question about the registry rather than about the payload, and is
/// [`Shell::revert_checkpoint`]'s.
///
/// `createdAt` is in the contract and deliberately absent here, for
/// [`SetRuntimeModePayload`]'s reason: it is the moment the *client* built the
/// command, and every event on this feed is stamped by [`crate::clock::now_iso`]
/// so that a skewed clock cannot reorder a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertCheckpointPayload {
    thread_id: String,
    turn_count: u64,
}

fn publish_revert_failure(
    threads: &Threads,
    thread_id: &str,
    turn_count: u64,
    why: &str,
    tree_restored: bool,
) {
    let message = if tree_restored {
        format!(
            "The project was put back to how it looked at turn {turn_count}, but OpenCode history \
             could not be rolled back. The restored files and later \
             checkpoint references were kept so this partial state can be recovered: {why}"
        )
    } else {
        format!(
            "The project could not be put back to how it looked at turn {turn_count}, so it is \
             still as this conversation left it: {why}"
        )
    };
    threads.apply(
        thread_id,
        Change::Activity(crate::threads::Activity::failed("revert.failed", &message)),
    );
}

/// A command whose whole payload is the conversation it is about.
///
/// `thread.session.stop`, `thread.archive`, `thread.unarchive` and
/// `thread.settle`. All four
/// carry a `threadId` and the fields every command carries, and this server reads
/// none of those — see the module documentation for `commandId` and
/// [`SetRuntimeModePayload`] for `createdAt`, which is the moment the *client*
/// built the command while every event on this feed is stamped by
/// [`crate::clock::now_iso`]. So the conversation is the payload, and naming none
/// is the only thing that can be wrong with one.
///
/// One struct for the four rather than four of one field, because four would
/// be four places to get the field name wrong. What each command can still be
/// refused for is a question about the *thread* — whether it exists, whether an
/// agent is running behind it, which list it is already on — and each is asked
/// where the answer lives.
///
/// Deliberately **not** a `turnId` on any of them. That is what distinguishes a
/// session stop from `thread.turn.interrupt`, where naming the wrong turn would
/// stop work the developer never saw start; here there is nothing for a turn to
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AboutAThread {
    thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinPayload {
    #[serde(flatten)]
    about: AboutAThread,
    order_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReorderPinPayload {
    #[serde(flatten)]
    about: AboutAThread,
    order_key: String,
}

/// `thread.unsettle` and `thread.unsnooze` — [`AboutAThread`] and the one thing
/// that makes the two of them their own payload.
///
/// The conversation is **flattened in** rather than declared again, which is
/// [`AboutAThread`]'s own argument applied to the two commands that have a fifth
/// field: a second `thread_id` here would be a second place to get the field name
/// wrong, and the sentence [`read_about_a_thread`] builds reads the id out of the
/// raw payload either way. One struct for both for the same reason there is one
/// for the four — the shape is one shape, and the *values* the reason may take
/// belong to each command rather than to this.
///
/// `reason` is **required rather than defaulted**, and that is the contract's own
/// shape: `ThreadUnsettleCommand` and `ThreadUnsnoozeCommand` both declare it as a
/// literal, not an optional. A default of `user` here would accept a payload the
/// contract calls malformed and then act on a guess about what the client meant —
/// and for an unsettle the two reasons do not leave the conversation in the same
/// state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WithAReason {
    #[serde(flatten)]
    about: AboutAThread,
    reason: String,
}

/// `thread.snooze` — the conversation and the moment it should come back.
///
/// The wake time is the only field on any of these commands that this server has
/// to *judge* rather than store: everything else is a value the developer chose
/// among ones the contract names, and this one is a moment that has to be ahead
/// of now. [`Command::parse`] is where that is decided, because it needs the
/// clock and nothing else — no conversation has to be looked at to know that a
/// time which has already passed is not a time to wake at.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snooze {
    #[serde(flatten)]
    about: AboutAThread,
    snoozed_until: String,
}

impl StartTurn {
    fn prepares_a_worktree(&self) -> bool {
        self.bootstrap
            .as_ref()
            .is_some_and(|bootstrap| bootstrap.prepare_worktree.is_some())
    }

    /// The thread this turn asks to have created, if it asks for one.
    fn bootstrap_thread(&self) -> Option<CreateThread> {
        let fields = self.bootstrap.as_ref()?.create_thread.clone()?;
        Some(CreateThread {
            thread_id: self.thread_id.clone(),
            thread: fields,
        })
    }
}

fn default_runtime_mode() -> String {
    DEFAULT_RUNTIME_MODE.to_string()
}

fn default_interaction_mode() -> String {
    DEFAULT_PROVIDER_INTERACTION_MODE.to_string()
}

/// Keep a client's `createdAt` only if it is one.
///
/// `IsoDateTime` is a bare `Schema.String` upstream, so nothing on either side
/// of the wire checks this — which is exactly why it is worth checking here.
/// The value is stored forever, is the registry's sort key
/// (`ORDER BY created_at`), and is what the UI parses to group projects by age.
/// A client that sent `"yesterday"` would not break the client's decode; it
/// would quietly sort itself to one end of the list on every run from now on.
///
/// The shape, not the calendar: `YYYY-MM-DDT…`, which is all that distinguishes
/// a timestamp from a typo. Anything else falls back to the database's clock,
/// because dropping a wrong answer for a right one costs nothing here.
fn usable_timestamp(stamp: String) -> Option<String> {
    let bytes = stamp.as_bytes();
    let shaped = bytes.len() >= 11
        && bytes[..10]
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => *byte == b'-',
                _ => byte.is_ascii_digit(),
            })
        && bytes[10] == b'T';

    shaped.then_some(stamp)
}

impl Command {
    /// Read one `orchestration.dispatchCommand` payload.
    ///
    /// The `type` is taken by hand before the rest is deserialized so that an
    /// unimplemented command names itself in the refusal. The contract carries
    /// twenty commands a client can dispatch and laplus now answers all twenty,
    /// so what reaches the arm at the bottom is a command the *contract* does not
    /// name — a server-side one a client cannot send, or a typo — and naming it
    /// is still the most useful thing the message can do.
    fn parse(payload: &Value) -> Result<Command, CommandError> {
        let kind = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| CommandError::new("A command must carry a 'type'."))?;

        match kind {
            "project.create" => {
                let create: CreateProject = read(payload, kind)?;
                Ok(Command::CreateProject(CreateProject {
                    project_id: non_blank(create.project_id, "projectId", kind)?,
                    created_at: create.created_at.and_then(usable_timestamp),
                    ..create
                }))
            }
            // **Only the title.** The command carries three other fields and
            // this server stores none of them: a workspace root is a project's
            // identity and moving one is a different act from renaming it, and
            // `defaultModelSelection` and `scripts` are constants on the wire
            // (see [`crate::projects`], where each is a later ticket's).
            //
            // Refused rather than accepted and ignored, which is ADR-0009's
            // declined-setting posture. The UI does send one of them — the script
            // editor sends `{projectId, scripts}` — and answering it with a
            // sequence would tell the developer their script was saved, leaving
            // them to find out at the next restart that it was not. A refusal is
            // the same outcome they get today, with a sentence saying why.
            //
            // On *presence*, not on the value. Two of the three would be a no-op
            // at the values this server reports — it publishes `scripts: []` and
            // `defaultModelSelection: null` as constants — so a value-sensitive
            // rule would let deleting the last script succeed while adding one
            // failed, which is a worse account of "this server does not keep
            // these" than refusing both.
            "project.meta.update" => {
                let rename: UpdateProjectMetaPayload = read(payload, kind)?;
                let project_id = non_blank(rename.project_id, "projectId", kind)?;
                for field in UNSTORED_PROJECT_FIELDS {
                    if payload.get(field).is_some() {
                        return Err(CommandError::new(format!(
                            "This server does not keep a project's {field}, so project \
                             '{project_id}' was left as it was. Its title is the one thing this \
                             command can change here."
                        )));
                    }
                }
                let Some(title) = rename.title else {
                    return Err(CommandError::new(format!(
                        "{kind} asked for no change to project '{project_id}'. Send a title."
                    )));
                };
                Ok(Command::RenameProject {
                    title: a_title(title, "project", &project_id)?,
                    project_id,
                })
            }
            "project.delete" => {
                let delete: DeleteProjectPayload = read(payload, kind)?;
                Ok(Command::DeleteProject {
                    project_id: non_blank(delete.project_id, "projectId", kind)?,
                })
            }
            "thread.create" => {
                let create: CreateThreadPayload = read(payload, kind)?;
                // The thread first, so the modes below have a conversation to
                // name — the same order the two mode commands take.
                let thread_id = non_blank(create.thread_id, "threadId", kind)?;
                create.thread.modes_the_contract_names(&thread_id)?;
                Ok(Command::CreateThread(CreateThread {
                    thread_id,
                    thread: ThreadFields {
                        project_id: non_blank(create.thread.project_id, "projectId", kind)?,
                        ..create.thread
                    },
                }))
            }
            "thread.meta.update" => {
                let UpdateThreadMetaPayload {
                    thread_id,
                    title,
                    regenerate_title,
                    command_id,
                    model_selection,
                    branch,
                    worktree_path,
                } = read_about_a_thread(payload, kind)?;
                // The thread first, so every refusal below has a conversation to
                // name — the same order the two mode commands take, and for the
                // same reason.
                let thread_id = non_blank(thread_id, "threadId", kind)?;
                if regenerate_title && title.is_some() {
                    return Err(CommandError::new(format!(
                        "{kind} cannot rename thread '{thread_id}' and regenerate its title in the same command."
                    )));
                }
                // A command carrying none of the four is refused, because the
                // event describing it would name nothing: a `thread.meta-updated`
                // whose payload is a `threadId` and a timestamp says a
                // conversation was updated while naming no part of it that was.
                // The client folds that as an update, so it would be an event
                // asserting a change nobody could point at.
                if title.is_none()
                    && !regenerate_title
                    && model_selection.is_none()
                    && branch.is_none()
                    && worktree_path.is_none()
                {
                    return Err(CommandError::new(format!(
                        "{kind} asked for no change to thread '{thread_id}'. Send a title, a model \
                         selection, a branch or a worktree path."
                    )));
                }
                // An object or nothing, for [`RespondToUserInput::answers`]'s
                // reason turned up one notch: the selection is published as part
                // of the thread, so one that is not an object fails the client's
                // decode of the *whole conversation* rather than of this write.
                //
                // The shape and not the contents. `ModelSelection` also names an
                // instance and a model, and neither is checked here — `{}` is
                // stored and would fail that decode too. `thread.create` accepts
                // the same field with no check at all, so tightening one without
                // the other would leave the looser door open; both together are a
                // ticket of their own.
                if model_selection
                    .as_ref()
                    .is_some_and(|selection| !selection.is_object())
                {
                    return Err(CommandError::new(format!(
                        "A model selection is an object, so thread '{thread_id}' was left as it \
                         was."
                    )));
                }
                Ok(Command::UpdateThreadMeta(UpdateThreadMetaPayload {
                    title: title
                        .map(|title| a_title(title, "thread", &thread_id))
                        .transpose()?,
                    model_selection,
                    regenerate_title,
                    command_id,
                    branch: cleared_or_named(branch, "branch", &thread_id)?,
                    worktree_path: cleared_or_named(worktree_path, "worktree path", &thread_id)?,
                    thread_id,
                }))
            }
            // A conversation and nothing else, as `thread.session.stop` is:
            // which conversation is the only thing either command carries that
            // could be wrong. Whether it is *already* on the shelf being asked
            // for is the world's question and is [`Shell::set_archived`]'s.
            //
            // One arm for both, because the payloads are one payload and two arms
            // would be two places to forget the blank check.
            "thread.archive" | "thread.unarchive" => {
                let about: AboutAThread = read_about_a_thread(payload, kind)?;
                let thread_id = non_blank(about.thread_id, "threadId", kind)?;
                Ok(match kind {
                    "thread.archive" => Command::Archive { thread_id },
                    _ => Command::Unarchive { thread_id },
                })
            }
            "thread.pin" => {
                let pin: PinPayload = read_about_a_thread(payload, kind)?;
                Ok(Command::Pin {
                    thread_id: non_blank(pin.about.thread_id, "threadId", kind)?,
                    order_key: pin
                        .order_key
                        .map(|key| non_blank(key, "orderKey", kind))
                        .transpose()?,
                })
            }
            "thread.unpin" => {
                let about: AboutAThread = read_about_a_thread(payload, kind)?;
                Ok(Command::Unpin {
                    thread_id: non_blank(about.thread_id, "threadId", kind)?,
                })
            }
            "thread.pin.reorder" => {
                let reorder: ReorderPinPayload = read_about_a_thread(payload, kind)?;
                Ok(Command::ReorderPin {
                    thread_id: non_blank(reorder.about.thread_id, "threadId", kind)?,
                    order_key: non_blank(reorder.order_key, "orderKey", kind)?,
                })
            }
            // A conversation and nothing else, as the archive commands are.
            // Whether it is already settled is deliberately *not* asked here: a
            // repeat re-emits rather than being refused, which is the one way
            // these two differ from the pair above — see
            // [`crate::threads::Change::re_emitted_at`].
            "thread.settle" => {
                let about: AboutAThread = read_about_a_thread(payload, kind)?;
                Ok(Command::Settle {
                    thread_id: non_blank(about.thread_id, "threadId", kind)?,
                })
            }
            // One field more, and it is the field that cannot be got wrong
            // quietly. `ThreadUnsettleCommand.reason` is the single literal
            // `user`, while the *event* carries a union of two — because the
            // neutral reset belongs to the server and a client that could send
            // `activity` could forge it. So a reason this contract does not name
            // is refused at the door rather than pinned as though it said `user`.
            "thread.unsettle" => {
                let unsettle: WithAReason = read_about_a_thread(payload, kind)?;
                let thread_id = non_blank(unsettle.about.thread_id, "threadId", kind)?;
                named_by_the_contract(
                    &unsettle.reason,
                    &UNSETTLE_REASONS,
                    "reason for unsettling",
                    &thread_id,
                )?;
                Ok(Command::Unsettle { thread_id })
            }
            // One field more again, and this one is judged rather than checked
            // against a list: a wake time has to be *ahead of now*, and a
            // conversation snoozed until a moment that has already passed would be
            // snoozed and awake at once, carrying state it can never leave.
            //
            // Decided here rather than beside the world's refusals because it is a
            // question about the payload and the clock alone — see
            // [`a_moment_still_ahead`], where an unparseable time taking the same
            // branch is argued.
            "thread.snooze" => {
                let snooze: Snooze = read_about_a_thread(payload, kind)?;
                let thread_id = non_blank(snooze.about.thread_id, "threadId", kind)?;
                a_moment_still_ahead(&snooze.snoozed_until, &thread_id)?;
                Ok(Command::Snooze {
                    thread_id,
                    until: snooze.snoozed_until,
                })
            }
            // `thread.unsettle`'s shape and its rule: the *event* carries two
            // reasons and the command carries one, because the neutral wake is
            // this server's own and a client that could send `activity` could wake
            // a conversation the developer had put to sleep and have it read as
            // their own doing.
            "thread.unsnooze" => {
                let unsnooze: WithAReason = read_about_a_thread(payload, kind)?;
                let thread_id = non_blank(unsnooze.about.thread_id, "threadId", kind)?;
                named_by_the_contract(
                    &unsnooze.reason,
                    &UNSNOOZE_REASONS,
                    "reason for waking",
                    &thread_id,
                )?;
                Ok(Command::Unsnooze { thread_id })
            }
            // A conversation and nothing else, as the archive commands are.
            // Which conversation is the only thing this command carries that
            // could be wrong; whether it has *already* been deleted is the
            // world's question and is [`Shell::delete`]'s, decided under the
            // fold's own lock.
            "thread.delete" => {
                let about: AboutAThread = read_about_a_thread(payload, kind)?;
                Ok(Command::Delete {
                    thread_id: non_blank(about.thread_id, "threadId", kind)?,
                })
            }
            // A turn carries a mode through *two* doors: the per-turn override
            // the composer sends beside every message, and the thread it asks to
            // have created. Both are checked, because both are written onto the
            // thread and published — see [`Shell::start_turn`].
            //
            // The overrides are checked only where one arrived, which is what
            // keeps absent meaning "leave the thread's alone" — see
            // [`StartTurn`], where that is why the fields have no default.
            "thread.turn.start" => {
                let start: StartTurn = read(payload, kind)?;
                let thread_id = non_blank(start.thread_id, "threadId", kind)?;
                if let Some(runtime_mode) = &start.runtime_mode {
                    named_by_the_contract(runtime_mode, &RUNTIME_MODES, "runtime mode", &thread_id)?;
                }
                if let Some(interaction_mode) = &start.interaction_mode {
                    named_by_the_contract(
                        interaction_mode,
                        &INTERACTION_MODES,
                        "interaction mode",
                        &thread_id,
                    )?;
                }
                if let Some(create) = start
                    .bootstrap
                    .as_ref()
                    .and_then(|bootstrap| bootstrap.create_thread.as_ref())
                {
                    create.modes_the_contract_names(&thread_id)?;
                }
                Ok(Command::StartTurn(Box::new(StartTurn {
                    thread_id,
                    message: TurnMessage {
                        message_id: non_blank(start.message.message_id, "messageId", kind)?,
                        ..start.message
                    },
                    ..start
                })))
            }
            "thread.turn.interrupt" => {
                let interrupt: InterruptTurn = read(payload, kind)?;
                Ok(Command::InterruptTurn(InterruptTurn {
                    thread_id: non_blank(interrupt.thread_id, "threadId", kind)?,
                    // Refused rather than dropped, unlike an *absent* one. Absent
                    // means "whatever is running" and is what the client sends
                    // when it believes nothing is; blank is a `TurnId` the
                    // contract types as trimmed and non-empty, and it would name
                    // no turn — so the stop would silently do nothing rather than
                    // stop the turn the developer is watching.
                    turn_id: interrupt
                        .turn_id
                        .map(|turn_id| non_blank(turn_id, "turnId", kind))
                        .transpose()?,
                }))
            }
            "thread.approval.respond" => {
                let respond: RespondToApproval = read(payload, kind)?;
                Ok(Command::RespondToApproval(RespondToApproval {
                    thread_id: non_blank(respond.thread_id, "threadId", kind)?,
                    request_id: non_blank(respond.request_id, "requestId", kind)?,
                    ..respond
                }))
            }
            "thread.user-input.respond" => {
                let respond: RespondToUserInput = read(payload, kind)?;
                // An object or nothing. The CLI reads these by key, so a list or
                // a string would reach it as answers with no questions attached
                // — which it drops silently, leaving an agent that asked a
                // question and was told nothing.
                if !respond.answers.is_object() {
                    return Err(CommandError::new(format!(
                        "{kind} needs answers, as an object keyed by question."
                    )));
                }
                Ok(Command::RespondToUserInput(RespondToUserInput {
                    thread_id: non_blank(respond.thread_id, "threadId", kind)?,
                    request_id: non_blank(respond.request_id, "requestId", kind)?,
                    answers: respond.answers,
                    rejected: false,
                }))
            }
            "thread.user-input.reject" => {
                let reject: RespondToUserInput = read(payload, kind)?;
                Ok(Command::RespondToUserInput(RespondToUserInput {
                    thread_id: non_blank(reject.thread_id, "threadId", kind)?,
                    request_id: non_blank(reject.request_id, "requestId", kind)?,
                    answers: serde_json::json!({}),
                    rejected: true,
                }))
            }
            "thread.runtime-mode.set" => {
                let set: SetRuntimeModePayload = read_about_a_thread(payload, kind)?;
                // The thread first, so the mode's refusal has a conversation to
                // name — and so a payload with two things wrong with it is
                // refused for the one the developer can act on.
                let thread_id = non_blank(set.thread_id, "threadId", kind)?;
                named_by_the_contract(
                    &set.runtime_mode,
                    &RUNTIME_MODES,
                    "runtime mode",
                    &thread_id,
                )?;
                Ok(Command::SetRuntimeMode(SetRuntimeModePayload {
                    runtime_mode: set.runtime_mode,
                    thread_id,
                }))
            }
            "thread.interaction-mode.set" => {
                let set: SetInteractionModePayload = read_about_a_thread(payload, kind)?;
                let thread_id = non_blank(set.thread_id, "threadId", kind)?;
                named_by_the_contract(
                    &set.interaction_mode,
                    &INTERACTION_MODES,
                    "interaction mode",
                    &thread_id,
                )?;
                Ok(Command::SetInteractionMode(SetInteractionModePayload {
                    interaction_mode: set.interaction_mode,
                    thread_id,
                }))
            }
            // The thread first, as the two mode commands do it: a payload with
            // two things wrong with it is refused for the one the developer can
            // act on, and every refusal after this has a conversation to name.
            "thread.checkpoint.revert" => {
                let revert: RevertCheckpointPayload = read_about_a_thread(payload, kind)?;
                Ok(Command::RevertCheckpoint(RevertCheckpointPayload {
                    thread_id: non_blank(revert.thread_id, "threadId", kind)?,
                    ..revert
                }))
            }
            // A conversation and nothing else. Which conversation is the only
            // thing this command can carry that could be wrong, so a blank one is
            // the whole of the payload check — whether the named conversation
            // exists, and whether anything is running behind it, are the world's
            // questions and are [`Shell::stop_session`]'s.
            "thread.session.stop" => {
                let stop: AboutAThread = read_about_a_thread(payload, kind)?;
                Ok(Command::StopSession {
                    thread_id: non_blank(stop.thread_id, "threadId", kind)?,
                })
            }
            unimplemented => Err(CommandError::new(format!(
                "Command not implemented by this server: {unimplemented}"
            ))),
        }
    }

    /// The conversation this command needs to still be there, if it names one.
    ///
    /// [`Shell::dispatch`] asks this of every command and refuses the ones aimed
    /// at a conversation the developer deleted. One question in one place, for
    /// the reason the guard exists at all: a deleted conversation leaves the
    /// developer's lists and stops taking commands, and a rule spread over
    /// nineteen dispatch arms is a rule the twentieth forgets.
    ///
    /// **`thread.delete` itself is deliberately not covered**, and that is the
    /// whole of the exception. Whether a conversation is *already* deleted is a
    /// question about the very field the change is about to move, so it is
    /// answered under the fold's own lock ([`Shell::delete`]) — asking it here
    /// and answering it there would let two windows both be told they deleted one
    /// conversation. It is the same argument [`Shell::set_archived`] makes, and
    /// the sentence it produces is the specific one.
    ///
    /// The three project commands name no conversation. `thread.create` does, and
    /// is covered: an id the developer deleted is not one a client may quietly
    /// reuse for a new conversation, and the refusal it would otherwise get —
    /// that the thread already exists — describes a thread that is no longer on
    /// any list.
    ///
    /// **The guard is read before the lock the change is folded under**, unlike
    /// the delete's own refusal, so a command dispatched in the instant a delete
    /// is committing can still get through. That is deliberate rather than
    /// overlooked: the losing command lands on a conversation that has just left
    /// both lists and is already refused everything afterwards, and none of the
    /// nineteen destroys anything — the one that is a *decision* about the same
    /// field is `thread.delete`, and it is the one decided under the lock.
    /// Moving this guard inside the fold would mean nineteen refusal closures
    /// where there is now one question.
    fn over_a_living_thread(&self) -> Option<&str> {
        match self {
            Command::CreateProject(_)
            | Command::RenameProject { .. }
            | Command::DeleteProject { .. }
            | Command::Delete { .. } => None,
            Command::CreateThread(create) => Some(&create.thread_id),
            Command::UpdateThreadMeta(update) => Some(&update.thread_id),
            Command::Archive { thread_id }
            | Command::Unarchive { thread_id }
            | Command::Pin { thread_id, .. }
            | Command::Unpin { thread_id }
            | Command::ReorderPin { thread_id, .. }
            | Command::Settle { thread_id }
            | Command::Unsettle { thread_id }
            | Command::Snooze { thread_id, .. }
            | Command::Unsnooze { thread_id }
            | Command::StopSession { thread_id } => Some(thread_id),
            Command::StartTurn(start) => Some(&start.thread_id),
            Command::InterruptTurn(interrupt) => Some(&interrupt.thread_id),
            Command::RespondToApproval(respond) => Some(&respond.thread_id),
            Command::RespondToUserInput(respond) => Some(&respond.thread_id),
            Command::SetRuntimeMode(set) => Some(&set.thread_id),
            Command::SetInteractionMode(set) => Some(&set.thread_id),
            Command::RevertCheckpoint(revert) => Some(&revert.thread_id),
        }
    }
}

fn read<T: serde::de::DeserializeOwned>(payload: &Value, kind: &str) -> Result<T, CommandError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| CommandError::new(format!("{kind} is malformed: {error}")))
}

/// [`read`], with the conversation the payload was about named in the refusal.
///
/// serde says which *field* it could not read and has no idea which thread the
/// command was for, so the sentence a client shows for a malformed payload is
/// otherwise the only refusal on this wire that does not say what it applies to.
/// The `threadId` is taken out of the raw payload rather than out of the struct,
/// because the struct is what failed to build.
///
/// Silent when the payload carries no readable `threadId` — that is the one case
/// where there is no thread to name, and it is exactly the case
/// [`non_blank`] would refuse next anyway.
fn read_about_a_thread<T: serde::de::DeserializeOwned>(
    payload: &Value,
    kind: &str,
) -> Result<T, CommandError> {
    read(payload, kind).map_err(|refusal| {
        match payload.get("threadId").and_then(Value::as_str) {
            Some(thread_id) if !thread_id.trim().is_empty() => CommandError::new(format!(
                "{} Thread '{thread_id}' was left as it was.",
                refusal.message()
            )),
            _ => refusal,
        }
    })
}

/// The contract types every identifier as a trimmed non-empty string. A blank
/// one would register a project nothing could later name, so it is refused at
/// the door rather than stored and puzzled over.
fn non_blank(value: String, field: &str, kind: &str) -> Result<String, CommandError> {
    if value.trim().is_empty() {
        return Err(CommandError::new(format!("{kind} needs a {field}.")));
    }
    Ok(value)
}

/// A name worth having, trimmed — or a refusal naming what stayed as it was.
///
/// The contract types every title as trimmed and non-empty, and the two rename
/// controls already refuse a blank one before dispatching. This is the same rule
/// where it is authoritative rather than convenient: a thread or a project called
/// "" is not a smaller thing than a named one, it is a row the developer cannot
/// pick out of a list.
///
/// Stored trimmed, which is what [`Shell::create_project`] does with the title on
/// a creation — the surrounding whitespace is the client's, and keeping it would
/// sort the list by something invisible.
///
/// `subject` is the word for the thing, because the sentence is the whole
/// diagnostic the UI can show and "a title cannot be blank" without saying whose
/// title is not something a developer with two windows open can act on.
fn a_title(value: String, subject: &str, id: &str) -> Result<String, CommandError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CommandError::new(format!(
            "A title cannot be blank, so {subject} '{id}' was left as it was."
        )));
    }
    Ok(trimmed.to_string())
}

/// A field the client either clears or names, never blanks.
///
/// `null` and `""` are different answers on `thread.meta.update`: the composer
/// clears a worktree path by sending `null` (`ChatView.logic.ts`), while a blank
/// branch is a ref name that names nothing. The contract types both fields as
/// trimmed and non-empty *or* null, so `""` is a third state neither side has a
/// meaning for — and the client has one place to show it, the branch toolbar,
/// where "on branch ''" is worse than either a name or nothing.
///
/// Nothing checks it beyond being non-blank, and that is deliberate rather than
/// missing. `branch` is carried and never acted on, and a `worktreePath` is a
/// folder rather than an argument — [`where_the_work_happens`] hands it to the
/// agent and to git as a working directory, where a path that is not there fails
/// by name and a path that is there needs no permission from this function. So
/// this is not the guard [`crate::refs`] would owe a ref name it was about to
/// interpolate; it is the contract's own rule about a spelling, enforced on the
/// one command that carries it.
///
/// **That command is `thread.meta.update` and only that one.** A `thread.create`
/// — or the bootstrap inside a first turn — does not come through here, so a
/// client that sent `worktreePath: ""` on one would have a conversation whose
/// work happens nowhere. No client sends it and the contract forbids it, and
/// what such a conversation gets is a turn that refuses — a blank working
/// directory is `NotFound` to the operating system, so the agent never starts
/// and the refusal names the folder — rather than one that quietly runs in the
/// project's folder as if the field had been absent. That is the failure worth
/// having, so this stays a spelling rule rather than growing a second door. It
/// is named here because the paragraph above would otherwise read as a guarantee
/// about both.
///
/// Absent and `null` are both passed through; only the blank string is refused.
fn cleared_or_named(
    value: Given<String>,
    field: &str,
    thread_id: &str,
) -> Result<Given<String>, CommandError> {
    match value {
        Some(Some(given)) => {
            let trimmed = given.trim();
            if trimmed.is_empty() {
                return Err(CommandError::new(format!(
                    "A blank {field} names nothing; send null to clear it. Thread '{thread_id}' \
                     was left as it was."
                )));
            }
            Ok(Some(Some(trimmed.to_string())))
        }
        absent_or_cleared => Ok(absent_or_cleared),
    }
}

/// One value out of a closed set the contract spells out — or a refusal naming
/// what was sent and which conversation it was sent about.
///
/// Checks rather than hands the value back, because two of the four doors a mode
/// arrives through carry theirs inside a [`ThreadFields`], where taking the
/// string out to return it would mean rebuilding the struct around it.
///
/// **Refused rather than rounded to the nearest mode this server understands.**
/// A declined setting, in ADR-0009's sense — a value refused on the way in — and
/// the shape is [`crate::settings`]'s `ENV_MODES` rather than anything new: a
/// closed array of the contract's own literals, and a sentence that lists them.
/// The reason to refuse rather than round is [`Shell::respond_to_approval`]'s:
/// the nearest mode this server has a `--permission-mode` for is `full-access`,
/// so rounding a typo would *widen* what the agent may do.
///
/// A blank value is refused here as well as an unknown one — it names no mode,
/// and the contract's own literals are what the picker renders.
///
/// The accepted values are in the message because the sentence is the whole
/// diagnostic `OrchestrationDispatchCommandError` can carry, and "not a runtime
/// mode" without saying what one is is not something a developer can act on.
/// Joined with `", "` rather than `settings`'s `" or "`, because four is a list
/// and two is a choice.
fn named_by_the_contract(
    value: &str,
    named: &[&str],
    what: &str,
    thread_id: &str,
) -> Result<(), CommandError> {
    if named.contains(&value) {
        return Ok(());
    }
    Err(CommandError::new(format!(
        "'{value}' is not a {what} this contract names, so thread '{thread_id}' was left as it \
         was. Send one of: {}.",
        named.join(", ")
    )))
}

/// Why a conversation cannot be put out of the developer's sight, as the
/// sentence they are shown.
///
/// One function for [`Shell::settle`] and [`Shell::snooze`] rather than a cascade
/// apiece, because the two differed by a gerund and nothing else — and the copy
/// would have had to carry a `Busy::Session` arm that [`Attention::Snoozing`]
/// cannot produce, which is a sentence written to be unreachable. Here every arm
/// is reached, by one caller or the other.
///
/// **The order the blocker was chosen in is [`Thread::busy`]'s**, and it is the
/// client's. This only turns the answer into words: an agent that has asked for
/// permission is also running, and which of those two facts is worth saying was
/// decided before this was called.
///
/// The sentence names the conversation as well as the reason, because
/// `OrchestrationDispatchCommandError` carries nothing else machine-readable and
/// a developer with two windows open cannot act on a sentence that does not say
/// which one it is about.
fn would_hide(thread_id: &str, busy: Busy, about: Attention) -> String {
    let doing = match about {
        Attention::Settling => "settling",
        Attention::Snoozing => "snoozing",
    };
    match busy {
        Busy::Approval => format!(
            "Conversation '{thread_id}' is waiting for a permission decision, so {doing} it would \
             hide a request that is waiting on you."
        ),
        Busy::Question => format!(
            "Conversation '{thread_id}' is waiting for an answer to a question, so {doing} it \
             would hide a request that is waiting on you."
        ),
        Busy::Session => format!(
            "Conversation '{thread_id}' has an agent still working, so {doing} it would hide work \
             in progress."
        ),
        Busy::QueuedTurn => format!(
            "Conversation '{thread_id}' has a turn no agent has picked up yet, so {doing} it would \
             hide work about to start."
        ),
    }
}

/// A wake time worth storing: one this server can place on its own clock, and
/// strictly ahead of the instant it is read at.
///
/// [`named_by_the_contract`]'s neighbour and the one field that cannot be
/// checked the same way: a wake time is not a value out of a closed set, so what
/// makes one acceptable is arithmetic rather than membership.
///
/// **Refused rather than quietly normalised.** A conversation snoozed until a
/// moment that has already passed is snoozed and awake at once — the client's
/// `effectiveSnoozed` stops classifying it the instant it reads the field — so it
/// would carry snooze state it can never leave and a "Woke" indicator for a wake
/// nobody chose. Clamping it to now would be the same conversation with the
/// developer told it worked.
///
/// **Strictly ahead**, so a wake time equal to the instant this reads is refused
/// with the past ones: it has already elapsed by the time anything else reads it,
/// and the conversation it would produce is exactly the one above. The clock is
/// read *here* rather than passed in, so "equal to now" is a case that cannot be
/// reached from a socket — a client's `now` is always a little older than this
/// one by the time it arrives. `a_wake_time_must_be_ahead_of_the_instant_it_is_read_at`
/// is where the comparison itself is pinned, because that is the only place the
/// two instants can be made the same one.
///
/// **An unparseable time is refused by the same guard and named by its own
/// sentence.** One check, because a string this server cannot place on a clock
/// is not one it can call future either; two sentences, because "that moment has
/// passed" is a lie about a time this server simply does not read, and the
/// sentence *is* the whole diagnostic. Refusing it is what keeps it out of a
/// field the contract types as an `IsoDateTime` — see
/// [`crate::clock::epoch_millis_from_iso`], where what counts as one is decided,
/// and where the one shape this wire renders is argued.
///
/// Both sentences name the time as well as the conversation, because a snooze is
/// sent from a preset menu (`Sidebar.snooze.ts`) and "that time will not do"
/// without saying which time is not something a developer can act on.
fn a_moment_still_ahead(until: &str, thread_id: &str) -> Result<(), CommandError> {
    match wake_time(until, crate::clock::now_epoch_millis()) {
        Ok(()) => Ok(()),
        Err(Unusable::Unreadable) => Err(CommandError::new(format!(
            "'{until}' is not a time this server can read, so conversation '{thread_id}' was left \
             awake. Send one like '2026-07-31T09:00:00.000Z'."
        ))),
        Err(Unusable::Elapsed) => Err(CommandError::new(format!(
            "'{until}' is not a moment still to come, so conversation '{thread_id}' was left \
             awake. Send a wake time in the future."
        ))),
    }
}

/// What is wrong with a wake time, if anything is.
///
/// Two, because they want different sentences — see [`a_moment_still_ahead`],
/// which is the only thing that turns one into words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unusable {
    /// Not one of this wire's renderings, so not a moment at all.
    Unreadable,
    /// A real moment, and not one still to come.
    Elapsed,
}

/// [`a_moment_still_ahead`]'s judgement, against an instant it can be given.
///
/// Split out for [`crate::clock::iso_from_epoch`]'s reason — a comparison that
/// cannot be told what "now" is is a comparison whose boundary cannot be tested,
/// and the boundary is the whole of what "strictly future" means. It is not
/// reachable through a socket: a client samples its clock, sends, and this server
/// reads its own afterwards, so a wake time of "now" has always already elapsed
/// by the time the guard sees it.
fn wake_time(until: &str, now: u64) -> Result<(), Unusable> {
    let wake = crate::clock::epoch_millis_from_iso(until).ok_or(Unusable::Unreadable)?;
    (wake > now).then_some(()).ok_or(Unusable::Elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        shell: Shell,
        /// The registry's neighbour rather than its property: a command is
        /// dispatched against both, because deleting a project releases what
        /// the index holds for it.
        index: Index,
        /// Dispatch reads one thing out of the configuration — which binary to
        /// start an agent with — so the tests that never start one still have to
        /// supply it.
        config: ServerConfig,
        directory: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            let directory = tempfile::tempdir().expect("a temporary directory");

            // **The agent this fixture cannot start.** A turn dispatched here
            // reaches `session::send`, which resolves `binaryPath` for real — so
            // the default `claude` would find the developer's own install and
            // run a real turn against the real API. That is what spec story 61
            // and the ticket forbid, and the default is a bare name precisely
            // because a bare name is looked up on `PATH`.
            //
            // A file that *exists and is not a program* is the one unusable case
            // that does not fall back to `PATH` (see `provider::resolve`, where
            // the asymmetry is deliberate), so it is the only configuration that
            // is deterministically offline. `socket_turn.rs` drives real turns,
            // against a scripted stand-in.
            let unusable = directory.path().join("not-an-agent.txt");
            std::fs::write(&unusable, "not a program").expect("writes the file");
            let mut config = ServerConfig::detect();
            config.settings.providers.claude_agent.binary_path =
                unusable.to_string_lossy().into_owned();

            Fixture {
                shell: Shell::new(Database::in_memory().expect("an in-memory database")),
                index: Index::new(),
                config,
                directory,
            }
        }

        fn folder(&self, name: &str) -> String {
            let path = self.directory.path().join(name);
            std::fs::create_dir_all(&path).expect("creates the folder");
            path.to_string_lossy().into_owned()
        }

        fn dispatch(&self, command: &Value) -> Result<Value, CommandError> {
            self.shell.dispatch(command, &self.index, &self.config)
        }

        /// The captured `project.create` payload, with the folder swapped.
        fn add(&self, id: &str, folder: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "project.create",
                "commandId": format!("test:create:{id}"),
                "projectId": id,
                "title": "",
                "workspaceRoot": folder,
                "createWorkspaceRootIfMissing": true,
                "defaultModelSelection": Value::Null,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
        }

        fn remove(&self, id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "project.delete",
                "commandId": format!("test:delete:{id}"),
                "projectId": id,
            }))
        }

        /// The `thread.create` payload the client-runtime's `createThread`
        /// builds.
        fn add_thread(&self, id: &str, project_id: &str) -> Result<Value, CommandError> {
            self.add_thread_with(id, project_id, json!({}))
        }

        /// The same, with whichever of the thread's fields the caller means to
        /// move off the value the client-runtime would have sent.
        ///
        /// Merged into the envelope rather than taken as arguments, for
        /// [`Fixture::update_thread_meta`]'s reason turned one notch: the two
        /// modes are separate vocabularies that are both bare strings, so an
        /// argument list would let a test swap them and still compile.
        fn add_thread_with(
            &self,
            id: &str,
            project_id: &str,
            fields: Value,
        ) -> Result<Value, CommandError> {
            let mut command = json!({
                "type": "thread.create",
                "commandId": format!("test:thread:{id}"),
                "threadId": id,
                "projectId": project_id,
                "title": "A conversation",
                "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
                "runtimeMode": "full-access",
                "interactionMode": "default",
                "branch": Value::Null,
                "worktreePath": Value::Null,
                "createdAt": "2026-07-26T00:23:04.909Z",
            });
            let envelope = command.as_object_mut().expect("the envelope is an object");
            for (field, value) in fields.as_object().expect("the fields are an object") {
                envelope.insert(field.clone(), value.clone());
            }
            self.dispatch(&command)
        }

        /// The `thread.turn.start` the composer sends, carrying whichever of the
        /// per-turn fields the caller means to test.
        ///
        /// Merged into the envelope for [`Fixture::update_thread_meta`]'s
        /// reason: *which fields are present* is the whole question, because an
        /// absent mode means "leave the thread's alone" and an argument list
        /// would have to spell absent and blank differently.
        ///
        /// Every caller here expects a refusal. A turn this fixture lets through
        /// reaches `session::send`, which wants a runtime and an agent to start —
        /// see [`Fixture::new`], and `tests/socket_turn.rs` for the turns that
        /// really run.
        fn start_turn(&self, thread_id: &str, fields: Value) -> Result<Value, CommandError> {
            let mut command = json!({
                "type": "thread.turn.start",
                "commandId": format!("test:turn:{thread_id}"),
                "threadId": thread_id,
                "message": {
                    "messageId": "message-1",
                    "role": "user",
                    "text": "hello",
                    "attachments": [],
                },
                "createdAt": "2026-07-26T00:23:04.909Z",
            });
            let envelope = command.as_object_mut().expect("the envelope is an object");
            for (field, value) in fields.as_object().expect("the fields are an object") {
                envelope.insert(field.clone(), value.clone());
            }
            self.dispatch(&command)
        }

        /// The `thread.meta.update` `updateThreadMetadata` builds, carrying
        /// whichever of the four fields the caller means to move.
        ///
        /// Merged into the envelope rather than taken as four arguments, because
        /// *which fields are present* is the whole question this command turns on
        /// — an argument list would have to spell absent and null differently and
        /// would then be testing the helper rather than the parse.
        fn update_thread_meta(
            &self,
            thread_id: &str,
            fields: Value,
        ) -> Result<Value, CommandError> {
            let mut command = json!({
                "type": "thread.meta.update",
                "commandId": format!("test:meta:{thread_id}"),
                "threadId": thread_id,
            });
            let envelope = command.as_object_mut().expect("the envelope is an object");
            for (field, value) in fields.as_object().expect("the fields are an object") {
                envelope.insert(field.clone(), value.clone());
            }
            self.dispatch(&command)
        }

        /// The sidebar's thread rename, which is a `thread.meta.update` carrying
        /// nothing but a title.
        fn rename_thread(&self, thread_id: &str, title: &str) -> Result<Value, CommandError> {
            self.update_thread_meta(thread_id, json!({"title": title}))
        }

        /// The sidebar's project rename dialog.
        fn rename_project(&self, project_id: &str, title: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "project.meta.update",
                "commandId": format!("test:project-meta:{project_id}"),
                "projectId": project_id,
                "title": title,
            }))
        }

        /// The `thread.runtime-mode.set` the composer's runtime picker sends.
        fn set_runtime_mode(&self, thread_id: &str, mode: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.runtime-mode.set",
                "commandId": format!("test:runtime-mode:{thread_id}"),
                "threadId": thread_id,
                "runtimeMode": mode,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
        }

        /// The `thread.interaction-mode.set` the composer's plan-mode toggle
        /// sends.
        fn set_interaction_mode(&self, thread_id: &str, mode: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.interaction-mode.set",
                "commandId": format!("test:interaction-mode:{thread_id}"),
                "threadId": thread_id,
                "interactionMode": mode,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
        }

        /// The `thread.session.stop` `stopThreadSession` builds.
        fn stop_session(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.session.stop",
                "commandId": format!("test:stop:{thread_id}"),
                "threadId": thread_id,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
        }

        /// The `thread.archive` the sidebar's context menu sends, and its twin.
        fn archive(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.archive",
                "commandId": format!("test:archive:{thread_id}"),
                "threadId": thread_id,
            }))
        }

        fn unarchive(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.unarchive",
                "commandId": format!("test:unarchive:{thread_id}"),
                "threadId": thread_id,
            }))
        }

        /// The `thread.settle` the sidebar's context menu sends.
        fn settle(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.settle",
                "commandId": format!("test:settle:{thread_id}"),
                "threadId": thread_id,
            }))
        }

        /// Its twin, carrying the one reason a client is allowed to give.
        fn unsettle(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.unsettle",
                "commandId": format!("test:unsettle:{thread_id}"),
                "threadId": thread_id,
                "reason": "user",
            }))
        }

        /// The `thread.snooze` the sidebar's snooze presets send.
        fn snooze(&self, thread_id: &str, until: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.snooze",
                "commandId": format!("test:snooze:{thread_id}"),
                "threadId": thread_id,
                "snoozedUntil": until,
            }))
        }

        /// Its twin — "wake it now", carrying the one reason a client may give.
        fn unsnooze(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.unsnooze",
                "commandId": format!("test:unsnooze:{thread_id}"),
                "threadId": thread_id,
                "reason": "user",
            }))
        }

        /// The `thread.delete` the sidebar's context menu sends, once the
        /// developer has answered its confirmation.
        fn delete_thread(&self, thread_id: &str) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.delete",
                "commandId": format!("test:delete-thread:{thread_id}"),
                "threadId": thread_id,
            }))
        }

        /// The `thread.checkpoint.revert` the diff panel's undo sends.
        fn revert(&self, thread_id: &str, turn_count: u64) -> Result<Value, CommandError> {
            self.dispatch(&json!({
                "type": "thread.checkpoint.revert",
                "commandId": format!("test:revert:{thread_id}:{turn_count}"),
                "threadId": thread_id,
                "turnCount": turn_count,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
        }

        /// A registry with one project and one conversation in it, which is the
        /// least the mode commands need to have something to act on.
        fn with_a_conversation() -> Fixture {
            let fixture = Fixture::new();
            let folder = fixture.folder("modes");
            fixture.add("project-1", &folder).expect("registered");
            fixture
                .add_thread("thread-1", "project-1")
                .expect("created");
            fixture
        }

        /// The conversation as its own subscription describes it — where the
        /// picker reads the mode in force.
        fn detail(&self, thread_id: &str) -> Value {
            self.shell
                .threads()
                .detail_snapshot(thread_id)
                .expect("the thread is held")["thread"]
                .clone()
        }

        fn snapshot(&self) -> Value {
            self.shell.snapshot().expect("the registry is readable")
        }

        fn listed(&self) -> Vec<Value> {
            self.snapshot()["snapshot"]["projects"]
                .as_array()
                .expect("an array of projects")
                .clone()
        }

        fn listed_threads(&self) -> Vec<Value> {
            self.snapshot()["snapshot"]["threads"]
                .as_array()
                .expect("an array of threads")
                .clone()
        }

        /// The conversations on the *other* shelf — what
        /// `orchestration.getArchivedShellSnapshot` answers with.
        fn archived_threads(&self) -> Vec<Value> {
            self.shell
                .archived_shell_snapshot()
                .expect("the registry is readable")["threads"]
                .as_array()
                .expect("an array of threads")
                .clone()
        }
    }

    /// The command payload verbatim in shape from
    /// `fixtures/socket-wire/05-orchestration-and-backpressure.ndjson`, answered
    /// with the shape the capture shows.
    #[test]
    fn the_captured_create_command_is_answered_with_a_sequence() {
        let fixture = Fixture::new();
        let folder = fixture.folder("wire-capture");

        let answer = fixture
            .add("6ee34f01-3d27-4719-8254-2e9c255e5586", &folder)
            .expect("an existing folder is registrable");

        assert_eq!(answer, json!({"sequence": 1}));
    }

    /// The snapshot is the project list, and it has to carry every key the
    /// contract declares — a missing one fails the client's decode, and the
    /// user then sees no projects at all rather than a slightly wrong one.
    #[test]
    fn the_snapshot_lists_the_registered_projects() {
        let fixture = Fixture::new();
        let folder = fixture.folder("my-project");
        fixture.add("project-1", &folder).expect("registered");

        let snapshot = fixture.snapshot();
        assert_eq!(snapshot["kind"], "snapshot");
        assert_eq!(snapshot["snapshot"]["snapshotSequence"], json!(1));
        assert_eq!(snapshot["snapshot"]["threads"], json!([]));
        assert!(snapshot["snapshot"]["updatedAt"].is_string());

        let projects = fixture.listed();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["id"], "project-1");
        assert_eq!(projects[0]["workspaceRoot"], folder);
        // The client sent a blank title, so the folder names the project.
        assert_eq!(projects[0]["title"], "my-project");
        assert_eq!(projects[0]["createdAt"], "2026-07-26T00:23:04.909Z");
    }

    /// Ticket 31's `GET /api/orchestration/shell` answers with exactly what the
    /// subscription's opening chunk carries under `snapshot`, envelope removed.
    /// The client takes whichever of the two it can get, so one builder is the
    /// only way the shell it draws cannot depend on which transport won.
    #[test]
    fn the_route_and_the_subscription_describe_the_shell_identically() {
        let fixture = Fixture::new();
        let folder = fixture.folder("both-ways");
        fixture.add("project-1", &folder).expect("registered");
        fixture
            .add_thread("thread-1", "project-1")
            .expect("registered");

        let over_http = fixture
            .shell
            .shell_snapshot()
            .expect("the registry is readable");
        assert_eq!(over_http, fixture.snapshot()["snapshot"]);
        assert_eq!(over_http["projects"][0]["id"], "project-1");
        assert_eq!(over_http["threads"][0]["id"], "thread-1");
    }

    #[test]
    fn an_empty_registry_still_describes_itself() {
        let snapshot = Fixture::new().snapshot();
        assert_eq!(snapshot["snapshot"]["snapshotSequence"], json!(0));
        assert_eq!(snapshot["snapshot"]["projects"], json!([]));
    }

    /// A create and a delete each publish one event, and the sequence in the
    /// event is the one the command answered with. The client joins them by
    /// that number, so a mismatch would make the change invisible.
    #[test]
    fn a_committed_command_announces_itself_at_the_sequence_it_answered_with() {
        let fixture = Fixture::new();
        let folder = fixture.folder("watched");
        let mut updates = fixture.shell.inner.updates.subscribe();

        let created = fixture.add("project-1", &folder).expect("registered");
        let event = updates.try_recv().expect("an announcement");
        assert_eq!(event["kind"], "project-upserted");
        assert_eq!(event["sequence"], created["sequence"]);
        assert_eq!(event["project"]["id"], "project-1");
        assert_eq!(event["project"]["workspaceRoot"], folder);

        let removed = fixture.remove("project-1").expect("removed");
        assert_eq!(
            updates.try_recv().expect("an announcement"),
            json!({
                "kind": "project-removed",
                "sequence": removed["sequence"],
                "projectId": "project-1",
            })
        );
    }

    /// Nothing happened, so nothing is announced. An event here would make
    /// every subscriber re-render for a command that changed no state.
    #[test]
    fn a_refused_or_empty_command_announces_nothing() {
        let fixture = Fixture::new();
        let folder = fixture.folder("only-once");
        fixture.add("project-1", &folder).expect("registered");

        let mut updates = fixture.shell.inner.updates.subscribe();
        fixture
            .add("project-2", &folder)
            .expect_err("the same folder twice");
        fixture.remove("never-registered").expect("a no-op delete");

        assert!(
            updates.try_recv().is_err(),
            "nothing changed, so nothing is announced"
        );
    }

    /// The declared divergence, pinned. The upstream UI sends this flag as
    /// `true` on every add; a server that obeyed it would create the folder and
    /// report success, which is the behaviour this project rejects.
    #[test]
    fn a_missing_folder_is_refused_even_when_the_client_asks_for_it_to_be_created() {
        let fixture = Fixture::new();
        let missing = fixture.directory.path().join("please-create-me");

        let refusal = fixture
            .dispatch(&json!({
                "type": "project.create",
                "commandId": "test:create:1",
                "projectId": "project-1",
                "title": "please-create-me",
                "workspaceRoot": missing.to_string_lossy(),
                "createWorkspaceRootIfMissing": true,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
            .expect_err("the flag is not honoured");

        assert!(
            refusal.message().contains("does not exist"),
            "{}",
            refusal.message()
        );
        assert!(
            !missing.exists(),
            "the server created a directory it was asked to refuse"
        );
    }

    /// A client's `createdAt` is kept when it is one and replaced when it is
    /// not. Nothing on either side of the wire validates this field, and it is
    /// stored forever and sorted on — so a client that sent nonsense once would
    /// otherwise misplace that project in the list on every run from now on.
    #[test]
    fn a_missing_or_nonsensical_timestamp_is_replaced_by_the_servers() {
        let fixture = Fixture::new();

        for (name, sent) in [
            ("undated", None),
            ("blank", Some("   ")),
            ("prose", Some("yesterday")),
            ("nearly", Some("2026-07-26")),
        ] {
            let folder = fixture.folder(name);
            let mut command = json!({
                "type": "project.create",
                "commandId": format!("test:create:{name}"),
                "projectId": name,
                "title": name,
                "workspaceRoot": folder,
            });
            if let Some(sent) = sent {
                command["createdAt"] = json!(sent);
            }
            fixture.dispatch(&command).expect("registered");
        }

        for project in fixture.listed() {
            let created_at = project["createdAt"].as_str().expect("a timestamp");
            assert_eq!(
                created_at.len(),
                24,
                "{} kept {created_at}",
                project["id"]
            );
            assert!(created_at.ends_with('Z'), "{created_at} is not ISO");
        }

        // And a well-formed one is the client's, untouched.
        let folder = fixture.folder("dated");
        fixture.add("dated", &folder).expect("registered");
        let dated = fixture
            .listed()
            .into_iter()
            .find(|project| project["id"] == "dated")
            .expect("the project is listed");
        assert_eq!(dated["createdAt"], "2026-07-26T00:23:04.909Z");
    }

    /// The marker says "the initial catch-up is over". It is owed only when
    /// asked for, and only once — a second one after a resynchronisation would
    /// describe an event that has already happened.
    #[test]
    fn the_completion_marker_is_sent_once_and_only_when_requested() {
        let fixture = Fixture::new();

        let requested = fixture
            .shell
            .subscribe(&json!({"requestCompletionMarker": true}));
        let opening = requested.describe();
        assert_eq!(opening.len(), 2, "{opening:#?}");
        assert_eq!(opening[0]["kind"], "snapshot");
        assert_eq!(opening[1], json!({"kind": "synchronized"}));

        let again = requested.describe();
        assert_eq!(again.len(), 1, "the marker is owed once: {again:#?}");
        assert_eq!(again[0]["kind"], "snapshot");

        let plain = fixture.shell.subscribe(&json!({}));
        let opening = plain.describe();
        assert_eq!(opening.len(), 1, "{opening:#?}");
        assert_eq!(opening[0]["kind"], "snapshot");
    }

    /// A client resuming from a cursor this server cannot replay from asks for
    /// a replay and is answered with the whole registry, which is a superset of
    /// what it asked for. The point of the test is that it is an answer rather
    /// than a refusal.
    #[test]
    fn a_resume_request_is_answered_with_a_snapshot() {
        let fixture = Fixture::new();
        let folder = fixture.folder("resumed");
        fixture.add("project-1", &folder).expect("registered");

        let resumed = fixture.shell.subscribe(&json!({"afterSequence": 0}));
        let opening = resumed.describe();
        assert_eq!(opening[0]["kind"], "snapshot");
        assert_eq!(
            opening[0]["snapshot"]["projects"]
                .as_array()
                .expect("an array")
                .len(),
            1
        );
    }

    /// The case ADR-0016 is about, and the one every laplus window is in at
    /// boot: `GET /api/orchestration/shell` answered a moment ago, nothing has
    /// happened since, and the subscription opens carrying no second copy of it.
    ///
    /// `shell_snapshot` is read here rather than a literal, because the cursor
    /// the real client sends is the one it read off that payload
    /// (`packages/client-runtime/src/state/shell.ts`) and the test should fail
    /// if the two ever stop being the same number.
    #[test]
    fn a_cursor_that_is_still_current_opens_without_a_snapshot() {
        let fixture = Fixture::new();
        let folder = fixture.folder("held");
        fixture.add("project-1", &folder).expect("registered");

        let over_http = fixture.shell.shell_snapshot().expect("a snapshot");
        let cursor = over_http["snapshotSequence"].as_i64().expect("a sequence");

        let opening = fixture
            .shell
            .subscribe(&json!({"afterSequence": cursor, "requestCompletionMarker": true}))
            .describe();
        assert_eq!(
            opening,
            vec![json!({"kind": "synchronized"})],
            "the registry travelled twice: {opening:#?}"
        );

        // Without a marker there is nothing to say at all, and an empty opening
        // is a thing this wire can carry — the pump sends no chunk for it.
        let silent = fixture
            .shell
            .subscribe(&json!({"afterSequence": cursor}))
            .describe();
        assert!(silent.is_empty(), "{silent:#?}");
    }

    /// A cursor ahead of this server's log is a client holding a number from a
    /// previous run: the counter resumes from the last durable write, so every
    /// number issued after it is handed out again. Upstream guards the same case
    /// with `replayGap < 0` (`apps/server/src/ws.ts`) and calls it invalid.
    ///
    /// It has to reset the client rather than reassure it, which is why
    /// `Sequences::caught_up` is equality and not "at least".
    #[test]
    fn a_cursor_from_a_previous_run_is_answered_with_a_snapshot() {
        let fixture = Fixture::new();
        let folder = fixture.folder("stale");
        fixture.add("project-1", &folder).expect("registered");
        let ahead = fixture.shell.inner.sequences.current() + 1_000;

        let opening = fixture
            .shell
            .subscribe(&json!({"afterSequence": ahead}))
            .describe();
        assert_eq!(opening[0]["kind"], "snapshot", "{opening:#?}");
    }

    /// The registry half of the invariant that lets the cursor be re-read rather
    /// than remembered; `threads::tests` pins the same thing for a conversation,
    /// and the rule is only sound if it holds on *both* feeds.
    ///
    /// `describe` runs again whenever a subscriber falls a whole backlog behind,
    /// and that second description has to be a snapshot even though the first
    /// one was skipped. Nothing tracks which call it is: every change to the
    /// registry takes a number from `Sequences`, so falling behind is *itself*
    /// what makes the cursor stale. This is what would fail if a registry event
    /// were ever published without taking one, or if the caught-up test were
    /// hoisted out of the closure and answered once.
    #[test]
    fn a_subscription_that_opened_caught_up_is_re_described_once_it_has_not() {
        let fixture = Fixture::new();
        let held = fixture.folder("held");
        fixture.add("project-1", &held).expect("registered");

        let cursor = fixture.shell.inner.sequences.current();
        let source = fixture
            .shell
            .subscribe(&json!({"afterSequence": cursor, "requestCompletionMarker": true}));
        assert_eq!(source.describe(), vec![json!({"kind": "synchronized"})]);

        let later = fixture.folder("registered-later");
        fixture.add("project-2", &later).expect("registered");

        let again = source.describe();
        assert_eq!(again[0]["kind"], "snapshot", "{again:#?}");
        assert_eq!(
            again[0]["snapshot"]["projects"]
                .as_array()
                .expect("the registry")
                .len(),
            2,
            "the whole registry, not the part that was missed: {again:#?}"
        );
    }

    /// Every command a client can dispatch is now answered, so what is left to
    /// refuse by name is a command type the *contract* does not offer a client —
    /// and a malformed payload for one it does. Each refusal has to name what was
    /// asked for, or a developer cannot tell what this server made of it.
    #[test]
    fn an_unimplemented_or_malformed_command_is_refused_by_name() {
        let fixture = Fixture::new();

        // A server-side command: `thread.session.set` is in the contract's
        // `OrchestrationCommand` and deliberately not in the union a client
        // dispatches from, because this server is what decides a session's
        // status. Ticket 10 of the thread-lifecycle effort took `thread.delete`
        // out of this test by answering it.
        let refusal = fixture
            .dispatch(&json!({"type": "thread.session.set", "commandId": "c", "threadId": "t"}))
            .expect_err("a client does not set a session");
        assert!(
            refusal.message().contains("thread.session.set"),
            "{}",
            refusal.message()
        );
        assert_eq!(
            refusal.to_error()["_tag"],
            "OrchestrationDispatchCommandError"
        );
        assert_eq!(refusal.to_error()["message"], refusal.message());

        let refusal = fixture
            .dispatch(&json!({}))
            .expect_err("no type at all");
        assert!(refusal.message().contains("type"), "{}", refusal.message());

        let refusal = fixture
            .dispatch(&json!({"type": "project.create", "projectId": "p"}))
            .expect_err("no workspace root");
        assert!(
            refusal.message().contains("project.create") && refusal.message().contains("malformed"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .dispatch(&json!({"type": "project.delete", "projectId": "   "}))
            .expect_err("a blank id");
        assert!(
            refusal.message().contains("projectId"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .dispatch(&json!({"type": "thread.create", "commandId": "c", "threadId": "t"}))
            .expect_err("a thread needs a project and a model");
        assert!(
            refusal.message().contains("thread.create") && refusal.message().contains("malformed"),
            "{}",
            refusal.message()
        );
    }

    // -- threads -------------------------------------------------------------

    /// A conversation needs a project that exists, because the whole of what a
    /// thread does is run an agent in that project's folder.
    #[test]
    fn a_thread_is_registered_against_a_project_and_joins_the_shell_snapshot() {
        let fixture = Fixture::new();
        let folder = fixture.folder("workspace");
        fixture.add("project-1", &folder).expect("registered");

        let answer = fixture
            .add_thread("thread-1", "project-1")
            .expect("the project is there");
        assert!(answer["sequence"].as_i64().expect("a sequence") > 1);

        let threads = fixture.listed_threads();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0]["id"], "thread-1");
        assert_eq!(threads[0]["projectId"], "project-1");
        assert_eq!(threads[0]["session"], Value::Null, "nothing is running yet");
        assert_eq!(threads[0]["latestTurn"], Value::Null);
    }

    /// A thread against nothing is a conversation that could never take a turn,
    /// and the message names the project so the developer knows which one is
    /// missing rather than that "something" is.
    #[test]
    fn a_thread_for_an_unregistered_project_is_refused_by_name() {
        let fixture = Fixture::new();

        let refusal = fixture
            .add_thread("thread-1", "never-registered")
            .expect_err("there is no such project");
        assert!(
            refusal.message().contains("never-registered"),
            "{}",
            refusal.message()
        );
        assert!(fixture.listed_threads().is_empty());
    }

    /// A blank title is the folder's, the same way a project's is. A
    /// conversation called "" would be unreachable in the thread list.
    #[test]
    fn a_thread_with_no_title_takes_the_projects() {
        let fixture = Fixture::new();
        let folder = fixture.folder("my-project");
        fixture.add("project-1", &folder).expect("registered");

        fixture
            .dispatch(&json!({
                "type": "thread.create",
                "commandId": "c",
                "threadId": "thread-1",
                "projectId": "project-1",
                "title": "  ",
                "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
                "runtimeMode": "full-access",
                "interactionMode": "default",
                "branch": Value::Null,
                "worktreePath": Value::Null,
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
            .expect("registered");

        assert_eq!(fixture.listed_threads()[0]["title"], "my-project");
    }

    /// The composer's own path: a new conversation reaches this server for the
    /// first time as a turn carrying the thread it wants created. A server that
    /// only implemented `thread.create` would answer the real UI's first message
    /// with "there is no such thread".
    #[tokio::test]
    async fn a_turn_creates_the_thread_it_was_sent_for() {
        let fixture = Fixture::new();
        let folder = fixture.folder("workspace");
        fixture.add("project-1", &folder).expect("registered");

        let answer = fixture
            .dispatch(&json!({
                "type": "thread.turn.start",
                "commandId": "c",
                "threadId": "thread-1",
                "message": {
                    "messageId": "message-1",
                    "role": "user",
                    "text": "hello",
                    "attachments": [],
                },
                "runtimeMode": "full-access",
                "interactionMode": "default",
                "bootstrap": {
                    "createThread": {
                        "projectId": "project-1",
                        "title": "A conversation",
                        "modelSelection": {"instanceId": "claudeAgent", "model": "claude-opus-5"},
                        "runtimeMode": "full-access",
                        "interactionMode": "default",
                        "branch": Value::Null,
                        "worktreePath": Value::Null,
                        "createdAt": "2026-07-26T00:23:04.909Z",
                    },
                },
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
            .expect("the turn is accepted");

        assert!(answer["sequence"].as_i64().expect("a sequence") > 0);

        let thread = fixture
            .shell
            .threads()
            .get("thread-1")
            .expect("the turn created it");
        // The developer's own message is in the transcript before anything the
        // agent could say about it, and the session says it is starting — which
        // is the acknowledgement the ticket asks to be immediate.
        assert_eq!(thread.messages.len(), 1);
        assert_eq!(thread.messages[0].role, "user");
        assert_eq!(thread.messages[0].text, "hello");
        assert_eq!(
            thread.session.as_ref().map(|session| session.status),
            Some(SessionStatus::Starting)
        );
        assert_eq!(
            thread.latest_turn.as_ref().map(|turn| turn.state),
            Some(crate::settling::TurnState::Running)
        );

        fixture.shell.threads().shutdown().await;
    }

    /// A turn for a thread nobody created and that asks for none to be created
    /// is refused rather than quietly starting a conversation with no project
    /// behind it.
    #[test]
    fn a_turn_for_an_unknown_thread_with_no_bootstrap_is_refused() {
        let fixture = Fixture::new();

        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.turn.start",
                "commandId": "c",
                "threadId": "thread-1",
                "message": {"messageId": "m", "role": "user", "text": "hello", "attachments": []},
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
            .expect_err("there is no such thread");
        assert!(
            refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );
    }

    /// The declared divergence for threads. The composer asks for a worktree
    /// when the project is in worktree mode; running the turn in the project
    /// root instead would put the agent's changes somewhere the developer did
    /// not ask for, so it is refused rather than approximated.
    #[test]
    fn a_turn_that_wants_a_worktree_is_refused_rather_than_run_in_the_project_root() {
        let fixture = Fixture::new();
        let folder = fixture.folder("workspace");
        fixture.add("project-1", &folder).expect("registered");
        fixture.add_thread("thread-1", "project-1").expect("created");

        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.turn.start",
                "commandId": "c",
                "threadId": "thread-1",
                "message": {"messageId": "m", "role": "user", "text": "hello", "attachments": []},
                "bootstrap": {
                    "prepareWorktree": {
                        "projectCwd": folder,
                        "baseBranch": "main",
                    },
                },
                "createdAt": "2026-07-26T00:23:04.909Z",
            }))
            .expect_err("worktrees are not implemented");
        assert!(
            refusal.message().contains("worktree"),
            "{}",
            refusal.message()
        );

        let thread = fixture.shell.threads().get("thread-1").expect("the thread");
        assert!(
            thread.messages.is_empty(),
            "a refused turn must not leave the prompt in the transcript"
        );
    }

    // -- the two modes -------------------------------------------------------
    //
    // Payload validation only, which is the seam the spec assigns this file:
    // "each of these is one sentence about one payload". The sequence, the two
    // feeds, the second connection, the fresh subscriber, the restart and the
    // turn in flight are all `tests/socket_thread_modes.rs`, and asserting them
    // twice would be two accounts of one decision.

    /// Every mode the contract names is accepted, and the two commands stay in
    /// their own vocabularies — an interaction mode is not a runtime mode.
    #[test]
    fn every_mode_the_contract_names_is_accepted_and_no_other() {
        let fixture = Fixture::with_a_conversation();

        for mode in RUNTIME_MODES {
            fixture
                .set_runtime_mode("thread-1", mode)
                .unwrap_or_else(|refused| panic!("{mode}: {}", refused.message()));
            assert_eq!(fixture.detail("thread-1")["runtimeMode"], mode);
        }
        for mode in INTERACTION_MODES {
            fixture
                .set_interaction_mode("thread-1", mode)
                .unwrap_or_else(|refused| panic!("{mode}: {}", refused.message()));
            assert_eq!(fixture.detail("thread-1")["interactionMode"], mode);
        }

        fixture
            .set_runtime_mode("thread-1", "plan")
            .expect_err("'plan' is an interaction mode, not a runtime one");
        fixture
            .set_interaction_mode("thread-1", "full-access")
            .expect_err("'full-access' is a runtime mode, not an interaction one");
    }

    /// A mode the contract does not name is refused rather than rounded to the
    /// nearest one the server understands — the nearest one might be the one
    /// that lets the agent run anything. The sentence names the mode, the thread
    /// it was wrong about, and what would have been accepted, because the
    /// sentence is the whole diagnostic the UI can show.
    #[test]
    fn a_mode_the_contract_does_not_name_is_refused_rather_than_rounded() {
        let fixture = Fixture::with_a_conversation();

        for sent in ["bypassPermissions", "FULL-ACCESS", "", "  "] {
            let refusal = fixture
                .set_runtime_mode("thread-1", sent)
                .expect_err("not a runtime mode the contract names");
            assert!(
                refusal.message().contains("thread-1")
                    && refusal.message().contains("full-access"),
                "{sent:?}: {}",
                refusal.message()
            );
        }

        let refusal = fixture
            .set_interaction_mode("thread-1", "planning")
            .expect_err("not an interaction mode the contract names");
        assert!(
            refusal.message().contains("thread-1") && refusal.message().contains("plan"),
            "{}",
            refusal.message()
        );

        // Nothing moved, on either field.
        assert_eq!(fixture.detail("thread-1")["runtimeMode"], "full-access");
        assert_eq!(fixture.detail("thread-1")["interactionMode"], "default");
    }

    /// Both commands are parsed before the world is consulted, so a payload
    /// that cannot be read is refused at the door: a blank identifier, a
    /// missing mode, and — the case that shows the ordering — an unreadable
    /// mode for a thread that does not exist, which is refused for the mode
    /// rather than for the thread.
    #[test]
    fn a_malformed_mode_command_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .set_runtime_mode("  ", "auto")
            .expect_err("a blank thread id names no conversation");
        assert!(
            refusal.message().contains("threadId"),
            "{}",
            refusal.message()
        );
        let refusal = fixture
            .set_interaction_mode("  ", "plan")
            .expect_err("a blank thread id names no conversation");
        assert!(
            refusal.message().contains("threadId"),
            "{}",
            refusal.message()
        );

        // A payload that will not deserialize at all still says which
        // conversation it was about. serde knows the field it could not read and
        // nothing about the thread, so without this the one refusal a client
        // shows for a malformed mode command is the only one on this wire that
        // does not name what it applies to.
        for kind in ["thread.runtime-mode.set", "thread.interaction-mode.set"] {
            let refusal = fixture
                .dispatch(&json!({"type": kind, "commandId": "c", "threadId": "thread-1"}))
                .expect_err("a mode command needs a mode");
            assert!(
                refusal.message().contains(kind)
                    && refusal.message().contains("malformed")
                    && refusal.message().contains("thread-1"),
                "{kind}: {}",
                refusal.message()
            );
        }

        // And a payload with no thread in it at all falls back to serde's own
        // sentence rather than inventing a conversation to blame.
        let refusal = fixture
            .dispatch(&json!({"type": "thread.runtime-mode.set", "commandId": "c"}))
            .expect_err("no thread and no mode");
        assert!(
            refusal.message().contains("malformed") && !refusal.message().contains("Thread '"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .set_runtime_mode("never-created", "not-a-mode")
            .expect_err("both are wrong");
        assert!(
            refusal.message().contains("not-a-mode"),
            "the world was consulted before the payload was read: {}",
            refusal.message()
        );
    }

    /// The composer's own door, and the one almost every mode change actually
    /// goes through: it sends the per-turn override on *every* send, beside the
    /// command the picker dispatched. Ticket 02 guarded the picker's command and
    /// left this one open.
    ///
    /// The cost of letting one through is not a wrong badge. The contract types
    /// the field as a closed union, so the client's decode of the whole thread
    /// payload fails on a literal it does not know — and the conversation cannot
    /// be drawn at all.
    #[test]
    fn a_turn_cannot_carry_a_mode_the_contract_does_not_name() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .start_turn("thread-1", json!({"runtimeMode": "bypassPermissions"}))
            .expect_err("not a runtime mode the contract names");
        assert!(
            refusal.message().contains("bypassPermissions")
                && refusal.message().contains("thread-1")
                && refusal.message().contains("full-access"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .start_turn("thread-1", json!({"interactionMode": "planning"}))
            .expect_err("not an interaction mode the contract names");
        assert!(
            refusal.message().contains("planning") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        // Refused at the door, so the turn left nothing behind: no prompt in the
        // transcript, and neither mode moved. The same guarantee the worktree
        // refusal has, for the same reason.
        let thread = fixture.shell.threads().get("thread-1").expect("the thread");
        assert!(
            thread.messages.is_empty(),
            "a refused turn must not leave the prompt in the transcript"
        );
        assert_eq!(fixture.detail("thread-1")["runtimeMode"], "full-access");
        assert_eq!(fixture.detail("thread-1")["interactionMode"], "default");

        // And read before the world is consulted, which a thread that does not
        // exist is what shows: the mode is the payload's answer and the unknown
        // thread is the world's.
        let refusal = fixture
            .start_turn("never-created", json!({"runtimeMode": "bypassPermissions"}))
            .expect_err("both are wrong");
        assert!(
            refusal.message().contains("bypassPermissions"),
            "the world was consulted before the payload was read: {}",
            refusal.message()
        );
    }

    /// An absent per-turn mode still means "leave the thread's alone" rather
    /// than "the default": the guard is on the value that arrived, not on the
    /// field.
    ///
    /// Asserted against a thread that does not exist, because that is the only
    /// answer a turn this fixture lets through can give without an agent to
    /// start — and it is one only the *world* could have given, so the payload
    /// was read and found to be asking for nothing.
    #[test]
    fn a_turn_that_names_no_mode_is_not_refused_for_one() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .start_turn("never-created", json!({}))
            .expect_err("there is no such thread");
        assert!(
            refusal.message().contains("never-created")
                && !refusal.message().contains("runtime mode")
                && !refusal.message().contains("interaction mode"),
            "a turn that named no mode was refused for one: {}",
            refusal.message()
        );
    }

    /// The other door on the same command. A turn for a conversation this server
    /// has never heard of carries the thread it wants created, and the modes in
    /// that are the ones it would be created with — the composer's own path for
    /// a first message, so this is not the edge case it looks like.
    #[test]
    fn a_bootstrapped_thread_cannot_be_created_in_a_mode_the_contract_does_not_name() {
        let fixture = Fixture::new();
        let folder = fixture.folder("workspace");
        fixture.add("project-1", &folder).expect("registered");

        let refusal = fixture
            .start_turn(
                "thread-1",
                json!({
                    "bootstrap": {
                        "createThread": {
                            "projectId": "project-1",
                            "title": "A conversation",
                            "modelSelection": {
                                "instanceId": "claudeAgent",
                                "model": "claude-opus-5",
                            },
                            "runtimeMode": "bypassPermissions",
                            "interactionMode": "default",
                            "branch": Value::Null,
                            "worktreePath": Value::Null,
                            "createdAt": "2026-07-26T00:23:04.909Z",
                        },
                    },
                }),
            )
            .expect_err("not a runtime mode the contract names");
        assert!(
            refusal.message().contains("bypassPermissions")
                && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        assert!(
            fixture.listed_threads().is_empty(),
            "a refused turn must not create the conversation it asked for"
        );
    }

    /// The oldest door of the three, and the one the client-runtime uses when a
    /// conversation is started somewhere other than the composer's draft.
    ///
    /// Both modes, because they are separate vocabularies: a creation that named
    /// a runtime mode the contract knows and an interaction mode it does not
    /// would be just as undrawable.
    #[test]
    fn a_thread_cannot_be_created_in_a_mode_the_contract_does_not_name() {
        let fixture = Fixture::new();
        let folder = fixture.folder("workspace");
        fixture.add("project-1", &folder).expect("registered");

        let refusal = fixture
            .add_thread_with("thread-1", "project-1", json!({"runtimeMode": "bypassPermissions"}))
            .expect_err("not a runtime mode the contract names");
        assert!(
            refusal.message().contains("bypassPermissions")
                && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .add_thread_with("thread-2", "project-1", json!({"interactionMode": "planning"}))
            .expect_err("not an interaction mode the contract names");
        assert!(
            refusal.message().contains("planning") && refusal.message().contains("thread-2"),
            "{}",
            refusal.message()
        );

        assert!(
            fixture.listed_threads().is_empty(),
            "a refused creation must not leave a conversation behind"
        );
    }

    /// A revert is parsed before the world is consulted, so a payload that
    /// cannot be read is refused at the door — and every one of these is a
    /// conversation that exists, so nothing here could have been refused for a
    /// later reason.
    ///
    /// The turn count is the interesting half. It is `NonNegativeInt` on the
    /// contract, so a negative one is not a small number, it is not a turn — and
    /// reading it as a `u64` is the whole of the check.
    #[test]
    fn a_malformed_revert_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .revert("  ", 0)
            .expect_err("a blank thread id names no conversation");
        assert!(
            refusal.message().contains("threadId"),
            "{}",
            refusal.message()
        );

        for unreadable in [json!(-1), json!(1.5), json!("2"), Value::Null] {
            let refusal = fixture
                .dispatch(&json!({
                    "type": "thread.checkpoint.revert",
                    "commandId": "c",
                    "threadId": "thread-1",
                    "turnCount": unreadable,
                }))
                .expect_err("not a turn count");
            assert!(
                refusal.message().contains("malformed") && refusal.message().contains("thread-1"),
                "{unreadable}: {}",
                refusal.message()
            );
        }

        // A revert with no turn on it names no turn, which is a different thing
        // from naming turn zero — and turn zero is a revert of the whole
        // conversation, so defaulting to it would be the largest possible guess.
        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.checkpoint.revert",
                "commandId": "c",
                "threadId": "thread-1",
            }))
            .expect_err("a revert needs a turn");
        assert!(
            refusal.message().contains("malformed") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );
    }

    /// A revert with nothing behind it is refused rather than attempted, and the
    /// sentence names the turn — the whole diagnostic the panel can show.
    ///
    /// Both halves are here because they are the same refusal reached two ways:
    /// a conversation this server has never heard of, and one it holds that has
    /// finished no turn. The second is the one a developer meets, because the
    /// undo control is on a message and the first message of a conversation has
    /// one the moment it is sent.
    #[test]
    fn a_revert_of_a_turn_with_no_checkpoint_is_refused() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .revert("never-created", 1)
            .expect_err("no such conversation");
        assert!(
            refusal.message().contains("never-created"),
            "{}",
            refusal.message()
        );

        // The conversation is real and has recorded nothing, so even turn zero —
        // the baseline, and the target the panel names most often — has no tree
        // behind it yet.
        for turn in [0, 1, 9] {
            let refusal = fixture
                .revert("thread-1", turn)
                .expect_err("nothing has been recorded");
            assert!(
                refusal.message().contains(&format!("turn {turn}")),
                "the refusal has to name the turn: {}",
                refusal.message()
            );
        }
    }

    /// Both archive commands are parsed before the world is consulted, so a
    /// payload naming no conversation is refused at the door.
    ///
    /// The whole of what can be wrong with either payload, because the
    /// conversation is the whole of what either carries — which is the reason the
    /// two share one arm and one struct.
    #[test]
    fn a_malformed_archive_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        for kind in ["thread.archive", "thread.unarchive"] {
            for blank in ["", "   "] {
                let refusal = fixture
                    .dispatch(&json!({"type": kind, "commandId": "c", "threadId": blank}))
                    .expect_err("a blank thread id names no conversation");
                assert!(
                    refusal.message().contains("threadId"),
                    "{kind}: {}",
                    refusal.message()
                );
            }

            let refusal = fixture
                .dispatch(&json!({"type": kind, "commandId": "c"}))
                .expect_err("a command about no conversation");
            assert!(
                refusal.message().contains("malformed"),
                "{kind}: {}",
                refusal.message()
            );
        }
    }

    /// Archiving moves the conversation between the two snapshots and moves
    /// nothing else about it.
    ///
    /// The list the developer works from is the point of the ticket — a project
    /// list that carries every conversation ever started buries the one that needs
    /// attention — and the archived snapshot is the only way back, because the
    /// unarchive control lives on it.
    #[test]
    fn an_archived_conversation_leaves_the_project_list_and_joins_the_other_one() {
        let fixture = Fixture::with_a_conversation();

        assert_eq!(fixture.listed_threads().len(), 1);
        assert!(fixture.archived_threads().is_empty());

        fixture.archive("thread-1").expect("archived");

        assert!(
            fixture.listed_threads().is_empty(),
            "an archived conversation is still on the list the developer works from: {:#?}",
            fixture.listed_threads()
        );
        let put_away = fixture.archived_threads();
        assert_eq!(put_away.len(), 1, "{put_away:#?}");
        assert_eq!(put_away[0]["id"], "thread-1");
        assert!(
            put_away[0]["archivedAt"].is_string(),
            "the summary has to say when: {put_away:#?}"
        );
        // The other half of the snapshot is untouched: the panel groups the
        // threads by project and looks each one up in this list, so a project
        // list filtered alongside the threads would hide them.
        assert_eq!(
            fixture.shell.archived_shell_snapshot().expect("a snapshot")["projects"]
                .as_array()
                .expect("the registry")
                .len(),
            1
        );

        fixture.unarchive("thread-1").expect("unarchived");

        assert!(fixture.archived_threads().is_empty());
        let back = fixture.listed_threads();
        assert_eq!(back.len(), 1, "{back:#?}");
        assert_eq!(back[0]["id"], "thread-1");
        assert_eq!(back[0]["archivedAt"], Value::Null);
        // Archiving is not deleting: the conversation the client opens is the
        // one that was put away, not a new one wearing its id.
        assert_eq!(fixture.detail("thread-1")["title"], back[0]["title"]);
    }

    /// A repeat of either command is refused with a sentence naming the
    /// conversation and which list it is already on.
    ///
    /// Not [`Shell::set_mode`]'s reading, where a repeat is answered: this is a
    /// move between two lists rather than a write of a value, and a second
    /// archive is a click on a control that is no longer there. The sentence is
    /// the whole diagnostic `OrchestrationDispatchCommandError` carries.
    #[test]
    fn archiving_twice_or_unarchiving_what_is_not_archived_is_refused() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .unarchive("thread-1")
            .expect_err("it was never archived");
        assert!(
            refusal.message().contains("thread-1") && refusal.message().contains("not archived"),
            "{}",
            refusal.message()
        );

        fixture.archive("thread-1").expect("archived");
        let refusal = fixture.archive("thread-1").expect_err("already archived");
        assert!(
            refusal.message().contains("thread-1")
                && refusal.message().contains("already archived"),
            "{}",
            refusal.message()
        );

        // And the refusal changed nothing: one refused command does not put the
        // conversation back on a list it left.
        assert!(fixture.listed_threads().is_empty());
        assert_eq!(fixture.archived_threads().len(), 1);
    }

    /// Both settle commands are parsed before the world is consulted, and the
    /// unsettle has one field more than a blank check.
    ///
    /// `reason` is where a client can be wrong in a way that matters: the *event*
    /// carries two reasons and the *command* carries one, because the neutral
    /// reset belongs to the server. A payload sending `activity` is asking to
    /// forge it, and is refused rather than quietly treated as `user` — which
    /// would leave the conversation in the other of the two states.
    #[test]
    fn a_malformed_settle_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        for kind in ["thread.settle", "thread.unsettle"] {
            for blank in ["", "   "] {
                let refusal = fixture
                    .dispatch(
                        &json!({"type": kind, "commandId": "c", "threadId": blank, "reason": "user"}),
                    )
                    .expect_err("a blank thread id names no conversation");
                assert!(
                    refusal.message().contains("threadId"),
                    "{kind}: {}",
                    refusal.message()
                );
            }
        }

        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.unsettle",
                "commandId": "c",
                "threadId": "thread-1",
                "reason": "activity",
            }))
            .expect_err("the neutral reset is not a client's to send");
        assert!(
            refusal.message().contains("activity") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        // And a payload with no reason at all is malformed rather than defaulted:
        // the contract declares the field, and guessing which reset was meant
        // would leave the conversation in the other of the two states.
        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.unsettle",
                "commandId": "c",
                "threadId": "thread-1",
            }))
            .expect_err("a reason is not optional");
        assert!(
            refusal.message().contains("malformed") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        assert_eq!(
            fixture.detail("thread-1")["settledOverride"],
            Value::Null,
            "a refused parse moved the conversation"
        );
    }

    /// An hour from now, in the shape the client builds one with
    /// (`Date.toISOString()` in `Sidebar.snooze.ts`).
    ///
    /// Drawn from the clock rather than written out, because a hard-coded wake
    /// time is a test that passes until that date and then fails for a reason
    /// that has nothing to do with what it asserts.
    fn an_hour_from_now() -> String {
        crate::clock::iso_from_epoch_millis(crate::clock::now_epoch_millis() + 3_600_000)
    }

    fn an_hour_ago() -> String {
        crate::clock::iso_from_epoch_millis(crate::clock::now_epoch_millis() - 3_600_000)
    }

    /// Everything about a snooze that can be wrong in the payload alone, refused
    /// before the world is consulted and without touching the conversation.
    ///
    /// The wake time is the field that is new here, and it is refused rather than
    /// normalised for one reason: a conversation snoozed until a moment that has
    /// already passed is snoozed and awake at once, carrying state it can never
    /// leave. `now` itself is refused with it, because the comparison is strictly
    /// future — a wake time equal to this instant has already elapsed by the time
    /// anything reads it.
    #[test]
    fn a_malformed_snooze_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        for kind in ["thread.snooze", "thread.unsnooze"] {
            for blank in ["", "   "] {
                let refusal = fixture
                    .dispatch(&json!({
                        "type": kind,
                        "commandId": "c",
                        "threadId": blank,
                        "reason": "user",
                        "snoozedUntil": an_hour_from_now(),
                    }))
                    .expect_err("a blank thread id names no conversation");
                assert!(
                    refusal.message().contains("threadId"),
                    "{kind}: {}",
                    refusal.message()
                );
            }
        }

        // A time that has passed, this very instant, and a string that is not a
        // time at all — one refusal, because a wake time this server cannot place
        // on its own clock is not one it can call future either.
        for hopeless in [
            an_hour_ago(),
            crate::clock::now_iso(),
            "tomorrow".to_string(),
            "2026-13-45T09:00:00.000Z".to_string(),
        ] {
            let refusal = fixture
                .snooze("thread-1", &hopeless)
                .expect_err("a wake time that is not ahead of now");
            assert!(
                refusal.message().contains(&hopeless) && refusal.message().contains("thread-1"),
                "the sentence names neither the time nor the conversation: {}",
                refusal.message()
            );
        }

        // The neutral wake belongs to the server, for `thread.unsettle`'s reason:
        // a client that could send it could wake a conversation the developer had
        // put to sleep and have it read as their own doing.
        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.unsnooze",
                "commandId": "c",
                "threadId": "thread-1",
                "reason": "activity",
            }))
            .expect_err("the neutral wake is not a client's to send");
        assert!(
            refusal.message().contains("activity") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        assert_eq!(
            fixture.detail("thread-1")["snoozedUntil"],
            Value::Null,
            "a refused parse put the conversation to sleep"
        );
    }

    /// The boundary itself: a wake time equal to the instant it is read at is
    /// refused, and one a millisecond later is taken.
    ///
    /// Its own test against [`still_ahead_of`] rather than a case in the one
    /// above, because the boundary cannot be reached through a socket — a client
    /// samples its clock, sends, and this server reads its own afterwards, so a
    /// wake time of "now" has always already elapsed by the time the guard sees
    /// it. That makes the dispatch tests unable to tell `>` from `>=`, which is
    /// the whole of the criterion. Here the two instants can be made the same
    /// one.
    #[test]
    fn a_wake_time_must_be_ahead_of_the_instant_it_is_read_at() {
        let now = 1_800_000_000_000;
        let rendered = crate::clock::iso_from_epoch_millis(now);

        assert_eq!(
            wake_time(&rendered, now),
            Err(Unusable::Elapsed),
            "{rendered} is this instant, not one still to come"
        );
        assert_eq!(
            wake_time(&crate::clock::iso_from_epoch_millis(now - 1), now),
            Err(Unusable::Elapsed),
            "a millisecond ago is not the future"
        );
        assert_eq!(
            wake_time(&crate::clock::iso_from_epoch_millis(now + 1), now),
            Ok(())
        );

        // A time this server cannot place on a clock is refused by the same
        // guard and told apart, because "that moment has passed" is a lie about
        // a string that names no moment.
        for unreadable in ["tomorrow", "2026-13-45T09:00:00.000Z", ""] {
            assert_eq!(wake_time(unreadable, now), Err(Unusable::Unreadable));
        }
    }

    /// A snooze records the wake time the developer chose and the moment they
    /// chose it, and waking by hand clears both.
    ///
    /// The wake time is stored exactly as it arrived rather than re-rendered:
    /// the client parses it back with `Date.parse` and compares it against its
    /// own clock, so a second spelling of one moment would be a field this server
    /// and that one describe differently.
    #[test]
    fn snoozing_records_the_wake_time_and_the_moment_it_was_asked_for() {
        let fixture = Fixture::with_a_conversation();
        let wake = an_hour_from_now();

        fixture.snooze("thread-1", &wake).expect("snoozed");
        let asleep = fixture.detail("thread-1");
        assert_eq!(asleep["snoozedUntil"], json!(wake));
        assert_eq!(
            asleep["snoozedAt"], asleep["updatedAt"],
            "a snooze's two stamps are one moment: {asleep:#?}"
        );

        fixture.unsnooze("thread-1").expect("woken");
        let awake = fixture.detail("thread-1");
        assert_eq!(awake["snoozedUntil"], Value::Null);
        assert_eq!(awake["snoozedAt"], Value::Null);
    }

    /// A live agent is not a blocker for a snooze, and everything waiting on the
    /// developer is.
    ///
    /// `canSnooze` mirrored where it is authoritative — the client keeps a twin so
    /// the interface can refuse before a round trip. The running session is the
    /// entry that makes the two lists different: snooze governs the developer's
    /// attention and never the agent, so the work carries on and only where the
    /// conversation is drawn changes.
    #[test]
    fn a_snooze_is_refused_by_a_raised_hand_and_not_by_a_working_agent() {
        let fixture = Fixture::with_a_conversation();
        let wake = an_hour_from_now();

        fixture
            .shell
            .threads()
            .apply(
                "thread-1",
                Change::Session(Session {
                    status: crate::settling::SessionStatus::Running,
                    runtime_mode: "full-access".to_string(),
                    active_turn_id: Some("turn-1".to_string()),
                    last_error: None,
                    updated_at: crate::clock::now_iso(),
                }),
            )
            .expect("a session");
        fixture
            .snooze("thread-1", &wake)
            .expect("a live session is not a blocker for a snooze");
        assert_eq!(fixture.detail("thread-1")["snoozedUntil"], json!(wake));
        fixture.unsnooze("thread-1").expect("woken");

        fixture
            .shell
            .threads()
            .apply(
                "thread-1",
                Change::Activity(crate::worklog::requested(
                    &crate::approval::ApprovalRequest {
                        request_id: "req-1".to_string(),
                        tool_name: "Write".to_string(),
                        input: json!({"file_path": "note.txt"}),
                        tool_use_id: Some("toolu_1".to_string()),
                        description: None,
                        suggestions: Vec::new(),
                        available_decisions: None,
                        provider_request_id: None,
                    },
                    Some("turn-1".to_string()),
                )),
            )
            .expect("a request");
        let refusal = fixture
            .snooze("thread-1", &wake)
            .expect_err("a request waiting on the developer cannot be slept through");
        assert!(
            refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );
        assert_eq!(
            fixture.detail("thread-1")["snoozedUntil"],
            Value::Null,
            "a refused snooze put the conversation to sleep anyway"
        );
    }

    /// An archived conversation is refused by both commands, for the reason the
    /// settle pair refuses one: it is not in the inbox, so there is nothing to
    /// suppress it from and nothing to bring it back to.
    #[test]
    fn an_archived_conversation_can_be_neither_snoozed_nor_woken() {
        let fixture = Fixture::with_a_conversation();
        fixture.archive("thread-1").expect("archived");

        for refusal in [
            fixture
                .snooze("thread-1", &an_hour_from_now())
                .expect_err("archived"),
            fixture.unsnooze("thread-1").expect_err("archived"),
        ] {
            assert!(
                refusal.message().contains("thread-1") && refusal.message().contains("archived"),
                "{}",
                refusal.message()
            );
        }
    }

    /// A conversation this server does not hold is refused by both, with the
    /// sentence every command uses for one.
    #[test]
    fn snoozing_a_conversation_this_server_does_not_hold_is_refused() {
        let fixture = Fixture::with_a_conversation();

        for refusal in [
            fixture
                .snooze("nobody", &an_hour_from_now())
                .expect_err("no such conversation"),
            fixture.unsnooze("nobody").expect_err("no such conversation"),
        ] {
            assert!(
                refusal.message().contains("nobody"),
                "{}",
                refusal.message()
            );
        }
    }

    /// A blank identifier is refused before the world is consulted, an unknown
    /// conversation is refused by the world, and nothing moves either way.
    ///
    /// The blank is the only thing `thread.delete` carries that can be wrong in
    /// the payload alone — whether the conversation exists, and whether it has
    /// already been deleted, are the world's questions and are answered under the
    /// fold's lock.
    #[test]
    fn a_blank_or_unknown_conversation_cannot_be_deleted() {
        let fixture = Fixture::with_a_conversation();

        for blank in ["", "   "] {
            let refusal = fixture
                .delete_thread(blank)
                .expect_err("a blank thread id names no conversation");
            assert!(
                refusal.message().contains("threadId"),
                "{}",
                refusal.message()
            );
        }

        let refusal = fixture
            .delete_thread("nobody")
            .expect_err("no such conversation");
        assert!(
            refusal.message().contains("nobody"),
            "{}",
            refusal.message()
        );

        assert_eq!(
            fixture.detail("thread-1")["deletedAt"],
            Value::Null,
            "a refused delete stamped the conversation anyway"
        );
    }

    /// Deleting stamps the conversation and takes it off both of the developer's
    /// lists, and a second delete is refused rather than answered.
    ///
    /// Refused for the archive pair's reason rather than the settle pair's: this
    /// is a conversation leaving a list, not a standing answer that folding twice
    /// lands on either way, so a second delete is a click on a control that is no
    /// longer there.
    #[test]
    fn deleting_takes_the_conversation_off_both_lists_and_refuses_a_repeat() {
        let fixture = Fixture::with_a_conversation();

        fixture.delete_thread("thread-1").expect("deleted");
        assert!(
            fixture.listed_threads().is_empty(),
            "a deleted conversation is still on the project list: {:#?}",
            fixture.listed_threads()
        );
        assert!(
            fixture.archived_threads().is_empty(),
            "a deleted conversation turned up on the archived list: {:#?}",
            fixture.archived_threads()
        );
        // The row is still here, which is the whole of what makes the deletion
        // soft — and the stamp is on it.
        assert_eq!(
            fixture.detail("thread-1")["deletedAt"],
            fixture.detail("thread-1")["updatedAt"]
        );

        let refusal = fixture
            .delete_thread("thread-1")
            .expect_err("it is already deleted");
        assert!(
            refusal.message().contains("thread-1") && refusal.message().contains("already deleted"),
            "{}",
            refusal.message()
        );
    }

    /// Every command aimed at a deleted conversation is refused, so a stale
    /// window cannot go on driving one the developer removed.
    ///
    /// Every command rather than a representative few: the guard is one question
    /// asked once in [`Shell::dispatch`], and what this pins is that the list it
    /// is asked about covers the vocabulary. A command that reached the world
    /// would move a conversation nobody can see.
    #[test]
    fn a_deleted_conversation_takes_no_further_commands() {
        let fixture = Fixture::with_a_conversation();
        fixture.delete_thread("thread-1").expect("deleted");

        let refusals = [
            // The one that matters most: a stale window's next message must not
            // start a turn in a conversation the developer removed.
            fixture.start_turn("thread-1", json!({})),
            fixture.archive("thread-1"),
            fixture.unarchive("thread-1"),
            fixture.settle("thread-1"),
            fixture.unsettle("thread-1"),
            fixture.snooze("thread-1", &an_hour_from_now()),
            fixture.unsnooze("thread-1"),
            fixture.rename_thread("thread-1", "a new name"),
            fixture.set_runtime_mode("thread-1", "auto"),
            fixture.set_interaction_mode("thread-1", "plan"),
            fixture.revert("thread-1", 1),
            fixture.stop_session("thread-1"),
            fixture.add_thread("thread-1", "project-1"),
            fixture.dispatch(&json!({
                "type": "thread.turn.interrupt",
                "commandId": "c",
                "threadId": "thread-1",
                "createdAt": "2026-07-26T00:23:04.909Z",
            })),
            fixture.dispatch(&json!({
                "type": "thread.approval.respond",
                "commandId": "c",
                "threadId": "thread-1",
                "requestId": "req-1",
                "decision": "approve",
                "createdAt": "2026-07-26T00:23:04.909Z",
            })),
            fixture.dispatch(&json!({
                "type": "thread.user-input.respond",
                "commandId": "c",
                "threadId": "thread-1",
                "requestId": "req-1",
                "answers": {"Database": "Postgres"},
                "createdAt": "2026-07-26T00:23:04.909Z",
            })),
        ];

        for asked in refusals {
            let refusal = asked.expect_err("the conversation was deleted");
            assert!(
                refusal.message().contains("thread-1") && refusal.message().contains("deleted"),
                "{}",
                refusal.message()
            );
        }

        // And the conversation is where the delete left it: one stamp, and the
        // rest of the lifecycle untouched.
        let detail = fixture.detail("thread-1");
        assert_eq!(detail["archivedAt"], Value::Null);
        assert_eq!(detail["settledOverride"], Value::Null);
        assert_eq!(detail["snoozedUntil"], Value::Null);
    }

    /// Settling stores the override and the moment, and a user unsettle pins the
    /// conversation active rather than clearing it to neutral.
    ///
    /// Asserted on the conversation as its own subscription describes it, which is
    /// where `effectiveSettled` reads the two fields — this server stores them and
    /// the client decides what the inbox shows.
    #[test]
    fn settling_records_the_override_and_the_moment_it_settled_at() {
        let fixture = Fixture::with_a_conversation();

        fixture.settle("thread-1").expect("settled");
        let settled = fixture.detail("thread-1");
        assert_eq!(settled["settledOverride"], "settled");
        assert_eq!(
            settled["settledAt"], settled["updatedAt"],
            "a settle's two stamps are one moment: {settled:#?}"
        );

        fixture.unsettle("thread-1").expect("unsettled");
        let pinned = fixture.detail("thread-1");
        assert_eq!(
            pinned["settledOverride"], "active",
            "a user unsettle pins the conversation rather than clearing the override"
        );
        assert_eq!(pinned["settledAt"], Value::Null);
    }

    /// An archived conversation is refused by both commands: it is not in the
    /// inbox, so there is nothing to take it out of and nothing to pin it back
    /// into.
    ///
    /// The sentence names the conversation and says which of the two things is
    /// wrong, because it is the whole diagnostic the interface can show.
    #[test]
    fn settling_or_unsettling_an_archived_conversation_is_refused() {
        let fixture = Fixture::with_a_conversation();
        fixture.archive("thread-1").expect("archived");

        for asked in [fixture.settle("thread-1"), fixture.unsettle("thread-1")] {
            let refusal = asked.expect_err("it is archived");
            assert!(
                refusal.message().contains("thread-1") && refusal.message().contains("archived"),
                "{}",
                refusal.message()
            );
        }

        fixture.unarchive("thread-1").expect("unarchived");
        fixture.settle("thread-1").expect("settled once it is back");
    }

    /// Neither settle command reaches a conversation this server does not hold.
    #[test]
    fn settling_an_unknown_conversation_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        for asked in [
            fixture.settle("never-created"),
            fixture.unsettle("never-created"),
        ] {
            let refusal = asked.expect_err("there is no such conversation");
            assert!(
                refusal.message().contains("never-created"),
                "{}",
                refusal.message()
            );
        }
    }

    /// Neither command reaches a conversation this server does not hold, and the
    /// refusal names the one that was asked for.
    #[test]
    fn archiving_an_unknown_conversation_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        for asked in [
            fixture.archive("never-created"),
            fixture.unarchive("never-created"),
        ] {
            let refusal = asked.expect_err("there is no such conversation");
            assert!(
                refusal.message().contains("never-created"),
                "{}",
                refusal.message()
            );
        }
    }

    /// An unknown thread is refused by name. A mode written against a
    /// conversation this server has never heard of would be state no client
    /// could ever read back.
    #[test]
    fn a_mode_for_an_unknown_thread_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .set_runtime_mode("never-created", "auto")
            .expect_err("there is no such thread");
        assert!(
            refusal.message().contains("never-created"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .set_interaction_mode("never-created", "plan")
            .expect_err("there is no such thread");
        assert!(
            refusal.message().contains("never-created"),
            "{}",
            refusal.message()
        );
    }

    // -- stopping a session --------------------------------------------------
    //
    // Payload validation and the two answers the world can give, which is as far
    // as this seam reaches: a session needs an agent, and an agent needs a real
    // binary and a turn. `tests/socket_session_stop.rs` is the rest.

    /// The payload is read before the world is consulted, so a blank
    /// conversation is refused at the door.
    ///
    /// Driven against a fixture that *has* a conversation, which is what makes
    /// this a statement about the payload: nothing here could have been refused
    /// for a later reason.
    #[test]
    fn a_stop_with_no_conversation_on_it_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        for blank in ["", "  "] {
            let refusal = fixture
                .stop_session(blank)
                .expect_err("a blank thread id names no conversation");
            assert!(
                refusal.message().contains("threadId"),
                "{}",
                refusal.message()
            );
        }

        // A payload with no `threadId` at all is the same refusal reached one step
        // earlier, and it is the one case with no conversation to name.
        let refusal = fixture
            .dispatch(&json!({"type": "thread.session.stop", "commandId": "c"}))
            .expect_err("a stop has to say what to stop");
        assert!(
            refusal.message().contains("malformed"),
            "{}",
            refusal.message()
        );
    }

    /// A conversation this server has never heard of is refused with a sentence
    /// naming it.
    ///
    /// The one thing about this command that is not a race. Stopping a session
    /// that is not running is the developer getting what they asked for, but a
    /// command naming a conversation that does not exist is a client bug, and
    /// which id it sent is the whole of what would find it.
    #[test]
    fn stopping_an_unknown_conversation_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .stop_session("never-created")
            .expect_err("there is no such conversation");
        assert!(
            refusal.message().contains("never-created"),
            "{}",
            refusal.message()
        );
    }

    /// A conversation with no agent behind it is answered rather than refused,
    /// and answering does not invent a session to have stopped.
    ///
    /// There is nothing to stop and nothing went wrong — the same reading an
    /// interrupt takes of a turn that is not running. What it must not do is
    /// publish a stop: the client folds one onto the session it holds, and a
    /// conversation whose session is `null` has none to fold it onto, so an event
    /// here would be describing a process that never existed.
    #[test]
    fn stopping_a_conversation_with_no_session_is_answered_and_publishes_nothing() {
        let fixture = Fixture::with_a_conversation();
        let before = fixture.snapshot()["snapshot"]["snapshotSequence"].clone();

        let answer = fixture
            .stop_session("thread-1")
            .expect("there is nothing to stop, which is not a failure");

        assert_eq!(
            answer["sequence"], before,
            "nothing was committed, so the answer is the log position: {answer}"
        );
        let thread = fixture.detail("thread-1");
        assert_eq!(thread["session"], Value::Null, "{thread}");
        assert_eq!(thread["latestTurn"], Value::Null);
    }

    // -- the two renames -----------------------------------------------------
    //
    // Payload validation only, which is the seam the spec assigns this file:
    // each of these is one sentence about one payload, and asserting them end to
    // end would be a connection and a dispatch per sentence. The sequence, both
    // feeds, a second connection, a fresh subscriber and the restart are
    // `tests/socket_renaming.rs`.

    /// A blank title is refused for both, with a sentence naming the problem and
    /// the thing it applies to — and the thing is left as it was.
    ///
    /// The two rename controls already refuse a blank title before dispatching,
    /// so this is reached by a client that does not. It matters anyway: a
    /// conversation or a project called "" is a row the developer cannot pick
    /// out of a list, and the contract types both titles as trimmed and
    /// non-empty.
    #[test]
    fn a_blank_title_is_refused_and_says_what_it_was_about() {
        let fixture = Fixture::with_a_conversation();

        for blank in ["", "   ", "\t\n"] {
            let refusal = fixture
                .rename_thread("thread-1", blank)
                .expect_err("a blank title is not a name");
            assert!(
                refusal.message().contains("title") && refusal.message().contains("thread-1"),
                "{blank:?}: {}",
                refusal.message()
            );

            let refusal = fixture
                .rename_project("project-1", blank)
                .expect_err("a blank title is not a name");
            assert!(
                refusal.message().contains("title") && refusal.message().contains("project-1"),
                "{blank:?}: {}",
                refusal.message()
            );
        }

        assert_eq!(fixture.detail("thread-1")["title"], "A conversation");
        assert_eq!(fixture.listed()[0]["title"], "modes");
    }

    /// A title is stored trimmed, which is what a creation already does with
    /// one. The surrounding whitespace is the client's, and keeping it would
    /// order the list by something the developer cannot see.
    #[test]
    fn a_title_is_stored_trimmed() {
        let fixture = Fixture::with_a_conversation();

        fixture
            .rename_thread("thread-1", "  Renaming things  ")
            .expect("a title with whitespace around it is a title");
        assert_eq!(fixture.detail("thread-1")["title"], "Renaming things");

        fixture
            .rename_project("project-1", "\tThe project\t")
            .expect("the same rule");
        assert_eq!(fixture.listed()[0]["title"], "The project");
    }

    /// Renaming to the title already held is answered rather than refused.
    ///
    /// Both rename controls compare before dispatching, so this is reached by a
    /// retry or a second window that has not folded the first one's event yet.
    /// Folding what it publishes a second time lands on the same state, which is
    /// what "harmless" has to mean on a server that remembers no command ids.
    #[test]
    fn renaming_to_the_title_already_held_is_answered_rather_than_refused() {
        let fixture = Fixture::with_a_conversation();

        fixture.rename_thread("thread-1", "Same").expect("renamed");
        let again = fixture
            .rename_thread("thread-1", "Same")
            .expect("a repeat is harmless");
        assert!(again["sequence"].as_i64().expect("a sequence") > 0);
        assert_eq!(fixture.detail("thread-1")["title"], "Same");

        fixture.rename_project("project-1", "Same").expect("renamed");
        fixture
            .rename_project("project-1", "Same")
            .expect("a repeat is harmless");
        assert_eq!(fixture.listed()[0]["title"], "Same");
    }

    /// **`thread.meta.update` is not only the rename control.** The composer
    /// sends it on every send whose model or branch differs from the thread's,
    /// so a payload with no title in it is the ordinary case rather than an odd
    /// one — and each field that arrives is applied while the others are left
    /// exactly as they were.
    #[test]
    fn each_field_that_arrives_is_applied_and_the_others_are_left_alone() {
        let fixture = Fixture::with_a_conversation();

        fixture
            .update_thread_meta(
                "thread-1",
                json!({"modelSelection": {"instanceId": "claudeAgent", "model": "claude-sonnet-5"}}),
            )
            .expect("the composer's own payload");
        let thread = fixture.detail("thread-1");
        assert_eq!(thread["modelSelection"]["model"], "claude-sonnet-5");
        assert_eq!(
            thread["title"], "A conversation",
            "a payload with no title moved the title: {thread}"
        );

        // The branch change the composer sends: the branch it wants and a null
        // worktree path, in one command.
        fixture
            .update_thread_meta(
                "thread-1",
                json!({"branch": "feature/renaming", "worktreePath": Value::Null}),
            )
            .expect("a branch and a cleared worktree");
        let thread = fixture.detail("thread-1");
        assert_eq!(thread["branch"], "feature/renaming");
        assert_eq!(thread["worktreePath"], Value::Null);
        assert_eq!(thread["modelSelection"]["model"], "claude-sonnet-5");

        fixture
            .rename_thread("thread-1", "Renamed")
            .expect("renamed");
        let thread = fixture.detail("thread-1");
        assert_eq!(thread["title"], "Renamed");
        assert_eq!(
            thread["branch"], "feature/renaming",
            "the rename cleared the branch: {thread}"
        );
    }

    /// `null` clears a field and a blank string does not.
    ///
    /// The distinction is the client's: it clears a worktree path by sending
    /// `null`, and a blank branch is a ref name that names nothing — a third
    /// state the contract does not type and the branch toolbar has nothing to
    /// render for.
    #[test]
    fn null_clears_a_field_and_a_blank_string_is_refused() {
        let fixture = Fixture::with_a_conversation();

        fixture
            .update_thread_meta("thread-1", json!({"branch": "main"}))
            .expect("a branch");
        fixture
            .update_thread_meta("thread-1", json!({"branch": Value::Null}))
            .expect("null clears it");
        assert_eq!(fixture.detail("thread-1")["branch"], Value::Null);

        for blank in ["", "  "] {
            let refusal = fixture
                .update_thread_meta("thread-1", json!({"branch": blank}))
                .expect_err("a blank branch names nothing");
            assert!(
                refusal.message().contains("branch") && refusal.message().contains("thread-1"),
                "{blank:?}: {}",
                refusal.message()
            );
            let refusal = fixture
                .update_thread_meta("thread-1", json!({"worktreePath": blank}))
                .expect_err("a blank worktree path names nothing");
            assert!(
                refusal.message().contains("worktree") && refusal.message().contains("thread-1"),
                "{blank:?}: {}",
                refusal.message()
            );
        }
    }

    /// A command that asks for nothing is refused rather than answered with a
    /// sequence.
    ///
    /// It would otherwise publish a `thread.meta-updated` carrying nothing but a
    /// new `updatedAt`, which the client's reducer folds as an update — so a
    /// payload that asked for no change would move the conversation in a list
    /// ordered by when things changed.
    #[test]
    fn a_meta_update_that_asks_for_nothing_is_refused() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .update_thread_meta("thread-1", json!({}))
            .expect_err("nothing was asked for");
        assert!(
            refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .dispatch(&json!({
                "type": "project.meta.update",
                "commandId": "c",
                "projectId": "project-1",
            }))
            .expect_err("nothing was asked for");
        assert!(
            refusal.message().contains("project-1") && refusal.message().contains("title"),
            "{}",
            refusal.message()
        );
    }

    /// The three fields of `project.meta.update` this registry cannot keep are
    /// refused by name rather than accepted and dropped.
    ///
    /// The UI sends one of them: the script editor sends `{projectId, scripts}`.
    /// Answering it with a sequence would tell the developer their script was
    /// saved and leave them to discover at the next restart that it was not.
    #[test]
    fn a_project_field_this_server_cannot_keep_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        for (field, value) in [
            ("scripts", json!([])),
            ("workspaceRoot", json!("C:\\elsewhere")),
            ("defaultModelSelection", Value::Null),
        ] {
            let mut command = json!({
                "type": "project.meta.update",
                "commandId": "c",
                "projectId": "project-1",
                "title": "A new name",
            });
            command
                .as_object_mut()
                .expect("an object")
                .insert(field.to_string(), value);

            let refusal = fixture
                .dispatch(&command)
                .expect_err("this server keeps none of these");
            assert!(
                refusal.message().contains(field) && refusal.message().contains("project-1"),
                "{field}: {}",
                refusal.message()
            );
        }

        // Refused whole: the title that came with the unkeepable field did not
        // land either, because a command that half happened is worse than one
        // that did not.
        assert_eq!(fixture.listed()[0]["title"], "modes");
    }

    /// A model selection that is not an object is refused.
    ///
    /// One notch worse than a malformed field usually is: the selection is
    /// published as part of the thread, so storing a number there would fail the
    /// client's decode of the whole conversation rather than of this write.
    #[test]
    fn a_model_selection_that_is_not_an_object_is_refused() {
        let fixture = Fixture::with_a_conversation();

        // A slug rather than a selection, and deliberately *not* the one the
        // conversation already holds — otherwise the assertion below would pass
        // whether the write was refused or applied.
        let refusal = fixture
            .update_thread_meta("thread-1", json!({"modelSelection": "claude-sonnet-5"}))
            .expect_err("a model selection is an object");
        assert!(
            refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );
        assert_eq!(
            fixture.detail("thread-1")["modelSelection"],
            json!({"instanceId": "claudeAgent", "model": "claude-opus-5"}),
            "a refused selection was stored anyway"
        );
    }

    #[test]
    fn a_model_selection_cannot_move_a_conversation_to_another_provider() {
        let fixture = Fixture::with_a_conversation();
        let codex = json!({"instanceId": "codex", "model": "gpt-5.6-luna"});

        let metadata = fixture
            .update_thread_meta("thread-1", json!({"modelSelection": codex}))
            .expect_err("provider ownership is fixed for a conversation");
        assert!(metadata.message().contains("claudeAgent"), "{metadata:?}");
        assert!(metadata.message().contains("codex"), "{metadata:?}");

        let turn = fixture
            .start_turn("thread-1", json!({"modelSelection": codex}))
            .expect_err("a turn cannot switch providers");
        assert!(turn.message().contains("claudeAgent"), "{turn:?}");
        assert!(turn.message().contains("codex"), "{turn:?}");

        let thread = fixture.detail("thread-1");
        assert_eq!(
            thread["modelSelection"],
            json!({"instanceId": "claudeAgent", "model": "claude-opus-5"})
        );
        assert_eq!(thread["messages"], json!([]));
        assert_eq!(thread["latestTurn"], Value::Null);
    }

    #[test]
    fn an_unregistered_stored_provider_is_refused_before_the_turn_is_published() {
        let fixture = Fixture::with_a_conversation();
        let mut thread = crate::threads::tests::a_thread("thread-with-old-driver");
        thread.provider = crate::provider::ProviderIdentity {
            instance_id: "driver-removed-since-last-run".to_string(),
            driver: "removed".to_string(),
        };
        fixture
            .shell
            .inner
            .threads
            .create(thread)
            .expect("the stored conversation is loaded");

        let refusal = fixture
            .start_turn("thread-with-old-driver", json!({}))
            .expect_err("an unavailable driver cannot take a turn");
        assert!(refusal.message().contains("not registered"), "{refusal:?}");

        let thread = fixture.detail("thread-with-old-driver");
        assert_eq!(thread["messages"], json!([]));
        assert_eq!(thread["latestTurn"], Value::Null);
        assert_eq!(thread["session"], Value::Null);
    }

    #[test]
    fn a_refused_first_turn_leaves_the_draft_absent_from_the_server() {
        let fixture = Fixture::new();
        let folder = fixture.folder("project");
        fixture.add("project-1", &folder).expect("project created");

        let refusal = fixture
            .start_turn(
                "draft-1",
                json!({
                    "modelSelection": {"instanceId": "codex", "model": "gpt-5.6-luna"},
                    "bootstrap": {
                        "createThread": {
                            "projectId": "project-1",
                            "title": "A conversation",
                            "modelSelection": {
                                "instanceId": "claudeAgent",
                                "model": "claude-opus-5"
                            },
                            "runtimeMode": "full-access",
                            "interactionMode": "default",
                            "branch": Value::Null,
                            "worktreePath": Value::Null,
                            "createdAt": "2026-07-26T00:23:04.909Z"
                        }
                    }
                }),
            )
            .expect_err("the first turn cannot switch providers");
        assert!(refusal.message().contains("codex"), "{refusal:?}");
        assert!(
            fixture
                .listed_threads()
                .iter()
                .all(|thread| thread["id"] != "draft-1"),
            "a refused first turn stored its draft"
        );
    }

    /// Both commands are parsed before the world is consulted: a blank
    /// identifier is refused at the door, and a payload that will not
    /// deserialize still names the conversation it was about.
    #[test]
    fn a_malformed_rename_is_refused_before_the_world_is_consulted() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .rename_thread("  ", "A name")
            .expect_err("a blank thread id names no conversation");
        assert!(
            refusal.message().contains("threadId"),
            "{}",
            refusal.message()
        );
        let refusal = fixture
            .rename_project("  ", "A name")
            .expect_err("a blank project id names no project");
        assert!(
            refusal.message().contains("projectId"),
            "{}",
            refusal.message()
        );

        // Wrong in two ways at once, and refused for the payload rather than for
        // the world — which is what "parsed before the world is consulted"
        // means where it is observable.
        let refusal = fixture
            .rename_thread("never-created", "  ")
            .expect_err("both are wrong");
        assert!(
            refusal.message().contains("title"),
            "the world was consulted before the payload was read: {}",
            refusal.message()
        );
        let refusal = fixture
            .rename_project("never-registered", "  ")
            .expect_err("both are wrong");
        assert!(
            refusal.message().contains("title"),
            "the world was consulted before the payload was read: {}",
            refusal.message()
        );

        // A payload serde cannot read still says which conversation it was
        // about, the way the mode commands do.
        let refusal = fixture
            .dispatch(&json!({
                "type": "thread.meta.update",
                "commandId": "c",
                "threadId": "thread-1",
                "title": 7,
            }))
            .expect_err("a title is a string");
        assert!(
            refusal.message().contains("malformed") && refusal.message().contains("thread-1"),
            "{}",
            refusal.message()
        );
    }

    /// An unknown thread and an unknown project are both refused by name.
    ///
    /// A title written against something this server has never heard of would be
    /// state no client could ever read back.
    #[test]
    fn renaming_something_this_server_does_not_have_is_refused_by_name() {
        let fixture = Fixture::with_a_conversation();

        let refusal = fixture
            .rename_thread("never-created", "A name")
            .expect_err("there is no such thread");
        assert!(
            refusal.message().contains("never-created"),
            "{}",
            refusal.message()
        );

        let refusal = fixture
            .rename_project("never-registered", "A name")
            .expect_err("there is no such project");
        assert!(
            refusal.message().contains("never-registered"),
            "{}",
            refusal.message()
        );

        // And the registry is untouched: a rename of nothing must not leave a
        // project behind.
        assert_eq!(fixture.listed().len(), 1);
        assert_eq!(fixture.listed()[0]["id"], "project-1");
    }

    /// Projects and threads share one counter, because they share one
    /// subscription and a client folds both against one cursor. Two counters
    /// would have a thread's events overtake a project's and the client would
    /// drop whichever fell behind.
    #[test]
    fn projects_and_threads_are_numbered_from_the_same_counter() {
        let fixture = Fixture::new();
        let first = fixture.folder("first");
        let second = fixture.folder("second");

        let project = fixture.add("project-1", &first).expect("registered");
        let thread = fixture
            .add_thread("thread-1", "project-1")
            .expect("created");
        let later = fixture.add("project-2", &second).expect("registered");

        let (project, thread, later) = (
            project["sequence"].as_i64().expect("a sequence"),
            thread["sequence"].as_i64().expect("a sequence"),
            later["sequence"].as_i64().expect("a sequence"),
        );
        assert!(
            project < thread && thread < later,
            "{project}, {thread}, {later} are not one increasing log"
        );
        assert_eq!(
            fixture.snapshot()["snapshot"]["snapshotSequence"],
            json!(later)
        );
    }
}
