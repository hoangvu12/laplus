//! The database: laplus's durable state, and the only file that speaks SQL.
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
use rusqlite::types::Type;

use crate::orchestration::{
    DEFAULT_PROVIDER_INTERACTION_MODE, DEFAULT_RUNTIME_MODE, INTERACTION_MODES, RUNTIME_MODES,
};
use crate::pairing::{CredentialRefusal, Grant, PairingLink, Session, WebSocketTicket};
use crate::projects::{Project, WorkspaceRoot};
use crate::threads::{Activity, Checkpoint, Conversation, LatestTurn, Message, ThreadRow};
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
        -- Retained for rows written before provider resume cursors. Current
        -- readers expose a value here as the owning driver's v0 string cursor;
        -- current writes leave it NULL and use the provider-owned columns added
        -- by the later continuation migration.
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
    // v3 — ticket 12, the position the client sorts the work log by.
    r#"
    -- The sequence the activity was announced under. `ordinal` is already the
    -- position in this server's own list; this is the number the *client* orders
    -- the work log by (`session-logic.ts`, `compareActivitiesByOrder`), and the
    -- reason it has to be stored rather than derived is that the client's fallback
    -- is a millisecond timestamp. See `crate::threads::Activity::sequence`.
    --
    -- Nullable, and rows written before this migration keep a NULL: they are older
    -- than anything this build will number, and the client sorts an activity with
    -- no sequence ahead of one with — which is where they belong.
    ALTER TABLE thread_activities ADD COLUMN sequence INTEGER;
    "#,
    // v4 — ticket 20, the turns a diff can be taken between.
    r#"
    -- One row per turn that finished, naming the git ref `crate::checkpoints`
    -- recorded the working tree under. The *tree* is in the developer's own
    -- repository and outlives this file entirely; what is here is only the name
    -- of it, so that a conversation which came back from a restart can still
    -- offer its turns to the diff panel.
    CREATE TABLE thread_checkpoints (
        thread_id      TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
        -- The primary key with the thread, and it is the *turn* rather than the
        -- turn count because that is the client's own key
        -- (`threadReducer.ts`, `case "thread.turn-diff-completed"`). A turn
        -- captured twice has to replace its row here for the same reason it
        -- replaces its entry there: one turn is one row in the panel's list.
        turn_id        TEXT NOT NULL,
        turn_count     INTEGER NOT NULL,
        checkpoint_ref TEXT NOT NULL,
        status         TEXT NOT NULL,
        -- The per-file summary, verbatim. Same reasoning as `payload` on an
        -- activity: it is a list whose shape belongs to the contract, and
        -- nothing here ever queries into it.
        files          TEXT NOT NULL,
        assistant_message_id TEXT,
        completed_at   TEXT NOT NULL,
        PRIMARY KEY (thread_id, turn_id)
    ) STRICT;
    "#,
    // v5 — ticket 73, the credentials a phone pairs with.
    //
    // Follows `031_AuthAuthorizationScopes.ts` in the reference server rather
    // than the `020_AuthAccessManagement.ts` the ticket cites: 031 drops and
    // recreates both tables to add `scopes`, and is the shape upstream actually
    // runs. The columns this server has no use for are left out — `role` (031
    // replaced it with `scopes`), the DPoP thumbprint, and most of the client
    // metadata, which belongs to a client-session list ticket 73 puts out of
    // scope.
    r#"
    CREATE TABLE auth_pairing_links (
        id          TEXT PRIMARY KEY,
        -- UNIQUE is what makes a code single-use enforceable rather than merely
        -- checked for: two rows sharing one would make `consumed_at` ambiguous.
        credential  TEXT NOT NULL UNIQUE,
        method      TEXT NOT NULL,
        -- The granted scopes as a JSON array, verbatim. Same reasoning as
        -- `model_selection` on a thread: nothing here ever queries into it, and
        -- a join table would buy a shape this file has to keep in step with the
        -- contract's for no query it enables. The *wire* form is
        -- space-delimited (RFC 6749) and `crate::pairing` converts.
        scopes      TEXT NOT NULL,
        subject     TEXT NOT NULL,
        label       TEXT,
        created_at  TEXT NOT NULL,
        expires_at  TEXT NOT NULL,
        -- Both nullable, and both are the whole of single use: `consume` is one
        -- conditional UPDATE that sets `consumed_at` only where it is still
        -- NULL, so two simultaneous redemptions cannot both find it so.
        consumed_at TEXT,
        revoked_at  TEXT,
        -- Zero for every code a human is handed, and 1 for the one the desktop
        -- window is booted with.
        --
        -- Upstream's `remainingUses: "unbounded"`
        -- (`PairingGrantStore.ts:314-330`), narrowed to the one case that needs
        -- it. The window re-reads its credential out of the page URL on every
        -- reload, so a strictly single-use boot grant would let the developer
        -- press F5 once and then lock them out of their own window. A code
        -- carried to a phone is the opposite: it is read aloud off a screen,
        -- and the second use of one is somebody who should not have it.
        --
        -- An INTEGER rather than a nullable count, because there is no third
        -- answer. "How many uses are left" would be a number this server never
        -- decrements and never reads back.
        reusable    INTEGER NOT NULL DEFAULT 0
    ) STRICT;

    CREATE INDEX auth_pairing_links_active
        ON auth_pairing_links (revoked_at, consumed_at, expires_at);

    CREATE TABLE auth_sessions (
        session_id  TEXT PRIMARY KEY,
        -- Upstream has no such column: it signs its sessions and reads the id
        -- back out of the token. Ticket 73 chose rows over signatures — a row
        -- makes revocation a single UPDATE and needs no secret to keep — and a
        -- row has to hold the token it is looked up by. Plaintext, for the same
        -- reason the pairing code is: see `crate::pairing::PairingLink`.
        token       TEXT NOT NULL UNIQUE,
        subject     TEXT NOT NULL,
        scopes      TEXT NOT NULL,
        method      TEXT NOT NULL,
        label       TEXT,
        issued_at   TEXT NOT NULL,
        expires_at  TEXT NOT NULL,
        revoked_at  TEXT
    ) STRICT;

    CREATE INDEX auth_sessions_active
        ON auth_sessions (revoked_at, expires_at, issued_at);

    -- A ticket is a session, narrowed to one upgrade and five minutes, because
    -- the browser's WebSocket API cannot set a header and the credential has to
    -- ride in the query string. See `crate::pairing`.
    CREATE TABLE auth_websocket_tickets (
        ticket      TEXT PRIMARY KEY,
        -- Revoking a session takes its outstanding tickets with it, which is
        -- what makes "revoke" mean revoked rather than "revoked in five
        -- minutes".
        session_id  TEXT NOT NULL REFERENCES auth_sessions(session_id) ON DELETE CASCADE,
        issued_at   TEXT NOT NULL,
        expires_at  TEXT NOT NULL,
        consumed_at TEXT
    ) STRICT;

    CREATE INDEX auth_websocket_tickets_by_session
        ON auth_websocket_tickets (session_id);
    "#,
    // v6 — ticket 75, the key an asset URL is signed with.
    //
    // A table rather than a column on something, because what it holds is not
    // about any row this database already has: it is a server-lifetime secret
    // that happens to need somewhere durable, and the next one — there will be
    // one — belongs beside it rather than in a second table of one.
    //
    // Note what is *not* here. `crate::assets` does not store a row per issued
    // URL; the URL carries its own signed claims, and this is only the key they
    // are signed with. That is the opposite of ticket 73's decision one
    // migration above, and the module says why.
    r#"
    CREATE TABLE server_secrets (
        name       TEXT PRIMARY KEY,
        -- Raw bytes, not hex or base64: this column is never read by anything
        -- but the code that wrote it, and an encoding would be a second thing
        -- to agree about.
        secret     BLOB NOT NULL,
        created_at TEXT NOT NULL
    ) STRICT;
    "#,
    // v7 — ticket 06 of the headless-Linux effort, the name this server answers
    // to.
    //
    // **Not a row in `server_secrets` one migration above**, though the shape of
    // the get-or-create is the same. That column's own comment says it is never
    // read by anything but the code that wrote it, and this value is the
    // opposite: it is published unauthenticated in the environment descriptor,
    // printed in a boot line and read off a settings list. A secret and a
    // published name sharing a table would make that comment false for half its
    // rows.
    //
    // Here rather than in a file beside the database, because the id should
    // share the database's lifetime. A lost `state.sqlite` is every session,
    // every pairing and every thread gone, and every client re-pairing anyway —
    // so a new id costs nothing that was not already lost. A separate file could
    // outlive the database or be restored without it, and the id would then name
    // a server whose sessions it no longer matches.
    r#"
    CREATE TABLE environment (
        -- Exactly one row, the way `orchestration` above is one row: this is
        -- what is true of the whole database rather than of anything in it.
        id             INTEGER PRIMARY KEY CHECK (id = 0),
        -- `<machine>-<suffix>`, minted by `crate::config::fresh_environment_id`.
        -- Written once and never updated: a client stores a profile under this
        -- name, so changing it silently un-pairs everything that had paired.
        environment_id TEXT NOT NULL,
        created_at     TEXT NOT NULL
    ) STRICT;
    "#,
    // v8 — ticket 01 of the thread-lifecycle effort, where a conversation sits
    // in the developer's inbox.
    //
    // Six columns rather than a second table, per ADR-0002: these are properties
    // of the thread, and a thread is one row. See `crate::threads::Lifecycle`,
    // which is the shape they are read and published as.
    //
    // **Nullable with no default**, which is the whole point of the migration
    // rather than an omission from it. NULL already means "never happened"
    // everywhere else in this table, so a row written before today reads back
    // indistinguishable from a thread nobody has archived, settled, snoozed or
    // deleted — and `ALTER TABLE ADD COLUMN` fills the existing rows with
    // exactly that.
    //
    // No index. The project list is a few dozen conversations read whole on
    // every snapshot, and this database sorts and joins on ids and timestamps
    // and nothing else — an index here would be for a query nobody writes.
    r#"
    ALTER TABLE threads ADD COLUMN archived_at      TEXT;
    -- One of the contract's two literals, `settled` or `active`, and read back
    -- through `crate::threads::settled_override` so that anything else is no
    -- override rather than a value the client cannot decode.
    ALTER TABLE threads ADD COLUMN settled_override TEXT;
    ALTER TABLE threads ADD COLUMN settled_at       TEXT;
    -- When the conversation comes back on its own. There is no timer behind
    -- this: a snooze expires by being read, in the client. See `Lifecycle`.
    ALTER TABLE threads ADD COLUMN snoozed_until    TEXT;
    ALTER TABLE threads ADD COLUMN snoozed_at       TEXT;
    -- Deleting is soft, so this is the whole of what makes a thread deleted —
    -- the row, its transcript and its checkpoints all stay, and the git refs a
    -- turn wrote are not orphaned.
    ALTER TABLE threads ADD COLUMN deleted_at       TEXT;
    "#,
    // v9 — codex-driver ticket 02, the provider a conversation belongs to.
    // Every older row necessarily ran under the only driver the server had, so
    // the defaults are a migration of known history rather than a guess.
    r#"
    ALTER TABLE threads ADD COLUMN provider_instance_id TEXT NOT NULL DEFAULT 'claudeAgent';
    ALTER TABLE threads ADD COLUMN provider_driver      TEXT NOT NULL DEFAULT 'claudeAgent';
    "#,
    // Provider-owned continuation data. Binding columns remain outside the
    // opaque JSON so storage can enforce ownership without interpreting it.
    // The v2 `agent_session_id` column remains only as a read-time v0 source;
    // current inserts and updates write NULL there and persist continuation in
    // these columns.
    r#"
    ALTER TABLE threads ADD COLUMN provider_resume_cursor TEXT;
    ALTER TABLE threads ADD COLUMN cursor_provider_instance_id TEXT;
    ALTER TABLE threads ADD COLUMN cursor_provider_driver TEXT;
    "#,
    // External public endpoints are configuration, not discovered network
    // state. One environment can expose one operator-owned Cloudflare hostname
    // in this first slice, so the singleton row makes replacement atomic.
    r#"
    CREATE TABLE external_tunnel_endpoint (
        id                       INTEGER PRIMARY KEY CHECK (id = 0),
        https_origin             TEXT NOT NULL,
        verification_state       TEXT NOT NULL,
        failure_kind             TEXT,
        failure_message          TEXT,
        last_attempt_at          TEXT,
        last_verified_at         TEXT,
        created_at               TEXT NOT NULL,
        updated_at               TEXT NOT NULL
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

/// The same rendering as [`NOW`], displaced by a lifetime.
///
/// The modifier is interpolated rather than bound, because SQLite takes a
/// modifier as a literal and not as a parameter. That is safe here and only
/// here: every caller passes one of [`crate::pairing`]'s own `&'static str`
/// constants, and nothing user-supplied reaches it. `expiry_and_now_agree`
/// pins that this stays the same clock as [`NOW`].
fn expiry(ttl: &str) -> String {
    format!("strftime('%Y-%m-%dT%H:%M:%fZ','now','{ttl}')")
}

/// The pairing link table's columns, in the order [`pairing_link_from_row`]
/// reads them.
const PAIRING_LINK_COLUMNS: &str = "id, credential, scopes, subject, label, created_at, expires_at";

/// What [`Database::issue_pairing_link`] needs. A struct rather than six
/// arguments, because four of them are strings and an argument list of four
/// strings is a bug waiting for a refactor to reorder it.
#[derive(Debug, Clone, Copy)]
pub struct NewPairingLink<'a> {
    pub id: &'a str,
    pub credential: &'a str,
    pub method: &'a str,
    pub scopes: &'a [String],
    pub subject: &'a str,
    pub label: Option<&'a str>,
    /// How long this code lives. Five minutes for one a human carries;
    /// [`crate::pairing::DESKTOP_BOOT_TTL`] for the window's own.
    pub ttl: crate::pairing::Ttl<'a>,
    /// Survives being spent. True for the boot grant and false for everything
    /// else — see the `reusable` column's note in the migration.
    pub reusable: bool,
}

/// What [`Database::issue_session`] needs.
#[derive(Debug, Clone, Copy)]
pub struct NewSession<'a> {
    pub session_id: &'a str,
    pub token: &'a str,
    pub subject: &'a str,
    pub scopes: &'a [String],
    pub method: &'a str,
    pub label: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTunnelEndpoint {
    pub https_origin: String,
    pub verification_state: String,
    pub failure_kind: Option<String>,
    pub failure_message: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_verified_at: Option<String>,
}

/// Scopes go into their column as a JSON array. See the migration's note on
/// why this column is not a join table.
fn encode_scope_column(scopes: &[String]) -> String {
    serde_json::to_string(scopes).expect("a list of strings serializes")
}

/// The other half of [`encode_scope_column`].
///
/// A column that will not parse yields no scopes rather than an error. Nothing
/// in this server gates on a scope — see [`crate::pairing`] — so the choice is
/// between a session that works and reports nothing and a session that cannot
/// be read at all, and the second one locks out a paired phone over a display
/// string.
fn decode_scope_column(encoded: &str) -> Vec<String> {
    serde_json::from_str(encoded).unwrap_or_default()
}

fn pairing_link_from_row(row: &Row<'_>) -> rusqlite::Result<PairingLink> {
    Ok(PairingLink {
        id: row.get(0)?,
        credential: row.get(1)?,
        scopes: decode_scope_column(&row.get::<_, String>(2)?),
        subject: row.get(3)?,
        label: row.get(4)?,
        created_at: row.get(5)?,
        expires_at: row.get(6)?,
    })
}

/// The thread table's columns, in the order [`thread_from_row`] reads them.
const THREAD_COLUMNS: &str = "id, project_id, title, model_selection, runtime_mode, \
     interaction_mode, branch, worktree_path, agent_session_id, latest_turn, \
     latest_user_message_at, created_at, updated_at, archived_at, settled_override, \
     settled_at, snoozed_until, snoozed_at, deleted_at, provider_instance_id, \
     provider_driver, provider_resume_cursor, cursor_provider_instance_id, \
     cursor_provider_driver";

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

    /// Whether a client's `afterSequence` cursor says it holds everything this
    /// server has — in which case the replay it asked for is a replay of no
    /// events, and the subscription may open without a snapshot.
    ///
    /// **Equality, and nothing weaker.** A cursor *behind* this number is a
    /// replay this server cannot perform, having no log of events to replay
    /// from; a cursor *ahead* of it is not a client that is somehow early but a
    /// client holding a number from a previous run of this server, because the
    /// counter is seeded from the last *durable* write and every number issued
    /// after it was reissued at the next boot. Both want the same answer, and
    /// it is the one this server gave every cursor before ADR-0016: the whole
    /// snapshot, which replaces whatever the client holds.
    ///
    /// Safe to ask more than once, which matters because
    /// [`crate::subscriptions::EventSource::describe`] is called again whenever
    /// a subscriber falls a whole backlog behind. Falling behind is *itself*
    /// what makes a cursor stale: every event on every feed carries a number
    /// taken from here, so a subscriber that missed some cannot still be equal
    /// to the watermark. The re-description is a snapshot without needing to
    /// remember that it is the second one.
    pub fn caught_up(&self, cursor: Option<i64>) -> bool {
        cursor.is_some_and(|cursor| cursor == self.current())
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

/// What happened to an attempt to rename a project.
///
/// Not a third variant on [`Insert`]: an insert is refused because something is
/// already there, and a rename can only be refused because nothing is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rename {
    /// Written, and the log advanced to this sequence. Carries the row as it now
    /// stands, for [`Insert::Committed`]'s reason — `updated_at` is the
    /// database's, and the caller has to publish the row rather than a guess at
    /// it.
    Committed { sequence: i64, project: Project },
    /// Nothing is registered under that id, so nothing was renamed.
    ///
    /// Carries no sequence, unlike [`Removal::Absent`]. A repeated *delete*
    /// describes a world the server already agrees with, so answering it with
    /// the sequence the registry is at is honest; a rename of a project that is
    /// not there asks for a change that will never happen, and the caller
    /// refuses it by name.
    Absent,
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
    pub fn external_tunnel_endpoint(&self) -> Result<Option<ExternalTunnelEndpoint>, StorageError> {
        self.lock()
            .query_row(
                "SELECT https_origin, verification_state, failure_kind, failure_message, \
                 last_attempt_at, last_verified_at FROM external_tunnel_endpoint WHERE id = 0",
                [],
                |row| Ok(ExternalTunnelEndpoint {
                    https_origin: row.get(0)?,
                    verification_state: row.get(1)?,
                    failure_kind: row.get(2)?,
                    failure_message: row.get(3)?,
                    last_attempt_at: row.get(4)?,
                    last_verified_at: row.get(5)?,
                }),
            )
            .optional()
            .map_err(StorageError::while_("read the external tunnel endpoint"))
    }

    pub fn register_external_tunnel_endpoint(&self, origin: &str) -> Result<(), StorageError> {
        self.lock().execute(
            &format!(
                "INSERT INTO external_tunnel_endpoint \
                 (id, https_origin, verification_state, created_at, updated_at) \
                 VALUES (0, ?1, 'pending', {NOW}, {NOW}) \
                 ON CONFLICT(id) DO UPDATE SET https_origin = excluded.https_origin, \
                 verification_state = CASE WHEN https_origin = excluded.https_origin \
                     THEN verification_state ELSE 'pending' END, \
                 failure_kind = CASE WHEN https_origin = excluded.https_origin \
                     THEN failure_kind ELSE NULL END, \
                 failure_message = CASE WHEN https_origin = excluded.https_origin \
                     THEN failure_message ELSE NULL END, \
                 last_attempt_at = CASE WHEN https_origin = excluded.https_origin \
                     THEN last_attempt_at ELSE NULL END, \
                 last_verified_at = CASE WHEN https_origin = excluded.https_origin \
                     THEN last_verified_at ELSE NULL END, updated_at = {NOW}"
            ),
            [origin],
        ).map(|_| ()).map_err(StorageError::while_("register the external tunnel endpoint"))
    }

    pub fn forget_external_tunnel_endpoint(&self) -> Result<(), StorageError> {
        self.lock().execute("DELETE FROM external_tunnel_endpoint WHERE id = 0", [])
            .map(|_| ()).map_err(StorageError::while_("forget the external tunnel endpoint"))
    }

    pub fn record_external_tunnel_verification(
        &self,
        origin: &str,
        verified: bool,
        failure_kind: Option<&str>,
        failure_message: Option<&str>,
    ) -> Result<bool, StorageError> {
        self.lock().execute(
            &format!(
                "UPDATE external_tunnel_endpoint SET verification_state = ?1, failure_kind = ?2, \
                 failure_message = ?3, last_attempt_at = {NOW}, \
                 last_verified_at = CASE WHEN ?1 = 'verified' THEN {NOW} ELSE last_verified_at END, \
                 updated_at = {NOW} WHERE id = 0 AND https_origin = ?4"
            ),
            rusqlite::params![if verified { "verified" } else { "failed" }, failure_kind, failure_message, origin],
        ).map(|changed| changed == 1).map_err(StorageError::while_("record external tunnel verification"))
    }

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

        // **There can be two processes on this file.** Since
        // `laplus-server auth pairing create` (see [`crate::codes`]), a second
        // process opens the same database while a server is running and writes
        // a row to it. SQLite's default is to fail a busy write *immediately*
        // rather than wait, so without this the command would report
        // `database is locked` whenever it landed during a turn — a failure
        // that depends on timing, appears only under load, and is indeed how
        // this class of bug is usually met.
        //
        // Five seconds is far longer than any write here takes and far shorter
        // than a person's patience. The server benefits by the same rule: it
        // now waits for the CLI rather than the other way round.
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(StorageError::while_("set the busy timeout"))?;

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

    /// Give a project a new name.
    ///
    /// The title and `updated_at` and nothing else. A project's folder is its
    /// identity — `canonical_root` is what "the same project" is answered by, and
    /// what everything the server holds *about* a project is keyed by — so a
    /// rename cannot collide with anything and needs no uniqueness check.
    ///
    /// `updated_at` is the database's clock rather than the client's, the same
    /// way [`Database::insert_project`] sets it: the field describes when the row
    /// changed, and a client with a skewed clock would otherwise reorder the
    /// project list. `created_at` is left alone, because a rename is not a
    /// creation.
    ///
    /// Read back inside the transaction for the reason the insert does it: the
    /// timestamp is the database's, so this is the only account of the project
    /// that is not a guess — and it is what the caller publishes.
    pub fn rename_project(&self, id: &str, title: &str, at: i64) -> Result<Rename, StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("rename the project"))?;

        let renamed = transaction
            .execute(
                &format!("UPDATE projects SET title = ?2, updated_at = {NOW} WHERE id = ?1"),
                (id, title),
            )
            .map_err(StorageError::while_("rename the project"))?;
        // Dropped rather than committed, so the registry is left exactly as it
        // was — there is nothing to stamp, and stamping would advance the log for
        // a change that did not happen.
        if renamed == 0 {
            return Ok(Rename::Absent);
        }

        let project = find_project(&transaction, "id", id)?.ok_or_else(|| {
            StorageError::refusing(
                "rename the project",
                format!("the row for '{id}' was not there immediately after renaming it"),
            )
        })?;

        stamp(&transaction, at)?;
        transaction
            .commit()
            .map_err(StorageError::while_("rename the project"))?;
        Ok(Rename::Committed {
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
                Write::ProviderResumeCursor { thread_id, cursor } =>
                    remember_provider_resume_cursor(&transaction, thread_id, cursor)?,
                Write::Checkpoint {
                    thread_id,
                    checkpoint,
                } => upsert_checkpoint(&transaction, thread_id, checkpoint)?,
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
            "SELECT thread_id, id, tone, kind, summary, payload, turn_id, sequence, created_at \
             FROM thread_activities ORDER BY thread_id ASC, ordinal ASC",
            activity_from_row,
            "read the work logs",
        )?;
        // Ordered by the turn count itself rather than by a stored position,
        // because unlike a message or an activity a checkpoint *has* a natural
        // order: it is which turn it is.
        let checkpoints: Vec<(String, Checkpoint)> = query(
            &transaction,
            "SELECT thread_id, turn_id, turn_count, checkpoint_ref, status, files, \
                    assistant_message_id, completed_at \
             FROM thread_checkpoints ORDER BY thread_id ASC, turn_count ASC",
            checkpoint_from_row,
            "read the checkpoints",
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
                checkpoints: Vec::new(),
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
        for (thread_id, checkpoint) in checkpoints {
            if let Some(index) = at.get(&thread_id) {
                conversations[*index].checkpoints.push(checkpoint);
            }
        }

        Ok(conversations)
    }

    // --- ticket 73: pairing links, sessions and socket tickets ---------------
    //
    // The vocabulary here is the pairing flow's, not the table's, for the same
    // reason the registry's is: `crate::server`'s handlers ask to mint a code or
    // spend one, and never see a statement. What is unusual about this group is
    // that its *concurrency* is load-bearing rather than incidental — see
    // [`Database::consume_pairing_link`] — so the SQL is where the single-use
    // guarantee lives and cannot be moved out of.

    /// Mint a pairing code. The row is the code's whole existence: there is no
    /// in-memory half.
    pub fn issue_pairing_link(
        &self,
        input: NewPairingLink<'_>,
    ) -> Result<PairingLink, StorageError> {
        let connection = self.lock();
        let scopes = encode_scope_column(input.scopes);

        connection
            .execute(
                &format!(
                    "INSERT INTO auth_pairing_links \
                       (id, credential, method, scopes, subject, label, created_at, expires_at, \
                        reusable) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, {NOW}, {expiry}, ?7)",
                    expiry = expiry(input.ttl.0)
                ),
                rusqlite::params![
                    input.id,
                    input.credential,
                    input.method,
                    scopes,
                    input.subject,
                    input.label,
                    input.reusable,
                ],
            )
            .map_err(StorageError::while_("mint a pairing code"))?;

        connection
            .query_row(
                &format!("SELECT {PAIRING_LINK_COLUMNS} FROM auth_pairing_links WHERE id = ?1"),
                (input.id,),
                pairing_link_from_row,
            )
            .map_err(StorageError::while_("read back the pairing code"))
    }

    /// Spend a pairing code, or say why not.
    ///
    /// **The order of the two statements is the single-use guarantee**, and it
    /// is the reference server's (`PairingGrantStore.consume`). The conditional
    /// `UPDATE … RETURNING` runs *first* and does the whole check — unrevoked,
    /// unspent, unexpired — inside one statement, so two redemptions racing each
    /// other cannot both find `consumed_at` NULL. Only on a miss does the second
    /// statement look at the row to say which of the three it was, and by then
    /// the answer is only a log line. Checking first and updating second would
    /// read the same and be a race.
    ///
    /// **A `reusable` row is exempt from the spending, not from the checking.**
    /// `consumed_at` is still stamped — so Settings stops listing it and the
    /// user is not offered a code that is not theirs to hand out — but it is not
    /// what bars a second use. Revocation and expiry still are, which is what
    /// keeps the boot grant a credential rather than a permanent hole: it dies
    /// with its TTL and can be revoked like anything else. See the column's own
    /// note in the migration.
    pub fn consume_pairing_link(
        &self,
        credential: &str,
    ) -> Result<Result<Grant, CredentialRefusal>, StorageError> {
        let connection = self.lock();

        let consumed = connection
            .query_row(
                &format!(
                    "UPDATE auth_pairing_links \
                        SET consumed_at = {NOW} \
                      WHERE credential = ?1 \
                        AND revoked_at IS NULL \
                        AND (consumed_at IS NULL OR reusable = 1) \
                        AND expires_at > {NOW} \
                  RETURNING subject, scopes, label"
                ),
                (credential,),
                |row| {
                    Ok(Grant {
                        subject: row.get(0)?,
                        scopes: decode_scope_column(&row.get::<_, String>(1)?),
                        label: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::while_("spend a pairing code"))?;

        if let Some(grant) = consumed {
            return Ok(Ok(grant));
        }

        // Diagnosis only. Nothing below decides anything the caller acts on
        // differently — all four become the same 401 — so a row that changed
        // under us between the two statements costs an inaccurate log line and
        // nothing else.
        let refusal = connection
            .query_row(
                &format!(
                    "SELECT revoked_at, consumed_at, expires_at <= {NOW}, reusable \
                       FROM auth_pairing_links WHERE credential = ?1"
                ),
                (credential,),
                |row| {
                    let revoked: Option<String> = row.get(0)?;
                    let consumed: Option<String> = row.get(1)?;
                    let expired: bool = row.get(2)?;
                    let reusable: bool = row.get(3)?;
                    Ok(if revoked.is_some() {
                        CredentialRefusal::Revoked
                    // A spent `reusable` row is not why the UPDATE missed — it
                    // is exempt from that clause — so saying "already used"
                    // here would send whoever reads the log looking for a
                    // second redemption that never happened.
                    } else if consumed.is_some() && !reusable {
                        CredentialRefusal::AlreadyUsed
                    } else if expired {
                        CredentialRefusal::Expired
                    } else {
                        // Nothing was wrong with it, which means it was spent
                        // between the two statements above.
                        CredentialRefusal::AlreadyUsed
                    })
                },
            )
            .optional()
            .map_err(StorageError::while_("diagnose a pairing code"))?
            .unwrap_or(CredentialRefusal::Unknown);

        Ok(Err(refusal))
    }

    /// The pairing codes Settings should list: minted, still good, not yet
    /// spent — and issued to be handed to somebody.
    ///
    /// **The boot grant is excluded**, which is upstream's
    /// `listPairingLinks({ excludeSubjects })` and matters for the same reason.
    /// It is a credential the window issued to itself; offering it in a list
    /// headed "codes you can give a device" invites the developer to hand out
    /// the one that unlocks their own window, and to revoke it wondering why
    /// laplus then stopped opening. It is filtered by subject rather than by
    /// `reusable` so that the list is defined by *who a code is for*, which is
    /// the question the panel is actually asking.
    pub fn active_pairing_links(&self) -> Result<Vec<PairingLink>, StorageError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare(&format!(
                "SELECT {PAIRING_LINK_COLUMNS} FROM auth_pairing_links \
                  WHERE revoked_at IS NULL AND consumed_at IS NULL AND expires_at > {NOW} \
                    AND subject <> '{boot}' \
                  ORDER BY created_at DESC",
                boot = crate::pairing::DESKTOP_BOOT_SUBJECT
            ))
            .map_err(StorageError::while_("list pairing codes"))?;
        let rows = statement
            .query_map([], pairing_link_from_row)
            .map_err(StorageError::while_("list pairing codes"))?;
        rows.collect::<rusqlite::Result<Vec<PairingLink>>>()
            .map_err(StorageError::while_("list pairing codes"))
    }

    /// Mint the credential the desktop window boots with, and take the previous
    /// one out of circulation.
    ///
    /// One call rather than an insert beside a revoke, because the two must not
    /// be able to half-happen: a process that seeded a new grant and failed to
    /// retire the old one would leave yesterday's boot code live for its full
    /// day, and a process that retired the old one and failed to seed a new one
    /// would open a window it cannot let in.
    ///
    /// **Retiring the old one is why this is not simply `issue_pairing_link`.**
    /// The boot grant outlives a page reload by design, which means it also
    /// outlives the process unless something ends it — and a laptop that has
    /// opened laplus fifty times should not have fifty live keys to itself
    /// sitting in a table.
    pub fn issue_desktop_boot_grant(
        &self,
        id: &str,
        credential: &str,
        scopes: &[String],
    ) -> Result<(), StorageError> {
        let mut connection = self.lock();
        let transaction = connection
            .transaction()
            .map_err(StorageError::while_("open the desktop boot grant"))?;

        transaction
            .execute(
                &format!(
                    "UPDATE auth_pairing_links SET revoked_at = {NOW} \
                      WHERE subject = ?1 AND revoked_at IS NULL"
                ),
                (crate::pairing::DESKTOP_BOOT_SUBJECT,),
            )
            .map_err(StorageError::while_("retire the previous boot grant"))?;

        transaction
            .execute(
                &format!(
                    "INSERT INTO auth_pairing_links \
                       (id, credential, method, scopes, subject, label, created_at, expires_at, \
                        reusable) \
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, {NOW}, {expiry}, 1)",
                    expiry = expiry(crate::pairing::DESKTOP_BOOT_TTL.0)
                ),
                rusqlite::params![
                    id,
                    credential,
                    crate::pairing::ONE_TIME_TOKEN_METHOD,
                    encode_scope_column(scopes),
                    crate::pairing::DESKTOP_BOOT_SUBJECT,
                ],
            )
            .map_err(StorageError::while_("mint the desktop boot grant"))?;

        transaction
            .commit()
            .map_err(StorageError::while_("commit the desktop boot grant"))
    }

    /// Withdraw a pairing code that is still capable of being spent. `false` if
    /// there was no such live code, which is what the contract's `revoked` field
    /// reports.
    ///
    /// **A spent `reusable` row can still be revoked**, and the same clause that
    /// exempts it from [`Database::consume_pairing_link`] has to exempt it here.
    /// Without that, stamping the boot grant on first use would make it
    /// permanently unrevokable — the credential with the longest life would be
    /// the one nothing could withdraw, which is precisely backwards.
    pub fn revoke_pairing_link(&self, id: &str) -> Result<bool, StorageError> {
        let connection = self.lock();
        let changed = connection
            .execute(
                &format!(
                    "UPDATE auth_pairing_links SET revoked_at = {NOW} \
                      WHERE id = ?1 AND revoked_at IS NULL \
                        AND (consumed_at IS NULL OR reusable = 1)"
                ),
                (id,),
            )
            .map_err(StorageError::while_("revoke a pairing code"))?;
        Ok(changed > 0)
    }

    /// Open a session against a spent pairing code's grant.
    pub fn issue_session(&self, input: NewSession<'_>) -> Result<Session, StorageError> {
        let connection = self.lock();
        let scopes = encode_scope_column(input.scopes);
        let ttl = crate::pairing::SESSION_TTL.0;

        connection
            .execute(
                &format!(
                    "INSERT INTO auth_sessions \
                       (session_id, token, subject, scopes, method, label, issued_at, expires_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, {NOW}, {expiry})",
                    expiry = expiry(ttl)
                ),
                rusqlite::params![
                    input.session_id,
                    input.token,
                    input.subject,
                    scopes,
                    input.method,
                    input.label,
                ],
            )
            .map_err(StorageError::while_("open a session"))?;

        connection
            .query_row(
                // `expires_in` is computed here rather than in Rust so that it
                // and `expires_at` come from one reading of the clock. A client
                // that was told "30 days" and given a timestamp 40 ms earlier
                // would refresh 40 ms late forever.
                "SELECT session_id, token, subject, scopes, expires_at, \
                        CAST(strftime('%s', expires_at) - strftime('%s','now') AS INTEGER) \
                   FROM auth_sessions WHERE session_id = ?1",
                (input.session_id,),
                |row| {
                    Ok(Session {
                        session_id: row.get(0)?,
                        token: row.get(1)?,
                        subject: row.get(2)?,
                        scopes: decode_scope_column(&row.get::<_, String>(3)?),
                        expires_at: row.get(4)?,
                        expires_in: row.get(5)?,
                    })
                },
            )
            .map_err(StorageError::while_("read back the session"))
    }

    /// A named server secret, minted on first use and the same one after that.
    ///
    /// Get-or-create in one statement rather than a read, a branch and a write:
    /// two connections asking at once — two windows opening together, which is
    /// the ordinary case — would otherwise both find nothing, both insert, and
    /// one would lose. `ON CONFLICT DO NOTHING` followed by the read means the
    /// loser reads the winner's key instead of failing, and every caller gets
    /// the same bytes.
    ///
    /// The consequence of getting this wrong is quiet: every URL signed with
    /// the key that lost stops verifying, and a sidebar full of icons turns
    /// into a sidebar full of 404s an hour later.
    pub fn secret_or_create(&self, name: &str, bytes: usize) -> Result<Vec<u8>, StorageError> {
        let mut fresh = vec![0u8; bytes];
        getrandom::fill(&mut fresh).map_err(|error| {
            StorageError::refusing(
                "mint a server secret",
                format!("randomness is unavailable: {error}"),
            )
        })?;

        let connection = self.lock();
        connection
            .execute(
                &format!(
                    "INSERT INTO server_secrets (name, secret, created_at) \
                     VALUES (?1, ?2, {NOW}) ON CONFLICT (name) DO NOTHING"
                ),
                (name, &fresh),
            )
            .map_err(StorageError::while_("store a server secret"))?;

        connection
            .query_row(
                "SELECT secret FROM server_secrets WHERE name = ?1",
                (name,),
                |row| row.get(0),
            )
            .map_err(StorageError::while_("read a server secret"))
    }

    /// What this laplus calls itself, minted on first use and the same one after
    /// that.
    ///
    /// **This is the identity a client files this server under.** The client's
    /// connection registry is one slot per environment id
    /// (`packages/client-runtime/src/connection/registry.ts`), so an id shared
    /// with another laplus is not a cosmetic collision: the second server to
    /// register is dropped, silently, and the user is shown "No saved remote
    /// environments" after a pairing that succeeded at every step. That is
    /// ticket 06 of the headless-Linux effort, and it is what this function
    /// exists to prevent.
    ///
    /// Get-or-create in one statement rather than a read, a branch and a write,
    /// exactly as [`Database::secret_or_create`] does and for its reason: two
    /// handles on one file — two windows opening together — would otherwise both
    /// find nothing, both insert, and one would lose. `ON CONFLICT DO NOTHING`
    /// and then a read means the loser reads the winner's name instead of
    /// failing.
    ///
    /// The row is never updated. [`crate::Server::bind_with`] is the caller, and
    /// what it does with the answer is settle it into the config the descriptor
    /// is serialized from.
    pub fn environment_id_or_create(&self) -> Result<String, StorageError> {
        let fresh = crate::config::fresh_environment_id().map_err(|error| {
            StorageError::refusing("name this environment", error.to_string())
        })?;

        let connection = self.lock();
        connection
            .execute(
                &format!(
                    "INSERT INTO environment (id, environment_id, created_at) \
                     VALUES (0, ?1, {NOW}) ON CONFLICT (id) DO NOTHING"
                ),
                (&fresh,),
            )
            .map_err(StorageError::while_("store this environment's name"))?;

        connection
            .query_row(
                "SELECT environment_id FROM environment WHERE id = 0",
                [],
                |row| row.get(0),
            )
            .map_err(StorageError::while_("read this environment's name"))
    }

    /// Is this bearer a live session? The subject and scopes if so.
    pub fn verify_session(&self, token: &str) -> Result<Option<Grant>, StorageError> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "SELECT subject, scopes, label FROM auth_sessions \
                      WHERE token = ?1 AND revoked_at IS NULL AND expires_at > {NOW}"
                ),
                (token,),
                |row| {
                    Ok(Grant {
                        subject: row.get(0)?,
                        scopes: decode_scope_column(&row.get::<_, String>(1)?),
                        label: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::while_("verify a session"))
    }

    /// Mint a socket ticket against a live session.
    ///
    /// Takes the bearer rather than a session id so that the caller cannot mint
    /// a ticket for a session it did not prove it holds — the verification and
    /// the issue are one statement pair under one lock, and `None` means the
    /// bearer was no good.
    pub fn issue_websocket_ticket(
        &self,
        token: &str,
        ticket: &str,
    ) -> Result<Option<WebSocketTicket>, StorageError> {
        let connection = self.lock();
        let ttl = crate::pairing::WEBSOCKET_TICKET_TTL.0;

        let session_id: Option<String> = connection
            .query_row(
                &format!(
                    "SELECT session_id FROM auth_sessions \
                      WHERE token = ?1 AND revoked_at IS NULL AND expires_at > {NOW}"
                ),
                (token,),
                |row| row.get(0),
            )
            .optional()
            .map_err(StorageError::while_("find the session to ticket"))?;

        let Some(session_id) = session_id else {
            return Ok(None);
        };

        connection
            .execute(
                &format!(
                    "INSERT INTO auth_websocket_tickets (ticket, session_id, issued_at, expires_at) \
                     VALUES (?1, ?2, {NOW}, {expiry})",
                    expiry = expiry(ttl)
                ),
                (ticket, &session_id),
            )
            .map_err(StorageError::while_("mint a socket ticket"))?;

        connection
            .query_row(
                "SELECT ticket, expires_at FROM auth_websocket_tickets WHERE ticket = ?1",
                (ticket,),
                |row| {
                    Ok(WebSocketTicket {
                        ticket: row.get(0)?,
                        expires_at: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::while_("read back the socket ticket"))
    }

    /// Spend a socket ticket at the upgrade.
    ///
    /// Single use and the same shape as [`Database::consume_pairing_link`], for
    /// the same reason: one conditional `UPDATE` rather than a read and a write,
    /// so a ticket replayed from a log by someone who saw it cannot open a
    /// second socket. The join is what makes revoking a session take its
    /// outstanding tickets with it rather than leaving five minutes of them
    /// valid.
    pub fn consume_websocket_ticket(&self, ticket: &str) -> Result<Option<Grant>, StorageError> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "UPDATE auth_websocket_tickets SET consumed_at = {NOW} \
                      WHERE ticket = ?1 \
                        AND consumed_at IS NULL \
                        AND expires_at > {NOW} \
                        AND session_id IN ( \
                              SELECT session_id FROM auth_sessions \
                               WHERE revoked_at IS NULL AND expires_at > {NOW}) \
                  RETURNING ( \
                      SELECT subject FROM auth_sessions \
                       WHERE auth_sessions.session_id = auth_websocket_tickets.session_id), \
                      ( SELECT scopes FROM auth_sessions \
                         WHERE auth_sessions.session_id = auth_websocket_tickets.session_id), \
                      ( SELECT label FROM auth_sessions \
                         WHERE auth_sessions.session_id = auth_websocket_tickets.session_id)"
                ),
                (ticket,),
                |row| {
                    Ok(Grant {
                        subject: row.get(0)?,
                        scopes: decode_scope_column(&row.get::<_, String>(1)?),
                        label: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(StorageError::while_("spend a socket ticket"))
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
                 it was written by a newer version of laplus",
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
                latest_user_message_at, created_at, updated_at, archived_at, \
                settled_override, settled_at, snoozed_until, snoozed_at, deleted_at, \
                provider_instance_id, provider_driver, provider_resume_cursor, \
                cursor_provider_instance_id, cursor_provider_driver) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24) \
             ON CONFLICT (id) DO UPDATE SET \
                project_id = excluded.project_id, \
                title = excluded.title, \
                model_selection = excluded.model_selection, \
                runtime_mode = excluded.runtime_mode, \
                interaction_mode = excluded.interaction_mode, \
                branch = excluded.branch, \
                worktree_path = excluded.worktree_path, \
                agent_session_id = NULL, \
                latest_turn = excluded.latest_turn, \
                latest_user_message_at = excluded.latest_user_message_at, \
                updated_at = excluded.updated_at, \
                archived_at = excluded.archived_at, \
                settled_override = excluded.settled_override, \
                settled_at = excluded.settled_at, \
                snoozed_until = excluded.snoozed_until, \
                snoozed_at = excluded.snoozed_at, \
                deleted_at = excluded.deleted_at, \
                provider_instance_id = excluded.provider_instance_id, \
                provider_driver = excluded.provider_driver, \
                provider_resume_cursor = excluded.provider_resume_cursor, \
                cursor_provider_instance_id = excluded.cursor_provider_instance_id, \
                cursor_provider_driver = excluded.cursor_provider_driver",
            rusqlite::params![
                thread.id,
                thread.project_id,
                thread.title,
                thread.model_selection.to_string(),
                thread.runtime_mode,
                thread.interaction_mode,
                thread.branch,
                thread.worktree_path,
                Option::<&str>::None,
                thread.latest_turn.as_ref().map(|turn| turn.to_value().to_string()),
                thread.latest_user_message_at,
                thread.created_at,
                thread.updated_at,
                // The six travel with the rest of the row rather than in a
                // statement of their own: they arrive from `crate::threads` on the same in-memory
                // conversation every other column here comes from, so an update
                // that left them behind would be the row disagreeing with itself.
                thread.lifecycle.archived_at,
                thread.lifecycle.settled_override,
                thread.lifecycle.settled_at,
                thread.lifecycle.snoozed_until,
                thread.lifecycle.snoozed_at,
                thread.lifecycle.deleted_at,
                thread.provider.instance_id,
                thread.provider.driver,
                thread.provider_resume_cursor.as_ref().map(|cursor| cursor.value.to_string()),
                thread.provider_resume_cursor.as_ref().map(|cursor| &cursor.provider.instance_id),
                thread.provider_resume_cursor.as_ref().map(|cursor| &cursor.provider.driver),
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
                (thread_id, id, ordinal, tone, kind, summary, payload, turn_id, sequence, \
                 created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
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
                activity.sequence,
                activity.created_at,
            ],
        )
        .map_err(StorageError::while_("store the activity"))?;
    Ok(())
}

/// Record what the working tree looked like when a turn finished.
///
/// An upsert on `(thread_id, turn_id)`, which is the whole of what makes a turn
/// captured twice one row rather than two — see the table's own comment.
fn upsert_checkpoint(
    transaction: &Transaction<'_>,
    thread_id: &str,
    checkpoint: &Checkpoint,
) -> Result<(), StorageError> {
    let files: Vec<serde_json::Value> = checkpoint
        .files
        .iter()
        .map(crate::checkpoints::Changed::to_value)
        .collect();
    transaction
        .execute(
            "INSERT INTO thread_checkpoints \
                (thread_id, turn_id, turn_count, checkpoint_ref, status, files, \
                 assistant_message_id, completed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (thread_id, turn_id) DO UPDATE SET \
                turn_count = excluded.turn_count, \
                checkpoint_ref = excluded.checkpoint_ref, \
                status = excluded.status, \
                files = excluded.files, \
                assistant_message_id = excluded.assistant_message_id, \
                completed_at = excluded.completed_at",
            rusqlite::params![
                thread_id,
                checkpoint.turn_id,
                checkpoint.turn_count as i64,
                checkpoint.reference,
                checkpoint.status,
                serde_json::Value::Array(files).to_string(),
                checkpoint.assistant_message_id,
                checkpoint.completed_at,
            ],
        )
        .map_err(StorageError::while_("store the checkpoint"))?;
    Ok(())
}

fn remember_provider_resume_cursor(
    transaction: &Transaction<'_>,
    thread_id: &str,
    cursor: &crate::provider::ResumeCursor,
) -> Result<(), StorageError> {
    let updated = transaction.execute(
        "UPDATE threads SET agent_session_id = NULL, provider_resume_cursor = ?2, cursor_provider_instance_id = ?3, cursor_provider_driver = ?4 WHERE id = ?1 AND provider_instance_id = ?3 AND provider_driver = ?4",
        rusqlite::params![thread_id, cursor.value.to_string(), cursor.provider.instance_id, cursor.provider.driver],
    ).map_err(StorageError::while_("store the provider resume cursor"))?;
    if updated == 0 {
        return Err(StorageError::refusing("store the provider resume cursor", format!("thread '{thread_id}' is not owned by provider instance '{}' ({})", cursor.provider.instance_id, cursor.provider.driver)));
    }
    Ok(())
}

fn thread_from_row(row: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    let model_selection: String = row.get(3)?;
    let runtime_mode: String = row.get(4)?;
    let interaction_mode: String = row.get(5)?;
    let latest_turn: Option<String> = row.get(9)?;
    let settled_override: Option<String> = row.get(14)?;
    let cursor_json: Option<String> = row.get(21)?;
    let cursor_instance: Option<String> = row.get(22)?;
    let cursor_driver: Option<String> = row.get(23)?;
    let provider = crate::provider::ProviderIdentity {
        instance_id: row.get(19)?,
        driver: row.get(20)?,
    };
    let legacy_cursor: Option<String> = row.get(8)?;
    let provider_resume_cursor = decode_provider_resume_cursor(
        cursor_json,
        cursor_instance,
        cursor_driver,
        legacy_cursor,
        &provider,
    )?;

    Ok(ThreadRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        provider,
        // A selection that will not parse is a row somebody edited by hand.
        // `null` decodes on the client as "no selection", which is a worse answer
        // than the stored one and a much better one than no conversation.
        model_selection: serde_json::from_str(&model_selection).unwrap_or(serde_json::Value::Null),
        runtime_mode: match RUNTIME_MODES.contains(&runtime_mode.as_str()) {
            true => runtime_mode,
            false => DEFAULT_RUNTIME_MODE.to_string(),
        },
        interaction_mode: match INTERACTION_MODES.contains(&interaction_mode.as_str()) {
            true => interaction_mode,
            false => DEFAULT_PROVIDER_INTERACTION_MODE.to_string(),
        },
        branch: row.get(6)?,
        worktree_path: row.get(7)?,
        provider_resume_cursor,
        latest_turn: latest_turn.as_deref().and_then(latest_turn_from_json),
        latest_user_message_at: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        lifecycle: crate::threads::Lifecycle {
            archived_at: row.get(13)?,
            settled_override: settled_override
                .as_deref()
                .and_then(crate::threads::settled_override),
            settled_at: row.get(15)?,
            snoozed_until: row.get(16)?,
            snoozed_at: row.get(17)?,
            deleted_at: row.get(18)?,
        },
    })
}

fn decode_provider_resume_cursor(
    encoded: Option<String>,
    instance_id: Option<String>,
    driver: Option<String>,
    legacy: Option<String>,
    thread_provider: &crate::provider::ProviderIdentity,
) -> rusqlite::Result<Option<crate::provider::ResumeCursor>> {
    let (encoded, instance_id, driver) = match (encoded, instance_id, driver) {
        (None, None, None) => return Ok(legacy.map(|value| crate::provider::ResumeCursor {
            provider: thread_provider.clone(),
            value: serde_json::Value::String(value),
        })),
        (Some(encoded), Some(instance_id), Some(driver)) => (encoded, instance_id, driver),
        _ => return Err(incompatible_cursor("provider resume cursor has incomplete ownership data")),
    };
    let provider = crate::provider::ProviderIdentity { instance_id, driver };
    if &provider != thread_provider {
        return Err(incompatible_cursor("provider resume cursor belongs to another provider instance"));
    }
    let value = serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(21, Type::Text, Box::new(error))
    })?;
    Ok(Some(crate::provider::ResumeCursor { provider, value }))
}

fn incompatible_cursor(message: &'static str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        21,
        Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, message)),
    )
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
        state: crate::settling::TurnState::from_stored(
            turn.get("state").and_then(serde_json::Value::as_str)?,
        ),
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
            sequence: row.get(7)?,
            created_at: row.get(8)?,
        },
    ))
}

fn checkpoint_from_row(row: &Row<'_>) -> rusqlite::Result<(String, Checkpoint)> {
    let turn_count: i64 = row.get(2)?;
    let status: String = row.get(4)?;
    let files: String = row.get(5)?;

    Ok((
        row.get(0)?,
        Checkpoint {
            turn_id: row.get(1)?,
            // A negative count cannot be written by this build and would be a
            // file somebody edited by hand. Zero is the baseline, which is the
            // one turn count that is always safe to claim.
            turn_count: turn_count.max(0) as u64,
            reference: row.get(3)?,
            status: crate::threads::checkpoint_status(&status),
            files: crate::checkpoints::changed_from_stored(&files),
            assistant_message_id: row.get(6)?,
            completed_at: row.get(7)?,
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

        fn rename(&self, id: &str, title: &str) -> Rename {
            self.database
                .rename_project(id, title, self.sequences.commit().sequence())
                .expect("the rename reaches the database")
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

    /// A rename writes the title and the row's own timestamp, keeps the folder
    /// and the creation date, and advances the log.
    ///
    /// The row comes back from the write rather than from a second read, because
    /// `updated_at` is the database's — so this is the only account of the project
    /// that is not a guess, and it is what the caller publishes.
    #[test]
    fn renaming_a_project_writes_the_title_and_advances_the_log() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        let before = fixture.database.project("project-1").expect("reads").expect("registered");

        let renamed = fixture.rename("project-1", "The developer's own name");
        let (sequence, project) = match renamed {
            Rename::Committed { sequence, project } => (sequence, project),
            other => panic!("expected a committed rename, got {other:?}"),
        };

        assert_eq!(sequence, 2);
        assert_eq!(project.title, "The developer's own name");
        // A rename is not a move and it is not a creation.
        assert_eq!(project.workspace_root, before.workspace_root);
        assert_eq!(project.canonical_root, before.canonical_root);
        assert_eq!(project.created_at, before.created_at);
        assert!(project.updated_at.ends_with('Z'));

        let registry = fixture.database.registry().expect("reads");
        assert_eq!(registry.sequence, 2);
        assert_eq!(registry.projects[0].title, "The developer's own name");
    }

    /// Renaming something that is not registered changes nothing and says so.
    ///
    /// Unlike a delete, which agrees with a client describing a world it already
    /// wanted: a rename of nothing asks for a change that will never happen, so
    /// the caller refuses it — and the log must not move, or a client would be
    /// told something changed when nothing had.
    #[test]
    fn renaming_an_unregistered_project_changes_nothing() {
        let fixture = Fixture::new();
        fixture.add("project-1");

        assert_eq!(fixture.rename("never-registered", "A name"), Rename::Absent);

        let registry = fixture.database.registry().expect("reads");
        assert_eq!(registry.sequence, 1);
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].title, "project");
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

    /// The cursor rule, at its three positions. Only one of them lets a
    /// subscription open without a snapshot, and it is the narrow one — see
    /// ADR-0016 for why the other two cannot be treated as caught up.
    ///
    /// The *ahead* case is the one worth stating out loud: a number this server
    /// has not reached is not a client running early, it is a client holding a
    /// number from a previous run, because the counter resumes from the last
    /// durable write and hands everything after it out again. That is why this
    /// is equality rather than `cursor >= current`, which would have looked
    /// like the more forgiving choice and would have left such a client
    /// rendering a conversation nothing was going to correct.
    #[test]
    fn only_a_cursor_that_is_exactly_current_is_caught_up() {
        let sequences = Sequences::from(0);
        drop(sequences.commit());
        drop(sequences.commit());
        assert_eq!(sequences.current(), 2);

        assert!(sequences.caught_up(Some(2)));
        assert!(!sequences.caught_up(Some(1)), "behind");
        assert!(!sequences.caught_up(Some(3)), "ahead: a previous run's number");
        assert!(!sequences.caught_up(None), "a client that holds nothing");

        // And the moment anything happens, the cursor that was current is not.
        drop(sequences.commit());
        assert!(!sequences.caught_up(Some(2)));
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

    /// The companion the appended migration needs: an existing database gains
    /// the six lifecycle columns, and the rows already in it read back as never
    /// archived, settled, snoozed or deleted.
    ///
    /// Built by applying the *released* migrations up to v7, which is what a
    /// file on a developer's disk actually is. A test that wrote the current
    /// schema by hand and then read it back would be asserting its own setup —
    /// the claim here is specifically that a database without these columns
    /// opens.
    ///
    /// **Pinned to 7 rather than `MIGRATIONS.len() - 1`.** The relative form
    /// reads better and quietly stops testing anything the moment a v9 is
    /// appended: it would then build a v8 database, which already has the six
    /// columns, and the assertion below would pass on a row that was never
    /// migrated at all.
    #[test]
    fn a_database_at_the_previous_version_gains_the_lifecycle_columns() {
        const BEFORE_THE_LIFECYCLE: usize = 7;

        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");

        {
            let connection = Connection::open(&path).expect("creates the file");
            for (index, statements) in MIGRATIONS.iter().take(BEFORE_THE_LIFECYCLE).enumerate() {
                connection
                    .execute_batch(&format!(
                        "BEGIN; {statements} PRAGMA user_version = {}; COMMIT;",
                        index + 1
                    ))
                    .expect("applies a released migration");
            }
            // Written by hand rather than through `Database`, which would apply
            // every migration and defeat the point. Only the columns v7 had.
            connection
                .execute(
                    "INSERT INTO projects \
                        (id, title, workspace_root, canonical_root, created_at, updated_at) \
                     VALUES ('project-1', 'A project', '/tmp/p', '/tmp/p', \
                        '2026-07-26T00:23:04.909Z', '2026-07-26T00:23:04.909Z')",
                    [],
                )
                .expect("a project from before today");
            connection
                .execute(
                    "INSERT INTO threads \
                        (id, project_id, title, model_selection, runtime_mode, \
                         interaction_mode, created_at, updated_at) \
                     VALUES ('thread-1', 'project-1', 'A conversation', \
                        '{\"instanceId\":\"claudeAgent\"}', 'full-access', 'default', \
                        '2026-07-26T00:23:04.909Z', '2026-07-26T00:23:04.909Z')",
                    [],
                )
                .expect("a conversation from before today");
        }

        let database = Database::open(&path).expect("migrates the existing file");
        let conversations = database.conversations().expect("reads them back");

        assert_eq!(conversations.len(), 1, "the migration lost a conversation");
        assert_eq!(
            conversations[0].thread.lifecycle,
            crate::threads::Lifecycle::default(),
            "a row older than the columns must be indistinguishable from a \
             thread nobody has archived, settled, snoozed or deleted"
        );
        assert_eq!(conversations[0].thread.title, "A conversation");
    }

    #[test]
    fn a_conversation_from_before_driver_identity_is_named_as_claude() {
        const BEFORE_DRIVER_IDENTITY: usize = 8;

        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");
        {
            let connection = Connection::open(&path).expect("creates the file");
            for (index, statements) in MIGRATIONS.iter().take(BEFORE_DRIVER_IDENTITY).enumerate() {
                connection
                    .execute_batch(&format!(
                        "BEGIN; {statements} PRAGMA user_version = {}; COMMIT;",
                        index + 1
                    ))
                    .expect("applies a released migration");
            }
            connection
                .execute_batch(
                    "INSERT INTO projects \
                        (id, title, workspace_root, canonical_root, created_at, updated_at) \
                     VALUES ('project-1', 'A project', '/tmp/p', '/tmp/p', \
                        '2026-07-26T00:23:04.909Z', '2026-07-26T00:23:04.909Z'); \
                     INSERT INTO threads \
                        (id, project_id, title, model_selection, runtime_mode, \
                         interaction_mode, created_at, updated_at) \
                     VALUES ('thread-1', 'project-1', 'A conversation', \
                        '{\"instanceId\":\"claudeAgent\"}', 'full-access', 'default', \
                        '2026-07-26T00:23:04.909Z', '2026-07-26T00:23:04.909Z');",
                )
                .expect("a conversation from before driver identity");
        }

        let database = Database::open(&path).expect("migrates the existing file");
        let conversations = database.conversations().expect("reads them back");

        assert_eq!(conversations[0].thread.provider.instance_id, "claudeAgent");
        assert_eq!(conversations[0].thread.provider.driver, "claudeAgent");
    }

    /// A file written by a newer laplus is refused rather than guessed at.
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

    // -- this server's own name -----------------------------------------------

    /// Ticket 06 of the headless-Linux effort, and its first acceptance
    /// criterion: a data directory reports the **same** id after a restart.
    ///
    /// This is the whole reason the id lives in the database rather than being
    /// computed at startup. A client pairs with `desktop-19eumeb-8f2a`, stores a
    /// profile under it, and comes back tomorrow: an id that was minted fresh
    /// each boot would leave every paired client holding the name of an
    /// environment that no longer exists.
    #[test]
    fn one_data_directory_keeps_its_environment_id_across_a_restart() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");

        let first = {
            let database = Database::open(&path).expect("creates the database");
            database.environment_id_or_create().expect("mints an id")
        };
        let second = {
            let database = Database::open(&path).expect("reopens the database");
            database.environment_id_or_create().expect("reads the id back")
        };

        assert_eq!(
            first, second,
            "a restart against the same file must not rename the environment"
        );
    }

    /// The second criterion: two data directories are two environments, which is
    /// the entire point of the ticket — the desktop's own backend and a remote
    /// one have to be able to sit in a client's registry at the same time, and
    /// that registry is one slot per id.
    #[test]
    fn two_data_directories_report_different_environment_ids() {
        let one = tempfile::tempdir().expect("a temporary directory");
        let other = tempfile::tempdir().expect("a second temporary directory");

        let first = Database::open(&one.path().join("state.sqlite"))
            .expect("creates one database")
            .environment_id_or_create()
            .expect("mints an id");
        let second = Database::open(&other.path().join("state.sqlite"))
            .expect("creates another database")
            .environment_id_or_create()
            .expect("mints an id");

        assert_ne!(
            first, second,
            "two laplus installations must not answer with one name"
        );
    }

    /// The third: two handles on one file agree, which is what the get-or-create
    /// statement is for. [`Database::secret_or_create`]'s own comment carries the
    /// reasoning — two windows opening together would otherwise both find
    /// nothing, both insert, and one would lose.
    ///
    /// **The two handles are held open at once**, rather than opened one after
    /// the other as the restart test does, because that is the arrangement the
    /// race needs: the loser has to read the winner's row instead of failing.
    #[test]
    fn two_database_handles_on_one_file_agree_on_the_environment_id() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("state.sqlite");

        let one = Database::open(&path).expect("creates the database");
        let other = Database::open(&path).expect("opens the same file again");

        let first = one.environment_id_or_create().expect("mints or reads");
        let second = other.environment_id_or_create().expect("mints or reads");

        assert_eq!(first, second);
    }

    /// The fourth: the shape. `^[a-z0-9][a-z0-9-]*$` and beginning with this
    /// machine's slug, because an id that lands in a URL and in a settings list
    /// is read by someone — the ticket's argument for `desktop-19eumeb-8f2a`
    /// over `8f2a41c9d3e7` is that there will be *several*.
    #[test]
    fn an_environment_id_is_this_machine_and_a_suffix() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let database =
            Database::open(&directory.path().join("state.sqlite")).expect("creates the database");

        let id = database.environment_id_or_create().expect("mints an id");

        assert!(
            id.starts_with(&crate::config::machine_slug()),
            "{id} should name this machine"
        );
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "{id} should be safe in a URL path segment"
        );
        assert!(!id.starts_with('-') && !id.ends_with('-'), "{id} has a bare edge");
    }

    // -- conversations --------------------------------------------------------

    fn a_thread(id: &str, project_id: &str) -> ThreadRow {
        ThreadRow {
            id: id.to_string(),
            project_id: project_id.to_string(),
            title: "A conversation".to_string(),
            provider: crate::provider::ProviderIdentity {
                instance_id: "codex-work".to_string(),
                driver: "codex".to_string(),
            },
            model_selection: serde_json::json!({
                "instanceId": "claudeAgent",
                "model": "claude-opus-5",
            }),
            runtime_mode: "full-access".to_string(),
            interaction_mode: "default".to_string(),
            branch: None,
            worktree_path: None,
            provider_resume_cursor: None,
            latest_turn: None,
            latest_user_message_at: None,
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
            lifecycle: crate::threads::Lifecycle::default(),
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
            state: crate::settling::TurnState::Completed,
            requested_at: "2026-07-26T00:23:05.000Z".to_string(),
            started_at: Some("2026-07-26T00:23:05.100Z".to_string()),
            completed_at: Some("2026-07-26T00:23:07.108Z".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
        });
        thread.latest_user_message_at = Some("2026-07-26T00:23:05.000Z".to_string());
        thread.provider_resume_cursor = Some(crate::provider::ResumeCursor {
            provider: thread.provider.clone(),
            value: serde_json::json!({"version": 1, "sessionId": "upstream-alpha", "opaque": [1, 2]}),
        });
        // All six set, and to six *different* values, so a column wired to the
        // wrong parameter index is a failure here rather than a coincidence.
        thread.lifecycle = crate::threads::Lifecycle {
            archived_at: Some("2026-07-26T01:00:00.000Z".to_string()),
            settled_override: Some("active"),
            settled_at: Some("2026-07-26T02:00:00.000Z".to_string()),
            snoozed_until: Some("2026-07-27T03:00:00.000Z".to_string()),
            snoozed_at: Some("2026-07-26T04:00:00.000Z".to_string()),
            deleted_at: Some("2026-07-26T05:00:00.000Z".to_string()),
        };

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
        // One of each tone this server writes, because the stored tone comes back
        // through [`crate::threads::tone`] and a value it did not map would come
        // back as `info` — a tool row that lost its affordances, restored silently
        // wrong.
        let activities = vec![
            Activity {
                id: "activity-1".to_string(),
                tone: "info",
                kind: "turn.completed".to_string(),
                summary: "Turn completed in 2.0s · $0.0795 · end_turn".to_string(),
                payload: serde_json::json!({"durationMs": 2008, "isError": false}),
                turn_id: Some("turn-1".to_string()),
                sequence: Some(7),
                created_at: "2026-07-26T00:23:07.108Z".to_string(),
            },
            Activity {
                id: "activity-2".to_string(),
                tone: "tool",
                kind: "tool.completed".to_string(),
                summary: "Tool call".to_string(),
                payload: serde_json::json!({
                    "itemType": "dynamic_tool_call",
                    "status": "failed",
                    "data": {"toolCallId": "toolu_1"},
                }),
                turn_id: Some("turn-1".to_string()),
                sequence: Some(8),
                created_at: "2026-07-26T00:23:07.109Z".to_string(),
            },
            Activity {
                id: "activity-3".to_string(),
                tone: "error",
                kind: "session.failed".to_string(),
                summary: "The agent could not be started.".to_string(),
                payload: serde_json::json!({"detail": "The agent could not be started."}),
                turn_id: None,
                // No sequence, which is what a row written before the column
                // existed comes back as.
                sequence: None,
                created_at: "2026-07-26T00:23:07.110Z".to_string(),
            },
        ];

        let mut writes = vec![Write::Thread(Box::new(thread.clone()))];
        for (ordinal, message) in messages.iter().enumerate() {
            writes.push(Write::Message {
                thread_id: "thread-1".to_string(),
                ordinal,
                message: message.clone(),
            });
        }
        for (ordinal, activity) in activities.iter().enumerate() {
            writes.push(Write::Activity {
                thread_id: "thread-1".to_string(),
                ordinal,
                activity: Box::new(activity.clone()),
            });
        }
        fixture.database.transcribe(&writes).expect("stores");

        assert_eq!(
            fixture.database.conversations().expect("reads"),
            vec![Conversation {
                thread,
                messages,
                activities,
                checkpoints: Vec::new(),
            }]
        );
    }

    #[test]
    fn unnameable_stored_modes_are_rounded_on_read_without_rewriting_the_row() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture
            .database
            .transcribe(&[Write::Thread(Box::new(a_thread("thread-1", "project-1")))])
            .expect("stores");
        fixture
            .database
            .lock()
            .execute(
                "UPDATE threads SET runtime_mode = ?1, interaction_mode = ?2 WHERE id = ?3",
                ("bypassPermissions", "planning", "thread-1"),
            )
            .expect("simulates a historical row with unnameable modes");

        let restored = fixture.database.conversations().expect("reads");

        assert_eq!(restored[0].thread.runtime_mode, "full-access");
        assert_eq!(restored[0].thread.interaction_mode, "default");
        let stored: (String, String) = fixture
            .database
            .lock()
            .query_row(
                "SELECT runtime_mode, interaction_mode FROM threads WHERE id = ?1",
                ["thread-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("reads the stored modes directly");
        assert_eq!(stored, ("bypassPermissions".to_string(), "planning".to_string()));
    }

    #[test]
    fn a_cursor_write_is_bound_to_the_threads_provider_instance() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture.database.transcribe(&[Write::Thread(Box::new(a_thread("thread-1", "project-1")))])
            .expect("stores thread");
        let wrong = crate::provider::ResumeCursor {
            provider: crate::provider::ProviderIdentity {
                instance_id: "codex-personal".to_string(),
                driver: "codex".to_string(),
            },
            value: serde_json::json!({"version": 1, "session": "wrong"}),
        };

        assert!(fixture.database.transcribe(&[Write::ProviderResumeCursor {
            thread_id: "thread-1".to_string(),
            cursor: wrong,
        }]).is_err());
        assert!(fixture.database.conversations().expect("reads")[0]
            .thread.provider_resume_cursor.is_none());
    }

    #[test]
    fn malformed_or_misowned_stored_cursors_are_incompatible_not_absent() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture.database.transcribe(&[Write::Thread(Box::new(a_thread("thread-1", "project-1")))])
            .expect("stores thread");

        fixture.database.lock().execute(
            "UPDATE threads SET provider_resume_cursor = '{', cursor_provider_instance_id = provider_instance_id, cursor_provider_driver = provider_driver WHERE id = 'thread-1'",
            [],
        ).expect("corrupts cursor");
        assert!(fixture.database.conversations().is_err(), "malformed cursor became absent");

        fixture.database.lock().execute(
            "UPDATE threads SET provider_resume_cursor = '{}', cursor_provider_instance_id = 'other' WHERE id = 'thread-1'",
            [],
        ).expect("misowns cursor");
        assert!(fixture.database.conversations().is_err(), "misowned cursor became absent");
    }

    #[test]
    fn a_legacy_session_column_is_read_as_the_owning_drivers_v0_cursor() {
        let fixture = Fixture::new();
        fixture.add("project-1");
        fixture.database.transcribe(&[Write::Thread(Box::new(a_thread("thread-1", "project-1")))])
            .expect("stores thread");
        fixture.database.lock().execute(
            "UPDATE threads SET agent_session_id = 'legacy-upstream-id' WHERE id = 'thread-1'",
            [],
        ).expect("simulates a historical row");

        let stored = fixture.database.conversations().expect("reads historical row");

        assert_eq!(stored[0].thread.provider_resume_cursor, Some(crate::provider::ResumeCursor {
            provider: stored[0].thread.provider.clone(),
            value: serde_json::Value::String("legacy-upstream-id".to_string()),
        }));
        let legacy_before_update: Option<String> = fixture.database.lock().query_row(
            "SELECT agent_session_id FROM threads WHERE id = 'thread-1'",
            [],
            |row| row.get(0),
        ).expect("reads legacy column directly");
        assert_eq!(legacy_before_update.as_deref(), Some("legacy-upstream-id"));

        fixture.database.transcribe(&[Write::ProviderResumeCursor {
            thread_id: "thread-1".to_string(),
            cursor: crate::provider::ResumeCursor {
                provider: stored[0].thread.provider.clone(),
                value: serde_json::json!({"version": 1, "threadId": "current-upstream-id"}),
            },
        }]).expect("replaces the legacy cursor through the current write path");

        let migrated: (Option<String>, Option<String>) = fixture.database.lock().query_row(
            "SELECT agent_session_id, provider_resume_cursor FROM threads WHERE id = 'thread-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("reads continuation columns directly");
        assert_eq!(
            migrated,
            (None, Some(r#"{"threadId":"current-upstream-id","version":1}"#.to_string()))
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

    // --- ticket 73 ----------------------------------------------------------

    /// [`expiry`] repeats [`NOW`]'s format string, which is the kind of
    /// duplication that rots. A zero-length lifetime has to render the same
    /// instant, or a pairing code's `created_at` and `expires_at` are being read
    /// off two different clocks.
    #[test]
    fn expiry_and_now_agree_on_the_shape_of_an_instant() {
        let database = Database::in_memory().expect("an in-memory database");
        let connection = database.lock();
        let (now, immediately): (String, String) = connection
            .query_row(
                &format!("SELECT {NOW}, {}", expiry("+0 seconds")),
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("both render");

        assert_eq!(now.len(), immediately.len());
        // Same second, whatever the millisecond did in between.
        assert_eq!(now[..19], immediately[..19]);
        assert!(immediately.ends_with('Z'));
    }

    fn pairing_database() -> Database {
        Database::in_memory().expect("an in-memory database")
    }

    fn mint(database: &Database, credential: &str) -> PairingLink {
        database
            .issue_pairing_link(NewPairingLink {
                id: &format!("link-{credential}"),
                credential,
                method: crate::pairing::ONE_TIME_TOKEN_METHOD,
                scopes: &crate::pairing::default_scopes(),
                subject: "pairing",
                label: Some("A phone"),
                ttl: crate::pairing::PAIRING_CODE_TTL,
                reusable: false,
            })
            .expect("the code reaches the database")
    }

    #[test]
    fn a_minted_code_carries_its_label_and_scopes_back() {
        let database = pairing_database();
        let link = mint(&database, "AAAABBBBCCCC");

        assert_eq!(link.credential, "AAAABBBBCCCC");
        assert_eq!(link.label.as_deref(), Some("A phone"));
        assert_eq!(link.scopes, crate::pairing::default_scopes());
        assert!(link.expires_at > link.created_at);
    }

    /// The case the whole flow rests on.
    #[test]
    fn a_fresh_code_is_spent_once_and_the_second_attempt_fails() {
        let database = pairing_database();
        mint(&database, "AAAABBBBCCCC");

        let first = database.consume_pairing_link("AAAABBBBCCCC").expect("reads");
        assert_eq!(
            first.expect("the first redemption succeeds").subject,
            "pairing"
        );

        let second = database.consume_pairing_link("AAAABBBBCCCC").expect("reads");
        assert_eq!(
            second.expect_err("the second redemption fails"),
            CredentialRefusal::AlreadyUsed
        );
    }

    /// The boot grant's whole reason for existing: the window re-reads its
    /// credential out of the page URL on every reload, so a strictly single-use
    /// one would let the developer press F5 once and lock themselves out of
    /// their own window.
    #[test]
    fn the_boot_grant_survives_being_spent() {
        let database = pairing_database();
        database
            .issue_desktop_boot_grant("boot", "BOOTBOOTBOOT", &crate::pairing::default_scopes())
            .expect("the boot grant reaches the database");

        for attempt in 1..=3 {
            let grant = database
                .consume_pairing_link("BOOTBOOTBOOT")
                .expect("reads")
                .unwrap_or_else(|refusal| {
                    panic!("redemption {attempt} was refused as {refusal:?}")
                });
            assert_eq!(grant.subject, crate::pairing::DESKTOP_BOOT_SUBJECT);
            assert_eq!(grant.scopes, crate::pairing::default_scopes());
        }
    }

    /// Exempt from *spending*, not from *revocation*. This is what keeps the
    /// boot grant a credential rather than a permanent hole in the door.
    #[test]
    fn a_revoked_boot_grant_stops_working_despite_being_reusable() {
        let database = pairing_database();
        database
            .issue_desktop_boot_grant("boot", "BOOTBOOTBOOT", &crate::pairing::default_scopes())
            .expect("mints");
        assert!(database.consume_pairing_link("BOOTBOOTBOOT").expect("reads").is_ok());

        assert!(database.revoke_pairing_link("boot").expect("revokes"));

        assert_eq!(
            database
                .consume_pairing_link("BOOTBOOTBOOT")
                .expect("reads")
                .expect_err("a revoked boot grant is refused"),
            CredentialRefusal::Revoked
        );
    }

    /// Booting again retires the previous grant, so a laptop that has opened
    /// laplus fifty times does not hold fifty live keys to itself.
    #[test]
    fn minting_a_boot_grant_retires_the_one_before_it() {
        let database = pairing_database();
        let scopes = crate::pairing::default_scopes();
        database.issue_desktop_boot_grant("boot-1", "FIRSTFIRST22", &scopes).expect("mints");
        database.issue_desktop_boot_grant("boot-2", "SECONDSECND3", &scopes).expect("mints");

        assert_eq!(
            database
                .consume_pairing_link("FIRSTFIRST22")
                .expect("reads")
                .expect_err("yesterday's boot code is dead"),
            CredentialRefusal::Revoked
        );
        assert!(database.consume_pairing_link("SECONDSECND3").expect("reads").is_ok());
    }

    /// Settings must never offer the window's own credential as something to
    /// hand to a phone — upstream's `listPairingLinks({ excludeSubjects })`. A
    /// developer who revoked it wondering what it was would stop being able to
    /// open laplus.
    #[test]
    fn the_boot_grant_is_absent_from_the_list_settings_shows() {
        let database = pairing_database();
        database
            .issue_desktop_boot_grant("boot", "BOOTBOOTBOOT", &crate::pairing::default_scopes())
            .expect("mints");
        mint(&database, "AAAABBBBCCCC");

        let listed = database.active_pairing_links().expect("lists");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].credential, "AAAABBBBCCCC");
    }

    /// A spent boot grant is not why a later refusal happened, so the log must
    /// not say "already used" — that would send whoever reads it looking for a
    /// second redemption that never occurred.
    #[test]
    fn a_spent_boot_grant_that_expires_is_diagnosed_as_expired() {
        let database = pairing_database();
        database
            .issue_desktop_boot_grant("boot", "BOOTBOOTBOOT", &crate::pairing::default_scopes())
            .expect("mints");
        assert!(database.consume_pairing_link("BOOTBOOTBOOT").expect("reads").is_ok());

        // Reach past the TTL rather than wait for it: this asserts on the
        // decision the code makes, never on elapsed wall-clock.
        database
            .lock()
            .execute(
                "UPDATE auth_pairing_links SET expires_at = '2000-01-01T00:00:00.000Z' \
                  WHERE id = 'boot'",
                [],
            )
            .expect("ages the row");

        assert_eq!(
            database
                .consume_pairing_link("BOOTBOOTBOOT")
                .expect("reads")
                .expect_err("an expired boot grant is refused"),
            CredentialRefusal::Expired
        );
    }

    #[test]
    fn a_code_that_was_never_minted_is_unknown() {
        let database = pairing_database();
        assert_eq!(
            database
                .consume_pairing_link("NEVERMINTEDX")
                .expect("reads")
                .expect_err("refused"),
            CredentialRefusal::Unknown
        );
    }

    /// Expiry is the database's own clock, so the way to test it is to write a
    /// row that is already old rather than to wait five minutes. Asserting on a
    /// decision, not on elapsed wall-clock — `server/CLAUDE.md`.
    #[test]
    fn an_expired_code_is_refused() {
        let database = pairing_database();
        mint(&database, "AAAABBBBCCCC");
        database
            .lock()
            .execute(
                "UPDATE auth_pairing_links SET expires_at = '2000-01-01T00:00:00.000Z'",
                [],
            )
            .expect("ages the code");

        assert_eq!(
            database
                .consume_pairing_link("AAAABBBBCCCC")
                .expect("reads")
                .expect_err("refused"),
            CredentialRefusal::Expired
        );
    }

    #[test]
    fn a_revoked_code_is_refused_and_leaves_the_list() {
        let database = pairing_database();
        let link = mint(&database, "AAAABBBBCCCC");

        assert!(database.revoke_pairing_link(&link.id).expect("revokes"));
        assert_eq!(
            database
                .consume_pairing_link("AAAABBBBCCCC")
                .expect("reads")
                .expect_err("refused"),
            CredentialRefusal::Revoked
        );
        assert!(database.active_pairing_links().expect("lists").is_empty());
    }

    /// Revoking twice is not an error, it is `false` — which is what the
    /// contract's `revoked` field reports and what a double-click produces.
    #[test]
    fn revoking_a_code_twice_reports_that_the_second_did_nothing() {
        let database = pairing_database();
        let link = mint(&database, "AAAABBBBCCCC");

        assert!(database.revoke_pairing_link(&link.id).expect("revokes"));
        assert!(!database.revoke_pairing_link(&link.id).expect("revokes"));
        assert!(!database.revoke_pairing_link("no-such-link").expect("revokes"));
    }

    /// Settings lists what can still be used, and nothing else.
    #[test]
    fn the_active_list_omits_spent_expired_and_revoked_codes() {
        let database = pairing_database();
        mint(&database, "AAAABBBBCCCC");
        mint(&database, "DDDDEEEEFFFF");
        let revoked = mint(&database, "GGGGHHHHJJJJ");
        mint(&database, "KKKKMMMMNNNN");

        database
            .consume_pairing_link("DDDDEEEEFFFF")
            .expect("reads")
            .expect("spent");
        database.revoke_pairing_link(&revoked.id).expect("revokes");
        database
            .lock()
            .execute(
                "UPDATE auth_pairing_links SET expires_at = '2000-01-01T00:00:00.000Z' \
                   WHERE credential = 'KKKKMMMMNNNN'",
                [],
            )
            .expect("ages one");

        let listed: Vec<String> = database
            .active_pairing_links()
            .expect("lists")
            .into_iter()
            .map(|link| link.credential)
            .collect();
        assert_eq!(listed, ["AAAABBBBCCCC"]);
    }

    /// **The race the ordering in [`Database::consume_pairing_link`] exists
    /// for.** Eight threads redeem one code; exactly one may win.
    ///
    /// This asserts on the count and not on timing, so it is not a test that
    /// passes because a machine was fast. A read-then-write implementation fails
    /// it, which is the whole point of writing it.
    #[test]
    fn concurrent_redemptions_produce_exactly_one_grant() {
        let database = Arc::new(pairing_database());
        mint(&database, "AAAABBBBCCCC");

        let granted = Arc::new(AtomicI64::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let database = Arc::clone(&database);
                let granted = Arc::clone(&granted);
                scope.spawn(move || {
                    if database
                        .consume_pairing_link("AAAABBBBCCCC")
                        .expect("reads")
                        .is_ok()
                    {
                        granted.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(granted.load(Ordering::SeqCst), 1);
    }

    fn session(database: &Database, token: &str) -> Session {
        database
            .issue_session(NewSession {
                session_id: &format!("session-{token}"),
                token,
                subject: "pairing",
                scopes: &crate::pairing::default_scopes(),
                method: crate::pairing::BEARER_SESSION_METHOD,
                label: Some("A phone"),
            })
            .expect("the session reaches the database")
    }

    #[test]
    fn a_session_verifies_by_its_bearer_and_reports_a_lifetime() {
        let database = pairing_database();
        let opened = session(&database, "bearer-1");

        // 30 days, give or take the second the test took.
        assert!(opened.expires_in > 29 * 24 * 60 * 60);
        assert!(opened.expires_in <= 30 * 24 * 60 * 60);
        assert_eq!(
            database
                .verify_session("bearer-1")
                .expect("reads")
                .expect("a live session")
                .subject,
            "pairing"
        );
        assert!(database.verify_session("bearer-2").expect("reads").is_none());
    }

    #[test]
    fn an_expired_session_does_not_verify() {
        let database = pairing_database();
        session(&database, "bearer-1");
        database
            .lock()
            .execute(
                "UPDATE auth_sessions SET expires_at = '2000-01-01T00:00:00.000Z'",
                [],
            )
            .expect("ages the session");

        assert!(database.verify_session("bearer-1").expect("reads").is_none());
    }

    #[test]
    fn a_ticket_is_minted_from_a_live_bearer_and_spent_once() {
        let database = pairing_database();
        session(&database, "bearer-1");

        let ticket = database
            .issue_websocket_ticket("bearer-1", "ticket-1")
            .expect("reads")
            .expect("a live bearer gets a ticket");
        assert_eq!(ticket.ticket, "ticket-1");

        assert_eq!(
            database
                .consume_websocket_ticket("ticket-1")
                .expect("reads")
                .expect("the first upgrade succeeds")
                .subject,
            "pairing"
        );
        assert!(database
            .consume_websocket_ticket("ticket-1")
            .expect("reads")
            .is_none());
    }

    #[test]
    fn a_ticket_cannot_be_minted_from_a_bearer_that_does_not_verify() {
        let database = pairing_database();
        assert!(database
            .issue_websocket_ticket("bearer-nope", "ticket-1")
            .expect("reads")
            .is_none());
    }

    /// The reason a ticket names its session rather than copying it: revoking
    /// the session has to mean revoked, not "revoked once the outstanding
    /// tickets run out".
    #[test]
    fn revoking_a_session_invalidates_its_outstanding_tickets() {
        let database = pairing_database();
        session(&database, "bearer-1");
        database
            .issue_websocket_ticket("bearer-1", "ticket-1")
            .expect("reads")
            .expect("a ticket");

        database
            .lock()
            .execute(&format!("UPDATE auth_sessions SET revoked_at = {NOW}"), [])
            .expect("revokes the session");

        assert!(database
            .consume_websocket_ticket("ticket-1")
            .expect("reads")
            .is_none());
    }

    #[test]
    fn an_expired_ticket_does_not_upgrade() {
        let database = pairing_database();
        session(&database, "bearer-1");
        database
            .issue_websocket_ticket("bearer-1", "ticket-1")
            .expect("reads")
            .expect("a ticket");
        database
            .lock()
            .execute(
                "UPDATE auth_websocket_tickets SET expires_at = '2000-01-01T00:00:00.000Z'",
                [],
            )
            .expect("ages the ticket");

        assert!(database
            .consume_websocket_ticket("ticket-1")
            .expect("reads")
            .is_none());
    }

    /// Same race as the pairing code's, one step later. A ticket that could be
    /// spent twice would let anyone who read one out of a proxy log open a
    /// second socket beside the phone's.
    #[test]
    fn concurrent_upgrades_on_one_ticket_produce_exactly_one_socket() {
        let database = Arc::new(pairing_database());
        session(&database, "bearer-1");
        database
            .issue_websocket_ticket("bearer-1", "ticket-1")
            .expect("reads")
            .expect("a ticket");

        let upgraded = Arc::new(AtomicI64::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let database = Arc::clone(&database);
                let upgraded = Arc::clone(&upgraded);
                scope.spawn(move || {
                    if database
                        .consume_websocket_ticket("ticket-1")
                        .expect("reads")
                        .is_some()
                    {
                        upgraded.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(upgraded.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_verification_result_cannot_be_applied_after_the_hostname_changes() {
        let database = Database::in_memory().expect("a database");
        database.register_external_tunnel_endpoint("https://a.example.com").unwrap();
        database.register_external_tunnel_endpoint("https://b.example.com").unwrap();

        assert!(!database
            .record_external_tunnel_verification("https://a.example.com", true, None, None)
            .unwrap());
        let endpoint = database.external_tunnel_endpoint().unwrap().unwrap();
        assert_eq!(endpoint.https_origin, "https://b.example.com");
        assert_eq!(endpoint.verification_state, "pending");
    }
}
