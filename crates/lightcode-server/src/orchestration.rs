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
//! ## What this ticket does not do
//!
//! - **Threads are an empty array.** They join the same snapshot in tickets 10
//!   and 11 and share this sequence when they do.
//! - **`afterSequence` is honoured by over-answering.** The contract lets a
//!   client with a cached snapshot ask for a replay from a sequence instead of
//!   a fresh snapshot; lightcode sends the snapshot anyway. It is a superset of
//!   what was asked for and the client folds it as a reset, so the cost is
//!   bandwidth on reconnect rather than correctness.
//! - **`commandId` is not remembered.** Upstream uses it to recognise a command
//!   it has already run. lightcode keeps no log of ids, so a re-dispatched
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
//! **lightcode ignores the flag and refuses a path that is not there**, naming
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::projects::{Project, WorkspaceRoot};
use crate::store::{Conflict, Database, Insert, Registry, Removal, StorageError};
use crate::subscriptions::{EventSource, BACKLOG};

/// The tag that carries every write to the registry.
pub const DISPATCH_COMMAND: &str = "orchestration.dispatchCommand";

/// The subscription that *is* the project list.
pub const SUBSCRIBE_SHELL: &str = "orchestration.subscribeShell";

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
    database: Database,
    updates: broadcast::Sender<Value>,
    /// Held across "commit, then announce".
    ///
    /// The database would serialise the writes on its own; what this adds is
    /// that a command's event is published before the next command's is.
    /// Without it two concurrent adds could commit as 5 then 6 and publish as
    /// 6 then 5 — and a client that ignores events at or below the highest
    /// sequence it has seen would drop 5 permanently. The lock is held over a
    /// local SQLite write and a non-blocking broadcast send, neither of which
    /// awaits.
    commit: Mutex<()>,
}

/// A command this server understands, once its payload has been read.
///
/// Parsing to this is where a malformed or unimplemented command is turned
/// away, so by the time [`Shell::dispatch`] has one it is only the *world* that
/// can still refuse it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    CreateProject(CreateProject),
    DeleteProject { project_id: String },
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
    pub fn new(database: Database) -> Shell {
        Shell {
            inner: Arc::new(Inner {
                database,
                updates: broadcast::channel(BACKLOG).0,
                commit: Mutex::new(()),
            }),
        }
    }

    /// Carry out one `orchestration.dispatchCommand`, answering with the
    /// sequence it committed at.
    pub fn dispatch(&self, payload: &Value) -> Result<Value, CommandError> {
        let sequence = match Command::parse(payload)? {
            Command::CreateProject(create) => self.create_project(&create)?,
            Command::DeleteProject { project_id } => self.delete_project(&project_id)?,
        };

        Ok(json!({ "sequence": sequence }))
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

        let _commit = self.lock();
        let insert = self
            .inner
            .database
            .insert_project(&create.project_id, &title, &root, create.created_at.as_deref())
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

    fn delete_project(&self, project_id: &str) -> Result<i64, CommandError> {
        let _commit = self.lock();
        let removal = self
            .inner
            .database
            .remove_project(project_id)
            .map_err(unavailable("remove the project"))?;

        if let Removal::Committed(sequence) = removal {
            self.announce(project_removed(sequence, project_id));
        }
        Ok(removal.sequence())
    }

    /// Open an `orchestration.subscribeShell` subscription: the registry now,
    /// then every change to it.
    pub fn subscribe(&self, payload: &Value) -> EventSource {
        let wants_marker = payload
            .get("requestCompletionMarker")
            .and_then(Value::as_bool)
            .unwrap_or(false);

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
            move || match shell.snapshot() {
                Ok(snapshot) => {
                    let mut items = vec![snapshot];
                    if marker_owed.swap(false, Ordering::Relaxed) {
                        items.push(json!({"kind": "synchronized"}));
                    }
                    items
                }
                // Nothing rather than an empty registry, and the marker stays
                // owed. An empty snapshot would be a claim that the user has no
                // projects, which is a worse answer than silence — and the
                // marker would be a claim that a catch-up succeeded when it did
                // not.
                Err(error) => {
                    eprintln!("lightcode: cannot describe the project registry: {error}");
                    Vec::new()
                }
            },
            updates,
        )
    }

    fn snapshot(&self) -> Result<Value, StorageError> {
        Ok(snapshot_event(&self.inner.database.registry()?))
    }

    fn announce(&self, event: Value) {
        // `send` on a broadcast channel never blocks — it drops the oldest
        // value when the buffer is full, and a subscriber that lags is resent a
        // snapshot instead. So this cannot deadlock under the commit lock.
        let _ = self.inner.updates.send(event);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.inner
            .commit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

/// The opening chunk of a shell subscription.
///
/// `threads` is an empty array rather than an absent key: the contract requires
/// it, and a client decoding `OrchestrationShellSnapshot` rejects the whole
/// snapshot — and so shows no projects either — if it is missing.
fn snapshot_event(registry: &Registry) -> Value {
    json!({
        "kind": "snapshot",
        "snapshot": {
            "snapshotSequence": registry.sequence,
            "projects": registry
                .projects
                .iter()
                .map(Project::to_value)
                .collect::<Vec<Value>>(),
            "threads": [],
            "updatedAt": registry.updated_at,
        },
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
    /// roughly twenty command types and lightcode implements two, so "which
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
        directory: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture {
                shell: Shell::new(Database::in_memory().expect("an in-memory database")),
                directory: tempfile::tempdir().expect("a temporary directory"),
            }
        }

        fn folder(&self, name: &str) -> String {
            let path = self.directory.path().join(name);
            std::fs::create_dir_all(&path).expect("creates the folder");
            path.to_string_lossy().into_owned()
        }

        /// The captured `project.create` payload, with the folder swapped.
        fn add(&self, id: &str, folder: &str) -> Result<Value, CommandError> {
            self.shell.dispatch(&json!({
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
            self.shell.dispatch(&json!({
                "type": "project.delete",
                "commandId": format!("test:delete:{id}"),
                "projectId": id,
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
            .shell
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
            fixture.shell.dispatch(&command).expect("registered");
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

    /// A client resuming from a cached snapshot asks for a replay. Answering
    /// with the whole snapshot is a superset of that, and the point of the test
    /// is that it is still an answer rather than a refusal.
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

    /// Roughly twenty command types exist and lightcode implements two. Each
    /// refusal has to name what was asked for, or a developer cannot tell which
    /// of them is missing.
    #[test]
    fn an_unimplemented_or_malformed_command_is_refused_by_name() {
        let shell = Shell::new(Database::in_memory().expect("an in-memory database"));

        let refusal = shell
            .dispatch(&json!({"type": "thread.create", "commandId": "c", "threadId": "t"}))
            .expect_err("threads arrive in ticket 10");
        assert!(
            refusal.message().contains("thread.create"),
            "{}",
            refusal.message()
        );
        assert_eq!(
            refusal.to_error()["_tag"],
            "OrchestrationDispatchCommandError"
        );
        assert_eq!(refusal.to_error()["message"], refusal.message());

        let refusal = shell.dispatch(&json!({})).expect_err("no type at all");
        assert!(refusal.message().contains("type"), "{}", refusal.message());

        let refusal = shell
            .dispatch(&json!({"type": "project.create", "projectId": "p"}))
            .expect_err("no workspace root");
        assert!(
            refusal.message().contains("project.create") && refusal.message().contains("malformed"),
            "{}",
            refusal.message()
        );

        let refusal = shell
            .dispatch(&json!({"type": "project.delete", "projectId": "   "}))
            .expect_err("a blank id");
        assert!(
            refusal.message().contains("projectId"),
            "{}",
            refusal.message()
        );
    }
}
