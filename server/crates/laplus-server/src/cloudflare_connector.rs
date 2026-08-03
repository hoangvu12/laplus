//! Lifecycle of a laplus-managed `cloudflared` connector.

use crate::process::Search;
use crate::public_exposure::{Refusal, RefusalReason};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

const DIRECTORY: &str = "cloudflare";
const SETTINGS: &str = "connector.json";
const TOKEN: &str = "connector.token";
/// The narrow `<UUID>.json` run credential of a dedicated tunnel. One name
/// rather than the tunnel's own UUID, because there is one dedicated tunnel per
/// environment and a path the endpoint row records has to be reconstructible
/// before that row exists — which is what a resumed adoption does.
const CREDENTIAL: &str = "tunnel.json";
const INGRESS: &str = "connector.yml";
const MAX_RESTARTS: u8 = 3;
/// Who runs the connector *process*, which is a different question from who owns
/// the tunnel it serves — see [`crate::public_exposure::TunnelOwnership`]. This
/// manager only ever supervises laplus's own, so the answer here is a constant;
/// an externally managed connector is never represented by a [`Manager`] at all.
const CONNECTOR_OWNERSHIP: &str = "laplus";
const READY_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

crate::public_exposure::closed_vocabulary! {
    /// Whether laplus should be running this connector.
    ///
    /// The *desired* state, which is not the actual one: `connector_state`
    /// reports what the child is doing, and the gap between the two is what
    /// supervision closes. Persisted, because it is the answer a restart
    /// resumes from.
    DesiredState as "desired connector state" {
        Running => "running",
        Stopped => "stopped",
    }
}

/// Which narrow credential this connector runs on, and therefore what laplus
/// must write for it.
///
/// **The two are not interchangeable, and the difference is who configures the
/// tunnel.** A connector token belongs to a tunnel Cloudflare configures: the
/// token is the whole of the run authority and the ingress rules live at
/// Cloudflare. A `<UUID>.json` tunnel credential belongs to a tunnel *laplus*
/// configures, so laplus's own file has to name the tunnel and the credential
/// as well as the ingress — which is why this is a shape and not a flag.
///
/// Untagged and flattened on purpose: `{ "tokenFile": … }` is exactly what
/// every connector settings file written before adoption existed contains, so
/// an installed connector keeps running across this change rather than losing
/// the desired state that makes it start with its owner.
/// `rename_all` on the variants rather than on the enum, because on an enum it
/// renames the *variant names* — which an untagged enum never writes — and
/// leaves the fields in `snake_case`. That silently produced a settings file
/// no build could read back, and the connector that stopped coming back after a
/// restart looked like a supervision bug rather than a serialization one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RunCredential {
    /// Cloudflare owns the tunnel's configuration; laplus only runs it.
    #[serde(rename_all = "camelCase")]
    ConnectorToken { token_file: PathBuf },
    /// laplus owns the tunnel's configuration: an adopted or laplus-created
    /// dedicated tunnel, run from the credential retrieved or created for it.
    #[serde(rename_all = "camelCase")]
    TunnelCredential {
        tunnel_id: String,
        credential_file: PathBuf,
    },
}

impl RunCredential {
    /// Where the secret is, so that redaction and cleanup can find it without
    /// knowing which kind it is.
    pub fn file(&self) -> &Path {
        match self {
            Self::ConnectorToken { token_file } => token_file,
            Self::TunnelCredential { credential_file, .. } => credential_file,
        }
    }
}

/// What a connector needs to run, and nothing about who owns its tunnel.
///
/// **Ownership deliberately lives in one place, and it is not here.** This file
/// used to carry `ownership: "adopted"`, written unconditionally and never read
/// back; making it a real value would have left two records of one fact, and a
/// boot that restores the endpoint row from this file would then decide which
/// of them wins. The durable answer is the `public_exposure_endpoint` row —
/// `docs/adr/0049` — and `managed_connector_snapshot` in `server.rs` reads it
/// from there the same way it reads verification state.
///
/// [`RunCredential`] is not an exception to that. It says which *file* runs the
/// connector, which laplus needs in order to build a command line; it does not
/// say who may delete the tunnel, and an adopted tunnel and a laplus-created one
/// are indistinguishable here by design.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub https_origin: String,
    pub loopback_origin: String,
    pub executable_path: PathBuf,
    #[serde(flatten)]
    pub credential: RunCredential,
    pub desired_state: DesiredState,
}

/// What both configuration paths establish before either writes anything.
struct Prepared {
    https_origin: String,
    loopback_origin: String,
    executable_path: PathBuf,
    version: String,
}

#[derive(Debug)]
struct Runtime {
    configuration: Option<Configuration>,
    connector_state: String,
    readiness: Option<bool>,
    metrics_origin: Option<String>,
    detected_version: Option<String>,
    failure_message: Option<String>,
    restart_count: u8,
    logs: Vec<String>,
    shutdown: bool,
    generation: u64,
}

#[derive(Debug)]
pub struct Manager {
    directory: PathBuf,
    installer: crate::cloudflare_install::Installer,
    runtime: Mutex<Runtime>,
    changed: Notify,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    owner_origin: Mutex<Option<String>>,
}

impl Manager {
    pub fn open(preferences: &Path) -> Arc<Self> {
        let directory = preferences.join(DIRECTORY);
        let configuration = std::fs::read(directory.join(SETTINGS))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let connector_state = match configuration
            .as_ref()
            .map(|value: &Configuration| value.desired_state)
        {
            Some(DesiredState::Running) => "starting",
            _ => "stopped",
        };
        Arc::new(Self {
            installer: crate::cloudflare_install::Installer::open(&directory),
            directory,
            runtime: Mutex::new(Runtime {
                configuration,
                connector_state: connector_state.into(),
                readiness: None,
                metrics_origin: None,
                detected_version: None,
                failure_message: None,
                restart_count: 0,
                logs: Vec::new(),
                shutdown: false,
                generation: 0,
            }),
            changed: Notify::new(),
            task: Mutex::new(None),
            owner_origin: Mutex::new(None),
        })
    }

    pub fn begin(self: &Arc<Self>) {
        let mut task = self.task.lock().unwrap();
        if task.is_none() {
            let manager = Arc::clone(self);
            *task = Some(tokio::spawn(async move { manager.supervise().await }));
        }
    }

    pub fn set_loopback_origin(&self, origin: String) {
        *self.owner_origin.lock().unwrap() = Some(origin.clone());
        let configuration = {
            let mut runtime = self.runtime.lock().unwrap();
            let Some(configuration) = runtime.configuration.as_mut() else {
                return;
            };
            configuration.loopback_origin = origin;
            configuration.clone()
        };
        let _ = persist(&self.directory, &configuration);
    }

    pub async fn configure(
        self: &Arc<Self>,
        hostname: &str,
        executable: &Path,
        connector_token: &str,
    ) -> Result<(), Refusal> {
        let token_file = self.directory.join(TOKEN);
        if connector_token.trim().is_empty() && !token_file.is_file() {
            return Err(Refusal::rejected(
                RefusalReason::ConnectorRequired,
                "Enter the tunnel-specific connector token.",
            ));
        }
        // **Everything that can refuse, refuses before the secret is written.**
        // A hostname this server will not accept or an executable it cannot run
        // are answers it can give without keeping the token the request carried,
        // and writing it first would leave a rejected request's secret on disk.
        let prepared = self.prepare(hostname, executable).await?;
        private_directory(&self.directory)?;
        if !connector_token.trim().is_empty() {
            private_write(&token_file, connector_token.as_bytes())?;
        }
        self.commit(prepared, RunCredential::ConnectorToken { token_file })
    }

    /// Where the narrow run credential of a dedicated tunnel belongs.
    ///
    /// Answered before the credential exists, because adoption has to journal
    /// the path it is *about* to write and a resume has to look for the file at
    /// the same place without re-reading anything from Cloudflare.
    pub fn credential_path(&self) -> PathBuf {
        self.directory.join(CREDENTIAL)
    }

    /// Where laplus's own isolated ingress configuration belongs.
    ///
    /// **Never the developer's `~/.cloudflared/config.yml`** — ADR-0045 puts
    /// editing that out of scope entirely, so every connector laplus runs is
    /// pointed at this path with `--config`.
    pub fn configuration_path(&self) -> PathBuf {
        self.directory.join(INGRESS)
    }

    /// Whether this connector is already configured for `https_origin`.
    ///
    /// Asked of the manager rather than fished out of its JSON snapshot: three
    /// call sites were poking `snapshot()["configured"]` and
    /// `snapshot()["httpsOrigin"]` by hand, which is a typed question answered
    /// through an untyped keyhole.
    pub fn serves(&self, https_origin: &str) -> bool {
        self.runtime
            .lock()
            .unwrap()
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.https_origin == https_origin)
    }

    /// Whether a connector is configured at all, whatever it serves.
    pub fn configured(&self) -> bool {
        self.runtime.lock().unwrap().configuration.is_some()
    }

    /// The dedicated tunnel this connector is already configured to run, if it
    /// runs one.
    ///
    /// **Observed state, and the only record of it that survives a database
    /// write that did not.** Adoption writes the endpoint row last, so a crash
    /// between configuring the connector and recording the row leaves laplus
    /// running a tunnel nothing says it owns — and the next confirmation would
    /// see laplus's *own* connections in the listing and conclude the tunnel was
    /// somebody else's. This is how that resume tells the two apart.
    pub fn dedicated_tunnel_id(&self) -> Option<String> {
        match &self.runtime.lock().unwrap().configuration.as_ref()?.credential {
            RunCredential::TunnelCredential { tunnel_id, .. } => Some(tunnel_id.clone()),
            RunCredential::ConnectorToken { .. } => None,
        }
    }

    /// The loopback origin a connector would be pointed at, whether or not one
    /// is configured yet.
    ///
    /// The adoption offer has to show it *before* anything is configured: the
    /// developer is being asked to route a public hostname somewhere, and a
    /// confirmation that does not say where is a confirmation of an abstraction.
    pub fn loopback_origin(&self) -> Option<String> {
        self.owner_origin.lock().unwrap().clone()
    }

    /// Run a connector for a dedicated tunnel laplus configures itself.
    ///
    /// The credential is already on disk — retrieved by adoption or written by
    /// creation — so this only decides how to run it. Nothing here says who owns
    /// the tunnel; `docs/adr/0049` keeps that on the endpoint row.
    pub async fn dedicate(
        self: &Arc<Self>,
        hostname: &str,
        executable: &Path,
        tunnel_id: &str,
        credential_file: &Path,
    ) -> Result<(), Refusal> {
        if !credential_file.is_file() {
            return Err(Refusal::rejected(
                RefusalReason::ConnectorRequired,
                "The tunnel credential is not on disk.",
            ));
        }
        let prepared = self.prepare(hostname, executable).await?;
        self.commit(
            prepared,
            RunCredential::TunnelCredential {
                tunnel_id: tunnel_id.to_string(),
                credential_file: credential_file.to_path_buf(),
            },
        )
    }

    /// Everything both paths must establish before either writes anything.
    async fn prepare(&self, hostname: &str, executable: &Path) -> Result<Prepared, Refusal> {
        let https_origin = crate::public_exposure::normalize_hostname(hostname)
            .map_err(|message| Refusal::rejected(RefusalReason::HostnameInvalid, message))?;
        let executable_path = Search::from_environment().startable(executable).ok_or_else(|| {
            Refusal::rejected(
                RefusalReason::ExecutableUnusable,
                "The selected cloudflared executable cannot be started.",
            )
        })?;
        let version = compatible_version(&executable_path).await?;
        let loopback_origin = self.owner_origin.lock().unwrap().clone().ok_or_else(|| {
            Refusal::rejected(
                RefusalReason::ConnectorRequired,
                "The connector owner has not finished starting.",
            )
        })?;
        Ok(Prepared { https_origin, loopback_origin, executable_path, version })
    }

    /// Write the configuration down and ask supervision to converge on it.
    fn commit(self: &Arc<Self>, prepared: Prepared, credential: RunCredential) -> Result<(), Refusal> {
        private_directory(&self.directory)?;
        let Prepared { https_origin, loopback_origin, executable_path, version } = prepared;
        let configuration = Configuration {
            https_origin,
            loopback_origin,
            executable_path,
            credential,
            desired_state: DesiredState::Running,
        };
        // **Here as well as in `supervise`, so that it can fail out loud.** The
        // supervision loop writes the same file before each launch and can only
        // report a failure as a connector that never became ready; adoption
        // journals this as its `configuration` step and needs the refusal
        // synchronously, with the step marked failed and named as remaining
        // work. `write_ingress` is idempotent, so the second write is a no-op.
        write_ingress(&self.directory, &configuration)?;
        persist(&self.directory, &configuration)?;
        {
            let mut runtime = self.runtime.lock().unwrap();
            runtime.configuration = Some(configuration);
            runtime.detected_version = Some(version);
            runtime.connector_state = "starting".into();
            runtime.readiness = None;
            runtime.failure_message = None;
            runtime.restart_count = 0;
            runtime.generation = runtime.generation.wrapping_add(1);
        }
        self.begin();
        self.changed.notify_waiters();
        Ok(())
    }

    pub fn snapshot(&self) -> Value {
        let runtime = self.runtime.lock().unwrap();
        let Some(configuration) = &runtime.configuration else {
            let mut unconfigured = json!({
                "configured": false, "ownership": CONNECTOR_OWNERSHIP,
                "desiredState": DesiredState::Stopped,
                "connectorState": "unconfigured", "readiness": null, "httpsOrigin": null,
                "executablePath": null, "detectedVersion": null, "metricsOrigin": null,
                "failureMessage": null, "restartCount": 0, "logs": []
            });
            // Present before anything is configured, because ticket 05's
            // dedication confirmation has to show the developer where the
            // public hostname would be routed *to* — and there is nothing else
            // on the wire that knows.
            //
            // **Absent rather than `null` when there is nothing to say.** The
            // contract declares it `Schema.optional`, which admits a missing key
            // and not a null one, so emitting `null` would fail the decode of
            // the *whole* snapshot — and only in the window before the listener
            // has a port, which is exactly where nothing would be watching.
            if let Some(origin) = self.owner_origin.lock().unwrap().clone() {
                unconfigured["loopbackOrigin"] = json!(origin);
            }
            // Present before anything is configured for the same reason the
            // loopback target is: ticket 06's creation preview has to name where
            // the tunnel's run credential will be kept, and a confirmation that
            // says "somewhere private" is a confirmation of an abstraction. A
            // path and never contents — the rule `certificatePath` already
            // follows.
            unconfigured["credentialPath"] = json!(self.credential_path());
            return unconfigured;
        };
        json!({
            "configured": true,
            "ownership": CONNECTOR_OWNERSHIP,
            "desiredState": configuration.desired_state,
            "connectorState": runtime.connector_state,
            "readiness": runtime.readiness,
            "httpsOrigin": configuration.https_origin,
            "loopbackOrigin": configuration.loopback_origin,
            "credentialPath": configuration.credential.file(),
            "executablePath": configuration.executable_path,
            "detectedVersion": runtime.detected_version,
            "metricsOrigin": runtime.metrics_origin,
            "failureMessage": runtime.failure_message,
            "restartCount": runtime.restart_count,
            "logs": runtime.logs,
        })
    }

    /// Every executable this environment could run, in the order the policy
    /// prefers them: a system installation, then whatever the developer
    /// selected, then the copy laplus installed for itself.
    ///
    /// The source is what makes ownership legible — it is the difference
    /// between an executable laplus may replace and one it must leave alone.
    pub async fn discover(&self) -> Value {
        let selected = self
            .runtime
            .lock()
            .unwrap()
            .configuration
            .as_ref()
            .map(|configuration| configuration.executable_path.clone());
        let app_managed = self.installer.installed_path();
        let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();
        if let Some(path) = Search::from_environment().locate("cloudflared") {
            candidates.push((path, "system"));
        }
        for path in [selected.clone(), app_managed.clone()].into_iter().flatten() {
            if candidates.iter().any(|(known, _)| known == &path) {
                continue;
            }
            let source = if app_managed.as_ref() == Some(&path) {
                "app-managed"
            } else {
                "user-selected"
            };
            candidates.push((path, source));
        }
        let mut executables = Vec::new();
        for (path, source) in candidates {
            let detected = detect_version(&path).await;
            executables.push(json!({
                "path": path,
                "source": source,
                "selected": selected.as_ref() == Some(&path),
                "version": detected.as_ref().ok(),
                "compatibility": if detected.as_ref().is_ok_and(|version| compatible_version_text(version)) { "compatible" } else { "incompatible" },
                "failureMessage": detected.as_ref().err(),
            }));
        }
        json!({ "executables": executables })
    }

    /// What the wizard may offer, and what laplus has already installed.
    pub async fn install_snapshot(&self) -> Value {
        self.installer.snapshot().await
    }

    /// Install the release the developer approved, by version and digest.
    pub async fn install(
        &self,
        version: &str,
        checksum: &str,
    ) -> Result<(), crate::cloudflare_install::Refusal> {
        self.installer.install(version, checksum).await
    }

    pub fn set_desired(&self, running: bool, retry: bool) -> Result<(), Refusal> {
        let configuration = {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.configuration.is_none() {
                return Err(Refusal::rejected(
                    RefusalReason::ConnectorRequired,
                    "Configure a connector first.",
                ));
            }
            if running && !retry && runtime.connector_state == "restart-exhausted" {
                return Err(Refusal::rejected(
                    RefusalReason::RestartsExhausted,
                    "Automatic restarts are exhausted; use Retry to start the connector again.",
                ));
            }
            let desired = if running { DesiredState::Running } else { DesiredState::Stopped };
            if !retry
                && runtime
                    .configuration
                    .as_ref()
                    .is_some_and(|value| value.desired_state == desired)
            {
                return Ok(());
            }
            if retry {
                runtime.restart_count = 0;
                runtime.failure_message = None;
            }
            runtime.generation = runtime.generation.wrapping_add(1);
            runtime.connector_state = if running { "starting" } else { "stopping" }.into();
            runtime.readiness = None;
            let configuration = runtime.configuration.as_mut().unwrap();
            configuration.desired_state = desired;
            configuration.clone()
        };
        persist(&self.directory, &configuration)?;
        self.changed.notify_waiters();
        Ok(())
    }

    /// Ask this connector to stop and wait until it has, within a bounded wait.
    ///
    /// **Forget and delete both have to stop it before they touch anything.**
    /// Removing a running connector's configuration leaves a `cloudflared`
    /// serving a public hostname from a file nothing records, and `cloudflared
    /// tunnel delete` refuses outright while the tunnel still has connections —
    /// so "stop, then clean, then remove" is an order rather than a preference.
    ///
    /// Answers whether nothing is running any more, and never blocks forever:
    /// the supervision loop is what changes the word, so a loop that is wedged
    /// must produce a refusal the caller can report rather than a request that
    /// hangs. A connector that was never configured is already stopped.
    ///
    /// **Four words mean "no child of mine is running", not one.** `stopped` is
    /// what a connector asked to stop reaches, but a connector whose restart
    /// budget ran out sits in `restart-exhausted` and one that never started
    /// sits in `failed` — both with nothing alive and both already carrying
    /// `desired_state: stopped`, so `set_desired` has nothing left to change and
    /// the word never moves. Waiting for `stopped` alone made a cleanup refuse
    /// to remove the setup of a connector that had already died.
    pub async fn stop_and_settle(&self, budget: std::time::Duration) -> bool {
        match self.set_desired(false, false) {
            Ok(()) => {}
            // The only refusal `set_desired(false, …)` can give is "nothing is
            // configured", which is the state this is trying to reach.
            Err(_) => return true,
        }
        let deadline = std::time::Instant::now() + budget;
        loop {
            {
                let runtime = self.runtime.lock().unwrap();
                if runtime.configuration.is_none()
                    || matches!(
                        runtime.connector_state.as_str(),
                        "stopped" | "restart-exhausted" | "failed" | "unconfigured"
                    )
                {
                    return true;
                }
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Remove laplus's own connector configuration, and nothing else.
    ///
    /// **Two files, both written by laplus into its own private directory**: the
    /// ingress `--config` file and the settings file that makes the connector
    /// start with its owner. Never `~/.cloudflared/config.yml`, which ADR-0045
    /// puts out of scope, and never an executable — the app-managed
    /// `cloudflared` lives under this same directory and is a *tool* rather than
    /// this exposure's setup, so it survives (ADR-0052).
    ///
    /// Idempotent: a file that is already gone is this step having already
    /// happened, which is what makes a retried cleanup safe.
    pub fn remove_configuration(&self) -> Result<(), Refusal> {
        for path in [self.directory.join(INGRESS), self.directory.join(SETTINGS)] {
            remove_if_present(&path)?;
        }
        let mut runtime = self.runtime.lock().unwrap();
        runtime.configuration = None;
        runtime.connector_state = "unconfigured".into();
        runtime.readiness = None;
        runtime.metrics_origin = None;
        runtime.failure_message = None;
        runtime.restart_count = 0;
        runtime.logs.clear();
        runtime.generation = runtime.generation.wrapping_add(1);
        drop(runtime);
        self.changed.notify_waiters();
        Ok(())
    }

    /// Remove the run credentials laplus stored for this environment.
    ///
    /// **Both shapes, whichever this connector used.** A dedicated tunnel's
    /// `<UUID>.json` and a connector token file are the same thing to a cleanup —
    /// laplus-owned secrets in laplus's own directory — and the settings file
    /// that said which one is in use may already be gone, because
    /// [`Manager::remove_configuration`] runs first. Naming both is what lets
    /// this step be retried after a restart without a record to read.
    ///
    /// This is also what releases the credential that makes creation refuse: a
    /// forget that left `tunnel.json` behind would leave the next creation
    /// permanently refused with `ownership-conflict`.
    pub fn remove_credentials(&self) -> Result<(), Refusal> {
        for path in [self.credential_path(), self.directory.join(TOKEN)] {
            remove_if_present(&path)?;
        }
        Ok(())
    }

    /// Whether either run credential is still on disk.
    ///
    /// The observed half of [`crate::public_exposure::CleanupState`]: a cleanup
    /// reports `credential-remove` as done because the files are gone, not
    /// because a log line says so.
    ///
    /// **Anything at the path counts, not only a readable credential.** This is
    /// asked by a cleanup rather than by a connector, and a cleanup's question is
    /// whether there is still something of laplus's to remove — a truncated
    /// credential, or a directory somebody left in the way, is still that.
    pub fn holds_credentials(&self) -> bool {
        self.credential_path().exists() || self.directory.join(TOKEN).exists()
    }

    /// Whether laplus's own connector configuration is still on disk.
    pub fn holds_configuration(&self) -> bool {
        self.directory.join(INGRESS).exists() || self.directory.join(SETTINGS).exists()
    }

    pub async fn shutdown(&self) {
        self.runtime.lock().unwrap().shutdown = true;
        self.changed.notify_waiters();
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    async fn supervise(self: Arc<Self>) {
        loop {
            let (configuration, generation) = {
                let runtime = self.runtime.lock().unwrap();
                if runtime.shutdown {
                    return;
                }
                (
                    runtime
                        .configuration
                        .clone()
                        .filter(|value| value.desired_state == DesiredState::Running),
                    runtime.generation,
                )
            };
            let Some(configuration) = configuration else {
                // **Nothing is running, so say nothing is running.** The word
                // was previously left wherever the last iteration put it, and
                // "replace" writes `starting` optimistically before looping back
                // here — so a connector that was reconfigured and then stopped
                // reported `starting` for as long as it stayed stopped. The
                // compact row said "Starting" for a connector with no child at
                // all, and ticket 07's cleanups waited for a word that could
                // never arrive.
                //
                // `restart-exhausted` and `failed` survive: both already mean
                // nothing is running, and both are what an explicit Retry is
                // offered for.
                {
                    let mut runtime = self.runtime.lock().unwrap();
                    if runtime.configuration.is_none() {
                        runtime.connector_state = "unconfigured".into();
                    } else if !matches!(
                        runtime.connector_state.as_str(),
                        "restart-exhausted" | "failed"
                    ) {
                        runtime.connector_state = "stopped".into();
                    }
                    runtime.readiness = None;
                }
                self.changed.notified().await;
                continue;
            };
            let metrics = match free_loopback_address().await {
                Ok(address) => address,
                Err(message) => {
                    self.fail(message, true);
                    self.changed.notified().await;
                    continue;
                }
            };
            if let Err(refusal) = write_ingress(&self.directory, &configuration) {
                self.fail(refusal.message, true);
                self.changed.notified().await;
                continue;
            }
            let config_file = self.directory.join(INGRESS);
            let mut arguments = vec![
                "tunnel".to_string(),
                "--config".into(),
                config_file.to_string_lossy().into_owned(),
            ];
            // **Both secrets are passed by file and never as a value.** A
            // connector token names its file; a dedicated tunnel names nothing
            // at all here, because the credential's path is inside laplus's own
            // configuration — which is the strongest form of the same rule.
            if let RunCredential::ConnectorToken { token_file } = &configuration.credential {
                arguments.push("--token-file".into());
                arguments.push(token_file.to_string_lossy().into_owned());
            }
            arguments.extend(["--metrics".to_string(), metrics.clone(), "run".into()]);
            let mut command = tokio::process::Command::new(&configuration.executable_path);
            command
                .args(&arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(_) => {
                    self.child_failed("cloudflared could not be started.");
                    continue;
                }
            };
            let process_group = child.id();
            let mut log_task = child.stderr.take().map(|mut stderr| {
                let manager = Arc::clone(&self);
                let configuration = configuration.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let _ = stderr.read_to_end(&mut bytes).await;
                    manager.record_log(&String::from_utf8_lossy(&bytes), &configuration);
                })
            });
            {
                let mut runtime = self.runtime.lock().unwrap();
                runtime.connector_state = if runtime.restart_count == 0 {
                    "starting"
                } else {
                    "degraded"
                }
                .into();
                runtime.metrics_origin = Some(format!("http://{metrics}"));
                runtime.readiness = Some(false);
            }
            let client = reqwest::Client::new();
            loop {
                let action = {
                    let runtime = self.runtime.lock().unwrap();
                    if runtime.shutdown {
                        "shutdown"
                    } else if runtime
                        .configuration
                        .as_ref()
                        .is_none_or(|value| value.desired_state != DesiredState::Running)
                    {
                        "stop"
                    } else if runtime.generation != generation {
                        "replace"
                    } else {
                        "continue"
                    }
                };
                if action != "continue" {
                    terminate(&mut child).await;
                    if let Some(task) = log_task.take() {
                        let _ = task.await;
                    }
                    let mut runtime = self.runtime.lock().unwrap();
                    runtime.connector_state = if action == "replace" {
                        "starting"
                    } else {
                        "stopped"
                    }
                    .into();
                    runtime.readiness = None;
                    if action == "shutdown" {
                        return;
                    }
                    break;
                }
                match child.try_wait() {
                    Ok(Some(_)) => {
                        let replacement_may_hold_the_log =
                            log_task.as_ref().is_some_and(|task| !task.is_finished());
                        if replacement_may_hold_the_log
                            && replacement_is_ready(&client, &metrics).await
                        {
                            if let Some(task) = log_task.take() {
                                task.abort();
                            }
                            {
                                let mut runtime = self.runtime.lock().unwrap();
                                runtime.connector_state = "ready".into();
                                runtime.readiness = Some(true);
                                runtime.failure_message = None;
                            }
                            loop {
                                let action = {
                                    let runtime = self.runtime.lock().unwrap();
                                    if runtime.shutdown {
                                        "shutdown"
                                    } else if runtime
                                        .configuration
                                        .as_ref()
                                        .is_none_or(|value| value.desired_state != DesiredState::Running)
                                    {
                                        "stop"
                                    } else if runtime.generation != generation {
                                        "replace"
                                    } else {
                                        "continue"
                                    }
                                };
                                if action != "continue" {
                                    terminate_group(process_group).await;
                                    let mut runtime = self.runtime.lock().unwrap();
                                    runtime.connector_state = if action == "replace" {
                                        "starting"
                                    } else {
                                        "stopped"
                                    }
                                    .into();
                                    runtime.readiness = None;
                                    if action == "shutdown" {
                                        return;
                                    }
                                    break;
                                }
                                if !ready(&client, &metrics).await {
                                    self.child_failed(
                                        "The replacement cloudflared connector is no longer ready.",
                                    );
                                    self.wait_before_restart().await;
                                    break;
                                }
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {},
                                    _ = self.changed.notified() => {},
                                }
                            }
                            break;
                        }
                        if let Some(task) = log_task.take() {
                            if replacement_may_hold_the_log {
                                task.abort();
                            } else {
                                let _ = task.await;
                            }
                        }
                        self.child_failed("cloudflared exited before the connector was stopped.");
                        self.wait_before_restart().await;
                        break;
                    }
                    Err(_) => {
                        self.child_failed("cloudflared could not be observed.");
                        break;
                    }
                    Ok(None) => {}
                }
                if ready(&client, &metrics).await {
                    let mut runtime = self.runtime.lock().unwrap();
                    runtime.connector_state = "ready".into();
                    runtime.readiness = Some(true);
                    runtime.failure_message = None;
                }
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {},
                    _ = self.changed.notified() => {},
                }
            }
        }
    }

    fn child_failed(&self, message: &str) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.restart_count = runtime.restart_count.saturating_add(1);
        runtime.readiness = Some(false);
        runtime.failure_message = Some(message.into());
        if runtime.restart_count >= MAX_RESTARTS {
            runtime.connector_state = "restart-exhausted".into();
            if let Some(configuration) = runtime.configuration.as_mut() {
                configuration.desired_state = DesiredState::Stopped;
                let _ = persist(&self.directory, configuration);
            }
        } else {
            runtime.connector_state = "degraded".into();
        }
    }

    async fn wait_before_restart(&self) {
        let restart_count = self.runtime.lock().unwrap().restart_count;
        if restart_count >= MAX_RESTARTS {
            return;
        }
        tokio::select! {
            _ = tokio::time::sleep(restart_delay(restart_count)) => {},
            _ = self.changed.notified() => {},
        }
    }

    fn fail(&self, message: String, exhausted: bool) {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.failure_message = Some(message);
        runtime.connector_state = if exhausted {
            "restart-exhausted"
        } else {
            "failed"
        }
        .into();
        runtime.readiness = Some(false);
    }

    /// Keep the connector's own output, with its run credential taken out of it.
    ///
    /// Read from the file rather than remembered, and read for whichever
    /// credential this connector has: a tunnel credential is a JSON document
    /// whose `TunnelSecret` is the secret, so redacting the file's whole
    /// contents would miss it in every sentence cloudflared could quote it in.
    fn record_log(&self, text: &str, configuration: &Configuration) {
        let held = std::fs::read_to_string(configuration.credential.file()).unwrap_or_default();
        let mut redacted = text.to_string();
        for secret in secrets_within(&held) {
            redacted = redacted.replace(&secret, "[REDACTED]");
        }
        let mut runtime = self.runtime.lock().unwrap();
        runtime.logs.extend(
            redacted
                .lines()
                .take(20)
                .map(|line| line.chars().take(500).collect()),
        );
        let excess = runtime.logs.len().saturating_sub(50);
        if excess > 0 {
            runtime.logs.drain(..excess);
        }
    }
}

/// Every string a run-credential file holds, longest first.
///
/// **Two shapes, one rule.** A connector token file *is* the secret. A tunnel
/// credential file is JSON, and it is the `TunnelSecret` inside it that must
/// never be quoted back — redacting the whole document would leave the secret
/// itself readable in any sentence that mentioned only it.
///
/// Secret-shaped *fields*, not every field: an account tag and a tunnel id are
/// also in there, and both appear in the snapshot already — blanking them would
/// turn "the connector for tunnel 2222 failed" into a sentence nobody can act
/// on. Matched by what the key is called rather than by its exact spelling, so
/// a credential shape laplus has not seen still has its secret taken out.
/// Longest first, so a value that contains another is replaced before its
/// substring is.
fn secrets_within(held: &str) -> Vec<String> {
    let mut secrets = vec![held.trim().to_string()];
    if let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(held) {
        secrets.extend(
            fields
                .iter()
                .filter(|(name, _)| {
                    let name = name.to_ascii_lowercase();
                    name.contains("secret") || name.contains("token") || name.contains("password")
                })
                .filter_map(|(_, value)| value.as_str())
                .map(str::to_string),
        );
    }
    secrets.retain(|secret| !secret.trim().is_empty());
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
}

/// Whether the connector answers `/ready`, within a bounded wait.
///
/// **Bounded on purpose, and bounded here rather than on the client.** A probe
/// is the supervision loop's whole body, so one that can wait forever is a
/// connector that can never be stopped: a connector wedged with its metrics
/// port open but unanswering held the supervision task inside `send()` while a
/// stop request sat unprocessed. Wrapping the call means the budget survives a
/// client that was built without one. It is a hang detector rather than a
/// performance target — a connector that cannot answer on loopback within it is
/// not ready, which is what the next iteration concludes anyway.
async fn ready(client: &reqwest::Client, metrics: &str) -> bool {
    tokio::time::timeout(
        READY_PROBE_TIMEOUT,
        client.get(format!("http://{metrics}/ready")).send(),
    )
    .await
    .is_ok_and(|answer| answer.is_ok_and(|response| response.status().is_success()))
}

pub fn restart_delay(restart_count: u8) -> std::time::Duration {
    std::time::Duration::from_millis(
        100 * 2u64.saturating_pow(restart_count.saturating_sub(1).into()),
    )
}

async fn terminate(child: &mut tokio::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let process_group = child.id();
    terminate_group(process_group).await;
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        if tokio::time::timeout(std::time::Duration::from_secs(1), child.wait())
            .await
            .is_ok()
        {
            return;
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}

async fn terminate_group(process_group: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = process_group {
        let _ = tokio::process::Command::new("kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(windows)]
    if let Some(pid) = process_group {
        let mut command = tokio::process::Command::new("taskkill.exe");
        command
            .args(["/PID", &pid.to_string(), "/T"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = command.status().await;
    }
}

async fn replacement_is_ready(client: &reqwest::Client, metrics: &str) -> bool {
    for _ in 0..5 {
        if ready(client, metrics).await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

/// What an executable says it is, or nothing if it cannot say.
pub(crate) async fn detected_version(executable: &Path) -> Option<String> {
    detect_version(executable).await.ok()
}

pub(crate) async fn compatible_version(executable: &Path) -> Result<String, Refusal> {
    let version = detect_version(executable).await.map_err(|message| {
        Refusal::rejected(RefusalReason::ExecutableUnusable, message)
    })?;
    if !compatible_version_text(&version) {
        return Err(Refusal::rejected(
            RefusalReason::ExecutableUnusable,
            "The selected cloudflared executable is incompatible; version 2024 or newer is \
             required.",
        ));
    }
    Ok(version)
}

async fn detect_version(executable: &Path) -> Result<String, String> {
    let output = tokio::process::Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| {
            "The selected cloudflared executable could not report its version.".to_string()
        })?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err("The selected cloudflared executable could not report its version.".into());
    }
    Ok(version)
}

fn compatible_version_text(version: &str) -> bool {
    version
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| part.len() == 4)
        .and_then(|part| part.parse::<u16>().ok())
        .is_some_and(|year| year >= 2024)
}

async fn free_loopback_address() -> Result<String, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "No loopback port is available for cloudflared metrics.".to_string())?;
    let address = listener
        .local_addr()
        .map_err(|_| "The metrics address could not be read.".to_string())?;
    drop(listener);
    Ok(address.to_string())
}

pub(crate) fn private_directory(directory: &Path) -> Result<(), Refusal> {
    std::fs::create_dir_all(directory).map_err(|_| local_setup("directory could not be created"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| local_setup("directory could not be protected"))?;
    }
    #[cfg(windows)]
    protect_windows(directory, true)?;
    Ok(())
}

/// Remove one of laplus's own files, treating "already gone" as done.
///
/// A cleanup step is retried after a restart, and the whole point of retrying it
/// is that the first attempt may have removed some of what it was asked to. So
/// the absence of a file is this step having succeeded rather than a failure to
/// report, and only a file that is *there and will not go* is a refusal.
fn remove_if_present(path: &Path) -> Result<(), Refusal> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(local_setup("file could not be removed")),
    }
}

/// Everything that can fail while laplus writes its own private files says so
/// the same way: nothing at Cloudflare went wrong, and the retry is local.
fn local_setup(what: &str) -> Refusal {
    Refusal::rejected(
        RefusalReason::LocalSetupFailed,
        format!("The private connector {what}."),
    )
}

/// Write a private file, atomically, leaving nothing behind if it fails.
///
/// **The cleanup is the point of the wrapper.** The temporary is opened with
/// `create_new`, so a temporary left behind by a failed write makes every later
/// write to the same path fail for a reason that has nothing to do with it —
/// which is how a retried adoption came to fail again at a step whose cause had
/// already been removed.
pub(crate) fn private_write(path: &Path, bytes: &[u8]) -> Result<(), Refusal> {
    let temporary = path.with_file_name(format!(
        "{}.private-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("connector"),
        std::process::id(),
    ));
    let outcome = write_privately(&temporary, path, bytes);
    if outcome.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    outcome
}

fn write_privately(temporary: &Path, path: &Path, bytes: &[u8]) -> Result<(), Refusal> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(temporary)
        .map_err(|_| local_setup("credential could not be written"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| local_setup("credential could not be written"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| local_setup("credential could not be protected"))?;
    }
    #[cfg(windows)]
    protect_windows(temporary, false)?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path).map_err(|_| local_setup("credential could not be replaced"))?;
    }
    std::fs::rename(temporary, path)
        .map_err(|_| local_setup("credential could not be installed"))?;
    Ok(())
}

#[cfg(windows)]
fn protect_windows(path: &Path, directory: bool) -> Result<(), Refusal> {
    let user = std::env::var("USERNAME")
        .map_err(|_| local_setup("credential ACL needs a Windows account"))?;
    let grant = if directory {
        format!("{user}:(OI)(CI)F")
    } else {
        format!("{user}:F")
    };
    let mut command = std::process::Command::new("icacls.exe");
    command
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(grant);
    crate::process::without_a_console(&mut command);
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| local_setup("credential ACL could not be applied"))?;
    if !status.success() {
        return Err(local_setup("credential ACL could not be applied"));
    }
    Ok(())
}

fn persist(directory: &Path, configuration: &Configuration) -> Result<(), Refusal> {
    private_directory(directory)?;
    let bytes = serde_json::to_vec_pretty(configuration)
        .map_err(|_| local_setup("settings could not be encoded"))?;
    private_write(&directory.join(SETTINGS), &bytes)
}

/// laplus's own `cloudflared` configuration, and only ever laplus's own.
///
/// **Never `~/.cloudflared/config.yml`.** ADR-0045 puts editing the developer's
/// default configuration out of scope, so this file lives in laplus's private
/// directory and is reached with an explicit `--config`.
///
/// A dedicated tunnel adds two lines the connector-token form does not have and
/// cannot have: the tunnel it runs and the credential that runs it. Cloudflare
/// keeps a token tunnel's ingress, so writing either for one would be laplus
/// claiming configuration authority it does not hold.
fn write_ingress(directory: &Path, configuration: &Configuration) -> Result<(), Refusal> {
    let mut contents = String::new();
    if let RunCredential::TunnelCredential { tunnel_id, credential_file } = &configuration.credential
    {
        contents.push_str(&format!(
            "tunnel: {tunnel_id}\ncredentials-file: {}\n",
            credential_file.display()
        ));
    }
    contents.push_str(&format!(
        "ingress:\n  - hostname: {}\n    service: {}\n  - service: http_status:404\n",
        crate::public_exposure::hostname_of(&configuration.https_origin),
        configuration.loopback_origin
    ));
    // Idempotent, so that the two callers do not fight: a dedication writes
    // this to find out whether it *can*, and every launch writes it again to
    // recreate a file somebody removed. Rewriting identical bytes would replace
    // the inode a running connector is holding for no gain.
    let path = directory.join(INGRESS);
    if std::fs::read_to_string(&path).is_ok_and(|held| held == contents) {
        return Ok(());
    }
    private_write(&path, contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A settings file written before adoption existed still runs.**
    ///
    /// `connector.json` is what makes a connector start with its owner, so a
    /// shape this build cannot read is a public endpoint that silently stops
    /// coming back after an upgrade — and a connector-token file has no
    /// ownership in it to migrate, which is the whole reason the untagged form
    /// is the compatible one.
    #[test]
    fn a_connector_token_settings_file_from_before_adoption_still_reads() {
        let held = serde_json::json!({
            "httpsOrigin": "https://laplus.example.com",
            "loopbackOrigin": "http://127.0.0.1:4773",
            "executablePath": "/usr/bin/cloudflared",
            "tokenFile": "/private/connector.token",
            "desiredState": "running",
        });

        let configuration: Configuration = serde_json::from_value(held.clone()).expect("reads");
        assert_eq!(
            configuration.credential,
            RunCredential::ConnectorToken { token_file: "/private/connector.token".into() }
        );
        assert_eq!(configuration.desired_state, DesiredState::Running);
        // And writes back the same shape, so an upgrade is not a one-way door
        // for a downgrade either.
        assert_eq!(serde_json::to_value(&configuration).unwrap(), held);
    }

    /// The adopted twin, which the connector-token arm must not swallow.
    #[test]
    fn a_dedicated_tunnels_settings_file_names_its_tunnel_and_credential() {
        let held = serde_json::json!({
            "httpsOrigin": "https://spare.example.com",
            "loopbackOrigin": "http://127.0.0.1:4773",
            "executablePath": "/usr/bin/cloudflared",
            "tunnelId": "22222222-2222-2222-2222-222222222222",
            "credentialFile": "/private/tunnel.json",
            "desiredState": "stopped",
        });

        let configuration: Configuration = serde_json::from_value(held.clone()).expect("reads");
        assert_eq!(
            configuration.credential,
            RunCredential::TunnelCredential {
                tunnel_id: "22222222-2222-2222-2222-222222222222".into(),
                credential_file: "/private/tunnel.json".into(),
            }
        );
        assert_eq!(configuration.credential.file(), Path::new("/private/tunnel.json"));
        assert_eq!(serde_json::to_value(&configuration).unwrap(), held);
        // Still nothing about who owns the tunnel: that is the endpoint row's
        // answer and only the endpoint row's. `docs/adr/0049`.
        assert!(!held.to_string().contains("ownership"));
        assert!(!held.to_string().contains("adopted"));
    }

    /// laplus's configuration is laplus's, and a dedicated tunnel's carries two
    /// lines a connector-token tunnel must never be given — Cloudflare owns that
    /// one's configuration.
    #[test]
    fn only_a_dedicated_tunnel_gets_a_tunnel_and_a_credential_in_the_config() {
        let directory = tempfile::tempdir().expect("a directory");
        private_directory(directory.path()).expect("a private directory");
        let base = Configuration {
            https_origin: "https://spare.example.com".into(),
            loopback_origin: "http://127.0.0.1:4773".into(),
            executable_path: "/usr/bin/cloudflared".into(),
            credential: RunCredential::ConnectorToken { token_file: "/private/t".into() },
            desired_state: DesiredState::Running,
        };

        write_ingress(directory.path(), &base).expect("writes the token form");
        let token_form = std::fs::read_to_string(directory.path().join(INGRESS)).unwrap();
        assert!(!token_form.contains("tunnel:"), "{token_form}");
        assert!(!token_form.contains("credentials-file:"), "{token_form}");
        assert!(token_form.contains("hostname: spare.example.com"), "{token_form}");
        assert!(token_form.contains("service: http://127.0.0.1:4773"), "{token_form}");

        let dedicated = Configuration {
            credential: RunCredential::TunnelCredential {
                tunnel_id: "22222222".into(),
                credential_file: "/private/tunnel.json".into(),
            },
            ..base
        };
        write_ingress(directory.path(), &dedicated).expect("replaces it with the dedicated form");
        let dedicated_form = std::fs::read_to_string(directory.path().join(INGRESS)).unwrap();
        assert!(dedicated_form.starts_with("tunnel: 22222222\n"), "{dedicated_form}");
        assert!(
            dedicated_form.contains("credentials-file: /private/tunnel.json"),
            "{dedicated_form}"
        );
        assert!(dedicated_form.contains("hostname: spare.example.com"), "{dedicated_form}");
    }

    /// A credential file's secret is taken out of the connector's own output.
    ///
    /// A tunnel credential is a JSON document, so redacting the file's whole
    /// contents would leave the `TunnelSecret` inside it readable in any
    /// sentence that quoted only that.
    #[test]
    fn a_tunnel_credentials_secret_is_redacted_and_its_tunnel_id_is_not() {
        let held = r#"{"AccountTag":"account","TunnelID":"2222","TunnelSecret":"sekret-value"}"#;
        let secrets = secrets_within(held);
        assert!(secrets.contains(&"sekret-value".to_string()));
        assert!(secrets.contains(&held.to_string()));
        // Not every field: the tunnel id and the account tag are in the
        // snapshot already, and blanking them turns "the connector for tunnel
        // 2222 failed" into a sentence nobody can act on.
        assert!(!secrets.contains(&"2222".to_string()));
        assert!(!secrets.contains(&"account".to_string()));
        // Longest first, so a value containing another is replaced before its
        // substring is and the shorter one cannot half-redact the longer.
        assert_eq!(secrets.first().map(String::len), Some(held.len()));

        let mut spoken = "cloudflared refused sekret-value for tunnel 2222".to_string();
        for secret in secrets {
            spoken = spoken.replace(&secret, "[REDACTED]");
        }
        assert!(!spoken.contains("sekret-value"), "{spoken}");
        assert!(spoken.contains("[REDACTED]"), "{spoken}");
        assert!(spoken.contains("tunnel 2222"), "{spoken}");

        // A connector token file is the secret itself, and an empty file
        // redacts nothing rather than every gap in the sentence.
        assert_eq!(secrets_within("connector-secret\n"), ["connector-secret"]);
        assert!(secrets_within("   ").is_empty());
    }

    /// A failed write leaves nothing behind.
    ///
    /// The temporary is opened with `create_new`, so one left over makes every
    /// later write to the same path fail for a reason that has nothing to do
    /// with it — which is how a retried adoption failed a second time at a step
    /// whose cause had already been removed.
    #[test]
    fn a_failed_private_write_does_not_poison_the_next_one() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("connector.yml");
        // A file cannot be renamed over a directory, which is the same failure
        // an interrupted adoption produced.
        std::fs::create_dir(&path).expect("something in the way");

        assert!(private_write(&path, b"first").is_err());
        std::fs::remove_dir(&path).expect("the obstruction is removed");
        private_write(&path, b"second").expect("the next write succeeds");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .filter(|name| name.to_string_lossy().contains(".private-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn restart_backoff_grows_without_deciding_exhaustion() {
        assert_eq!(
            super::restart_delay(1),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            super::restart_delay(2),
            std::time::Duration::from_millis(200)
        );
        assert_eq!(
            super::restart_delay(3),
            std::time::Duration::from_millis(400)
        );
    }
}
