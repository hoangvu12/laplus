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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, RawQuery, State, WebSocketUpgrade};
use axum::body::Bytes;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post, MethodRouter};
use axum::{middleware, Json, Router};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch, Notify};

use crate::auth::{self, AuthInvalidBody, Credential, Rejection, UpgradeRequest};
use crate::config::ServerConfig;
use crate::config_store::ConfigStore;
use crate::filesystem::Index;
use crate::git::Repositories;
use crate::orchestration::Shell;
use crate::pairing;
use crate::process::Search;
use crate::public_exposure;
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
    mcp: Arc<dyn crate::mcp::Platform>,
    diagnostic_challenges: Mutex<HashMap<String, DiagnosticChallenge>>,
    external_verification_running: AtomicBool,
    external_verification_finished: Notify,
    external_verification_generation: AtomicU64,
    endpoint_verifier: Arc<dyn public_exposure::EndpointVerifier>,
    cloudflare_connector: Arc<crate::cloudflare_connector::Manager>,
    cloudflare_account: Arc<crate::cloudflare_account::Account>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticChallenge {
    Http,
    WebSocket,
}

impl ServerState {
    fn new(
        services: Services,
        ui: Assets,
        shutdown: watch::Receiver<bool>,
        mcp: Arc<dyn crate::mcp::Platform>,
        endpoint_verifier: Arc<dyn public_exposure::EndpointVerifier>,
        cloudflare_connector: Arc<crate::cloudflare_connector::Manager>,
        cloudflare_account: Arc<crate::cloudflare_account::Account>,
    ) -> Self {
        ServerState {
            services,
            ui,
            shutdown,
            live_connections: AtomicUsize::new(0),
            live_subscriptions: Arc::new(AtomicUsize::new(0)),
            fiber_ids: AtomicU64::new(1),
            unrecognized_messages: AtomicUsize::new(0),
            unparseable_frames: AtomicUsize::new(0),
            mcp,
            diagnostic_challenges: Mutex::new(HashMap::new()),
            external_verification_running: AtomicBool::new(false),
            external_verification_finished: Notify::new(),
            external_verification_generation: AtomicU64::new(0),
            endpoint_verifier,
            cloudflare_connector,
            cloudflare_account,
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

    pub fn live_mcp_sessions(&self) -> usize {
        self.mcp.live_sessions()
    }

    pub fn open_mcp_session(&self, thread_id: &str) -> Result<crate::mcp::Session, crate::mcp::OpenError> {
        self.mcp.open_session(thread_id)
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
    /// `address` is what was actually asked of the operating system, wildcard
    /// and all. It used to be the port with `127.0.0.1:` written in front of it,
    /// which was true until the exposure switch existed and is now the wrong
    /// half of the sentence: "cannot listen on 127.0.0.1:4773" sends somebody
    /// who passed `--network` looking for a conflict on an address this process
    /// never asked for.
    Listen {
        address: SocketAddr,
        error: std::io::Error,
    },
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupFailure::Database(error) => write!(formatter, "{error}"),
            StartupFailure::Listen { address, error } => {
                write!(formatter, "cannot listen on {address}: {error}")
            }
        }
    }
}

impl std::error::Error for StartupFailure {}

impl Server {
    /// Open the registry, bind, and start serving. Port 0 asks the OS for a
    /// free one, which is what the tests use.
    ///
    /// Loopback unless `remote-access.json` says otherwise, and `docs/adr/0022`
    /// is why that file is allowed to say otherwise at all.
    ///
    /// `ui` is the web bundle to serve, or [`Assets::none`] for a server that
    /// only answers calls. The shell passes one; the plain binary does so only
    /// when handed `--ui`, which is what keeps `cargo run` a socket endpoint the
    /// real UI can be pointed at from a development server.
    ///
    /// `exposure` is what the command line insisted on, overriding the file for
    /// this process and not writing to it — `laplus-server --network`, and
    /// `docs/adr/0023` for why it does not persist. `None` is nothing insisted
    /// on, which is every caller but that flag: the shell passes it because the
    /// switch in Settings owns the file and takes no flag.
    pub async fn bind(
        port: u16,
        ui: Assets,
        exposure: Option<crate::remote_access::Exposure>,
    ) -> Result<Server, StartupFailure> {
        let database =
            Database::open(&crate::store::default_path()).map_err(StartupFailure::Database)?;
        let config = ServerConfig::detect();
        // Through `with_remote_access` rather than onto the field, because
        // `auth.policy` is derived from the bind address and has to move with
        // it. That method's own note is about the time it did not.
        let config = match exposure {
            None => config,
            Some(exposure) => {
                let overridden = config.remote_access.with_exposure(exposure);
                config.with_remote_access(overridden)
            }
        };
        let address = SocketAddr::from((config.remote_access.bind_address(), port));
        let server = Server::bind_with(port, config, database, ui)
            .await
            .map_err(|error| StartupFailure::Listen { address, error })?;
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
        Self::bind_with_maintenance(
            port, config, database, ui,
            crate::provider_maintenance::ProviderMaintenance::new(),
        ).await
    }

    /// Assembly seam for hosts and hermetic tests that supply command
    /// execution while keeping the socket and maintenance paths unchanged.
    pub async fn bind_with_maintenance(
        port: u16,
        config: ServerConfig,
        database: Database,
        ui: Assets,
        provider_maintenance: crate::provider_maintenance::ProviderMaintenance,
    ) -> std::io::Result<Server> {
        let host = crate::mcp::Host::new();
        Self::bind_with_platform(port, config, database, ui, provider_maintenance, Arc::new(host)).await
    }

    /// Assembly seam for tests that observe or refuse MCP session acquisition
    /// while retaining the production HTTP host and conversation lifecycle.
    pub async fn bind_with_platform(
        port: u16,
        config: ServerConfig,
        database: Database,
        ui: Assets,
        provider_maintenance: crate::provider_maintenance::ProviderMaintenance,
        mcp_platform: Arc<dyn crate::mcp::Platform>,
    ) -> std::io::Result<Server> {
        Self::bind_with_platform_and_verifier(
            port,
            config,
            database,
            ui,
            provider_maintenance,
            mcp_platform,
            Arc::new(public_exposure::NetworkEndpointVerifier::default()),
        )
        .await
    }

    pub async fn bind_with_platform_and_verifier(
        port: u16,
        config: ServerConfig,
        database: Database,
        ui: Assets,
        provider_maintenance: crate::provider_maintenance::ProviderMaintenance,
        mcp_platform: Arc<dyn crate::mcp::Platform>,
        endpoint_verifier: Arc<dyn public_exposure::EndpointVerifier>,
    ) -> std::io::Result<Server> {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        // And the name this data directory answers to, settled here for the
        // reason above and one more: this is the one place the config and the
        // database are together, and the id has to come from the database to
        // survive a restart. `ServerConfig::with_environment_id` is why it is
        // done at all, and ticket 06 of the headless-Linux effort is what
        // happened when every laplus answered `local`.
        //
        // A failure is logged and survived, as the boot grant below is: the
        // config keeps the id `detect` minted, which is legal and unique to this
        // process but does not survive a restart. A server that answers is worth
        // more than one that refuses to start over its own name, and a client
        // that has to re-pair is a better outcome than a window that will not
        // open.
        let config = match database.environment_id_or_create() {
            Ok(environment_id) => config.with_environment_id(environment_id),
            Err(error) => {
                eprintln!(
                    "laplus: cannot read this environment's durable name, using a \
                     temporary one for this run: {error}"
                );
                config
            }
        };
        // The index is built first because the working trees listen to its
        // watcher: there is one watcher in the process and both the file tree
        // and the status are kept fresh by it.
        let index = Index::new();
        let cloudflare_connector = crate::cloudflare_connector::Manager::open(&config.preferences);
        let cloudflare_account =
            crate::cloudflare_account::Account::open(&config.preferences.join("cloudflare"));
        // **Restore, not register.** The endpoint row is the record of who owns
        // this tunnel (`docs/adr/0049`), and it has just survived the restart
        // this is recovering from — so re-registering the connector's hostname
        // here would overwrite an `adopted` or `laplus-created` row with the
        // only ownership a connector file can imply. `restore` writes `external`
        // when there is nothing recorded and leaves an existing answer alone.
        if let Some(origin) = cloudflare_connector.snapshot()["httpsOrigin"].as_str() {
            if let Err(error) = database.restore_public_exposure_endpoint(origin) {
                eprintln!("laplus: cannot restore managed public endpoint: {error}");
            }
        }
        let services = Services {
            // `opening` rather than `new`: what the developer configured last
            // time is read in here, and a file that will not read is an issue
            // in the payload rather than a server that will not start.
            config: ConfigStore::opening(config),
            shell: Shell::new_with_mcp(database, Arc::clone(&mcp_platform)),
            repositories: Repositories::new(&index),
            index,
            terminals: Terminals::new(),
            provider_maintenance,
        };
        let state = Arc::new(ServerState::new(
            services,
            ui,
            shutdown.subscribe(),
            mcp_platform,
            endpoint_verifier,
            Arc::clone(&cloudflare_connector),
            cloudflare_account,
        ));

        // Public checks are intentionally much less frequent than connector
        // readiness checks. Failure doubles the bounded delay; a small process-
        // local jitter prevents several environments restarting together from
        // repeatedly probing Cloudflare in lockstep.
        let verifier_state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut delay = std::time::Duration::from_secs(30);
            let mut verifier_shutdown = verifier_state.shutdown.clone();
            loop {
                let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.subsec_millis() as u64 % 5).unwrap_or(0);
                tokio::select! {
                    _ = tokio::time::sleep(delay + std::time::Duration::from_secs(jitter)) => {},
                    changed = verifier_shutdown.changed() => {
                        if changed.is_err() || *verifier_shutdown.borrow() { break; }
                        continue;
                    }
                }
                if verifier_state.services.shell.database().public_exposure_endpoint().ok().flatten().is_none() {
                    delay = std::time::Duration::from_secs(30);
                    continue;
                }
                let succeeded = run_external_verification(&verifier_state).await;
                delay = public_exposure::next_background_delay(delay, succeeded);
            }
        });

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
            .route(
                "/.well-known/t3/environment",
                cross_origin(get(environment_descriptor)),
            )
            .route("/api/auth/session", cross_origin(get(auth_session)))
            // The two the UI asks for *instead of* the socket, and falls back
            // to the socket without. Real routes rather than fallback paths for
            // a second reason beyond answering them: `/api/orchestration/…` has
            // no extension, so the asset fallback would otherwise hand a thread
            // id to the UI's own router and answer a `fetch` with an HTML page.
            .route(
                "/api/orchestration/shell",
                cross_origin(get(shell_snapshot)),
            )
            .route(
                "/api/orchestration/threads/{threadId}",
                cross_origin(get(thread_snapshot)),
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
            //
            // Ticket 02 puts [`cross_origin`] on the first three and not on the
            // last three. The first three are how a client that holds nothing
            // comes to hold something, which is exactly what the desktop
            // application is doing when it adds a *remote* environment; the last
            // three are Settings managing the codes for the backend it is
            // already attached to, and no cross-origin caller asks for them.
            .route(
                "/api/auth/browser-session",
                cross_origin(post(browser_session)),
            )
            .route("/oauth/token", cross_origin(post(token_exchange)))
            .route(
                "/api/auth/websocket-ticket",
                cross_origin(post(websocket_ticket)),
            )
            .route("/api/auth/pairing-token", post(pairing_credential))
            .route("/api/auth/pairing-links", get(pairing_links))
            .route("/api/auth/pairing-links/revoke", post(revoke_pairing_link))
            .route("/api/access/cloudflare", get(external_tunnel_status).post(register_external_tunnel))
            .route("/api/access/cloudflare/test", post(test_external_tunnel))
            .route("/api/access/cloudflare/forget", post(forget_external_tunnel))
            .route("/api/access/cloudflare/challenge", get(diagnostic_http_challenge))
            .route("/api/access/cloudflare/challenge/ws", get(diagnostic_ws_challenge))
            .route("/api/access/cloudflare/executables", get(cloudflare_executables))
            .route("/api/access/cloudflare/install", get(cloudflare_install_state).post(install_cloudflared))
            .route("/api/access/cloudflare/account", get(cloudflare_account_state))
            .route("/api/access/cloudflare/account/login", post(begin_cloudflare_login))
            .route("/api/access/cloudflare/account/login/cancel", post(cancel_cloudflare_login))
            .route("/api/access/cloudflare/account/consent", post(consent_to_cloudflare_certificate))
            .route("/api/access/cloudflare/account/tunnels", post(list_cloudflare_tunnels))
            .route("/api/access/cloudflare/account/select", post(select_cloudflare_tunnel))
            .route("/api/access/cloudflare/account/adopt", post(adopt_cloudflare_tunnel))
            .route("/api/access/cloudflare/connector", get(cloudflare_connector_status))
            .route("/api/access/cloudflare/connector/configure", post(configure_cloudflare_connector))
            .route("/api/access/cloudflare/connector/start", post(start_cloudflare_connector))
            .route("/api/access/cloudflare/connector/stop", post(stop_cloudflare_connector))
            .route("/api/access/cloudflare/connector/retry", post(retry_cloudflare_connector))
            // A file out of a project, for an `<img>` the browser fetches
            // itself. A real route and not the fallback for the same reason as
            // the two above it: a token has no extension, so the asset fallback
            // would answer this with the UI's own `index.html`.
            //
            // Two segments rather than a wildcard, because the filename is a
            // basename and [`crate::assets`] percent-encodes the separator out
            // of it. See that module for why nothing here checks a credential.
            .route("/api/assets/{token}/{name}", get(project_asset))
            .route("/mcp/{sessionId}", get(mcp_get).post(mcp_post))
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
        state.mcp.set_origin(format!("http://127.0.0.1:{}", local_addr.port()));
        cloudflare_connector.set_loopback_origin(format!("http://127.0.0.1:{}", local_addr.port()));
        cloudflare_connector.begin();

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

        if cloudflare_connector.snapshot()["desiredState"] == "running" {
            let verification_state = Arc::clone(&state);
            tokio::spawn(async move { let _ = run_external_verification(&verification_state).await; });
        }

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

    /// [`Server::http_url`] for a client that is **not** on this machine.
    ///
    /// `host` is [`crate::endpoints::advertised_host`]'s answer — the address
    /// the routing table says other machines send to. Only the *port* comes
    /// from the listener, as everywhere else here.
    ///
    /// Ticket 03 of the headless-Linux effort is why this and its pair exist. A
    /// server with no window prints its URL for somebody to type into a phone,
    /// and [`Server::reachable_addr`] is deliberately the wrong answer to that
    /// question: it names loopback, which is right for the shell's window and
    /// useless on the device it was printed for.
    pub fn url_for(&self, host: &str) -> String {
        format!("http://{host}:{}/", self.local_addr.port())
    }

    /// [`Server::window_url`] for a client that is not on this machine: the
    /// same port, credential and fragment as the window's, against `host`.
    ///
    /// `None` for the same reason [`Server::window_url`] is — no boot grant was
    /// minted — and the caller falls back to [`Server::url_for`], which lands
    /// on the pairing screen.
    pub fn pairing_url_for(&self, host: &str) -> Option<String> {
        self.boot_credential
            .as_ref()
            .map(|credential| format!("{}#token={credential}", self.url_for(host)))
    }

    /// The boot credential itself, for a caller that has to show it rather than
    /// put it in a URL — a pairing screen takes it typed.
    ///
    /// This is the one thing in this server that is deliberately printed. See
    /// [`Server::window_url`] for why the fragment keeps it off the wire, and
    /// `docs/running-headless.md` for what an operator owes it once it is on
    /// their terminal.
    pub fn boot_credential(&self) -> Option<&str> {
        self.boot_credential.as_deref()
    }

    /// Where this server was told to listen, and whether a file said so.
    ///
    /// The configuration the listener was actually opened with, so a caller
    /// reporting the exposure cannot describe a posture the socket does not
    /// have — which is the whole reason `laplus-server` reads it back from here
    /// rather than from the arguments it parsed.
    pub fn remote_access(&self) -> crate::remote_access::RemoteAccess {
        self.state.config().current().remote_access.clone()
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
        let probes = crate::provider::reserve_probes(&config);
        // Taken before the task rather than inside it, so the blocking half holds
        // paths and not a handle to the registry they came from.
        let roots = self.state.workspace_roots();
        tokio::task::spawn_blocking(move || {
            crate::provider::refresh_reserved(
                &config,
                &Search::from_environment(),
                &roots,
                probes,
            )
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
        self.state.cloudflare_connector.shutdown().await;
        self.state.config().stop_provider_processes().await;
        let _ = self.serving.await;
        self.state.services.shell.threads().shutdown().await;
        self.state.services.terminals.shutdown().await;
        self.state.services.shell.flush().await;
    }

    /// Serve until the process is asked to stop. This is what the binary calls.
    pub async fn serve_until_interrupted(self) {
        asked_to_stop().await;
        self.shutdown().await;
    }
}

/// Resolves when the operating system asks this process to stop.
///
/// **`SIGTERM` as well as `SIGINT`, because the headless case only ever gets
/// the first one.** `docs/adr/0048` says laplus gracefully shuts down the
/// connectors it started and names systemd doing it; `systemctl stop`,
/// `docker stop` and a plain `kill` all send `SIGTERM`, and this only ever
/// waited for `ctrl_c`. So the default disposition ran — the process died
/// immediately, [`Server::shutdown`] never ran, and the connector child, which
/// [`crate::cloudflare_connector`] deliberately puts in its own process group so
/// that a terminal's `^C` cannot reach it, was left running with nothing in
/// laplus able to stop it. A public hostname outlived the server it exposed.
///
/// The agents and terminals `shutdown` reaps have the same property and the
/// same bug, which is the other half of why this is not only a Cloudflare fix.
///
/// Windows has no `SIGTERM`; `ctrl_c` there covers `CTRL_C_EVENT` and
/// `CTRL_CLOSE_EVENT`, which are the ways that platform asks.
async fn asked_to_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // A handler that will not install is reported and not fatal: a server
        // that refused to run because it could not hear one signal would be a
        // worse outcome than one that still hears the other.
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("laplus: cannot listen for termination: {error}");
                if let Err(error) = tokio::signal::ctrl_c().await {
                    eprintln!("laplus: cannot listen for interrupt: {error}");
                }
                return;
            }
        };
        tokio::select! {
            interrupt = tokio::signal::ctrl_c() => {
                if let Err(error) = interrupt {
                    eprintln!("laplus: cannot listen for interrupt: {error}");
                }
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        eprintln!("laplus: cannot listen for interrupt: {error}");
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
/// **These are enforced, on one surface.** Every `/api/access/cloudflare` route
/// checks `access:read` or `access:write` through [`require_scope`] before it
/// answers, which is what `docs/adr/0047` decided and what makes the window's
/// grant the difference between administering public exposure and merely using
/// it. Everywhere else the scopes are still only reported — see
/// [`crate::pairing`] — and the UI reads what a session reports to decide which
/// panels to offer.
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

/// A route a **remote** client's browser calls, wearing the headers that let it
/// read the answer.
///
/// Ticket 02 of the headless-Linux effort, and the whole of it. The desktop
/// window's page is served by its own server, so every call it makes to a second
/// laplus is cross-origin; without these headers the browser refuses the
/// response it already has, and the user is shown "could not reach the backend"
/// for a server that answered fine.
///
/// **A layer, on each route, rather than a line in each handler.** This is the
/// first `.layer()` in this router and it earns the exception: the seven routes
/// have some twenty return points between them — every refusal, every typed 404,
/// every `Err` out of [`authorized`] — and a *refused* cross-origin request that
/// forgot its headers is unreadable, which is the exact bug this ticket is
/// about. One place per route cannot forget. It is `axum`'s own
/// [`middleware::map_response`] and not a dependency; `tower-http` is in
/// `Cargo.lock` only through the shell's updater plugin and does not become
/// this crate's for fourteen lines of constant headers.
///
/// Applied route by route and never to the whole `Router`, because `/ws` must
/// **not** have them — see [`upgrade`] — and neither the asset fallback nor
/// `/api/assets/…` has any reason to.
fn cross_origin(routes: MethodRouter<Arc<ServerState>>) -> MethodRouter<Arc<ServerState>> {
    routes
        .options(preflight)
        .layer(middleware::map_response(allow_a_browser_to_read_it))
}

/// The `OPTIONS` answer, for the question the browser asks before the request
/// the page made.
///
/// **Nothing above it is reachable without this.** A JSON body and an
/// `Authorization` header each force a preflight, and this router registered no
/// `OPTIONS` handler anywhere — so every one of them was answered `405` by the
/// `MethodRouter`'s own default, with an `Allow: GET,HEAD`, and the real request
/// was never sent. Not by the asset fallback: `Router::fallback` is for paths
/// that match nothing, and these paths matched.
///
/// No credential is checked, and there is nothing here to check one against: a
/// preflight is the browser asking on the page's behalf, before the page's
/// request exists, and it carries neither cookie nor `Authorization`. What it
/// answers is "may that request be made", which this server's answer to is
/// unconditionally yes — see [`crate::http::browser_api_cors_headers`] for why
/// that widens nothing. The request itself still meets [`authorized`].
///
/// 204 rather than 200 because there is no body to describe. The headers are
/// [`cross_origin`]'s layer, the same ones the real answer carries.
async fn preflight() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// [`crate::http::browser_api_cors_headers`], on the way out.
///
/// `insert` rather than `append`: nothing here sets these, and a duplicate
/// `Access-Control-Allow-Origin` is a browser error rather than two chances at
/// being read.
async fn allow_a_browser_to_read_it(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in http::browser_api_cors_headers() {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
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
///
/// **A conversation the developer deleted is one of those**, and it is refused
/// here as well as on the socket because of how the two are used *together*: the
/// client fetches this snapshot, folds it, and then subscribes with a cursor
/// because it now holds the conversation (`client-runtime/src/state/threads.ts`).
/// So a route that answered for a deleted thread would seed a pane with a
/// conversation the developer removed and then resume past the `thread.deleted`
/// that would have told it — a stale window with no way left to learn. A client
/// that held the conversation *before* it asked still resumes into a snapshot
/// stamped `deletedAt`, which is the resume rule this ticket left alone.
///
/// **It is the same refusal as a thread that never existed**, where the socket
/// gives the two different sentences. Not an oversight: this route's refusal is a
/// [`crate::http::Refusal`], which carries no message at all — a tag, a code, and
/// a `reason` whose type is `Schema.Literals(["thread_not_found"])`, one member
/// wide. There is nowhere for a second sentence to go, and inventing a reason
/// would fail the typed decode this route exists to keep clean.
async fn thread_snapshot(
    State(state): State<Arc<ServerState>>,
    Path(thread_id): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(refused) = authorized(&state, query.as_deref(), &headers) {
        return refused;
    }

    let threads = state.services.shell.threads();
    // Asked before the conversation is built rather than filtered afterwards:
    // a detail snapshot is a copy of the whole transcript, and there is no
    // reason to make one to throw it away.
    if threads.deleted(&thread_id) {
        return refuse(http::thread_not_found());
    }

    match threads.detail_snapshot(&thread_id) {
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
    body: String,
) -> Response {
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
    body: String,
) -> Response {
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

fn require_scope(grant: &pairing::Grant, scope: &str) -> Result<(), Response> {
    if grant.scopes.iter().any(|granted| granted == scope) { return Ok(()); }
    Err((StatusCode::FORBIDDEN, Json(serde_json::json!({
        "_tag": "EnvironmentScopeRequiredError",
        "code": "insufficient_scope",
        "requiredScope": scope,
        "traceId": auth::trace_id(),
    }))).into_response())
}

/// The status and tag a refusal is answered with.
///
/// **The shape changed; the codes did not.** Every Cloudflare route already
/// refused a precondition with `409` and a rejection with `400` — what was
/// missing was a body a client could decode, which is Gap 4 in
/// `.scratch/contract-parity/ledger.md`. Changing a status here as well would
/// have been a second, unasked-for change hidden inside the first.
///
/// **This is never the answer to a missing scope.** ADR-0047 requires a refused
/// client to learn only which scope it needs, and every
/// [`public_exposure::RefusalReason`] would disclose Cloudflare state — whether
/// a tunnel exists, whether laplus created it, how far setup got.
/// [`require_scope`] answers first and returns `EnvironmentScopeRequiredError`
/// on its own.
impl IntoResponse for public_exposure::Refusal {
    fn into_response(self) -> Response {
        let (status, tag) = match self.kind {
            public_exposure::RefusalKind::Precondition => {
                (StatusCode::CONFLICT, "EnvironmentPublicExposurePreconditionError")
            }
            public_exposure::RefusalKind::Rejected => {
                (StatusCode::BAD_REQUEST, "EnvironmentPublicExposureRejectedError")
            }
        };
        (status, Json(serde_json::json!({
            "_tag": tag,
            "code": "public_exposure_refused",
            "reason": self.reason,
            // Kept beside the reason rather than replaced by it: the reason is
            // what the UI branches on, and the message is what cloudflared or
            // this server actually said, which is the half a developer debugs
            // with. Secrets are removed by `Refusal::redacting` before here.
            "message": self.message,
            "completed": self.completed,
            "remaining": self.remaining,
            "traceId": auth::trace_id(),
        }))).into_response()
    }
}

fn external_tunnel_snapshot(state: &ServerState) -> Result<public_exposure::Snapshot, Response> {
    let endpoint = state.services.shell.database().public_exposure_endpoint().map_err(|error| {
        eprintln!("laplus: cannot read public endpoint state: {error}");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    })?;
    Ok(match endpoint {
        None => public_exposure::Snapshot { configured: false, https_origin: None, wss_origin: None,
            ownership: public_exposure::TunnelOwnership::External, deletable_at_cloudflare: false, health: serde_json::json!({"connector":"external","https":"unknown","webSocket":"unknown"}), verification_state: "unconfigured".into(), failure_kind: None,
            failure_message: None, last_attempt_at: None, last_verified_at: None, advertised_endpoint: None },
        Some(endpoint) => {
            let wss = endpoint.https_origin.replacen("https://", "wss://", 1);
            let connector = state.cloudflare_connector.snapshot();
            let managed = connector["configured"] == true
                && connector["httpsOrigin"].as_str() == Some(endpoint.https_origin.as_str());
            let advertised_endpoint = (endpoint.verification_state == "verified").then(|| serde_json::json!({
                "id": format!("cloudflare-{}:{}", if managed { "managed" } else { "external" }, endpoint.https_origin),
                "label": "Cloudflare Tunnel", "provider": { "id": "cloudflare", "label": "Cloudflare Tunnel", "kind": "tunnel", "isAddon": true },
                "httpBaseUrl": endpoint.https_origin, "wsBaseUrl": wss, "reachability": "public",
                "compatibility": { "hostedHttpsApp": "compatible", "desktopApp": "compatible" },
                "source": if managed { "server" } else { "user" }, "status": "available",
                "description": if managed { "Connector supervised by laplus" } else { "Externally managed by your operator" }
            }));
            public_exposure::Snapshot { configured: true, https_origin: Some(endpoint.https_origin),
                wss_origin: Some(wss), ownership: endpoint.ownership,
                deletable_at_cloudflare: endpoint.ownership.deletable_at_cloudflare(),
                health: serde_json::json!({
                    // Who runs the connector, which the row now knows rather
                    // than assumes: this said `external` even while laplus was
                    // supervising the connector behind the hostname.
                    "connector": if managed { "laplus" } else { "external" },
                    "https": if endpoint.verification_state == "verified" || matches!(endpoint.failure_kind.as_deref(), Some("websocket" | "cloudflare-access-websocket")) { "healthy" } else if endpoint.verification_state == "failed" { "failed" } else { "unknown" },
                    "webSocket": if endpoint.verification_state == "verified" { "healthy" } else if matches!(endpoint.failure_kind.as_deref(), Some("websocket" | "cloudflare-access-websocket")) { "failed" } else { "unknown" }
                }), verification_state: endpoint.verification_state,
                failure_kind: endpoint.failure_kind, failure_message: endpoint.failure_message,
                last_attempt_at: endpoint.last_attempt_at, last_verified_at: endpoint.last_verified_at,
                advertised_endpoint }
        }
    })
}

async fn external_tunnel_status(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    let (_, grant) = match authorized(&state, query.as_deref(), &headers) { Ok(value) => value, Err(response) => return response };
    if let Err(response) = require_scope(&grant, "access:read") { return response; }
    match external_tunnel_snapshot(&state) { Ok(snapshot) => Json(snapshot).into_response(), Err(response) => response }
}

/// Refuse to claim as *external* an exposure laplus is already running.
///
/// **ADR-0045 gives every lifecycle action one owner**, and an external tunnel
/// endpoint is a promise that laplus will not start, stop, reconfigure or delete
/// the connector behind it. Registering one while laplus supervises a connector
/// of its own would leave this server both operating and disclaiming the same
/// exposure — and the record it would overwrite is the one the connector
/// restores itself from at boot.
///
/// The connector's own registration writes to the database directly and never
/// passes through here, and so does adoption's: a tunnel is chosen *before* its
/// connector is configured, so this refuses nothing tickets 05 and 06 need.
fn managed_connector_already_owns_exposure(state: &ServerState) -> Option<Response> {
    if state.cloudflare_connector.snapshot()["configured"] != true {
        return None;
    }
    Some(public_exposure::Refusal::precondition(
        public_exposure::RefusalReason::OwnershipConflict,
        "laplus already runs a connector for this environment. Stop and forget it before \
         registering a hostname somebody else operates.",
    ).into_response())
}

/// Refuse to re-describe a tunnel laplus owns as somebody else's.
///
/// **Ownership is not a field a client may set.** The guard above catches the
/// case where a connector is *configured*, and that is not the same question:
/// an adopted tunnel whose connector is stopped, or a laplus-created one whose
/// connector failed to restore, would leave a persisted `adopted` or
/// `laplus-created` row that a plain hostname registration would quietly rewrite
/// to `external` — and with it the record of the tunnel and DNS resources laplus
/// made and is the only owner of.
///
/// Ticket 07 requires that adopted and external tunnels never reach a deletion
/// command "including through repeated, stale, or forged client requests"; this
/// is the same rule pointed the other way, because a laundered ownership is how
/// a forged request would earn one. Adoption and creation write their ownership
/// through the store directly and never pass through here.
fn ownership_is_not_the_clients_to_change(state: &ServerState) -> Option<Response> {
    let recorded = state.services.shell.database().public_exposure_endpoint().ok().flatten()?;
    if recorded.ownership == public_exposure::TunnelOwnership::External {
        return None;
    }
    Some(public_exposure::Refusal::precondition(
        public_exposure::RefusalReason::OwnershipConflict,
        format!(
            "This environment already has a {} Cloudflare tunnel. Forget it before registering a \
             hostname somebody else operates.",
            recorded.ownership
        ),
    ).into_response())
}

async fn register_external_tunnel(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap, Json(body): Json<public_exposure::RegisterRequest>) -> Response {
    let (_, grant) = match authorized(&state, query.as_deref(), &headers) { Ok(value) => value, Err(response) => return response };
    if let Err(response) = require_scope(&grant, "access:write") { return response; }
    if let Some(response) = managed_connector_already_owns_exposure(&state) { return response; }
    if let Some(response) = ownership_is_not_the_clients_to_change(&state) { return response; }
    let origin = match public_exposure::normalize_hostname(&body.hostname) { Ok(origin) => origin, Err(message) => return public_exposure::Refusal::rejected(public_exposure::RefusalReason::HostnameInvalid, message).into_response() };
    // Registration through this route is always a claim about somebody else's
    // tunnel: `managed_connector_already_owns_exposure` above has just refused
    // the case where laplus runs the connector, and adoption and creation write
    // their own ownership directly rather than passing through here.
    match state.services.shell.database().register_public_exposure_endpoint(crate::store::NewPublicExposure::external(&origin)) {
        Ok(()) => Json(external_tunnel_snapshot(&state).unwrap()).into_response(),
        Err(error) => { eprintln!("laplus: cannot register public endpoint: {error}"); StatusCode::INTERNAL_SERVER_ERROR.into_response() }
    }
}

/// Forget the local record of an endpoint. Never a Cloudflare change.
///
/// **This removes the endpoint row and nothing else**, which is the whole of
/// what forget means for a hostname laplus only verifies. It is deliberately
/// still reachable for an adopted tunnel — ticket 05's acceptance keeps stop and
/// forget available — but it is not yet the forget a *supervised* connector
/// needs: ticket 07 stops the connector and removes laplus's own configuration
/// and credential first. Until then, forgetting one leaves that connector
/// running against a hostname nothing records, and the next boot restores it as
/// `external` because the connector's settings file says nothing about
/// ownership (`docs/adr/0049`). Losing a record is the safe direction —
/// `external` authorizes no deletion — but it is a record laplus needs, so
/// ticket 07 replaces this.
async fn forget_external_tunnel(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    let (_, grant) = match authorized(&state, query.as_deref(), &headers) { Ok(value) => value, Err(response) => return response };
    if let Err(response) = require_scope(&grant, "access:write") { return response; }
    match state.services.shell.database().forget_public_exposure_endpoint() { Ok(()) => Json(external_tunnel_snapshot(&state).unwrap()).into_response(), Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response() }
}

async fn test_external_tunnel(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    let (_, grant) = match authorized(&state, query.as_deref(), &headers) { Ok(value) => value, Err(response) => return response };
    if let Err(response) = require_scope(&grant, "access:write") { return response; }
    if state.services.shell.database().public_exposure_endpoint().ok().flatten().is_none() { return StatusCode::NOT_FOUND.into_response() };
    run_external_verification(&state).await;
    if state.external_verification_running.load(Ordering::Acquire) {
        return (StatusCode::GATEWAY_TIMEOUT, Json(serde_json::json!({"message": "Verification is still running."}))).into_response();
    }
    Json(external_tunnel_snapshot(&state).unwrap()).into_response()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigureCloudflareConnector {
    hostname: String,
    executable_path: std::path::PathBuf,
    connector_token: String,
}

fn managed_connector_snapshot(state: &ServerState) -> serde_json::Value {
    let mut snapshot = state.cloudflare_connector.snapshot();
    let verification = state.services.shell.database().public_exposure_endpoint().ok().flatten();
    if let Some(object) = snapshot.as_object_mut() {
        // Who owns the tunnel comes from the endpoint row and never from the
        // connector's own file — one record of one fact, `docs/adr/0049`. A
        // connector configured before its endpoint was recorded reads
        // `external`, which is the ownership that authorizes nothing.
        let ownership = verification.as_ref()
            .map(|endpoint| endpoint.ownership)
            .unwrap_or(public_exposure::TunnelOwnership::External);
        object.insert("tunnelOwnership".into(), serde_json::json!(ownership));
        // The offer and the refusal read the same answer — see `Snapshot`.
        object.insert("deletableAtCloudflare".into(),
            serde_json::json!(ownership.deletable_at_cloudflare()));
        object.insert("verificationState".into(), serde_json::json!(verification.as_ref()
            .map(|endpoint| endpoint.verification_state.as_str()).unwrap_or("unconfigured")));
        object.insert("failureKind".into(), serde_json::json!(verification.as_ref().and_then(|endpoint| endpoint.failure_kind.as_deref())));
        object.insert("publicFailureMessage".into(), serde_json::json!(verification.as_ref().and_then(|endpoint| endpoint.failure_message.as_deref())));
        object.insert("lastVerifiedAt".into(), serde_json::json!(verification.as_ref().and_then(|endpoint| endpoint.last_verified_at.as_deref())));
    }
    snapshot
}

fn connector_authorized(
    state: &ServerState,
    query: Option<&str>,
    headers: &HeaderMap,
    scope: &str,
) -> Result<(), Response> {
    let (_, grant) = authorized(state, query, headers)?;
    require_scope(&grant, scope)
}

async fn cloudflare_executables(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:read") { return response; }
    Json(state.cloudflare_connector.discover().await).into_response()
}

async fn cloudflare_install_state(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:read") { return response; }
    Json(state.cloudflare_connector.install_snapshot().await).into_response()
}

/// Approving an installation means approving one identified release, so the
/// client sends back the version and digest it was shown. A feed that has moved
/// on is a conflict the developer re-approves, never a different executable
/// installed under an approval given for another one.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovedCloudflaredRelease {
    version: String,
    checksum: String,
}

async fn install_cloudflared(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<ApprovedCloudflaredRelease>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    match state.cloudflare_connector.install(&body.version, &body.checksum).await {
        Ok(()) => Json(state.cloudflare_connector.install_snapshot().await).into_response(),
        // A feed that has moved on is a conflict the developer re-approves, so
        // it names the release rather than the executable: the approval was for
        // one artifact and that artifact is no longer what would be installed.
        Err(crate::cloudflare_install::Refusal::Conflict(message)) => {
            public_exposure::Refusal::precondition(
                public_exposure::RefusalReason::ReleaseMoved,
                message,
            )
            .into_response()
        }
        Err(crate::cloudflare_install::Refusal::Rejected(message)) => {
            public_exposure::Refusal::rejected(
                public_exposure::RefusalReason::CommandFailed,
                message,
            )
            .into_response()
        }
    }
}

/// Every account-management action names the executable it should run, because
/// which `cloudflared` this environment uses is the wizard's earlier answer and
/// not something to be re-guessed per request.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudflareAccountCommand {
    executable_path: std::path::PathBuf,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudflareCertificateConsent {
    consented: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectCloudflareTunnel {
    tunnel_id: String,
    hostname: String,
}

async fn cloudflare_account_state(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:read") { return response; }
    Json(state.cloudflare_account.snapshot()).into_response()
}

async fn begin_cloudflare_login(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<CloudflareAccountCommand>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    match state.cloudflare_account.begin_login(&body.executable_path).await {
        Ok(()) => Json(state.cloudflare_account.snapshot()).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn cancel_cloudflare_login(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    match state.cloudflare_account.cancel_login() {
        Ok(()) => Json(state.cloudflare_account.snapshot()).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn consent_to_cloudflare_certificate(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<CloudflareCertificateConsent>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    match state.cloudflare_account.consent(body.consented) {
        Ok(()) => Json(state.cloudflare_account.snapshot()).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

async fn list_cloudflare_tunnels(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<CloudflareAccountCommand>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    match state.cloudflare_account.list_tunnels(&body.executable_path).await {
        Ok(()) => Json(state.cloudflare_account.snapshot()).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// Choosing an active tunnel registers the hostname the developer supplied as
/// an external endpoint — verification and advertisement, and no lifecycle
/// ownership. Choosing an inactive one records the candidate and stops there:
/// adoption is a separate confirmation, and until it succeeds laplus manages
/// nothing.
async fn select_cloudflare_tunnel(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<SelectCloudflareTunnel>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    if let Some(response) = managed_connector_already_owns_exposure(&state) { return response; }
    if let Some(response) = ownership_is_not_the_clients_to_change(&state) { return response; }
    let selection = match state.cloudflare_account.select(&body.tunnel_id, &body.hostname) {
        Ok(selection) => selection,
        Err(refusal) => return refusal.into_response(),
    };
    if selection.classification == crate::cloudflare_account::Classification::External {
        if let Err(response) = record_external_endpoint(&state, &selection.https_origin) {
            return response;
        }
    }
    Json(state.cloudflare_account.snapshot()).into_response()
}

/// Record a hostname as somebody else's, and start proving it reaches here.
///
/// **Both places an active tunnel is discovered end the same way**: choosing one
/// from the listing, and finding that the one being dedicated has become active.
/// ADR-0045 makes them one outcome — an external tunnel endpoint, verified and
/// advertised and never operated — so they are one function rather than two
/// copies that could drift into meaning different things.
fn record_external_endpoint(state: &Arc<ServerState>, origin: &str) -> Result<(), Response> {
    state
        .services
        .shell
        .database()
        .register_public_exposure_endpoint(crate::store::NewPublicExposure::external(origin))
        .map_err(|error| {
            eprintln!("laplus: cannot register the external tunnel endpoint: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })?;
    let verifying = Arc::clone(state);
    tokio::spawn(async move { let _ = run_external_verification(&verifying).await; });
    Ok(())
}

/// The two mutations dedicating an inactive tunnel performs, in order.
///
/// Named once because three things have to agree about them: what is journaled,
/// what a refusal reports as completed, and what it reports as still
/// outstanding. Neither touches the Cloudflare *allocation* — adoption
/// retrieves a credential for a tunnel that already exists and writes laplus's
/// own configuration, which is why an adopted tunnel is never laplus's to
/// delete (ADR-0049).
const ADOPTION_STEPS: [public_exposure::MutationStep; 2] = [
    public_exposure::MutationStep::Credential,
    public_exposure::MutationStep::Configuration,
];

fn adoption_remaining(
    completed: &[public_exposure::MutationStep],
) -> Vec<public_exposure::MutationStep> {
    ADOPTION_STEPS
        .into_iter()
        .filter(|step| !completed.contains(step))
        .collect()
}

/// Dedicate an inactive existing tunnel to this environment.
///
/// **Three rules, and each of them is a separate refusal.**
///
/// *The offer is evidence about the past.* A connector can start between the
/// listing that produced the dedication screen and the button that confirms it,
/// and ADR-0045 makes an active tunnel externally managed. So activity is
/// re-read immediately before the first mutation, and a tunnel that has become
/// active falls back to an external tunnel endpoint — the hostname is still
/// verified and advertised, and laplus operates nothing.
///
/// *A repeat is a reconciliation.* An adoption already recorded returns what it
/// recorded. That is not merely an optimisation: the recheck above would find
/// laplus's *own* connector serving the tunnel and would disown a tunnel this
/// environment is correctly running.
///
/// *A partial adoption resumes.* Each mutation is journaled before it happens
/// and settled after, and a credential already on disk is the mutation having
/// already occurred — so a retry after a failure spends the account certificate
/// once, not twice, and the refusal names both what is done and what is left.
async fn adopt_cloudflare_tunnel(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<CloudflareAccountCommand>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    let Some(selection) = state.cloudflare_account.selection() else {
        return public_exposure::Refusal::precondition(
            public_exposure::RefusalReason::SelectionStale,
            "Choose a tunnel and give its hostname before dedicating one.",
        ).into_response();
    };
    if selection.classification != crate::cloudflare_account::Classification::Adoptable {
        return public_exposure::Refusal::precondition(
            public_exposure::RefusalReason::OwnershipConflict,
            "That tunnel is already serving connections, so it is externally managed. laplus can \
             verify and advertise its hostname but never operate it.",
        ).into_response();
    }
    let database = state.services.shell.database();
    let recorded = database.public_exposure_endpoint().ok().flatten();
    if let Some(endpoint) = &recorded {
        if endpoint.ownership != public_exposure::TunnelOwnership::External
            && endpoint.tunnel_id.as_deref() != Some(selection.tunnel_id.as_str())
        {
            return public_exposure::Refusal::precondition(
                public_exposure::RefusalReason::OwnershipConflict,
                format!(
                    "This environment already has a {} Cloudflare tunnel. Forget it before \
                     dedicating another.",
                    endpoint.ownership
                ),
            ).into_response();
        }
    }

    // **Is laplus already running this tunnel?** Two records answer it and
    // either is enough, because they are written at different moments: the
    // endpoint row is written last, so a confirmation interrupted between
    // configuring the connector and recording the row leaves only the second.
    // Skipping the recheck on either is what stops a resume from reading
    // laplus's *own* connections as somebody else's and disowning a tunnel this
    // environment is correctly serving. ADR-0050.
    let already_running_it = recorded.as_ref().is_some_and(|endpoint| {
        endpoint.ownership == public_exposure::TunnelOwnership::Adopted
            && endpoint.tunnel_id.as_deref() == Some(selection.tunnel_id.as_str())
    }) || state.cloudflare_connector.dedicated_tunnel_id().as_deref()
        == Some(selection.tunnel_id.as_str());

    let credential_file = state.cloudflare_connector.credential_path();
    let configuration_file = state.cloudflare_connector.configuration_path();
    // **What a previous attempt left, before anything else is decided.** Every
    // refusal below has to name the work already done, and a refusal that
    // reported none because *this* attempt had done none would be the
    // untruthful recovery state the spec forbids — the credential a failed
    // first try retrieved is still at Cloudflare's expense and still on disk.
    let mut completed: Vec<public_exposure::MutationStep> = Vec::new();
    if crate::cloudflare_account::credential_for(&credential_file, &selection.tunnel_id) {
        completed.push(public_exposure::MutationStep::Credential);
    }

    let activity = if already_running_it {
        crate::cloudflare_account::Activity::Inactive
    } else {
        match state
            .cloudflare_account
            .recheck_activity(&body.executable_path, &selection.tunnel_id)
            .await
        {
            Ok(activity) => activity,
            Err(refusal) => return refusal.into_response(),
        }
    };
    if activity == crate::cloudflare_account::Activity::Active {
        if let Err(refusal) = state.cloudflare_account.reclassify_as_external() {
            return refusal.into_response();
        }
        if let Err(response) = record_external_endpoint(&state, &selection.https_origin) {
            return response;
        }
        // This attempt mutated nothing, and the hostname is now registered as
        // somebody else's — which is a complete answer rather than a partial
        // adoption. `completed` is whatever an *earlier* attempt left behind,
        // so the sentence never claims a rollback that did not happen.
        let remaining = adoption_remaining(&completed);
        return public_exposure::Refusal::precondition(
            public_exposure::RefusalReason::TunnelBecameActive,
            "A connector started serving that tunnel, so it is externally managed. laplus \
             registered the hostname as an external tunnel endpoint instead and will verify and \
             advertise it without operating the tunnel.",
        )
        .after(&completed, &remaining)
        .into_response();
    }

    if !completed.contains(&public_exposure::MutationStep::Credential) {
        let sequence = database
            .begin_mutation_step(
                public_exposure::MutationIntent::Adopt,
                public_exposure::MutationStep::Credential,
                Some(&credential_file.to_string_lossy()),
            )
            .ok();
        let retrieved = state
            .cloudflare_account
            .retrieve_tunnel_credential(
                &body.executable_path,
                &selection.tunnel_id,
                &credential_file,
            )
            .await;
        settle_adoption_step(&state, sequence, retrieved.is_ok());
        if let Err(refusal) = retrieved {
            return refusal.after(&completed, &adoption_remaining(&completed)).into_response();
        }
        completed.push(public_exposure::MutationStep::Credential);
    }

    let sequence = database
        .begin_mutation_step(
            public_exposure::MutationIntent::Adopt,
            public_exposure::MutationStep::Configuration,
            Some(&configuration_file.to_string_lossy()),
        )
        .ok();
    let configured = state
        .cloudflare_connector
        .dedicate(
            &selection.https_origin,
            &body.executable_path,
            &selection.tunnel_id,
            &credential_file,
        )
        .await;
    settle_adoption_step(&state, sequence, configured.is_ok());
    if let Err(refusal) = configured {
        return refusal.after(&completed, &adoption_remaining(&completed)).into_response();
    }
    completed.push(public_exposure::MutationStep::Configuration);

    // `Adopted`, with no DNS record: laplus configured and runs this tunnel and
    // did not allocate it or route it, so it is the one ownership that is
    // laplus-managed locally and undeletable at Cloudflare. ADR-0049.
    if let Err(error) = database.register_public_exposure_endpoint(crate::store::NewPublicExposure {
        https_origin: &selection.https_origin,
        ownership: public_exposure::TunnelOwnership::Adopted,
        tunnel_id: Some(&selection.tunnel_id),
        dns_record: None,
        credential_path: Some(&credential_file.to_string_lossy()),
        configuration_path: Some(&configuration_file.to_string_lossy()),
    }) {
        eprintln!("laplus: cannot persist the adopted tunnel endpoint: {error}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Err(refusal) = state.cloudflare_account.confirm_adoption() {
        return refusal.into_response();
    }
    // Nothing about this adoption is outstanding, so its journal is not
    // residue a later command has to reason about.
    if let Err(error) = database.clear_mutation_journal(public_exposure::MutationIntent::Adopt) {
        eprintln!("laplus: cannot clear the adoption journal: {error}");
    }
    let verification_state = Arc::clone(&state);
    tokio::spawn(async move { let _ = run_external_verification(&verification_state).await; });
    Json(state.cloudflare_account.snapshot()).into_response()
}

/// Settle a journaled adoption step, if journaling it worked at all.
///
/// A journal that cannot be written must not stop the mutation it describes:
/// the endpoint row is the durable record, and refusing to adopt because a log
/// line would not write would trade a working tunnel for a tidier history.
fn settle_adoption_step(state: &ServerState, sequence: Option<i64>, succeeded: bool) {
    let Some(sequence) = sequence else { return };
    let state_word = if succeeded {
        public_exposure::MutationState::Completed
    } else {
        public_exposure::MutationState::Failed
    };
    if let Err(error) = state
        .services
        .shell
        .database()
        .settle_mutation_step(sequence, state_word, None)
    {
        eprintln!("laplus: cannot settle a Cloudflare mutation step: {error}");
    }
}

async fn cloudflare_connector_status(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:read") { return response; }
    Json(managed_connector_snapshot(&state)).into_response()
}

async fn configure_cloudflare_connector(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(body): Json<ConfigureCloudflareConnector>,
) -> Response {
    if let Err(response) = connector_authorized(&state, query.as_deref(), &headers, "access:write") { return response; }
    // The third laundering route, and the one adoption opened. A connector-token
    // connector is `external` by definition, so configuring one over an adopted
    // or laplus-created tunnel would rewrite the row that says laplus configured
    // it — and with it the credential and configuration paths Forget removes.
    if let Some(response) = ownership_is_not_the_clients_to_change(&state) { return response; }
    if let Err(refusal) = state.cloudflare_connector.configure(
        &body.hostname, &body.executable_path, &body.connector_token,
    ).await {
        // One redaction, at the boundary: the token reaches several of the
        // sentences below by way of cloudflared's own output, and a redaction
        // that has to be remembered at each raise site is one that will be
        // forgotten at the next.
        return refusal.redacting(&body.connector_token).into_response();
    }
    // Registered as `external` on purpose: a connector-token tunnel is
    // configured at Cloudflare and merely run here, so laplus owns the process
    // and nothing it could delete. Tickets 05 and 06 register `adopted` and
    // `laplus-created` from their own routes.
    if let Some(origin) = state.cloudflare_connector.snapshot()["httpsOrigin"].as_str() {
        if let Err(error) = state.services.shell.database().register_public_exposure_endpoint(crate::store::NewPublicExposure::external(origin)) {
            eprintln!("laplus: cannot persist managed public endpoint: {error}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    let verification_state = Arc::clone(&state);
    tokio::spawn(async move { let _ = run_external_verification(&verification_state).await; });
    Json(managed_connector_snapshot(&state)).into_response()
}

async fn mutate_cloudflare_connector(
    state: Arc<ServerState>, query: Option<&str>, headers: HeaderMap,
    running: bool, retry: bool,
) -> Response {
    if let Err(response) = connector_authorized(&state, query, &headers, "access:write") { return response; }
    match state.cloudflare_connector.set_desired(running, retry) {
        Ok(()) => {
            if running {
                let verification_state = Arc::clone(&state);
                tokio::spawn(async move { let _ = run_external_verification(&verification_state).await; });
            }
            Json(managed_connector_snapshot(&state)).into_response()
        }
        Err(refusal) => refusal.into_response(),
    }
}

async fn start_cloudflare_connector(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    mutate_cloudflare_connector(state, query.as_deref(), headers, true, false).await
}
async fn stop_cloudflare_connector(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    mutate_cloudflare_connector(state, query.as_deref(), headers, false, false).await
}
async fn retry_cloudflare_connector(State(state): State<Arc<ServerState>>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    mutate_cloudflare_connector(state, query.as_deref(), headers, true, true).await
}

async fn run_external_verification(state: &Arc<ServerState>) -> bool {
    let generation = state.external_verification_generation.load(Ordering::Acquire);
    let finished = state.external_verification_finished.notified();
    if state.external_verification_running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        if state.external_verification_generation.load(Ordering::Acquire) != generation {
            return false;
        }
        return tokio::time::timeout(std::time::Duration::from_secs(55), finished).await.is_ok();
    }
    let Some(endpoint) = state.services.shell.database().public_exposure_endpoint().ok().flatten() else {
        state.external_verification_running.store(false, Ordering::Release);
        state.external_verification_generation.fetch_add(1, Ordering::AcqRel);
        state.external_verification_finished.notify_waiters();
        return false;
    };
    let http_token = pairing::opaque_token().expect("operating system randomness");
    let ws_token = pairing::opaque_token().expect("operating system randomness");
    state.diagnostic_challenges.lock().expect("diagnostic challenges").extend([
        (http_token.clone(), DiagnosticChallenge::Http),
        (ws_token.clone(), DiagnosticChallenge::WebSocket),
    ]);
    let environment_id = state.config().current().environment.environment_id.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        state.endpoint_verifier
            .verify(&endpoint.https_origin, &environment_id, &http_token, &ws_token),
    )
    .await
    .unwrap_or_else(|_| {
        Err(public_exposure::VerificationFailure {
            kind: "http",
            message: "Public endpoint verification timed out.",
        })
    });
    state.diagnostic_challenges.lock().expect("diagnostic challenges").remove(&http_token);
    state.diagnostic_challenges.lock().expect("diagnostic challenges").remove(&ws_token);
    let succeeded = result.is_ok();
    let recorded = match result { Ok(()) => state.services.shell.database().record_public_exposure_verification(&endpoint.https_origin, true, None, None), Err(failure) => state.services.shell.database().record_public_exposure_verification(&endpoint.https_origin, false, Some(failure.kind), Some(failure.message)) };
    state.external_verification_running.store(false, Ordering::Release);
    state.external_verification_generation.fetch_add(1, Ordering::AcqRel);
    state.external_verification_finished.notify_waiters();
    succeeded && matches!(recorded, Ok(true))
}

fn diagnostic_token(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

async fn diagnostic_http_challenge(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let Some(token) = diagnostic_token(&headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    if !consume_diagnostic_challenge(&state, token, DiagnosticChallenge::Http) { return StatusCode::UNAUTHORIZED.into_response(); }
    Json(serde_json::json!({"ok": true})).into_response()
}

async fn diagnostic_ws_challenge(State(state): State<Arc<ServerState>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let Some(token) = diagnostic_token(&headers) else { return StatusCode::UNAUTHORIZED.into_response() };
    if !consume_diagnostic_challenge(&state, token, DiagnosticChallenge::WebSocket) { return StatusCode::UNAUTHORIZED.into_response(); }
    ws.on_upgrade(|mut socket| async move { let _ = socket.send(Message::Text("ok".into())).await; })
}

fn consume_diagnostic_challenge(
    state: &ServerState,
    token: &str,
    expected: DiagnosticChallenge,
) -> bool {
    let mut challenges = state.diagnostic_challenges.lock().expect("diagnostic challenges");
    if challenges.get(token) != Some(&expected) {
        return false;
    }
    challenges.remove(token);
    true
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
    let presented = auth::authorize(presented(query, headers));

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

fn refuse(refusal: http::Refusal) -> Response {
    (
        StatusCode::from_u16(refusal.status).expect("the contract's statuses are valid"),
        Json(refusal.to_value()),
    )
        .into_response()
}

async fn mcp_get() -> Response {
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

async fn mcp_post(
    State(state): State<Arc<ServerState>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()).is_some_and(|origin| {
        reqwest::Url::parse(origin).ok().and_then(|url| url.host_str().map(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host.parse::<std::net::IpAddr>().is_ok_and(|address| address.is_loopback())
        })) != Some(true)
    }) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let authorization = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).unwrap_or_default();
    if !state.mcp.authorizes(&session_id, authorization) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let message: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if message.get("method").is_none() || message.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let response = state.mcp.dispatch(&session_id, message).await;
    ([(header::CONTENT_TYPE, "application/json")], Json(response)).into_response()
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
        //
        // **Ticket 02 admits it on seven other routes and not on this one**, and
        // the difference is what the refused client can do with the answer. There
        // the request being refused is one the remote client meant to make, and
        // reading *which* refusal it was is its whole recovery. A browser cannot
        // put a header on an upgrade — which is why the ticket rides in the query
        // string — so nothing here is waiting to be told apart. See
        // [`cross_origin`].
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
            let config = ServerConfig::detect();
            let connector_preferences = tempfile::tempdir().expect("connector preferences");
            let cloudflare_connector =
                crate::cloudflare_connector::Manager::open(connector_preferences.path());
            let cloudflare_account = crate::cloudflare_account::Account::open(
                &connector_preferences.path().join("cloudflare"),
            );
            let state = Arc::new(ServerState::new(
                Services {
                    config: ConfigStore::new(config),
                    shell: Shell::new(
                        Database::in_memory().expect("an in-memory database"),
                    ),
                    repositories: Repositories::new(&index),
                    index,
                    terminals: Terminals::new(),
                    provider_maintenance: crate::provider_maintenance::ProviderMaintenance::new(),
                },
                Assets::none(),
                watch::channel(false).1,
                Arc::new(crate::mcp::Host::new()),
                Arc::new(public_exposure::NetworkEndpointVerifier::default()),
                cloudflare_connector,
                cloudflare_account,
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
