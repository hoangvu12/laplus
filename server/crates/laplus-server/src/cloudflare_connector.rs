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

/// What a connector needs to run, and nothing about who owns its tunnel.
///
/// **Ownership deliberately lives in one place, and it is not here.** This file
/// used to carry `ownership: "adopted"`, written unconditionally and never read
/// back; making it a real value would have left two records of one fact, and a
/// boot that restores the endpoint row from this file would then decide which
/// of them wins. The durable answer is the `public_exposure_endpoint` row —
/// `docs/adr/0049` — and `managed_connector_snapshot` in `server.rs` reads it
/// from there the same way it reads verification state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    pub https_origin: String,
    pub loopback_origin: String,
    pub executable_path: PathBuf,
    pub token_file: PathBuf,
    pub desired_state: DesiredState,
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
        let https_origin = crate::public_exposure::normalize_hostname(hostname)
            .map_err(|message| Refusal::rejected(RefusalReason::HostnameInvalid, message))?;
        let token_file = self.directory.join(TOKEN);
        if connector_token.trim().is_empty() && !token_file.is_file() {
            return Err(Refusal::rejected(
                RefusalReason::ConnectorRequired,
                "Enter the tunnel-specific connector token.",
            ));
        }
        let executable = Search::from_environment().startable(executable).ok_or_else(|| {
            Refusal::rejected(
                RefusalReason::ExecutableUnusable,
                "The selected cloudflared executable cannot be started.",
            )
        })?;
        let version = compatible_version(&executable).await?;
        let loopback_origin = self.owner_origin.lock().unwrap().clone().ok_or_else(|| {
            Refusal::rejected(
                RefusalReason::ConnectorRequired,
                "The connector owner has not finished starting.",
            )
        })?;
        private_directory(&self.directory)?;
        if !connector_token.trim().is_empty() {
            private_write(&token_file, connector_token.as_bytes())?;
        }
        let configuration = Configuration {
            https_origin,
            loopback_origin,
            executable_path: executable,
            token_file,
            desired_state: DesiredState::Running,
        };
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
            return json!({
                "configured": false, "ownership": CONNECTOR_OWNERSHIP,
                "desiredState": DesiredState::Stopped,
                "connectorState": "unconfigured", "readiness": null, "httpsOrigin": null,
                "executablePath": null, "detectedVersion": null, "metricsOrigin": null,
                "failureMessage": null, "restartCount": 0, "logs": []
            });
        };
        json!({
            "configured": true,
            "ownership": CONNECTOR_OWNERSHIP,
            "desiredState": configuration.desired_state,
            "connectorState": runtime.connector_state,
            "readiness": runtime.readiness,
            "httpsOrigin": configuration.https_origin,
            "loopbackOrigin": configuration.loopback_origin,
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
            let mut command = tokio::process::Command::new(&configuration.executable_path);
            command
                .args([
                    "tunnel",
                    "--config",
                    &config_file.to_string_lossy(),
                    "--token-file",
                    &configuration.token_file.to_string_lossy(),
                    "--metrics",
                    &metrics,
                    "run",
                ])
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

    fn record_log(&self, text: &str, configuration: &Configuration) {
        let token = std::fs::read_to_string(&configuration.token_file).unwrap_or_default();
        let redacted = text.replace(token.trim(), "[REDACTED]");
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

/// Everything that can fail while laplus writes its own private files says so
/// the same way: nothing at Cloudflare went wrong, and the retry is local.
fn local_setup(what: &str) -> Refusal {
    Refusal::rejected(
        RefusalReason::LocalSetupFailed,
        format!("The private connector {what}."),
    )
}

pub(crate) fn private_write(path: &Path, bytes: &[u8]) -> Result<(), Refusal> {
    let temporary = path.with_file_name(format!(
        "{}.private-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("connector"),
        std::process::id(),
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| local_setup("credential could not be written"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| local_setup("credential could not be written"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| local_setup("credential could not be protected"))?;
    }
    #[cfg(windows)]
    protect_windows(&temporary, false)?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path).map_err(|_| local_setup("credential could not be replaced"))?;
    }
    std::fs::rename(&temporary, path)
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

fn write_ingress(directory: &Path, configuration: &Configuration) -> Result<(), Refusal> {
    let contents = format!(
        "ingress:\n  - hostname: {}\n    service: {}\n  - service: http_status:404\n",
        configuration.https_origin.trim_start_matches("https://"),
        configuration.loopback_origin
    );
    private_write(&directory.join(INGRESS), contents.as_bytes())
}

#[cfg(test)]
mod tests {
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
