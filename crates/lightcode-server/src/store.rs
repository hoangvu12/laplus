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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, Row};

use crate::projects::{Project, WorkspaceRoot};

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
