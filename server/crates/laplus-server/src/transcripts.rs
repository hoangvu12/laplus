//! Writing a conversation down, behind the stream rather than in it.
//!
//! [`crate::threads`] holds the conversation the UI reads and [`crate::store`]
//! knows the SQL. This is the queue between them, and it exists for one
//! criterion of ticket 11: *transcript writes do not block or stutter the live
//! stream*. Everything here follows from taking that literally.
//!
//! ## Nothing on the publishing path ever waits for a disk
//!
//! [`Threads::apply`](crate::threads::Threads::apply) is synchronous and is
//! called from two places that must not stall: the socket's read loop, which
//! owes the next frame, and an agent's driver, which is mid-turn. So a change
//! *queues* a [`Write`] and returns. The queue is drained by a thread of its
//! own — not a `tokio` task, because every write here ends in a commit and a
//! worker parked on an `fsync` is a worker the socket is not using.
//!
//! The queue is unbounded, which is a claim about its depth rather than
//! optimism. Writes happen at message boundaries and nowhere else — a token
//! delta owes the database nothing, because the buffered message that supersedes
//! it is the authoritative one — so what fills this queue is whole messages,
//! which a person and an agent produce a few of per turn. [`Transcripts::pending`]
//! is the gauge that says so, and a number that climbs is the disk falling
//! behind rather than something to guess at.
//!
//! ## What that costs, stated rather than discovered
//!
//! A reply the app was killed in the middle of is not in the database. Its
//! deltas were never written and its buffered message never arrived, so a
//! restart shows the conversation up to the developer's prompt. That is the
//! honest answer — the agent's own session holds what it actually said, and the
//! next turn resumes into it — and it is the price of keeping the disk out of the
//! streaming path. An ordinary close is not affected: [`Transcripts::flush`] is
//! part of shutdown, so the last message of a finished turn is on disk before the
//! process ends.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};

use crate::store::Database;
use crate::threads::{Activity, Message, ThreadRow};

/// How many queued writes one transaction may carry.
///
/// A turn produces a handful in quick succession — the prompt, the reply, two
/// activities, and the row beside each — and one commit for all of them is one
/// `fsync` instead of six.
///
/// It is deliberately not larger, and the reason is the lock rather than the
/// statement count: this transaction holds [`Database`]'s connection, and the
/// registry's own commands take the same one from the socket's read loop. A batch
/// is therefore also the longest a `project.create` can be made to wait, so the
/// cap is set at "a few turns' worth" rather than at whatever SQLite would
/// tolerate.
const BATCH: usize = 64;

/// One durable change to a conversation.
///
/// The vocabulary is the transcript's, not the table's — the same division
/// [`crate::store`] keeps for projects. Nothing here mentions a column.
#[derive(Debug, Clone, PartialEq)]
pub enum Write {
    /// The conversation itself: its title, its selection, its latest turn. Every
    /// change to a thread writes one of these, because every change moves
    /// `updatedAt`.
    ///
    /// Boxed because it is much the largest member and every other one would
    /// otherwise be padded out to its size, the same way
    /// [`crate::orchestration`]'s command vocabulary boxes its own outlier.
    Thread(Box<ThreadRow>),
    /// A message at a position in the transcript. Written on the way in and again
    /// when the buffered message replaces what the deltas built, which is why
    /// this is an upsert rather than an append.
    Message {
        thread_id: String,
        ordinal: usize,
        message: Message,
    },
    /// An activity at a position in the work log.
    ///
    /// Boxed for the reason [`Write::Thread`] is, and it became the reason once an
    /// activity carried a payload, a kind, a summary and a sequence: it is now the
    /// member every other one would be padded out to.
    Activity {
        thread_id: String,
        ordinal: usize,
        activity: Box<Activity>,
    },
    /// The `claude` session this conversation is being held in — the handle
    /// `--resume` is given after a restart.
    AgentSession {
        thread_id: String,
        session_id: String,
    },
    /// What the working tree looked like when a turn finished.
    ///
    /// Stored because the *ref* it names outlives the process: the tree is in
    /// the developer's own repository, so a conversation that came back from a
    /// restart without its checkpoints would be one whose diffs exist and
    /// cannot be found. Boxed for the reason [`Write::Thread`] is.
    Checkpoint {
        thread_id: String,
        checkpoint: Box<crate::threads::Checkpoint>,
    },
}

impl Write {
    /// The conversation this write is about.
    fn thread(&self) -> &str {
        match self {
            Write::Thread(thread) => &thread.id,
            Write::Message { thread_id, .. }
            | Write::Activity { thread_id, .. }
            | Write::AgentSession { thread_id, .. }
            | Write::Checkpoint { thread_id, .. } => thread_id,
        }
    }
}

/// What travels on the queue: work, or a request to be told the work is done.
#[derive(Debug)]
enum Queued {
    Write(Write),
    /// Answered once everything queued before it has reached the disk.
    Barrier(oneshot::Sender<()>),
}

/// The write queue, as everything that changes a conversation sees it.
///
/// Cheap to clone and every clone is the same queue, like the registries it sits
/// behind. The thread draining it lives as long as the last clone.
#[derive(Debug, Clone)]
pub struct Transcripts {
    queue: mpsc::UnboundedSender<Queued>,
    pending: Arc<AtomicUsize>,
    /// Conversations that have been deleted, whose queued writes are to be
    /// dropped. See [`Transcripts::discard`].
    forgotten: Arc<Mutex<HashSet<String>>>,
}

impl Transcripts {
    /// A queue whose writes reach `database`.
    pub fn writing_to(database: Arc<Database>) -> Transcripts {
        Transcripts::draining(Some(database))
    }

    /// A queue that keeps nothing.
    ///
    /// For callers that hold no database: the unit tests of the live
    /// conversation, which have nothing to say about the stored one. The same
    /// role [`Database::in_memory`] plays for the registry's own tests, and the
    /// same reason — a test about folding a change should neither need a project
    /// row to hang a thread off nor leave a file behind.
    pub fn nowhere() -> Transcripts {
        Transcripts::draining(None)
    }

    fn draining(database: Option<Arc<Database>>) -> Transcripts {
        let (queue, mut queued) = mpsc::unbounded_channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&pending);
        let forgotten: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let deleted = Arc::clone(&forgotten);

        // A thread rather than a task, for the reason in this module's
        // documentation: the loop lives as long as the process and every pass
        // through it blocks on a commit. `blocking_recv` is what an
        // `UnboundedReceiver` offers outside a runtime, and outside a runtime is
        // exactly where this is.
        std::thread::spawn(move || {
            while let Some(first) = queued.blocking_recv() {
                let mut batch = vec![first];
                // Whatever is already waiting joins the same transaction.
                while batch.len() < BATCH {
                    match queued.try_recv() {
                        Ok(next) => batch.push(next),
                        Err(_) => break,
                    }
                }

                let (writes, barriers) = split(batch);
                // Counted before the drop, because a caller waiting on the gauge
                // is waiting for these writes to be *dealt with* rather than
                // stored — and a deleted conversation's writes are dealt with by
                // being dropped.
                counted.fetch_sub(writes.len(), Ordering::SeqCst);

                let writes = keeping(writes, &deleted);
                if let Some(database) = &database {
                    if !writes.is_empty() {
                        store(database, &writes);
                    }
                }

                // After the commit, so a caller released by a barrier can open
                // the database and find what it was waiting for.
                for barrier in barriers {
                    let _ = barrier.send(());
                }
            }
        });

        Transcripts {
            queue,
            pending,
            forgotten,
        }
    }

    /// Put one change on the queue. Never waits, and never fails visibly — a
    /// closed queue means the process is on its way out.
    pub fn queue(&self, write: Write) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self.queue.send(Queued::Write(write)).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// These conversations have been deleted; drop whatever is still queued for
    /// them.
    ///
    /// Not tidiness — correctness of the *log*. A project delete removes its
    /// conversations' rows synchronously, while their writes may already be on this
    /// queue; those writes would then be refused by the foreign key whose project
    /// has gone, and a refused batch is retried one write at a time and printed.
    /// Deleting a project in the middle of a conversation is an ordinary thing to
    /// do and should not produce a page of complaints about it.
    ///
    /// The set is never emptied, because there is no moment at which it is provably
    /// safe to: the writes it exists to drop are somewhere behind an unknown
    /// number of others. It grows by one entry per conversation the developer
    /// deletes in one run of the app, which is a bound set by their clicking.
    pub fn discard(&self, thread_ids: &[String]) {
        let mut forgotten = lock(&self.forgotten);
        for thread_id in thread_ids {
            forgotten.insert(thread_id.clone());
        }
    }

    /// Writes that have been queued and are not yet on disk.
    ///
    /// The gauge that makes "the disk is keeping up" observable, in the same
    /// family as [`crate::ServerState::live_agents`]. What keeps the unbounded
    /// queue bounded in practice is that writes happen at message boundaries; this
    /// is how a stall would be seen rather than guessed at.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Wait until everything queued before this call is on disk.
    ///
    /// A barrier on the queue rather than a poll of [`Transcripts::pending`],
    /// because the two answer different questions: pending reaching zero says the
    /// writer has caught up with whatever it happened to have, and this says it
    /// has caught up with *me*. Shutdown wants the second one.
    pub async fn flush(&self) {
        let (done, waiting) = oneshot::channel();
        if self.queue.send(Queued::Barrier(done)).is_err() {
            return;
        }
        // An error here is the writer thread having gone, which means nothing
        // more will ever be written and there is nothing left to wait for.
        let _ = waiting.await;
    }
}

/// Write a batch down, and if it will not go, find out which of it would not.
///
/// The batch is one transaction, so a single write the database refuses rolls
/// back everything beside it — including writes for other conversations that had
/// nothing wrong with them. Retrying one at a time costs a commit per write in
/// the case that is already going badly and narrows the loss to the row that
/// actually failed, which is also the row worth naming in the log.
fn store(database: &Database, writes: &[Write]) {
    let Err(error) = database.transcribe(writes) else {
        return;
    };
    if writes.len() <= 1 {
        eprintln!("laplus: a transcript write was not stored: {error} ({writes:?})");
        return;
    }

    eprintln!(
        "laplus: a batch of {} transcript writes was refused ({error}); retrying them singly",
        writes.len()
    );
    for write in writes {
        store(database, std::slice::from_ref(write));
    }
}

/// Drop whatever belongs to a conversation that has been deleted.
fn keeping(writes: Vec<Write>, forgotten: &Mutex<HashSet<String>>) -> Vec<Write> {
    // Read once for the batch, not once per write: the set only ever grows, so a
    // conversation deleted between these two lines is caught by the next batch.
    let forgotten = lock(forgotten).clone();
    if forgotten.is_empty() {
        return writes;
    }
    writes
        .into_iter()
        .filter(|write| !forgotten.contains(write.thread()))
        .collect()
}

/// A poisoned lock means a previous holder panicked mid-insert. What is behind it
/// is a set of strings with no invariant a panic could have broken halfway, so
/// refusing to use it would turn one panic into a server that can never write a
/// transcript again.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Separate the work from the acknowledgements, keeping the work in order.
fn split(batch: Vec<Queued>) -> (Vec<Write>, Vec<oneshot::Sender<()>>) {
    let mut writes = Vec::with_capacity(batch.len());
    let mut barriers = Vec::new();
    for item in batch {
        match item {
            Queued::Write(write) => writes.push(write),
            Queued::Barrier(barrier) => barriers.push(barrier),
        }
    }
    (writes, barriers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a_row(id: &str) -> ThreadRow {
        ThreadRow {
            id: id.to_string(),
            // Every row in these tests hangs off the one project `a_database`
            // registers, because the foreign key requires one.
            project_id: "project-1".to_string(),
            title: "A conversation".to_string(),
            model_selection: json!({"instanceId": "claudeAgent", "model": "claude-opus-5"}),
            runtime_mode: "full-access".to_string(),
            interaction_mode: "default".to_string(),
            branch: None,
            worktree_path: None,
            agent_session_id: None,
            latest_turn: None,
            latest_user_message_at: None,
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
            lifecycle: crate::threads::Lifecycle::default(),
        }
    }

    fn a_message(id: &str, text: &str) -> Message {
        Message {
            id: id.to_string(),
            role: "user".to_string(),
            text: text.to_string(),
            turn_id: Some("turn-1".to_string()),
            streaming: false,
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
        }
    }

    /// A database with somewhere to hang a thread off, which the foreign key
    /// requires.
    fn a_database() -> (Arc<Database>, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database = Database::in_memory().expect("an in-memory database");
        let root = crate::projects::WorkspaceRoot::check(&directory.path().to_string_lossy())
            .expect("a temporary directory is a usable workspace root");
        database
            .insert_project("project-1", "project", &root, None, 1)
            .expect("registers");
        (Arc::new(database), directory)
    }

    /// The contract of the barrier: after it, what was queued is readable back.
    /// Everything about "survives a restart" rests on this, because shutdown is
    /// what calls it.
    #[tokio::test]
    async fn a_flush_waits_for_what_was_queued_before_it() {
        let (database, _directory) = a_database();
        let transcripts = Transcripts::writing_to(Arc::clone(&database));

        transcripts.queue(Write::Thread(Box::new(a_row("thread-1"))));
        for (ordinal, text) in ["first", "second"].into_iter().enumerate() {
            transcripts.queue(Write::Message {
                thread_id: "thread-1".to_string(),
                ordinal,
                message: a_message(&format!("message-{ordinal}"), text),
            });
        }

        transcripts.flush().await;

        assert_eq!(transcripts.pending(), 0);
        let stored = database.conversations().expect("reads");
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0]
                .messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<&str>>(),
            vec!["first", "second"]
        );
    }

    /// The reconciliation rule, reaching the disk. A message is written when the
    /// deltas built it and written again when the buffered message replaces it,
    /// at the same position — so the second write has to *replace* the first
    /// rather than put a second copy of the reply in the transcript.
    #[tokio::test]
    async fn a_message_written_twice_is_replaced_rather_than_duplicated() {
        let (database, _directory) = a_database();
        let transcripts = Transcripts::writing_to(Arc::clone(&database));
        transcripts.queue(Write::Thread(Box::new(a_row("thread-1"))));

        for text in ["the beginning ", "the beginning and the end"] {
            transcripts.queue(Write::Message {
                thread_id: "thread-1".to_string(),
                ordinal: 0,
                message: a_message("assistant-1", text),
            });
        }
        transcripts.flush().await;

        let stored = database.conversations().expect("reads");
        assert_eq!(
            stored[0].messages,
            vec![a_message("assistant-1", "the beginning and the end")]
        );
    }

    /// More writes than one transaction carries, so the batching loop is driven
    /// rather than assumed. A backlog has to arrive whole and in order.
    #[tokio::test]
    async fn a_backlog_larger_than_one_batch_arrives_whole_and_in_order() {
        let (database, _directory) = a_database();
        let transcripts = Transcripts::writing_to(Arc::clone(&database));
        transcripts.queue(Write::Thread(Box::new(a_row("thread-1"))));

        let queued = BATCH * 2 + 1;
        for ordinal in 0..queued {
            transcripts.queue(Write::Message {
                thread_id: "thread-1".to_string(),
                ordinal,
                message: a_message(&format!("message-{ordinal:05}"), &ordinal.to_string()),
            });
        }
        transcripts.flush().await;

        let stored = database.conversations().expect("reads");
        assert_eq!(stored[0].messages.len(), queued);
        assert!(
            stored[0]
                .messages
                .iter()
                .enumerate()
                .all(|(index, message)| message.text == index.to_string()),
            "the transcript came back in a different order than it was written"
        );
    }

    /// A conversation that has been deleted is not written down.
    ///
    /// The case this exists for is a write already on the queue when the delete
    /// happens: its thread's rows have gone with the project, so storing it would
    /// be refused by the foreign key, and a refused batch is retried singly and
    /// printed. Driven with the discard first, because a test that queued first
    /// would be racing the writer for which of the two got there — and the filter
    /// it is checking is the same one either way.
    #[tokio::test]
    async fn a_discarded_conversation_is_not_written_down() {
        let (database, _directory) = a_database();
        let transcripts = Transcripts::writing_to(Arc::clone(&database));

        transcripts.discard(&["thread-1".to_string()]);
        transcripts.queue(Write::Thread(Box::new(a_row("thread-1"))));
        transcripts.queue(Write::Message {
            thread_id: "thread-1".to_string(),
            ordinal: 0,
            message: a_message("message-1", "into the void"),
        });
        // A conversation nobody deleted, in the same batch, to show the filter is
        // per write rather than per batch.
        transcripts.queue(Write::Thread(Box::new(a_row("thread-2"))));
        transcripts.flush().await;

        let stored = database.conversations().expect("reads");
        assert_eq!(
            stored
                .iter()
                .map(|conversation| conversation.thread.id.as_str())
                .collect::<Vec<&str>>(),
            vec!["thread-2"]
        );
        assert_eq!(transcripts.pending(), 0, "a dropped write still counts as done");
    }

    /// A queue with nothing behind it still answers a flush. Otherwise every
    /// caller would have to know whether there was a database.
    #[tokio::test]
    async fn a_queue_that_keeps_nothing_still_completes() {
        let transcripts = Transcripts::nowhere();
        transcripts.queue(Write::Thread(Box::new(a_row("thread-1"))));
        transcripts.flush().await;
        assert_eq!(transcripts.pending(), 0);
    }
}
