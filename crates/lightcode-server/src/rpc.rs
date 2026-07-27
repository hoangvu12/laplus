//! Method dispatch: a request tag in, an answer out.
//!
//! The vocabulary is roughly sixty methods. Eighteen are implemented — the
//! configuration the UI fetches before it can do anything else and the
//! subscription that keeps it current, the command that writes the project
//! registry and starts a conversation, the subscription that *is* the project
//! list, the subscription that *is* one conversation, the three that enumerate
//! names on disk for the picker, the tree and the `@` mention, the two that open
//! and save one of those files, the one that hands a file to the developer's
//! own editor, the five that open a terminal, read it, type into it, resize
//! it and list them, and the two that say what has changed in the working tree
//! and keep saying it — and every other tag lands in the unknown-method path,
//! which is itself part of the contract and is pinned by a capture.
//!
//! Nothing in a `Request` says whether the answer will be one value, a stream
//! of them, or a value that is not ready yet; that is knowledge the method name
//! carries and the client already has. [`Answer`] is where the three part
//! company.
//!
//! Two of the methods here declare no success value at all. `Schema.Void`
//! encodes to `null` over this wire — `SchemaAST.Void::toCodecJson` links it
//! through `undefinedToNull` — so a bare [`Value::Null`] is the whole of what
//! `terminal.write` and `terminal.resize` answer with, and it decodes.

use std::fmt;

use serde_json::Value;

use crate::config_store::ConfigStore;
use crate::editor::{self, OpenInEditor};
use crate::files::{self, ReadFile, WriteFile};
use crate::filesystem::{self, Browse, Index, ListEntries, SearchEntries};
use crate::git::{self, Repositories, StatusCall};
use crate::orchestration::{self, Shell};
use crate::subscriptions::EventSource;
use crate::terminal::{self, Attach, Clear, Close, Resize, Restart, Terminals, WriteInput};
use crate::threads;

/// The payload of an `orchestration.subscribeThread`.
///
/// Read by hand rather than deserialized, because there are two fields and one
/// of them decides whether the call is answerable at all — a subscription to a
/// blank thread id would open a stream against a conversation nothing can name.
struct SubscribeThread {
    thread_id: String,
    wants_marker: bool,
}

impl SubscribeThread {
    fn read(payload: &Value) -> Result<SubscribeThread, Value> {
        let thread_id = payload
            .get("threadId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(SubscribeThread {
            thread_id: non_blank(thread_id, "OrchestrationGetSnapshotError", "thread id")?,
            wants_marker: payload
                .get("requestCompletionMarker")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// The tag the UI sends first, and the tag it re-sends as a liveness probe
/// when the server does not advertise `connectionProbe`.
pub const SERVER_GET_CONFIG: &str = "server.getConfig";

/// The configuration subscription — the simplest of the eight the UI opens,
/// and the one ticket 04 proves the streaming mechanism on.
pub const SUBSCRIBE_SERVER_CONFIG: &str = "subscribeServerConfig";

/// Everything a method is allowed to read or change.
///
/// One value rather than a widening argument list, and deliberately *not* the
/// server state: dispatch has no business knowing about connection counts,
/// shutdown or drift counters. Each ticket that implements a method adds the
/// thing it needs here.
#[derive(Debug, Clone)]
pub struct Services {
    pub config: ConfigStore,
    pub shell: Shell,
    /// The last scan of each workspace, and the watcher that keeps it honest.
    /// Shared rather than per-connection: two windows on one project are
    /// looking at one filesystem, and scanning it twice — or watching it twice
    /// — would be paying twice for the same answer.
    pub index: Index,
    /// The shells the developer has open. Shared for the same reason the index
    /// is, and for a stronger one: a terminal outlives the connection that
    /// opened it, so a per-connection registry would kill a build every time
    /// the socket blinked.
    pub terminals: Terminals,
    /// The working trees whose status is being kept. Shared for the reason the
    /// index is — two windows on one project are looking at one working tree —
    /// and built from the index, because the watcher that keeps a status honest
    /// is the same one that keeps a listing honest.
    pub repositories: Repositories,
}

/// What a method answers with.
#[derive(Debug)]
pub enum Answer {
    /// One value, one `Exit`. The whole of a unary call, answered from memory.
    Value(Value),
    /// A stream of values, chunked until the client cancels it.
    Stream(EventSource),
    /// One value, but not yet — work that has to touch the world before it can
    /// say anything. See [`Deferred`].
    Deferred(Deferred),
}

/// A unary answer that has to be produced somewhere other than the read loop.
///
/// The connection loop reads frames one at a time and answers each before
/// taking the next, which is right while every method answers from memory: the
/// waiting is nil and the ordering is free. Walking a repository of twenty-five
/// thousand files is not nil. Answering that inline would hold the socket's
/// only reader for the length of the walk, and the `Ack` that releases a
/// subscription's next chunk, the `Ping` the UI sends every five seconds, and
/// every other call the window makes would all queue behind it — the file tree
/// would arrive and the rest of the app would have stopped.
///
/// The line is *unbounded* work rather than "touches the world": adding a
/// project stats a folder and writes a row on the read loop, and starting a
/// terminal's shell is one process spawn. Those are bounded by something other
/// than the size of the developer's repository, which is what makes them
/// affordable inline.
///
/// So the method hands back the work instead of the answer, and
/// [`crate::server`] runs it on a blocking thread that writes the `Exit`
/// itself. Correlation on this wire is by `requestId` and never by order, so
/// answering out of order is not merely tolerable — it is what the reference
/// server does.
///
/// Blocking rather than `async`: this is disk work, and there is no
/// non-blocking way to enumerate a directory. Pretending otherwise with an
/// `async` wrapper would move the same stall onto a runtime worker.
///
/// `Err` carries the method's own declared error, already in the shape the
/// client decodes — the same thing [`DispatchError::to_error`] produces, which
/// is why the two meet at [`ServerMessage::failure`](crate::wire::ServerMessage).
pub struct Deferred(Box<dyn FnOnce() -> Result<Value, Value> + Send>);

impl Deferred {
    pub fn new(work: impl FnOnce() -> Result<Value, Value> + Send + 'static) -> Deferred {
        Deferred(Box::new(work))
    }

    pub fn run(self) -> Result<Value, Value> {
        (self.0)()
    }
}

impl fmt::Debug for Deferred {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Deferred")
    }
}

/// Why a call produced no value.
///
/// Two cases and not one per method: dispatch has no business enumerating the
/// error type of every method it routes to. What it needs to distinguish is
/// "there is no such method", which is the server's own answer, from "the
/// method answered and the answer was a refusal", which is the method's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// No method is wired to this tag.
    UnknownMethod(String),
    /// The method ran and refused. Unlike [`DispatchError::UnknownMethod`],
    /// this is an error the method *declares*, already in the shape the client
    /// decodes against the contract, so it is shown to the user rather than
    /// treated as a broken response.
    Declared(Value),
}

impl DispatchError {
    /// The typed error to put in an `Exit`/`Failure`'s `Fail` cause.
    ///
    /// **This is a deliberate divergence from the reference server, and it is
    /// the one place in this ticket where a capture is not followed.**
    ///
    /// The reference server answers an unknown tag with a bare `Defect`
    /// (`fixtures/socket-wire/03-typed-error.ndjson`), which carries no
    /// `requestId`. In the client that is not a scoped failure:
    /// `RpcClient.ts` handles it as `clearEntries(Exit.die(message.defect))`,
    /// which fails *every* in-flight request and *every* open subscription on
    /// the socket, and the connection supervisor then reconnects on a
    /// 1/2/4/8/16-second backoff. An `Exit` whose error fails to decode, by
    /// contrast, is caught per request — `decodeExit(...).matchCauseEffect`
    /// writes the failure back under the same `requestId` and nothing else is
    /// touched. Both readings are from `effect@4.0.0-beta.78` in the vendored
    /// checkout; this answers open question 4 in `docs/socket-wire-format.md`.
    ///
    /// The reference server can afford `Defect` because it implements every
    /// tag its client sends, so a `Defect` only ever answers a tag no real
    /// client uses. lightcode implements one method of roughly sixty, so
    /// during the build-out `Defect` would be the *normal* answer to the UI's
    /// own boot sequence — and each one would tear down the session. The
    /// ticket asks for an error "the client understands, rather than dropping
    /// the connection", and this is what that means in practice.
    ///
    /// The error is `_tag`-discriminated like every other error on this wire.
    /// It will not decode against the method's declared error union, which
    /// costs exactly one request.
    pub fn to_error(&self) -> Value {
        match self {
            DispatchError::UnknownMethod(tag) => serde_json::json!({
                "_tag": "ServerMethodNotImplementedError",
                "method": tag,
                "message": format!("Method not implemented by this server: {tag}"),
            }),
            DispatchError::Declared(error) => error.clone(),
        }
    }
}

/// A typed error under `tag`, carrying a sentence and nothing else.
///
/// Every method that parses a payload needs this shape for the case where the
/// payload was not one — a missing field, a blank path, a number where a string
/// belongs. There is deliberately no `failure` code: each method's failure
/// literals describe things that went wrong with a request that *was*
/// well-formed, and none of them describes a request that never arrived
/// properly. The field is optional on the wire, so leaving it out still decodes.
pub fn declared(tag: &str, message: impl std::fmt::Display) -> Value {
    serde_json::json!({"_tag": tag, "message": message.to_string()})
}

/// A required string from a payload, trimmed, or the method's own refusal.
///
/// The contract types these as `TrimmedNonEmptyString` throughout, so a blank
/// one is not a value — and letting one through would mean a workspace root of
/// `""` reaching the disk, where it means the process's own directory.
pub fn non_blank(value: &str, tag: &str, subject: &str) -> Result<String, Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(declared(
            tag,
            format_args!("This call needs a {subject}; none was given."),
        ));
    }
    Ok(trimmed.to_string())
}

/// Answer one call.
pub fn dispatch(
    services: &Services,
    tag: &str,
    payload: &Value,
) -> Result<Answer, DispatchError> {
    match tag {
        SERVER_GET_CONFIG => Ok(Answer::Value(services.config.current().to_value())),
        // The payload is an empty struct in the contract, so there is nothing
        // to read out of it and nothing that can be wrong with it.
        SUBSCRIBE_SERVER_CONFIG => Ok(Answer::Stream(services.config.subscribe())),
        orchestration::DISPATCH_COMMAND => services
            .shell
            .dispatch(payload, &services.index, &services.config.current())
            .map(Answer::Value)
            .map_err(|refusal| DispatchError::Declared(refusal.to_error())),
        orchestration::SUBSCRIBE_SHELL => Ok(Answer::Stream(services.shell.subscribe(payload))),
        threads::SUBSCRIBE_THREAD => SubscribeThread::read(payload)
            .map(|call| {
                Answer::Stream(
                    services
                        .shell
                        .threads()
                        .subscribe(&call.thread_id, call.wants_marker),
                )
            })
            .map_err(DispatchError::Declared),
        // Every method that touches a disk reads its payload here and does its
        // work elsewhere. Reading is arithmetic on a string, so a malformed
        // call is refused immediately rather than after a thread has been found
        // for it; everything after that is I/O.
        filesystem::BROWSE => Browse::read(payload)
            .map(|call| Answer::Deferred(Deferred::new(move || call.run())))
            .map_err(DispatchError::Declared),
        filesystem::LIST_ENTRIES => ListEntries::read(payload)
            .map(|call| deferred_with(&services.index, |index| call.run(index)))
            .map_err(DispatchError::Declared),
        filesystem::SEARCH_ENTRIES => SearchEntries::read(payload)
            .map(|call| deferred_with(&services.index, |index| call.run(index)))
            .map_err(DispatchError::Declared),
        files::READ_FILE => ReadFile::read(payload)
            .map(|call| Answer::Deferred(Deferred::new(move || call.run())))
            .map_err(DispatchError::Declared),
        files::WRITE_FILE => WriteFile::read(payload)
            .map(|call| deferred_with(&services.index, |index| call.run(index)))
            .map_err(DispatchError::Declared),
        editor::OPEN_IN_EDITOR => OpenInEditor::read(payload)
            .map(|call| Answer::Deferred(Deferred::new(move || call.run())))
            .map_err(DispatchError::Declared),
        // Opening a terminal stats a directory and starts a process, so it goes
        // off the read loop. Attaching to one cannot — a stream is answered by
        // its own pump and there is no deferred form of that — but it only
        // *opens* in the case where the client's own open has not landed yet,
        // and a process spawn is bounded work. See [`Deferred`].
        terminal::OPEN => terminal::Open::read(payload)
            .map(|call| {
                let terminals = services.terminals.clone();
                Answer::Deferred(Deferred::new(move || terminals.open(call)))
            })
            .map_err(DispatchError::Declared),
        terminal::ATTACH => Attach::read(payload)
            .and_then(|call| services.terminals.attach(call))
            .map(Answer::Stream)
            .map_err(DispatchError::Declared),
        terminal::WRITE => WriteInput::read(payload)
            .and_then(|call| services.terminals.write(&call))
            .map(Answer::Value)
            .map_err(DispatchError::Declared),
        terminal::RESIZE => Resize::read(payload)
            .and_then(|call| services.terminals.resize(&call))
            .map(Answer::Value)
            .map_err(DispatchError::Declared),
        // Clearing is arithmetic on a string it already holds, so it answers
        // from the read loop like a write does.
        terminal::CLEAR => Clear::read(payload)
            .and_then(|call| services.terminals.clear(&call))
            .map(Answer::Value)
            .map_err(DispatchError::Declared),
        // Restarting and closing both end a process and wait for it, which is
        // the one thing a read loop must never do.
        terminal::RESTART => Restart::read(payload)
            .map(|call| {
                let terminals = services.terminals.clone();
                Answer::Deferred(Deferred::new(move || terminals.restart(&call)))
            })
            .map_err(DispatchError::Declared),
        terminal::CLOSE => Close::read(payload)
            .map(|call| {
                let terminals = services.terminals.clone();
                Answer::Deferred(Deferred::new(move || terminals.close(&call)))
            })
            .map_err(DispatchError::Declared),
        // The payload is an empty struct in the contract, like the
        // configuration subscription's.
        terminal::SUBSCRIBE_METADATA => Ok(Answer::Stream(services.terminals.subscribe_metadata())),
        // The status subscription answers from the read loop because it does
        // not run git: it describes itself from the last read and the reading
        // happens elsewhere. See [`crate::git`].
        git::SUBSCRIBE_STATUS => StatusCall::read(payload, git::SUBSCRIBE_STATUS)
            .and_then(|call| services.repositories.subscribe(&call))
            .map(Answer::Stream)
            .map_err(DispatchError::Declared),
        // Asking for one *does* run git, which on a large repository is the
        // longest wait any method here has.
        git::REFRESH_STATUS => StatusCall::read(payload, git::REFRESH_STATUS)
            .map(|call| {
                let repositories = services.repositories.clone();
                Answer::Deferred(Deferred::new(move || repositories.refresh(&call)))
            })
            .map_err(DispatchError::Declared),
        unknown => Err(DispatchError::UnknownMethod(unknown.to_string())),
    }
}

/// Defer work that needs the workspace index.
///
/// The index outlives the call by design — it is the server's, not the
/// connection's — so the closure takes a clone rather than a borrow of
/// `Services`, which the deferred task cannot hold.
fn deferred_with(
    index: &Index,
    work: impl FnOnce(&Index) -> Result<Value, Value> + Send + 'static,
) -> Answer {
    let index = index.clone();
    Answer::Deferred(Deferred::new(move || work(&index)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use crate::store::Database;
    use serde_json::json;

    fn services() -> Services {
        let index = Index::new();
        Services {
            config: ConfigStore::new(ServerConfig::detect()),
            shell: Shell::new(Database::in_memory().expect("an in-memory database")),
            repositories: Repositories::new(&index),
            index,
            terminals: Terminals::new(),
        }
    }

    fn value(answer: Answer) -> Value {
        match answer {
            Answer::Value(value) => value,
            other => panic!("expected a unary answer, got {other:?}"),
        }
    }

    #[test]
    fn get_config_returns_the_config() {
        let services = services();
        let answer = dispatch(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(answer), services.config.current().to_value());
    }

    /// The client re-sends `server.getConfig` as its liveness probe, so a
    /// second call has to answer identically rather than consume anything.
    #[test]
    fn get_config_is_repeatable() {
        let services = services();
        let first = dispatch(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        let second = dispatch(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(first), value(second));
    }

    /// Every subscription is dispatched by the same path as a unary call and
    /// only parts company at the answer.
    #[test]
    fn the_subscriptions_answer_with_a_stream() {
        let services = services();
        for (tag, payload) in [
            (SUBSCRIBE_SERVER_CONFIG, json!({})),
            (orchestration::SUBSCRIBE_SHELL, json!({})),
            // Against a thread that does not exist, because that is what the UI
            // opens first: a new conversation is a client-side draft until its
            // first turn reaches this server.
            (threads::SUBSCRIBE_THREAD, json!({"threadId": "thread-1"})),
            (terminal::SUBSCRIBE_METADATA, json!({})),
        ] {
            let answer = dispatch(&services, tag, &payload).expect("dispatches");
            assert!(matches!(answer, Answer::Stream(_)), "{tag} does not stream");
        }
    }

    /// A method that refuses fails its own call with the error *it* declares —
    /// not with the unknown-method error, which would tell the client the
    /// server cannot add projects or read directories at all.
    #[test]
    fn a_refused_call_fails_with_the_methods_own_declared_error() {
        let services = services();

        for (tag, payload, expected) in [
            (
                orchestration::DISPATCH_COMMAND,
                json!({"type": "project.create", "projectId": "p", "workspaceRoot": "  "}),
                "OrchestrationDispatchCommandError",
            ),
            (filesystem::BROWSE, json!({}), "FilesystemBrowseError"),
            (
                filesystem::LIST_ENTRIES,
                json!({"cwd": "  "}),
                "ProjectListEntriesError",
            ),
            (
                threads::SUBSCRIBE_THREAD,
                json!({}),
                "OrchestrationGetSnapshotError",
            ),
        ] {
            let error = dispatch(&services, tag, &payload).expect_err("a refusal");

            assert!(matches!(error, DispatchError::Declared(_)), "{tag}");
            assert_eq!(error.to_error()["_tag"], expected, "{tag}");
            assert!(error.to_error()["message"].is_string(), "{tag}");
        }
    }

    /// The terminal methods refuse the same way, but **without a `message`** —
    /// and that is the contract rather than an omission. Every class in
    /// `TerminalError` defines `message` as a getter over its declared fields,
    /// so the client computes the sentence and a server that sent one would be
    /// sending a field the reference server does not.
    #[test]
    fn a_terminal_that_is_not_there_is_refused_by_its_two_names() {
        let services = services();

        for (tag, payload) in [
            (terminal::ATTACH, json!({})),
            (terminal::WRITE, json!({"data": "ls\r"})),
            (terminal::RESIZE, json!({"cols": 80, "rows": 24})),
            (terminal::CLEAR, json!({})),
        ] {
            let payload = {
                let mut payload = payload;
                payload["threadId"] = json!("thread-1");
                payload["terminalId"] = json!("term-1");
                payload
            };
            let error = dispatch(&services, tag, &payload).expect_err("a refusal");

            assert!(matches!(error, DispatchError::Declared(_)), "{tag}");
            let error = error.to_error();
            assert_eq!(error["_tag"], "TerminalSessionLookupError", "{tag}");
            assert_eq!(error["threadId"], "thread-1", "{tag}");
            assert_eq!(error["terminalId"], "term-1", "{tag}");
            assert!(error["message"].is_null(), "{tag}: {error}");
        }
    }

    /// The two filesystem methods answer with work rather than a value. That is
    /// the whole of what dispatch decides about them — where the work runs is
    /// [`crate::server`]'s business.
    #[test]
    fn the_filesystem_methods_answer_with_deferred_work() {
        let services = services();
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().to_string_lossy().into_owned();

        for (tag, payload) in [
            (filesystem::BROWSE, json!({"partialPath": path})),
            (filesystem::LIST_ENTRIES, json!({"cwd": path})),
        ] {
            let answer = dispatch(&services, tag, &payload).expect("dispatches");
            match answer {
                Answer::Deferred(work) => {
                    work.run().unwrap_or_else(|error| panic!("{tag}: {error}"));
                }
                other => panic!("{tag} answered with {other:?}"),
            }
        }
    }

    /// The two terminal calls that end a process answer with work rather than a
    /// value, and that is not a preference: both wait for a shell to die and for
    /// the threads driving its pty to be joined, which on the connection's read
    /// loop would stop every other call on that connection.
    #[test]
    fn the_terminal_calls_that_end_a_process_answer_with_deferred_work() {
        let services = services();
        let directory = tempfile::tempdir().expect("a temporary directory");
        let named = json!({
            "threadId": "thread-1",
            "terminalId": "term-1",
            "cwd": directory.path().to_string_lossy(),
            "cols": 80,
            "rows": 24,
        });

        for tag in [terminal::CLOSE, terminal::RESTART] {
            let answer = dispatch(&services, tag, &named).expect("dispatches");
            assert!(matches!(answer, Answer::Deferred(_)), "{tag} answered inline");
        }

        // …and clearing does not, because it is arithmetic on a string the
        // registry already holds.
        let answer = dispatch(&services, terminal::CLEAR, &named);
        assert!(
            matches!(answer, Err(DispatchError::Declared(_))),
            "a clear of a terminal that is not there is answered where it is asked"
        );
    }

    /// The tag has to survive into the error, because it is the only thing
    /// that tells a developer which of the sixty methods is missing.
    #[test]
    fn an_unknown_tag_becomes_a_typed_error_naming_the_method() {
        let services = services();
        let error = dispatch(&services, "orchestration.getTurnDiff", &json!({}))
            .expect_err("not implemented");
        assert_eq!(
            error,
            DispatchError::UnknownMethod("orchestration.getTurnDiff".to_string())
        );

        let payload = error.to_error();
        assert_eq!(payload["_tag"], "ServerMethodNotImplementedError");
        assert_eq!(payload["method"], "orchestration.getTurnDiff");
        assert!(payload["message"]
            .as_str()
            .expect("a message")
            .contains("orchestration.getTurnDiff"));
    }
}
