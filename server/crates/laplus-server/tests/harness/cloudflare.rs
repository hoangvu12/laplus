//! One fake `cloudflared`, one fake Cloudflare DNS API, one administrative
//! client.
//!
//! **There were three fake cloudflareds and four copies of `client_with`.**
//! `http_cloudflare_connector.rs` had a supervision fake that served `/ready`,
//! `http_cloudflare_install.rs` had a downloadable one that did the same with a
//! different version string, and `http_cloudflare_account.rs` had a disjoint one
//! that answered `tunnel login` and `tunnel list` and `raise SystemExit(2)` on
//! everything else. Between them they emulated none of `tunnel create`,
//! `tunnel token`, `tunnel route dns` or `tunnel delete` — which tickets 05, 06
//! and 07 cannot begin without, and which a fourth copy would have had to grow
//! on its own.
//!
//! **`create` and `token` are two different credentials.** `tunnel create
//! --credentials-file` allocates a tunnel and writes its `<UUID>.json`;
//! `tunnel token --cred-file` retrieves the same file for a tunnel that already
//! exists. Ticket 06 uses the first and ticket 05 uses the second, and running
//! either connector needs laplus's own configuration file to name the tunnel and
//! point at the credential — which the `run` branch below checks, because a
//! connector given a config missing either would look to a test exactly like one
//! that was merely slow to become ready.
//!
//! So this is one script with every verb, switched on by what it is asked to do
//! rather than by which test wrote it. A test that needs only `--version` pays
//! for nothing else; a test that needs a running connector gets the metrics
//! server; a test that needs a partial creation stops the script where it asks.
//!
//! **`cloudflared` has no `route dns delete`.** Removing a DNS record is a
//! Cloudflare API call needing DNS authority of its own — see the cleanup
//! asymmetry in `.scratch/cloudflare-tunnel/research.md`. Modelling that as a
//! CLI verb would let ticket 07 be built against a command that does not exist,
//! so [`FakeCloudflareApi`] is a real local HTTP server and the deletion path
//! goes through it, in the way `FakeRelease` already models the download feed.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::{ClientIdentity, TestServer};

/// The three literals `AuthTokenExchangeRequest` pins, as the client encodes
/// them. Percent-encoded because that is what actually goes over the wire: a
/// `:` in a form value is legal unencoded, and the client's form encoder escapes
/// it anyway.
pub const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange";
pub const BOOTSTRAP_TOKEN_TYPE: &str =
    "urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap";
pub const ACCESS_TOKEN_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token";

/// A session holding exactly `scopes`, as a real client would obtain one.
///
/// Minted and exchanged rather than forged, because the thing under test on
/// every `/api/access/cloudflare` route is what a *session* carries — ADR-0047
/// makes the desktop and headless boot grants administrative and ordinary
/// pairing not, and a hand-made grant would prove nothing about that.
pub async fn client_with(server: &TestServer, scopes: &[&str]) -> ClientIdentity {
    let minted = server
        .post_json("/api/auth/pairing-token", &json!({ "scopes": scopes }))
        .await;
    let credential = minted.body["credential"]
        .as_str()
        .unwrap_or_else(|| panic!("a pairing token: {}", minted.text));
    let exchanged = server.post_form(
        "/oauth/token",
        &format!("grant_type={GRANT_TYPE}&subject_token={credential}&subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"),
    ).await;
    ClientIdentity::anonymous().with_bearer(
        exchanged.body["access_token"]
            .as_str()
            .unwrap_or_else(|| panic!("an access token: {}", exchanged.text)),
    )
}

/// A public endpoint that always verifies.
///
/// The layered public path is covered end to end by `http_public_exposure.rs`
/// and `network_public_exposure.rs`; the connector and installation tests need
/// only that an endpoint *can* become verified, so that advertisement and
/// pairing are reachable from them. Injecting it keeps those tests off the
/// network without a flag in production code.
#[derive(Debug)]
pub struct VerifiedEndpoint;

impl laplus_server::public_exposure::EndpointVerifier for VerifiedEndpoint {
    fn verify<'a>(
        &'a self,
        _origin: &'a str,
        _environment_id: &'a str,
        _http_token: &'a str,
        _ws_token: &'a str,
    ) -> laplus_server::public_exposure::VerificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

/// What the account fake's `tunnel list` answers with by default: one active
/// tunnel (externally managed), one inactive (adoptable), one deleted (not a
/// choice at all).
pub const LISTED_TUNNELS: &str = r#"[
  {"id":"11111111-1111-1111-1111-111111111111","name":"already-running",
   "created_at":"2026-01-01T00:00:00Z","deleted_at":null,
   "connections":[{"id":"c1","origin_ip":"203.0.113.5"},{"id":"c2","origin_ip":"203.0.113.6"}]},
  {"id":"22222222-2222-2222-2222-222222222222","name":"spare",
   "created_at":"2026-02-02T00:00:00Z","deleted_at":null,"connections":[]},
  {"id":"33333333-3333-3333-3333-333333333333","name":"removed",
   "created_at":"2026-03-03T00:00:00Z","deleted_at":"2026-04-04T00:00:00Z","connections":[]}
]"#;

/// The UUID `tunnel create` allocates, so a test can assert cleanup targets the
/// resource the creation actually made rather than the name it asked for.
pub const CREATED_TUNNEL_ID: &str = "44444444-4444-4444-4444-444444444444";

/// A `cloudflared` that answers every verb laplus can invoke.
///
/// Unix only, because it is a Python script made executable by mode bits and
/// the three test binaries that drive it are `#![cfg(unix)]` for the same
/// reason. Windows CI runs the rest of the suite; see `server/CLAUDE.md`.
///
/// The files beside the executable are the whole of its configuration, so a
/// test changes behaviour by writing one rather than by rebuilding the script:
/// `mode` switches the failure being rehearsed, `signal` releases a login that
/// is waiting for a browser, `tunnels` is what `list` prints.
#[cfg(unix)]
pub struct FakeCloudflared {
    pub executable: PathBuf,
    /// Every invocation, one JSON array of arguments per line, plus a bare
    /// `stopped` line when a connector shuts down gracefully.
    pub trace: PathBuf,
    /// One word, read at the start of each invocation. See [`FakeCloudflared::rehearse`].
    pub mode: PathBuf,
    /// Touch it to release a `tunnel login` running in `await` mode.
    pub signal: PathBuf,
    /// What `tunnel list --output json` prints.
    pub tunnels: PathBuf,
    /// Where `tunnel login` writes the account certificate. Set
    /// `TUNNEL_ORIGIN_CERT` to this so laplus finds it the way it would on a
    /// developer's machine.
    pub certificate: PathBuf,
}

/// The certificate contents, which no test may ever find in a snapshot, a log
/// or an error.
pub const CERTIFICATE: &str = "FAKE-ACCOUNT-CERTIFICATE-SECRET";

/// The connector token the supervision fake insists on being handed by file.
pub const CONNECTOR_TOKEN: &str = "connector-secret";

/// What a `<UUID>.json` run credential contains, and the string ticket 05's
/// redaction test hunts for in four places: the response, the snapshot, the
/// process argv, and the database file read as bytes.
pub const TUNNEL_CREDENTIAL_SECRET: &str = "FAKE-TUNNEL-CREDENTIAL-SECRET";

/// `--version` for every copy of the fake. Compatible: 2024 or newer.
pub const VERSION: &str = "cloudflared version 2026.7.3";

#[cfg(unix)]
impl FakeCloudflared {
    /// Write the fake into `directory`, which is also where it keeps its files.
    pub fn write_into(directory: &Path) -> Self {
        let fake = Self {
            executable: directory.join("cloudflared-fake.py"),
            trace: directory.join("cloudflared.trace"),
            mode: directory.join("cloudflared.mode"),
            signal: directory.join("browser.signal"),
            tunnels: directory.join("tunnels.json"),
            certificate: directory.join("cert.pem"),
        };
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(directory).expect("the fake's directory");
        std::fs::write(&fake.executable, fake.source()).expect("the fake is written");
        std::fs::set_permissions(&fake.executable, std::fs::Permissions::from_mode(0o700))
            .expect("the fake is executable");
        std::fs::write(&fake.tunnels, LISTED_TUNNELS).expect("the default listing");
        fake
    }

    /// The same script, as bytes to be served by a download feed rather than
    /// written to disk — `http_cloudflare_install.rs` installs it.
    pub fn artifact(directory: &Path) -> Vec<u8> {
        Self {
            executable: directory.join("unused"),
            trace: directory.join("cloudflared.trace"),
            mode: directory.join("cloudflared.mode"),
            signal: directory.join("browser.signal"),
            tunnels: directory.join("tunnels.json"),
            certificate: directory.join("cert.pem"),
        }
        .source()
        .into_bytes()
    }

    /// Rehearse a failure. One of:
    ///
    /// - `fail` — `tunnel login` cannot reach Cloudflare.
    /// - `await` — `tunnel login` waits for [`FakeCloudflared::open_browser`].
    /// - `crash` — the connector exits non-zero, mentioning the token, so a test
    ///   can prove the log is redacted.
    /// - `hang` — the connector's `/ready` never answers, so a test can prove a
    ///   wedged connector can still be stopped.
    /// - `replace` — the connector forks and the original exits, which is what
    ///   cloudflared's self-replacement looks like to a supervisor.
    /// - `token-fails` — `tunnel token --cred-file` writes a truncated
    ///   credential and *then* refuses, which is ticket 05's failed retrieval
    ///   and the reason a resume cannot decide by file existence alone.
    /// - `create-fails` — `tunnel create` refuses *after* writing nothing, so a
    ///   resume has no orphan to reconcile.
    /// - `route-fails` — `tunnel create` succeeds and `tunnel route dns` refuses,
    ///   which is ticket 06's partial creation.
    /// - `delete-fails` — `tunnel delete` refuses, which is ticket 07's partial
    ///   remote cleanup.
    pub fn rehearse(&self, mode: &str) {
        std::fs::write(&self.mode, mode).expect("the mode is written");
    }

    /// Stop rehearsing anything.
    pub fn behave(&self) {
        let _ = std::fs::remove_file(&self.mode);
    }

    /// Release a `tunnel login` waiting in `await` mode.
    pub fn open_browser(&self) {
        std::fs::write(&self.signal, "opened").expect("the browser signal is written");
    }

    /// How many recorded invocations mention `verb`.
    ///
    /// A substring match over the trace rather than an argv comparison, and the
    /// trace is appended *before* the fake dispatches — so `invocations("create")`
    /// counts what laplus asked for, including a `create` the fake then refused.
    /// That is what makes `assert_eq!(fake.invocations("create"), 0)` mean "the
    /// server never asked" rather than "the fake said no".
    pub fn invocations(&self, verb: &str) -> usize {
        self.lines().filter(|line| line.contains(verb)).count()
    }

    /// How many times the executable was launched, of any kind.
    pub fn launches(&self) -> usize {
        self.lines().filter(|line| line.starts_with('[')).count()
    }

    /// Whether the last thing a connector did was shut down gracefully.
    ///
    /// The `stopped` line is written from the fake's `finally`, so it appears
    /// only when the process was asked to stop and had the chance to answer —
    /// which is what ADR-0048's "shut down gracefully with the owner" means and
    /// what a `SIGKILL`, or a laplus that never handled `SIGTERM` at all, does
    /// not produce.
    pub fn stopped_gracefully(&self) -> bool {
        self.lines().next_back().is_some_and(|line| line == "stopped")
    }

    fn lines(&self) -> impl DoubleEndedIterator<Item = String> {
        std::fs::read_to_string(&self.trace)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Where `tunnel create --credentials-file` was told to write, if it ran.
    pub fn credential_written_to(&self) -> Option<PathBuf> {
        self.argument_after("create", "--credentials-file")
    }

    /// Where `tunnel token --cred-file` was told to write, if it ran.
    ///
    /// The adoption twin of [`FakeCloudflared::credential_written_to`]: an
    /// adopted tunnel already exists at Cloudflare, so laplus *retrieves* its
    /// narrow run credential rather than creating one, and the flag the CLI
    /// spells for that is `--cred-file`.
    pub fn retrieved_credential_path(&self) -> Option<PathBuf> {
        self.argument_after("token", "--cred-file")
    }

    /// The value the first invocation containing `verb` passed after `flag`.
    fn argument_after(&self, verb: &str, flag: &str) -> Option<PathBuf> {
        self.lines()
            .filter_map(|line| serde_json::from_str::<Vec<String>>(&line).ok())
            .find(|argv| argv.iter().any(|word| word == verb))
            .and_then(|argv| {
                let index = argv.iter().position(|word| word == flag)?;
                argv.get(index + 1).map(PathBuf::from)
            })
    }

    fn source(&self) -> String {
        // Written as one script rather than assembled per capability, so that
        // the argument shapes every branch asserts on live in one place. Every
        // branch appends to the trace first and acts second.
        format!(
            r#"#!/usr/bin/env python3
import http.server, json, os, signal, sys, time
ARGS = sys.argv[1:]
TRACE = {trace:?}
MODE = {mode:?}
SIGNAL = {signal:?}
TUNNELS = {tunnels:?}
CERTIFICATE = {certificate:?}
CREATED = {created:?}

if '--version' in ARGS:
    print({version:?})
    raise SystemExit(0)

with open(TRACE, 'a') as f:
    f.write(json.dumps(ARGS) + '\n')

mode = open(MODE).read().strip() if os.path.exists(MODE) else 'ok'
certificate = os.environ.get('TUNNEL_ORIGIN_CERT', CERTIFICATE)

def after(flag, default=None):
    return ARGS[ARGS.index(flag) + 1] if flag in ARGS else default

# --- account management: everything that spends the account certificate ---

if ARGS[:2] == ['tunnel', 'login']:
    print('Please open the following URL and log in with your Cloudflare account:')
    print('https://dash.cloudflare.com/argotunnel?callback=test-callback')
    sys.stdout.flush()
    if mode == 'fail':
        print('failed to reach the Cloudflare login page', file=sys.stderr)
        raise SystemExit(1)
    if mode == 'await':
        while not os.path.exists(SIGNAL):
            time.sleep(0.02)
    with open(certificate, 'w') as f:
        f.write({content:?})
    raise SystemExit(0)

if 'list' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    if not os.path.exists(certificate):
        print('Cannot determine default origin certificate path', file=sys.stderr)
        raise SystemExit(1)
    print(open(TUNNELS).read())
    raise SystemExit(0)

if 'token' in ARGS:
    # An adopted tunnel already exists, so its narrow run credential is
    # *retrieved* rather than created. `--cred-file` writes the same
    # `<UUID>.json` shape `create` does, which is what lets laplus run a tunnel
    # it did not allocate without ever holding account-wide authority again.
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    credentials = after('--cred-file')
    assert credentials is not None, ARGS
    if mode == 'token-fails':
        # Writes and *then* fails, which is the shape that matters: a resume
        # deciding by file existence alone would skip a retrieval it still needs
        # and run a connector against a credential that authenticates nothing.
        with open(credentials, 'w') as f:
            f.write('{{"AccountTag": "acc')
        print('failed to retrieve credentials for tunnel %s' % ARGS[-1], file=sys.stderr)
        raise SystemExit(1)
    with open(credentials, 'w') as f:
        json.dump({{'AccountTag': 'account', 'TunnelID': ARGS[-1],
                   'TunnelSecret': {credential:?}}}, f)
    os.chmod(credentials, 0o600)
    raise SystemExit(0)

if 'create' in ARGS:
    # `--credentials-file` is what keeps the narrow run credential out of
    # cloudflared's own default location and inside laplus's private directory.
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    credentials = after('--credentials-file')
    assert credentials is not None, ARGS
    if mode == 'create-fails':
        print('failed to create tunnel', file=sys.stderr)
        raise SystemExit(1)
    with open(credentials, 'w') as f:
        json.dump({{'AccountTag': 'account', 'TunnelID': CREATED,
                   'TunnelSecret': {credential:?}}}, f)
    os.chmod(credentials, 0o600)
    if '--output' in ARGS:
        print(json.dumps({{'id': CREATED, 'name': ARGS[-1],
                           'created_at': '2026-08-03T00:00:00Z'}}))
    else:
        print('Created tunnel %s with id %s' % (ARGS[-1], CREATED))
    raise SystemExit(0)

if 'route' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    # `cloudflared tunnel route dns` creates a CNAME and there is no symmetric
    # `route dns delete` — removing a record is a Cloudflare DNS API call with
    # its own authority (`research.md`). Refused explicitly rather than by
    # falling through, so the fixture cannot quietly grow the verb and let
    # ticket 07 be built against a command that does not exist.
    if 'delete' in ARGS or 'remove' in ARGS:
        print('Error: unknown command "delete" for "cloudflared tunnel route dns"',
              file=sys.stderr)
        raise SystemExit(1)
    if mode == 'route-fails':
        print('failed to add route', file=sys.stderr)
        raise SystemExit(1)
    print('Added CNAME %s which will route to this tunnel' % ARGS[-1])
    raise SystemExit(0)

if 'delete' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    if mode == 'delete-fails':
        print('cannot delete tunnel with active connections', file=sys.stderr)
        raise SystemExit(1)
    print('Deleted tunnel %s' % ARGS[-1])
    raise SystemExit(0)

# --- running a connector ---

if 'run' not in ARGS:
    raise SystemExit(2)

metrics = after('--metrics')
token_file = after('--token-file')
config = after('--config')
if token_file is not None:
    with open(token_file) as f:
        assert f.read() == {token:?}
else:
    # A dedicated tunnel carries no connector token: everything cloudflared
    # needs is in laplus's own configuration file, which must name the tunnel
    # and point at a credential that is actually there. Asserted rather than
    # assumed, because a config missing either would make the connector fail
    # at Cloudflare where a test can only see "not ready".
    assert config is not None, ARGS
    lines = [line.strip() for line in open(config).read().splitlines()]
    named = [line for line in lines if line.startswith('tunnel:')]
    held = [line for line in lines if line.startswith('credentials-file:')]
    assert named and held, lines
    assert os.path.exists(held[0].split(':', 1)[1].strip()), lines

if mode == 'crash':
    print('connector failed with %s' % {token:?}, file=sys.stderr)
    raise SystemExit(17)
if mode == 'replace':
    os.remove(MODE)
    if os.fork() > 0:
        raise SystemExit(0)

class Ready(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        while os.path.exists(MODE) and open(MODE).read().strip() == 'hang':
            time.sleep(0.05)
        self.send_response(200 if self.path == '/ready' else 404)
        self.end_headers()
    def log_message(self, *args): pass

host, port = metrics.rsplit(':', 1)
server = http.server.HTTPServer((host, int(port)), Ready)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
try:
    server.serve_forever()
finally:
    with open(TRACE, 'a') as f:
        f.write('stopped\n')
"#,
            trace = self.trace.display().to_string(),
            mode = self.mode.display().to_string(),
            signal = self.signal.display().to_string(),
            tunnels = self.tunnels.display().to_string(),
            certificate = self.certificate.display().to_string(),
            created = CREATED_TUNNEL_ID,
            version = VERSION,
            content = CERTIFICATE,
            token = CONNECTOR_TOKEN,
            credential = TUNNEL_CREDENTIAL_SECRET,
        )
    }
}

/// The Cloudflare DNS API, for the one operation the CLI cannot do.
///
/// `cloudflared` has no `route dns delete`, so ticket 07's "Delete everywhere"
/// has to remove the recorded DNS record through Cloudflare's API with DNS
/// authority of its own. This models that end: it holds a record per zone,
/// answers a `DELETE`, and remembers what it was asked so a test can prove the
/// *exact* recorded record was targeted and no other.
///
/// `FakeRelease` in `http_cloudflare_install.rs` is the precedent — a real local
/// HTTP server pointed at by an environment variable, rather than a seam carved
/// into production code for the test's benefit.
pub struct FakeCloudflareApi {
    pub origin: String,
    pub address: SocketAddr,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    records: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}

impl FakeCloudflareApi {
    /// Start with one record, as a laplus-created route would have left.
    pub async fn start(zone_id: &str, record_id: &str, name: &str) -> Self {
        let records = Arc::new(Mutex::new(vec![json!({
            "id": record_id, "zone_id": zone_id, "name": name, "type": "CNAME",
        })]));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port for the fake Cloudflare API");
        let address = listener.local_addr().expect("the fake API's address");
        let router = axum::Router::new()
            .route(
                "/client/v4/zones/{zone}/dns_records/{record}",
                axum::routing::delete(delete_record).get(get_record),
            )
            .with_state((Arc::clone(&records), Arc::clone(&requests)));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            origin: format!("http://{address}"),
            address,
            requests,
            records,
            server,
        }
    }

    /// Every request this API received, as `(method, path)`.
    pub fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().expect("the recorded requests").clone()
    }

    /// The records that still exist.
    pub fn records(&self) -> Vec<Value> {
        self.records.lock().expect("the records").clone()
    }

    pub fn stop(self) {
        self.server.abort();
    }
}

type ApiState = (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<(String, String)>>>);

async fn delete_record(
    axum::extract::State((records, requests)): axum::extract::State<ApiState>,
    axum::extract::Path((zone, record)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    requests
        .lock()
        .expect("the recorded requests")
        .push(("DELETE".into(), format!("/client/v4/zones/{zone}/dns_records/{record}")));
    let mut records = records.lock().expect("the records");
    let before = records.len();
    records.retain(|held| {
        held["id"].as_str() != Some(record.as_str()) || held["zone_id"].as_str() != Some(zone.as_str())
    });
    if records.len() == before {
        // Cloudflare's own shape for "that record is not here", which a retry
        // after a partial deletion has to be able to read as already-done
        // rather than as a new failure.
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({"success": false, "errors": [{"code": 81044, "message": "Record does not exist."}]})),
        )
            .into_response();
    }
    axum::Json(json!({"success": true, "errors": [], "result": {"id": record}})).into_response()
}

async fn get_record(
    axum::extract::State((records, requests)): axum::extract::State<ApiState>,
    axum::extract::Path((zone, record)): axum::extract::Path<(String, String)>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    requests
        .lock()
        .expect("the recorded requests")
        .push(("GET".into(), format!("/client/v4/zones/{zone}/dns_records/{record}")));
    let records = records.lock().expect("the records");
    match records.iter().find(|held| held["id"].as_str() == Some(record.as_str())) {
        Some(found) => {
            axum::Json(json!({"success": true, "errors": [], "result": found})).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({"success": false, "errors": [{"code": 81044, "message": "Record does not exist."}]})),
        )
            .into_response(),
    }
}
