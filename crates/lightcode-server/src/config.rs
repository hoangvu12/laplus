//! `server.getConfig` — the payload the UI fetches before it can do anything
//! else, and therefore this project's tracer bullet.
//!
//! The types are hand-written from `t3code/packages/contracts/src/server.ts`
//! and validated against `fixtures/socket-wire/02-request-response.ndjson`.
//! They are a *blueprint*, not a mirror: the fork is free to diverge, and it
//! does — every field lightcode does not yet know is either empty or absent
//! rather than invented. `tests/socket_conformance.rs` walks the captured
//! response against a live one and requires each divergence to be declared, so
//! the list below is enforced rather than aspirational.
//!
//! What is deliberately empty at this ticket, and who fills it:
//!
//! | Field | Filled by |
//! |---|---|
//! | `providers` | ticket 09 — provider config and binary resolution |
//! | `keybindings` | ticket 22 — settings and keybindings |
//! | `availableEditors` | story 18 — open in an external editor |
//! | `settings.textGenerationModelSelection` | ticket 09, once model slugs are known |
//!
//! Two capability flags are *false rather than absent-and-assumed*: the
//! contract reads every optional capability as "unsupported when missing", and
//! advertising one we have not built would invite the UI to send commands this
//! server cannot answer.

use std::path::PathBuf;

use serde::Serialize;

/// The whole `server.getConfig` response value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub environment: EnvironmentDescriptor,
    pub auth: AuthDescriptor,
    /// The directory the server was started in. The UI shows it and uses it as
    /// the default working directory for a new project.
    pub cwd: String,
    pub keybindings_config_path: String,
    pub keybindings: Vec<ResolvedKeybinding>,
    pub issues: Vec<ConfigIssue>,
    pub providers: Vec<Provider>,
    pub available_editors: Vec<String>,
    pub observability: Observability,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDescriptor {
    /// v1 has exactly one environment and no remote ones — cloud and remote
    /// environments are out of scope — so this is a constant rather than a
    /// generated id that would have to be persisted to stay stable across
    /// restarts. The contract types it as a non-empty string, not a UUID.
    pub environment_id: String,
    pub label: String,
    pub platform: Platform,
    pub server_version: String,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Serialize)]
pub struct Platform {
    pub os: &'static str,
    pub arch: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Reporting a repository's canonical identity so threads can be grouped
    /// across checkouts. Not built; the git tickets (19–21) may.
    pub repository_identity: bool,
    /// `server.probe`, a cheap liveness call. Absent here, so the client falls
    /// back to probing with `server.getConfig` — which this ticket implements,
    /// and which is small enough for lightcode to serve on repeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_probe: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_settlement: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_snooze: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthDescriptor {
    pub policy: &'static str,
    /// How a client can *establish* trust. lightcode has no pairing flow: the
    /// handshake is permissive and local-only, so there is nothing to bootstrap
    /// through and the honest answer is none.
    pub bootstrap_methods: Vec<&'static str>,
    /// The credential shapes accepted on an established connection. All three
    /// are taken at the upgrade — see [`crate::auth`] — though none is verified.
    pub session_methods: Vec<&'static str>,
    pub session_cookie_name: &'static str,
}

/// A resolved keybinding: the compiled form the UI consumes, not the `mod+b`
/// source form.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedKeybinding {
    pub command: String,
    pub shortcut: KeybindingShortcut,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingShortcut {
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub mod_key: bool,
}

/// A problem found while assembling this config — a malformed keybindings
/// file, and nothing else so far. Surfaced in the UI rather than logged.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigIssue {
    pub kind: String,
    pub message: String,
}

/// A configured provider instance. Empty until ticket 09 resolves the `claude`
/// binary; the type is here so the field's element shape is pinned rather than
/// left to be discovered later.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub instance_id: String,
    pub driver: String,
    pub display_name: String,
    pub enabled: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub status: String,
    pub auth: ProviderAuth,
    pub checked_at: String,
    pub models: Vec<serde_json::Value>,
    pub slash_commands: Vec<serde_json::Value>,
    pub skills: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuth {
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observability {
    pub logs_directory_path: String,
    pub local_tracing_enabled: bool,
    pub otlp_traces_enabled: bool,
    pub otlp_metrics_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enable_assistant_streaming: bool,
    /// Off, where upstream defaults it on. The spec's story 7 is that the app
    /// runs entirely on the user's machine with no network service of its own;
    /// an update check on boot contradicts that. Ticket 22 can make it a
    /// user-facing choice.
    pub enable_provider_update_checks: bool,
    pub automatic_git_fetch_interval: u64,
    pub default_thread_env_mode: &'static str,
    pub new_worktrees_start_from_origin: bool,
    pub add_project_base_directory: String,
    pub providers: ProviderSettings,
    pub provider_instances: serde_json::Map<String, serde_json::Value>,
    pub observability: ObservabilitySettings,
}

/// Only the one driver v1 ships. Upstream's struct has five keys, each with a
/// decoding default, so the four lightcode does not implement are simply
/// absent rather than described as disabled.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub claude_agent: ClaudeSettings,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    pub enabled: bool,
    /// Empty means "resolve from PATH". Ticket 09 owns the resolution.
    pub binary_path: String,
    pub home_path: String,
    pub launch_args: String,
    pub custom_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySettings {
    pub otlp_traces_url: String,
    pub otlp_metrics_url: String,
}

impl ServerConfig {
    /// Assemble the config from the machine the server is running on.
    pub fn detect() -> Self {
        let data_dir = data_dir();

        ServerConfig {
            environment: EnvironmentDescriptor {
                environment_id: "local".to_string(),
                label: machine_label(),
                platform: Platform {
                    os: platform_os(),
                    arch: platform_arch(),
                },
                server_version: env!("CARGO_PKG_VERSION").to_string(),
                capabilities: Capabilities {
                    repository_identity: false,
                    connection_probe: None,
                    thread_settlement: None,
                    thread_snooze: None,
                },
            },
            auth: AuthDescriptor {
                policy: "loopback-browser",
                bootstrap_methods: Vec::new(),
                session_methods: vec![
                    "browser-session-cookie",
                    "bearer-access-token",
                    "dpop-access-token",
                ],
                session_cookie_name: crate::auth::SESSION_COOKIE_NAME,
            },
            cwd: display_path(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                ".",
            ),
            keybindings_config_path: display_path(data_dir.join("keybindings.json"), "keybindings.json"),
            keybindings: Vec::new(),
            issues: Vec::new(),
            providers: Vec::new(),
            available_editors: Vec::new(),
            observability: Observability {
                logs_directory_path: display_path(data_dir.join("logs"), "logs"),
                local_tracing_enabled: false,
                otlp_traces_enabled: false,
                otlp_metrics_enabled: false,
            },
            settings: Settings {
                enable_assistant_streaming: false,
                enable_provider_update_checks: false,
                automatic_git_fetch_interval: 30_000,
                default_thread_env_mode: "local",
                new_worktrees_start_from_origin: true,
                add_project_base_directory: String::new(),
                providers: ProviderSettings {
                    claude_agent: ClaudeSettings {
                        enabled: true,
                        binary_path: "claude".to_string(),
                        home_path: String::new(),
                        launch_args: String::new(),
                        custom_models: Vec::new(),
                    },
                },
                provider_instances: serde_json::Map::new(),
                observability: ObservabilitySettings {
                    otlp_traces_url: String::new(),
                    otlp_metrics_url: String::new(),
                },
            },
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("server config serializes")
    }
}

/// Where lightcode keeps its own files: keybindings, logs, and — since ticket
/// 05 — the SQLite database. [`crate::store::default_path`] is the one caller
/// outside this module.
pub(crate) fn data_dir() -> PathBuf {
    for variable in ["LOCALAPPDATA", "APPDATA", "XDG_DATA_HOME"] {
        if let Some(base) = non_empty_env(variable) {
            return PathBuf::from(base).join("lightcode");
        }
    }
    if let Some(home) = non_empty_env("USERPROFILE").or_else(|| non_empty_env("HOME")) {
        return PathBuf::from(home).join(".lightcode");
    }
    PathBuf::from(".lightcode")
}

fn machine_label() -> String {
    non_empty_env("COMPUTERNAME")
        .or_else(|| non_empty_env("HOSTNAME"))
        .unwrap_or_else(|| "lightcode".to_string())
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn platform_os() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "darwin",
        "linux" => "linux",
        _ => "unknown",
    }
}

fn platform_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => "other",
    }
}

/// Several config fields are typed as non-empty strings in the contract, and a
/// path that will not render as UTF-8 would encode as an empty one. Falling
/// back to a name rather than an empty string keeps the payload decodable.
fn display_path(path: PathBuf, fallback: &str) -> String {
    let rendered = path.to_string_lossy().trim().to_string();
    if rendered.is_empty() {
        fallback.to_string()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract types a good half of this payload as `TrimmedNonEmptyString`.
    /// An empty one decodes as a schema failure on the client, which the UI
    /// reports as a broken server rather than a missing field — so it is worth
    /// checking directly rather than only through the conformance walk.
    #[test]
    fn no_required_string_is_empty() {
        let config = ServerConfig::detect();
        let value = config.to_value();

        for path in [
            "/environment/environmentId",
            "/environment/label",
            "/environment/serverVersion",
            "/auth/policy",
            "/auth/sessionCookieName",
            "/cwd",
            "/keybindingsConfigPath",
            "/observability/logsDirectoryPath",
        ] {
            let found = value.pointer(path).unwrap_or_else(|| panic!("{path} is present"));
            let text = found
                .as_str()
                .unwrap_or_else(|| panic!("{path} is a string, got {found}"));
            assert_eq!(text, text.trim(), "{path} is trimmed");
            assert!(!text.is_empty(), "{path} is non-empty");
        }
    }

    /// The descriptor names the cookie the UI should send and [`crate::auth`]
    /// reads it back. If they ever disagree the browser would be told to
    /// present a credential the server does not look for.
    #[test]
    fn the_auth_descriptor_names_the_cookie_the_handshake_reads() {
        let config = ServerConfig::detect();
        assert_eq!(
            config.auth.session_cookie_name,
            crate::auth::SESSION_COOKIE_NAME
        );
    }

    /// Advertising a capability the server has not built invites the UI to
    /// send commands nothing answers. Every flag here is off until its ticket
    /// lands.
    #[test]
    fn no_unbuilt_capability_is_advertised() {
        let capabilities = ServerConfig::detect().environment.capabilities;
        assert!(!capabilities.repository_identity);
        assert_eq!(capabilities.connection_probe, None);
        assert_eq!(capabilities.thread_settlement, None);
        assert_eq!(capabilities.thread_snooze, None);
    }

    #[test]
    fn absent_capabilities_are_omitted_rather_than_serialized_as_null() {
        let value = ServerConfig::detect().to_value();
        let capabilities = &value["environment"]["capabilities"];
        assert_eq!(capabilities["repositoryIdentity"], serde_json::json!(false));
        assert!(capabilities.get("connectionProbe").is_none());
        assert!(capabilities.get("threadSettlement").is_none());
        assert!(capabilities.get("threadSnooze").is_none());
    }
}
