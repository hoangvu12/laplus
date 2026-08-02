//! Settings: what the developer configured, kept between runs.
//!
//! Two method tags land here — `server.getSettings` and `server.updateSettings`
//! — and behind them one file, `settings.json`, beside the keybindings in the
//! app's data directory. [`crate::config`] owns what a settings value *is*;
//! this owns reading one off a disk, changing it, and writing it back.
//!
//! ## A patch, not a value
//!
//! `server.updateSettings` takes `ServerSettingsPatch`, every field of which is
//! optional, and that is the difference this module turns on: an absent field
//! means **unchanged**, not "set to the default". The UI's settings panel sends
//! only the control the developer touched, so a server that read the patch as a
//! whole value would reset every other setting each time somebody flicked a
//! switch.
//!
//! The same rule nests. `providers.claudeAgent.binaryPath` arriving alone leaves
//! `enabled`, `homePath`, `launchArgs` and `customModels` where they were.
//!
//! ## Rejection has to leave the previous values standing
//!
//! One of ticket 22's criteria in as many words, and it is the reason a patch is
//! *checked whole before anything is applied*. A patch that set a good binary
//! path and a negative fetch interval must change neither — half-applying it
//! would leave the developer with settings they never asked for and no way to
//! tell which half landed.
//!
//! ## What is not settable, and why
//!
//! Two fields of `ServerSettings` are reported rather than configured, and both
//! were decided before this ticket:
//!
//! - **`enableAssistantStreaming`** is a description of what this server does.
//!   It streams; ticket 10's second criterion requires it. A switch that turned
//!   it off would be a switch that did nothing.
//! - The legacy `providers.claudeAgent` and `providers.codex` buckets remain
//!   accepted as input and are normalized into the durable default envelopes
//!   under `providerInstances`. Provider operations read only those envelopes.
//!   An explicit envelope in the same patch takes precedence.
//!
//! A patch that would **change** either is refused rather than ignored, because
//! a settings panel whose control moves back on its own is worse than one that
//! says no. A patch that merely *repeats* the value in force is not a change and
//! is accepted — which is not a nicety: [`save`] writes every field and [`load`]
//! reads the result back through the same [`apply`], so a field that could not
//! be repeated would be one this server writes and then throws away, and every
//! setting would be forgotten at the next restart.
//!
//! Shapes are hand-written from `ServerSettings`, `ServerSettingsPatch` and
//! `ServerSettingsError` in `t3code/packages/contracts/src/settings.ts`.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::config::{ClaudeSettings, CodexSettings, Settings};

/// Reading the settings, without the rest of the configuration around them.
pub const GET: &str = "server.getSettings";

/// Changing some of them.
pub const UPDATE: &str = "server.updateSettings";

/// The `_tag` both methods refuse under.
const ERROR: &str = "ServerSettingsError";

/// The file, inside the app's data directory.
pub const FILE: &str = "settings.json";

/// Settings as they may cross a client boundary. Provider credentials remain
/// in the runtime value and settings file, but are never reflected to a UI,
/// subscription, snapshot or RPC response.
pub(crate) fn public_value(settings: &Settings) -> Value {
    let mut value = serde_json::to_value(settings).unwrap_or(Value::Null);
    if let Some(instances) = value
        .get_mut("providerInstances")
        .and_then(Value::as_object_mut)
    {
        for instance in instances.values_mut() {
            if instance.get("driver").and_then(Value::as_str) == Some("opencode") {
                if let Some(config) = instance.get_mut("config").and_then(Value::as_object_mut) {
                    config.remove("serverPassword");
                }
            }
        }
    }
    value
}

/// The longest an interval may be — a little over a day.
///
/// Not a rule of the contract's, which types it only as a duration. What this
/// bounds is a number that becomes a timer: a fetch interval of `u64::MAX`
/// milliseconds is not a preference, it is a client sending something it did
/// not mean, and a stored one would be indistinguishable from "never" forever.
const LONGEST_INTERVAL: u64 = 24 * 60 * 60 * 1_000;

/// The environment modes a thread can start in — the contract's `ThreadEnvMode`.
///
/// v1 runs the agent in the project's own folder, so `worktree` is listed and
/// refused rather than silently accepted: a developer who chose it and got
/// `local` would be told their work was isolated when it was not. See
/// [`crate::threads`], where a turn asking for a worktree is refused by name for
/// the same reason.
const ENV_MODES: [&str; 2] = ["local", "worktree"];

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

/// Read the settings file over the defaults.
///
/// **Never fails**, and that is ticket 22's criterion rather than a convenience:
/// a corrupt store falls back to defaults with a warning rather than stopping
/// the app. A developer whose settings file has been truncated by a full disk
/// needs the app to open so they can fix it.
///
/// **The warning is a log line and not a `ConfigIssue`**, which is a divergence
/// from what a reader might reasonably expect. `ServerConfigIssue` is a closed
/// union of two *keybindings* members (see [`crate::config::ConfigIssue`]) and
/// `ServerConfig.issues` is an array of it — so a settings row invented for this
/// would fail the client's decode of the whole `server.getConfig` payload, and
/// a bad settings file would stop the app opening. Which is the thing this
/// function exists to prevent. There is no other channel: `ServerSettingsError`
/// answers a *call*, and nothing is calling at startup.
///
/// **A field that will not read costs itself**, not the file: the default
/// stands for that one and everything else in the file still applies. That is
/// deliberately *not* how an update behaves — see [`apply`] — and the asymmetry
/// is the point. A patch comes from this client now and is all-or-nothing so a
/// refusal changes nothing; a file may have been written by another build, and
/// one key this version has never heard of should not throw away the twenty it
/// does.
pub fn load(directory: &Path, defaults: Settings) -> Settings {
    let path = directory.join(FILE);
    let complain = |detail: &str| {
        eprintln!(
            "laplus: the settings file at {} was not fully used: {detail}",
            path.display()
        );
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // Nothing written yet, which is every first run.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return defaults,
        Err(error) => {
            complain(&format!("it could not be read: {error}"));
            return defaults;
        }
    };

    let stored: Value = match serde_json::from_str(&raw) {
        Ok(stored) => stored,
        Err(error) => {
            complain(&format!("it is not valid JSON: {error}"));
            return defaults;
        }
    };
    let Some(stored) = stored.as_object() else {
        complain("it is not a settings object.");
        return defaults;
    };

    // One field at a time, over the defaults. Each is the same operation an
    // update performs on the same field, so a stored file and a live change
    // cannot disagree about what a field means.
    let mut settings = defaults;
    for (field, value) in stored {
        let mut one = Map::new();
        one.insert(field.clone(), value.clone());
        if let Err(why) = apply(&mut settings, &one) {
            complain(&format!("{why} That setting kept its default."));
        }
    }
    // `providers` is the legacy input shape and `providerInstances` is the
    // routing source of truth. JSON object order must not decide which wins.
    if let Some(instances) = stored.get("providerInstances") {
        let mut one = Map::new();
        one.insert("providerInstances".to_string(), instances.clone());
        if let Err(why) = apply(&mut settings, &one) {
            complain(&format!("{why} That setting kept its default."));
        }
    }
    settings
}

/// Write the settings down.
///
/// Whole rather than merged, because this file is *this server's* document
/// unlike the keybindings one: it is written only from here, and a merge would
/// preserve keys a newer build had already stopped believing.
fn save(directory: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = directory.join(FILE).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("its directory could not be made: {error}"))?;
    }
    let written = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("they could not be written out: {error}"))?;
    std::fs::write(directory.join(FILE), written + "\n")
        .map_err(|error| format!("they could not be written: {error}"))
}

// ---------------------------------------------------------------------------
// Applying a patch
// ---------------------------------------------------------------------------

/// Fold a patch into a settings value, or refuse the whole patch.
///
/// **Nothing is written until everything has been read**, which is what makes a
/// rejected update leave the previous values standing: `settings` is the
/// caller's copy and the caller only keeps it on `Ok`.
///
/// The refusal is a sentence rather than a field name because that is all
/// `ServerSettingsError` carries to the developer — its `message` is composed
/// from `operation` and `settingsPath`, so anything specific has to be in the
/// text.
fn apply(settings: &mut Settings, patch: &Map<String, Value>) -> Result<(), String> {
    let mut next = settings.clone();

    for (field, value) in patch {
        match field.as_str() {
            "enableProviderUpdateChecks" => {
                next.enable_provider_update_checks = boolean(field, value)?
            }
            "newWorktreesStartFromOrigin" => {
                next.new_worktrees_start_from_origin = boolean(field, value)?
            }
            "addProjectBaseDirectory" => next.add_project_base_directory = text(field, value)?,
            "automaticGitFetchInterval" => {
                let interval = value
                    .as_u64()
                    .filter(|interval| *interval <= LONGEST_INTERVAL);
                next.automatic_git_fetch_interval = interval.ok_or_else(|| {
                    format!(
                        "'automaticGitFetchInterval' has to be a whole number of milliseconds \
                         between 0 and {LONGEST_INTERVAL}, and was {value}."
                    )
                })?;
            }
            "defaultThreadEnvMode" => {
                next.default_thread_env_mode = match value.as_str() {
                    Some("local") => "local",
                    // Named rather than lumped in with an unknown value: a
                    // developer choosing this is choosing something the contract
                    // has and this build does not, and the sentence should say so.
                    Some("worktree") => {
                        return Err("Threads that run in their own worktree are not \
                                    supported by this server, so 'defaultThreadEnvMode' \
                                    cannot be set to 'worktree'."
                            .to_string())
                    }
                    _ => {
                        return Err(format!(
                            "'defaultThreadEnvMode' has to be one of {}, and was {value}.",
                            ENV_MODES.join(" or ")
                        ))
                    }
                };
            }
            "observability" => {
                let inside = object(field, value)?;
                let mut observability = next.observability.clone();
                for (field, value) in inside {
                    match field.as_str() {
                        "otlpTracesUrl" => observability.otlp_traces_url = text(field, value)?,
                        "otlpMetricsUrl" => observability.otlp_metrics_url = text(field, value)?,
                        unknown => return Err(unrecognised(&format!("observability.{unknown}"))),
                    }
                }
                next.observability = observability;
            }
            "providers" => {
                let inside = object(field, value)?;
                for (driver, value) in inside {
                    match driver.as_str() {
                        "claudeAgent" => {
                            next.providers.claude_agent =
                                claude(next.providers.claude_agent.clone(), object(driver, value)?)?
                        }
                        "codex" => {
                            next.providers.codex =
                                codex(next.providers.codex.clone(), object(driver, value)?)?
                        }
                        // The three drivers upstream has and this server does
                        // not. Refused by name so the sentence says what is
                        // missing rather than "unknown field".
                        "cursor" | "grok" | "opencode" => {
                            return Err(format!(
                                "This server has no '{driver}' driver, so it \
                                 cannot be configured."
                            ))
                        }
                        unknown => return Err(unrecognised(&format!("providers.{unknown}"))),
                    }
                }
            }
            // Reported rather than configured — see the module documentation.
            //
            // **What is refused is a change, not a mention.** A patch asking for
            // the value already in force is asking for what it already has, and
            // saying no to that would break the one thing a settings file has to
            // do: [`save`] writes every field and [`load`] reads the result back
            // through here, so a field that could not be *repeated* would be a
            // file this server writes and then refuses.
            "enableAssistantStreaming" => {
                if boolean(field, value)? != next.enable_assistant_streaming {
                    return Err("This server always streams the agent's reply, so \
                                'enableAssistantStreaming' cannot be turned off."
                        .to_string());
                }
            }
            "providerInstances" => {
                let provided = object(field, value)?;
                let mut instances = provider_instances(provided)?;
                for (instance_id, instance) in &mut instances {
                    let Some(previous) = next.provider_instances.get(instance_id) else {
                        continue;
                    };
                    if instance.get("driver").and_then(Value::as_str) != Some("opencode")
                        || previous.get("driver").and_then(Value::as_str) != Some("opencode")
                    {
                        continue;
                    }
                    let previous_password = previous.pointer("/config/serverPassword").cloned();
                    if let (Some(password), Some(config)) = (
                        previous_password,
                        instance.get_mut("config").and_then(Value::as_object_mut),
                    ) {
                        let password_was_omitted = provided
                            .get(instance_id)
                            .and_then(|instance| instance.pointer("/config/serverPassword"))
                            .is_none();
                        if password_was_omitted {
                            config.insert("serverPassword".to_string(), password);
                        }
                    }
                }
                for instance_id in [
                    crate::provider::CLAUDE_INSTANCE_ID,
                    crate::provider::CODEX_INSTANCE_ID,
                ] {
                    if !instances.contains_key(instance_id) {
                        if let Some(default) = next.provider_instances.get(instance_id) {
                            instances.insert(instance_id.to_string(), default.clone());
                        }
                    }
                }
                next.provider_instances = instances;
            }
            // `textGenerationModelSelection` is a stored preference and nothing
            // else — no call reads it yet — so it round-trips rather than being
            // refused. A developer who set it and lost it on the next write
            // would have a settings panel that forgets.
            "textGenerationModelSelection" => {
                next.text_generation_model_selection = selection(value)?
            }
            unknown => return Err(unrecognised(unknown)),
        }
    }

    // The legacy driver buckets remain part of the contract, but provider
    // operations consume one registry. Normalize old-shaped writes at this
    // boundary unless the same patch supplied the default instance explicitly.
    let explicit_instances = patch.get("providerInstances").and_then(Value::as_object);
    if patch
        .get("providers")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.contains_key("claudeAgent"))
        && !explicit_instances
            .is_some_and(|instances| instances.contains_key(crate::provider::CLAUDE_INSTANCE_ID))
    {
        next.provider_instances.insert(
            crate::provider::CLAUDE_INSTANCE_ID.to_string(),
            next.providers.claude_agent.instance_envelope("Claude"),
        );
    }
    if patch
        .get("providers")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.contains_key("codex"))
        && !explicit_instances
            .is_some_and(|instances| instances.contains_key(crate::provider::CODEX_INSTANCE_ID))
    {
        next.provider_instances.insert(
            crate::provider::CODEX_INSTANCE_ID.to_string(),
            next.providers.codex.instance_envelope("Codex"),
        );
    }

    *settings = next;
    Ok(())
}

fn provider_instances(instances: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut normalized = Map::new();
    for (instance_id, value) in instances {
        if !slug(instance_id) {
            return Err(format!(
                "'{instance_id}' is not a provider instance id: it has to start with a letter and \
                 hold only letters, digits, '-' and '_'."
            ));
        }
        let envelope = object(&format!("providerInstances.{instance_id}"), value)?;
        for field in envelope.keys() {
            if !matches!(
                field.as_str(),
                "driver" | "displayName" | "enabled" | "config"
            ) {
                return Err(unrecognised(&format!(
                    "providerInstances.{instance_id}.{field}"
                )));
            }
        }
        let driver = envelope
            .get("driver")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if !slug(driver) {
            return Err(format!(
                "Provider instance '{instance_id}' needs a valid driver kind."
            ));
        }
        let Some(registered) = crate::provider::registration(driver) else {
            return Err(format!(
                "Provider instance '{instance_id}' uses unsupported driver kind '{driver}'."
            ));
        };
        let default_driver = match instance_id.as_str() {
            crate::provider::CLAUDE_INSTANCE_ID => Some(crate::provider::CLAUDE_DRIVER),
            crate::provider::CODEX_INSTANCE_ID => Some(crate::provider::CODEX_DRIVER),
            _ => None,
        };
        if let Some(expected) = default_driver.filter(|expected| *expected != driver) {
            return Err(format!(
                "Default provider instance '{instance_id}' belongs to driver '{expected}', not '{driver}'."
            ));
        }
        let display_name = envelope
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                format!("Provider instance '{instance_id}' needs a non-empty displayName.")
            })?;
        let enabled = match envelope.get("enabled") {
            Some(value) => boolean("enabled", value)?,
            None => true,
        };
        let config = envelope
            .get("config")
            .map(|value| object(&format!("providerInstances.{instance_id}.config"), value))
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let (binary_path, home_path, launch_args, custom_models) = match registered.kind {
            crate::provider::DriverKind::Claude => {
                let settings = claude(
                    ClaudeSettings {
                    enabled,
                    binary_path: "claude".to_string(),
                    home_path: String::new(),
                    launch_args: String::new(),
                    custom_models: Vec::new(),
                    },
                    &config,
                )?;
                (
                    settings.binary_path,
                    settings.home_path,
                    settings.launch_args,
                    settings.custom_models,
                )
            }
            crate::provider::DriverKind::Codex => {
                let settings = codex(
                    CodexSettings {
                    enabled,
                    binary_path: "codex".to_string(),
                    home_path: String::new(),
                    launch_args: String::new(),
                    custom_models: Vec::new(),
                    },
                    &config,
                )?;
                (
                    settings.binary_path,
                    settings.home_path,
                    settings.launch_args,
                    settings.custom_models,
                )
            }
            crate::provider::DriverKind::OpenCode => {
                let settings = opencode(&config)?;
                normalized.insert(instance_id.clone(), json!({
                    "driver": driver, "displayName": display_name, "enabled": enabled,
                    "config": {"binaryPath": settings.binary_path, "serverUrl": settings.server_url,
                        "serverPassword": settings.server_password, "customModels": settings.custom_models}
                }));
                continue;
            }
        };
        normalized.insert(
            instance_id.clone(),
            json!({
            "driver": driver,
            "displayName": display_name,
            "enabled": enabled,
            "config": {
                "binaryPath": binary_path,
                "homePath": home_path,
                "launchArgs": launch_args,
                "customModels": custom_models,
            }
            }),
        );
    }
    Ok(normalized)
}

fn opencode(config: &Map<String, Value>) -> Result<crate::config::OpenCodeSettings, String> {
    for field in config.keys() {
        if !matches!(
            field.as_str(),
            "binaryPath" | "serverUrl" | "serverPassword" | "customModels"
        ) {
            return Err(unrecognised(&format!("providers.opencode.{field}")));
        }
    }
    let server_url = config
        .get("serverUrl")
        .map(|v| text("serverUrl", v))
        .transpose()?
        .unwrap_or_default();
    if !server_url.is_empty() {
        let valid = reqwest::Url::parse(&server_url)
            .ok()
            .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
        if !valid {
            return Err("'serverUrl' has to be an HTTP or HTTPS URL.".to_string());
        }
    }
    Ok(crate::config::OpenCodeSettings {
        enabled: true,
        binary_path: config
            .get("binaryPath")
            .map(|v| text("binaryPath", v))
            .transpose()?
            .unwrap_or_else(|| "opencode".to_string()),
        server_url,
        server_password: config
            .get("serverPassword")
            .map(|v| secret_text("serverPassword", v))
            .transpose()?
            .unwrap_or_default(),
        custom_models: config
            .get("customModels")
            .map(models)
            .transpose()?
            .unwrap_or_default(),
    })
}

/// The Claude instance's own half of a patch.
///
/// Every field optional and every absent one unchanged, like the patch around
/// it. This is the criterion "the Claude Code provider instance can be
/// configured, including model selection": `customModels` is what the composer's
/// model picker offers, and `binaryPath` is what the next session starts.
fn claude(
    mut claude: ClaudeSettings,
    patch: &Map<String, Value>,
) -> Result<ClaudeSettings, String> {
    for (field, value) in patch {
        match field.as_str() {
            "enabled" => claude.enabled = boolean(field, value)?,
            "binaryPath" => claude.binary_path = text(field, value)?,
            "homePath" => claude.home_path = text(field, value)?,
            "launchArgs" => claude.launch_args = text(field, value)?,
            "customModels" => {
                let listed = value.as_array().ok_or_else(|| {
                    format!("'customModels' has to be a list of model names, and was {value}.")
                })?;
                let mut models = Vec::with_capacity(listed.len());
                for model in listed {
                    let named = model
                        .as_str()
                        .map(str::trim)
                        .filter(|named| !named.is_empty())
                        .ok_or_else(|| {
                            format!("'customModels' has to hold model names, and held {model}.")
                        })?;
                    models.push(named.to_string());
                }
                claude.custom_models = models;
            }
            unknown => return Err(unrecognised(&format!("providers.claudeAgent.{unknown}"))),
        }
    }
    Ok(claude)
}

/// Codex settings are stored before the Codex driver consumes them. Shadow
/// homes are deliberately excluded: they select an account, and this server
/// runs one Codex account.
fn codex(mut codex: CodexSettings, patch: &Map<String, Value>) -> Result<CodexSettings, String> {
    for (field, value) in patch {
        match field.as_str() {
            "enabled" => codex.enabled = boolean(field, value)?,
            "binaryPath" => codex.binary_path = text(field, value)?,
            "homePath" => codex.home_path = text(field, value)?,
            "launchArgs" => codex.launch_args = text(field, value)?,
            "customModels" => codex.custom_models = models(value)?,
            "shadowHomePath" => {
                return Err(
                    "'providers.codex.shadowHomePath' is an account-selection setting, and \
                     this server runs one Codex account, so it cannot honour or store it."
                        .to_string(),
                )
            }
            unknown => return Err(unrecognised(&format!("providers.codex.{unknown}"))),
        }
    }
    Ok(codex)
}

fn models(value: &Value) -> Result<Vec<String>, String> {
    let listed = value.as_array().ok_or_else(|| {
        format!("'customModels' has to be a list of model names, and was {value}.")
    })?;
    listed
        .iter()
        .map(|model| {
            model
                .as_str()
                .map(str::trim)
                .filter(|named| !named.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("'customModels' has to hold model names, and held {model}."))
        })
        .collect()
}

/// A `ModelSelectionPatch`, which is an instance and a model.
///
/// Kept as JSON rather than typed, for the reason [`crate::threads`] keeps a
/// thread's own `modelSelection` as JSON: nothing here reads into it, and a
/// shape mirrored for storage alone is a shape to keep in step for no query it
/// enables. What *is* checked is that it is an object with two strings, because
/// a stored value the client cannot decode would fail its whole settings read.
fn selection(value: &Value) -> Result<Value, String> {
    let object = value.as_object().ok_or_else(|| {
        format!(
            "'textGenerationModelSelection' has to name an instance and a model, and was {value}."
        )
    })?;
    for field in ["instanceId", "model"] {
        let named = object
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if named.trim().is_empty() {
            return Err(format!(
                "'textGenerationModelSelection' has to name a non-empty '{field}'."
            ));
        }
    }

    // **`instanceId` is a slug, and it is checked here or never.** It is stored,
    // and it is read back inside `ServerSettings` — so a value that does not
    // match `ProviderInstanceId` would fail the client's decode of every
    // settings read *and* of `server.getConfig`, from the moment it was written
    // until somebody edited the file by hand. This is the one field here that
    // can poison a later read rather than only its own call.
    let instance = object["instanceId"].as_str().unwrap_or_default().trim();
    if !slug(instance) {
        return Err(format!(
            "'{instance}' is not a provider instance id: it has to start with a letter and \
             hold only letters, digits, '-' and '_'."
        ));
    }
    Ok(value.clone())
}

/// `PROVIDER_SLUG_PATTERN` — `^[a-zA-Z][a-zA-Z0-9_-]*$`, at most
/// `PROVIDER_SLUG_MAX_CHARS`.
fn slug(value: &str) -> bool {
    /// `PROVIDER_SLUG_MAX_CHARS`.
    const LONGEST: usize = 64;

    !value.is_empty()
        && value.chars().count() <= LONGEST
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

fn boolean(field: &str, value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("'{field}' has to be true or false, and was {value}."))
}

fn text(field: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(|text| text.trim().to_string())
        .ok_or_else(|| format!("'{field}' has to be text, and was {value}."))
}

fn secret_text(field: &str, value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(|text| text.trim().to_string())
        .ok_or_else(|| format!("'{field}' has to be text."))
}

fn object<'a>(field: &str, value: &'a Value) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("'{field}' has to be a group of settings, and was {value}."))
}

/// A field this build does not know.
///
/// Refused rather than ignored, and this is the one place that choice is
/// arguable: a *newer UI* sending a setting this server has never heard of gets
/// a refusal rather than a partial success. Refusing is still the better half of
/// the trade, because the alternative is a settings panel that reports success
/// and changes nothing — and the criterion this serves says an invalid setting
/// is rejected *with a message*.
fn unrecognised(field: &str) -> String {
    format!("'{field}' is not a setting this server knows.")
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// A validated `server.updateSettings`.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    patch: Map<String, Value>,
}

impl Update {
    /// Read one, naming the file a refusal is about. See
    /// [`crate::keybindings::Upsert::read`], whose reasoning this shares.
    pub fn read(payload: &Value, directory: &Path) -> Result<Update, Value> {
        let refuse = |detail: &str| refused(directory, Operation::Normalize, detail);
        let patch = payload.get("patch").ok_or_else(|| {
            refuse("This call needs a patch of the settings to change; none was given.")
        })?;
        let patch = patch
            .as_object()
            .ok_or_else(|| refuse("The patch has to be a group of settings."))?;
        Ok(Update {
            patch: patch.clone(),
        })
    }

    /// Apply it, write it down, and hand back the settings that are now in
    /// force.
    ///
    /// Deferred — see [`crate::rpc::Deferred`] — because it writes a file, and
    /// because of the second half below, which starts a process.
    pub fn run(
        self,
        store: &crate::config_store::ConfigStore,
        roots: &[std::path::PathBuf],
    ) -> Result<Value, Value> {
        let previous_instances = store.current().settings.provider_instances.clone();
        let settings = store.reconfigure(|settings| apply(settings, &self.patch))?;

        // A provider is re-checked when its configuration moved, and only then.
        // A new `binaryPath` points at a different install with a
        // different version, and a new `customModels` list is a different set of
        // slugs to offer — neither of which the picker would show until
        // something looked. Without this the criterion "a configuration change
        // reaches the UI without a restart" would hold for the settings panel
        // and quietly fail for the thing the developer configured.
        //
        // After the write rather than before, so a refused patch cannot start a
        // process; and inline, because this is already off the read loop and the
        // developer is waiting for the answer that says it worked.
        let search = crate::process::Search::from_environment();
        let current = store.current();
        let removed = previous_instances
            .keys()
            .filter(|instance_id| {
                !current
                    .settings
                    .provider_instances
                    .contains_key(*instance_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !removed.is_empty() {
            for instance_id in &removed {
                store.begin_provider_probe(instance_id);
            }
            let providers = current
                .providers
                .iter()
                .filter(|provider| !removed.contains(&provider.instance_id))
                .cloned()
                .collect();
            store.apply(crate::config_store::ConfigChange::Providers(providers));
        }
        for (instance_id, instance) in &current.settings.provider_instances {
            if previous_instances.get(instance_id) != Some(instance) {
                crate::provider::refresh_configured(store, instance_id, &search, roots);
            }
        }
        Ok(settings)
    }
}

/// One of the contract's nine `ServerSettingsOperation` literals.
///
/// It is not decoration: `ServerSettingsError`'s `message` getter composes the
/// sentence the developer reads out of this and `settingsPath`, so a patch this
/// server would not *read* reported as a failure to **write** would send them
/// looking at a file that is perfectly fine.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Operation {
    /// The patch itself was not one — the request never got as far as a file.
    Normalize,
    /// The settings were good and the disk would not take them.
    Write,
}

impl Operation {
    fn as_str(self) -> &'static str {
        match self {
            Operation::Normalize => "normalize",
            Operation::Write => "write-file",
        }
    }
}

/// The typed refusal, in the shape the client decodes.
///
/// `cause` is required by the schema and carries the sentence, because
/// everything else about a `ServerSettingsError` is machinery.
pub(crate) fn refused(path: &Path, operation: Operation, detail: &str) -> Value {
    json!({
        "_tag": ERROR,
        "settingsPath": path.join(FILE).display().to_string(),
        "operation": operation.as_str(),
        "cause": detail,
    })
}

/// Change the settings, or say why not — the half [`crate::config_store`] needs
/// that knows the format.
pub(crate) fn reconfigure(
    directory: &Path,
    settings: &mut Settings,
    change: impl FnOnce(&mut Settings) -> Result<(), String>,
) -> Result<(), Value> {
    let mut next = settings.clone();
    // A patch this server will not apply never reaches the disk, so it is a
    // `normalize` rather than a `write-file` — the file is fine and the request
    // was not.
    change(&mut next).map_err(|why| refused(directory, Operation::Normalize, &why))?;
    save(directory, &next).map_err(|why| {
        refused(
            directory,
            Operation::Write,
            &format!("they were not saved: {why}"),
        )
    })?;
    *settings = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    fn defaults() -> Settings {
        ServerConfig::detect_in(std::path::PathBuf::from("unused")).settings
    }

    fn patched(patch: Value) -> Result<Settings, String> {
        let mut settings = defaults();
        apply(
            &mut settings,
            patch.as_object().expect("a patch is an object"),
        )?;
        Ok(settings)
    }

    /// The rule the whole module turns on: an absent field is unchanged, not
    /// reset. The settings panel sends only the control the developer touched.
    #[test]
    fn a_patch_changes_what_it_names_and_nothing_else() {
        let before = defaults();
        let after = patched(json!({"addProjectBaseDirectory": "/work"})).expect("applies");

        assert_eq!(after.add_project_base_directory, "/work");
        assert_eq!(
            after.enable_provider_update_checks,
            before.enable_provider_update_checks
        );
        assert_eq!(
            after.automatic_git_fetch_interval,
            before.automatic_git_fetch_interval
        );
        assert_eq!(after.providers.claude_agent, before.providers.claude_agent);
    }

    /// And it nests. One field of the Claude instance leaves the other four
    /// standing — which is what makes the settings panel's binary-path box
    /// usable without retyping the model list.
    #[test]
    fn a_nested_patch_leaves_its_neighbours_standing() {
        let after = patched(json!({
            "providers": {"claudeAgent": {"binaryPath": "/opt/claude"}},
        }))
        .expect("applies");

        assert_eq!(after.providers.claude_agent.binary_path, "/opt/claude");
        assert!(after.providers.claude_agent.enabled, "the neighbour moved");
        assert_eq!(
            after.providers.claude_agent.custom_models,
            Vec::<String>::new()
        );
    }

    /// The criterion: an invalid patch is refused *and changes nothing*. Driven
    /// with a patch whose first field is perfectly good, because half-applying
    /// is the failure this is about.
    #[test]
    fn a_refused_patch_leaves_every_previous_value_intact() {
        let mut settings = defaults();
        let before = settings.clone();

        let why = apply(
            &mut settings,
            json!({
                "addProjectBaseDirectory": "/work",
                "automaticGitFetchInterval": -5,
            })
            .as_object()
            .expect("an object"),
        )
        .expect_err("a refusal");

        assert!(why.contains("automaticGitFetchInterval"), "{why}");
        assert_eq!(settings, before, "a refused patch was half applied");
    }

    /// Each kind of malformed value gets a sentence naming the field. The
    /// contract carries no field name of its own, so the sentence is the whole
    /// diagnostic.
    #[test]
    fn an_invalid_value_is_refused_with_a_sentence_naming_the_field() {
        for (patch, named) in [
            (
                json!({"enableProviderUpdateChecks": "yes"}),
                "enableProviderUpdateChecks",
            ),
            (
                json!({"automaticGitFetchInterval": 999_999_999_999u64}),
                "automaticGitFetchInterval",
            ),
            (
                json!({"defaultThreadEnvMode": "elsewhere"}),
                "defaultThreadEnvMode",
            ),
            (
                json!({"addProjectBaseDirectory": 7}),
                "addProjectBaseDirectory",
            ),
            (
                json!({"observability": {"otlpTracesUrl": 7}}),
                "otlpTracesUrl",
            ),
            (
                json!({"providers": {"claudeAgent": {"customModels": "opus"}}}),
                "customModels",
            ),
            (
                json!({"providers": {"claudeAgent": {"customModels": [""]}}}),
                "customModels",
            ),
            (
                json!({"textGenerationModelSelection": {"model": "x"}}),
                "instanceId",
            ),
            (json!({"nonsense": true}), "nonsense"),
        ] {
            let why = patched(patch.clone()).expect_err("a refusal");
            assert!(why.contains(named), "{patch} was refused with {why}");
        }
    }

    /// Fields this server reports rather than obeys, refused by name.
    /// Silently ignoring one would be a control that springs back.
    #[test]
    fn a_setting_this_server_does_not_have_says_so_rather_than_ignoring_it() {
        for (patch, expected) in [
            (json!({"enableAssistantStreaming": false}), "always streams"),
            (json!({"defaultThreadEnvMode": "worktree"}), "own worktree"),
            (
                json!({"providers": {"codex": {"shadowHomePath": "/accounts/work"}}}),
                "account-selection",
            ),
        ] {
            let why = patched(patch.clone()).expect_err("a refusal");
            assert!(why.contains(expected), "{patch} was refused with {why}");
        }
    }

    /// …and asking for the value already in force is not a change, so it is not
    /// refused. Without this the file [`save`] writes is one [`load`] throws
    /// away, and every setting would be forgotten at the next restart — which is
    /// how this rule was found.
    #[test]
    fn a_reported_setting_may_be_repeated_even_though_it_cannot_be_changed() {
        let after = patched(json!({
            "enableAssistantStreaming": true,
            "providerInstances": {},
            "defaultThreadEnvMode": "local",
        }))
        .expect("a patch that changes nothing is not a change");
        assert_eq!(after, defaults());
    }

    /// A first run has no file, and that is not a complaint.
    #[test]
    fn a_machine_with_no_settings_file_gets_the_defaults_quietly() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        assert_eq!(load(directory.path(), defaults()), defaults());
    }

    /// Written, then read back — which is the whole of "settings survive a
    /// restart", since a restart is a fresh [`load`] of the same directory.
    #[test]
    fn what_is_written_is_read_back() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let mut settings = defaults();

        reconfigure(directory.path(), &mut settings, |settings| {
            apply(
                settings,
                json!({
                    "addProjectBaseDirectory": "/work",
                    "providers": {
                        "claudeAgent": {"customModels": ["claude-opus-5"]},
                        "codex": {
                            "binaryPath": "/opt/codex",
                            "homePath": "/home/developer/.codex",
                            "launchArgs": "--config model_reasoning_effort=high"
                        }
                    },
                })
                .as_object()
                .expect("an object"),
            )
        })
        .expect("saves");

        let loaded = load(directory.path(), defaults());
        assert_eq!(loaded, settings);
        assert_eq!(loaded.add_project_base_directory, "/work");
        assert_eq!(
            loaded.providers.claude_agent.custom_models,
            vec!["claude-opus-5".to_string()]
        );
        assert_eq!(loaded.providers.codex.binary_path, "/opt/codex");
        assert_eq!(loaded.providers.codex.home_path, "/home/developer/.codex");
        assert_eq!(
            loaded.providers.codex.launch_args,
            "--config model_reasoning_effort=high"
        );
    }

    /// The criterion: a corrupt store falls back to the defaults rather than
    /// failing to start. Nothing here returns an error at all — the complaint
    /// goes to the log, because `ServerConfigIssue` has no member for it.
    #[test]
    fn a_corrupt_file_falls_back_to_the_defaults() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join(FILE), "{ not json").expect("writes");

        assert_eq!(load(directory.path(), defaults()), defaults());
    }

    /// A file written by a build that had a setting this one does not — the
    /// shape of every downgrade. **The key it does not know costs itself**, and
    /// the twenty beside it still apply, which is the difference between reading
    /// a file and applying a patch.
    #[test]
    fn a_file_from_another_build_loses_only_the_setting_this_build_lacks() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            directory.path().join(FILE),
            r#"{"addProjectBaseDirectory": "/work", "somethingNewer": true}"#,
        )
        .expect("writes");

        let loaded = load(directory.path(), defaults());
        assert_eq!(
            loaded.add_project_base_directory, "/work",
            "one unreadable key threw away a setting beside it"
        );
    }

    /// The payload's own shape, before any setting is looked at.
    #[test]
    fn a_call_without_a_patch_is_refused_by_name() {
        let directory = std::path::PathBuf::from("somewhere");
        for payload in [json!({}), json!({"patch": "everything"})] {
            let error = Update::read(&payload, &directory).expect_err("a refusal");
            assert_eq!(error["_tag"], ERROR, "{payload}");
            assert!(error["cause"].is_string(), "{payload}");
            // The request never reached a file, so it is not a failure to write
            // one — see [`Operation`].
            assert_eq!(error["operation"], "normalize", "{payload}");
            assert!(
                error["settingsPath"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with(FILE),
                "{error}"
            );
        }
    }
}
