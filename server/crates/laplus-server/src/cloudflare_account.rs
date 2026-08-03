//! Cloudflare browser authorization, and the tunnels an authorized account has.
//!
//! Two things here are deliberately narrow.
//!
//! **The account certificate is cloudflared's, not laplus's.** `cert.pem` can
//! create, list, route and delete every tunnel in the account and stays valid
//! for years, so laplus reads its *location*, never its contents, uses it in
//! place for one requested command, and never copies, moves, replaces or
//! deletes it — ADR-0045, and the certificate-lifecycle finding behind it.
//! Detecting one grants nothing: an existing certificate is used only after the
//! developer consents to that authority, which is what [`Account::consent`]
//! records. A sign-in laplus itself performed *is* that consent, because
//! nothing but a deliberate action starts it.
//!
//! **A tunnel listing says less than it looks like it does.** `tunnel list
//! --output json` carries ids, names, timestamps and connections — and no
//! hostname and no management mode. So this module classifies only what the
//! output supports: a tunnel with connections is *active* and therefore
//! externally managed, one without is merely *adoptable*, and the public
//! hostname is asked for and verified rather than inferred.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Notify;

const SETTINGS: &str = "account.json";
/// Long enough for a real browser round trip, bounded so an abandoned sign-in
/// cannot leave a child process waiting for the life of the server.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

/// What the developer is told before a detected certificate is used.
pub const CERTIFICATE_WARNING: &str =
    "The Cloudflare account certificate can create, list, route, and delete every tunnel in your \
     account, and stays valid for years. laplus uses it where cloudflared put it and never copies, \
     moves, replaces, or deletes it.";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    consented_at: Option<String>,
    tunnels: Vec<Tunnel>,
    listed_at: Option<String>,
    selection: Option<Selection>,
}

crate::public_exposure::closed_vocabulary! {
    /// Whether cloudflared reported any connection for a listed tunnel.
    ///
    /// The only evidence of activity `tunnel list --output json` carries, and
    /// therefore the only thing this module may claim about one.
    Activity as "tunnel activity" {
        Active => "active",
        Inactive => "inactive",
    }
}

crate::public_exposure::closed_vocabulary! {
    /// What laplus may *do* with a listed tunnel, and nothing more.
    ///
    /// Not a claim about who owns the Cloudflare allocation — that is
    /// [`crate::public_exposure::TunnelOwnership`], which is settled by adoption
    /// or creation rather than by a listing.
    Classification as "tunnel classification" {
        /// Someone else's connector is already serving it.
        External => "external",
        /// Inactive, so it *may* be dedicated to laplus — after the separate
        /// confirmation ADR-0045 requires, which ticket 05 builds.
        Adoptable => "adoptable",
    }
}

crate::public_exposure::closed_vocabulary! {
    /// Cloudflare browser authorization, as this server can observe it.
    LoginState as "login state" {
        NotStarted => "not-started",
        AwaitingBrowser => "awaiting-browser",
        Complete => "complete",
        Cancelled => "cancelled",
        TimedOut => "timed-out",
        Failed => "failed",
    }
}

crate::public_exposure::closed_vocabulary! {
    /// Which step of the account wizard an interrupted setup resumes at.
    ///
    /// Computed from what is durably true rather than remembered by the
    /// browser, which is what lets a reopened dialog, a reloaded page and a
    /// restarted server agree about how far setup got.
    ///
    /// **Ticket 05 adds `adopting` here and ticket 06 adds `creating`.** Adding
    /// one is now a compile error in `step` and a type error in the UI's
    /// `WIZARD_STEP_LABELS`, which is the point of the enum.
    SetupStep as "setup step" {
        SignIn => "sign-in",
        Consent => "consent",
        ChooseTunnel => "choose-tunnel",
        VerifyHostname => "verify-hostname",
        ConfirmAdoption => "confirm-adoption",
    }
}

/// One row of `tunnel list --output json`, reduced to what it actually proves.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub id: String,
    pub name: String,
    pub created_at: Option<String>,
    pub connection_count: usize,
    pub activity: Activity,
    pub classification: Classification,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub tunnel_id: String,
    pub name: String,
    pub classification: Classification,
    pub https_origin: String,
    /// Always false here. Adoption is a later, separate confirmation; until it
    /// succeeds an inactive tunnel is a choice and not a laplus-managed tunnel.
    pub adoption_confirmed: bool,
}

#[derive(Debug)]
struct Login {
    state: LoginState,
    authorization_url: Option<String>,
    failure: Option<String>,
    running: bool,
    /// Asked to stop. A flag as well as a notification because a cancel can
    /// arrive before the task that would hear it has started waiting, and
    /// `notify_waiters` only reaches whoever is already there — a lost wakeup
    /// would leave a browser sign-in running with nothing able to end it.
    cancelled: bool,
}

#[derive(Debug)]
pub struct Account {
    directory: PathBuf,
    settings: Mutex<Settings>,
    login: Mutex<Login>,
    cancel: Notify,
}

/// Every refusal this module can produce, in the shape the route needs.
///
/// **The type is `crate::public_exposure::Refusal`**, shared with the connector
/// manager and the routes. This module used to have its own two-variant enum
/// carrying only a sentence, which left `server.rs` recovering the contract's
/// reason by matching the prose — see [`RefusalReason`]'s own note.
pub use crate::public_exposure::{Refusal, RefusalReason};

/// The two sentences about a missing certificate, which three callers share.
fn sign_in_first() -> Refusal {
    Refusal::precondition(
        RefusalReason::SignInRequired,
        "No Cloudflare account certificate was found. Sign in first.",
    )
}

fn unusable_executable() -> Refusal {
    Refusal::rejected(
        RefusalReason::ExecutableUnusable,
        "The selected cloudflared executable cannot be started.",
    )
}

impl Account {
    /// A file that will not read starts the wizard over rather than stopping the
    /// server.
    ///
    /// **Everything in it is recoverable by asking again.** Consent is a
    /// question the developer can answer twice, a listing is a read at
    /// Cloudflare, and a selection is a choice — none of it is a credential and
    /// none of it can be reconstructed from anywhere else, so refusing to boot
    /// over a truncated write would cost a running server to protect a
    /// recomputable file. The certificate itself is cloudflared's and is
    /// untouched either way.
    pub fn open(cloudflare_directory: &Path) -> Arc<Account> {
        let settings: Settings = std::fs::read(cloudflare_directory.join(SETTINGS))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Arc::new(Account {
            directory: cloudflare_directory.to_path_buf(),
            settings: Mutex::new(settings),
            login: Mutex::new(Login {
                state: LoginState::NotStarted,
                authorization_url: None,
                failure: None,
                running: false,
                cancelled: false,
            }),
            cancel: Notify::new(),
        })
    }

    pub fn snapshot(&self) -> Value {
        let certificate = certificate_path();
        let detected = certificate.is_file();
        let settings = self.settings.lock().unwrap();
        let login = self.login.lock().unwrap();
        // A sign-in that was running when the server stopped is gone with it.
        // What is true after a restart is whether a certificate is there, which
        // is what makes the wizard resumable rather than stuck mid-step.
        let login_state = if login.state == LoginState::NotStarted && detected {
            LoginState::Complete
        } else {
            login.state
        };
        json!({
            "certificateDetected": detected,
            "certificatePath": certificate,
            "certificateConsentedAt": settings.consented_at,
            "certificateWarning": CERTIFICATE_WARNING,
            "loginState": login_state,
            "authorizationUrl": login.authorization_url,
            "failureMessage": login.failure,
            "tunnels": settings.tunnels,
            "listedAt": settings.listed_at,
            "selection": settings.selection,
            "step": step(detected, settings.consented_at.is_some(), settings.selection.as_ref()),
        })
    }

    /// Start cloudflared's browser authorization, or report the one already
    /// running. Repeating the command never starts a second sign-in.
    pub async fn begin_login(self: &Arc<Self>, executable: &Path) -> Result<(), Refusal> {
        let executable = crate::process::Search::from_environment()
            .startable(executable)
            .ok_or_else(unusable_executable)?;
        crate::cloudflare_connector::compatible_version(&executable).await?;
        {
            let mut login = self.login.lock().unwrap();
            if login.running {
                return Ok(());
            }
            login.running = true;
            login.cancelled = false;
            login.state = LoginState::AwaitingBrowser;
            login.authorization_url = None;
            login.failure = None;
        }
        let account = Arc::clone(self);
        tokio::spawn(async move { account.authorize(executable).await });
        Ok(())
    }

    async fn authorize(self: Arc<Self>, executable: PathBuf) {
        // No `--origincert`: where the certificate is written is cloudflared's
        // decision, and redirecting it would mean managing a file laplus does
        // not own.
        let mut command = tokio::process::Command::new(&executable);
        command
            .args(["tunnel", "login"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let Ok(mut child) = command.spawn() else {
            self.finish(LoginState::Failed, Some("cloudflared could not be started.".into()));
            return;
        };
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            let account = Arc::clone(&self);
            readers.push(tokio::spawn(async move { account.announce(stdout).await }));
        }
        if let Some(stderr) = child.stderr.take() {
            let account = Arc::clone(&self);
            readers.push(tokio::spawn(async move { account.announce(stderr).await }));
        }
        let outcome = tokio::select! {
            status = child.wait() => match status {
                Ok(status) if status.success() => None,
                Ok(_) => Some((LoginState::Failed, "Cloudflare authorization did not complete.")),
                Err(_) => Some((LoginState::Failed, "Cloudflare authorization could not be observed.")),
            },
            () = self.cancellation() => Some((LoginState::Cancelled, "Cloudflare authorization was cancelled.")),
            () = tokio::time::sleep(LOGIN_TIMEOUT) => {
                Some((LoginState::TimedOut, "Cloudflare authorization timed out. Start it again when you are ready."))
            }
        };
        let _ = child.kill().await;
        for reader in readers {
            let _ = reader.await;
        }
        match outcome {
            None if certificate_path().is_file() => {
                // laplus ran this sign-in because the developer asked it to, so
                // the authority it granted was chosen rather than merely found.
                let consented = crate::clock::now_iso();
                let settings = {
                    let mut settings = self.settings.lock().unwrap();
                    settings.consented_at = Some(consented);
                    settings.clone()
                };
                let _ = self.persist(&settings);
                self.finish(LoginState::Complete, None);
            }
            None => self.finish(
                LoginState::Failed,
                Some("Cloudflare authorization finished without writing a certificate.".into()),
            ),
            Some((state, message)) => self.finish(state, Some(message.into())),
        }
    }

    /// cloudflared prints the authorization URL on whichever stream it likes,
    /// so both are read the same way.
    async fn announce(self: Arc<Self>, stream: impl tokio::io::AsyncRead + Unpin) {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = authorization_url(&line) {
                let mut login = self.login.lock().unwrap();
                if login.authorization_url.is_none() {
                    login.authorization_url = Some(url);
                }
            }
        }
    }

    /// Resolves once this sign-in has been asked to stop.
    ///
    /// Polled as well as notified: the flag is the truth and the notification
    /// is only how this wakes promptly, so a cancel that landed before this
    /// future existed is still seen.
    async fn cancellation(&self) {
        loop {
            if self.login.lock().unwrap().cancelled {
                return;
            }
            tokio::select! {
                () = self.cancel.notified() => {}
                () = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }

    fn finish(&self, state: LoginState, failure: Option<String>) {
        let mut login = self.login.lock().unwrap();
        login.running = false;
        login.state = state;
        login.failure = failure;
        if !matches!(state, LoginState::AwaitingBrowser | LoginState::Complete) {
            login.authorization_url = None;
        }
    }

    pub fn cancel_login(&self) -> Result<(), Refusal> {
        {
            let mut login = self.login.lock().unwrap();
            if !login.running {
                return Err(Refusal::precondition(
                    RefusalReason::NothingRunning,
                    "No Cloudflare authorization is running.",
                ));
            }
            login.cancelled = true;
        }
        self.cancel.notify_waiters();
        Ok(())
    }

    /// Record — or withdraw — consent to use the certificate cloudflared owns.
    pub fn consent(&self, consented: bool) -> Result<(), Refusal> {
        if consented && !certificate_path().is_file() {
            return Err(sign_in_first());
        }
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            settings.consented_at = consented.then(crate::clock::now_iso);
            if !consented {
                // Everything downstream of the certificate goes with it. The
                // listing was read *using* the certificate and the selection was
                // read out of the listing, so keeping either would let a later
                // re-consent resume at a tunnel nothing had re-listed — the
                // wizard would skip to `verify-hostname` for a choice made under
                // an authority that had since been withdrawn.
                settings.tunnels.clear();
                settings.listed_at = None;
                settings.selection = None;
            }
            settings.clone()
        };
        self.persist(&settings)
    }

    /// List the account's tunnels, using the certificate where it already is.
    ///
    /// Read-only at Cloudflare: repeating it reconciles what laplus knows and
    /// mutates nothing, which is what makes an interrupted discovery safe to
    /// simply run again.
    pub async fn list_tunnels(&self, executable: &Path) -> Result<(), Refusal> {
        let certificate = certificate_path();
        if !certificate.is_file() {
            return Err(sign_in_first());
        }
        if self.settings.lock().unwrap().consented_at.is_none() {
            return Err(Refusal::precondition(
                RefusalReason::ConsentRequired,
                format!(
                    "Confirm that laplus may use the Cloudflare account certificate. \
                     {CERTIFICATE_WARNING}"
                ),
            ));
        }
        let executable = crate::process::Search::from_environment()
            .startable(executable)
            .ok_or_else(unusable_executable)?;
        crate::cloudflare_connector::compatible_version(&executable).await?;
        let output = tokio::process::Command::new(&executable)
            .args([
                "tunnel",
                "--origincert",
                &certificate.to_string_lossy(),
                "list",
                "--output",
                "json",
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| {
                Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not list the account's tunnels.",
                )
            })?;
        if !output.status.success() {
            return Err(Refusal::rejected(
                RefusalReason::CommandFailed,
                "cloudflared could not list the account's tunnels. Sign in again if the \
                 certificate has expired.",
            ));
        }
        let tunnels = parse_tunnels(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
            Refusal::rejected(
                RefusalReason::CommandFailed,
                "cloudflared listed the tunnels in a shape laplus cannot read.",
            )
        })?;
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            settings.selection = settings
                .selection
                .take()
                .filter(|selection| tunnels.iter().any(|tunnel| tunnel.id == selection.tunnel_id));
            settings.tunnels = tunnels;
            settings.listed_at = Some(crate::clock::now_iso());
            settings.clone()
        };
        self.persist(&settings)
    }

    /// Choose a listed tunnel and say which hostname reaches it.
    ///
    /// The hostname is the developer's answer, never anything read out of the
    /// listing — the listing does not carry one. An active tunnel is classified
    /// external here and takes no laplus lifecycle action; an inactive one is
    /// only a candidate until adoption is separately confirmed.
    pub fn select(&self, tunnel_id: &str, hostname: &str) -> Result<Selection, Refusal> {
        let https_origin = crate::public_exposure::normalize_hostname(hostname)
            .map_err(|message| Refusal::rejected(RefusalReason::HostnameInvalid, message))?;
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            let tunnel = settings
                .tunnels
                .iter()
                .find(|tunnel| tunnel.id == tunnel_id)
                .ok_or_else(|| {
                    Refusal::precondition(
                        RefusalReason::SelectionStale,
                        "That tunnel is not in the current listing. Refresh and choose again.",
                    )
                })?
                .clone();
            settings.selection = Some(Selection {
                tunnel_id: tunnel.id,
                name: tunnel.name,
                classification: tunnel.classification,
                https_origin,
                adoption_confirmed: false,
            });
            settings.clone()
        };
        self.persist(&settings)?;
        Ok(settings.selection.expect("a selection was just recorded"))
    }

    /// Write the wizard's own state down.
    ///
    /// Everything that can go wrong here is local — a directory that will not
    /// open, a file that will not write — so it is `LocalSetupFailed` rather
    /// than `CommandFailed`: nothing at Cloudflare went wrong and the retry is
    /// on this machine.
    fn persist(&self, settings: &Settings) -> Result<(), Refusal> {
        crate::cloudflare_connector::private_directory(&self.directory)?;
        let bytes = serde_json::to_vec_pretty(settings).map_err(|_| {
            Refusal::rejected(
                RefusalReason::LocalSetupFailed,
                "Cloudflare account settings could not be encoded.",
            )
        })?;
        crate::cloudflare_connector::private_write(&self.directory.join(SETTINGS), &bytes)
    }
}

/// Where the account certificate is, by cloudflared's own rules.
///
/// `TUNNEL_ORIGIN_CERT` is cloudflared's documented override and is honoured
/// for the same reason `--origincert` is: a developer who moved the certificate
/// told cloudflared, not laplus.
pub fn certificate_path() -> PathBuf {
    if let Some(configured) = std::env::var("TUNNEL_ORIGIN_CERT")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return PathBuf::from(configured);
    }
    home_directory().join(".cloudflared").join("cert.pem")
}

fn home_directory() -> PathBuf {
    for variable in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var(variable)
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(".")
}

/// The one thing worth reading out of cloudflared's sign-in chatter.
fn authorization_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..]
        .chars()
        .take_while(|character| !character.is_whitespace())
        .collect();
    (url.len() > "https://".len()).then_some(url)
}

/// `tunnel list --output json`, read for what it proves and nothing more.
///
/// A deleted tunnel is not a choice, and a `connections` array is the only
/// evidence of activity the output carries.
pub fn parse_tunnels(output: &str) -> Option<Vec<Tunnel>> {
    #[derive(Deserialize)]
    struct Listed {
        id: String,
        name: String,
        created_at: Option<String>,
        deleted_at: Option<String>,
        connections: Option<Vec<Value>>,
    }
    let listed: Vec<Listed> = serde_json::from_str(output.trim()).ok()?;
    Some(
        listed
            .into_iter()
            .filter(|tunnel| tunnel.deleted_at.is_none())
            .map(|tunnel| {
                let connection_count = tunnel.connections.map(|value| value.len()).unwrap_or(0);
                Tunnel {
                    id: tunnel.id,
                    name: tunnel.name,
                    created_at: tunnel.created_at,
                    connection_count,
                    activity: if connection_count > 0 {
                        Activity::Active
                    } else {
                        Activity::Inactive
                    },
                    classification: if connection_count > 0 {
                        Classification::External
                    } else {
                        Classification::Adoptable
                    },
                }
            })
            .collect(),
    )
}

/// Which step of the wizard an interrupted setup resumes at.
fn step(certificate: bool, consented: bool, selection: Option<&Selection>) -> SetupStep {
    if !certificate {
        return SetupStep::SignIn;
    }
    if !consented {
        return SetupStep::Consent;
    }
    match selection.map(|selection| selection.classification) {
        None => SetupStep::ChooseTunnel,
        Some(Classification::External) => SetupStep::VerifyHostname,
        Some(Classification::Adoptable) => SetupStep::ConfirmAdoption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_is_read_from_connections_and_deleted_tunnels_are_not_choices() {
        let tunnels = parse_tunnels(
            r#"[
                {"id":"aaa","name":"live","created_at":"2026-01-01T00:00:00Z","deleted_at":null,
                 "connections":[{"id":"c1"},{"id":"c2"}]},
                {"id":"bbb","name":"idle","created_at":"2026-02-02T00:00:00Z","deleted_at":null,
                 "connections":[]},
                {"id":"ccc","name":"gone","created_at":"2026-03-03T00:00:00Z",
                 "deleted_at":"2026-04-04T00:00:00Z","connections":[]}
            ]"#,
        )
        .expect("cloudflared's own list shape");

        assert_eq!(tunnels.len(), 2);
        assert_eq!(tunnels[0].activity, Activity::Active);
        assert_eq!(tunnels[0].classification, Classification::External);
        assert_eq!(tunnels[0].connection_count, 2);
        assert_eq!(tunnels[1].activity, Activity::Inactive);
        assert_eq!(tunnels[1].classification, Classification::Adoptable);
        // Nothing in the output says where a tunnel is reachable, so nothing
        // here may claim to.
        let encoded = serde_json::to_string(&tunnels).unwrap();
        assert!(!encoded.contains("hostname"));
        assert!(!encoded.contains("managementMode"));
    }

    #[test]
    fn a_listing_that_is_not_the_documented_shape_is_refused() {
        assert!(parse_tunnels("not json").is_none());
        assert!(parse_tunnels("{}").is_none());
        assert_eq!(parse_tunnels("[]"), Some(Vec::new()));
    }

    #[test]
    fn the_authorization_url_is_taken_from_whatever_cloudflared_printed() {
        assert_eq!(
            authorization_url(
                "Please open the following URL: https://dash.cloudflare.com/argotunnel?callback=x "
            )
            .as_deref(),
            Some("https://dash.cloudflare.com/argotunnel?callback=x")
        );
        assert_eq!(authorization_url("Waiting for login..."), None);
    }

    #[test]
    fn an_interrupted_setup_resumes_at_the_step_it_reached() {
        assert_eq!(step(false, false, None), SetupStep::SignIn);
        assert_eq!(step(true, false, None), SetupStep::Consent);
        assert_eq!(step(true, true, None), SetupStep::ChooseTunnel);
        let external = Selection {
            tunnel_id: "aaa".into(),
            name: "live".into(),
            classification: Classification::External,
            https_origin: "https://laplus.example.com".into(),
            adoption_confirmed: false,
        };
        assert_eq!(step(true, true, Some(&external)), SetupStep::VerifyHostname);
        let adoptable = Selection {
            classification: Classification::Adoptable,
            ..external
        };
        assert_eq!(step(true, true, Some(&adoptable)), SetupStep::ConfirmAdoption);
    }
}
