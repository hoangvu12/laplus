//! Method dispatch: a request tag in, an answer out.
//!
//! The vocabulary is roughly sixty methods, and what this server answers of it
//! is counted in `.scratch/contract-parity/ledger.md` rather than here — a
//! figure in prose is a claim nothing re-checks, and this one had gone stale by
//! ten.
//!
//! What is answered is the
//! configuration the UI fetches before it can do anything else and the
//! subscription that keeps it current, the command that writes the project
//! registry and starts a conversation, the subscription that *is* the project
//! list, the call that answers with the other half of it, the subscription that
//! *is* one conversation, the three that enumerate
//! names on disk for the picker, the tree and the `@` mention, the two that open
//! and save one of those files, the one that hands a file to the developer's
//! own editor, the five that open a terminal, read it, type into it, resize
//! it and list them, the two that say what has changed in the working tree
//! and keep saying it, the five that list the branches, move between them,
//! make one, take a second checkout off disk and make the repository a project
//! has not got yet, and the two that
//! read one turn and a whole conversation back as a diff — and every other tag
//! lands in the unknown-method path, which is itself part of the contract, is
//! pinned by a capture, and refuses under a tag the method it names declares.
//! [`crate::refusals`] is where that tag comes from.
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

use crate::assets::{self, CreateUrl};
use crate::checkpoints::{self, Diff};
use crate::config_store::ConfigStore;
use crate::editor::{self, OpenInEditor};
use crate::files::{self, ReadFile, WriteFile};
use crate::filesystem::{self, Browse, Index, ListEntries, SearchEntries};
use crate::git::{self, Repositories, StatusCall};
use crate::keybindings::{self, Upsert};
use crate::orchestration::{self, Shell};
use crate::refs::{self, CreateRef, Init, ListRefs, RemoveWorktree, SwitchRef};
use crate::settings;
use crate::subscriptions::EventSource;
use crate::terminal::{self, Attach, Clear, Close, Resize, Restart, Terminals, WriteInput};
use crate::threads::{self, Watch};
use crate::usage;

/// The tag the UI sends first, and the tag it re-sends as a liveness probe
/// when the server does not advertise `connectionProbe`.
pub const SERVER_GET_CONFIG: &str = "server.getConfig";

/// The liveness probe itself: an empty payload in, an empty payload out.
///
/// The contract is `payload: Schema.Struct({})`, `success: Schema.Struct({})` —
/// nothing to read and nothing to say, and the arm consults no state, so a probe
/// cannot fail for a reason that has nothing to do with whether the connection is
/// alive. What it buys is what it does *not* carry: `session.ts` probes on a
/// timer, and against a server that stays quiet about
/// `capabilities.connectionProbe` it probes by re-sending [`SERVER_GET_CONFIG`],
/// dragging the whole config payload back over the wire to prove the socket is
/// still there.
///
/// The capability must not be flipped without this arm, and the reason is a trap
/// rather than a preference: [`crate::config::Capabilities::connection_probe`]
/// holds it, and [`crate::refusals`] repeats it where the refusal used to live.
pub const SERVER_PROBE: &str = "server.probe";

const ORCHESTRATION_READ_SCOPE: &str = "orchestration:read";

/// The configuration subscription — the simplest of the eight the UI opens,
/// and the one ticket 04 proves the streaming mechanism on.
pub const SUBSCRIBE_SERVER_CONFIG: &str = "subscribeServerConfig";

/// The access subscription — what Settings reads its pairing links from.
///
/// Ticket 73 built `GET /api/auth/pairing-links` because the contract declares
/// it, and the Settings panel does not call it: `ConnectionsSettings.tsx:1559`
/// opens this subscription instead and folds the snapshot. So until this
/// existed, a code could be minted over HTTP and never appeared on the screen
/// that minted it.
pub const SUBSCRIBE_AUTH_ACCESS: &str = "subscribeAuthAccess";

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
    pub provider_maintenance: crate::provider_maintenance::ProviderMaintenance,
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
    /// client uses. laplus implemented one method of the seventy when this was
    /// decided, so during the build-out `Defect` would be the *normal* answer
    /// to the UI's own boot sequence — and each one would tear down the
    /// session. The ticket asks for an error "the client understands, rather
    /// than dropping the connection", and this is what that means in practice.
    ///
    /// The error is `_tag`-discriminated like every other error on this wire,
    /// and **which** tag is [`crate::refusals`]'s question rather than this
    /// one's: a refusal costs one request only while the client can decode it,
    /// and it can only decode a tag the method it called declares. Answering
    /// every method with one invented tag cost the request *and* put the
    /// decoder's complaint on the screen. Ticket 39.
    pub fn to_error(&self) -> Value {
        match self {
            DispatchError::UnknownMethod(tag) => crate::refusals::refusal(tag),
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

/// A subscription's `afterSequence` cursor, or `None` for a client that holds
/// nothing to resume from.
///
/// Both subscriptions read this field and have to read it the same way. The rule
/// ADR-0016 states is one rule, and two readers would let `subscribeShell` and
/// `subscribeThread` disagree about what a cursor even is before either of them
/// got as far as comparing it.
///
/// The contract types it `NonNegativeInt`, so no conforming client is affected
/// by the filter — but a client that sends nonsense is a client with no cache to
/// go without a snapshot, and reading it as absent hands it one. Letting it
/// through instead would open a stream with nothing to fold events into, which
/// is ticket 28's failure exactly.
pub fn resume_cursor(payload: &Value) -> Option<i64> {
    payload
        .get("afterSequence")
        .and_then(Value::as_i64)
        .filter(|cursor| *cursor >= 0)
}

/// Answer one call.
pub fn dispatch_without_grant(
    services: &Services,
    tag: &str,
    payload: &Value,
) -> Result<Answer, DispatchError> {
    dispatch_scoped(services, &[], tag, payload)
}

pub fn dispatch(
    services: &Services,
    grant: &crate::pairing::Grant,
    tag: &str,
    payload: &Value,
) -> Result<Answer, DispatchError> {
    dispatch_scoped(services, &grant.scopes, tag, payload)
}

fn dispatch_scoped(
    services: &Services,
    scopes: &[String],
    tag: &str,
    payload: &Value,
) -> Result<Answer, DispatchError> {
    match tag {
        SERVER_GET_CONFIG => Ok(Answer::Value(services.config.current().to_value())),
        SERVER_PROBE => Ok(Answer::Value(serde_json::json!({}))),
        usage::GET_SUMMARY => {
            if !scopes.iter().any(|scope| scope == ORCHESTRATION_READ_SCOPE) {
                return Err(DispatchError::Declared(serde_json::json!({
                    "_tag": "EnvironmentAuthorizationError",
                    "message": "This method requires orchestration:read access.",
                    "requiredScope": ORCHESTRATION_READ_SCOPE
                })));
            }
            let call = usage::UsageScan::from_payload(payload).map_err(DispatchError::Declared)?;
            let current = services.config.current();
            let settings = current.settings.clone();
            let host_id = crate::config::machine_slug();
            let preferences = current.preferences.clone();
            Ok(Answer::Deferred(Deferred::new(move || {
                call.run(settings, host_id, preferences)
            })))
        }
        // The payload is an empty struct in the contract, so there is nothing
        // to read out of it and nothing that can be wrong with it.
        SUBSCRIBE_SERVER_CONFIG => Ok(Answer::Stream(services.config.subscribe())),
        // Also an empty struct in the contract, and also nothing to read.
        SUBSCRIBE_AUTH_ACCESS => Ok(Answer::Stream(services.shell.subscribe_auth_access())),
        orchestration::DISPATCH_COMMAND => services
            .shell
            .dispatch(payload, &services.index, &services.config.current())
            .map(|answer| {
                // A project is a place skills are kept, so registering or
                // removing one changes what the `$` menu should offer. Done
                // here rather than inside the command because the registry and
                // the provider snapshot are different aggregates and only this
                // layer holds both — and after the dispatch, so a refused
                // command rescans nothing.
                if changes_where_skills_live(payload) {
                    let config = services.config.clone();
                    let probes = crate::provider::reserve_skill_rescan(&config);
                    let roots = services.shell.workspace_roots();
                    tokio::task::spawn_blocking(move || {
                        crate::provider::rescan_skills_reserved(&config, &roots, probes)
                    });
                }
                Answer::Value(answer)
            })
            .map_err(|refusal| DispatchError::Declared(refusal.to_error())),
        orchestration::SUBSCRIBE_SHELL => Ok(Answer::Stream(services.shell.subscribe(payload))),
        // Answered from the read loop, because it is the same work the shell
        // subscription already does to describe itself: one indexed read of the
        // project registry, and the conversations are in memory. The payload is
        // an empty struct in the contract, so there is nothing to read out of it.
        //
        // A registry that cannot be read is the method's *declared* error rather
        // than a defect. The panel that asks for this renders "Failed to load
        // archived threads" from a refusal (`archivedThreads.ts`) and would tear
        // down the whole socket on a defect.
        orchestration::GET_ARCHIVED_SHELL_SNAPSHOT => services
            .shell
            .archived_shell_snapshot()
            .map(Answer::Value)
            .map_err(|error| {
                DispatchError::Declared(declared(
                    "OrchestrationGetSnapshotError",
                    format_args!("Could not read the archived conversations: {error}"),
                ))
            }),
        threads::SUBSCRIBE_THREAD => Watch::read(payload)
            .and_then(|call| services.shell.threads().subscribe(&call))
            .map(Answer::Stream)
            .map_err(DispatchError::Declared),
        // One subagent's work stream. Answered from the read loop like the
        // thread subscription beside it — a stream is in memory, and the whole
        // point of the separate method is that it is *asked for* rather than
        // carried by every thread snapshot.
        crate::subagents::SUBSCRIBE_SUBAGENT => crate::subagents::Watch::read(payload)
            .and_then(|call| services.shell.threads().subagents().subscribe(&call))
            .map(Answer::Stream)
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
        // Reads the project looking for an icon and then signs what it found,
        // so it is deferred for the reason every other disk-touching method is.
        // The key is loaded on the read loop and not inside the deferred work:
        // it is one indexed row, and threading the database into the closure
        // would be a clone of it per favicon per sidebar render.
        assets::CREATE_URL => {
            let call = CreateUrl::read(payload, services.config.current().preferences.clone()).map_err(DispatchError::Declared)?;
            let secret = services
                .shell
                .database()
                .secret_or_create(assets::SIGNING_SECRET_NAME, assets::SIGNING_SECRET_BYTES)
                .map_err(|failure| DispatchError::Declared(call.signing_key_error(failure)))?;
            let now = crate::clock::now_epoch_millis() as i64;
            Ok(Answer::Deferred(Deferred::new(move || {
                call.run(&secret, now)
            })))
        }
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
        // The five ref-shaped methods. All of them run git and none of them
        // streams, so all of them are deferred — and the four that change
        // something take the registry with them, because a working tree they
        // moved is one a panel is still describing from before.
        refs::LIST_REFS => ListRefs::read(payload)
            .map(|call| Answer::Deferred(Deferred::new(move || call.run())))
            .map_err(DispatchError::Declared),
        refs::CREATE_REF => CreateRef::read(payload)
            .map(|call| deferred_on(&services.repositories, |kept| call.run(kept)))
            .map_err(DispatchError::Declared),
        refs::SWITCH_REF => SwitchRef::read(payload)
            .map(|call| deferred_on(&services.repositories, |kept| call.run(kept)))
            .map_err(DispatchError::Declared),
        refs::REMOVE_WORKTREE => RemoveWorktree::read(payload)
            .map(|call| deferred_on(&services.repositories, |kept| call.run(kept)))
            .map_err(DispatchError::Declared),
        refs::INIT => Init::read(payload)
            .map(|call| deferred_on(&services.repositories, |kept| call.run(kept)))
            .map_err(DispatchError::Declared),
        // The two diffs. Both run git over a range of the developer's own
        // history, so both are deferred; both take the registry with them,
        // because a diff is asked for by thread and has to be run in the folder
        // that thread's project is.
        checkpoints::GET_TURN_DIFF => Diff::read_turn(payload)
            .map(|call| deferred_in(&services.shell, |shell| call.run(shell)))
            .map_err(DispatchError::Declared),
        checkpoints::GET_FULL_THREAD_DIFF => Diff::read_thread(payload)
            .map(|call| deferred_in(&services.shell, |shell| call.run(shell)))
            .map_err(DispatchError::Declared),
        crate::provider::REFRESH => {
            let payload = payload.clone();
            let roots = services.shell.workspace_roots();
            Ok(deferred_over(&services.config, move |config| {
                crate::provider::refresh_call(&payload, config, &roots)
            }))
        }
        crate::provider_maintenance::UPDATE => {
            let payload = payload.clone();
            let roots = services.shell.workspace_roots();
            let maintenance = services.provider_maintenance.clone();
            Ok(deferred_over(&services.config, move |config| {
                maintenance.update_call(&payload, config, &roots)
            }))
        }
        // Reading the settings is reading memory — they were loaded at startup
        // and every change since has gone through the store — so it answers on
        // the read loop. The payload is an empty struct in the contract.
        settings::GET => Ok(Answer::Value(settings::public_value(
            &services.config.current().settings,
        ))),
        // The three that write a file do not. Small work, but a disk that is
        // busy is bounded by nothing this server controls, and the read loop
        // owes the next frame.
        settings::UPDATE => settings::Update::read(payload, &services.config.current().preferences)
            .map(|call| {
                // Read here rather than inside the deferred work: this is the
                // read loop, where the registry is a memory-speed question, and
                // a settings change that re-probes the provider needs the same
                // project list a startup probe would have used.
                let roots = services.shell.workspace_roots();
                deferred_over(&services.config, move |config| call.run(config, &roots))
            })
            .map_err(DispatchError::Declared),
        keybindings::UPSERT => Upsert::read(payload, &services.config.current().preferences)
            .map(|call| deferred_over(&services.config, |config| call.run(config)))
            .map_err(DispatchError::Declared),
        keybindings::REMOVE => {
            keybindings::Remove::read(payload, &services.config.current().preferences)
                .map(|call| deferred_over(&services.config, |config| call.run(config)))
                .map_err(DispatchError::Declared)
        }
        unknown => Err(DispatchError::UnknownMethod(unknown.to_string())),
    }
}

/// Defer work that needs the workspace index.
///
/// The index outlives the call by design — it is the server's, not the
/// connection's — so the closure takes a clone rather than a borrow of
/// `Services`, which the deferred task cannot hold.
/// Did this command change the set of directories skills are read from?
///
/// The two that touch the project registry, named by the same strings
/// `Command::parse` matches on. A command this does not recognise — including
/// every command about a conversation — changes nothing about the filesystem
/// the `$` menu is built from.
fn changes_where_skills_live(payload: &Value) -> bool {
    matches!(
        payload.get("type").and_then(Value::as_str),
        Some("project.create") | Some("project.delete")
    )
}

fn deferred_with(
    index: &Index,
    work: impl FnOnce(&Index) -> Result<Value, Value> + Send + 'static,
) -> Answer {
    let index = index.clone();
    Answer::Deferred(Deferred::new(move || work(&index)))
}

/// Defer work that changes a working tree the server is keeping a status for.
///
/// The registry outlives the call for the same reason the index does, and is
/// cloned for the same reason.
fn deferred_on(
    repositories: &Repositories,
    work: impl FnOnce(&Repositories) -> Result<Value, Value> + Send + 'static,
) -> Answer {
    let repositories = repositories.clone();
    Answer::Deferred(Deferred::new(move || work(&repositories)))
}

/// Defer work that has to find out which project a conversation is in.
///
/// The third of the same family, and cloned for the same reason: the registry
/// outlives the call, and the deferred task cannot hold a borrow of
/// [`Services`].
fn deferred_in(
    shell: &Shell,
    work: impl FnOnce(&Shell) -> Result<Value, Value> + Send + 'static,
) -> Answer {
    let shell = shell.clone();
    Answer::Deferred(Deferred::new(move || work(&shell)))
}

/// Defer work that changes what the developer configured.
///
/// The fourth of the same family. The store outlives the call — a subscription
/// is still describing the configuration long afterwards — and is cloned for the
/// reason the others are.
fn deferred_over(
    config: &ConfigStore,
    work: impl FnOnce(&ConfigStore) -> Result<Value, Value> + Send + 'static,
) -> Answer {
    let config = config.clone();
    Answer::Deferred(Deferred::new(move || work(&config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use crate::store::Database;
    use crate::threads::tests::a_thread;
    use serde_json::json;

    fn services() -> Services {
        let index = Index::new();
        Services {
            config: ConfigStore::new(ServerConfig::detect()),
            shell: Shell::new(Database::in_memory().expect("an in-memory database")),
            repositories: Repositories::new(&index),
            provider_maintenance: crate::provider_maintenance::ProviderMaintenance::new(),
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

    /// The cursor rule both subscriptions now share, read at the one place they
    /// share it. `Sequences::caught_up` decides what a cursor *means*; this
    /// decides what counts as one at all, and the two questions fail
    /// differently: a cursor misread here does not send the wrong snapshot, it
    /// opens a stream to a client with nothing to fold events into.
    ///
    /// The contract types the field `NonNegativeInt`
    /// (`packages/contracts/src/orchestration.ts`), so every case below except
    /// the first is a client that is already wrong. Reading those as absent is
    /// what routes them back to ticket 28's refusal instead of a silent stream.
    #[test]
    fn only_a_non_negative_integer_is_a_cursor() {
        assert_eq!(resume_cursor(&json!({"afterSequence": 4})), Some(4));
        assert_eq!(
            resume_cursor(&json!({"afterSequence": 0})),
            Some(0),
            "a client that has read an empty registry holds a real position"
        );

        assert_eq!(
            resume_cursor(&json!({"afterSequence": -1})),
            None,
            "negative"
        );
        assert_eq!(resume_cursor(&json!({"afterSequence": null})), None, "null");
        assert_eq!(
            resume_cursor(&json!({"afterSequence": "3"})),
            None,
            "a string"
        );
        assert_eq!(
            resume_cursor(&json!({"afterSequence": 1.5})),
            None,
            "not whole"
        );
        assert_eq!(
            resume_cursor(&json!({})),
            None,
            "a client that holds nothing"
        );
    }

    #[test]
    fn get_config_returns_the_config() {
        let services = services();
        let answer = dispatch_without_grant(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(answer), services.config.current().to_value());
    }

    /// The client re-sends `server.getConfig` as its liveness probe, so a
    /// second call has to answer identically rather than consume anything.
    #[test]
    fn get_config_is_repeatable() {
        let services = services();
        let first = dispatch_without_grant(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        let second = dispatch_without_grant(&services, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(first), value(second));
    }

    /// The empty answer is the contract's — `success: Schema.Struct({})` — and
    /// a probe that answered anything else would be a probe with a reason to
    /// fail. Repeatable for the reason `server.getConfig` is: this is what the
    /// client sends on a timer for the life of the connection.
    #[test]
    fn probe_answers_empty_every_time() {
        let services = services();
        for _ in 0..3 {
            let answer = dispatch_without_grant(&services, SERVER_PROBE, &json!({})).expect("dispatches");
            assert_eq!(value(answer), json!({}));
        }
    }

    /// Every subscription is dispatched by the same path as a unary call and
    /// only parts company at the answer.
    #[test]
    fn the_subscriptions_answer_with_a_stream() {
        let services = services();
        // Against a thread that exists, because a subscription to one that does
        // not is a refusal rather than a stream — see
        // [`a_refused_call_fails_with_the_methods_own_declared_error`], which
        // holds that half, and [`crate::threads::Threads::subscribe`] for why.
        services
            .shell
            .threads()
            .create(a_thread("thread-1"))
            .expect("created");

        for (tag, payload) in [
            (SUBSCRIBE_SERVER_CONFIG, json!({})),
            (orchestration::SUBSCRIBE_SHELL, json!({})),
            (threads::SUBSCRIBE_THREAD, json!({"threadId": "thread-1"})),
            (terminal::SUBSCRIBE_METADATA, json!({})),
        ] {
            let answer = dispatch_without_grant(&services, tag, &payload).expect("dispatches");
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
            // The draft the composer opens on. Ticket 28: this has to be the
            // *declared* error, because the client reads a refusal as "ask me
            // again in 250ms" and a defect as "this whole socket is broken".
            (
                threads::SUBSCRIBE_THREAD,
                json!({"threadId": "a-thread-nothing-created"}),
                "OrchestrationGetSnapshotError",
            ),
        ] {
            let error = dispatch_without_grant(&services, tag, &payload).expect_err("a refusal");

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
            let error = dispatch_without_grant(&services, tag, &payload).expect_err("a refusal");

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
            let answer = dispatch_without_grant(&services, tag, &payload).expect("dispatches");
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
            let answer = dispatch_without_grant(&services, tag, &named).expect("dispatches");
            assert!(
                matches!(answer, Answer::Deferred(_)),
                "{tag} answered inline"
            );
        }

        // …and clearing does not, because it is arithmetic on a string the
        // registry already holds.
        let answer = dispatch_without_grant(&services, terminal::CLEAR, &named);
        assert!(
            matches!(answer, Err(DispatchError::Declared(_))),
            "a clear of a terminal that is not there is answered where it is asked"
        );
    }

    /// The tag has to survive into the error, because it is the only thing
    /// that tells a developer which of the seventy methods is missing.
    ///
    /// Named `no.such.method` rather than a real one since ticket 39: a method
    /// the contract declares is now refused under a tag *it* declares, and only
    /// a tag the contract has never heard of comes back as not-implemented. The
    /// case below is the other half.
    #[test]
    fn an_unknown_tag_becomes_a_typed_error_naming_the_method() {
        let services = services();
        let error = dispatch_without_grant(&services, "no.such.method", &json!({})).expect_err("not implemented");
        assert_eq!(
            error,
            DispatchError::UnknownMethod("no.such.method".to_string())
        );

        let payload = error.to_error();
        assert_eq!(payload["_tag"], "ServerMethodNotImplementedError");
        assert_eq!(payload["method"], "no.such.method");
        assert!(payload["message"]
            .as_str()
            .expect("a message")
            .contains("no.such.method"));
    }

    /// Every method this server refuses, against the union each one declares in
    /// `packages/contracts/src/rpc.ts`.
    ///
    /// The enumeration is the point. [`crate::refusals`] proves the table
    /// matches the contract; this proves that *dispatch* is what consults it —
    /// that no method slips past into a refusal built somewhere else — and it
    /// finds the refused set by asking, rather than by holding a second list
    /// that could drift from the one above.
    ///
    /// An empty payload for every call, which is safe because each method reads
    /// its payload before it does anything: the implemented ones answer or
    /// refuse in their own words and are skipped here, and the unimplemented
    /// ones never get that far.
    #[test]
    fn every_method_this_server_refuses_answers_under_its_own_union() {
        let services = services();
        let declared = crate::refusals::contract::declared_unions();
        let mut refused = std::collections::BTreeSet::new();

        for (method, union) in &declared {
            let Err(DispatchError::UnknownMethod(_)) = dispatch_without_grant(&services, method, &json!({}))
            else {
                continue;
            };
            refused.insert(method.as_str());

            let error = DispatchError::UnknownMethod(method.clone()).to_error();
            let tag = error["_tag"].as_str().expect("a tag");
            crate::refusals::contract::assert_declares(method, tag, union);
        }

        // One named method rather than a count of them, so that this says
        // something the code decided rather than roughly how much is left to
        // build. It is the method `tests/socket_handshake.rs` refuses end to
        // end, and it is here to catch the loop above running zero times —
        // which is how an enumeration passes while checking nothing.
        //
        // It was `orchestration.replayEvents` until that method left the
        // contract. The replacement is chosen to be the last one standing:
        // preview automation is last in `.scratch/contract-parity/ledger.md`'s
        // order, and it is the one cluster that waits on something the contract
        // does not declare at all — there is no MCP server here to ask for a
        // click, so answering the method would build a router with no traffic.
        // When even this is implemented the whole assertion goes, because
        // `refused` will be empty and there will be nothing left to name.
        assert!(
            refused.contains("previewAutomation.respond"),
            "dispatch answers previewAutomation.respond, so this checked \
             nothing it was written to check: {refused:?}"
        );
    }

    /// Both diffs run git over a range of the developer's history, so neither
    /// may be answered where the socket's only reader is waiting for it.
    #[test]
    fn the_diff_methods_answer_with_deferred_work() {
        let services = services();
        let asked = json!({
            "threadId": "thread-1",
            "fromTurnCount": 0,
            "toTurnCount": 1,
        });

        for tag in [
            checkpoints::GET_TURN_DIFF,
            checkpoints::GET_FULL_THREAD_DIFF,
        ] {
            let answer = dispatch_without_grant(&services, tag, &asked).expect("dispatches");
            assert!(
                matches!(answer, Answer::Deferred(_)),
                "{tag} answered inline"
            );
        }
    }

    /// A diff for a conversation this server has never heard of is refused under
    /// the asking method's own tag — the client decodes each against a union of
    /// one, so the other method's error would cost the call rather than show the
    /// sentence.
    #[test]
    fn a_diff_of_an_unknown_conversation_fails_with_the_methods_own_error() {
        let services = services();
        let asked = json!({
            "threadId": "thread-1",
            "fromTurnCount": 0,
            "toTurnCount": 1,
        });

        for (tag, expected) in [
            (checkpoints::GET_TURN_DIFF, "OrchestrationGetTurnDiffError"),
            (
                checkpoints::GET_FULL_THREAD_DIFF,
                "OrchestrationGetFullThreadDiffError",
            ),
        ] {
            let answer = dispatch_without_grant(&services, tag, &asked).expect("dispatches");
            let Answer::Deferred(work) = answer else {
                panic!("{tag} answered inline");
            };
            let error = work.run().expect_err("a refusal");
            assert_eq!(error["_tag"], expected, "{tag}");
            assert!(error["message"].is_string(), "{tag}: {error}");
        }
    }
}
