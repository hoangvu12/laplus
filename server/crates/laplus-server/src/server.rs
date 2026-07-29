//! The socket endpoint: one `GET /ws` on loopback, and the connection loop
//! behind it.
//!
//! Everything the UI *does* goes through this one endpoint, and — per ticket
//! 01's captures — there is no transport-level handshake either: the socket
//! opens and the client's first frame is already a `Request`. The handful of
//! plain `GET`s routed beside it are not a REST surface: two are the boot
//! handshake and two answer with payloads this socket already carries, so that
//! the client's HTTP fast path stops being a guaranteed miss. All four are
//! [`crate::http`]'s, and none of them is a way to *change* anything.
//!
//! A connection is three parts, and the split is what streaming needs:
//!
//! - a **read loop**, which owns the incoming half and the subscription
//!   registry, and is the only thing that touches either;
//! - a **frame queue**, which everything writes into;
//! - a **writer task**, which owns the outgoing half and drains the queue.
//!
//! The sink has one owner because a subscription's pump and the read loop both
//! produce frames. Correlation is by `requestId` rather than by order, so
//! interleaving them is not merely tolerable — it is what the reference server
//! does, and a client that assumed otherwise would already be broken against
//! it.
//!
//! A unary call that answers from memory is still answered inline, in arrival
//! order — the waiting is nil and the ordering is free. A call that has to wait
//! on the world is not: it comes back from dispatch as
//! [`Answer::Deferred`](crate::rpc::Answer) and is run on a blocking thread
//! that writes its own `Exit`, so the read loop stays free to take the next
//! frame. Ticket 06's file tree is the first method that needed this, and the
//! reason is spelled out on [`crate::rpc::Deferred`].

use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, RawQuery, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};

use crate::auth::{self, AuthInvalidBody, Credential, Rejection, UpgradeRequest};
use crate::config::ServerConfig;
use crate::config_store::ConfigStore;
use crate::filesystem::Index;
use crate::git::Repositories;
use crate::orchestration::Shell;
use crate::pairing;
use crate::process::Search;
use crate::rpc::{Answer, Deferred, Services};
use crate::store::{Database, NewPairingLink, NewSession, StorageError};
use crate::subscriptions::Subscriptions;
use crate::terminal::Terminals;
use crate::ui::Assets;
use crate::wire::{Cause, ClientMessage, Exit, RequestId, ServerMessage};
use crate::{http, rpc};

/// How many frames may be waiting for the socket before whoever produced them
/// has to wait too.
///
/// This is the second half of the bound on a slow client. The first is
/// [`crate::subscriptions::BACKLOG`], which caps what one subscription will
/// hold; this caps what the connection will hold across all of them. A pump
/// blocked here is simply not producing, which is the correct response to a
/// client that is not reading.
const FRAME_QUEUE: usize = 64;

/// Everything a connection can read. One instance, shared by every socket.
#[derive(Debug)]
pub struct ServerState {
    services: Services,
    /// The UI, if this server was given one. Empty for the plain binary and for
    /// every server the suite starts; the shell is the only caller that brings
    /// a bundle. See [`crate::ui`] for why the assets are served from here at
    /// all rather than from the webview's own scheme handler.
    ui: Assets,
    /// Flipped once when the server is asked to stop. Open sockets watch it,
    /// because `axum`'s graceful shutdown waits for connections to end and a
    /// long-lived socket never would on its own.
    shutdown: watch::Receiver<bool>,
    live_connections: AtomicUsize,
    /// Shared with every connection's registry rather than owned by it, so
    /// the number is the server's and not one socket's.
    live_subscriptions: Arc<AtomicUsize>,
    fiber_ids: AtomicU64,
    unrecognized_messages: AtomicUsize,
    unparseable_frames: AtomicUsize,
}

impl ServerState {
    fn new(services: Services, ui: Assets, shutdown: watch::Receiver<bool>) -> Self {
        ServerState {
            services,
            ui,
            shutdown,
            live_connections: AtomicUsize::new(0),
            live_subscriptions: Arc::new(AtomicUsize::new(0)),
            fiber_ids: AtomicU64::new(1),
            unrecognized_messages: AtomicUsize::new(0),
            unparseable_frames: AtomicUsize::new(0),
        }
    }

    pub fn config(&self) -> &ConfigStore {
        &self.services.config
    }

    /// The folder of every registered project, for [`crate::catalogue`]'s scan.
    /// See [`crate::orchestration::Shell::workspace_roots`], which is where the
    /// question is actually answered.
    pub fn workspace_roots(&self) -> Vec<std::path::PathBuf> {
        self.services.shell.workspace_roots()
    }

    /// Sockets currently open. A gauge rather than a counter: it is how
    /// "disconnection does not leak server state" is observed from outside,
    /// and it is the number to look at when the app feels stuck.
    pub fn live_connections(&self) -> usize {
        self.live_connections.load(Ordering::Relaxed)
    }

    /// Subscriptions currently being pumped, across every connection. The
    /// streaming half of the same accounting: a subscription outlives the call
    /// that opened it, so this is where a stream that was never released shows
    /// up.
    pub fn live_subscriptions(&self) -> usize {
        self.live_subscriptions.load(Ordering::Relaxed)
    }

    /// Workspaces the server is watching for changes it did not make. The third
    /// of the same family of gauges: it is how "watchers are released when a
    /// project is closed" is observed from outside, without a test reaching
    /// into the index to look.
    pub fn watched_workspaces(&self) -> usize {
        self.services.index.watched()
    }

    /// Agent processes currently running. The fourth of the family, and the one
    /// that makes "the subprocess is terminated and reaped when the session
    /// ends" something a test can observe rather than assert about internals.
    pub fn live_agents(&self) -> usize {
        self.services.shell.threads().live_agents()
    }

    /// Shells still running behind a terminal. The fifth of the family, and the
    /// one that makes "closing the app reaps its terminals" something a test
    /// can observe rather than assert about internals.
    pub fn live_terminals(&self) -> usize {
        self.services.terminals.live()
    }

    /// How often the buffered assistant message and the deltas before it agreed.
    /// See [`crate::threads::Reconciliation`], which is the check itself.
    pub fn reconciliation(&self) -> crate::threads::Reconciliation {
        self.services.shell.threads().reconciliation()
    }

    fn subscription_gauge(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.live_subscriptions)
    }

    /// The id to name in an `Interrupt` cause.
    ///
    /// The reference server puts a real runtime fiber id here (2494, 1836 in
    /// the captures). laplus has no fibers, so this is the nearest true
    /// thing: a distinct number per cancelled call, which is what a fiber id
    /// is from the client's side. The client decodes it and does not act on
    /// it — the `_tag` is what tells it the stream ended normally.
    fn next_fiber_id(&self) -> u64 {
        self.fiber_ids.fetch_add(1, Ordering::Relaxed)
    }

    /// Frames whose `_tag` this build does not know. The socket's half of the
    /// same drift accounting [`crate::protocol`] keeps for the CLI: a client
    /// that learns a new message type must not be able to kill a connection,
    /// and the fact that it happened must not vanish.
    pub fn unrecognized_messages(&self) -> usize {
        self.unrecognized_messages.load(Ordering::Relaxed)
    }

    /// Frames that were not parseable at all.
    pub fn unparseable_frames(&self) -> usize {
        self.unparseable_frames.load(Ordering::Relaxed)
    }
}

/// A running server.
///
/// Dropping this does not stop it; call [`Server::shutdown`]. That is
/// deliberate — the Tauri shell in ticket 23 holds one for the life of the
/// process, and the test harness wants an explicit stop it can await.
pub struct Server {
    local_addr: SocketAddr,
    state: Arc<ServerState>,
    shutdown: watch::Sender<bool>,
    serving: tokio::task::JoinHandle<()>,
    /// The credential this server booted with, for [`Server::window_url`].
    ///
    /// `None` if minting it failed, which on a healthy machine does not happen
    /// — it is one row and one call to the operating system's randomness. Held
    /// rather than re-read from the database because the whole point of it is
    /// that it never travels: the row stores it so it can be *verified*, and
    /// this is the only copy anything reads back out.
    boot_credential: Option<String>,
}

/// Why the server did not start.
///
/// Two distinct failures with two distinct fixes — a port already in use is
/// something the user can change, an unusable database is not — so they are not
/// flattened into one `io::Error` whose message would have to guess which
/// happened.
#[derive(Debug)]
pub enum StartupFailure {
    Database(StorageError),
    Listen { port: u16, error: std::io::Error },
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupFailure::Database(error) => write!(formatter, "{error}"),
            StartupFailure::Listen { port, error } => {
                write!(formatter, "cannot listen on 127.0.0.1:{port}: {error}")
            }
        }
    }
}

impl std::error::Error for StartupFailure {}

impl Server {
    /// Bind to loopback on `port`, open the registry, and start serving. Port 0
    /// asks the OS for a free one, which is what the tests use.
    ///
    /// Loopback is not a default here, it is the security model: v1 has no
    /// identity store, so reachability *is* the boundary.
    ///
    /// `ui` is the web bundle to serve, or [`Assets::none`] for a server that
    /// only answers calls. The shell passes one; the plain binary does not,
    /// which is what keeps `cargo run` a socket endpoint the real UI can be
    /// pointed at from a development server.
    pub async fn bind(port: u16, ui: Assets) -> Result<Server, StartupFailure> {
        let database =
            Database::open(&crate::store::default_path()).map_err(StartupFailure::Database)?;
        let server = Server::bind_with(port, ServerConfig::detect(), database, ui)
            .await
            .map_err(|error| StartupFailure::Listen { port, error })?;
        server.probe_provider();
        Ok(server)
    }

    /// Serve a database the caller already opened.
    ///
    /// The seam the tests use: an in-memory registry for the ones that have
    /// nothing to say about persistence, and a temporary file for the ones that
    /// do. It is not a test-only entry point — ticket 23's Tauri shell will
    /// want the same control over where the app's state lives.
    ///
    /// Binding does **not** go looking for the agent binary; [`Server::bind`]
    /// does that as a second step, and a shell assembling its own startup wants
    /// [`Server::probe_provider`] in the same place.
    pub async fn bind_with(
        port: u16,
        config: ServerConfig,
        database: Database,
        ui: Assets,
    ) -> std::io::Result<Server> {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        // A server that ships a UI answers with that UI's version rather than
        // this crate's. Here rather than in either caller because both of them
        // hand over a config and a bundle in the same breath, and the one place
        // where the two are together is the place that cannot forget.
        // `ServerConfig::serving_ui_version` is why it is done at all.
        let config = match ui.version() {
            Some(version) => config.serving_ui_version(version),
            None => config,
        };
        // The index is built first because the working trees listen to its
        // watcher: there is one watcher in the process and both the file tree
        // and the status are kept fresh by it.
        let index = Index::new();
        let services = Services {
            // `opening` rather than `new`: what the developer configured last
            // time is read in here, and a file that will not read is an issue
            // in the payload rather than a server that will not start.
            config: ConfigStore::opening(config),
            shell: Shell::new(database),
            repositories: Repositories::new(&index),
            index,
            terminals: Terminals::new(),
        };
        let state = Arc::new(ServerState::new(services, ui, shutdown.subscribe()));

        // Minted before the listener exists, so that there is no window in
        // which the server is answering and the credential it will admit its
        // own window with does not yet exist.
        //
        // A failure here is logged and survived rather than returned. The
        // server still starts, `window_url` answers `None`, and the developer
        // lands on the pairing screen where they can type a code from
        // somewhere else — which is worse than booting cleanly and much better
        // than an application that will not open.
        let boot_credential = match mint_boot_grant(&state) {
            Ok(credential) => Some(credential),
            Err(error) => {
                eprintln!("laplus: cannot mint the credential this window boots with: {error}");
                None
            }
        };

        let app = Router::new()
            .route("/ws", get(upgrade))
            // The two answers the UI needs before it will open the socket at
            // all. See `crate::http`.
            .route("/.well-known/t3/environment", get(environment_descriptor))
            .route("/api/auth/session", get(auth_session))
            // The two the UI asks for *instead of* the socket, and falls back
            // to the socket without. Real routes rather than fallback paths for
            // a second reason beyond answering them: `/api/orchestration/…` has
            // no extension, so the asset fallback would otherwise hand a thread
            // id to the UI's own router and answer a `fetch` with an HTML page.
            .route("/api/orchestration/shell", get(shell_snapshot))
            .route(
                "/api/orchestration/threads/{threadId}",
                get(thread_snapshot),
            )
            // Ticket 73's five, in the order a phone walks them: mint a code
            // in Settings on the PC, trade it for a bearer, trade that for a
            // socket ticket. The last two are Settings' own view of what it has
            // handed out.
            //
            // The paths are `EnvironmentAuthHttpApi`'s and not the ticket's:
            // the ticket names `/api/auth/pairing-credential`,
            // `/api/auth/revoke-pairing-link` and a `GET` for the ticket, and
            // the contract says otherwise in all three places. The contract is
            // what the client is built from, so the contract wins — recorded in
            // the ticket's own Comments.
            .route("/api/auth/browser-session", post(browser_session))
            .route("/oauth/token", post(token_exchange))
            .route("/api/auth/websocket-ticket", post(websocket_ticket))
            .route("/api/auth/pairing-token", post(pairing_credential))
            .route("/api/auth/pairing-links", get(pairing_links))
            .route("/api/auth/pairing-links/revoke", post(revoke_pairing_link))
            // A file out of a project, for an `<img>` the browser fetches
            // itself. A real route and not the fallback for the same reason as
            // the two above it: a token has no extension, so the asset fallback
            // would answer this with the UI's own `index.html`.
            //
            // Two segments rather than a wildcard, because the filename is a
            // basename and [`crate::assets`] percent-encodes the separator out
            // of it. See that module for why nothing here checks a credential.
            .route("/api/assets/{token}/{name}", get(project_asset))
            // Last, and only for paths nothing above matched: the UI itself.
            // A route wins over a fallback, so attaching a bundle cannot move
            // an answer the client already decodes — which is the property
            // `tests/http_ui.rs` pins.
            //
            // `any` rather than `get`, and the method checked inside: a
            // `get`-only fallback answers a `POST` with 405, which tells the
            // client the path exists. Nothing here exists that is not a file,
            // so every method that is not a read is the same 404 as a path
            // that is not there.
            .fallback(any(asset))
            .with_state(Arc::clone(&state));

        // Loopback unless the developer turned the switch on, and read from the
        // configuration this server was built with rather than from the disk
        // again — `remote-access.json` is read once, at `ServerConfig::detect`,
        // so the address bound and the origins admitted cannot come from two
        // different readings of one file.
        //
        // `docs/adr/0022` is why binding wider is allowed at all. The short
        // version: `0019` made every request carry a credential that verifies,
        // which is the thing that was missing when `0015` reasoned that
        // loopback *was* the boundary.
        let bind = state.config().current().remote_access.bind_address();
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((bind, port))).await?;
        let local_addr = listener.local_addr()?;

        let serving = tokio::spawn(async move {
            let served = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    // `changed()` only errors once every sender is gone, which
                    // would mean the `Server` was dropped — also a reason to
                    // stop accepting.
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(error) = served {
                eprintln!("laplus: socket endpoint stopped: {error}");
            }
        });

        Ok(Server {
            local_addr,
            state,
            shutdown,
            serving,
            boot_credential,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// What [`Server::reachable_addr`] does to a wildcard, without a listener.
    ///
    /// Split out so the rule can be tested at all: binding `0.0.0.0` in the
    /// suite means a test that opens a port to the network on whoever's machine
    /// is running it, and the thing worth pinning is the arithmetic rather than
    /// the socket.
    fn reachable_from(bound: SocketAddr) -> SocketAddr {
        if bound.ip().is_unspecified() {
            return SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, bound.port()));
        }
        bound
    }

    /// The URL the UI connects to.
    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.reachable_addr())
    }

    /// Where a client on *this machine* reaches this server.
    ///
    /// [`Server::local_addr`] is the socket's own address, and once the network
    /// access switch exists that is `0.0.0.0` — a wildcard meaning "every
    /// interface", which is a thing to bind and not a thing to connect to. A
    /// browser sent there answers `ERR_ADDRESS_INVALID`, which is exactly what
    /// the window did the first time the switch was turned on.
    ///
    /// So every URL this server hands out for local use names loopback
    /// explicitly, and only the *port* comes from the listener. The addresses
    /// other machines use are [`crate::endpoints`]'s, which is a different
    /// question with a different answer.
    fn reachable_addr(&self) -> SocketAddr {
        Server::reachable_from(self.local_addr)
    }

    /// The URL the UI is *at*. What the shell points its window at, and the
    /// origin every request the window makes will carry.
    pub fn http_url(&self) -> String {
        format!("http://{}/", self.reachable_addr())
    }

    /// The URL to open the UI *with a credential already in hand*.
    ///
    /// [`Server::http_url`] with the boot grant in the fragment:
    ///
    /// ```text
    /// http://127.0.0.1:4773/#token=ABCD2345WXYZ
    /// ```
    ///
    /// **The fragment is the whole point.** A URL fragment is never sent to the
    /// server — the browser keeps it and hands it to the page's JavaScript — so
    /// this credential reaches the window without ever travelling over HTTP,
    /// and a request arriving from anywhere else cannot ask for it. That is
    /// what makes it the equivalent of the reference server's Electron preload
    /// hand-off (`PairingGrantStore.ts:314-330`), which laplus cannot copy
    /// directly: its window has no channel to the shell that is not this
    /// server. `issueStartupPairingUrl` (`EnvironmentAuth.ts:911-921`) builds
    /// the same URL upstream, for the same reason.
    ///
    /// The client half already exists and is untouched: `setPairingTokenOnUrl`
    /// and `getPairingTokenFromUrl` in `packages/shared/src/remote.ts` are what
    /// puts a token in a fragment and reads it back, and `PairingRouteSurface`
    /// spends it and strips it from the address bar.
    ///
    /// `None` when the boot grant could not be minted. The caller opens
    /// [`Server::http_url`] instead — a window that lands on the pairing screen
    /// is recoverable, and one that never opens is not.
    pub fn window_url(&self) -> Option<String> {
        self.boot_credential
            .as_ref()
            .map(|credential| format!("{}#token={credential}", self.http_url()))
    }

    pub fn state(&self) -> &Arc<ServerState> {
        &self.state
    }

    /// Go looking for the agent binary on this machine's `PATH`, and publish what
    /// was found.
    ///
    /// Returns immediately; the answer arrives on the configuration subscription
    /// whenever the lookup is done. That is the whole reason it is not part of
    /// binding: resolving means walking `PATH` and then waiting on a child
    /// process, and a socket that did not open until the agent had answered would
    /// not open at all on a machine where the agent is wedged. Until it lands, the
    /// configuration reports no provider instance, which is the state upstream's
    /// UI renders as "Checking provider status".
    ///
    /// Not a startup-only call. The reference server re-probes every five minutes
    /// and after any change to the provider's settings; this is the method those
    /// will use.
    ///
    /// Blocking work on a blocking thread, for the same reason every filesystem
    /// method is a [`Deferred`]. The task is untracked, like a deferred call's:
    /// it holds a clone of the configuration store and nothing else, so if the
    /// server is dropped first it publishes into a store nobody is reading and
    /// the thread comes back.
    pub fn probe_provider(&self) {
        let config = self.state.config().clone();
        // Taken before the task rather than inside it, so the blocking half holds
        // paths and not a handle to the registry they came from.
        let roots = self.state.workspace_roots();
        tokio::task::spawn_blocking(move || {
            crate::provider::refresh(&config, &Search::from_environment(), &roots)
        });
    }

    /// Stop accepting, close open sockets, end every agent session and every
    /// terminal, write down what the agents said, and wait for all of it to
    /// actually be done.
    ///
    /// The order is the whole content of this method. The agents are reaped
    /// before the transcripts are flushed, because a session publishes its last
    /// changes on the way down and those are exactly the ones a flush that ran
    /// first would miss — the last message of a conversation is the one a
    /// developer notices missing. And the agents are waited for rather than
    /// asked: a `claude` outliving the server that started it is the one leak
    /// this process can produce that survives the process, since it holds the
    /// project's files open and keeps talking to an API on the developer's
    /// account.
    ///
    /// The terminals are the second thing with that property and they are
    /// waited for too. A shell left behind holds the project's files open, and
    /// whatever the developer had running in it goes on running with nothing
    /// left that can show it to them.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.serving.await;
        self.state.services.shell.threads().shutdown().await;
        self.state.services.terminals.shutdown().await;
        self.state.services.shell.flush().await;
    }

    /// Serve until the process is interrupted. This is what the binary calls.
    pub async fn serve_until_interrupted(self) {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("laplus: cannot listen for interrupt: {error}");
        }
        self.shutdown().await;
    }
}

/// Mint the credential this server will admit its own window with.
///
/// **Administrative scopes**, unlike a code minted for a phone. The window is
/// the machine's own console: it manages the pairing links, which is
/// `access:read` and `access:write`, and nothing is served by a window that
/// cannot open its own Settings panel. The reference server grants its boot
/// grant the same set (`PairingGrantStore.ts:318`).
///
/// Nothing gates on a scope in this server — see [`crate::pairing`] — so this
/// is what the session *reports*, not what it is permitted. It matters because
/// the UI reads the reported scopes to decide which panels to offer.
fn mint_boot_grant(state: &ServerState) -> Result<String, Box<dyn std::error::Error>> {
    let id = pairing::record_id()?;
    let credential = pairing::pairing_code()?;
    state.services.shell.database().issue_desktop_boot_grant(
        &id,
        &credential,
        &pairing::administrative_scopes(),
    )?;
    Ok(credential)
}

async fn environment_descriptor(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config().current();
    Json(http::environment_descriptor(&config).clone()).into_response()
}

/// `GET /api/auth/session` — "am I signed in, and as what?"
///
/// **A refusal here is answered `200 {authenticated: false}` and not `401`**,
/// which is the one place in this server that reads a [`authorized`] `Err` and
/// throws the response away. This route is the probe a client makes *before* it
/// holds anything: `bootstrapServerAuth` calls it first and exchanges its boot
/// credential only if the answer is `false`. A 401 would be the same
/// information in a shape the client treats as a transport failure and retries.
///
/// Until this route looked at the request at all it answered `true` to
/// everyone, and that is what stopped the window connecting — see
/// [`crate::http::AuthSessionState`], which carries the whole reasoning.
async fn auth_session(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let config = state.config().current();
    let state = match authorized(&state, query.as_deref(), &headers) {
        Ok((presented, grant)) => {
            http::authenticated_session(&config, grant.scopes, session_method(presented.shape))
        }
        Err(_) => http::unauthenticated_session(&config),
    };
    Json(state.to_value()).into_response()
}

/// Which of `ServerAuthSessionMethod`'s three a verified credential was.
///
/// `None` for the two shapes that are not one of the three: a `wsTicket` opens
/// a socket rather than holding a session, and a DPoP token never reaches here
/// because [`authorized`] refuses it. Guessing on either would put a string on
/// the wire that the contract's closed union does not have, which costs the
/// client the decode of the whole response rather than the one field.
fn session_method(shape: auth::Credential) -> Option<&'static str> {
    match shape {
        auth::Credential::BearerToken => Some(pairing::BEARER_SESSION_METHOD),
        auth::Credential::SessionCookie => Some(pairing::BROWSER_SESSION_METHOD),
        auth::Credential::WebSocketTicket | auth::Credential::DpopToken => None,
        auth::Credential::Absent => None,
    }
}

/// `GET /api/orchestration/shell` — the project list, over HTTP.
///
/// The same object the shell subscription opens with. Answered from the
/// registry, which is a read of two tables and the one thing here that can
/// fail.
async fn shell_snapshot(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    match state.services.shell.shell_snapshot() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => {
            // Said out loud, because the client's answer to a failed fetch is
            // to fall back to the socket — where the subscription is about to
            // fail to describe the registry for the same reason and be just as
            // quiet about it.
            eprintln!("laplus: cannot describe the project registry over HTTP: {error}");
            refuse(http::shell_snapshot_unavailable())
        }
    }
}

/// `GET /api/orchestration/threads/{threadId}` — one conversation, over HTTP.
///
/// Answered from memory, so the only outcome besides the snapshot is that this
/// server does not hold the thread — which is the ordinary case for a "New
/// thread" pane, and a typed 404 rather than a bare one for exactly that
/// reason. See [`crate::http::thread_not_found`].
async fn thread_snapshot(
    State(state): State<Arc<ServerState>>,
    Path(thread_id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    match state.services.shell.threads().detail_snapshot(&thread_id) {
        Some(snapshot) => Json(snapshot).into_response(),
        None => refuse(http::thread_not_found()),
    }
}

// --- ticket 73: the pairing routes -------------------------------------------
//
// Five handlers, and between them they hold no policy: what a credential is
// belongs to `crate::pairing`, the single-use guarantee belongs to the SQL in
// `crate::store`, and the bodies belong to `crate::http`. What is here is the
// order those are called in and which refusal each failure wears — which is the
// part that needs a web framework and so is the part that has to live beside
// one.
//
// **Four of the six go through `authorized` and two through `origin_admitted`.**
// The two are `/api/auth/browser-session` and `/oauth/token`, which take their
// credential in the body because they are how a client that holds nothing comes
// to hold something. Every other route in this file — and the socket upgrade,
// and both snapshot routes — needs a credential that verifies.

/// `POST /api/auth/browser-session` — trade a pair code for a session cookie.
///
/// **The route the desktop window and a phone both actually take.** A browser
/// that loaded the app from this server is talking to its *primary*
/// environment, and the client's primary path is `exchangeBootstrapCredential`
/// → `client.auth.browserSession`
/// (`apps/web/src/environments/primary/auth.ts:231-240`). `/oauth/token` below
/// is for a client that added this machine as a *second* backend from
/// somewhere else.
///
/// Ticket 73's scope list omits this route. That is a gap in the ticket: it
/// points at `packages/client-runtime/src/authorization/remote.ts` as "the
/// client half, already written", and that file is the remote half. The primary
/// half is in `apps/web` and reaches for a cookie.
///
/// Like `/oauth/token`, the credential is in the body, so this cannot be made
/// to require one in a header — it is how a browser with nothing gets a
/// session.
async fn browser_session(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(refused) = origin_admitted(&state, query.as_deref(), &headers) {
        return refused;
    }

    let credential = match http::read_browser_session_request(&body) {
        Ok(credential) => credential,
        Err(problem) => {
            eprintln!("laplus: cannot read the credential to open a browser session ({problem:?})");
            return refuse(http::unsupported_request());
        }
    };

    let database = state.services.shell.database();
    let grant = match database.consume_pairing_link(&credential) {
        Ok(Ok(grant)) => grant,
        Ok(Err(refusal)) => {
            return unauthorized(auth::Rejection::invalid_credential(format!(
                "refusing to open a browser session: {}",
                refusal.detail()
            )));
        }
        Err(error) => {
            eprintln!("laplus: cannot spend a pairing code: {error}");
            return refuse(http::browser_session_unavailable());
        }
    };

    let (session_id, token) = match (pairing::record_id(), pairing::opaque_token()) {
        (Ok(session_id), Ok(token)) => (session_id, token),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("laplus: cannot mint a session token: {error}");
            return refuse(http::browser_session_unavailable());
        }
    };

    match database.issue_session(NewSession {
        session_id: &session_id,
        token: &token,
        subject: &grant.subject,
        scopes: &grant.scopes,
        method: pairing::BROWSER_SESSION_METHOD,
        label: grant.label.as_deref(),
    }) {
        Ok(session) => (
            [(
                header::SET_COOKIE,
                http::session_cookie(&session.token, session.expires_in),
            )],
            Json(http::browser_session(&session)),
        )
            .into_response(),
        Err(error) => {
            eprintln!("laplus: cannot open a browser session: {error}");
            refuse(http::browser_session_unavailable())
        }
    }
}

/// `POST /oauth/token` — trade a pair code for a bearer good for thirty days.
///
/// Its credential is in the **body** rather than the headers, so it goes
/// through [`origin_admitted`] rather than [`authorized`] — a route that
/// required a session would be requiring the thing it exists to issue.
///
/// Reached by a client that added *this machine* as a second backend from
/// somewhere else. A browser that loaded the app from this server takes
/// [`browser_session`] above instead.
async fn token_exchange(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(refused) = origin_admitted(&state, query.as_deref(), &headers) {
        return refused;
    }

    // Form-urlencoded, because `AuthTokenExchangeRequest` ends
    // `.pipe(HttpApiSchema.asFormUrlEncoded())` and RFC 6749 says so. See
    // `crate::http::form_fields` for why not `axum`'s `Form`.
    let fields = http::form_fields(&body);
    let field = |name| http::form_field(&fields, name);

    if field("grant_type") != Some(pairing::TOKEN_EXCHANGE_GRANT_TYPE)
        || field("subject_token_type") != Some(pairing::BOOTSTRAP_TOKEN_TYPE)
        || field("requested_token_type") != Some(pairing::ACCESS_TOKEN_TYPE)
    {
        eprintln!("laplus: refusing a token exchange that is not the one grant this server implements");
        return refuse(http::unsupported_request());
    }

    let credential = field("subject_token").unwrap_or_default().trim();
    if credential.is_empty() {
        return unauthorized(auth::Rejection::missing_credential(
            "refusing a token exchange carrying no pairing code",
        ));
    }

    // Read before the code is spent, so a client that asked for a scope this
    // server does not know does not also burn its one attempt.
    let requested = match field("scope") {
        Some(scope) => match pairing::parse_scopes(scope) {
            Some(scopes) => Some(scopes),
            None => {
                eprintln!("laplus: refusing a token exchange asking for an unreadable scope list");
                return refuse(http::invalid_scope());
            }
        },
        None => None,
    };

    let database = state.services.shell.database();
    let grant = match database.consume_pairing_link(credential) {
        Ok(Ok(grant)) => grant,
        Ok(Err(refusal)) => {
            // The three cases are told apart here and nowhere else. The
            // contract has one 401 for all of them, so this line is the only
            // place a user reporting "it says the code is wrong" can be
            // answered with which of the three it actually was.
            return unauthorized(auth::Rejection::invalid_credential(format!(
                "refusing a token exchange: {}",
                refusal.detail()
            )));
        }
        Err(error) => {
            eprintln!("laplus: cannot spend a pairing code: {error}");
            return refuse(http::access_token_unavailable());
        }
    };

    // Asking for nothing asks for everything the code granted, which is the
    // reference server's `requestedScopes ?? grant.scopes`.
    let scopes = requested.unwrap_or_else(|| grant.scopes.clone());
    if !pairing::covers(&grant.scopes, &scopes) {
        eprintln!(
            "laplus: refusing a token exchange asking for scopes the pairing code did not grant"
        );
        return refuse(http::scope_not_granted());
    }

    let (session_id, token) = match (pairing::record_id(), pairing::opaque_token()) {
        (Ok(session_id), Ok(token)) => (session_id, token),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("laplus: cannot mint a session token: {error}");
            return refuse(http::access_token_unavailable());
        }
    };

    match state.services.shell.database().issue_session(NewSession {
        session_id: &session_id,
        token: &token,
        subject: &grant.subject,
        scopes: &scopes,
        method: pairing::BEARER_SESSION_METHOD,
        label: grant.label.as_deref(),
    }) {
        Ok(session) => Json(http::access_token(&session)).into_response(),
        Err(error) => {
            eprintln!("laplus: cannot open a session: {error}");
            refuse(http::access_token_unavailable())
        }
    }
}

/// `POST /api/auth/websocket-ticket` — trade a bearer for five minutes and one
/// upgrade.
///
/// **`POST`, not `GET`.** Ticket 73's table says `GET`; the contract and
/// `packages/client-runtime/src/authorization/remote.ts:81-98` both say `POST`.
///
/// This step exists because the browser's WebSocket API cannot set a request
/// header, so the credential has to travel in the query string — and a
/// thirty-day bearer in a query string ends up in a proxy log. A five-minute
/// single-use ticket in the same log is worthless by the time anyone reads it.
async fn websocket_ticket(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    // Minted against whatever session authorized the request — a bearer or a
    // cookie — matching upstream's `issueWebSocketTicket(session)`. Not only a
    // bearer: a browser holding a cookie has no reason to ask for one, because
    // the browser attaches the cookie to the upgrade by itself, but a route
    // that refused it would be refusing a client that had already proved
    // exactly as much.
    //
    // A request that authorized with a `wsTicket` names no session, so
    // `issue_websocket_ticket` finds nothing and this answers 401. Tickets do
    // not beget tickets.
    let token = match authorized(&state, query.as_deref(), &headers) {
        Ok((presented, _grant)) => presented.token,
        Err(refused) => return refused,
    };

    let ticket = match pairing::opaque_token() {
        Ok(ticket) => ticket,
        Err(error) => {
            eprintln!("laplus: cannot mint a socket ticket: {error}");
            return refuse(http::websocket_ticket_unavailable());
        }
    };

    match state
        .services
        .shell
        .database()
        .issue_websocket_ticket(token, &ticket)
    {
        Ok(Some(issued)) => Json(http::websocket_ticket(&issued)).into_response(),
        // The bearer did not name a live session. `issue_websocket_ticket`
        // verifies and inserts under one lock precisely so this cannot be a
        // caller that checked and then minted.
        Ok(None) => unauthorized(auth::Rejection::invalid_credential(
            "refusing to mint a socket ticket for a bearer that names no live session",
        )),
        Err(error) => {
            eprintln!("laplus: cannot mint a socket ticket: {error}");
            refuse(http::websocket_ticket_unavailable())
        }
    }
}

/// `POST /api/auth/pairing-token` — mint a code for the user to carry to their
/// phone.
///
/// **`/pairing-token`, not the ticket's `/pairing-credential`.** The contract
/// again.
async fn pairing_credential(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    let request = match http::read_pairing_credential_request(&body) {
        Ok(request) => request,
        Err(http::PayloadProblem::InvalidScope) => {
            eprintln!("laplus: refusing to mint a pairing code for an unreadable scope list");
            return refuse(http::invalid_scope());
        }
        Err(http::PayloadProblem::Malformed) => {
            eprintln!("laplus: refusing to mint a pairing code for a body that is not the payload");
            return refuse(http::unsupported_request());
        }
    };

    let (id, credential) = match (pairing::record_id(), pairing::pairing_code()) {
        (Ok(id), Ok(credential)) => (id, credential),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("laplus: cannot mint a pairing code: {error}");
            return refuse(http::pairing_credential_unavailable());
        }
    };

    match state
        .services
        .shell
        .database()
        .issue_pairing_link(NewPairingLink {
            id: &id,
            credential: &credential,
            method: pairing::ONE_TIME_TOKEN_METHOD,
            scopes: &request.scopes,
            subject: pairing::PAIRING_SUBJECT,
            label: request.label.as_deref(),
            ttl: pairing::PAIRING_CODE_TTL,
            // A code minted here is read off a screen and typed into a phone.
            // The second use of one is somebody who should not have it.
            reusable: false,
        }) {
        Ok(link) => {
            // Settings reads its list from `subscribeAuthAccess`, not from the
            // response to this call, so a code that is minted and not announced
            // is a code the user cannot see well enough to carry anywhere.
            state.services.shell.auth_access_changed();
            Json(http::pairing_credential(&link)).into_response()
        }
        Err(error) => {
            eprintln!("laplus: cannot mint a pairing code: {error}");
            refuse(http::pairing_credential_unavailable())
        }
    }
}

/// `GET /api/auth/pairing-links` — the codes Settings can still show.
///
/// Live ones only: a code that was spent, revoked or has expired is not
/// something the user can hand to a phone, and the panel's whole purpose is to
/// let them re-read one they minted a minute ago.
async fn pairing_links(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    match state.services.shell.database().active_pairing_links() {
        Ok(links) => Json(http::pairing_links(&links)).into_response(),
        Err(error) => {
            eprintln!("laplus: cannot list the pairing codes: {error}");
            refuse(http::pairing_links_unavailable())
        }
    }
}

/// `POST /api/auth/pairing-links/revoke` — take a code back.
///
/// **`/pairing-links/revoke`, not the ticket's `/revoke-pairing-link`.**
///
/// A body that will not parse is answered with the 500 rather than a 400,
/// which is the one place these five routes cannot say what they mean:
/// `EnvironmentScopedOperationErrors` gives this endpoint a 403 and a 500 and
/// no `EnvironmentRequestInvalidError`, so a 400 here is a status the client
/// cannot decode and would report as a generic failure. The real cause goes to
/// the log, and `revoked: false` was the alternative — rejected because
/// "nothing was revoked" is a true sentence about a request that never named
/// anything to revoke, and it would hide the client bug completely.
async fn revoke_pairing_link(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: String,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    let id = match http::read_revoke_pairing_link_request(&body) {
        Ok(id) => id,
        Err(problem) => {
            eprintln!("laplus: cannot read which pairing code to revoke ({problem:?})");
            return refuse(http::pairing_link_revoke_failed());
        }
    };

    match state.services.shell.database().revoke_pairing_link(&id) {
        // `false` for an id that names nothing, rather than a 404. The contract
        // gives this route no `EnvironmentResourceNotFoundError`, and the state
        // the caller wanted — that code cannot be spent — holds either way.
        Ok(revoked) => {
            // Only when something actually went: republishing an unchanged list
            // is work every open panel does for nothing.
            if revoked {
                state.services.shell.auth_access_changed();
            }
            Json(http::pairing_link_revoked(revoked)).into_response()
        }
        Err(error) => {
            eprintln!("laplus: cannot revoke the pairing code: {error}");
            refuse(http::pairing_link_revoke_failed())
        }
    }
}

/// Write a 401 to the log and answer with the body the client decodes.
///
/// The same shape the upgrade refuses with, and deliberately: a phone that
/// cannot mint a ticket and a phone that cannot open a socket are one failure
/// to the person holding it.
fn unauthorized(rejection: Rejection) -> Response {
    (StatusCode::UNAUTHORIZED, refused(rejection)).into_response()
}

/// May this request proceed? The 401 to return if not.
///
/// **The whole of the check, and the only place it is made.** Both halves:
/// [`auth::authorize`] settles the origin, and then the credential it reports
/// is verified against [`crate::store`]. Splitting them across two functions
/// would be splitting them across two things a future caller could forget one
/// of.
///
/// Every route that is not `/oauth/token`, `/api/auth/browser-session` or a
/// static file goes through this, and they all get the same answer — a
/// credential good enough to open the socket that was not good enough to read a
/// snapshot would simply send the client back to the socket, which is the
/// failed round trip ticket 31 exists to remove.
///
/// ## Why a ticket is spent here and a session is not
///
/// A `wsTicket` is single use by construction: it rides in a query string,
/// which is the one place in this chain a credential lands in a log, and it is
/// worth nothing five minutes later or one upgrade later. So verifying it
/// *consumes* it. A bearer or a cookie names a session that is meant to be
/// presented over and over, so verifying one only reads it.
///
/// That makes this function's effect depend on what arrived, which is worth
/// knowing before calling it twice on one request. Nothing does.
///
/// ## What it hands back
///
/// The credential that arrived **and the grant it named** — what that session
/// is entitled to, straight out of the store. Most callers want only the gate
/// and ignore the second half; `/api/auth/session` is the one that reports it,
/// and it is reported rather than re-read because the read has already
/// happened here. A second lookup would be a second answer that could differ
/// from the one this function refused or admitted on.
fn authorized<'a>(
    state: &ServerState,
    query: Option<&'a str>,
    headers: &'a HeaderMap,
) -> Result<(auth::Presented<'a>, pairing::Grant), Response> {
    let allowed = &state.config().current().remote_access;
    let presented = auth::authorize(presented(query, headers), allowed).map_err(|rejection| {
        // No `Access-Control-Allow-Origin` here, unlike the upgrade's own
        // 401. There it lets a browser read the body rather than reporting
        // a CORS error for a handshake it cannot see into; here the refused
        // request *is* the cross-origin one, and helping it read the answer
        // would be the only thing this refusal gives away.
        (StatusCode::UNAUTHORIZED, refused(rejection)).into_response()
    })?;

    let database = state.services.shell.database();
    let verified = match presented.shape {
        // The union's `missing_credential`, and the change ticket 73 is
        // fundamentally about — this used to be `Ok`. See `crate::auth`.
        auth::Credential::Absent => {
            return Err(unauthorized(auth::Rejection::missing_credential(
                "refusing a request that presented no credential",
            )))
        }
        auth::Credential::WebSocketTicket => database.consume_websocket_ticket(presented.token),
        auth::Credential::BearerToken | auth::Credential::SessionCookie => {
            database.verify_session(presented.token)
        }
        // Advertised in the descriptor's `sessionMethods` because the shape is
        // read at the upgrade, and refused here because this server implements
        // no proof-of-possession — ticket 73 puts DPoP out of scope. Taking one
        // as a bearer would be accepting a credential while ignoring the proof
        // that is the entire point of the scheme.
        auth::Credential::DpopToken => {
            return Err(unauthorized(auth::Rejection::invalid_credential(
                "refusing a DPoP credential: this server implements no proof-of-possession",
            )))
        }
    };

    match verified {
        Ok(Some(grant)) => Ok((presented, grant)),
        Ok(None) => Err(unauthorized(auth::Rejection::invalid_credential(format!(
            "refusing a {:?} that names no live session",
            presented.shape
        )))),
        Err(error) => {
            // A database that will not answer is not a credential that failed,
            // and saying 401 would send the user to re-pair over a disk error.
            eprintln!("laplus: cannot verify a credential: {error}");
            Err(refuse(http::credential_verification_failed()))
        }
    }
}

/// The origin half of [`authorized`], and none of the credential half.
///
/// **Exactly two routes may use this**, and both for the same reason: they are
/// how a client holding nothing comes to hold something. `/oauth/token` and
/// `/api/auth/browser-session` take their credential in the request *body* — a
/// pairing code — so requiring one in a header would be requiring the thing
/// they exist to issue.
///
/// That is not a hole. What they accept is a pairing code, which is a
/// credential this server minted, is single use, lives five minutes, and can be
/// revoked. The origin check still applies, so a page on an unnamed origin
/// cannot even reach them. What is skipped is only the *session* check, and a
/// caller that had a session would not be here.
fn origin_admitted(
    state: &ServerState,
    query: Option<&str>,
    headers: &HeaderMap,
) -> Result<(), Response> {
    let allowed = &state.config().current().remote_access;
    auth::authorize(presented(query, headers), allowed)
        .map(|_presented| ())
        .map_err(|rejection| (StatusCode::UNAUTHORIZED, refused(rejection)).into_response())
}

fn refuse(refusal: http::Refusal) -> Response {
    (
        StatusCode::from_u16(refusal.status).expect("the contract's statuses are valid"),
        Json(refusal.to_value()),
    )
        .into_response()
}

/// Everything the routes above did not answer: a file of the UI, the page
/// standing in for one of its own routes, or a 404.
///
/// The bytes are copied on the way out. They are `&'static` in the shell and
/// could be handed to the body without one, at the cost of putting `axum`'s
/// `Bytes` into [`crate::ui`] and so the web framework into the policy — which
/// [`crate::auth`] and [`crate::http`] are both deliberately free of. What it
/// buys is one copy of at most a few megabytes, once per window, over loopback.
async fn asset(
    State(state): State<Arc<ServerState>>,
    method: axum::http::Method,
    uri: axum::http::Uri,
) -> Response {
    // `HEAD` is the same answer without the body, which `axum` takes care of.
    if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }

    match state.ui.resolve(uri.path()) {
        Some(asset) => (
            [
                (header::CONTENT_TYPE, asset.content_type),
                (header::CACHE_CONTROL, asset.caching.header()),
            ],
            asset.bytes.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A project's own icon, for the `<img>` the sidebar draws it in.
///
/// The token is the whole of the authorization — see [`crate::assets`], which
/// is also why every failure here is the same 404. Off the async runtime
/// because it reads a row and then a file.
async fn project_asset(
    State(state): State<Arc<ServerState>>,
    Path((token, _name)): Path<(String, String)>,
) -> Response {
    // The whole state moves in rather than the database: `Database` is not
    // `Clone` — it owns the connection — and the `Arc` is what everything else
    // here shares it by.
    let served = tokio::task::spawn_blocking(move || {
        let secret = state
            .services
            .shell
            .database()
            .secret_or_create(
                crate::assets::SIGNING_SECRET_NAME,
                crate::assets::SIGNING_SECRET_BYTES,
            )
            .ok()?;
        crate::assets::serve(&token, &secret, crate::clock::now_epoch_millis() as i64)
    })
    .await;

    match served {
        Ok(Some(asset)) => (
            [
                (header::CONTENT_TYPE, asset.content_type),
                // Private because this is one developer's project and not a
                // public file, and an hour because that is the token's own life
                // — a cache entry that outlived it would answer from disk for a
                // URL the server has stopped honouring.
                (header::CACHE_CONTROL, "private, max-age=3600"),
                // The content type is inferred from an extension on a file
                // inside somebody's project, so it is a guess about a file the
                // server did not write. Sniffing on top of a wrong guess is how
                // an "icon" becomes a script.
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            asset.bytes,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn upgrade(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // The upgrade is where a `wsTicket` is spent, so this is the call that
    // consumes it — see [`authorized`]. A refused upgrade has therefore not
    // spent anything, which is what lets a client retry with a fresh ticket
    // rather than having to re-pair.
    match authorized(&state, query.as_deref(), &headers) {
        Ok((presented, _grant)) => {
            let credential = presented.shape;
            ws.on_upgrade(move |socket| connection(socket, state, credential))
        }
        // `Access-Control-Allow-Origin` is not added here, unlike before ticket
        // 73. The reference server sets it on this refusal so that a browser
        // reads the body rather than a CORS error — but it did that when the
        // only reason to refuse was the origin. Now the ordinary refusal is a
        // credential that did not verify, and echoing `*` at every caller would
        // let any page on any origin read the 401 it provoked. The body says
        // nothing secret; the header is simply no longer buying anything the
        // refused client needs.
        Err(refusal) => refusal,
    }
}

/// What the client presented, read off an `axum` request.
///
/// Here rather than in [`crate::auth`] because this is the one place the web
/// framework meets the policy. [`UpgradeRequest`] takes strings precisely so
/// the decision can be made and tested without a `HeaderMap`, and a
/// constructor over one would undo that.
fn presented<'a>(query: Option<&'a str>, headers: &'a HeaderMap) -> UpgradeRequest<'a> {
    UpgradeRequest {
        query,
        origin: header_str(headers, header::ORIGIN),
        authorization: header_str(headers, header::AUTHORIZATION),
        cookie: header_str(headers, header::COOKIE),
    }
}

/// Write a refusal to the log and hand back the body the client decodes.
///
/// The status and any headers are the caller's, because they are the only
/// thing the two callers disagree about.
fn refused(rejection: Rejection) -> Json<AuthInvalidBody> {
    eprintln!("laplus: {} (traceId {})", rejection.detail, rejection.trace_id);
    Json(rejection.body())
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// One connection's state, minus the socket: what it is streaming, and where
/// its frames go.
///
/// Separating it from the socket is what lets the frame vocabulary be driven
/// without one — see this module's tests — and it keeps the three things that
/// every frame needs from travelling as three arguments.
struct Connection {
    state: Arc<ServerState>,
    subscriptions: Subscriptions,
    frames: mpsc::Sender<String>,
}

impl Connection {
    fn new(state: Arc<ServerState>, frames: mpsc::Sender<String>) -> Connection {
        let subscriptions = Subscriptions::new(state.subscription_gauge(), frames.clone());
        Connection {
            state,
            subscriptions,
            frames,
        }
    }

    /// Handle one text frame, writing whatever it owes the client into the
    /// frame queue. Breaks only when the connection is gone.
    async fn handle(&mut self, text: &str) -> ControlFlow<()> {
        let message = match ClientMessage::parse(text) {
            Ok(message) => message,
            Err(error) => {
                self.state.unparseable_frames.fetch_add(1, Ordering::Relaxed);
                eprintln!("laplus: unparseable socket frame: {error}");
                return ControlFlow::Continue(());
            }
        };

        match message {
            ClientMessage::Request { id, tag, payload } => {
                match rpc::dispatch(&self.state.services, &tag, &payload) {
                    Ok(Answer::Value(value)) => self.send(ServerMessage::success(id, value)).await,
                    // A subscription's first chunk comes from its own task, so
                    // there is nothing to write here. The `Request` is answered
                    // eventually — by the terminal `Exit` when it ends.
                    Ok(Answer::Stream(source)) => {
                        self.subscriptions.start(id, source).await;
                        ControlFlow::Continue(())
                    }
                    // Off the read loop, on a thread that may block. Nothing is
                    // written here either — the task answers under the same
                    // `requestId` whenever it is done, which is what lets the
                    // client keep talking meanwhile.
                    Ok(Answer::Deferred(work)) => {
                        self.defer(id, work);
                        ControlFlow::Continue(())
                    }
                    // An `Exit`/`Failure` under the caller's own `requestId`,
                    // *not* the bare `Defect` the reference server sends. A
                    // `Defect` carries no id and the client fails every pending
                    // request and open subscription on the socket when it sees
                    // one — see `DispatchError::to_error` for the evidence and
                    // the reasoning.
                    Err(error) => {
                        self.send(ServerMessage::failure(id, error.to_error()))
                            .await
                    }
                }
            }
            ClientMessage::Ping => self.send(ServerMessage::Pong).await,
            // Back-pressure: release the pump to send its next chunk. Silent,
            // and silent for an id nothing is streaming.
            ClientMessage::Ack { request_id } => {
                self.subscriptions.acknowledge(&request_id);
                ControlFlow::Continue(())
            }
            ClientMessage::Interrupt { request_id } => {
                if self.subscriptions.interrupt(&request_id).await {
                    // A client-initiated unsubscribe ends as a *failure* with
                    // an interrupt cause rather than a success — the captured
                    // behaviour, which a client reads as a normal end.
                    let exit = Exit::Failure {
                        cause: vec![Cause::Interrupt {
                            fiber_id: self.state.next_fiber_id(),
                        }],
                    };
                    self.send(ServerMessage::Exit { request_id, exit }).await
                } else {
                    // Cancelling something that is not streaming: a unary call
                    // that has already been answered, or a cancellation that
                    // lost a race with the stream's own end. Ordinary traffic,
                    // and answering it would put a second `Exit` on an id that
                    // already has one.
                    ControlFlow::Continue(())
                }
            }
            ClientMessage::Unrecognized => {
                self.state
                    .unrecognized_messages
                    .fetch_add(1, Ordering::Relaxed);
                ControlFlow::Continue(())
            }
        }
    }

    /// Run one deferred call on a blocking thread and let it answer itself.
    ///
    /// Nothing here tracks the task. It holds a clone of the frame queue and
    /// nothing else, so when the connection goes the queue closes and the send
    /// fails; the work then finishes into nowhere and the thread is returned.
    /// A cancellation is the same story from the other side — an `Interrupt`
    /// for a unary call is already ignored (see [`Connection::handle`]), and
    /// the client has dropped the entry, so the late `Exit` lands on an id it
    /// no longer knows.
    ///
    /// The bound on all of that is [`crate::filesystem::MAX_ENTRIES`]: the work
    /// is finite whether or not anyone is still listening.
    fn defer(&self, request_id: RequestId, work: Deferred) {
        let frames = self.frames.clone();
        tokio::task::spawn_blocking(move || {
            let message = match work.run() {
                Ok(value) => ServerMessage::success(request_id, value),
                Err(error) => ServerMessage::failure(request_id, error),
            };
            // `blocking_send` and not `try_send`: a client that is behind
            // should make this thread wait, not lose its answer. It is a
            // blocking thread, which is the one place waiting is free.
            let _ = frames.blocking_send(message.to_frame());
        });
    }

    /// Queue one frame. The send fails only once the writer has gone, which
    /// means the socket has.
    async fn send(&self, message: ServerMessage) -> ControlFlow<()> {
        match self.frames.send(message.to_frame()).await {
            Ok(()) => ControlFlow::Continue(()),
            Err(_) => ControlFlow::Break(()),
        }
    }

    /// Release everything this connection holds, in the order that matters.
    ///
    /// Stopping the pumps first means no chunk can be queued behind the close
    /// frame; dropping the last sender afterwards is what tells the writer
    /// there is nothing more coming, and a pump still holding a clone would
    /// leave it waiting forever.
    async fn close(self) {
        self.subscriptions.shutdown().await;
        drop(self.frames);
    }
}

async fn connection(socket: WebSocket, state: Arc<ServerState>, credential: Credential) {
    let _live = LiveConnection::open(&state);
    let _ = credential; // recorded at the upgrade; nothing verifies it in v1.

    let mut shutdown = state.shutdown.clone();
    let (mut outgoing, mut incoming) = socket.split();
    let (frames, mut queued) = mpsc::channel::<String>(FRAME_QUEUE);

    // One owner for the sink, because the read loop and every subscription
    // pump produce frames for it.
    let writer = tokio::spawn(async move {
        while let Some(frame) = queued.recv().await {
            if outgoing.send(Message::Text(frame.into())).await.is_err() {
                return;
            }
        }
        // The queue closes when the read loop has finished and released every
        // pump. A close frame here is redundant if the client left first and
        // fails harmlessly if it did; it is the courtesy that matters when the
        // *server* is the one stopping.
        let _ = outgoing.send(Message::Close(None)).await;
    });

    let mut connection = Connection::new(Arc::clone(&state), frames);

    loop {
        let frame = tokio::select! {
            _ = shutdown.changed() => break,
            frame = incoming.next() => frame,
        };

        let frame = match frame {
            Some(Ok(frame)) => frame,
            // A reset connection is how a closing browser tab often looks.
            // Nothing to report, and the cleanup below is the same either way.
            Some(Err(_)) | None => break,
        };

        match frame {
            Message::Text(text) => {
                if connection.handle(text.as_str()).await.is_break() {
                    break;
                }
            }
            Message::Close(_) => break,
            // Every captured frame was text; the vocabulary has no binary
            // member. Ignore rather than close, so a stray frame is not fatal.
            Message::Binary(_) => {}
            // WebSocket-level control frames. Distinct from the JSON `Ping`
            // the UI sends every ~5 s, and answered by the library.
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    connection.close().await;
    let _ = writer.await;
}

/// Keeps the live-connection gauge honest whichever way the loop exits —
/// clean close, reset, or panic.
struct LiveConnection<'a> {
    state: &'a ServerState,
}

impl<'a> LiveConnection<'a> {
    fn open(state: &'a ServerState) -> Self {
        state.live_connections.fetch_add(1, Ordering::Relaxed);
        LiveConnection { state }
    }
}

impl Drop for LiveConnection<'_> {
    fn drop(&mut self) {
        self.state.live_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `0.0.0.0` is an address to bind and not an address to reach.
    ///
    /// The first time the network access switch was turned on, the window
    /// opened at `http://0.0.0.0:4773/#token=…` and the webview answered
    /// `ERR_ADDRESS_INVALID` — a wildcard means "every interface" to a listener
    /// and nothing at all to a client. So the switch made the application
    /// unopenable, on the machine that turned it on, with the credential it
    /// needed sitting in a URL that could not be fetched.
    ///
    /// Only the address is replaced. The port is the listener's, because
    /// `--port` and `LAPLUS_PORT` both move it and a loopback URL naming the
    /// wrong one is the same failure wearing a different error.
    #[test]
    fn a_wildcard_bind_is_reported_to_this_machine_as_loopback() {
        let wildcard = SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 4773));
        assert_eq!(
            Server::reachable_from(wildcard),
            SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 4773))
        );

        let overridden = SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 4774));
        assert_eq!(Server::reachable_from(overridden).port(), 4774);
    }

    /// And a real address is left alone, or a server told to bind one interface
    /// would advertise a different one.
    #[test]
    fn an_address_that_is_not_a_wildcard_is_reported_as_it_is() {
        for bound in [
            SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 4773)),
            SocketAddr::from((std::net::Ipv4Addr::new(192, 168, 10, 45), 4773)),
        ] {
            assert_eq!(Server::reachable_from(bound), bound);
        }
    }

    /// The real [`Connection`] with the socket taken off the end: frames go in
    /// as text and come back out of the queue.
    ///
    /// The streaming behaviour is not tested here — a pump writes from its own
    /// task, so reading the queue immediately afterwards would be a race, and
    /// `tests/socket_streaming.rs` drives it through a real socket where the
    /// waiting is honest. What is left here is the frame-by-frame vocabulary,
    /// which is worth having fast and precise.
    struct Loopback {
        connection: Connection,
        queued: mpsc::Receiver<String>,
    }

    impl Loopback {
        fn new() -> Loopback {
            let index = Index::new();
            let state = Arc::new(ServerState::new(
                Services {
                    config: ConfigStore::new(ServerConfig::detect()),
                    shell: Shell::new(
                        Database::in_memory().expect("an in-memory database"),
                    ),
                    repositories: Repositories::new(&index),
                    index,
                    terminals: Terminals::new(),
                },
                Assets::none(),
                watch::channel(false).1,
            ));
            let (frames, queued) = mpsc::channel(FRAME_QUEUE);
            Loopback {
                connection: Connection::new(state, frames),
                queued,
            }
        }

        fn state(&self) -> &ServerState {
            &self.connection.state
        }

        /// Feed one frame in and take everything it wrote back.
        async fn feed(&mut self, text: &str) -> Vec<serde_json::Value> {
            let flow = self.connection.handle(text).await;
            assert!(flow.is_continue(), "the connection was dropped on {text}");

            let mut written = Vec::new();
            while let Ok(frame) = self.queued.try_recv() {
                written.push(serde_json::from_str(&frame).expect("valid json"));
            }
            written
        }

        /// Wait for the next frame from the queue, whoever wrote it.
        ///
        /// [`Loopback::feed`] drains what is already there, which is right for
        /// a frame the read loop wrote before returning. A deferred call's
        /// answer comes from a thread, so the only honest way to read it is to
        /// wait — with a bound, because "never arrives" is the failure this is
        /// most likely to catch.
        async fn next_queued(&mut self) -> serde_json::Value {
            let frame = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.queued.recv(),
            )
            .await
            .expect("a frame arrives within the timeout")
            .expect("the queue is still open");
            serde_json::from_str(&frame).expect("valid json")
        }

        /// Feed one frame in and take the single reply it owed.
        async fn reply_to(&mut self, text: &str) -> serde_json::Value {
            let written = self.feed(text).await;
            assert_eq!(written.len(), 1, "expected one reply, got {written:#?}");
            written.into_iter().next().expect("a reply")
        }

        /// Feed one frame in and take the single `Exit` it produced.
        ///
        /// Unlike [`Loopback::reply_to`] this tolerates a chunk arriving
        /// alongside: a pump writes when it is scheduled, so which of two
        /// frames a snapshot lands behind is genuinely not determined. That it
        /// is exactly one `Exit` is.
        async fn exit_from(&mut self, text: &str) -> serde_json::Value {
            let written = self.feed(text).await;
            let exits: Vec<serde_json::Value> = written
                .iter()
                .filter(|frame| frame["_tag"] == "Exit")
                .cloned()
                .collect();
            assert_eq!(exits.len(), 1, "expected one exit, got {written:#?}");
            exits.into_iter().next().expect("an exit")
        }
    }

    #[tokio::test]
    async fn a_request_for_an_implemented_method_is_answered_with_a_success_exit() {
        let mut loopback = Loopback::new();
        let reply = loopback
            .reply_to(
                r#"{"_tag":"Request","id":"7","tag":"server.getConfig","payload":{},"headers":[]}"#,
            )
            .await;

        assert_eq!(reply["_tag"], "Exit");
        assert_eq!(reply["requestId"], "7");
        assert_eq!(reply["exit"]["_tag"], "Success");
        assert_eq!(
            reply["exit"]["value"],
            loopback.state().config().current().to_value()
        );
    }

    #[tokio::test]
    async fn ping_is_answered_with_pong() {
        let mut loopback = Loopback::new();
        assert_eq!(
            loopback.reply_to(r#"{"_tag":"Ping"}"#).await,
            json!({"_tag": "Pong"})
        );
    }

    /// Neither is an error, and neither owes the client a frame when there is
    /// no stream behind the id.
    #[tokio::test]
    async fn ack_and_interrupt_for_nothing_are_accepted_silently() {
        let mut loopback = Loopback::new();
        assert!(loopback
            .feed(r#"{"_tag":"Ack","requestId":"1"}"#)
            .await
            .is_empty());
        assert!(loopback
            .feed(r#"{"_tag":"Interrupt","requestId":"1"}"#)
            .await
            .is_empty());
        assert_eq!(loopback.state().unrecognized_messages(), 0);
        assert_eq!(loopback.state().unparseable_frames(), 0);
    }

    /// A subscription is answered by its own task, so the request itself is
    /// silent — and the registry, not the reply, is where it shows up.
    #[tokio::test]
    async fn a_subscription_request_is_registered_rather_than_answered() {
        let mut loopback = Loopback::new();
        let written = loopback
            .feed(
                r#"{"_tag":"Request","id":"3","tag":"subscribeServerConfig","payload":{},"headers":[]}"#,
            )
            .await;

        assert!(
            written.iter().all(|frame| frame["_tag"] == "Chunk"),
            "a subscription owes no immediate reply, only chunks: {written:#?}"
        );
        assert_eq!(loopback.connection.subscriptions.len(), 1);
        assert_eq!(loopback.state().live_subscriptions(), 1);

        // And cancelling it produces the terminal exit, once.
        let exit = loopback
            .exit_from(r#"{"_tag":"Interrupt","requestId":"3"}"#)
            .await;
        assert_eq!(exit["_tag"], "Exit");
        assert_eq!(exit["requestId"], "3");
        assert_eq!(exit["exit"]["_tag"], "Failure");
        assert_eq!(exit["exit"]["cause"][0]["_tag"], "Interrupt");
        assert!(exit["exit"]["cause"][0]["fiberId"].is_u64());
        assert_eq!(loopback.state().live_subscriptions(), 0);

        assert!(
            loopback
                .feed(r#"{"_tag":"Interrupt","requestId":"3"}"#)
                .await
                .is_empty(),
            "a second cancellation must not put a second exit on the same id"
        );
    }

    /// A deferred call owes nothing to the frame the request arrived on. It is
    /// answered later, under the same `requestId`, by whoever ran it — which is
    /// the whole mechanism, seen from the read loop's side.
    #[tokio::test]
    async fn a_deferred_request_is_answered_by_its_own_task_rather_than_inline() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let mut loopback = Loopback::new();

        let immediate = loopback
            .feed(&format!(
                r#"{{"_tag":"Request","id":"9","tag":"projects.listEntries","payload":{{"cwd":{}}},"headers":[]}}"#,
                json!(directory.path().to_string_lossy())
            ))
            .await;
        assert!(
            immediate.is_empty(),
            "the read loop answered inline and was not free to take the next frame: {immediate:#?}"
        );

        let answer = loopback.next_queued().await;
        assert_eq!(answer["_tag"], "Exit");
        assert_eq!(answer["requestId"], "9");
        assert_eq!(answer["exit"]["_tag"], "Success");
        assert_eq!(answer["exit"]["value"]["entries"], json!([]));
    }

    #[tokio::test]
    async fn an_unrecognised_frame_is_counted_rather_than_answered() {
        let mut loopback = Loopback::new();
        assert!(loopback
            .feed(r#"{"_tag":"Eof","requestId":"0"}"#)
            .await
            .is_empty());
        assert_eq!(loopback.state().unrecognized_messages(), 1);
        assert_eq!(loopback.state().unparseable_frames(), 0);
    }

    #[tokio::test]
    async fn a_malformed_frame_is_counted_rather_than_answered() {
        let mut loopback = Loopback::new();
        assert!(loopback.feed("{not json").await.is_empty());
        assert_eq!(loopback.state().unparseable_frames(), 1);
        assert_eq!(loopback.state().unrecognized_messages(), 0);
    }

    /// The failure has to arrive under the caller's `requestId`, so it fails
    /// one call rather than the whole session.
    #[tokio::test]
    async fn an_unimplemented_method_fails_only_its_own_request() {
        let mut loopback = Loopback::new();
        let reply = loopback
            .reply_to(
                r#"{"_tag":"Request","id":"1","tag":"no.such.method","payload":{},"headers":[]}"#,
            )
            .await;

        assert_eq!(
            reply,
            json!({
                "_tag": "Exit",
                "requestId": "1",
                "exit": {
                    "_tag": "Failure",
                    "cause": [{
                        "_tag": "Fail",
                        "error": {
                            "_tag": "ServerMethodNotImplementedError",
                            "method": "no.such.method",
                            "message": "Method not implemented by this server: no.such.method",
                        },
                    }],
                },
            })
        );
    }

    /// Each cancelled call gets its own id, the way a fiber id is distinct per
    /// fiber. Nothing depends on the value, but a constant would be a lie
    /// about what it names.
    #[tokio::test]
    async fn each_cancellation_names_a_distinct_fiber() {
        let mut loopback = Loopback::new();
        let mut seen = Vec::new();

        for id in ["0", "1"] {
            loopback
                .feed(&format!(
                    r#"{{"_tag":"Request","id":"{id}","tag":"subscribeServerConfig","payload":{{}},"headers":[]}}"#
                ))
                .await;
            let exit = loopback
                .exit_from(&format!(r#"{{"_tag":"Interrupt","requestId":"{id}"}}"#))
                .await;
            seen.push(exit["exit"]["cause"][0]["fiberId"].clone());
        }

        assert_ne!(seen[0], seen[1]);
    }
}
