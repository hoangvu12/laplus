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
//! - **`afterSequence` is answered at its two ends and not in between.** The
//!   contract lets a client with a cached snapshot ask for a replay from a
//!   sequence. A cursor that is still [`Sequences::current`] is a replay of no
//!   events, and that is answered exactly: the opening carries no snapshot, and
//!   for the real client — which asks for no completion marker — no chunk at
//!   all. Any other cursor is answered with the whole snapshot, because
//!   replaying from a position needs a log of events and this server keeps
//!   none. See ADR-0016 for why it keeps none and why the two ends are enough.
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
use crate::store::{Conflict, Database, Insert, Registry, Removal, Sequences, StorageError};
use crate::subscriptions::{EventSource, BACKLOG};
use crate::settling::SessionStatus;
use crate::threads::{self, Change, Prompt, Session, Thread, Threads};
use crate::transcripts::Transcripts;

/// The tag that carries every write to the registry.
pub const DISPATCH_COMMAND: &str = "orchestration.dispatchCommand";

/// The subscription that *is* the project list.
pub const SUBSCRIBE_SHELL: &str = "orchestration.subscribeShell";

/// The contract's default when a client sends no runtime mode
/// (`DEFAULT_RUNTIME_MODE` in `orchestration.ts`). Repeated here rather than
/// inferred, because it decides how much latitude the agent is given.
const DEFAULT_RUNTIME_MODE: &str = "full-access";

/// The contract's `DEFAULT_PROVIDER_INTERACTION_MODE`.
const DEFAULT_INTERACTION_MODE: &str = "default";

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
    /// The number every change on this wire is ordered by, shared with
    /// [`Threads`] because both aggregates travel on the same subscription and
    /// a client folds them against one cursor.
    sequences: Sequences,
    threads: Threads,
    transcripts: Transcripts,
}

/// A command this server understands, once its payload has been read.
///
/// Parsing to this is where a malformed or unimplemented command is turned
/// away, so by the time [`Shell::dispatch`] has one it is only the *world* that
/// can still refuse it.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    CreateProject(CreateProject),
    DeleteProject { project_id: String },
    CreateThread(CreateThread),
    StartTurn(Box<StartTurn>),
    InterruptTurn(InterruptTurn),
    RespondToApproval(RespondToApproval),
    RespondToUserInput(RespondToUserInput),
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
                transcripts,
            }),
        }
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
            // The worktree when the conversation has one, the project's folder
            // otherwise — the same rule [`crate::turn::starting`] follows for
            // where the agent runs, and it has to be, because the tree a
            // checkpoint recorded is the tree the agent was working in.
            workspace_root: thread
                .worktree_path
                .clone()
                .unwrap_or(project.workspace_root),
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
    pub fn dispatch(
        &self,
        payload: &Value,
        index: &Index,
        config: &ServerConfig,
    ) -> Result<Value, CommandError> {
        let sequence = match Command::parse(payload)? {
            Command::CreateProject(create) => self.create_project(&create)?,
            Command::DeleteProject { project_id } => self.delete_project(&project_id, index)?,
            Command::CreateThread(create) => self.create_thread(&create)?,
            Command::StartTurn(start) => self.start_turn(&start, config)?,
            Command::InterruptTurn(interrupt) => self.interrupt_turn(&interrupt)?,
            Command::RespondToApproval(respond) => self.respond_to_approval(&respond)?,
            Command::RespondToUserInput(respond) => self.respond_to_user_input(&respond)?,
        };

        Ok(json!({ "sequence": sequence }))
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

    /// Register a conversation.
    ///
    /// The project has to be one this server knows, because the thread's whole
    /// purpose is to run an agent in that project's folder — a thread pointing
    /// at nothing would be a conversation that could never take a turn.
    fn create_thread(&self, create: &CreateThread) -> Result<i64, CommandError> {
        let project = self.project(&create.thread.project_id)?;
        self.inner
            .threads
            .create(create.to_thread(&project))
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

        // Bootstrapping is how the UI's composer starts a *new* conversation:
        // the thread is a client-side draft until the first turn, which carries
        // the thread it wants created alongside the message. Creating it here
        // rather than expecting a separate `thread.create` is not a shortcut —
        // it is the only path the real composer takes.
        if !self.inner.threads.contains(&start.thread_id) {
            let Some(create) = start.bootstrap_thread() else {
                return Err(CommandError::new(format!(
                    "There is no thread '{}' on this server, and the turn did not ask for one to \
                     be created.",
                    start.thread_id
                )));
            };
            self.create_thread(&create)?;
        }

        // Everything that can still refuse the turn happens before anything is
        // published. A refusal that had already put the prompt in the transcript
        // would leave a conversation showing a message and a turn marked running
        // with nothing left alive to settle it.
        let project = self.project(&self.open_thread(&start.thread_id)?.project_id)?;

        let turn_id = threads::fresh_turn_id();
        // The developer's own message first, so it is in the transcript before
        // anything the agent says about it can be.
        self.inner
            .threads
            .apply(
                &start.thread_id,
                Change::UserMessage {
                    message_id: start.message.message_id.clone(),
                    text: start.message.text.clone(),
                    turn_id: turn_id.clone(),
                },
            )
            .ok_or_else(|| self.not_open(&start.thread_id))?;
        self.inner.threads.apply(
            &start.thread_id,
            Change::TurnRequested {
                turn_id: turn_id.clone(),
                message_id: start.message.message_id.clone(),
                model_selection: start.model_selection.clone(),
                runtime_mode: start.runtime_mode.clone(),
                interaction_mode: start.interaction_mode.clone(),
            },
        );

        // Read *after* the turn request, because that is what carries the
        // composer's current selection: a model or a runtime mode picked for
        // this turn has to be the one the agent is started with, not the one the
        // thread was created with.
        let thread = self.open_thread(&start.thread_id)?;
        let sequence = self
            .inner
            .threads
            .apply(
                &start.thread_id,
                Change::Session(Session {
                    status: SessionStatus::Starting,
                    runtime_mode: thread.runtime_mode.clone(),
                    active_turn_id: Some(turn_id.clone()),
                    last_error: None,
                    updated_at: now_iso(),
                }),
            )
            .ok_or_else(|| self.not_open(&start.thread_id))?;

        let starting = crate::turn::starting(
            &thread,
            &project.workspace_root,
            &config.settings.providers.claude_agent,
        );
        if let Err(why) = crate::turn::send(
            &self.inner.threads,
            &starting,
            Prompt {
                turn_id,
                text: start.message.text.clone(),
            },
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

        Ok(sequence)
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
            .ok_or_else(|| {
                CommandError::new(format!(
                    "Project '{project_id}' is not registered with this server."
                ))
            })
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
        Ok(shell_snapshot(
            &self.inner.database.registry()?,
            &self.inner.threads,
            self.inner.sequences.current(),
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
fn shell_snapshot(registry: &Registry, threads: &Threads, sequence: i64) -> Value {
    let updated_at = threads
        .latest_change()
        .filter(|latest| latest > &registry.updated_at)
        .unwrap_or_else(|| registry.updated_at.clone());

    json!({
        "snapshotSequence": sequence,
        "projects": registry
            .projects
            .iter()
            .map(Project::to_value)
            .collect::<Vec<Value>>(),
        "threads": threads.shell_summaries(),
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
    fn to_thread(&self, project: &Project) -> Thread {
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

        Thread {
            id: self.thread_id.clone(),
            project_id: self.thread.project_id.clone(),
            title,
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
            // Nothing has run yet, so there is no agent session to resume into.
            // The first turn's `init` line is where this is filled in.
            agent_session_id: None,
        }
    }
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
    DEFAULT_INTERACTION_MODE.to_string()
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
    /// roughly twenty command types and laplus implements six, so "which
    /// one" is the most useful thing a message can say during the build-out.
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
            "project.delete" => {
                let delete: DeleteProjectPayload = read(payload, kind)?;
                Ok(Command::DeleteProject {
                    project_id: non_blank(delete.project_id, "projectId", kind)?,
                })
            }
            "thread.create" => {
                let create: CreateThreadPayload = read(payload, kind)?;
                Ok(Command::CreateThread(CreateThread {
                    thread_id: non_blank(create.thread_id, "threadId", kind)?,
                    thread: ThreadFields {
                        project_id: non_blank(create.thread.project_id, "projectId", kind)?,
                        ..create.thread
                    },
                }))
            }
            "thread.turn.start" => {
                let start: StartTurn = read(payload, kind)?;
                Ok(Command::StartTurn(Box::new(StartTurn {
                    thread_id: non_blank(start.thread_id, "threadId", kind)?,
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
                    ..respond
                }))
            }
            unimplemented => Err(CommandError::new(format!(
                "Command not implemented by this server: {unimplemented}"
            ))),
        }
    }
}

fn read<T: serde::de::DeserializeOwned>(payload: &Value, kind: &str) -> Result<T, CommandError> {
    serde_json::from_value(payload.clone())
        .map_err(|error| CommandError::new(format!("{kind} is malformed: {error}")))
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
            // reaches `turn::send`, which resolves `binaryPath` for real — so
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
            self.dispatch(&json!({
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
            }))
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

    /// Roughly twenty command types exist and laplus implements six. Each
    /// refusal has to name what was asked for, or a developer cannot tell which
    /// of them is missing.
    #[test]
    fn an_unimplemented_or_malformed_command_is_refused_by_name() {
        let fixture = Fixture::new();

        let refusal = fixture
            .dispatch(&json!({"type": "thread.archive", "commandId": "c", "threadId": "t"}))
            .expect_err("archiving is not implemented");
        assert!(
            refusal.message().contains("thread.archive"),
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
