//! The socket test harness: start a server, connect a client, drive methods.
//!
//! This is the project's **primary test seam**. The spec puts the bulk of
//! testing here rather than giving each subsystem its own seam, because this
//! is the genuine contract with the UI — filesystem, projects, provider,
//! orchestration, terminal and git are all meant to be exercised through it.
//! Ticket 03 builds it for one method; every later ticket adds calls, not
//! plumbing.
//!
//! Three things it deliberately does:
//!
//! - **Speaks a different WebSocket implementation from the server.** The
//!   server is `axum`/`tungstenite`-on-the-inside; the client here is
//!   `tokio-tungstenite` driven directly. A passing test means two stacks
//!   agree on the framing.
//! - **Correlates by `requestId`, never by arrival order.** The reference
//!   server answers concurrent calls out of order and a conforming client must
//!   cope, so the harness copes — otherwise it would quietly bake in an
//!   assumption the real UI does not make.
//! - **Times every read out.** A protocol bug usually presents as "nothing
//!   arrives". Without a timeout that is a hung suite instead of a failure.

#![allow(dead_code)]

pub mod agent;
pub mod captures;
pub mod conversation;
pub mod shape;
pub mod terminal;
pub mod workspace;

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use laplus_server::config::ServerConfig;
use laplus_server::config_store::ConfigChange;
use laplus_server::config::ProviderState;
use laplus_server::process::Search;
use laplus_server::remote_access::RemoteAccess;
use laplus_server::store::Database;
use laplus_server::threads::Reconciliation;
use laplus_server::ui::Assets;
use laplus_server::Server;
use tempfile::TempDir;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

/// How long any single read may take before the test fails instead of hanging.
///
/// **A hang detector, not a performance budget.** The distinction is the whole
/// of ticket 29, and it is worth keeping straight before touching this number.
///
/// Many of these tests drive real `git` in real temporary workspaces, a dozen
/// test binaries at a time. How long that takes is a fact about the machine, not
/// about the server, so any value tight enough to work as a budget fails on a
/// busy laptop against code that is perfectly correct. Worse, it fails as
/// *`no frame within READ_TIMEOUT`* — which reads like a server that stopped
/// answering rather than like a machine that ran out of room, and sends whoever
/// meets it looking for a protocol bug. At five seconds that misdirection cost
/// three separate tickets an afternoon each.
///
/// So this is deliberately far larger than any read should ever need. The cost
/// of being too generous is paid only when something is genuinely wedged, once;
/// the cost of being too tight is paid on every busy machine, forever. If the
/// suite is uncomfortably slow, `--test-threads` is the lever — not this.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// A server bound to a free loopback port, with the sockets it hands out.
pub struct TestServer {
    server: Server,
    /// The developer's configuration, somewhere throwaway.
    ///
    /// **Every** server here gets one, and it is not optional: since ticket 22
    /// the server reads `settings.json` and `keybindings.json` at startup and
    /// writes them back when they change. A suite that let those default to the
    /// real data directory would read whatever this machine's developer had
    /// configured — so the same test would pass here and fail there — and, far
    /// worse, would *overwrite* it. Removed on drop, like the workspaces.
    ///
    /// `None` only for [`TestServer::start_configured_in`], where the *test*
    /// owns the directory because it is about to start a second server on it.
    _preferences: Option<TempDir>,
    /// A live session, paired through the real routes when this handle was
    /// built. See [`TestServer::bootstrapped`].
    session: Session,
}

/// What the harness paired itself with at startup.
///
/// Three forms of the same thing, because the three shapes a credential can
/// arrive in are themselves under test: a cookie is what a browser sends, a
/// bearer is what a client attached from elsewhere sends, and the boot code is
/// what both were minted from.
#[derive(Debug, Default, Clone)]
struct Session {
    /// `t3_session=…`, ready to be a `Cookie` header.
    cookie: String,
    bearer: String,
    boot: String,
}

/// Point the provider's Claude home at a directory that does not exist.
///
/// Without this, [`laplus_server::catalogue`] scans `~/.claude/skills` — the
/// **developer's own** — and the provider snapshot a test asserts against then
/// depends on which skills the person running the suite happens to have
/// installed. `socket_conformance.rs` is the one that would notice first, and it
/// would notice by passing on the author's machine and failing in CI, which is
/// the failure this repository has already paid for once (ticket 36).
///
/// A path under the temporary preferences rather than a blank one: an empty
/// `homePath` is what makes the server fall back to `~/.claude`, so blanking it
/// is the opposite of what is wanted. The directory is deliberately never
/// created — the scan tolerates a root that is not there, which is the ordinary
/// case for a developer with no skills.
///
/// A test that means to exercise discovery sets one before starting the server,
/// and keeps it — which is what the emptiness check below is for.
fn somewhere_that_is_not_the_developers(config: &mut ServerConfig) {
    if !config
        .settings
        .providers
        .claude_agent
        .home_path
        .trim()
        .is_empty()
    {
        return;
    }
    config.settings.providers.claude_agent.home_path = config
        .preferences
        .join("claude-home")
        .display()
        .to_string();
}

impl TestServer {
    /// A server whose registry lives only as long as it does. What every test
    /// that is not about persistence wants: nothing is shared between tests,
    /// and the developer's own project list is never touched.
    pub async fn start() -> TestServer {
        TestServer::start_on(
            None,
            Database::in_memory().expect("an in-memory database"),
            Assets::none(),
        )
        .await
    }

    /// A server that also serves a UI, as the shell's does.
    ///
    /// The bundle is the caller's — four files rather than four hundred. What
    /// is being driven is the *policy* in [`laplus_server::ui`] reaching the
    /// wire: the real assets are the shell's, and putting them here would make
    /// every test build carry 17 MB.
    pub async fn start_serving(assets: Assets) -> TestServer {
        TestServer::start_on(None, Database::in_memory().expect("an in-memory database"), assets)
            .await
    }

    /// A server started from a configuration the test built.
    ///
    /// The configuration's `preferences` is replaced with a throwaway
    /// directory whatever the caller set, for the reason [`TestServer`]'s own
    /// field gives. A test that wants to *drive* the preferences directory —
    /// one about settings surviving a restart — uses
    /// [`TestServer::start_configured_in`].
    pub async fn start_with(config: ServerConfig) -> TestServer {
        TestServer::start_on(
            Some(config),
            Database::in_memory().expect("an in-memory database"),
            Assets::none(),
        )
        .await
    }

    /// A server keeping the developer's configuration in `preferences`.
    ///
    /// Start a second one on the same directory and that is a restart, which is
    /// how "settings survive a restart" is driven without a second process —
    /// the same shape as [`TestServer::start_at`] for the registry.
    pub async fn start_configured_in(preferences: &Path) -> TestServer {
        // `detect_in` reads `remote-access.json` from the directory it is
        // given, which here is a temporary one — so this arrives loopback-only
        // already, unlike the `detect` path in `start_on`. Said rather than
        // relied upon: the two entry points must agree about this, and the one
        // that gets it for free is the one where that is easiest to miss.
        let mut config = ServerConfig::detect_in(preferences.to_path_buf())
            .with_remote_access(RemoteAccess::none());
        somewhere_that_is_not_the_developers(&mut config);
        let server =
            Server::bind_with(0, config, Database::in_memory().expect("a database"), Assets::none())
                .await
                .expect("server binds to a free loopback port");
        TestServer::bootstrapped(server, None).await
    }

    /// A server that will start `binary` when a turn is dispatched.
    ///
    /// The injection the spec asks for, and the whole of it: the agent-executable
    /// path is `settings.providers.claudeAgent.binaryPath`, a value the server
    /// already needs for real use, so pointing it at a stand-in adds no test-only
    /// seam. Everything downstream — resolution, the child, the stdio, the fold,
    /// the socket — is the production path.
    pub async fn start_with_agent(binary: &str) -> TestServer {
        let mut config = ServerConfig::detect();
        config.settings.providers.claude_agent.binary_path = binary.to_string();
        TestServer::start_with(config).await
    }

    /// A server whose registry is a file. Start a second one on the same path
    /// and that is a restart — which is how the "survives a restart" test is
    /// driven without a second process.
    pub async fn start_at(database: &Path) -> TestServer {
        TestServer::start_on(
            None,
            Database::open(database).expect("the database opens"),
            Assets::none(),
        )
        .await
    }

    /// A restart that can also take a turn: a registry on disk *and* an agent to
    /// start when one is dispatched.
    ///
    /// The two halves of "a restored conversation can be continued, not just
    /// read" — the transcript comes from the file and the continuation comes from
    /// the agent, which is handed the same stand-in the first run used so that
    /// `--resume` reaches something that can answer it.
    pub async fn start_at_with_agent(database: &Path, binary: &str) -> TestServer {
        let mut config = ServerConfig::detect();
        config.settings.providers.claude_agent.binary_path = binary.to_string();
        TestServer::start_on(
            Some(config),
            Database::open(database).expect("the database opens"),
            Assets::none(),
        )
        .await
    }

    /// The one place a server here is actually started.
    ///
    /// Whatever the caller's configuration said about where the developer's
    /// files live is **overwritten** with a throwaway directory. See
    /// [`TestServer`]'s own field: this is the seam that keeps the suite off the
    /// developer's real `settings.json`, and it is here rather than at each call
    /// site so that a test added later cannot forget it.
    ///
    /// The exposure switch is overwritten for the same reason, and it was not
    /// until ticket 05 of the headless-Linux effort — `tests/http_boot.rs` has
    /// the account of what that cost and now asserts it cannot recur.
    ///
    /// Through [`ServerConfig::with_remote_access`] rather than the field, so
    /// that `auth.policy` — which is derived from the bind address — is settled
    /// with it. Assigning the field alone left a loopback server advertising
    /// `remote-reachable`. A test server listens on loopback: it is one process
    /// talking to itself, and nothing in this suite wants a second machine to
    /// reach it.
    async fn start_on(
        config: Option<ServerConfig>,
        database: Database,
        assets: Assets,
    ) -> TestServer {
        let preferences = tempfile::tempdir().expect("a temporary directory");
        let mut config = config
            .unwrap_or_else(ServerConfig::detect)
            .with_remote_access(RemoteAccess::none());
        config.preferences = preferences.path().to_path_buf();
        somewhere_that_is_not_the_developers(&mut config);

        let server = Server::bind_with(0, config, database, assets)
            .await
            .expect("server binds to a free loopback port");
        TestServer::bootstrapped(server, Some(preferences)).await
    }

    /// Build the handle, then pair it with the server it is holding.
    ///
    /// **Since ticket 73 every request needs a credential that verifies**, so a
    /// harness that presented nothing would be a harness in which no test but
    /// the authentication ones could run. This walks the same three steps the
    /// desktop window walks at startup — take the boot code out of the URL the
    /// shell would open, trade it for a session — through the real routes,
    /// against the real store. Nothing here reaches around the policy it is
    /// setting up, which is what keeps two hundred tests from quietly proving
    /// the wrong thing.
    async fn bootstrapped(server: Server, preferences: Option<TempDir>) -> TestServer {
        let mut harness = TestServer {
            server,
            _preferences: preferences,
            session: Session::default(),
        };

        // The credential the window boots with, read off the URL the shell
        // would point a webview at. `None` only if the machine's randomness
        // failed, which would be a broken machine rather than a failing test.
        let url = harness
            .server
            .window_url()
            .expect("the server minted a boot credential");
        let boot = url
            .split_once("#token=")
            .expect("the boot url carries a credential in its fragment")
            .1
            .to_string();

        let opened = harness
            .send(
                "POST",
                "/api/auth/browser-session",
                &ClientIdentity::anonymous(),
                Some(("application/json", &json!({ "credential": boot }).to_string())),
            )
            .await;
        assert_eq!(
            opened.status, 200,
            "the harness could not open a session: {}",
            opened.text
        );
        let cookie = opened
            .header("set-cookie")
            .expect("a session cookie")
            .split(';')
            .next()
            .expect("a cookie value")
            .to_string();

        // The boot grant is re-usable — that is what lets the window survive a
        // page reload — so the same code opens a bearer session too, which is
        // what a client attached from somewhere else would hold.
        let exchanged = harness
            .send(
                "POST",
                "/oauth/token",
                &ClientIdentity::anonymous(),
                Some((
                    "application/x-www-form-urlencoded",
                    &format!(
                        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange\
                         &subject_token={boot}\
                         &subject_token_type=urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap\
                         &requested_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token"
                    ),
                )),
            )
            .await;
        assert_eq!(
            exchanged.status, 200,
            "the harness could not exchange the boot credential: {}",
            exchanged.text
        );

        harness.session = Session {
            boot,
            cookie,
            bearer: exchanged.body["access_token"]
                .as_str()
                .expect("an access token")
                .to_string(),
        };
        harness
    }

    /// A credential this server actually minted, in the shape the browser UI
    /// presents: a session cookie and a loopback origin.
    ///
    /// What [`TestServer::connect`] and [`TestServer::get`] use, so that a test
    /// which is not about authentication does not have to think about it.
    pub fn browser(&self) -> ClientIdentity {
        ClientIdentity {
            cookie: Some(self.session.cookie.clone()),
            origin: Some(format!("http://{}", self.addr())),
            ..ClientIdentity::default()
        }
    }

    /// The shape a client attached from somewhere else presents: a bearer, and
    /// no cookie and no origin.
    pub fn bearer(&self) -> ClientIdentity {
        ClientIdentity {
            authorization: Some(format!("Bearer {}", self.session.bearer)),
            ..ClientIdentity::default()
        }
    }

    /// The credential the window booted with. Re-usable, so a test may spend it
    /// as often as it likes — which is itself worth a test, and has one.
    pub fn boot_credential(&self) -> &str {
        &self.session.boot
    }

    pub fn bearer_token(&self) -> &str {
        &self.session.bearer
    }

    /// A **fresh** socket ticket, in the query parameter the browser is forced
    /// to use.
    ///
    /// Minted per call rather than held, because a ticket is single use: two
    /// upgrades sharing one is the thing the store refuses, so a harness that
    /// cached one would make every second connection fail for a reason that
    /// had nothing to do with the test.
    pub async fn ticketed(&self) -> ClientIdentity {
        let issued = self
            .send(
                "POST",
                "/api/auth/websocket-ticket",
                &self.bearer(),
                Some(("application/json", "")),
            )
            .await;
        assert_eq!(
            issued.status, 200,
            "the harness could not mint a socket ticket: {}",
            issued.text
        );
        ClientIdentity {
            ticket: Some(
                issued.body["ticket"]
                    .as_str()
                    .expect("a ticket")
                    .to_string(),
            ),
            ..ClientIdentity::default()
        }
    }

    pub fn ws_url(&self) -> String {
        self.server.ws_url()
    }

    pub fn addr(&self) -> std::net::SocketAddr {
        self.server.local_addr()
    }

    /// Sockets currently open, as the server counts them. Used to check that
    /// disconnecting leaves nothing behind.
    pub fn live_connections(&self) -> usize {
        self.server.state().live_connections()
    }

    /// Wait for the gauge to reach `expected`, or fail saying what it was.
    ///
    /// A closed socket is torn down by the connection's own task, so the
    /// count drops a moment after the client stops caring. Polling is the
    /// honest way to observe that without pretending it is synchronous.
    pub async fn await_live_connections(&self, expected: usize) {
        self.await_gauge("live connections", expected, || self.live_connections())
            .await;
    }

    /// The waiting the three gauges share.
    ///
    /// Each of them is a number some other task moves — a connection's own
    /// teardown, a pump ending, a deferred listing finishing — so none of them
    /// settles synchronously with the call that caused it. `name` is only for
    /// the failure message, and the failure message is the point: without it a
    /// gauge that never settles is a hung suite instead of a sentence saying
    /// which one and what it stuck at.
    async fn await_gauge(&self, name: &str, expected: usize, read: impl Fn() -> usize) {
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        while read() != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "{name} stayed at {} instead of settling to {expected}",
                read()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Subscriptions the server is currently pumping, across every
    /// connection. The subscription half of [`TestServer::live_connections`],
    /// and how "resources are released on unsubscribe" is observed from
    /// outside.
    pub fn live_subscriptions(&self) -> usize {
        self.server.state().live_subscriptions()
    }

    /// Wait for the subscription gauge to reach `expected`, or fail saying
    /// what it was. Same reasoning as [`TestServer::await_live_connections`]:
    /// a pump is torn down by its own task, a moment after whatever ended it.
    pub async fn await_live_subscriptions(&self, expected: usize) {
        self.await_gauge("live subscriptions", expected, || {
            self.live_subscriptions()
        })
        .await;
    }

    /// Workspaces the server is watching. The filesystem half of the same
    /// accounting: a watch outlives the call that started it, so this is where
    /// one that was never released shows up.
    pub fn watched_workspaces(&self) -> usize {
        self.server.state().watched_workspaces()
    }

    /// Wait for the watch gauge to reach `expected`, or fail saying what it
    /// was. `projects.listEntries` is answered off the read loop, so the watch
    /// it starts is in place a moment after the client has its answer.
    pub async fn await_watched_workspaces(&self, expected: usize) {
        self.await_gauge("watched workspaces", expected, || {
            self.watched_workspaces()
        })
        .await;
    }

    /// Agent processes running, as the server counts them. How "the subprocess
    /// is terminated and reaped when the session ends" is observed from outside.
    pub fn live_agents(&self) -> usize {
        self.server.state().live_agents()
    }

    /// Wait for the agent gauge to reach `expected`, or fail saying what it was.
    /// A session starts and ends on its own task, so neither settles
    /// synchronously with the call that caused it.
    pub async fn await_live_agents(&self, expected: usize) {
        self.await_gauge("live agents", expected, || self.live_agents())
            .await;
    }

    /// Shells running behind a terminal. The fifth of the same family, and how
    /// "closing the app reaps its terminals" is observed from outside.
    pub fn live_terminals(&self) -> usize {
        self.server.state().live_terminals()
    }

    /// Wait for the terminal gauge to reach `expected`, or fail saying what it
    /// was. A shell is reaped by a thread of its own, so it settles a moment
    /// after whatever ended it.
    pub async fn await_live_terminals(&self, expected: usize) {
        self.await_gauge("live terminals", expected, || self.live_terminals())
            .await;
    }

    /// How often the buffered assistant message and the deltas before it
    /// agreed — the continuous check on the assumption that makes streaming
    /// safe. See `laplus_server::threads::Reconciliation`.
    pub fn reconciliation(&self) -> Reconciliation {
        self.server.state().reconciliation()
    }

    pub fn unrecognized_messages(&self) -> usize {
        self.server.state().unrecognized_messages()
    }

    pub fn unparseable_frames(&self) -> usize {
        self.server.state().unparseable_frames()
    }

    /// The config the server will answer `server.getConfig` with.
    pub fn config(&self) -> Value {
        self.server.state().config().current().to_value()
    }

    /// Make a server-side configuration change, as a later ticket's provider
    /// registry or settings writer will. This is the *cause* a subscriber
    /// observes; nothing about the assertion reaches into the server.
    pub fn change_config(&self, change: ConfigChange) {
        self.server.state().config().apply(change);
    }

    /// Resolve the agent binary the way the app's own startup does, off the
    /// machine's `PATH`. Returns at once; the answer arrives from its own thread,
    /// so pair it with [`TestServer::await_provider_state`].
    pub fn probe_provider(&self) {
        self.server.probe_provider();
    }

    /// Go looking for the agent binary in `search` and wait for the answer to be
    /// published — the same call [`Server::probe_provider`] makes, with the
    /// directories it may look in supplied by the test rather than read off the
    /// machine.
    ///
    /// That substitution is data, not a switch: resolution runs in full, and only
    /// the list of directories differs. It has to be an argument because `PATH` is
    /// process-wide mutable state, so a test that set it would be changing it for
    /// every other test running beside it.
    pub async fn refresh_providers(&self, search: Search) {
        let config = self.server.state().config().clone();
        let roots = self.server.state().workspace_roots();
        tokio::task::spawn_blocking(move || {
            laplus_server::provider::refresh(&config, &search, &roots)
        })
            .await
            .expect("the probe finishes");
    }

    /// Wait for the provider instance to appear in `state`, or fail saying what it
    /// was instead.
    ///
    /// [`Server::probe_provider`] publishes from its own thread, so the state a
    /// test is waiting for arrives a moment after the call — the same reasoning as
    /// [`TestServer::await_live_connections`]. Takes a [`ProviderState`] rather
    /// than a string so a typo is a compile error.
    pub async fn await_provider_state(&self, state: ProviderState) -> Value {
        let wanted = serde_json::to_value(state).expect("a provider state serializes");
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        loop {
            let providers = self.config()["providers"].clone();
            if providers[0]["status"] == wanted {
                return providers[0].clone();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the provider stayed at {providers} instead of settling to {wanted}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A settings change that alters one visible flag. The cheapest real
    /// change to make, and the one most tests want.
    pub fn toggle_a_setting(&self) -> bool {
        let mut settings = self.server.state().config().current().settings.clone();
        settings.enable_assistant_streaming = !settings.enable_assistant_streaming;
        let now = settings.enable_assistant_streaming;
        self.change_config(ConfigChange::Settings(Box::new(settings)));
        now
    }

    /// Connect the way a non-browser client does: a `wsTicket`, no `Origin`.
    ///
    /// The ticket is a real one, minted for this call — see
    /// [`TestServer::ticketed`]. Before ticket 73 it was an invented string,
    /// because nothing verified it.
    pub async fn connect(&self) -> SocketClient {
        self.connect_as(self.ticketed().await)
            .await
            .expect("upgrade is accepted")
    }

    pub async fn connect_as(&self, identity: ClientIdentity) -> Result<SocketClient, Refusal> {
        let mut request = identity
            .ticketed(&self.ws_url())
            .into_client_request()
            .expect("the server's own url is a valid websocket request");
        for (name, value) in identity.headers() {
            if let Some(value) = value {
                request.headers_mut().insert(
                    name,
                    value.parse().expect("header value is valid ascii"),
                );
            }
        }

        match connect_async(request).await {
            Ok((socket, _response)) => Ok(SocketClient {
                socket,
                next_id: 0,
                buffered: Vec::new(),
            }),
            Err(WsError::Http(response)) => {
                let body = response
                    .body()
                    .as_ref()
                    .and_then(|bytes| serde_json::from_slice(bytes).ok())
                    .unwrap_or(Value::Null);
                Err(Refusal {
                    status: response.status().as_u16(),
                    body,
                })
            }
            Err(error) => panic!("connecting failed for a non-http reason: {error}"),
        }
    }

    /// A plain `GET` presenting nothing. Raw HTTP rather than a client library,
    /// for the same reason [`TestServer::raw_upgrade`] is: no dependency, and
    /// nothing between the assertion and the bytes.
    /// A `GET` presenting a credential this server minted, the way the window
    /// does.
    ///
    /// **Not anonymous, since ticket 73.** Two of the routes reachable this way
    /// require a credential that verifies, and the others do not care — so
    /// presenting one is what an ordinary request looks like, and a test that
    /// wants to show a request with *nothing* says so with
    /// `get_as(path, &ClientIdentity::anonymous())`.
    pub async fn get(&self, path: &str) -> HttpResponse {
        self.get_as(path, &self.browser()).await
    }

    /// A `GET` presenting what a client presents.
    ///
    /// The same [`ClientIdentity`] the socket upgrade takes, because ticket 31's
    /// two snapshot routes accept exactly what the upgrade accepts — so a test
    /// that shows a credential reading a snapshot and the same credential
    /// opening a socket is showing it with one vocabulary rather than two.
    pub async fn get_as(&self, path: &str, identity: &ClientIdentity) -> HttpResponse {
        self.send("GET", path, identity, None).await
    }

    /// A `POST` of a JSON body, presenting what the window presents.
    pub async fn post_json(&self, path: &str, body: &Value) -> HttpResponse {
        self.post_json_as(path, &self.browser(), body).await
    }

    pub async fn post_json_as(
        &self,
        path: &str,
        identity: &ClientIdentity,
        body: &Value,
    ) -> HttpResponse {
        self.send(
            "POST",
            path,
            identity,
            Some(("application/json", &body.to_string())),
        )
        .await
    }

    /// A `POST` of a form body. One route takes one — `/oauth/token`, because
    /// RFC 6749 says so and `AuthTokenExchangeRequest` ends
    /// `.pipe(HttpApiSchema.asFormUrlEncoded())`.
    /// Only `/oauth/token` takes one, and it needs no credential — so this
    /// stays anonymous where the JSON helpers do not.
    pub async fn post_form(&self, path: &str, body: &str) -> HttpResponse {
        self.post_form_as(path, &ClientIdentity::anonymous(), body)
            .await
    }

    pub async fn post_form_as(
        &self,
        path: &str,
        identity: &ClientIdentity,
        body: &str,
    ) -> HttpResponse {
        self.send(
            "POST",
            path,
            identity,
            Some(("application/x-www-form-urlencoded", body)),
        )
        .await
    }

    /// A `POST` with no body at all. `/api/auth/websocket-ticket` takes none —
    /// the contract declares no payload for it, and what it reads is the
    /// bearer in the header.
    pub async fn post_as(&self, path: &str, identity: &ClientIdentity) -> HttpResponse {
        self.send("POST", path, identity, Some(("application/json", "")))
            .await
    }

    /// The preflight a browser sends before a cross-origin call it cannot make
    /// simply — which is any of them with a JSON body or an `Authorization`
    /// header. Ticket 02 of the headless-Linux effort.
    ///
    /// **Presents no credential, because a preflight carries none.** That is not
    /// this harness being economical: a browser sends the `Origin` and the two
    /// `Access-Control-Request-*` headers and nothing else, so a route that
    /// checked a credential here would refuse every cross-origin request before
    /// the real one was ever sent.
    ///
    /// Written out by hand rather than going through [`TestServer::send`],
    /// because those two request headers are the whole of what makes this a
    /// preflight rather than a bare `OPTIONS` — and a future CORS
    /// implementation that reads them would answer a bare one as an ordinary
    /// request.
    pub async fn preflight(&self, method: &str, path: &str) -> HttpResponse {
        let request = format!(
            "OPTIONS {path} HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Origin: {DESKTOP_WINDOW_ORIGIN}\r\n\
             Access-Control-Request-Method: {method}\r\n\
             Access-Control-Request-Headers: authorization, content-type\r\n\
             Connection: close\r\n\r\n",
            addr = self.addr(),
        );
        parse_response(&self.raw_request(&request).await)
    }

    /// One request, written by hand. Raw HTTP rather than a client library, for
    /// the same reason [`TestServer::raw_upgrade`] is: no dependency, and
    /// nothing between the assertion and the bytes.
    async fn send(
        &self,
        method: &str,
        path: &str,
        identity: &ClientIdentity,
        body: Option<(&str, &str)>,
    ) -> HttpResponse {
        let mut request = format!("{method} {}", identity.ticketed(path));
        request.push_str(&format!(" HTTP/1.1\r\nHost: {}\r\n", self.addr()));
        for (name, value) in identity.headers() {
            if let Some(value) = value {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
        }
        if let Some((content_type, payload)) = body {
            request.push_str(&format!("Content-Type: {content_type}\r\n"));
            request.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        request.push_str("Connection: close\r\n\r\n");
        if let Some((_, payload)) = body {
            request.push_str(payload);
        }

        parse_response(&self.raw_request(&request).await)
    }

    /// Perform the upgrade by hand and return the raw response head.
    ///
    /// Some of what ticket 01 pinned is in the handshake itself rather than in
    /// any frame — that `permessage-deflate` is declined, that no subprotocol
    /// is negotiated. A WebSocket library normalises those away, so the only
    /// way to assert them is to speak HTTP directly.
    pub async fn raw_upgrade(&self, request_head: &str) -> String {
        // A 101 leaves the connection open, so stop at the blank line rather
        // than waiting for an end that never comes.
        self.raw_exchange(request_head, StopAt::EndOfHead).await
    }

    /// Send a request and read until the server closes. Only safe with
    /// `Connection: close`.
    pub async fn raw_request(&self, request_head: &str) -> String {
        self.raw_exchange(request_head, StopAt::EndOfStream).await
    }

    async fn raw_exchange(&self, request_head: &str, stop: StopAt) -> String {
        let mut stream = TcpStream::connect(self.addr())
            .await
            .expect("connects to the listener");
        stream
            .write_all(request_head.as_bytes())
            .await
            .expect("writes the request");

        let mut response = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
                .await
                .expect("no HTTP response within READ_TIMEOUT — wedged, not merely slow")
                .expect("reads from the socket");
            if read == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..read]);
            if stop == StopAt::EndOfHead
                && response.windows(4).any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }

        String::from_utf8_lossy(&response).into_owned()
    }

    pub async fn stop(self) {
        self.server.shutdown().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopAt {
    EndOfHead,
    EndOfStream,
}

/// The origin the desktop window's page is served from, and so the origin every
/// call to a *remote* laplus arrives with. `crate::launch::DEFAULT_PORT`, which
/// is the point: it is a second laplus on another port or another host that this
/// one is being reached from, never this server's own address.
pub const DESKTOP_WINDOW_ORIGIN: &str = "http://127.0.0.1:4773";

/// A response head and body, split apart.
///
/// Free rather than a method because [`TestServer::preflight`] writes its own
/// request and still wants the same parse — the alternative is two readings of
/// one wire format, which is how a test starts asserting against a bug in its
/// own harness.
fn parse_response(raw: &str) -> HttpResponse {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no header/body boundary in: {raw}"));

    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status code in: {head}"));

    HttpResponse {
        status,
        head: head.to_string(),
        body: serde_json::from_str(body).unwrap_or(Value::Null),
        text: body.to_string(),
    }
}

/// A plain HTTP response, with its body parsed as JSON when it is JSON.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub head: String,
    pub body: Value,
    /// The body as it arrived. What the UI's own files are checked against —
    /// they are HTML and JavaScript, so [`HttpResponse::body`] is `Null` for
    /// every one of them.
    pub text: String,
}

impl HttpResponse {
    /// One response header by name, case-insensitively.
    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    }
}

/// What a client presents at the upgrade.
#[derive(Debug, Default, Clone)]
pub struct ClientIdentity {
    pub ticket: Option<String>,
    pub origin: Option<String>,
    pub cookie: Option<String>,
    pub authorization: Option<String>,
}

impl ClientIdentity {
    /// Nothing at all — the permissive path.
    pub fn anonymous() -> Self {
        ClientIdentity::default()
    }

    /// The shape a non-browser client sends, as captured in
    /// `fixtures/socket-wire/02-request-response.ndjson`.
    pub fn ticket() -> Self {
        ClientIdentity {
            ticket: Some("eyJ2IjoxLCJraW5kIjoid2Vic29ja2V0In0.c2lnbmF0dXJl".to_string()),
            ..ClientIdentity::default()
        }
    }

    /// The shape the browser UI sends: a session cookie and a loopback origin.
    pub fn browser() -> Self {
        ClientIdentity {
            cookie: Some("t3_session=eyJ2IjoxLCJraW5kIjoic2Vzc2lvbiJ9.c2lnbmF0dXJl".to_string()),
            origin: Some("http://127.0.0.1".to_string()),
            ..ClientIdentity::default()
        }
    }

    /// The shape a remote-attached client sends: a bearer token and no cookie.
    pub fn bearer() -> Self {
        ClientIdentity {
            authorization: Some("Bearer eyJ2IjoxLCJraW5kIjoiYWNjZXNzIn0.c2lnbmF0dXJl".to_string()),
            ..ClientIdentity::default()
        }
    }

    pub fn with_origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_string());
        self
    }

    /// A bearer this test minted, rather than [`ClientIdentity::bearer`]'s
    /// invented one. Ticket 73's routes are the first that care about the
    /// difference.
    pub fn with_bearer(mut self, token: &str) -> Self {
        self.authorization = Some(format!("Bearer {token}"));
        self
    }

    /// A socket ticket this test minted, in the query parameter the browser is
    /// forced to use.
    pub fn with_ticket(mut self, ticket: &str) -> Self {
        self.ticket = Some(ticket.to_string());
        self
    }

    /// The url or path with the ticket appended, if there is one.
    ///
    /// The browser cannot set headers on a WebSocket, which is why the ticket
    /// travels in the query string and why this is not simply another header.
    fn ticketed(&self, target: &str) -> String {
        match &self.ticket {
            Some(ticket) => {
                let separator = if target.contains('?') { '&' } else { '?' };
                format!("{target}{separator}wsTicket={ticket}")
            }
            None => target.to_string(),
        }
    }

    /// The three headers a credential can arrive in, present or not.
    ///
    /// Shared by [`TestServer::connect_as`] and [`TestServer::get_as`], because
    /// the two snapshot routes accept exactly what the upgrade accepts and a
    /// second copy of this list is a second chance for them to stop doing so
    /// without a test noticing.
    fn headers(&self) -> [(&'static str, Option<&str>); 3] {
        [
            ("origin", self.origin.as_deref()),
            ("cookie", self.cookie.as_deref()),
            ("authorization", self.authorization.as_deref()),
        ]
    }
}

/// A refused upgrade: the socket never opened.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub status: u16,
    pub body: Value,
}

/// One open socket, and the request-id space that belongs to it.
#[derive(Debug)]
pub struct SocketClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// Frames that arrived while waiting for a different `requestId`. Kept so
    /// out-of-order answers are not lost, which is the whole point of
    /// correlating rather than assuming FIFO.
    buffered: Vec<Value>,
}

/// What a call came back as. A `Defect` is a distinct outcome rather than an
/// error case because it carries no `requestId` and therefore is not, strictly,
/// an answer to anything.
#[derive(Debug, Clone)]
pub enum Outcome {
    Success(Value),
    Failure(Vec<Value>),
    Defect(Value),
}

impl Outcome {
    /// The success value, or a panic naming what came back instead.
    pub fn expect_success(self) -> Value {
        match self {
            Outcome::Success(value) => value,
            other => panic!("expected a success exit, got {other:?}"),
        }
    }

    /// The typed error from a refused call, checked to be one the method
    /// declares.
    ///
    /// Two assertions, and each is a distinct way of failing the client: an
    /// error that is not `Fail` is not a refusal at all, and one whose `_tag`
    /// is not in the method's declared union fails to decode and costs the
    /// call.
    ///
    /// **Not asserted here: a `message`.** Whether the sentence is on the wire
    /// depends on the error's own schema. `ProjectReadFileError` declares
    /// `message` as a field and carries it — see the captured payload in
    /// `fixtures/socket-wire/03-typed-error.ndjson` — while every
    /// `ExternalLauncher*` class defines `message` as a getter over its
    /// structured fields, so the client computes it and the reference server
    /// sends nothing. A blanket assertion here would force laplus to send a
    /// field the reference server does not. Tests that care assert on it
    /// themselves.
    pub fn expect_declared(self, tag: &str) -> Value {
        match self {
            Outcome::Failure(cause) => {
                assert_eq!(cause.len(), 1, "one cause entry: {cause:?}");
                assert_eq!(cause[0]["_tag"], "Fail");
                assert_eq!(
                    cause[0]["error"]["_tag"], tag,
                    "the error must be one the method declares, or the client cannot decode it"
                );
                cause[0]["error"].clone()
            }
            other => panic!("expected a typed failure, got {other:?}"),
        }
    }
}

impl SocketClient {
    /// Send a `Request` and wait for its answer, correlating by `requestId`.
    pub async fn call(&mut self, tag: &str, payload: Value) -> Outcome {
        let id = self.send_request(tag, payload).await;
        self.await_outcome(&id).await
    }

    /// Send a `Request` and return the answering frame untouched, envelope and
    /// all. Conformance tests compare envelopes, not just payloads.
    pub async fn call_raw(&mut self, tag: &str, payload: Value) -> Value {
        self.send_request(tag, payload).await;
        self.recv().await
    }

    /// Send a `Request` without waiting, returning its id. For driving
    /// concurrent calls.
    pub async fn send_request(&mut self, tag: &str, payload: Value) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.send(json!({
            "_tag": "Request",
            "id": id,
            "tag": tag,
            "payload": payload,
            "headers": [],
        }))
        .await;
        id
    }

    /// Wait for the answer to an already-sent request.
    pub async fn await_outcome(&mut self, request_id: &str) -> Outcome {
        let frame = self.take_frame(|frame| answers(frame, request_id)).await;
        outcome(frame)
    }

    /// The next frame matching `wanted`, reading until one arrives.
    ///
    /// Frames that do not match are buffered rather than dropped, which is the
    /// whole reason this exists: the reference server answers concurrent calls
    /// out of order and interleaves a subscription's chunks with everything
    /// else the connection is doing, so a client that consumed frames in
    /// arrival order would lose the ones it was not waiting for.
    async fn take_frame(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Some(index) = self.buffered.iter().position(&wanted) {
                return self.buffered.remove(index);
            }

            let frame = self.recv().await;
            if wanted(&frame) {
                return frame;
            }
            self.buffered.push(frame);
        }
    }

    /// Open a subscription and return its request id.
    ///
    /// There is no `subscribe` verb on this wire — a subscription is an
    /// ordinary `Request` — so this is [`SocketClient::send_request`] under a
    /// name that says what the caller means.
    pub async fn subscribe(&mut self, tag: &str, payload: Value) -> String {
        self.send_request(tag, payload).await
    }

    /// Acknowledge a chunk, releasing the server to send the next one.
    ///
    /// Not optional: the server holds at most one un-acknowledged chunk per
    /// request, so a test that forgets this simply stops receiving.
    pub async fn ack(&mut self, request_id: &str) {
        self.send(json!({"_tag": "Ack", "requestId": request_id})).await;
    }

    /// Cancel a call. For a subscription this is the unsubscribe.
    pub async fn interrupt(&mut self, request_id: &str) {
        self.send(json!({"_tag": "Interrupt", "requestId": request_id}))
            .await;
    }

    /// The next frame that concerns `request_id`, whole — a `Chunk` or the
    /// terminal `Exit`.
    pub async fn next_frame_for(&mut self, request_id: &str) -> Value {
        self.take_frame(|frame| concerns(frame, request_id)).await
    }

    /// The `values` of the next `Chunk` for `request_id`.
    ///
    /// Deliberately does **not** acknowledge. `Ack` is load-bearing on this
    /// wire — the server stops after one un-acknowledged chunk — so a test
    /// that wants to keep receiving has to say so, the same way the UI does.
    pub async fn next_chunk(&mut self, request_id: &str) -> Vec<Value> {
        let frame = self.next_frame_for(request_id).await;
        assert_eq!(
            frame["_tag"],
            json!("Chunk"),
            "expected a chunk for {request_id}, the stream ended instead: {frame}"
        );
        frame["values"]
            .as_array()
            .unwrap_or_else(|| panic!("a chunk's values are an array: {frame}"))
            .clone()
    }

    /// Everything a subscription has produced up to and including the first
    /// value matching `wanted`, acknowledging as it goes.
    ///
    /// What a streamed turn needs: it produces a dozen values whose split
    /// across chunks is decided by how fast the agent talks relative to how fast
    /// the test acknowledges, so a test that counted chunks would be asserting
    /// on timing. This asserts on the *sequence of values*, which is what the UI
    /// folds, and reads until the turn is over rather than a fixed number of
    /// times.
    pub async fn values_until(
        &mut self,
        request_id: &str,
        wanted: impl Fn(&Value) -> bool,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let values = self.next_chunk(request_id).await;
            self.ack(request_id).await;
            let arrived = values.iter().any(&wanted);
            seen.extend(values);
            if arrived {
                return seen;
            }
        }
    }

    /// The single value of the next chunk, acknowledged. `values` genuinely
    /// batches, so a test that expects one value says so.
    pub async fn next_event(&mut self, request_id: &str) -> Value {
        let values = self.next_chunk(request_id).await;
        self.ack(request_id).await;
        assert_eq!(values.len(), 1, "expected a single value, got {values:#?}");
        values.into_iter().next().expect("a value")
    }

    /// `Ping` and the `Pong` that answers it.
    ///
    /// Strict: the `Pong` must be the *next* frame. Every caller but one wants
    /// that, because a pong arriving behind unexpected traffic is worth
    /// failing on.
    pub async fn ping(&mut self) -> Value {
        self.send(json!({"_tag": "Ping"})).await;
        self.recv().await
    }

    /// Send a frame exactly as given. For malformed and unrecognised frames,
    /// which the typed helpers cannot express.
    pub async fn send(&mut self, frame: Value) {
        self.send_text(&frame.to_string()).await;
    }

    pub async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.into()))
            .await
            .expect("sends a text frame");
    }

    /// The next frame, whatever it is.
    pub async fn recv(&mut self) -> Value {
        loop {
            let frame = tokio::time::timeout(READ_TIMEOUT, self.socket.next())
                .await
                .expect("no frame within READ_TIMEOUT — wedged, not merely slow")
                .expect("the socket is still open")
                .expect("the frame is readable");

            match frame {
                Message::Text(text) => {
                    return serde_json::from_str(text.as_str())
                        .unwrap_or_else(|error| panic!("frame is not json: {error}: {text}"))
                }
                Message::Binary(bytes) => panic!("unexpected binary frame of {} bytes", bytes.len()),
                Message::Close(frame) => panic!("server closed the socket: {frame:?}"),
                // Control frames the library handles; keep reading.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    /// Assert nothing more arrives for a while. Used to check that a frame the
    /// server should ignore really is ignored.
    pub async fn expect_silence(&mut self, how_long: Duration) {
        if let Ok(Some(frame)) = tokio::time::timeout(how_long, self.socket.next()).await {
            panic!("expected silence, got {frame:?}");
        }
    }

    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }

    /// Vanish without a close handshake — a dropped TCP connection, which is
    /// what a killed browser tab or a lost network looks like from the
    /// server's side.
    pub fn abandon(self) {
        drop(self);
    }
}

/// Does this frame answer `request_id`? A `Defect` answers everything, because
/// it carries no id and no `Exit` will follow.
fn answers(frame: &Value, request_id: &str) -> bool {
    match frame["_tag"].as_str() {
        Some("Exit") => frame["requestId"] == json!(request_id),
        Some("Defect") => true,
        _ => false,
    }
}

/// Is this frame about `request_id` at all — a `Chunk` or an `Exit`?
fn concerns(frame: &Value, request_id: &str) -> bool {
    matches!(frame["_tag"].as_str(), Some("Chunk") | Some("Exit"))
        && frame["requestId"] == json!(request_id)
        || answers(frame, request_id)
}

fn outcome(frame: Value) -> Outcome {
    match frame["_tag"].as_str() {
        Some("Defect") => Outcome::Defect(frame["defect"].clone()),
        Some("Exit") => match frame["exit"]["_tag"].as_str() {
            Some("Success") => Outcome::Success(frame["exit"]["value"].clone()),
            Some("Failure") => Outcome::Failure(
                frame["exit"]["cause"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            ),
            other => panic!("unknown exit tag {other:?} in {frame}"),
        },
        other => panic!("frame {other:?} is not an answer: {frame}"),
    }
}
