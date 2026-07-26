//! The database: lightcode's durable state, and the only file that speaks SQL.
//!
//! This is the first slice with state that outlives the process, so it arrives
//! with ticket 05 rather than in a later persistence phase. The spec's build
//! order lists persistence eighth; that ordering is rejected here on purpose —
//! a project registry that forgets its projects is not a smaller version of the
//! feature, it is a different one. Each slice owns the storage it needs and
//! this one establishes the store the rest extend.
//!
//! Three things shape the interface, and all three are about keeping SQL from
//! spreading:
//!
//! - **The vocabulary is the registry's, not the table's.** Callers ask to
//!   insert a project or remove one; they never see a statement, a row, or a
//!   transaction. That is what lets [`crate::orchestration`] be about the wire
//!   and nothing else.
//! - **A commit is one transaction.** Registering a project both writes the row
//!   and advances the sequence the client uses to order events. Those cannot be
//!   allowed to disagree, so they are never two calls.
//! - **The database is the clock.** Every timestamp the registry produces comes
//!   from SQLite's own `strftime`, which renders exactly the millisecond-ISO
//!   form the captures show. A second time source would be a second answer to
//!   "when did this happen".
//!
//! The connection sits behind a plain [`Mutex`] rather than a pool. There is
//! one process, the work under the lock is local SQLite and never awaits, and a
//! desktop app's registry is a few dozen rows. A pool would be machinery
//! standing in for a problem this does not have.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, Row, Transaction};

use crate::projects::{Project, WorkspaceRoot};
use crate::threads::{Activity, Conversation, LatestTurn, Message, ThreadRow};
use crate::transcripts::Write;

/// The schema, one entry per version. The index is the `user_version` the
/// entry brings the database *to*, so appending is the only supported edit —
/// a released version's statements are history and are never rewritten.
const MIGRATIONS: &[&str] = &[
    // v1 — ticket 05, the project registry.
    r#"
    CREATE TABLE projects (
        id             TEXT PRIMARY KEY,
        title          TEXT NOT NULL,
        workspace_root TEXT NOT NULL,
        -- The same folder in the form two spellings of it share. UNIQUE is
        -- what makes "adding the same folder twice" impossible rather than
        -- merely checked for; see `crate::projects::WorkspaceRoot`.
        canonical_root TEXT NOT NULL UNIQUE,
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL
    ) STRICT;

    -- Exactly one row, holding what is true of the registry as a whole. The
    -- sequence is the client's ordering key for shell events and must survive
    -- a restart: a client caches a snapshot and ignores events at or below the
    -- sequence it already holds, so a counter that restarted at zero would
    -- make the server's next few changes invisible.
    CREATE TABLE orchestration (
        id         INTEGER PRIMARY KEY CHECK (id = 0),
        sequence   INTEGER NOT NULL,
        updated_at TEXT NOT NULL
    ) STRICT;
    "#,
    // v2 — ticket 11, conversations and their transcripts.
    r#"
    CREATE TABLE threads (
        id         TEXT PRIMARY KEY,
        -- A conversation exists to run an agent in a project's folder, so a
        -- thread whose project has gone is not a smaller thing, it is nothing.
        -- The cascade is what makes `project.delete` complete rather than
        -- leaving rows only a later restart would find.
        project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        title      TEXT NOT NULL,
        -- Two columns hold JSON verbatim: `model_selection` is the contract's
        -- `ModelSelection` and `latest_turn` its `OrchestrationLatestTurn`.
        -- Neither is ever queried into — this database sorts and joins on ids
        -- and timestamps and nothing else — so spreading them over nine columns
        -- would buy a shape this file has to keep in step with the client's, for
        -- no query it enables.
        model_selection        TEXT NOT NULL,
        runtime_mode           TEXT NOT NULL,
        interaction_mode       TEXT NOT NULL,
        branch                 TEXT,
        worktree_path          TEXT,
        -- The `claude` session this conversation is being held in, as the agent
        -- itself reported it. What `--resume` is given, and therefore the whole
        -- of how continuity survives a restart: the context is in the agent's
        -- own store and this is the handle on it.
        agent_session_id       TEXT,
        latest_turn            TEXT,
        latest_user_message_at TEXT,
        created_at             TEXT NOT NULL,
        updated_at             TEXT NOT NULL
    ) STRICT;

    CREATE INDEX threads_by_project ON threads (project_id);

    CREATE TABLE thread_messages (
        thread_id  TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        id         TEXT NOT NULL,
        -- The position in the transcript. Stored rather than derived from
        -- `created_at`, because that is a millisecond timestamp and two messages
        -- inside one would come back in whichever order the file happened to
        -- yield — a transcript that reordered itself across a restart would be a
        -- different conversation.
        ordinal    INTEGER NOT NULL,
        role       TEXT NOT NULL,
        text       TEXT NOT NULL,
        turn_id    TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        -- Keyed by both, so the buffered message that replaces what the deltas
        -- built is an upsert of one row rather than a second copy of the reply.
        PRIMARY KEY (thread_id, id)
    ) STRICT;

    CREATE INDEX thread_messages_in_order ON thread_messages (thread_id, ordinal);

    CREATE TABLE thread_activities (
        thread_id  TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        id         TEXT NOT NULL,
        ordinal    INTEGER NOT NULL,
        tone       TEXT NOT NULL,
        kind       TEXT NOT NULL,
        summary    TEXT NOT NULL,
        -- The activity's own payload, verbatim. Same reasoning as
        -- `model_selection`: its shape belongs to whatever appended it.
        payload    TEXT NOT NULL,
        turn_id    TEXT,
        created_at TEXT NOT NULL,
        PRIMARY KEY (thread_id, id)
    ) STRICT;

    CREATE INDEX thread_activities_in_order ON thread_activities (thread_id, ordinal);
    "#,
];

/// SQLite's rendering of the contract's `IsoDateTime`, matching the captured
/// `2026-07-26T00:23:04.909Z` exactly — `%f` is seconds with milliseconds, and
/// `'now'` is UTC.
const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ','now')";

/// The projects table's columns, in the order [`project_from_row`] reads them.
/// Named once so the two cannot drift apart: adding a column to one without the
/// other is a runtime error rather than a compile-time one.
const PROJECT_COLUMNS: &str =
    "id, title, workspace_root, canonical_root, created_at, updated_at";

/// The thread table's columns, in the order [`thread_from_row`] reads them.
const THREAD_COLUMNS: &str = "id, project_id, title, model_selection, runtime_mode, \
     interaction_mode, branch, worktree_path, agent_session_id, latest_turn, \
     latest_user_message_at, created_at, updated_at";

/// The registry's durable half.
#[derive(Debug)]
pub struct Database {
    connection: Mutex<Connection>,
}

/// Hands out the number every orchestration change is ordered by, and keeps the
/// announcements in that order.
///
/// Ticket 05 had the database do the numbering, incrementing a column inside the
/// same transaction as the write. Ticket 10 could not keep that: a streamed turn
/// publishes an event per token, and a counter that lived in SQLite would mean
/// a transaction commit — an `fsync` — per token of a reply that is not
/// persisted at all. So the counter moved into memory and the database records
/// the number a durable write *was given* rather than choosing it.
///
/// Two properties survive the move, and they are the ones ticket 05 was after:
///
/// - **It is seeded from the database**, so a restart never re-issues a number a
///   committed change already used. That was the reason the sequence was
///   persisted, and it still is.
/// - **A commit stamps the database with its own number**, so the stored value
///   stays a high-water mark rather than a count of commits.
///
/// What the move costs is gaps: a number is taken before a command knows whether
/// it will commit, and a refused command leaves its number unused. Nothing reads
/// this as a dense log — the client only ever asks whether an event is newer than
/// what it holds — so a gap is invisible and a *reused* number would not be.
///
/// ## Taking a number is holding the log open
///
/// [`Sequences::commit`] returns a **guard**, not an integer, and that is the
/// whole of the ordering guarantee. Projects are committed by a socket's read
/// loop and threads by an agent's driver task; both publish onto the one feed the
/// project list is folding, and a client drops anything at or below the sequence
/// it already holds. So a project numbered 5 that published after a thread event
/// numbered 6 would be dropped permanently — the project would simply never
/// appear.
///
/// Holding the guard from taking the number to publishing the change makes that
/// impossible. It replaces the plain `Mutex<()>` ticket 05 held for the same
/// reason across one aggregate; there are two now, and one lock has to cover
/// both.
#[derive(Debug, Clone)]
pub struct Sequences {
    committed: Arc<Mutex<i64>>,
    /// Read without the lock, for the callers that only want to know how far the
    /// log has got — a snapshot describing when it was taken. Kept in step with
    /// the value behind the lock, and only ever written while it is held.
    watermark: Arc<AtomicI64>,
}

/// A number taken, and the log held open until the change it numbers has been
/// announced. Dropping it releases the next writer.
#[derive(Debug)]
pub struct Numbered<'a> {
    sequence: i64,
    _log: std::sync::MutexGuard<'a, i64>,
}

impl Numbered<'_> {
    pub fn sequence(&self) -> i64 {
        self.sequence
    }
}

impl Sequences {
    /// Continue from wherever this database was left.
    pub fn resuming(database: &Database) -> Result<Sequences, StorageError> {
        Ok(Sequences::from(database.registry()?.sequence))
    }

    /// Continue from a known position. The seam the tests use, and what a
    /// caller that already read the registry wants.
    pub fn from(committed: i64) -> Sequences {
        Sequences {
            committed: Arc::new(Mutex::new(committed)),
            watermark: Arc::new(AtomicI64::new(committed)),
        }
    }

    /// Take the number for the change about to happen, and hold the log until it
    /// has been announced.
    ///
    /// A poisoned lock means a previous holder panicked between taking a number
    /// and publishing. The counter behind it is one integer with no invariant a
    /// panic could have left half-built, so carrying on is better than turning
    /// one panic into a server that can never change anything again.
    pub fn commit(&self) -> Numbered<'_> {
        let mut log = self
            .committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *log += 1;
        let sequence = *log;
        self.watermark.store(sequence, Ordering::SeqCst);
        Numbered {
            sequence,
            _log: log,
        }
    }

    /// The highest number handed out so far — what a snapshot describes itself
    /// as being taken at, so that every event issued after it is strictly newer
    /// and none is dropped by a client comparing the two.
    pub fn current(&self) -> i64 {
        self.watermark.load(Ordering::SeqCst)
    }
}

/// Everything a shell snapshot is made of, read as one consistent picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    /// The sequence of the most recent committed change. A client resuming
    /// from a cached snapshot compares against this.
    pub sequence: i64,
    pub updated_at: String,
    pub projects: Vec<Project>,
}

/// What happened to an attempt to register a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insert {
    /// Written, and the log advanced to this sequence. Carries the row as it
    /// was actually stored — two of its timestamps are the database's, so this
    /// is the only account of the project that is not a guess.
    Committed { sequence: i64, project: Project },
    /// Refused, because this project is already here.
    Occupied {
        existing: Project,
        conflict: Conflict,
    },
}

/// Which uniqueness rule an insert ran into. The two are worth distinguishing
/// because they mean different things to the person reading the message: one
/// is a folder already registered, the other is an id already in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Id,
    WorkspaceRoot,
}

/// What happened to an attempt to remove a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removal {
    /// Gone, and the log advanced to this sequence.
    Committed {
        sequence: i64,
        /// The folder that is no longer a project, in the form
        /// [`crate::projects::WorkspaceRoot::canonical`] gives — which is the
        /// form everything the server holds *about* a project is keyed by.
        ///
        /// Read out of the row before it is deleted rather than resolved
        /// afterwards, because by then there is nothing left to resolve it
        /// from: the point of carrying it is that a caller can release what it
        /// was holding, and a deleted project the caller cannot name is one
        /// whose resources stay held.
        canonical_root: String,
    },
    /// Nothing was registered under that id, so nothing changed. Carries the
    /// unchanged sequence, because the caller still owes the client one.
    Absent(i64),
}

impl Removal {
    /// The sequence to answer the client with, whichever way it went.
    pub fn sequence(&self) -> i64 {
        match self {
            Removal::Committed { sequence, .. } | Removal::Absent(sequence) => *sequence,
        }
    }
}

/// The database could not be used.
///
/// Carries what was being attempted, because a bare SQLite message ("unable to
/// open database file") does not say which of the server's several reasons for
/// touching the disk failed.
#[derive(Debug)]
pub struct StorageError {
    attempting: &'static str,
    detail: String,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "cannot {}: {}", self.attempting, self.detail)
    }
}

impl std::error::Error for StorageError {}

impl StorageError {
    /// Curried so the call sites read `.map_err(StorageError::while_("…"))`,
    /// which keeps the description next to the statement it describes.
    fn while_(attempting: &'static str) -> impl Fn(rusqlite::Error) -> StorageError {
        move |error| StorageError {
            attempting,
            detail: error.to_string(),
        }
    }

    fn refusing(attempting: &'static str, detail: String) -> StorageError {
        StorageError { attempting, detail }
    }
}

/// Where the database lives when the server is not being tested.
///
/// Beside the keybindings and the logs, in the same per-user directory
/// [`crate::config`] already reports to the UI — one place to look, and one
/// place to delete to start over.
pub fn default_path() -> PathBuf {
    crate::config::data_dir().join("state.sqlite")
}

impl Database {
    /// Open the database at `path`, creating the file, its parent directories
    /// and its schema if they are not there.
    ///
    /// "Without manual setup" is a requirement of the ticket, so every step
    /// from a bare machine to a usable registry happens here. A first run and
    /// a hundredth run take the same path.
    pub fn open(path: &Path) -> Result<Database, StorageError> {
        // Reported as a storage failure rather than an I/O one: from the
        // caller's side this is the database refusing to exist. `parent` is
        // empty for a bare filename, which needs no directory made.
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| {
                StorageError::refusing(
                    "create the data directory",
                    format!("{}: {error}", parent.display()),
                )
            })?;
        }

        let connection = Connection::open(path).map_err(StorageError::while_("open the database"))?;
        Database::prepare(connection)
    }

    /// A database that exists only for as long as it is held. Used by tests
    /// that have nothing to say about persistence, so they neither touch the
    /// developer's real registry nor leave a file behind.
    pub fn in_memory() -> Result<Database, StorageError> {
        let connection =
            Connection::open_in_memory().map_err(StorageError::while_("open the database"))?;
        Database::prepare(connection)
    }

    fn prepare(connection: Connection) -> Result<Database, StorageError> {
        // Ticket 11 gives threads a foreign key onto projects. SQLite ignores
        // such a constraint unless this is on, and it is per-connection rather
        // than stored in the file, so it belongs at every open.
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(StorageError::while_("enable foreign keys"))?;

        migrate(&connection)?;

        // The singleton the sequence lives on. `OR IGNORE` rather than a guard,
        // so this is the same statement on a first run and every run after.
        connection
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO orchestration (id, sequence, updated_at) \
                     VALUES (0, 0, {NOW})"
                ),
                [],
            )
            .map_err(StorageError::while_("initialise the registry"))?;

        Ok(Database {
            connection: Mutex::new(connection),
        })
    }

    /// The whole registry, read inside one transaction so the projects and the
    /// sequence describing them cannot come from either side of a commit.
    pub fn registry(&self) -> Result<Registry, StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("read the registry"))?;

        let (sequence, updated_at) = orchestration_row(&transaction)?;
        let projects = {
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT {PROJECT_COLUMNS} FROM projects ORDER BY created_at ASC, id ASC"
                ))
                .map_err(StorageError::while_("read the projects"))?;
            let rows = statement
                .query_map([], project_from_row)
                .map_err(StorageError::while_("read the projects"))?;
            rows.collect::<rusqlite::Result<Vec<Project>>>()
                .map_err(StorageError::while_("read the projects"))?
        };

        Ok(Registry {
            sequence,
            updated_at,
            projects,
        })
    }

    /// One project by id, for a caller that has a thread and needs the folder
    /// the agent should run in.
    pub fn project(&self, id: &str) -> Result<Option<Project>, StorageError> {
        find_project(&self.lock(), "id", id)
    }

    /// Register a project, unless its id or its folder is already taken.
    ///
    /// The check and the write are one transaction, so two clients racing to
    /// add the same folder cannot both win. `created_at` comes from the client
    /// — it is part of the command in the contract — while `updated_at` is the
    /// server's, because it describes the row rather than the intent. A client
    /// that omits its timestamp gets the database's, which is why this is an
    /// `Option` rather than a string the caller had to invent: the contract
    /// types the field as non-empty, so there is no honest empty value.
    ///
    /// `at` is the sequence this write commits at, taken from [`Sequences`] by
    /// the caller. It is an argument rather than something read here because the
    /// same counter orders changes that never reach a database at all — see
    /// [`Sequences`] for why that had to become true.
    pub fn insert_project(
        &self,
        id: &str,
        title: &str,
        root: &WorkspaceRoot,
        created_at: Option<&str>,
        at: i64,
    ) -> Result<Insert, StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("register the project"))?;

        if let Some(existing) = find_project(&transaction, "id", id)? {
            return Ok(Insert::Occupied {
                existing,
                conflict: Conflict::Id,
            });
        }
        if let Some(existing) = find_project(&transaction, "canonical_root", root.canonical())? {
            return Ok(Insert::Occupied {
                existing,
                conflict: Conflict::WorkspaceRoot,
            });
        }

        transaction
            .execute(
                &format!(
                    "INSERT INTO projects \
                     (id, title, workspace_root, canonical_root, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, COALESCE(?5, {NOW}), {NOW})"
                ),
                (id, title, root.display(), root.canonical(), created_at),
            )
            .map_err(StorageError::while_("register the project"))?;

        // Read back inside the transaction rather than reconstructed by the
        // caller. Two timestamps here are the database's, so this is the only
        // way to hand back exactly what was written — and doing it before the
        // commit means there is no window in which a project is registered but
        // the caller was told it failed.
        let project = find_project(&transaction, "id", id)?.ok_or_else(|| {
            StorageError::refusing(
                "register the project",
                format!("the row for '{id}' was not there immediately after inserting it"),
            )
        })?;

        stamp(&transaction, at)?;
        transaction
            .commit()
            .map_err(StorageError::while_("register the project"))?;
        Ok(Insert::Committed {
            sequence: at,
            project,
        })
    }

    /// Take a project off the registry.
    ///
    /// A row and nothing else — the folder on disk is not the registry's to
    /// touch, and this is the only place that could have been tempted to.
    ///
    /// Removing an id that is not there is not an error. A client that retries
    /// a delete it already succeeded at is describing a world the server agrees
    /// with, and answering it with a failure would be the server disagreeing
    /// about the past. Such a no-op answers with the sequence the registry is
    /// *already* at rather than with `at`, because nothing happened at `at` and
    /// a client that waited for an event there would wait forever.
    pub fn remove_project(&self, id: &str, at: i64) -> Result<Removal, StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("remove the project"))?;

        // Read before the delete, inside the same transaction: after it there
        // is no row to read the folder out of, and the caller needs the folder
        // to release what it was holding for it.
        let going = find_project(&transaction, "id", id)?;

        let removed = transaction
            .execute("DELETE FROM projects WHERE id = ?1", (id,))
            .map_err(StorageError::while_("remove the project"))?;
        if removed == 0 {
            let (sequence, _) = orchestration_row(&transaction)?;
            return Ok(Removal::Absent(sequence));
        }

        // The read above and the delete are one transaction, so a row that
        // deleted must have been readable a statement earlier. Checked rather
        // than defaulted: an empty `canonical_root` is not a harmless fallback,
        // it is a value the caller would release nothing under and never be
        // told about. Refusing here is before the commit, so the transaction
        // rolls back and the registry is left as it was — the same shape as the
        // symmetric check in `insert_project`.
        let canonical_root = going.map(|project| project.canonical_root).ok_or_else(|| {
            StorageError::refusing(
                "remove the project",
                format!("the row for '{id}' deleted but could not be read a statement earlier"),
            )
        })?;

        stamp(&transaction, at)?;
        transaction
            .commit()
            .map_err(StorageError::while_("remove the project"))?;
        Ok(Removal::Committed {
            sequence: at,
            canonical_root,
        })
    }

    /// Write down a batch of conversation changes, as one transaction.
    ///
    /// The vocabulary is [`crate::transcripts::Write`] and the order is the order
    /// they were queued in, which is load-bearing twice: a message cannot be
    /// stored before the thread it belongs to exists, and a buffered message has
    /// to land after the accumulation it replaces.
    ///
    /// One transaction for the batch is the whole point — a commit is an `fsync`,
    /// and a turn's several writes are worth one of them rather than six. The
    /// caller's part of that bargain is that a failing batch is retried a write
    /// at a time; see [`crate::transcripts`].
    pub fn transcribe(&self, writes: &[Write]) -> Result<(), StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("store the transcript"))?;

        for write in writes {
            match write {
                Write::Thread(thread) => upsert_thread(&transaction, thread)?,
                Write::Message {
                    thread_id,
                    ordinal,
                    message,
                } => upsert_message(&transaction, thread_id, *ordinal, message)?,
                Write::Activity {
                    thread_id,
                    ordinal,
                    activity,
                } => upsert_activity(&transaction, thread_id, *ordinal, activity)?,
                Write::AgentSession {
                    thread_id,
                    session_id,
                } => remember_agent_session(&transaction, thread_id, session_id)?,
            }
        }

        transaction
            .commit()
            .map_err(StorageError::while_("store the transcript"))
    }

    /// Every conversation this database holds, with its transcript.
    ///
    /// Three queries rather than a join, and the reason is the shape of the
    /// answer: a join would return one row per message with the thread's columns
    /// repeated on each, and rebuilding the nesting from that is more code than
    /// reading three ordered lists and walking them. It is also one pass over
    /// each table rather than an index lookup per thread, which is what makes a
    /// boot's cost the size of the history rather than the number of
    /// conversations times its depth.
    pub fn conversations(&self) -> Result<Vec<Conversation>, StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("read the conversations"))?;

        let threads: Vec<ThreadRow> = query(
            &transaction,
            &format!("SELECT {THREAD_COLUMNS} FROM threads ORDER BY created_at ASC, id ASC"),
            thread_from_row,
            "read the conversations",
        )?;
        let messages: Vec<(String, Message)> = query(
            &transaction,
            "SELECT thread_id, id, role, text, turn_id, created_at, updated_at \
             FROM thread_messages ORDER BY thread_id ASC, ordinal ASC",
            message_from_row,
            "read the transcripts",
        )?;
        let activities: Vec<(String, Activity)> = query(
            &transaction,
            "SELECT thread_id, id, tone, kind, summary, payload, turn_id, created_at \
             FROM thread_activities ORDER BY thread_id ASC, ordinal ASC",
            activity_from_row,
            "read the work logs",
        )?;

        // Where each thread ended up, so the two ordered lists can be walked
        // once and dealt into place. A row whose thread is not here is
        // impossible while the foreign key holds; ignoring one rather than
        // failing means a database somebody edited by hand still opens.
        let at: HashMap<String, usize> = threads
            .iter()
            .enumerate()
            .map(|(index, thread)| (thread.id.clone(), index))
            .collect();
        let mut conversations: Vec<Conversation> = threads
            .into_iter()
            .map(|thread| Conversation {
                thread,
                messages: Vec::new(),
                activities: Vec::new(),
            })
            .collect();

        for (thread_id, message) in messages {
            if let Some(index) = at.get(&thread_id) {
                conversations[*index].messages.push(message);
            }
        }
        for (thread_id, activity) in activities {
            if let Some(index) = at.get(&thread_id) {
                conversations[*index].activities.push(activity);
            }
        }

        Ok(conversations)
    }

    /// A poisoned lock means a previous holder panicked mid-statement. SQLite
    /// rolls its transaction back on drop, so the connection is still sound and
    /// refusing to use it would turn one panic into a dead registry.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Bring the schema up to the version this build expects.
///
/// `user_version` is a counter stored in the file itself, which is what makes
/// this safe to run unconditionally at every open: it is the database's own
/// account of how far it has been taken, not something the server remembers.
fn migrate(connection: &Connection) -> Result<(), StorageError> {
    let applied: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(StorageError::while_("read the schema version"))?;

    if applied as usize > MIGRATIONS.len() {
        return Err(StorageError::refusing(
            "use the database",
            format!(
                "it is at schema version {applied} and this build only knows up to {}; \
                 it was written by a newer version of lightcode",
                MIGRATIONS.len()
            ),
        ));
    }

    for (index, statements) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let version = index + 1;
        connection
            .execute_batch(&format!(
                "BEGIN; {statements} PRAGMA user_version = {version}; COMMIT;"
            ))
            .map_err(StorageError::while_("apply the schema"))?;
    }

    Ok(())
}

/// Record the sequence a change committed at, and stamp the registry. The
/// single writer of both, so "the log reached here" and "the registry changed
/// at" cannot drift apart.
///
/// `MAX` rather than an assignment: the counter is in memory now (see
/// [`Sequences`]) and nothing stops two commits reaching this out of the order
/// they took their numbers in. What the column has to be is the highest number
/// any committed change has used, because that is what the next boot resumes
/// from — and a plain assignment would let a slower commit lower it.
fn stamp(connection: &Connection, at: i64) -> Result<(), StorageError> {
    connection
        .execute(
            &format!("UPDATE orchestration SET sequence = MAX(sequence, ?1), updated_at = {NOW}"),
            (at,),
        )
        .map_err(StorageError::while_("advance the registry"))?;
    Ok(())
}

fn orchestration_row(connection: &Connection) -> Result<(i64, String), StorageError> {
    connection
        .query_row(
            "SELECT sequence, updated_at FROM orchestration WHERE id = 0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(StorageError::while_("read the registry"))
}

/// The one project whose `column` is `key`, if there is one.
///
/// `column` is only ever a literal from this file — the two the schema declares
/// unique — so interpolating it is not a hole a client can reach. The *key* is
/// always bound, never interpolated.
fn find_project(
    connection: &Connection,
    column: &'static str,
    key: &str,
) -> Result<Option<Project>, StorageError> {
    connection
        .query_row(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE {column} = ?1"),
            (key,),
            project_from_row,
        )
        .optional()
        .map_err(StorageError::while_("look for an existing project"))
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        title: row.get(1)?,
        workspace_root: row.get(2)?,
        canonical_root: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// Read a whole ordered table, mapping each row.
///
/// The statement is a literal from this file every time; the shared shape is
/// here because [`Database::conversations`] runs three of them and the borrow
/// dance around a prepared statement is the same each time.
fn query<T>(
    connection: &Connection,
    statement: &str,
    read: impl Fn(&Row<'_>) -> rusqlite::Result<T>,
    attempting: &'static str,
) -> Result<Vec<T>, StorageError> {
    let mut prepared = connection
        .prepare(statement)
        .map_err(StorageError::while_(attempting))?;
    let rows = prepared
        .query_map([], read)
        .map_err(StorageError::while_(attempting))?;
    rows.collect::<rusqlite::Result<Vec<T>>>()
        .map_err(StorageError::while_(attempting))
}

/// Store a thread's own row, replacing whatever was there.
///
/// An upsert rather than an insert because every change to a conversation writes
/// this — `updatedAt` moves on all of them — so the second one onwards is
/// necessarily a replacement. `created_at` is excluded from the update: the row
/// is rewritten from a live thread whose `created_at` is the stored one, but
/// leaving it out means a future caller cannot move it by accident.
fn upsert_thread(transaction: &Transaction<'_>, thread: &ThreadRow) -> Result<(), StorageError> {
    transaction
        .execute(
            "INSERT INTO threads (id, project_id, title, model_selection, runtime_mode, \
                interaction_mode, branch, worktree_path, agent_session_id, latest_turn, \
                latest_user_message_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
             ON CONFLICT (id) DO UPDATE SET \
                project_id = excluded.project_id, \
                title = excluded.title, \
                model_selection = excluded.model_selection, \
                runtime_mode = excluded.runtime_mode, \
                interaction_mode = excluded.interaction_mode, \
                branch = excluded.branch, \
                worktree_path = excluded.worktree_path, \
                agent_session_id = excluded.agent_session_id, \
                latest_turn = excluded.latest_turn, \
                latest_user_message_at = excluded.latest_user_message_at, \
                updated_at = excluded.updated_at",
            rusqlite::params![
                thread.id,
                thread.project_id,
                thread.title,
                thread.model_selection.to_string(),
                thread.runtime_mode,
                thread.interaction_mode,
                thread.branch,
                thread.worktree_path,
                thread.agent_session_id,
                thread.latest_turn.as_ref().map(|turn| turn.to_value().to_string()),
                thread.latest_user_message_at,
                thread.created_at,
                thread.updated_at,
            ],
        )
        .map_err(StorageError::while_("store the conversation"))?;
    Ok(())
}

/// Store one message at its position in the transcript.
///
/// `streaming` is not a column. Only whole messages are ever written — a delta
/// owes the database nothing, because the buffered message supersedes it — so a
/// stored message was never mid-stream, and [`message_from_row`] says `false`
/// because that is what it was rather than as a default.
fn upsert_message(
    transaction: &Transaction<'_>,
    thread_id: &str,
    ordinal: usize,
    message: &Message,
) -> Result<(), StorageError> {
    transaction
        .execute(
            "INSERT INTO thread_messages \
                (thread_id, id, ordinal, role, text, turn_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (thread_id, id) DO UPDATE SET \
                ordinal = excluded.ordinal, \
                text = excluded.text, \
                updated_at = excluded.updated_at",
            rusqlite::params![
                thread_id,
                message.id,
                ordinal as i64,
                message.role,
                message.text,
                message.turn_id,
                message.created_at,
                message.updated_at,
            ],
        )
        .map_err(StorageError::while_("store the message"))?;
    Ok(())
}

/// Store one activity at its position in the work log.
///
/// An upsert, though nothing rewrites an activity today: the same id arriving
/// twice is a repeat rather than a second thing that happened, and a second row
/// would put it in the work log twice.
fn upsert_activity(
    transaction: &Transaction<'_>,
    thread_id: &str,
    ordinal: usize,
    activity: &Activity,
) -> Result<(), StorageError> {
    transaction
        .execute(
            "INSERT INTO thread_activities \
                (thread_id, id, ordinal, tone, kind, summary, payload, turn_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (thread_id, id) DO UPDATE SET \
                ordinal = excluded.ordinal, \
                tone = excluded.tone, \
                summary = excluded.summary, \
                payload = excluded.payload",
            rusqlite::params![
                thread_id,
                activity.id,
                ordinal as i64,
                activity.tone,
                activity.kind,
                activity.summary,
                activity.payload.to_string(),
                activity.turn_id,
                activity.created_at,
            ],
        )
        .map_err(StorageError::while_("store the activity"))?;
    Ok(())
}

/// Record the `claude` session a conversation is being held in.
///
/// Its own statement rather than part of the row upsert, because it arrives from
/// somewhere else entirely: the agent's `init` line, mid-turn, while the thread
/// row is being written by every other change beside it. Touching one column
/// means the two cannot overwrite each other's work.
/// Refuses to update nothing, rather than succeeding at it. The stored session is
/// what a restart resumes into, so an update that matched no row is continuity
/// silently lost — the same reasoning as the checked row counts in
/// [`Database::remove_project`], and refusing before the commit rolls the
/// transaction back so the write is retried on its own and named in the log.
fn remember_agent_session(
    transaction: &Transaction<'_>,
    thread_id: &str,
    session_id: &str,
) -> Result<(), StorageError> {
    let updated = transaction
        .execute(
            "UPDATE threads SET agent_session_id = ?2 WHERE id = ?1",
            (thread_id, session_id),
        )
        .map_err(StorageError::while_("store the agent session"))?;
    if updated == 0 {
        return Err(StorageError::refusing(
            "store the agent session",
            format!("there is no thread '{thread_id}' to record session '{session_id}' against"),
        ));
    }
    Ok(())
}

fn thread_from_row(row: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    let model_selection: String = row.get(3)?;
    let latest_turn: Option<String> = row.get(9)?;

    Ok(ThreadRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        // A selection that will not parse is a row somebody edited by hand.
        // `null` decodes on the client as "no selection", which is a worse answer
        // than the stored one and a much better one than no conversation.
        model_selection: serde_json::from_str(&model_selection).unwrap_or(serde_json::Value::Null),
        runtime_mode: row.get(4)?,
        interaction_mode: row.get(5)?,
        branch: row.get(6)?,
        worktree_path: row.get(7)?,
        agent_session_id: row.get(8)?,
        latest_turn: latest_turn.as_deref().and_then(latest_turn_from_json),
        latest_user_message_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// A stored `OrchestrationLatestTurn` back as one.
///
/// Read field by field rather than deserialized, for the same reason every
/// payload in this crate is built by hand: the shape is the contract's and the
/// two ends of it are worth being able to see together. A turn missing its id or
/// its request time is not a turn, so it comes back as `None` and the
/// conversation as one that has not taken a turn yet.
fn latest_turn_from_json(stored: &str) -> Option<LatestTurn> {
    let turn: serde_json::Value = serde_json::from_str(stored).ok()?;
    let text = |key: &str| {
        turn.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };

    Some(LatestTurn {
        turn_id: text("turnId")?,
        state: crate::threads::turn_state(turn.get("state").and_then(serde_json::Value::as_str)?),
        requested_at: text("requestedAt")?,
        started_at: text("startedAt"),
        completed_at: text("completedAt"),
        assistant_message_id: text("assistantMessageId"),
    })
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<(String, Message)> {
    Ok((
        row.get(0)?,
        Message {
            id: row.get(1)?,
            role: row.get(2)?,
            text: row.get(3)?,
            turn_id: row.get(4)?,
            streaming: false,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        },
    ))
}

fn activity_from_row(row: &Row<'_>) -> rusqlite::Result<(String, Activity)> {
    let tone: String = row.get(2)?;
    let payload: String = row.get(5)?;

    Ok((
        row.get(0)?,
        Activity {
            id: row.get(1)?,
            tone: crate::threads::tone(&tone),
            kind: row.get(3)?,
            summary: row.get(4)?,
            payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
            turn_id: row.get(6)?,
            created_at: row.get(7)?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real directory to register, a database to register it in, and the
    /// counter the writes take their sequence from — which is the arrangement a
    /// real caller has, since [`crate::orchestration::Shell`] owns one of each.
    struct Fixture {
        database: Database,
        _directory: tempfile::TempDir,
        root: WorkspaceRoot,
        sequences: Sequences,
    }

    impl Fixture {
        fn new() -> Fixture {
            let directory = tempfile::tempdir().expect("a temporary directory");
            let root = WorkspaceRoot::check(&directory.path().to_string_lossy())
                .expect("a temporary directory is a usable workspace root");
            let database = Database::in_memory().expect("an in-memory database");
            let sequences = Sequences::resuming(&database).expect("a fresh log");
            Fixture {
                database,
                _directory: directory,
                root,
                sequences,
            }
        }

        fn add(&self, id: &str) -> Insert {
            self.database
                .insert_project(
                    id,
                    "project",
                    &self.root,
                    Some("2026-07-26T00:23:04.909Z"),
                    self.sequences.commit().sequence(),
                )
                .expect("the insert reaches the database")
        }

        fn remove(&self, id: &str) -> Removal {
            self.database
                .remove_project(id, self.sequences.commit().sequence())
                .expect("the delete reaches the database")
        }
    }

    fn sequence(insert: Insert) -> i64 {
        match insert {
            Insert::Committed { sequence, .. } => sequence,
            other => panic!("expected a committed insert, got {other:?}"),
        }
    }

    /// The ticket's "created on first run without manual setup", at its
    /// smallest: an empty database answers questions rather than failing them.
    #[test]
    fn a_fresh_database_has_an_empty_registry() {
        let database = Database::in_memory().expect("an in-memory database");
        let registry = database.registry().expect("reads");

        assert_eq!(registry.sequence, 0);
        assert!(registry.projects.is_empty());
        assert!(
            registry.updated_at.ends_with('Z'),
            "{} is not an ISO timestamp",
            registry.updated_at
        );
    }

    #[test]
    fn a_registered_project_is_readable_back() {
        let fixture = Fixture::new();
        assert_eq!(sequence(fixture.add("project-1")), 1);

        let registry = fixture.database.registry().expect("reads");
        assert_eq!(registry.sequence, 1);
        assert_eq!(registry.projects.len(), 1);

        let project = &registry.projects[0];
        assert_eq!(project.id, "project-1");
        assert_eq!(project.title, "project");
        assert_eq!(project.workspace_root, fixture.root.display());
        assert_eq!(project.canonical_root, fixture.root.canonical());
        // The client's timestamp is kept; the server's is its own.
        assert_eq!(project.created_at, "2026-07-26T00:23:04.909Z");
        assert!(project.updated_at.ends_with('Z'));
    }

    /// The same folder cannot be registered twice, and the refusal carries the
    /// project standing in the way — which is the only thing that lets the
    /// caller tell the user *which* project that is.
    #[test]
    fn a_second_project_on_the_same_folder_is_refused() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        match fixture.add("project-2") {
            Insert::Occupied { existing, conflict } => {
                assert_eq!(conflict, Conflict::WorkspaceRoot);
                assert_eq!(existing.id, "project-1");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }

        assert_eq!(
            fixture.database.registry().expect("reads").projects.len(),
            1
        );
    }

    /// Reusing an id is a distinct refusal from reusing a folder. Both are
    /// "already here", but only one of them is the user's mistake.
    #[test]
    fn a_reused_id_is_refused_and_says_so() {
        let fixture = Fixture::new();
        let other = tempfile::tempdir().expect("a second temporary directory");
        let other_root =
            WorkspaceRoot::check(&other.path().to_string_lossy()).expect("accepted");

        fixture.add("project-1");
        let refusal = fixture
            .database
            .insert_project(
                "project-1",
                "elsewhere",
                &other_root,
                Some("2026-07-26T00:00:00.000Z"),
                fixture.sequences.commit().sequence(),
            )
            .expect("the insert reaches the database");

        match refusal {
            Insert::Occupied { existing, conflict } => {
                assert_eq!(conflict, Conflict::Id);
                assert_eq!(existing.workspace_root, fixture.root.display());
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// A refused insert must leave the log where it was. If it advanced, a
    /// client would be told something changed when nothing had.
    ///
    /// The *number* it was offered is spent either way — see [`Sequences`], where
    /// gaps are the accepted cost of a counter that does not touch a disk — but
    /// what the registry reports is the last number something actually happened
    /// at, and that is what a client compares against.
    #[test]
    fn a_refused_insert_does_not_advance_the_sequence() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        let before = fixture.database.registry().expect("reads").sequence;

        fixture.add("project-2");

        assert_eq!(fixture.database.registry().expect("reads").sequence, before);
    }

    #[test]
    fn removing_a_project_takes_it_off_the_registry_and_advances_the_log() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        let removal = fixture.remove("project-1");
        // The folder comes back with the sequence. It is what the caller
        // releases the project's watcher and held scan by, and it is only
        // readable while the row still exists — so if it were resolved after
        // the delete rather than before it, there would be nothing to resolve.
        assert_eq!(
            removal,
            Removal::Committed {
                sequence: 2,
                canonical_root: fixture.root.canonical().to_string(),
            }
        );

        assert!(fixture
            .database
            .registry()
            .expect("reads")
            .projects
            .is_empty());
    }

    /// Removing something that is not there is agreement, not an error — and
    /// it must not move the log, because nothing happened. It answers with the
    /// sequence the registry is already at rather than with the number it was
    /// offered, so a client that waits for that event is not left waiting.
    #[test]
    fn removing_an_unregistered_project_changes_nothing() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        let removal = fixture.remove("never-registered");
        assert_eq!(removal, Removal::Absent(1));
        assert_eq!(removal.sequence(), 1);
        assert_eq!(
            fixture.database.registry().expect("reads").projects.len(),
            1
        );
    }

    /// Once a folder is removed it can be added again. This is why removal is
    /// a deleted row rather than a flag on a kept one: the uniqueness rule that
    /// stops duplicates would otherwise also stop a legitimate re-add.
    #[test]
    fn a_removed_folder_can_be_registered_again() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture.remove("project-1");

        assert!(matches!(fixture.add("project-2"), Insert::Committed { .. }));
        assert_eq!(
            fixture.database.registry().expect("reads").projects[0].id,
            "project-2"
        );
    }

    /// The ticket's "survives a server restart", at the storage seam: the file
    /// is opened twice with nothing shared but the path.
    #[test]
    fn a_registry_reopened_from_the_same_file_is_the_same_registry() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("nested").join("state.sqlite");
        let workspace = WorkspaceRoot::check(&directory.path().to_string_lossy())
            .expect("accepted");

        // A number well past the first, as a live server's would be: the counter
        // is shared with changes that never reach this database, so what is
        // stored has to be the number the write was given rather than a count of
        // writes.
        let sequence = {
            let database = Database::open(&path).expect("creates the database and its directory");
            let insert = database
                .insert_project(
                    "project-1",
                    "project",
                    &workspace,
                    Some("2026-07-26T00:23:04.909Z"),
                    41,
                )
                .expect("registers");
            sequence(insert)
        };
        assert_eq!(sequence, 41);
        assert!(path.exists(), "the database file was not created");

        let reopened = Database::open(&path).expect("opens the existing database");
        let registry = reopened.registry().expect("reads");
        assert_eq!(registry.sequence, sequence);
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].id, "project-1");

        // And the next boot carries on from there rather than from zero, which
        // is the whole reason the number is stored at all.
        let resumed = Sequences::resuming(&reopened).expect("a resumed log");
        assert_eq!(resumed.current(), 41);
        assert_eq!(resumed.commit().sequence(), 42);
    }

    /// Taking a number holds the log, and that is the whole ordering guarantee:
    /// a second writer cannot take its number — let alone publish — until the
    /// first has announced what it took.
    ///
    /// Without it a project committed on a socket's read loop and a thread event
    /// published by an agent's driver could reach the one feed they share in the
    /// opposite order to their numbers, and a client drops anything at or below
    /// the sequence it holds — so the lower of the two would be lost rather than
    /// reordered.
    #[test]
    fn a_number_is_not_handed_out_while_the_last_one_is_still_being_announced() {
        let sequences = Sequences::from(0);
        let held = sequences.commit();
        assert_eq!(held.sequence(), 1);

        let waiting = sequences.clone();
        let second = std::thread::spawn(move || waiting.commit().sequence());

        // Long enough that a counter without the lock would have finished.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !second.is_finished(),
            "a second writer took a number while the first was still announcing"
        );

        drop(held);
        assert_eq!(second.join().expect("the second writer finishes"), 2);
        assert_eq!(sequences.current(), 2);
    }

    /// Two commits that took their numbers in one order and reached the database
    /// in the other must not leave the stored high-water mark behind the higher
    /// of them — the next boot resumes from it, and a lowered mark would re-issue
    /// a number a committed change had already used.
    #[test]
    fn a_commit_never_lowers_the_stored_high_water_mark() {
        let fixture = Fixture::new();

        fixture
            .database
            .insert_project("later", "later", &fixture.root, None, 9)
            .expect("registers");
        let second = tempfile::tempdir().expect("a second temporary directory");
        let elsewhere =
            WorkspaceRoot::check(&second.path().to_string_lossy()).expect("accepted");
        fixture
            .database
            .insert_project("earlier", "earlier", &elsewhere, None, 4)
            .expect("registers");

        assert_eq!(fixture.database.registry().expect("reads").sequence, 9);
    }

    /// Opening an already-migrated database must not try to migrate it again —
    /// `CREATE TABLE` without `IF NOT EXISTS` would fail loudly if it did, and
    /// this test is what makes that failure a feature rather than a trap.
    #[test]
    fn opening_an_existing_database_applies_no_migration_twice() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");

        for _ in 0..3 {
            Database::open(&path).expect("opens without re-applying the schema");
        }
    }

    /// A file written by a newer lightcode is refused rather than guessed at.
    /// Downgrading and silently ignoring tables you do not understand is how a
    /// registry loses rows.
    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");
        Database::open(&path).expect("creates the database");

        let connection = Connection::open(&path).expect("opens directly");
        connection
            .pragma_update(None, "user_version", MIGRATIONS.len() as u32 + 1)
            .expect("bumps the schema version past this build");
        drop(connection);

        let failure = Database::open(&path).expect_err("a newer schema is refused");
        assert!(
            failure.to_string().contains("newer version"),
            "the refusal must say why: {failure}"
        );
    }

    // -- conversations --------------------------------------------------------

    fn a_thread(id: &str, project_id: &str) -> ThreadRow {
        ThreadRow {
            id: id.to_string(),
            project_id: project_id.to_string(),
            title: "A conversation".to_string(),
            model_selection: serde_json::json!({
                "instanceId": "claudeAgent",
                "model": "claude-opus-5",
            }),
            runtime_mode: "full-access".to_string(),
            interaction_mode: "default".to_string(),
            branch: None,
            worktree_path: None,
            agent_session_id: None,
            latest_turn: None,
            latest_user_message_at: None,
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
        }
    }

    /// A conversation comes back as what was written, field for field. Everything
    /// ticket 11 promises rests on this being exact — a restored thread that
    /// differed from the live one would show a developer a different conversation
    /// depending on when they opened it.
    #[test]
    fn a_stored_conversation_comes_back_as_what_was_written() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        let mut thread = a_thread("thread-1", "project-1");
        thread.latest_turn = Some(LatestTurn {
            turn_id: "turn-1".to_string(),
            state: "completed",
            requested_at: "2026-07-26T00:23:05.000Z".to_string(),
            started_at: Some("2026-07-26T00:23:05.100Z".to_string()),
            completed_at: Some("2026-07-26T00:23:07.108Z".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
        });
        thread.latest_user_message_at = Some("2026-07-26T00:23:05.000Z".to_string());
        thread.agent_session_id = Some("session-alpha".to_string());

        let messages = vec![
            Message {
                id: "message-1".to_string(),
                role: "user".to_string(),
                text: "the question".to_string(),
                turn_id: Some("turn-1".to_string()),
                streaming: false,
                created_at: "2026-07-26T00:23:05.000Z".to_string(),
                updated_at: "2026-07-26T00:23:05.000Z".to_string(),
            },
            Message {
                id: "assistant-1".to_string(),
                role: "assistant".to_string(),
                text: "the answer".to_string(),
                turn_id: Some("turn-1".to_string()),
                streaming: false,
                created_at: "2026-07-26T00:23:06.000Z".to_string(),
                updated_at: "2026-07-26T00:23:07.000Z".to_string(),
            },
        ];
        let activities = vec![Activity {
            id: "activity-1".to_string(),
            tone: "info",
            kind: "turn.completed".to_string(),
            summary: "Turn completed in 2.0s · $0.0795 · end_turn".to_string(),
            payload: serde_json::json!({"durationMs": 2008, "isError": false}),
            turn_id: Some("turn-1".to_string()),
            created_at: "2026-07-26T00:23:07.108Z".to_string(),
        }];

        let mut writes = vec![Write::Thread(Box::new(thread.clone()))];
        for (ordinal, message) in messages.iter().enumerate() {
            writes.push(Write::Message {
                thread_id: "thread-1".to_string(),
                ordinal,
                message: message.clone(),
            });
        }
        writes.push(Write::Activity {
            thread_id: "thread-1".to_string(),
            ordinal: 0,
            activity: activities[0].clone(),
        });
        fixture.database.transcribe(&writes).expect("stores");

        assert_eq!(
            fixture.database.conversations().expect("reads"),
            vec![Conversation {
                thread,
                messages,
                activities,
            }]
        );
    }

    /// The one column that arrives from somewhere else. The agent's `init` line
    /// lands mid-turn, while every other change is rewriting the whole row beside
    /// it, so it is its own statement — and a later row write must not lose it.
    #[test]
    fn the_agents_session_survives_the_row_being_rewritten() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        let thread = a_thread("thread-1", "project-1");

        fixture
            .database
            .transcribe(&[
                Write::Thread(Box::new(thread.clone())),
                Write::AgentSession {
                    thread_id: "thread-1".to_string(),
                    session_id: "session-alpha".to_string(),
                },
            ])
            .expect("stores");

        // The next change to the conversation writes the row it has, which now
        // carries the session — so the round trip is the live thread's copy of it
        // rather than the column being overwritten with nothing.
        let stored = &fixture.database.conversations().expect("reads")[0].thread;
        assert_eq!(stored.agent_session_id, Some("session-alpha".to_string()));

        fixture
            .database
            .transcribe(&[Write::Thread(Box::new(stored.clone()))])
            .expect("stores");
        assert_eq!(
            fixture.database.conversations().expect("reads")[0]
                .thread
                .agent_session_id,
            Some("session-alpha".to_string())
        );
    }

    /// Deleting a project takes its conversations with it, by the schema's own
    /// cascade. Which conversations there *were* is not this layer's answer —
    /// `crate::threads` holds the live view and the stored rows are a subset of it
    /// — so what is checked here is only that nothing is left behind.
    #[test]
    fn removing_a_project_removes_its_conversations() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture
            .database
            .transcribe(&[
                Write::Thread(Box::new(a_thread("thread-1", "project-1"))),
                Write::Thread(Box::new(a_thread("thread-2", "project-1"))),
                Write::Message {
                    thread_id: "thread-1".to_string(),
                    ordinal: 0,
                    message: Message {
                        id: "message-1".to_string(),
                        role: "user".to_string(),
                        text: "hello".to_string(),
                        turn_id: Some("turn-1".to_string()),
                        streaming: false,
                        created_at: "2026-07-26T00:23:05.000Z".to_string(),
                        updated_at: "2026-07-26T00:23:05.000Z".to_string(),
                    },
                },
            ])
            .expect("stores");
        assert_eq!(fixture.database.conversations().expect("reads").len(), 2);

        assert!(matches!(
            fixture.remove("project-1"),
            Removal::Committed { .. }
        ));
        assert!(fixture
            .database
            .conversations()
            .expect("reads")
            .is_empty());
    }

    /// A session recorded against a thread that is not there is refused rather
    /// than succeeding at updating nothing. The stored session is what a restart
    /// resumes into, so an update that quietly matched no row is continuity lost
    /// with nothing said about it.
    #[test]
    fn a_session_for_a_thread_that_does_not_exist_is_refused() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        let refusal = fixture
            .database
            .transcribe(&[Write::AgentSession {
                thread_id: "never-created".to_string(),
                session_id: "session-alpha".to_string(),
            }])
            .expect_err("there is no such thread");
        assert!(
            refusal.to_string().contains("never-created")
                && refusal.to_string().contains("session-alpha"),
            "{refusal}"
        );
    }

    /// A conversation cannot be stored against a project that is not there. The
    /// foreign key is what makes the cascade above possible, so this is the other
    /// half of the same decision rather than a separate rule.
    #[test]
    fn a_conversation_needs_a_project_to_belong_to() {
        let fixture = Fixture::new();

        fixture
            .database
            .transcribe(&[Write::Thread(Box::new(a_thread("thread-1", "never-registered")))])
            .expect_err("there is no such project");
        assert!(fixture
            .database
            .conversations()
            .expect("reads")
            .is_empty());
    }

    /// The registry's timestamps are the contract's `IsoDateTime`, and the
    /// captures show milliseconds. Nothing parses this back, so the format is
    /// only ever as right as the test that pins it.
    #[test]
    fn timestamps_are_millisecond_iso_in_utc() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        let updated_at = &fixture.database.registry().expect("reads").projects[0].updated_at;

        // 2026-07-26T00:23:04.909Z
        assert_eq!(updated_at.len(), 24, "{updated_at} is not the captured shape");
        assert_eq!(&updated_at[4..5], "-");
        assert_eq!(&updated_at[10..11], "T");
        assert_eq!(&updated_at[19..20], ".");
        assert!(updated_at.ends_with('Z'));
    }
}
