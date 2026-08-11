//! `server.getConfig` — the payload the UI fetches before it can do anything
//! else, and therefore this project's tracer bullet.
//!
//! The types are hand-written from `t3code/packages/contracts/src/server.ts`
//! and validated against `fixtures/socket-wire/02-request-response.ndjson`.
//! They are a *blueprint*, not a mirror: the fork is free to diverge, and it
//! does — every field laplus does not yet know is either empty or absent
//! rather than invented. `tests/socket_conformance.rs` walks the captured
//! response against a live one and requires each divergence to be declared, so
//! the list below is enforced rather than aspirational.
//!
//! What was deliberately empty here is now filled, and by two modules rather
//! than this one: [`crate::keybindings`] compiles `keybindings` and the issues
//! found reading them, and [`crate::settings`] reads `settings` off the disk
//! over the defaults below. Both are I/O and this file is not, which is the
//! whole of why they are elsewhere — what stays here is what a setting *is* and
//! what one is worth when nobody has chosen.
//!
//! `providers` is the one that is still empty, and its emptiness is a *state*
//! rather than a gap:
//! ticket 09 fills it, but from [`crate::provider::refresh`] rather than from
//! here. Assembling this payload starts no child process, so the first
//! `server.getConfig` is answered without waiting on an agent that may not exist,
//! and the UI renders "Checking provider status" — which is what upstream's own
//! `getProviderSummary` does for an absent instance — until the answer arrives
//! through the change feed.
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
    /// Where this server keeps the developer's own files — `keybindings.json`,
    /// `settings.json`, the logs and the registry.
    ///
    /// **Not on the wire**, which is why it is skipped rather than named in
    /// camel case: the contract has no such field, and
    /// `tests/socket_conformance.rs` would report it as an undeclared addition.
    /// It is here rather than passed beside the config because the two things
    /// that *are* on the wire — `keybindingsConfigPath` and the logs directory —
    /// are derived from it, and a server whose advertised path and real path
    /// could differ would send the developer to edit a file nothing reads.
    #[serde(skip)]
    pub preferences: PathBuf,
    /// Where this server listens, and therefore whether anything but this
    /// machine can reach it.
    ///
    /// **Not on the wire**, and skipped for the same reason `preferences` is:
    /// the contract has no such field and `tests/socket_conformance.rs` reports
    /// an undeclared addition as a break. It rides on the config because this is
    /// the object assembled from the machine at startup, and the switch is one
    /// more thing read out of the preferences directory.
    ///
    /// See [`crate::remote_access`], which is where the reasoning lives.
    #[serde(skip)]
    pub remote_access: crate::remote_access::RemoteAccess,
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
    /// What a client files this server under, shaped `<machine>-<suffix>` —
    /// `desktop-19eumeb-8f2a`. The contract types it as a non-empty string, not
    /// a UUID.
    ///
    /// **Generated once per data directory and persisted**, which it has to be
    /// twice over: the client's connection registry is one slot per environment
    /// id, so two laplus servers answering the same one means the second is
    /// dropped on arrival; and a client stores its bearer profile under this
    /// name, so an id minted fresh each boot would un-pair every client on every
    /// restart. [`crate::store::Database::environment_id_or_create`] mints and
    /// keeps it, [`ServerConfig::with_environment_id`] is how it arrives here,
    /// and [`fresh_environment_id`] is the shape.
    ///
    /// This was the constant `"local"` until ticket 06 of the headless-Linux
    /// effort. The comment here argued that "v1 has exactly one environment and
    /// no remote ones", which was true when it was written and stopped being
    /// true when ticket 02 answered a cross-origin request: the desktop
    /// application then walked the whole pairing chain against a second laplus
    /// and had nowhere to put the result. The symptom was not an error but an
    /// empty "Remote environments" list after a pairing that succeeded at every
    /// step.
    pub environment_id: String,
    pub label: String,
    pub platform: Platform,
    /// What the client compares its own `APP_VERSION` against. This crate's
    /// version by default, and the shipped UI's when there is one — see
    /// [`ServerConfig::serving_ui_version`], which is the whole of that story.
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
    /// `server.probe`, a cheap liveness call — [`crate::rpc::SERVER_PROBE`].
    ///
    /// **This flag and the method ship together, in that order**, and the order
    /// is not a preference: `session.ts` reads an `EnvironmentAuthorizationError`
    /// from `server.probe` as `ConnectionBlockedError`, a connection refused on
    /// permission and not retried. Advertising this while the method was still
    /// refused would have blocked every connection rather than degraded one
    /// probe. `crate::refusals` carries the same warning from the other side.
    ///
    /// What it changes: the client stops re-sending `server.getConfig` to prove
    /// the socket is alive, so a liveness check costs an empty round trip
    /// instead of the whole config payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_probe: Option<bool>,
    /// `thread.settle` and `thread.unsettle`, which ticket 07 of the
    /// thread-lifecycle effort answers.
    ///
    /// **The controls are gated on this**, which is why the flag is part of that
    /// ticket rather than a note beside it: `useThreadActions.ts` refuses to
    /// dispatch either command to a server that does not advertise it, and the
    /// sidebar and the chat view hide the menu items entirely
    /// (`SidebarV2.tsx`, `ChatView.tsx`). A server that answered both commands and
    /// left this absent would have built two commands nothing sends.
    ///
    /// **It also switches on the client's own inactivity auto-settle**, which is
    /// the part worth knowing before flipping it: `SidebarV2.tsx` reads the same
    /// flag before letting `effectiveSettled` classify a thread at all, so a
    /// conversation nobody has touched for `autoSettleAfterDays` now leaves the
    /// inbox by itself. That derivation is the client's and ships unmodified
    /// (ADR-0012), and its premise — that the server un-settles on real
    /// activity — is now true: [`crate::threads::Change::wakes`] is the
    /// three resets, so an auto-settled conversation comes back the moment there
    /// is work in it again rather than staying gone until the developer opens it.
    ///
    /// This is the whole of what advertising it costs, and the alternative was
    /// shipping the commands with no control that sends them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_settlement: Option<bool>,
    /// `thread.snooze` and `thread.unsnooze`, which ticket 09 of the
    /// thread-lifecycle effort answers.
    ///
    /// **The controls are gated on this** exactly as
    /// [`Capabilities::thread_settlement`]'s are, and for the same reason it is
    /// part of the ticket rather than a note beside it: `useThreadActions.ts`
    /// refuses to dispatch either command to a server that does not advertise it,
    /// so a server that answered both and left this absent would have built two
    /// commands nothing sends.
    ///
    /// What it switches on beyond the menu items is the sidebar's **snoozed
    /// section** and its "Woke" indicator, both of which are drawn from
    /// derivations that ship in the client (`effectiveSnoozed`, `threadWokeAt`)
    /// and read the two fields this server now stores. There is no premise here
    /// waiting on a later ticket, which is where this differs from settlement:
    /// a snooze expires by being read, so nothing has to happen for one to end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_snooze: Option<bool>,
    /// `thread.pin` and `thread.unpin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_pinning: Option<bool>,
    /// Manual ordering through `thread.pin.reorder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_pin_reorder: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_title_regeneration: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthDescriptor {
    pub policy: &'static str,
    /// How a client can *establish* trust.
    ///
    /// Empty until ticket 73, which is when there was a pairing flow to name.
    /// It says `one-time-token` and not `desktop-bootstrap`: the second is
    /// upstream's trusted hand-off from an Electron main process to its
    /// renderer, and laplus's window and server are one process reaching each
    /// other over loopback, so there is nothing to hand off. See
    /// [`crate::pairing::ONE_TIME_TOKEN_METHOD`].
    ///
    /// The only reader is `PairingRouteSurface`, the `/pair` page a client sees
    /// when it is *not* yet paired — so this describes the way in for a phone
    /// and never for the window.
    pub bootstrap_methods: Vec<&'static str>,
    /// The credential shapes accepted on an established connection. All three
    /// are taken at the upgrade — see [`crate::auth`] — though none is verified.
    pub session_methods: Vec<&'static str>,
    pub session_cookie_name: &'static str,
}

/// A resolved keybinding: the compiled form the UI consumes, not the `mod+b`
/// source form. [`crate::keybindings`] is what turns one into the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedKeybinding {
    pub command: String,
    pub shortcut: KeybindingShortcut,
    /// The parsed `when` expression, for a binding that has one.
    ///
    /// **Absent rather than null** when the binding is unconditional: the
    /// contract types it `Schema.optional`, and a `null` would decode as a
    /// `when` whose type is neither of the four the union allows — which costs
    /// the whole keybindings array, not the one rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_ast: Option<crate::keybindings::WhenNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeybindingShortcut {
    pub key: String,
    pub meta_key: bool,
    pub ctrl_key: bool,
    pub shift_key: bool,
    pub alt_key: bool,
    pub mod_key: bool,
}

/// A problem found while assembling this config. Surfaced in the UI rather than
/// logged, because it is a thing the developer can go and fix.
///
/// **`kind` is one of two literals, not a label.** `ServerConfigIssue` in
/// `server.ts` is a closed union — `keybindings.malformed-config` and
/// `keybindings.invalid-entry`, the second carrying the entry's `index` — and
/// `ServerConfig.issues` is an array of it. So a `kind` of this server's own
/// invention would not be an oddly-named row: it would fail the client's decode
/// of the **whole `server.getConfig` payload**, and the app would not open at
/// all. On a broken keybindings file. Which is the one case this field exists
/// for.
///
/// [`crate::keybindings`] is the only thing that builds one, and that follows
/// from the same fact: there is no member of the union for a settings problem,
/// so [`crate::settings`] logs instead. See its `load`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigIssue {
    pub kind: &'static str,
    pub message: String,
    /// Which entry of the file was refused — required by
    /// `keybindings.invalid-entry` and absent from the other member, so it is
    /// skipped rather than sent as null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// A configured provider instance: what the UI shows in its picker and routes a
/// turn through. [`crate::provider`] builds them; this is only their shape.
///
/// Every field the reference server sends and this does not is declared in
/// `tests/socket_conformance.rs` with the ticket that owns it, so the list of
/// omissions is enforced rather than remembered.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub instance_id: String,
    pub driver: String,
    pub display_name: String,
    pub enabled: bool,
    pub installed: bool,
    /// `null` rather than absent when unknown: the contract types this as
    /// `NullOr`, and a missing key would decode as a schema failure.
    pub version: Option<String>,
    pub status: ProviderState,
    /// What the developer needs to know, when there is something. Optional in
    /// the contract, so absent — not an empty string — when a provider is simply
    /// working; the UI substitutes its own phrasing per state and would render an
    /// empty sentence as a blank line under the provider's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub auth: ProviderAuth,
    pub checked_at: String,
    pub models: Vec<ProviderModel>,
    pub slash_commands: Vec<serde_json::Value>,
    pub skills: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_advisory: Option<ProviderVersionAdvisory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_state: Option<ProviderUpdateState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderVersionAdvisory {
    pub status: ProviderVersionAdvisoryStatus,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_command: Option<String>,
    pub can_update: bool,
    pub checked_at: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVersionAdvisoryStatus { Unknown, Current, BehindLatest }

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdateState {
    pub status: ProviderUpdateStatus,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub message: Option<String>,
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUpdateStatus { Idle, Queued, Running, Succeeded, Failed, Unchanged }

/// How the UI should present a provider instance. The contract's
/// `ServerProviderState`, as a closed set rather than a string, because these
/// four literals are what the client dispatches on: `ready` fills the model
/// picker, `warning` and `error` raise a banner, `disabled` hides the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderState {
    Ready,
    Warning,
    Error,
    Disabled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAuth {
    pub status: AuthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Whether the agent has credentials. `Unknown` is the contract's own literal
/// for "nothing looked", which is the honest answer until a ticket reads one —
/// see [`crate::provider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthStatus {
    Authenticated,
    Unauthenticated,
    Unknown,
}

/// One model the UI may offer for this provider.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    /// What the CLI is given as `--model`.
    pub slug: String,
    pub name: String,
    pub is_custom: bool,
    /// Who serves this model behind the driver, when the driver is a front for
    /// somebody else. OpenCode is: `openai/gpt-5` and `siliconflow/Qwen/Qwen3.5-27B`
    /// arrive from the same instance and the picker has nothing but this to tell
    /// them apart — two upstreams routinely name the same model identically, and
    /// the model search indexes this field rather than the slug. Absent for the
    /// drivers that answer for themselves.
    ///
    /// **Omitted rather than null when there is none**: the contract types it
    /// `Schema.optional`, where absent decodes and null does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// The reasoning-effort, fast-mode and context-window toggles the composer
    /// shows for this model. Claude has none; Codex carries each live model's
    /// supported reasoning efforts so a later turn can send the chosen value.
    pub capabilities: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observability {
    pub logs_directory_path: String,
    pub local_tracing_enabled: bool,
    pub otlp_traces_enabled: bool,
    pub otlp_metrics_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// On, where upstream defaults it off.
    ///
    /// This is a report of what the server does rather than a switch over it —
    /// nothing can write a setting until ticket 22 — and what this server does
    /// is publish assistant text as it arrives. Upstream's `false` means its
    /// ingestion buffers up to 24,000 characters before sending anything, which
    /// is a reply that appears in one jump at the end; ticket 10's second
    /// criterion rules that out. Reporting `false` while streaming anyway would
    /// be the payload disagreeing with the wire beside it.
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
    /// Which model generates text the developer did not ask for — a thread
    /// title, a commit message.
    ///
    /// **Sent rather than left absent, and that is ticket 22's doing.** The
    /// contract's decoding default is `{instanceId: "codex", model:
    /// "gpt-5.6-luna"}`, so a client filling it in for itself would name an
    /// instance and a model v1 does not ship — and the first feature to want
    /// generated text would ask a provider that is not there. Naming the one
    /// instance this server has costs nothing and cannot be wrong.
    ///
    /// Nothing reads it yet: thread titles and commit messages are later
    /// tickets. It is stored because a settings panel that forgets what the
    /// developer chose is worse than one with a control that is not wired up.
    ///
    /// JSON rather than a struct for the reason [`crate::threads`] keeps a
    /// thread's own selection as JSON: nothing here reads into it, so a mirrored
    /// shape would be one more thing to keep in step for no query it enables.
    pub text_generation_model_selection: serde_json::Value,
    pub providers: ProviderSettings,
    pub provider_instances: serde_json::Map<String, serde_json::Value>,
    pub observability: ObservabilitySettings,
}

/// Settings for the drivers this server knows. A settings section may precede
/// the driver that reads it so a developer's choices survive that rollout.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    pub claude_agent: ClaudeSettings,
    pub codex: CodexSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettings {
    pub enabled: bool,
    /// Where the agent binary is, or what it is called. A value containing a
    /// path separator is a *place*; anything else — including the default
    /// `claude` and the empty string — is a name to look up on `PATH`. See
    /// [`crate::provider::resolve`], which owns the rule.
    pub binary_path: String,
    pub home_path: String,
    pub launch_args: String,
    pub custom_models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSettings {
    pub enabled: bool,
    pub binary_path: String,
    /// `CODEX_HOME`, separate from the process home directory.
    pub home_path: String,
    pub launch_args: String,
    pub custom_models: Vec<String>,
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeSettings {
    pub enabled: bool,
    pub binary_path: String,
    pub server_url: String,
    pub server_password: String,
    pub custom_models: Vec<String>,
}

impl std::fmt::Debug for OpenCodeSettings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenCodeSettings")
            .field("enabled", &self.enabled)
            .field("binary_path", &self.binary_path)
            .field("server_url", &self.server_url)
            .field("server_password", &"[redacted]")
            .field("custom_models", &self.custom_models)
            .finish()
    }
}

impl ClaudeSettings {
    pub(crate) fn instance_envelope(&self, display_name: &str) -> serde_json::Value {
        provider_instance_envelope(
            crate::provider::CLAUDE_DRIVER,
            display_name,
            self.enabled,
            &self.binary_path,
            &self.home_path,
            &self.launch_args,
            &self.custom_models,
        )
    }
}

impl CodexSettings {
    pub(crate) fn instance_envelope(&self, display_name: &str) -> serde_json::Value {
        provider_instance_envelope(
            crate::provider::CODEX_DRIVER,
            display_name,
            self.enabled,
            &self.binary_path,
            &self.home_path,
            &self.launch_args,
            &self.custom_models,
        )
    }
}

fn provider_instance_envelope(
    driver: &str,
    display_name: &str,
    enabled: bool,
    binary_path: &str,
    home_path: &str,
    launch_args: &str,
    custom_models: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "driver": driver,
        "displayName": display_name,
        "enabled": enabled,
        "config": {
            "binaryPath": binary_path,
            "homePath": home_path,
            "launchArgs": launch_args,
            "customModels": custom_models,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilitySettings {
    pub otlp_traces_url: String,
    pub otlp_metrics_url: String,
}

impl ServerConfig {
    /// Assemble the config from the machine the server is running on.
    ///
    /// Not free — `available_editors` stats every candidate command against every
    /// `PATH` entry — but it starts **no child process**, which is the property
    /// that matters: this is called before the listener exists, and a server whose
    /// startup waited on the agent answering would not start at all on a machine
    /// where the agent is wedged. See [`crate::provider::refresh`], which is the
    /// part that does wait, off the startup path.
    pub fn detect() -> Self {
        ServerConfig::detect_in(data_dir())
    }

    /// The same, keeping the developer's files somewhere the caller chose.
    ///
    /// The seam the suite uses, and it is not test-only for the same reason
    /// [`crate::Server::bind_with`] is not: ticket 23's shell wants to decide
    /// where the app's state lives. It has to be an argument rather than an
    /// environment variable, because `LOCALAPPDATA` is process-global mutable
    /// state — a test that set it would be setting it for every test running
    /// beside it, and a suite that wrote to the developer's real configuration
    /// would be a suite nobody could run twice.
    pub fn detect_in(data_dir: PathBuf) -> Self {
        let remote_access = crate::remote_access::RemoteAccess::load(&data_dir);
        // Settled from the *bind host*, which is what upstream does
        // (`EnvironmentAuthPolicy.ts:16-24`, via `auth/utils.ts:53`). An
        // earlier version read the tunnel hostname list instead, on the
        // reasoning that laplus always binds loopback so the host could never
        // say `remote-reachable`. That stopped being true when the exposure
        // switch landed: `network-accessible` binds `0.0.0.0`, and the host
        // answers the question directly again.
        //
        // Reading the address rather than the file is also what keeps this
        // honest. A `&'static str` settled at startup cannot track a file the
        // user edits while the server runs, and it did not — a hostname added
        // from Settings left this reporting `loopback-browser` until the next
        // restart, which hides the whole "Authorized clients" section and with
        // it the only button that mints a pairing code
        // (`ConnectionsSettings.tsx:3022`). The bound address has no such
        // problem: a socket cannot move under a running server, so what is
        // read here at startup is still true for as long as the value lives.
        let policy = policy_for(&remote_access);
        let claude_settings = ClaudeSettings {
            enabled: true,
            binary_path: "claude".to_string(),
            home_path: String::new(),
            launch_args: String::new(),
            custom_models: Vec::new(),
        };
        let codex_settings = CodexSettings {
            enabled: true,
            binary_path: "codex".to_string(),
            home_path: String::new(),
            launch_args: String::new(),
            custom_models: Vec::new(),
        };
        let provider_instances = serde_json::Map::from_iter([
            (
                crate::provider::CLAUDE_INSTANCE_ID.to_string(),
                claude_settings.instance_envelope("Claude"),
            ),
            (
                crate::provider::CODEX_INSTANCE_ID.to_string(),
                codex_settings.instance_envelope("Codex"),
            ),
        ]);
        ServerConfig {
            remote_access,
            preferences: data_dir.clone(),
            environment: EnvironmentDescriptor {
                // Minted rather than read, and replaced at bind: the durable one
                // lives in `state.sqlite` and this constructor has no database.
                // [`ServerConfig::with_environment_id`] is the other half, and
                // `server_version` two fields down has exactly this shape
                // already — the crate's answer here, the bundle's at bind.
                //
                // A fresh id rather than an empty string because a config that
                // is never bound still has to serialize: the contract types this
                // as a non-empty string, and `no_required_string_is_empty` walks
                // `/environment/environmentId` for that reason. An empty one
                // decodes as a schema failure the UI reports as a broken server.
                //
                // Randomness failing leaves the machine's slug alone, which is
                // legal, non-empty, and stable enough for the only thing that
                // reaches this path without a database — a config assembled and
                // serialized inside one process. See
                // [`crate::pairing::RandomError`] for why this is not a panic.
                environment_id: fresh_environment_id().unwrap_or_else(|_| machine_slug()),
                label: machine_label(),
                platform: Platform {
                    os: platform_os(),
                    arch: platform_arch(),
                },
                server_version: crate::version::PRODUCT_VERSION.to_string(),
                capabilities: Capabilities {
                    repository_identity: false,
                    connection_probe: Some(true),
                    thread_settlement: Some(true),
                    thread_snooze: Some(true),
                    thread_pinning: Some(true),
                    thread_pin_reorder: Some(true),
                    thread_title_regeneration: Some(true),
                },
            },
            auth: AuthDescriptor {
                policy,
                bootstrap_methods: vec![crate::pairing::ONE_TIME_TOKEN_METHOD],
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
            keybindings_config_path: display_path(
                data_dir.join("keybindings.json"),
                "keybindings.json",
            ),
            keybindings: Vec::new(),
            issues: Vec::new(),
            // Empty until something has looked. See the module docs.
            providers: Vec::new(),
            available_editors: crate::editor::available(),
            observability: Observability {
                logs_directory_path: display_path(data_dir.join(LOGS), LOGS),
                local_tracing_enabled: false,
                otlp_traces_enabled: false,
                otlp_metrics_enabled: false,
            },
            settings: Settings {
                enable_assistant_streaming: true,
                enable_provider_update_checks: false,
                automatic_git_fetch_interval: 30_000,
                default_thread_env_mode: "local",
                new_worktrees_start_from_origin: true,
                add_project_base_directory: String::new(),
                // The cheapest model this server offers, because the work is
                // a thread title rather than the developer's own turn.
                text_generation_model_selection: serde_json::json!({
                    "instanceId": crate::provider::CLAUDE_INSTANCE_ID,
                    "model": "claude-haiku-4-5",
                }),
                providers: ProviderSettings {
                    claude_agent: claude_settings,
                    codex: codex_settings,
                },
                provider_instances,
                observability: ObservabilitySettings {
                    otlp_traces_url: String::new(),
                    otlp_metrics_url: String::new(),
                },
            },
        }
    }

    /// Report the version of the UI this server ships, in place of the server
    /// crate's own.
    ///
    /// **This does not make a version check pass; it removes one.** The UI
    /// compares `APP_VERSION` — the number baked into its bundle at build time
    /// — against `environment.serverVersion` by string equality, and shows a
    /// banner offering to relaunch the server when they differ. That check is
    /// for upstream's shape, where a long-running server is talked to by a
    /// browser holding whatever the last deploy gave it. Here the server *is*
    /// the UI's container: they are one executable, built together, and there
    /// is no arrangement of the two that could produce a real skew. Reporting
    /// `env!("CARGO_PKG_VERSION")` meant advertising `0.1.0` at a client
    /// certain it was `0.0.28`, and every launch opened on advice — relaunch
    /// the server to sync them — that named no action the developer could take.
    ///
    /// So the two numbers are made equal because they describe one artifact,
    /// and the client's comparison is left with nothing to find. Anyone reading
    /// a passing check here as *evidence* of anything is reading it wrongly:
    /// ticket 26 is where the reasoning is, and the honest summary is that the
    /// check is vestigial in laplus rather than satisfied by it.
    ///
    /// A server with no bundle — the plain `laplus-server` binary, which is
    /// pointed at by a development server or a browser this project did not
    /// build — never calls this and keeps the crate version, which is the true
    /// answer for a server that is not claiming to match any particular UI.
    ///
    /// See `docs/adr/0011-the-server-reports-the-version-of-the-ui-it-ships.md`.
    pub fn serving_ui_version(mut self, version: &str) -> ServerConfig {
        self.environment.server_version = version.to_string();
        self
    }

    /// Report the durable name this data directory was given, in place of the
    /// one [`ServerConfig::detect_in`] minted.
    ///
    /// **The same shape as [`ServerConfig::serving_ui_version`] above, for the
    /// same reason.** `detect` assembles the config from the machine and has no
    /// database to ask; [`crate::Server::bind_with`] has both in one hand. So the
    /// constructor mints a value that is legal on its own and the binder replaces
    /// it with the one that survives a restart — which is where the id has to
    /// come from, because a client files this server under it and comes back
    /// tomorrow expecting to find it.
    ///
    /// Ticket 06 of the headless-Linux effort;
    /// [`crate::store::Database::environment_id_or_create`] is what produces the
    /// argument.
    pub fn with_environment_id(mut self, environment_id: String) -> ServerConfig {
        self.environment.environment_id = environment_id;
        self
    }

    /// Change where this server is willing to be reached from.
    ///
    /// **The policy travels with it**, which is the whole reason this is a
    /// method rather than an assignment to a public field. `auth.policy` is
    /// *derived* from the bind address — see [`ServerConfig::detect_in`] — so a
    /// caller that set `remote_access` on its own would leave the descriptor
    /// describing the reachability the config no longer has. That is not
    /// hypothetical: the test harness did exactly that, and a server bound to
    /// loopback went on advertising `remote-reachable` to every client that
    /// asked. Two fields, one decision, one place that can make it.
    pub fn with_remote_access(
        mut self,
        remote_access: crate::remote_access::RemoteAccess,
    ) -> ServerConfig {
        self.auth.policy = policy_for(&remote_access);
        self.remote_access = remote_access;
        self
    }

    pub fn to_value(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("server config serializes");
        value["settings"] = crate::settings::public_value(&self.settings);
        value
    }
}

/// The directory name the developer is pointed at for logs, inside the
/// preferences directory. One spelling, because the config tells the UI where it
/// is and the shell writes into it.
const LOGS: &str = "logs";

/// Where laplus's logs go on this machine.
///
/// The same directory `observability.logsDirectoryPath` names in the payload,
/// resolved without a running server — which is what the shell needs, because
/// the failures it has to report are the ones that happen instead of a server.
pub fn logs_dir() -> PathBuf {
    data_dir().join(LOGS)
}

/// Where laplus keeps its own files: keybindings, logs, and — since ticket
/// 05 — the SQLite database. [`crate::store::default_path`] is the one caller
/// outside this module.
pub(crate) fn data_dir() -> PathBuf {
    for variable in ["LOCALAPPDATA", "APPDATA", "XDG_DATA_HOME"] {
        if let Some(base) = non_empty_env(variable) {
            return PathBuf::from(base).join("laplus");
        }
    }
    if let Some(home) = non_empty_env("USERPROFILE").or_else(|| non_empty_env("HOME")) {
        return PathBuf::from(home).join(".laplus");
    }
    PathBuf::from(".laplus")
}

/// Whether a client reaching this server could have come from another machine,
/// which is what the auth descriptor's `policy` reports.
///
/// Settled from the *bind host* — [`ServerConfig::detect_in`] carries the full
/// reasoning, and [`ServerConfig::with_remote_access`] is why it is a function
/// rather than four lines inside that constructor: two callers settling this
/// two ways is the drift the method exists to prevent.
fn policy_for(remote_access: &crate::remote_access::RemoteAccess) -> &'static str {
    if crate::remote_access::is_remote_reachable_host(remote_access.bind_address()) {
        "remote-reachable"
    } else {
        "loopback-browser"
    }
}

/// What this machine calls itself, for the `environment.label` the UI shows
/// beside the connection.
///
/// Three sources, in the order of how much each knows about *this* box.
/// `COMPUTERNAME` is set by Windows for every process. `HOSTNAME` is asked
/// second and is usually absent off Windows — bash sets it as a *shell*
/// variable and does not export it, so a server started by systemd or over
/// `ssh host cmd` never sees it. (A container is the exception: Docker exports
/// `HOSTNAME` to the process, so the second source answers there and the third
/// is not reached.) That left `/etc/hostname`, added by ticket 05 of the
/// headless-Linux effort: without it a headless laplus answered `"laplus"`, and
/// a user pairing a phone against three of them would have been shown one name
/// three times.
fn machine_label() -> String {
    non_empty_env("COMPUTERNAME")
        .or_else(|| non_empty_env("HOSTNAME"))
        .or_else(configured_hostname)
        .unwrap_or_else(|| "laplus".to_string())
}

/// How much of a machine's name an environment id carries.
///
/// Twenty-eight characters. A fully-qualified cloud hostname —
/// `ip-10-0-1-42.eu-west-1.compute.internal` — is over twice that and nobody
/// reads the end of it; what the prefix is for is recognising *which* box, which
/// the front of a name does. The id is a route segment
/// (`_chat.$environmentId.$threadId`) and appears in a settings list, so the cap
/// is about both being usable and being read.
const MACHINE_SLUG_LIMIT: usize = 28;

/// A name for a laplus that does not have one yet: this machine's slug, a dash,
/// and a short random suffix — `desktop-19eumeb-8f2a`.
///
/// **Minted here and made durable elsewhere.** The one place an id is *kept* is
/// [`crate::store::Database::environment_id_or_create`], which is the only
/// caller that persists what this returns; [`ServerConfig::detect_in`] calls it
/// too, for a value that lives as long as one unbound config. Both go through
/// this function so that the shape has one definition — an id read from the
/// database and an id minted by a config that was never bound should not be
/// distinguishable, because a reader of a log line cannot tell which one they
/// are looking at.
///
/// Ticket 06 of the headless-Linux effort is where `<machine>-<suffix>` and the
/// argument for a legible id over an opaque one are written down.
pub(crate) fn fresh_environment_id() -> Result<String, crate::pairing::RandomError> {
    Ok(format!(
        "{}-{}",
        machine_slug(),
        crate::pairing::identifier_suffix()?
    ))
}

/// The prefix half of this laplus's environment id: what
/// [`machine_label`] answers, in a form a URL can carry.
///
/// **The label is what the UI shows and this is not it.** Two machines with the
/// same hostname show the same label, and that stays true — they are told apart
/// by the suffix [`crate::pairing::identifier_suffix`] adds, not by this. See
/// ticket 06 of the headless-Linux effort, which is where the shape
/// `<machine>-<suffix>` and the reasons for it are written down.
///
/// A machine whose name slugs to nothing falls back to the same `"laplus"` the
/// label does, so the id is still legal and still says what product it belongs
/// to.
pub(crate) fn machine_slug() -> String {
    slug_of(&machine_label()).unwrap_or_else(|| "laplus".to_string())
}

/// [`machine_slug`]'s parsing, separated from the machine so it can be checked
/// without setting a process-global environment variable — the split
/// [`hostname_in`] already makes, for the reason its own comment gives.
///
/// [`None`] rather than an empty string for a name with nothing keepable in it:
/// `""` would be a broken id the client reports as a broken server, while
/// [`None`] is a machine that did not usefully say and leaves the caller its
/// fallback.
fn slug_of(name: &str) -> Option<String> {
    let mut slug = String::with_capacity(name.len().min(MACHINE_SLUG_LIMIT));
    for character in name.chars() {
        if slug.len() == MACHINE_SLUG_LIMIT {
            break;
        }
        match character.to_ascii_lowercase() {
            kept @ ('a'..='z' | '0'..='9') => slug.push(kept),
            // One dash per run of anything else, which is what turns
            // `Ada's  Laptop` into `ada-s-laptop` rather than `ada-s--laptop`.
            // A leading run pushes nothing, so the trim below only ever has the
            // tail to deal with.
            _ if slug.ends_with('-') || slug.is_empty() => {}
            _ => slug.push('-'),
        }
    }

    // After the cap and not before it: a name cut mid-run would otherwise keep
    // the dash it was cut through, and the acceptance criterion is that an id
    // matches `^[a-z0-9][a-z0-9-]*$`.
    let trimmed = slug.trim_end_matches('-');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// What `hostname(1)` reads, when there is such a file to read.
///
/// [`None`] rather than an error on a machine that has none: the caller has a
/// fallback, and this is a label the UI shows rather than anything load-bearing.
///
/// Windows returns early — it names the machine through `COMPUTERNAME`, and a
/// bare `/etc/hostname` there would resolve against the current drive root, a
/// path this has no business reading.
///
/// `cfg!` rather than a `#[cfg]` twin, which is the split this crate already
/// makes: the attribute is for a body that *cannot* compile on the other
/// platform — the symlink helpers in [`crate::files`], the console flag in
/// [`crate::process`] — and `cfg!` is for a body that compiles anywhere and
/// only needs the answer to differ, as in [`crate::editor`] and
/// [`crate::projects`]. Nothing below is platform-specific, so there is nothing
/// to gate out; and a body that is compiled on both runners is a body both
/// runners type-check.
fn configured_hostname() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    hostname_in(&std::fs::read_to_string("/etc/hostname").ok()?)
}

/// The name inside `/etc/hostname`, separated from the read so the parsing can
/// be tested without a filesystem — and so it can be tested at all on the
/// platform this suite has always run on, which has no such file.
///
/// `hostname(5)` describes a configuration file rather than a value: one
/// newline-terminated name, with `#` comments and blank lines ignored. A file
/// with no name in it answers [`None`] and the caller falls through, which is
/// the difference between a machine that did not say and one called "".
fn hostname_in(contents: &str) -> Option<String> {
    contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
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
    use crate::remote_access::Exposure;

    /// Ticket 23's "application state is stored in the appropriate per-user
    /// location", checked where the answer is decided.
    ///
    /// Read-only on purpose. The branches below take their base from process
    /// environment variables, which are global and mutable, and a test that set
    /// one would be setting it for every test running beside it — the reason
    /// [`super::ServerConfig::detect_in`] takes a directory instead of reading
    /// one. So this asserts against the machine as it is: whatever base this
    /// platform advertises, laplus's files are *inside* it, under a name of
    /// its own, and never beside the executable.
    #[test]
    fn the_developers_files_live_under_this_users_own_directory() {
        let directory = data_dir();

        let name = directory.file_name().and_then(|name| name.to_str());
        assert!(
            matches!(name, Some("laplus" | ".laplus")),
            "the directory should be named for the app: {}",
            directory.display()
        );

        let base = [
            "LOCALAPPDATA",
            "APPDATA",
            "XDG_DATA_HOME",
            "USERPROFILE",
            "HOME",
        ]
            .iter()
            .find_map(|name| non_empty_env(name));
        let Some(base) = base else {
            // A machine with none of them. The fallback is relative and that is
            // all there is to say about it.
            return;
        };

        assert!(
            directory.is_absolute(),
            "{} should be an absolute path",
            directory.display()
        );
        assert!(
            directory.starts_with(&base),
            "{} should be under {base}",
            directory.display()
        );
        assert!(
            crate::store::default_path().starts_with(&directory),
            "the registry should live with the rest of the developer's files"
        );
        assert!(logs_dir().starts_with(&directory), "so should the logs");
    }

    /// The shell writes a startup failure into this directory before any server
    /// exists to say where it is, so the two answers have to be the same one.
    #[test]
    fn the_logs_directory_the_shell_writes_to_is_the_one_the_config_advertises() {
        let config = ServerConfig::detect();
        assert_eq!(
            config.observability.logs_directory_path,
            logs_dir().to_string_lossy()
        );
    }

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
            let found = value
                .pointer(path)
                .unwrap_or_else(|| panic!("{path} is present"));
            let text = found
                .as_str()
                .unwrap_or_else(|| panic!("{path} is a string, got {found}"));
            assert_eq!(text, text.trim(), "{path} is trimmed");
            assert!(!text.is_empty(), "{path} is non-empty");
        }
    }

    /// Ticket 26. A server that ships a UI answers with that UI's version, so
    /// the client's skew banner has nothing to report; one that ships no UI
    /// keeps this crate's version, which is the only honest answer it has.
    #[test]
    fn a_shipped_ui_lends_the_server_its_version_and_nothing_else_does() {
        let plain = ServerConfig::detect();
        assert_eq!(plain.environment.server_version, env!("CARGO_PKG_VERSION"));

        let serving = ServerConfig::detect().serving_ui_version("0.0.28");
        assert_eq!(serving.environment.server_version, "0.0.28");
        assert_eq!(
            serving.to_value()["environment"]["serverVersion"],
            serde_json::json!("0.0.28"),
            "the wire carries the number the client compares, not the one behind it"
        );
    }

    /// Ticket 04 of the headless-Linux effort. `--network` overrides the file
    /// for one run, and this is the arithmetic that stands behind it: the mode
    /// moves, the bind address moves with it, and so does the policy the
    /// descriptor reports. The third is the one worth pinning — a server bound
    /// to `0.0.0.0` still advertising `loopback-browser` hides the section of
    /// Settings holding the only button that mints a pairing code, which is the
    /// bug [`ServerConfig::with_remote_access`] exists to prevent.
    #[test]
    fn an_exposure_override_moves_the_address_and_the_policy_together() {
        let detected = ServerConfig::detect_in(PathBuf::from("does-not-exist"));
        assert_eq!(detected.remote_access.exposure(), Exposure::LocalOnly);
        assert_eq!(detected.auth.policy, "loopback-browser");

        let opened = {
            let exposed = detected
                .remote_access
                .with_exposure(Exposure::NetworkAccessible);
            detected.with_remote_access(exposed)
        };
        assert_eq!(
            opened.remote_access.bind_address(),
            std::net::Ipv4Addr::UNSPECIFIED
        );
        assert_eq!(opened.auth.policy, "remote-reachable");

        // And back, which is `--network=false` over a file that says otherwise.
        let closed = {
            let hidden = opened.remote_access.with_exposure(Exposure::LocalOnly);
            opened.with_remote_access(hidden)
        };
        assert_eq!(
            closed.remote_access.bind_address(),
            std::net::Ipv4Addr::LOCALHOST
        );
        assert_eq!(closed.auth.policy, "loopback-browser");
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
    /// lands — and on once it has, because the settle and snooze controls are
    /// hidden entirely from a server that stays quiet about them.
    #[test]
    fn a_capability_is_advertised_exactly_when_it_has_been_built() {
        let capabilities = ServerConfig::detect().environment.capabilities;
        assert!(!capabilities.repository_identity);
        // `server.probe`, which `crate::rpc` answers. Advertised only because it
        // is answered — see [`Capabilities::connection_probe`] for what a flag
        // ahead of its method would cost here, which is the connection.
        assert_eq!(capabilities.connection_probe, Some(true));
        // `thread.settle` and `thread.unsettle`, ticket 07.
        assert_eq!(capabilities.thread_settlement, Some(true));
        // `thread.snooze` and `thread.unsnooze`, ticket 09.
        assert_eq!(capabilities.thread_snooze, Some(true));
        assert_eq!(capabilities.thread_pinning, Some(true));
        assert_eq!(capabilities.thread_pin_reorder, Some(true));
    }

    #[test]
    fn absent_capabilities_are_omitted_rather_than_serialized_as_null() {
        let mut config = ServerConfig::detect();
        // Every optional flag this server holds is on today, so the omission
        // half of this test has to be posed rather than observed: turning one
        // off is what proves `skip_serializing_if` is still doing the work. The
        // contract reads absent as unsupported and `null` as a decode failure,
        // so the difference is the whole of the test.
        config.environment.capabilities.connection_probe = None;
        let value = config.to_value();
        let capabilities = &value["environment"]["capabilities"];
        assert_eq!(capabilities["repositoryIdentity"], serde_json::json!(false));
        assert!(capabilities.get("connectionProbe").is_none());
        // And one that is present is the literal `true` the client compares
        // against, rather than any other truthy shape.
        assert_eq!(capabilities["threadSettlement"], serde_json::json!(true));
        assert_eq!(capabilities["threadSnooze"], serde_json::json!(true));
        assert_eq!(capabilities["threadPinning"], serde_json::json!(true));
        assert_eq!(capabilities["threadPinReorder"], serde_json::json!(true));
    }

    /// Whatever this platform answers, the label is a name rather than an empty
    /// string — the same read-only shape as the `data_dir` test above, and for
    /// the same reason: these sources are process-global and a test that set
    /// one would set it for every test beside it.
    #[test]
    fn this_machine_has_a_label_whatever_it_is_running_on() {
        let label = machine_label();
        assert_eq!(label, label.trim());
        assert!(!label.is_empty());
    }

    /// Ticket 05 of the headless-Linux effort — not the `rust-server-tauri` 05
    /// that `data_dir` above cites. The parsing half of
    /// [`super::configured_hostname`], separated
    /// from the read so it can be checked on the platform this suite has always
    /// run on — which has no `/etc/hostname` at all.
    ///
    /// `hostname(5)` describes a file, not a value, and the difference shows up
    /// on a machine whose installer left a comment above the name: a `trim()`
    /// would advertise "# set by cloud-init" as the label the UI puts beside
    /// the connection.
    #[test]
    fn the_configured_hostname_is_the_first_line_that_is_not_a_comment() {
        assert_eq!(hostname_in("orpheus\n").as_deref(), Some("orpheus"));
        assert_eq!(hostname_in("  orpheus  \n").as_deref(), Some("orpheus"));
        assert_eq!(
            hostname_in("# set by cloud-init\norpheus\n").as_deref(),
            Some("orpheus"),
            "a comment is ignored, not read"
        );
        assert_eq!(
            hostname_in("\n\norpheus\n").as_deref(),
            Some("orpheus"),
            "and so is a blank line"
        );

        assert_eq!(hostname_in(""), None);
        assert_eq!(hostname_in("   \n\n"), None);
        assert_eq!(
            hostname_in("# nothing but a comment\n"),
            None,
            "a file naming nothing is not an answer of \"\" — the caller falls \
             through to the fallback, which is the whole point of the Option"
        );
    }

    /// Ticket 06 of the headless-Linux effort. The prefix half of an
    /// environment id, and the parsing is separated from the machine for the
    /// same reason [`super::hostname_in`] is: a test that set `COMPUTERNAME`
    /// would set it for every test beside it.
    ///
    /// The cases below are the ones a real machine produces. `DESKTOP-19EUMEB`
    /// is this developer's Windows box, which is where the ticket's own example
    /// came from; a hostname with a dot in it is every cloud instance; and the
    /// last two are why this answers [`Option`] rather than a string — a name
    /// that slugs to nothing is not an id of `""`, it is a machine that did not
    /// usefully say, and the caller has a fallback for that.
    #[test]
    fn a_machine_name_slugs_into_a_url_safe_prefix() {
        assert_eq!(
            slug_of("DESKTOP-19EUMEB").as_deref(),
            Some("desktop-19eumeb")
        );
        assert_eq!(slug_of("orpheus").as_deref(), Some("orpheus"));
        assert_eq!(
            slug_of("ip-10-0-1-42.eu-west-1.compute.internal").as_deref(),
            Some("ip-10-0-1-42-eu-west-1-compu"),
            "capped, because this is a route segment and not a label"
        );
        assert_eq!(
            slug_of("Ada's  Laptop!!").as_deref(),
            Some("ada-s-laptop"),
            "every run of anything else collapses to one dash, and none is left \
             hanging off either end"
        );
        assert_eq!(
            slug_of("--host--").as_deref(),
            Some("host"),
            "a name that already had dashes keeps one word rather than growing \
             empty segments"
        );

        assert_eq!(slug_of(""), None);
        assert_eq!(
            slug_of("!!!"),
            None,
            "a name with nothing keepable in it is not an id of \"\""
        );
    }

    /// A cap that cut mid-run would leave the dash it cut through on the end,
    /// which is the one case the trimming has to happen *after* the truncation
    /// rather than before it. Twenty-eight characters of hostname is already
    /// past the point of being read.
    #[test]
    fn a_capped_slug_does_not_end_in_the_dash_it_was_cut_through() {
        let slug = slug_of("averyveryverylongmachinename-with-more-after-it")
            .expect("a name this long still slugs");
        assert_eq!(slug.len(), MACHINE_SLUG_LIMIT);
        assert!(!slug.ends_with('-'), "{slug} should not end in a dash");
    }

    /// Whatever machine this is running on, the prefix is a legal id — the same
    /// read-only shape as the label test above, and the property the ticket's
    /// acceptance criterion states: `^[a-z0-9][a-z0-9-]*$`.
    #[test]
    fn this_machine_has_a_slug_whatever_it_is_running_on() {
        let slug = machine_slug();
        assert!(!slug.is_empty());
        assert!(
            slug.bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "{slug} should begin with a character and not a dash"
        );
        assert!(
            slug.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
            "{slug} should be lowercase, digits and dashes only"
        );
        assert!(!slug.ends_with('-'));
    }

    /// Ticket 05 of the headless-Linux effort, and the finding that made it a
    /// ticket: `HOSTNAME` is a
    /// *shell* variable on most Linux distributions, exported by nothing, so
    /// both variables above answer `None` on the box this effort exists to put
    /// a server on — and every headless laplus called itself `laplus`, which is
    /// a label distinguishing no machine from any other at exactly the moment
    /// there is more than one to distinguish.
    ///
    /// Read-only, like the `data_dir` test above and for the same reason. So it
    /// asks the machine what it can discover and insists the label is *that*:
    /// `COMPUTERNAME` on the Windows runner, `/etc/hostname` on the Linux one
    /// this ticket added. A machine that can discover nothing keeps the
    /// fallback and this has nothing to check — which is what the assertion
    /// above it covers.
    #[test]
    fn a_machine_that_knows_its_own_name_is_labelled_with_it() {
        let discoverable = non_empty_env("COMPUTERNAME")
            .or_else(|| non_empty_env("HOSTNAME"))
            .or_else(configured_hostname);
        let Some(name) = discoverable else {
            return;
        };

        assert_eq!(
            machine_label(),
            name,
            "the label should name this machine rather than the product"
        );
    }
}
