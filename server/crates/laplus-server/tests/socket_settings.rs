//! Configuring the app once and having it stay configured.
//!
//! Ticket 22 at the seam the spec calls primary: a real socket, the four methods
//! the settings panel calls, and the subscription every window has open. Nothing
//! here reaches into the server — a setting is read back the way the UI reads it,
//! and "survives a restart" is a second server started on the same directory.
//!
//! ## Two files, and the difference between them matters
//!
//! `settings.json` is **this server's** document: it is written whole, from one
//! place, and nothing else edits it. `keybindings.json` is the **developer's**:
//! its path is advertised in `server.getConfig` precisely so it can be edited by
//! hand, so the tests that matter most for it are the ones where what is on disk
//! is not what this server would have written.
//!
//! ## Where the throwaway directory comes from
//!
//! `TestServer` gives every server a temporary preferences directory of its own
//! — see `harness::TestServer`. The tests here that need *two* servers to share
//! one use `TestServer::start_configured_in`, which is the restart seam.

mod harness;

use harness::agent::{ScriptedAgent, WORKING_DIRECTORY_MARKER};
use harness::conversation::start_turn;
use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};

/// The settings as the panel reads them.
async fn settings(client: &mut harness::SocketClient) -> Value {
    client
        .call("server.getSettings", json!({}))
        .await
        .expect_success()
}

/// One patch, as the panel sends one.
async fn update(client: &mut harness::SocketClient, patch: Value) -> harness::Outcome {
    client
        .call("server.updateSettings", json!({"patch": patch}))
        .await
}

/// The keybinding bound to `command`, out of a `getConfig` payload.
fn bound(config: &Value, command: &str) -> Value {
    config["keybindings"]
        .as_array()
        .unwrap_or_else(|| panic!("keybindings are an array: {config}"))
        .iter()
        .find(|binding| binding["command"] == command)
        .cloned()
        .unwrap_or_else(|| panic!("nothing bound to {command}"))
}

/// The whole first half in one test: a setting is read, changed, and read back
/// — and the change is the value that comes out, not a claim that it worked.
#[tokio::test]
async fn a_setting_can_be_read_and_changed() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let before = settings(&mut client).await;
    assert_eq!(before["addProjectBaseDirectory"], "");

    let after = update(&mut client, json!({"addProjectBaseDirectory": "/work"}))
        .await
        .expect_success();
    assert_eq!(
        after["addProjectBaseDirectory"], "/work",
        "an update answers with the settings now in force"
    );

    // …and the next reader sees it, rather than the answer being a one-off.
    assert_eq!(
        settings(&mut client).await["addProjectBaseDirectory"],
        "/work"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_configured_claude_instance_is_accepted_and_persisted() {
    let agent = ScriptedAgent::emitting(&["{}"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let after = update(
        &mut client,
        json!({"providerInstances": {
            "claudeWork": {
                "driver": "claudeAgent",
                "displayName": "Claude Work",
                "enabled": true,
                "config": {
                    "binaryPath": agent.configured(),
                    "homePath": "/work/claude",
                    "launchArgs": "--verbose",
                    "customModels": ["claude-work-model"]
                }
            }
        }}),
    )
    .await
    .expect_success();

    assert_eq!(
        after["providerInstances"]["claudeWork"]["driver"],
        "claudeAgent"
    );
    assert_eq!(
        after["providerInstances"]["claudeWork"]["displayName"],
        "Claude Work"
    );
    assert_eq!(
        after["providerInstances"]["claudeWork"]["config"]["homePath"],
        "/work/claude"
    );
    assert_eq!(
        settings(&mut client).await["providerInstances"]["claudeWork"]["config"]["customModels"],
        json!(["claude-work-model"])
    );
    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let snapshot = config["providers"]
        .as_array()
        .expect("provider snapshots")
        .iter()
        .find(|provider| provider["instanceId"] == "claudeWork")
        .expect("configured Claude snapshot");
    assert_eq!(snapshot["driver"], "claudeAgent");
    assert_eq!(snapshot["displayName"], "Claude Work");
    assert_eq!(
        snapshot["models"]
            .as_array()
            .expect("models")
            .last()
            .expect("custom model")["slug"],
        "claude-work-model"
    );

    server.stop().await;
}

#[tokio::test]
async fn invalid_provider_instance_envelopes_are_refused_actionably() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    for (patch, named) in [
        (
            json!({"providerInstances": {"1bad": {"driver": "claudeAgent", "displayName": "Bad"}}}),
            "provider instance id",
        ),
        (
            json!({"providerInstances": {"work": {"driver": "opencode", "displayName": "Work"}}}),
            "unsupported driver kind",
        ),
        (
            json!({"providerInstances": {"work": {"driver": "claudeAgent", "displayName": "Work", "config": {"customModels": [""]}}}}),
            "customModels",
        ),
    ] {
        let error = update(&mut client, patch)
            .await
            .expect_declared("ServerSettingsError");
        assert!(
            error["cause"].as_str().unwrap_or_default().contains(named),
            "{error}"
        );
    }
    assert_eq!(settings(&mut client).await["providerInstances"], json!({}));
    server.stop().await;
}

#[tokio::test]
async fn a_turn_routes_through_the_selected_configured_claude_instance() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    update(
        &mut client,
        json!({"providerInstances": {"claudeWork": {
            "driver": "claudeAgent", "displayName": "Claude Work", "enabled": true,
            "config": {"binaryPath": agent.configured()}
        }}}),
    )
    .await
    .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            harness::conversation::create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    let mut turn = start_turn("thread-work", "message-1", "say ok");
    turn["modelSelection"]["instanceId"] = json!("claudeWork");
    turn["bootstrap"]["createThread"]["modelSelection"]["instanceId"] = json!("claudeWork");
    client
        .call("orchestration.dispatchCommand", turn)
        .await
        .expect_success();
    let subscription = client.watch_draft("thread-work").await;
    let events = client.events_through_the_turn(&subscription).await;
    assert!(
        events.iter().any(|event| {
            event["event"]["type"] == "thread.message-sent"
                && event["event"]["payload"]["role"] == "assistant"
                && event["event"]["payload"]["text"] == "ok"
        }),
        "{events:?}"
    );
    assert!(workspace.path().join(WORKING_DIRECTORY_MARKER).exists());
    server.stop().await;
}

#[tokio::test]
async fn disabling_one_claude_instance_does_not_refresh_its_sibling() {
    let first = ScriptedAgent::emitting(&["{}"]);
    let second = ScriptedAgent::emitting(&["{}"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let instance = |binary: String, enabled: bool| {
        json!({
            "driver": "claudeAgent", "displayName": "Claude", "enabled": enabled,
            "config": {"binaryPath": binary}
        })
    };
    update(
        &mut client,
        json!({"providerInstances": {
            "claudeFirst": instance(first.configured(), true),
            "claudeSecond": instance(second.configured(), true)
        }}),
    )
    .await
    .expect_success();
    let sibling_starts = second.starts();

    update(
        &mut client,
        json!({"providerInstances": {
            "claudeFirst": instance(first.configured(), false),
            "claudeSecond": instance(second.configured(), true)
        }}),
    )
    .await
    .expect_success();

    assert_eq!(
        second.starts(),
        sibling_starts,
        "the unchanged instance was probed again"
    );
    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let providers = config["providers"].as_array().expect("providers");
    assert_eq!(
        providers
            .iter()
            .find(|p| p["instanceId"] == "claudeFirst")
            .expect("first")["status"],
        "disabled"
    );
    assert_eq!(
        providers
            .iter()
            .find(|p| p["instanceId"] == "claudeSecond")
            .expect("second")["status"],
        "ready"
    );
    server.stop().await;
}

/// The criterion, driven the only way that means anything: a second server on
/// the same directory is a restart.
#[tokio::test]
async fn settings_survive_a_restart() {
    let preferences = tempfile::tempdir().expect("a temporary directory");

    let server = TestServer::start_configured_in(preferences.path()).await;
    let mut client = server.connect().await;
    update(
        &mut client,
        json!({
            "addProjectBaseDirectory": "/work",
            "enableProviderUpdateChecks": true,
            "providers": {
                "claudeAgent": {"customModels": ["claude-opus-5-ultra"]},
                "codex": {
                    "binaryPath": "/opt/codex",
                    "homePath": "/home/developer/.codex",
                    "launchArgs": "--config model_reasoning_effort=high"
                }
            },
            "providerInstances": {
                "claudeWork": {
                    "driver": "claudeAgent",
                    "displayName": "Claude Work",
                    "enabled": false,
                    "config": {"binaryPath": "/opt/claude-work"}
                }
            },
        }),
    )
    .await
    .expect_success();
    client.close().await;
    server.stop().await;

    let restarted = TestServer::start_configured_in(preferences.path()).await;
    let mut client = restarted.connect().await;

    let after = settings(&mut client).await;
    assert_eq!(after["addProjectBaseDirectory"], "/work");
    assert_eq!(after["enableProviderUpdateChecks"], json!(true));
    assert_eq!(
        after["providers"]["claudeAgent"]["customModels"],
        json!(["claude-opus-5-ultra"])
    );
    assert_eq!(after["providers"]["codex"]["binaryPath"], "/opt/codex");
    assert_eq!(
        after["providers"]["codex"]["homePath"],
        "/home/developer/.codex"
    );
    assert_eq!(
        after["providers"]["codex"]["launchArgs"],
        "--config model_reasoning_effort=high"
    );
    assert_eq!(
        after["providerInstances"]["claudeWork"]["driver"],
        "claudeAgent"
    );
    assert_eq!(
        after["providerInstances"]["claudeWork"]["displayName"],
        "Claude Work"
    );
    assert_eq!(
        after["providerInstances"]["claudeWork"]["config"]["binaryPath"],
        "/opt/claude-work"
    );

    restarted.stop().await;
}

#[tokio::test]
async fn codex_paths_saved_in_settings_drive_the_next_provider_probe() {
    let codex = harness::codex::ScriptedCodex::provider_probe();
    let home = tempfile::tempdir().expect("a CODEX_HOME");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    update(
        &mut client,
        json!({
            "providers": {
                "codex": {
                    "enabled": true,
                    "binaryPath": codex.configured(),
                    "homePath": home.path().display().to_string(),
                    "launchArgs": "--config model_reasoning_effort=high"
                }
            }
        }),
    )
    .await
    .expect_success();

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let provider = config["providers"]
        .as_array()
        .expect("provider snapshots")
        .iter()
        .find(|provider| provider["instanceId"] == "codex")
        .expect("the Codex provider");
    assert_eq!(provider["status"], "ready", "{provider}");
    assert_eq!(codex.codex_home(), home.path().display().to_string());
    assert!(codex.arguments().contains("model_reasoning_effort=high"));
    codex.assert_reaped();

    server.stop().await;
}

#[tokio::test]
async fn a_slow_old_codex_probe_cannot_replace_a_new_settings_probe() {
    let old = harness::codex::ScriptedCodex::blocked_provider_probe_with_email("old@example.com");
    let new = harness::codex::ScriptedCodex::provider_probe_with_email("new@example.com");
    let mut config = laplus_server::config::ServerConfig::detect();
    config.settings.providers.codex.binary_path = old.configured();
    let server = TestServer::start_with(config).await;

    let old_refresh =
        server.refresh_providers_in_background(laplus_server::process::Search::over(&[]));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !old.started() {
        assert!(
            std::time::Instant::now() < deadline,
            "the old probe never reached its deterministic stop point"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let mut client = server.connect().await;
    update(
        &mut client,
        json!({"providers": {"codex": {"binaryPath": new.configured()}}}),
    )
    .await
    .expect_success();

    old.release();
    old_refresh.await.expect("the old probe finishes");

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let codex = config["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["instanceId"] == "codex")
        .expect("Codex provider");
    assert_eq!(codex["auth"]["email"], "new@example.com", "{codex}");

    server.stop().await;
}

/// The Claude instance, configured — and the model list that comes out of it,
/// which is what "including model selection" means from the composer's side.
///
/// The custom slug arrives **after** the built-in ones and marked as custom,
/// because the UI's default model for a new conversation is the first
/// non-custom entry: a developer adding a model must not silently change what
/// their next conversation starts with.
#[tokio::test]
async fn the_claude_instance_can_be_configured_including_the_models_it_offers() {
    let agent = ScriptedAgent::emitting(&["{}"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    server
        .refresh_providers(laplus_server::process::Search::from_environment())
        .await;
    let mut client = server.connect().await;

    update(
        &mut client,
        json!({
            "providers": {"claudeAgent": {"customModels": ["my-own-model", "another"]}},
        }),
    )
    .await
    .expect_success();

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let models = config["providers"][0]["models"]
        .as_array()
        .unwrap_or_else(|| panic!("a model list: {config}"))
        .clone();

    let custom: Vec<&Value> = models
        .iter()
        .filter(|model| model["isCustom"] == json!(true))
        .collect();
    assert_eq!(
        custom
            .iter()
            .map(|model| model["slug"].as_str().unwrap_or_default())
            .collect::<Vec<&str>>(),
        vec!["my-own-model", "another"],
        "{models:#?}"
    );
    assert_eq!(
        models[0]["isCustom"],
        json!(false),
        "a custom model must not become the default a new conversation starts with"
    );

    server.stop().await;
}

/// The criterion, and it is two claims: the update is refused *with a sentence*,
/// and nothing moved.
///
/// Driven with a patch whose first field is perfectly good, because a
/// half-applied patch is the failure this is about — and read back through the
/// socket rather than out of the answer, since the answer to a refused call is
/// the refusal.
#[tokio::test]
async fn an_invalid_setting_is_refused_and_leaves_the_previous_values_intact() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    update(&mut client, json!({"addProjectBaseDirectory": "/before"}))
        .await
        .expect_success();

    let error = update(
        &mut client,
        json!({
            "addProjectBaseDirectory": "/after",
            "automaticGitFetchInterval": "whenever",
        }),
    )
    .await
    .expect_declared("ServerSettingsError");
    assert!(
        error["cause"]
            .as_str()
            .expect("a sentence the panel can show")
            .contains("automaticGitFetchInterval"),
        "{error}"
    );

    assert_eq!(
        settings(&mut client).await["addProjectBaseDirectory"],
        "/before",
        "a refused patch was half applied"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_codex_shadow_home_is_refused_as_unsupported_account_selection() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = update(
        &mut client,
        json!({
            "addProjectBaseDirectory": "/after",
            "providers": {"codex": {"shadowHomePath": "/accounts/work"}},
        }),
    )
    .await
    .expect_declared("ServerSettingsError");
    let cause = error["cause"]
        .as_str()
        .expect("a sentence the panel can show");
    assert!(cause.contains("account-selection"), "{error}");
    assert!(cause.contains("one Codex account"), "{error}");
    assert_eq!(settings(&mut client).await["addProjectBaseDirectory"], "");

    server.stop().await;
}

/// A change reaches a window that is already open, without a restart. The
/// subscription is the one every window holds, and `settingsUpdated` is the
/// event the client projects onto its own copy.
#[tokio::test]
async fn a_change_reaches_an_open_window_without_a_restart() {
    let server = TestServer::start().await;
    let mut watcher = server.connect().await;
    let subscription = watcher.subscribe("subscribeServerConfig", json!({})).await;
    // The snapshot the subscription opens with, so what follows is the change.
    watcher.next_chunk(&subscription).await;
    watcher.ack(&subscription).await;

    // A *different* connection makes the change, which is what two windows on
    // one app look like.
    let mut other = server.connect().await;
    update(&mut other, json!({"addProjectBaseDirectory": "/work"}))
        .await
        .expect_success();

    let event = watcher.next_event(&subscription).await;
    assert_eq!(event["type"], "settingsUpdated", "{event}");
    assert_eq!(
        event["payload"]["settings"]["addProjectBaseDirectory"],
        "/work"
    );

    server.stop().await;
}

/// Binding, rebinding and unbinding — the three the ticket names, each answering
/// with the whole configuration the UI should now hold.
#[tokio::test]
async fn a_keybinding_can_be_added_changed_and_removed() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let bound_to = |answer: &Value, command: &str| bound(answer, command)["shortcut"].clone();

    let added = client
        .call(
            "server.upsertKeybinding",
            json!({"key": "ctrl+alt+s", "command": "sidebar.toggle"}),
        )
        .await
        .expect_success();
    assert_eq!(bound_to(&added, "sidebar.toggle")["key"], "s");
    assert_eq!(bound_to(&added, "sidebar.toggle")["ctrlKey"], json!(true));
    assert_eq!(
        bound_to(&added, "terminal.toggle")["key"],
        "j",
        "rebinding one shortcut moved another"
    );

    // Changed, naming what it replaces — without which the old key would still
    // fire alongside the new one.
    let changed = client
        .call(
            "server.upsertKeybinding",
            json!({
                "key": "ctrl+alt+z",
                "command": "sidebar.toggle",
                "replace": {"key": "ctrl+alt+s", "command": "sidebar.toggle"},
            }),
        )
        .await
        .expect_success();
    assert_eq!(bound_to(&changed, "sidebar.toggle")["key"], "z");
    assert_eq!(
        changed["keybindings"]
            .as_array()
            .expect("an array")
            .iter()
            .filter(|binding| binding["command"] == "sidebar.toggle")
            .count(),
        1,
        "the shortcut it replaced is still bound: {changed:#?}"
    );

    // Removed, and the default comes back — it was shadowed, never deleted.
    let removed = client
        .call(
            "server.removeKeybinding",
            json!({"key": "ctrl+alt+z", "command": "sidebar.toggle"}),
        )
        .await
        .expect_success();
    assert_eq!(bound_to(&removed, "sidebar.toggle")["key"], "b");
    assert_eq!(bound_to(&removed, "sidebar.toggle")["modKey"], json!(true));

    server.stop().await;
}

/// Keybindings survive a restart too, and they survive it in the developer's own
/// file rather than in this server's memory.
#[tokio::test]
async fn keybindings_survive_a_restart_and_reach_an_open_window() {
    let preferences = tempfile::tempdir().expect("a temporary directory");

    let server = TestServer::start_configured_in(preferences.path()).await;
    let mut watcher = server.connect().await;
    let subscription = watcher.subscribe("subscribeServerConfig", json!({})).await;
    watcher.next_chunk(&subscription).await;
    watcher.ack(&subscription).await;

    let mut client = server.connect().await;
    client
        .call(
            "server.upsertKeybinding",
            json!({"key": "ctrl+alt+s", "command": "sidebar.toggle", "when": "!terminalFocus"}),
        )
        .await
        .expect_success();

    // The open window hears about it.
    let event = watcher.next_event(&subscription).await;
    assert_eq!(event["type"], "keybindingsUpdated", "{event}");
    assert_eq!(
        bound(&event["payload"], "sidebar.toggle")["shortcut"]["key"],
        "s"
    );
    // The condition arrives as a tree rather than as the text that was typed,
    // because the client evaluates it and holds no parser.
    assert_eq!(
        bound(&event["payload"], "sidebar.toggle")["whenAst"],
        json!({"type": "not", "node": {"type": "identifier", "name": "terminalFocus"}})
    );

    client.close().await;
    watcher.close().await;
    server.stop().await;

    let restarted = TestServer::start_configured_in(preferences.path()).await;
    let mut client = restarted.connect().await;
    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(bound(&config, "sidebar.toggle")["shortcut"]["key"], "s");

    restarted.stop().await;
}

/// The criterion that keeps a bad file from being a dead app: a corrupt store
/// falls back to the defaults *and says so*, rather than stopping the server.
///
/// Both files, because both are read at startup and either could be the one that
/// is broken. The warning lands in `issues`, which the UI already renders.
#[tokio::test]
async fn a_corrupt_store_falls_back_to_defaults_with_a_warning_rather_than_failing_to_start() {
    let preferences = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(preferences.path().join("settings.json"), "{ not json").expect("writes");
    std::fs::write(preferences.path().join("keybindings.json"), "not json either")
        .expect("writes");

    let server = TestServer::start_configured_in(preferences.path()).await;
    let mut client = server.connect().await;

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    // The app is up, and the defaults are in force for both.
    assert_eq!(config["settings"]["addProjectBaseDirectory"], "");
    assert_eq!(bound(&config, "sidebar.toggle")["shortcut"]["key"], "b");

    // The keybindings file gets a row, because the contract has one for it —
    // and it has to be one of the **two literals** `ServerConfigIssue` allows.
    // A kind of this server's own invention would not be an oddly-named row: it
    // would fail the client's decode of this whole payload, so a broken
    // keybindings file would stop the app opening. Which is what this test is
    // about.
    let kinds: Vec<&str> = config["issues"]
        .as_array()
        .unwrap_or_else(|| panic!("issues are an array: {config}"))
        .iter()
        .map(|issue| issue["kind"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["keybindings.malformed-config"], "{config}");

    // The settings file gets none, and that is the declared gap rather than an
    // omission: the union has no member for a settings problem, so the warning
    // goes to the log. See `crate::settings::load`.
    server.stop().await;
}

/// A binding the file could not hold is refused before it reaches the file,
/// under the tag the client decodes — and the configuration is untouched.
#[tokio::test]
async fn an_unreadable_keybinding_is_refused_by_name() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "server.upsertKeybinding",
            json!({"key": "a+b", "command": "sidebar.toggle"}),
        )
        .await
        .expect_declared("KeybindingsConfigParseError");
    assert!(error["detail"].is_string(), "{error}");

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(bound(&config, "sidebar.toggle")["shortcut"]["key"], "b");

    server.stop().await;
}

/// The last criterion, end to end: what the developer configures is what the
/// next agent session runs.
///
/// Driven by pointing `binaryPath` at a *second* stand-in and checking that the
/// second one is the one that took the turn. That is the strongest form of the
/// claim available — the setting is the real production seam
/// (`settings.providers.claudeAgent.binaryPath`), and nothing about the assertion
/// reaches into the server: it is the agent's own record of having been started.
#[tokio::test]
async fn a_newly_configured_provider_is_used_by_the_next_agent_session() {
    let first = ScriptedAgent::emitting(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the old one"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10}"#,
    ]);
    let second = ScriptedAgent::emitting(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"the new one"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&first.configured()).await;
    let mut client = server.connect().await;

    update(
        &mut client,
        json!({"providers": {"claudeAgent": {"binaryPath": second.configured()}}}),
    )
    .await
    .expect_success();

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert_eq!(second.starts(), 1, "the newly configured agent never ran");
    assert_eq!(
        first.starts(),
        0,
        "the turn went to the agent configured before the change"
    );

    server.stop().await;
}
