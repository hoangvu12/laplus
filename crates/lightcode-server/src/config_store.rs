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
//! One thing mutates the configuration: [`crate::provider::refresh`] publishes
//! `providers` when it has resolved the `claude` binary, which is the first real
//! use these two invariants were built for. Ticket 22 owns keybindings and
//! settings. [`ConfigChange`] is the vocabulary, and it is closed on purpose: it
//! mirrors `ServerConfigStreamEvent` in the contract, which has exactly these
//! three update members plus the snapshot.

use std::sync::{Arc, RwLock};

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
    updates: broadcast::Sender<Value>,
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
    pub fn new(config: ServerConfig) -> ConfigStore {
        ConfigStore {
            inner: Arc::new(Inner {
                current: RwLock::new(Arc::new(config)),
                updates: broadcast::channel(BACKLOG).0,
            }),
        }
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
            !store.current().settings.enable_assistant_streaming,
            "the value this test moves off"
        );

        store.apply(settings_change(&store, true));

        assert!(store.current().settings.enable_assistant_streaming);
        assert_eq!(
            store.current().to_value()["settings"]["enableAssistantStreaming"],
            json!(true)
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
