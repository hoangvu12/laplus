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
    /// Adding one is a compile error in `step` and a type error in the UI's
    /// `WIZARD_STEP_LABELS`, which is the point of the enum.
    SetupStep as "setup step" {
        SignIn => "sign-in",
        Consent => "consent",
        ChooseTunnel => "choose-tunnel",
        VerifyHostname => "verify-hostname",
        ConfirmAdoption => "confirm-adoption",
        /// Dedication is confirmed: laplus holds the tunnel's run credential,
        /// wrote its own configuration, and is bringing the connector up. The
        /// step an adopted setup resumes at from here on, because there is
        /// nothing left to ask and everything left to watch.
        Adopting => "adopting",
        /// laplus allocated the tunnel, routed the DNS name to it and wrote its
        /// own configuration, and is bringing the connector up. The creation
        /// twin of `adopting`, and separate from it because the two differ in
        /// the one thing the screen after them has to say: only this one's
        /// Cloudflare resources are laplus's to delete.
        ///
        /// **Not the screen that asks.** The name and hostname a creation is
        /// confirmed against are answers nothing has recorded yet, so the offer
        /// is the client's own step; this is what is true once the mutations
        /// have happened.
        Creating => "creating",
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

/// Which tunnel this environment's setup is about, and how far laplus has gone.
///
/// **Two ways in, and never both.** A tunnel is either chosen out of the account
/// listing — in which case `select` writes this and dedication may later confirm
/// it — or made by laplus, in which case creation writes it and `created` is the
/// only flag that is ever true. They are two booleans rather than one word
/// because `adoptionConfirmed` was already on the wire before creation existed
/// and a client that reads it must go on getting the answer it has always got:
/// a created tunnel was never adopted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub tunnel_id: String,
    pub name: String,
    pub classification: Classification,
    pub https_origin: String,
    /// Set only by [`Account::confirm_adoption`]. False when a tunnel is merely
    /// chosen: adoption is a later, separate confirmation, and until it succeeds
    /// an inactive tunnel is a choice and not a laplus-managed tunnel.
    pub adoption_confirmed: bool,
    /// Set only by [`Account::confirm_creation`], which is the only thing that
    /// can be true of a tunnel this environment allocated. Defaulted on read so
    /// that an `account.json` written before creation existed still parses —
    /// the reason ticket 05 gave for the same care over `RunCredential`.
    #[serde(default)]
    pub created: bool,
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

/// A `tunnel create` that did not finish, and the resource it may have left.
///
/// **The identifier outlives the credential.** cloudflared can allocate a tunnel
/// and still leave laplus without a usable `<UUID>.json` — it can exit non-zero
/// after writing a truncated one, or exit zero having written one for a
/// different tunnel — and in both cases laplus removes the file, because a
/// resume that trusted it would configure a connector against garbage. What must
/// *not* go with the file is the UUID cloudflared reported: it is the only name
/// anyone has for a tunnel that may exist at Cloudflare and cannot be run from
/// here, so it is journaled rather than discarded. `docs/adr/0051`.
#[derive(Debug)]
pub struct FailedAllocation {
    pub refusal: Refusal,
    pub tunnel_id: Option<String>,
}

impl FailedAllocation {
    /// A refusal from before the command ran, so nothing was allocated.
    fn without_a_tunnel(refusal: Refusal) -> Self {
        Self { refusal, tunnel_id: None }
    }
}

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

    /// A startable, compatible cloudflared and the certificate it may spend.
    ///
    /// Every account-management command needs the same four answers — there is
    /// a certificate, its authority was accepted, the named executable runs,
    /// and it is new enough — and each of them is a different refusal. Asked
    /// once here so that adoption cannot answer them differently from listing,
    /// which is exactly the sort of drift a second copy produces.
    async fn consented_command(&self, executable: &Path) -> Result<(PathBuf, PathBuf), Refusal> {
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
        Ok((executable, certificate))
    }

    /// What the developer chose, if anything.
    pub fn selection(&self) -> Option<Selection> {
        self.settings.lock().unwrap().selection.clone()
    }

    /// List the account's tunnels, using the certificate where it already is.
    ///
    /// Read-only at Cloudflare: repeating it reconciles what laplus knows and
    /// mutates nothing, which is what makes an interrupted discovery safe to
    /// simply run again.
    pub async fn list_tunnels(&self, executable: &Path) -> Result<(), Refusal> {
        let (executable, certificate) = self.consented_command(executable).await?;
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
                created: false,
            });
            settings.clone()
        };
        self.persist(&settings)?;
        Ok(settings.selection.expect("a selection was just recorded"))
    }

    /// Re-read the selected tunnel's activity from Cloudflare, right now.
    ///
    /// **The listing that produced the offer is evidence about the past.** A
    /// connector can be started between the moment a developer is shown "no
    /// connector is serving it" and the moment they press the button, and
    /// ADR-0045 makes an active tunnel externally managed — so the answer laplus
    /// acts on has to be re-read immediately before the first mutation rather
    /// than carried from the screen. `list` mutates nothing at Cloudflare, so
    /// asking again costs a read and buys the race.
    pub async fn recheck_activity(
        &self,
        executable: &Path,
        tunnel_id: &str,
    ) -> Result<Activity, Refusal> {
        self.list_tunnels(executable).await?;
        self.settings
            .lock()
            .unwrap()
            .tunnels
            .iter()
            .find(|tunnel| tunnel.id == tunnel_id)
            .map(|tunnel| tunnel.activity)
            .ok_or_else(|| {
                Refusal::precondition(
                    RefusalReason::SelectionStale,
                    "That tunnel is no longer in the account's listing. Refresh and choose again.",
                )
            })
    }

    /// Record that the selected tunnel turned out to be somebody else's after
    /// all, and keep the hostname the developer supplied.
    ///
    /// The fallback ADR-0045 requires: an active tunnel is an external tunnel
    /// endpoint, which laplus verifies and advertises and never operates. The
    /// choice survives so that the wizard lands on `verify-hostname` rather than
    /// throwing away an answer that is still true.
    pub fn reclassify_as_external(&self) -> Result<(), Refusal> {
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            if let Some(selection) = settings.selection.as_mut() {
                selection.classification = Classification::External;
                selection.adoption_confirmed = false;
            }
            settings.clone()
        };
        self.persist(&settings)
    }

    /// Retrieve the narrow run credential for one existing tunnel.
    ///
    /// **`token --cred-file`, not `create`.** An adopted tunnel already exists,
    /// so laplus fetches the `<UUID>.json` that runs it rather than allocating
    /// anything — the whole difference between adoption and creation at
    /// Cloudflare. The credential is written by cloudflared straight into
    /// laplus's private directory and never passes through a laplus argument,
    /// a log, a snapshot or an error: what crosses this boundary is a path.
    ///
    /// Idempotent by observation. A credential already on disk *for this tunnel*
    /// is the mutation having already happened, which is what lets an
    /// interrupted adoption resume without spending the account certificate a
    /// second time.
    pub async fn retrieve_tunnel_credential(
        &self,
        executable: &Path,
        tunnel_id: &str,
        credential_file: &Path,
    ) -> Result<(), Refusal> {
        if credential_for(credential_file, tunnel_id) {
            return Ok(());
        }
        let (executable, certificate) = self.consented_command(executable).await?;
        if let Some(parent) = credential_file.parent() {
            crate::cloudflare_connector::private_directory(parent)?;
        }
        let output = tokio::process::Command::new(&executable)
            .args([
                "tunnel",
                "--origincert",
                &certificate.to_string_lossy(),
                "token",
                "--cred-file",
                &credential_file.to_string_lossy(),
                tunnel_id,
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| {
                Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not retrieve the tunnel credential.",
                )
            })?;
        if !output.status.success() || !credential_for(credential_file, tunnel_id) {
            // **Take the wreckage with it.** A `token` that failed after
            // creating the file leaves something that is not a usable
            // credential, and the resume above decides by looking — so a
            // half-written file left here would make the *next* attempt skip
            // the retrieval and configure a connector against garbage.
            let _ = std::fs::remove_file(credential_file);
            return Err(Refusal::rejected(
                RefusalReason::CommandFailed,
                "cloudflared could not retrieve a run credential for that tunnel. Tunnels created \
                 before cloudflared 2022.3.0 cannot supply one.",
            ));
        }
        // cloudflared's own umask decides the mode it wrote with, and this file
        // is the whole of the authority to run the tunnel.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(credential_file, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| {
                    Refusal::rejected(
                        RefusalReason::LocalSetupFailed,
                        "The tunnel credential could not be protected.",
                    )
                })?;
        }
        Ok(())
    }

    /// Allocate a new tunnel, writing its narrow run credential into laplus's
    /// own private directory.
    ///
    /// **`create --credentials-file`, not `token --cred-file`.** A created
    /// tunnel does not exist until this runs, so the same command that allocates
    /// it is the one that writes the `<UUID>.json` that will run it — which is
    /// why creation never asks Cloudflare for a credential separately, and why
    /// there is no `Credential` step in a creation's journal. `--credentials-file`
    /// is what keeps that file out of cloudflared's default directory and inside
    /// laplus's, and `--output json` is what makes the allocated UUID readable
    /// rather than scraped from a sentence.
    ///
    /// **The name is asked for and the UUID is allocated.** They are different
    /// things and cleanup targets the second, so the id is returned rather than
    /// the caller being left to assume the two are related.
    ///
    /// **Not idempotent on its own.** A credential already on disk is an
    /// allocation that already happened — but only the *caller* can say whether
    /// it happened as part of this creation, because an adopted tunnel's
    /// credential lives at the same path and reusing one would turn a tunnel
    /// laplus merely borrowed into a tunnel laplus claims to have made. The
    /// route answers that from the endpoint row and the journal before calling
    /// this; see `create_cloudflare_tunnel`.
    pub async fn create_tunnel(
        &self,
        executable: &Path,
        name: &str,
        credential_file: &Path,
    ) -> Result<String, FailedAllocation> {
        let name = match normalize_tunnel_name(name) {
            Ok(name) => name,
            Err(refusal) => return Err(FailedAllocation::without_a_tunnel(refusal)),
        };
        let (executable, certificate) = self
            .consented_command(executable)
            .await
            .map_err(FailedAllocation::without_a_tunnel)?;
        if let Some(parent) = credential_file.parent() {
            crate::cloudflare_connector::private_directory(parent)
                .map_err(FailedAllocation::without_a_tunnel)?;
        }
        let output = tokio::process::Command::new(&executable)
            .args([
                "tunnel",
                "--origincert",
                &certificate.to_string_lossy(),
                "create",
                "--credentials-file",
                &credential_file.to_string_lossy(),
                "--output",
                "json",
                &name,
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| {
                FailedAllocation::without_a_tunnel(Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not create the tunnel.",
                ))
            })?;
        let allocated = created_tunnel_id(&String::from_utf8_lossy(&output.stdout));
        if !output.status.success() {
            // **Take the wreckage with it**, for the reason retrieval does: the
            // resume decides by looking, and a truncated `<UUID>.json` left here
            // would make the next attempt skip an allocation it still needs and
            // then configure a connector against garbage.
            let _ = std::fs::remove_file(credential_file);
            return Err(FailedAllocation {
                tunnel_id: allocated,
                refusal: Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not create the tunnel. A tunnel with that name may already \
                     exist in this Cloudflare account.",
                ),
            });
        }
        let Some(held) = credential_tunnel_id(credential_file).filter(|held| {
            // What the command *said* and what it *wrote* have to be the same
            // tunnel, because the connector runs on the file and cleanup targets
            // the id. A disagreement is a credential laplus cannot vouch for.
            allocated.as_ref().is_none_or(|allocated| allocated == held)
        }) else {
            // The credential goes, and the *identifier does not*: cloudflared
            // exited successfully, so a tunnel may well exist at Cloudflare with
            // nothing here able to run it. That id is the only name a cleanup
            // would have for it, so it is carried out to the journal rather than
            // thrown away with the file.
            let _ = std::fs::remove_file(credential_file);
            return Err(FailedAllocation {
                tunnel_id: allocated,
                refusal: Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared created a tunnel but did not write a run credential laplus can \
                     use. A tunnel of that name may now exist in your Cloudflare account.",
                ),
            });
        };
        // cloudflared's own umask decides the mode it wrote with, and this file
        // is the whole of the authority to run the tunnel.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(credential_file, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| FailedAllocation {
                    tunnel_id: Some(held.clone()),
                    refusal: Refusal::rejected(
                        RefusalReason::LocalSetupFailed,
                        "The tunnel credential could not be protected.",
                    ),
                })?;
        }
        Ok(held)
    }

    /// Route a DNS name to a tunnel laplus created.
    ///
    /// **No `--overwrite-dns`.** The flag would let laplus take a hostname that
    /// already answers for something else, which is somebody's DNS zone rather
    /// than laplus's to reassign — and creation is the one path that is supposed
    /// to leave everything it did not make alone. A name already in use is
    /// refused by Cloudflare and reported as such.
    ///
    /// **The CLI reports no identifiers.** It prints `Added CNAME <hostname>
    /// which will route to this tunnel` and stops, so the name is the whole of
    /// what laplus may write down about the record it made — see
    /// [`crate::store::DnsRecord`] and `docs/adr/0051`.
    pub async fn route_dns(
        &self,
        executable: &Path,
        tunnel_id: &str,
        hostname: &str,
    ) -> Result<(), Refusal> {
        let (executable, certificate) = self.consented_command(executable).await?;
        let output = tokio::process::Command::new(&executable)
            .args([
                "tunnel",
                "--origincert",
                &certificate.to_string_lossy(),
                "route",
                "dns",
                tunnel_id,
                hostname,
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| {
                Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not create the DNS route.",
                )
            })?;
        if !output.status.success() {
            return Err(Refusal::rejected(
                RefusalReason::CommandFailed,
                "cloudflared could not route that hostname to the tunnel. A DNS record for it may \
                 already exist, or the hostname may be outside this Cloudflare account's zones.",
            ));
        }
        Ok(())
    }

    /// Delete a tunnel laplus created, by the UUID Cloudflare allocated.
    ///
    /// **By id, and never by name.** The name is a label the account can hold
    /// twice and a developer can retype; the UUID is the resource, and it is
    /// what the endpoint row and the journal both record for exactly this
    /// moment. Whether laplus is *allowed* to run this at all is decided from
    /// the recorded ownership before the call — see `delete_cloudflare_tunnel`
    /// in `server.rs`, which is where ADR-0049's rule lives.
    ///
    /// **No `--cascade` and no force.** A tunnel that still has connections is a
    /// tunnel something is still serving, and cloudflared refusing to delete it
    /// is the answer laplus wants: the connector is stopped first, and a refusal
    /// after that means somebody else's replica is running and the developer has
    /// to decide, not laplus.
    ///
    /// The DNS record is *not* removed by this, and cannot be: `cloudflared` has
    /// no `route dns delete`. See [`crate::cloudflare_dns`].
    pub async fn delete_tunnel(&self, executable: &Path, tunnel_id: &str) -> Result<(), Refusal> {
        let (executable, certificate) = self.consented_command(executable).await?;
        let output = tokio::process::Command::new(&executable)
            .args([
                "tunnel",
                "--origincert",
                &certificate.to_string_lossy(),
                "delete",
                tunnel_id,
            ])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|_| {
                Refusal::rejected(
                    RefusalReason::CommandFailed,
                    "cloudflared could not delete the tunnel.",
                )
            })?;
        if !output.status.success() {
            return Err(Refusal::rejected(
                RefusalReason::CommandFailed,
                "cloudflared could not delete that tunnel. A connector may still be serving it, \
                 or the account certificate may no longer have authority over it.",
            ));
        }
        Ok(())
    }

    /// Record the tunnel laplus created, and that it created it.
    ///
    /// Written last, after the tunnel, the route and the configuration exist,
    /// for the reason [`Account::confirm_adoption`] is: it is what [`step`]
    /// reads to say the wizard has moved past the offer, and a setup claiming to
    /// have moved on from work it had not done is what the journal exists to
    /// prevent.
    pub fn confirm_creation(
        &self,
        tunnel_id: &str,
        name: &str,
        https_origin: &str,
    ) -> Result<(), Refusal> {
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            settings.selection = Some(Selection {
                tunnel_id: tunnel_id.to_string(),
                name: name.to_string(),
                // Inactive at Cloudflare until laplus's own connector reaches
                // it, which is the only honest reading of a listing this tunnel
                // was never in. Nothing branches on it for a created tunnel:
                // `created` is answered first by `step`.
                classification: Classification::Adoptable,
                https_origin: https_origin.to_string(),
                adoption_confirmed: false,
                created: true,
            });
            settings.clone()
        };
        self.persist(&settings)
    }

    /// Record that dedication was confirmed and completed.
    ///
    /// Written last, after the credential and the configuration exist, because
    /// it is what [`step`] reads to say the wizard has moved past the offer —
    /// and a setup that claimed to have moved on from work it had not done is
    /// the thing the journal exists to prevent.
    pub fn confirm_adoption(&self) -> Result<(), Refusal> {
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            let Some(selection) = settings.selection.as_mut() else {
                return Err(Refusal::precondition(
                    RefusalReason::SelectionStale,
                    "Choose a tunnel before dedicating one.",
                ));
            };
            selection.adoption_confirmed = true;
            settings.clone()
        };
        self.persist(&settings)
    }

    /// Forget which tunnel this environment's setup was about.
    ///
    /// **The selection is what [`step`] resumes from**, so a forget or a delete
    /// that removed the endpoint row and left this behind would put the wizard
    /// back on `adopting` or `creating` for a setup that no longer exists — a
    /// screen naming a connector nothing is running and a tunnel nothing
    /// records.
    ///
    /// Consent and the listing survive on purpose. Consent is authority over the
    /// account certificate and is not what was removed; the listing is a read
    /// that costs a Cloudflare round trip, and keeping it means setting up again
    /// starts at the tunnel list rather than at a refresh button. Neither says
    /// anything about a tunnel laplus owns.
    pub fn forget_selection(&self) -> Result<(), Refusal> {
        let settings = {
            let mut settings = self.settings.lock().unwrap();
            settings.selection = None;
            settings.clone()
        };
        self.persist(&settings)
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

/// Which tunnel the run credential at `path` is for, if it is one at all.
///
/// **Existence is not the question a resume should ask.** An interrupted
/// adoption or creation decides whether to spend the account certificate again
/// by looking at what is on disk, and a file that exists but is truncated,
/// empty, or left over from a different tunnel would make it skip a mutation it
/// still needs — and then configure a connector that can never authenticate.
/// `TunnelID` is cloudflared's own field name in the `<UUID>.json` credential it
/// writes, and it is the only thing in there laplus reads.
///
/// The id rather than a yes/no because creation does not know one to compare
/// against: `tunnel create` is asked for a name and answers with a UUID, so what
/// a resumed creation needs from the file is *which* tunnel it already made.
pub fn credential_tunnel_id(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|held| held.get("TunnelID")?.as_str().map(str::to_string))
}

/// Whether the file at `path` is a usable run credential for `tunnel_id`.
pub fn credential_for(path: &Path, tunnel_id: &str) -> bool {
    credential_tunnel_id(path).is_some_and(|held| held == tunnel_id)
}

/// The UUID `tunnel create --output json` says it allocated.
///
/// Read from the structured output rather than from the sentence the plain form
/// prints, and treated as advisory: the credential file is what the connector
/// actually runs on, so a disagreement between the two is a refusal rather than
/// a preference. See [`Account::create_tunnel`].
fn created_tunnel_id(output: &str) -> Option<String> {
    serde_json::from_str::<Value>(output.trim())
        .ok()?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

/// A name Cloudflare will accept for a tunnel, or the refusal saying why not.
///
/// Checked here rather than left to `cloudflared`, because this is one of the
/// two questions a creation asks the developer and a rejection that arrives as
/// "cloudflared said no" does not say which field to fix. Deliberately narrow:
/// letters, digits, dash, dot and underscore are what Cloudflare's own tunnel
/// names use, and anything else — a slash above all — would go into a URL path
/// laplus builds no request from but a person reads as a resource it is not.
pub fn normalize_tunnel_name(name: &str) -> Result<String, Refusal> {
    let candidate = name.trim();
    let refused = |why: &str| {
        Refusal::rejected(RefusalReason::TunnelNameInvalid, format!("Enter a tunnel name {why}."))
    };
    if candidate.is_empty() {
        return Err(refused("for laplus to create"));
    }
    if candidate.chars().count() > 64 {
        return Err(refused("of 64 characters or fewer"));
    }
    if !candidate
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-._".contains(character))
    {
        return Err(refused("using letters, digits, dashes, dots and underscores only"));
    }
    Ok(candidate.to_string())
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
    match selection {
        None => SetupStep::ChooseTunnel,
        // Answered before classification, because a tunnel laplus made was never
        // in a listing and has no classification worth branching on — and a
        // created tunnel that fell through to `confirm-adoption` would re-offer
        // the dedication of a tunnel this environment already owns outright.
        Some(selection) if selection.created => SetupStep::Creating,
        Some(selection) => match selection.classification {
            Classification::External => SetupStep::VerifyHostname,
            Classification::Adoptable if selection.adoption_confirmed => SetupStep::Adopting,
            Classification::Adoptable => SetupStep::ConfirmAdoption,
        },
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
            created: false,
        };
        assert_eq!(step(true, true, Some(&external)), SetupStep::VerifyHostname);
        let adoptable = Selection {
            classification: Classification::Adoptable,
            ..external.clone()
        };
        assert_eq!(step(true, true, Some(&adoptable)), SetupStep::ConfirmAdoption);
        assert_eq!(
            step(true, true, Some(&Selection { adoption_confirmed: true, ..adoptable.clone() })),
            SetupStep::Adopting
        );
        // A tunnel laplus made is answered before classification. Falling
        // through to the adoptable arm would re-offer the dedication of a tunnel
        // this environment already owns outright.
        assert_eq!(
            step(true, true, Some(&Selection { created: true, ..adoptable })),
            SetupStep::Creating
        );
        assert_eq!(
            step(true, true, Some(&Selection { created: true, ..external })),
            SetupStep::Creating
        );
    }

    /// An `account.json` written before creation existed still reads, and reads
    /// as a tunnel laplus did not create.
    ///
    /// The same care ticket 05 took over `RunCredential`, and for the same
    /// reason: the wizard resumes from this file, and a shape a new build cannot
    /// parse restarts a setup that was finished.
    #[test]
    fn a_selection_recorded_before_creation_existed_still_reads_as_not_created() {
        let held = serde_json::json!({
            "tunnelId": "22222222-2222-2222-2222-222222222222",
            "name": "spare",
            "classification": "adoptable",
            "httpsOrigin": "https://spare.example.com",
            "adoptionConfirmed": true,
        });

        let selection: Selection = serde_json::from_value(held).expect("reads");
        assert!(selection.adoption_confirmed);
        assert!(!selection.created);
        assert_eq!(step(true, true, Some(&selection)), SetupStep::Adopting);
    }

    /// Two questions, two answers. A creation asks what to call the tunnel and
    /// where it answers, and one refusal for both leaves a developer guessing.
    #[test]
    fn a_tunnel_name_is_validated_separately_from_the_hostname() {
        assert_eq!(normalize_tunnel_name("  laplus-workstation  ").unwrap(), "laplus-workstation");
        assert_eq!(
            normalize_tunnel_name("laplus.work_station-1").unwrap(),
            "laplus.work_station-1"
        );
        for refused in ["", "   ", "laplus/workstation", "laplus workstation", &"n".repeat(65)] {
            let failure = normalize_tunnel_name(refused).expect_err(refused);
            assert_eq!(failure.reason, RefusalReason::TunnelNameInvalid, "{refused}");
            assert_eq!(failure.kind, crate::public_exposure::RefusalKind::Rejected);
        }
    }

    /// `tunnel create` is asked for a name and answers with a UUID, and cleanup
    /// targets the UUID — so the two are never assumed to be related.
    #[test]
    fn the_allocated_tunnel_is_read_from_the_structured_output_and_from_the_credential() {
        assert_eq!(
            created_tunnel_id(
                r#"{"id":"44444444-4444-4444-4444-444444444444","name":"laplus-workstation"}"#
            )
            .as_deref(),
            Some("44444444-4444-4444-4444-444444444444")
        );
        assert_eq!(created_tunnel_id("Created tunnel laplus with id 4444"), None);

        let directory = tempfile::tempdir().expect("a directory");
        let credential = directory.path().join("tunnel.json");
        assert_eq!(credential_tunnel_id(&credential), None);
        std::fs::write(&credential, r#"{"AccountTag":"a","TunnelID":"4444","TunnelSecret":"s"}"#)
            .unwrap();
        assert_eq!(credential_tunnel_id(&credential).as_deref(), Some("4444"));
        assert!(credential_for(&credential, "4444"));
        assert!(!credential_for(&credential, "2222"));
        // A file that exists and is not a credential is not a mutation that
        // happened, which is the whole reason a resume reads it rather than
        // asking whether it is there.
        std::fs::write(&credential, r#"{"AccountTag": "acc"#).unwrap();
        assert_eq!(credential_tunnel_id(&credential), None);
    }
}
