//! The server configuration as a *live* value: what it currently is, and what
//! changed about it.
//!
//! [`crate::config`] holds the payload's types and how to assemble one from
//! the machine. This holds the one that is in force, and the change feed the
//! `subscribeServerConfig` subscription streams. The split matters: the types
//! are pure and pinned against a capture, while the store is shared mutable
//! state with an ordering guarantee to keep.
//!
//! Two invariants, and both are the reason this is a module rather than a
//! field on the server state:
//!
//! - **A change is stored before it is announced.** Otherwise a subscriber
//!   resynchronised at the wrong moment could be sent a snapshot older than
//!   the update it had already been told about.
//! - **Changes are announced in the order they were applied.** Two concurrent
//!   writers must not be able to publish in the opposite order to the one they
//!   wrote in, or a client's projection ends up on the losing value.
//!
//! Provider probes add one constraint to those invariants: blocking work may
//! finish out of order. A per-instance generation is reserved before that work
//! starts, and [`ConfigStore::apply_providers_if_current`] compares and publishes
//! under the same locks so an old account, model or skill snapshot cannot replace
//! a newer one. Ticket 22 owns keybindings and settings. [`ConfigChange`] is the
//! event vocabulary, closed to the contract's three update members.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::config::{ConfigIssue, Provider, ResolvedKeybinding, ServerConfig, Settings};
use crate::subscriptions::{EventSource, BACKLOG};

/// The current server configuration and its change feed.
///
/// Cheap to clone — every clone is the same store — because a subscription
/// outlives the call that opened it and needs to be able to describe the world
/// again long after.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    current: RwLock<Arc<ServerConfig>>,
    provider_probes: Mutex<HashMap<String, u64>>,
    updates: broadcast::Sender<Value>,
}

/// One provider probe's place in the start order for that provider instance.
#[derive(Debug)]
pub(crate) struct ProviderProbe {
    instance_id: String,
    generation: u64,
}

/// A change to the server configuration, in the terms the client can project.
///
/// Each member corresponds to one update event in the contract's
/// `ServerConfigStreamEvent` union. Keeping them together is what makes the
/// two things a change has to do — update the stored value and describe itself
/// to subscribers — impossible to do inconsistently.
#[derive(Debug, Clone)]
pub enum ConfigChange {
    /// Ticket 22. Keybindings and the issues found parsing them travel
    /// together because a malformed file produces both.
    Keybindings {
        keybindings: Vec<ResolvedKeybinding>,
        issues: Vec<ConfigIssue>,
    },
    /// [`crate::provider::refresh`], and every later provider status refresh.
    Providers(Vec<Provider>),
    /// Ticket 22. Boxed because it is much the largest member and an enum is
    /// as big as its widest arm.
    Settings(Box<Settings>),
}

impl ConfigChange {
    /// How this change describes itself to a subscriber.
    fn to_event(&self) -> Value {
        match self {
            ConfigChange::Keybindings { keybindings, issues } => json!({
                "version": 1,
                "type": "keybindingsUpdated",
                "payload": {"keybindings": keybindings, "issues": issues},
            }),
            ConfigChange::Providers(providers) => json!({
                "version": 1,
                "type": "providerStatuses",
                "payload": {"providers": providers},
            }),
            ConfigChange::Settings(settings) => json!({
                "version": 1,
                "type": "settingsUpdated",
                "payload": {"settings": settings},
            }),
        }
    }

    fn apply_to(self, config: &mut ServerConfig) {
        match self {
            ConfigChange::Keybindings { keybindings, issues } => {
                config.keybindings = keybindings;
                config.issues = issues;
            }
            ConfigChange::Providers(providers) => config.providers = providers,
            ConfigChange::Settings(settings) => config.settings = *settings,
        }
    }
}

impl ConfigStore {
    /// A store over the configuration exactly as given, touching no disk.
    ///
    /// What the unit tests of everything *around* the configuration want. The
    /// server uses [`ConfigStore::opening`], which is this plus the developer's
    /// own two files.
    pub fn new(config: ServerConfig) -> ConfigStore {
        ConfigStore {
            inner: Arc::new(Inner {
                current: RwLock::new(Arc::new(config)),
                provider_probes: Mutex::new(HashMap::new()),
                updates: broadcast::channel(BACKLOG).0,
            }),
        }
    }

    /// The same, with what the developer configured last time read in over it.
    ///
    /// **Nothing here can fail**, which is ticket 22's criterion rather than a
    /// convenience: a settings file the app refused to start on is one the
    /// developer cannot open the app to fix. Both readers answer with the
    /// defaults and a [`ConfigIssue`] instead, and the issues land in the same
    /// list a malformed keybinding lands in — which the UI already renders.
    pub fn opening(config: ServerConfig) -> ConfigStore {
        let mut config = config;
        let keybindings = crate::keybindings::load(&config.preferences);

        config.settings = crate::settings::load(&config.preferences, config.settings.clone());
        config.keybindings = keybindings.keybindings;
        // Only the keybindings file can put a row here — `ServerConfigIssue` has
        // no member for a settings problem, and one invented for it would fail
        // the client's decode of this whole payload. See `crate::settings::load`.
        config.issues = keybindings.issues;
        ConfigStore::new(config)
    }

    /// The configuration in force. An `Arc` rather than a guard so a reader
    /// never holds the lock across anything it might await on.
    pub fn current(&self) -> Arc<ServerConfig> {
        let current = self
            .inner
            .current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(&current)
    }

    /// Apply a change and tell every subscriber about it.
    pub fn apply(&self, change: ConfigChange) {
        let event = change.to_event();

        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = (**current).clone();
        change.apply_to(&mut next);
        *current = Arc::new(next);

        // Published while the write lock is held. `send` on a broadcast
        // channel never blocks — it drops the oldest value when the buffer is
        // full — so this cannot deadlock, and it is what makes concurrent
        // changes announce themselves in the order they were applied.
        let _ = self.inner.updates.send(event);
    }

    /// Reserve the next publication slot for one provider instance.
    ///
    /// Reserved before blocking work starts. A later reservation supersedes an
    /// earlier one, so completion order cannot turn into publication order.
    pub(crate) fn begin_provider_probe(&self, instance_id: &str) -> ProviderProbe {
        let mut probes = self
            .inner
            .provider_probes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = probes.entry(instance_id.to_string()).or_default();
        *generation = generation.wrapping_add(1);
        ProviderProbe {
            instance_id: instance_id.to_string(),
            generation: *generation,
        }
    }

    /// Publish a provider result only if no newer probe or relevant settings
    /// change has made it stale.
    ///
    /// The generation lock stays held through the configuration write, and the
    /// event is sent under that write lock exactly as in [`ConfigStore::apply`].
    /// That keeps the store's two ordering invariants intact while making the
    /// probe's compare-and-publish one atomic operation.
    pub(crate) fn apply_providers_if_current(
        &self,
        probe: ProviderProbe,
        settings_are_current: impl FnOnce(&ServerConfig) -> bool,
        update: impl FnOnce(&[Provider]) -> Vec<Provider>,
    ) -> bool {
        let mut probes = self
            .inner
            .provider_probes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(generation) = probes.get_mut(&probe.instance_id) else {
            return false;
        };
        if *generation != probe.generation {
            return false;
        }
        *generation = generation.wrapping_add(1);

        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !settings_are_current(&current) {
            return false;
        }

        let providers = update(&current.providers);
        let change = ConfigChange::Providers(providers);
        let event = change.to_event();
        let mut next = (**current).clone();
        change.apply_to(&mut next);
        *current = Arc::new(next);
        let _ = self.inner.updates.send(event);
        true
    }

    /// Change the settings, write them down, then publish them.
    ///
    /// The settings half of [`ConfigStore::rebind`], with the same division of
    /// labour: [`crate::settings`] knows what a patch means and this knows that
    /// only one of them happens at a time. `change` returns the sentence for a
    /// patch it will not apply, and **the store is not touched when it does** —
    /// which is the criterion "invalid settings are rejected with a message,
    /// leaving the previous values intact", enforced by the lock rather than by
    /// each caller remembering.
    pub fn reconfigure(
        &self,
        change: impl FnOnce(&mut crate::config::Settings) -> Result<(), String>,
    ) -> Result<Value, Value> {
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut settings = current.settings.clone();
        crate::settings::reconfigure(&current.preferences, &mut settings, change)?;

        let mut next = (**current).clone();
        next.settings = settings.clone();
        *current = Arc::new(next);

        let answer = serde_json::to_value(&settings).unwrap_or(Value::Null);
        let _ = self
            .inner
            .updates
            .send(ConfigChange::Settings(Box::new(settings)).to_event());
        Ok(answer)
    }

    /// Change the developer's keybindings file, then publish what it now says.
    ///
    /// **One rebind at a time**, and that is the whole reason this lives here
    /// rather than in [`crate::keybindings`]: the file is read, changed and
    /// written back, and two of those interleaving would lose one developer's
    /// edit under the other's. The lock this takes is the same one
    /// [`ConfigStore::apply`] takes, so a rebind and a settings change also
    /// cannot announce themselves out of order.
    ///
    /// The module owns the *format*; this owns the *ordering*. Neither knows
    /// the other's half.
    pub fn rebind(
        &self,
        change: impl FnOnce(&mut Vec<crate::keybindings::Rule>),
    ) -> Result<Value, Value> {
        let directory = self.current().preferences.clone();
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let loaded = crate::keybindings::rebind(&directory, change)?;
        let answer = crate::keybindings::to_result(&loaded);

        let mut next = (**current).clone();
        next.keybindings = loaded.keybindings.clone();
        next.issues = loaded.issues.clone();
        *current = Arc::new(next);

        // Under the write lock, like every other announcement here — see
        // [`ConfigStore::apply`], whose reasoning this shares rather than
        // repeats.
        let _ = self.inner.updates.send(
            ConfigChange::Keybindings {
                keybindings: loaded.keybindings,
                issues: loaded.issues,
            }
            .to_event(),
        );
        Ok(answer)
    }

    /// Change who may reach this server, write it down, and publish it.
    ///
    /// The third of the same shape as [`ConfigStore::reconfigure`] and
    /// [`ConfigStore::rebind`], and here for the same reason both of those are:
    /// the file is read, changed and written back, and two of those
    /// interleaving would lose one edit under the other. Turning the switch on
    /// while adding a tunnel hostname is exactly that pair.
    ///
    /// **A change here does not move the listener.** The address was bound at
    /// startup and cannot be re-bound from under an open socket, so
    /// [`crate::remote_access::Exposure`] takes effect on the next start and the
    /// shell restarts the application to make that immediate — which is what
    /// upstream's switch does too. The *hostname list* has no such problem:
    /// [`crate::auth`] reads it per request, so an added tunnel works at once.
    /// The caller is what knows which of the two it changed.
    pub fn readdress(
        &self,
        change: impl FnOnce(&crate::remote_access::RemoteAccess) -> crate::remote_access::RemoteAccess,
    ) -> Result<crate::remote_access::RemoteAccess, String> {
        let mut current = self
            .inner
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let next_access = change(&current.remote_access);
        next_access
            .save(&current.preferences)
            .map_err(|error| format!("cannot write the remote access file: {error}"))?;

        let mut next = (**current).clone();
        next.remote_access = next_access.clone();
        *current = Arc::new(next);

        // No event. `remote_access` is `#[serde(skip)]` and carried on the
        // config only so one reading of the file is shared — publishing a
        // config change would send every subscriber the whole payload to
        // announce a field none of them can see.
        Ok(next_access)
    }

    /// Open a subscription: the configuration now, then every change to it.
    pub fn subscribe(&self) -> EventSource {
        // Subscribed to *before* the snapshot function is handed over, so a
        // change landing between here and the pump's first read is delivered
        // as an update rather than falling into the gap. The cost is that such
        // a change can be seen twice — once in the snapshot and once as an
        // update — which the client's projection absorbs, being a fold of
        // wholesale replacements rather than deltas.
        let updates = self.inner.updates.subscribe();
        let store = self.clone();
        EventSource::new(move || vec![snapshot_event(&store.current())], updates)
    }
}

/// The subscription's opening event: the whole configuration, wrapped.
///
/// `version` is a literal `1` in the contract and the client refuses anything
/// else, so it is not a field that tracks the server's own version.
fn snapshot_event(config: &ServerConfig) -> Value {
    json!({"version": 1, "type": "snapshot", "config": config.to_value()})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ConfigStore {
        ConfigStore::new(ServerConfig::detect())
    }

    fn fields(value: &Value) -> Vec<&str> {
        let mut fields: Vec<&str> = value
            .as_object()
            .unwrap_or_else(|| panic!("an object, got {value}"))
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort();
        fields
    }

    fn settings_change(store: &ConfigStore, streaming: bool) -> ConfigChange {
        let mut settings = store.current().settings.clone();
        settings.enable_assistant_streaming = streaming;
        ConfigChange::Settings(Box::new(settings))
    }

    fn provider(version: &str) -> Provider {
        Provider {
            instance_id: "codex".to_string(),
            driver: "codex".to_string(),
            display_name: "Codex".to_string(),
            enabled: true,
            installed: true,
            version: Some(version.to_string()),
            status: crate::config::ProviderState::Ready,
            message: None,
            auth: crate::config::ProviderAuth {
                status: crate::config::AuthStatus::Authenticated,
                r#type: None,
                label: None,
                email: None,
            },
            checked_at: "2026-07-31T00:00:00.000Z".to_string(),
            models: Vec::new(),
            slash_commands: Vec::new(),
            skills: Vec::new(),
        }
    }

    #[test]
    fn an_older_workspace_rescan_cannot_replace_newer_provider_skills() {
        let store = store();
        let older = store.begin_provider_probe("codex");
        let newer = store.begin_provider_probe("codex");

        assert!(store.apply_providers_if_current(newer, |_| true, |_| {
            let mut provider = provider("new");
            provider.skills = vec![json!({"name": "new-workspace"})];
            vec![provider]
        }));
        assert!(!store.apply_providers_if_current(older, |_| true, |_| {
            let mut provider = provider("old");
            provider.skills = vec![json!({"name": "old-workspace"})];
            vec![provider]
        }));

        assert_eq!(store.current().providers[0].version.as_deref(), Some("new"));
        assert_eq!(
            store.current().providers[0].skills,
            vec![json!({"name": "new-workspace"})]
        );
    }

    #[test]
    fn a_probe_for_old_settings_cannot_publish_after_settings_change() {
        let store = store();
        let expected = store.current().settings.providers.codex.clone();
        let probe = store.begin_provider_probe("codex");
        let mut changed = expected.clone();
        changed.binary_path = "new-codex".to_string();
        let mut settings = store.current().settings.clone();
        settings.providers.codex = changed;
        store.apply(ConfigChange::Settings(Box::new(settings)));

        assert!(!store.apply_providers_if_current(
            probe,
            |current| current.settings.providers.codex == expected,
            |_| vec![provider("stale")],
        ));
        assert!(store.current().providers.is_empty());
    }

    #[test]
    fn the_snapshot_is_the_configuration_the_unary_method_returns() {
        let store = store();
        let snapshot = snapshot_event(&store.current());

        assert_eq!(snapshot["version"], json!(1));
        assert_eq!(snapshot["type"], json!("snapshot"));
        assert_eq!(snapshot["config"], store.current().to_value());
    }

    /// A change is not only announced, it is remembered — otherwise a client
    /// that connected a moment later would be told something the server had
    /// already stopped believing.
    #[test]
    fn a_change_is_visible_to_the_next_reader() {
        let store = store();
        assert!(
            store.current().settings.enable_assistant_streaming,
            "the value this test moves off"
        );

        store.apply(settings_change(&store, false));

        assert!(!store.current().settings.enable_assistant_streaming);
        assert_eq!(
            store.current().to_value()["settings"]["enableAssistantStreaming"],
            json!(false)
        );
    }

    /// Every member of the contract's closed union, with the event shape the
    /// client dispatches on. A `type` the client does not know is dropped
    /// silently, so a typo here is a subscription that quietly stops working.
    #[test]
    fn each_change_describes_itself_the_way_the_client_projects_it() {
        let store = store();

        let keybindings = ConfigChange::Keybindings {
            keybindings: Vec::new(),
            issues: Vec::new(),
        }
        .to_event();
        assert_eq!(keybindings["version"], json!(1));
        assert_eq!(keybindings["type"], json!("keybindingsUpdated"));
        assert!(keybindings["payload"]["keybindings"].is_array());
        assert!(keybindings["payload"]["issues"].is_array());

        let providers = ConfigChange::Providers(Vec::new()).to_event();
        assert_eq!(providers["type"], json!("providerStatuses"));
        assert!(providers["payload"]["providers"].is_array());

        let settings = settings_change(&store, true).to_event();
        assert_eq!(settings["type"], json!("settingsUpdated"));
        assert_eq!(
            settings["payload"]["settings"]["enableAssistantStreaming"],
            json!(true)
        );
        // The client replaces its whole `settings` object with this one, so a
        // partial payload would silently drop every field it omitted.
        assert_eq!(
            fields(&settings["payload"]["settings"]),
            fields(&store.current().to_value()["settings"])
        );
    }

    /// The store is one store however many handles exist. A clone that had its
    /// own copy would leave subscribers watching a configuration nothing
    /// updates.
    #[test]
    fn a_clone_is_the_same_store() {
        let store = store();
        let handle = store.clone();

        store.apply(settings_change(&store, true));

        assert!(handle.current().settings.enable_assistant_streaming);
    }

    #[tokio::test]
    async fn an_open_subscription_hears_about_a_change() {
        let store = store();
        let mut updates = store.inner.updates.subscribe();

        store.apply(settings_change(&store, true));

        let event = updates.try_recv().expect("an announcement");
        assert_eq!(event["type"], json!("settingsUpdated"));
        assert_eq!(
            event["payload"]["settings"]["enableAssistantStreaming"],
            json!(true)
        );
    }

    /// The ordering invariant, from the subscriber's side: by the time a
    /// change has been announced, a resynchronisation would already describe a
    /// world in which it happened.
    #[tokio::test]
    async fn a_change_is_stored_before_it_is_announced() {
        let store = store();
        let mut updates = store.inner.updates.subscribe();

        store.apply(settings_change(&store, true));

        assert!(updates.try_recv().is_ok());
        assert!(store.current().settings.enable_assistant_streaming);
    }
}
