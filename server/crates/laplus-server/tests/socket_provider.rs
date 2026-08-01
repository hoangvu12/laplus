//! Provider configuration and agent binary resolution, driven through the
//! socket.
//!
//! Ticket 09's last criterion asks for the resolution order and each failure
//! mode to be covered *through the socket boundary*, and "without requiring a
//! real agent binary to be installed". Both halves matter, and together they
//! decide the shape of this file: every test here writes its own stand-in binary
//! (see [`harness::agent`]) and reads the answer back out of `server.getConfig`
//! or the configuration subscription, which is where the UI would read it.
//!
//! Nothing here sets `PATH`. It is process-global mutable state, so a test that
//! set it would be changing it for every other test running beside it; the
//! directories a lookup may see are passed in instead, which is what
//! `process::Search` is for. The spec's rule that the fake CLI goes in "through
//! the existing agent-executable-path configuration … so no test-only seam is
//! added to production code" is honoured both ways: `binaryPath` is a real
//! setting, and a `Search` is data a real caller supplies.
//!
//! **What is driven where.** The resolution *rules* — which of two candidates
//! wins, what a name means against what a path means, how a version string is
//! read — are unit tests in `provider.rs`, where a failure names the case rather
//! than a JSON pointer, and where a private function can be reached. What is here
//! is only what the UI observes, and it is not a second copy of the same
//! assertions: precedence, for instance, is asserted through the reported
//! *version* rather than the resolved path, because the version is the outcome and
//! the path is the mechanism.
//!
//! One trigger is real and the rest are injected, which the last criterion allows
//! and this says out loud. There is no method on this wire that means "re-probe
//! the provider" — upstream refreshes on a timer and on settings changes, and
//! laplus has neither yet — so the *cause* cannot come over the socket.
//! `the_provider_is_resolved_by_the_call_the_app_makes_at_startup` drives
//! `Server::probe_provider`, the production trigger; every other test calls the
//! same resolution with directories of its own and reads the answer where the UI
//! would.

mod harness;

use harness::agent::FakeAgent;
use harness::TestServer;
use laplus_server::config::{ClaudeSettings, CodexSettings, ProviderState, ServerConfig};
use laplus_server::process::Search;
use serde_json::{json, Value};

/// A server configuration whose provider settings are the test's.
///
/// The `binaryPath` setting is the seam the spec names for injecting a stand-in
/// agent — "a value the server already needs for real use, so no test-only seam
/// is added to production code".
fn configured(binary_path: &str) -> ServerConfig {
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent = ClaudeSettings {
        binary_path: binary_path.to_string(),
        ..config.settings.providers.claude_agent
    };
    config
}

/// The `providers` array in a `server.getConfig` answer, read the way the UI
/// reads it.
async fn providers_over_the_socket(server: &TestServer) -> Vec<Value> {
    let mut client = server.connect().await;
    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let providers = config["providers"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of providers: {config}"))
        .clone();
    client.close().await;
    providers
}

/// The Claude provider instance, over the socket.
async fn provider_over_the_socket(server: &TestServer) -> Value {
    let providers = providers_over_the_socket(server).await;
    providers
        .into_iter()
        .find(|provider| provider["instanceId"] == "claudeAgent")
        .expect("the Claude provider")
}

async fn provider_named(server: &TestServer, instance_id: &str) -> Value {
    providers_over_the_socket(server)
        .await
        .into_iter()
        .find(|provider| provider["instanceId"] == instance_id)
        .unwrap_or_else(|| panic!("no provider named {instance_id}"))
}

#[tokio::test]
async fn codex_probe_reports_its_account_paged_models_reasoning_and_workspace_skills() {
    let codex = harness::codex::ScriptedCodex::provider_probe();
    let home = tempfile::tempdir().expect("a CODEX_HOME");
    let mut config = ServerConfig::detect();
    config.settings.providers.codex = CodexSettings {
        binary_path: codex.configured(),
        home_path: home.path().display().to_string(),
        launch_args: "--config model_reasoning_effort=high".to_string(),
        ..config.settings.providers.codex
    };
    let server = TestServer::start_with(config).await;

    server.refresh_providers(Search::over(&[])).await;
    codex.assert_reaped();
    codex.assert_exchange();
    let provider = provider_named(&server, "codex").await;

    assert_eq!(provider["driver"], "codex", "{provider}");
    assert_eq!(provider["displayName"], "Codex", "{provider}");
    assert_eq!(provider["status"], "ready", "{provider}");
    assert_eq!(provider["version"], "0.146.0", "{provider}");
    assert_eq!(provider["auth"]["status"], "authenticated", "{provider}");
    assert_eq!(
        provider["auth"]["email"], "developer@example.com",
        "{provider}"
    );
    assert_eq!(provider["auth"]["type"], "chatgpt", "{provider}");
    assert_eq!(
        provider["auth"]["label"], "ChatGPT Pro 5x Subscription",
        "{provider}"
    );

    assert_eq!(slugs(&provider), vec!["gpt-5.6-sol", "gpt-5.6-luna"]);
    assert_eq!(
        provider["models"][0].get("isDefault"),
        None,
        "laplus must not replace the agent's default: {provider}"
    );
    assert_eq!(provider["models"][1]["isDefault"], true, "{provider}");
    assert_eq!(
        provider["models"][0]["capabilities"]["optionDescriptors"][0],
        json!({
            "id": "reasoningEffort",
            "label": "Reasoning",
            "type": "select",
            "options": [
                {"id": "low", "label": "Low"},
                {"id": "high", "label": "High", "isDefault": true}
            ],
            "currentValue": "high"
        })
    );
    assert_eq!(
        provider["models"][1]["capabilities"]["optionDescriptors"][0]["options"]
            .as_array()
            .map(Vec::len),
        Some(3),
        "reasoning efforts belong to each model: {provider}"
    );
    assert_eq!(provider["skills"][0]["name"], "tdd", "{provider}");
    assert_eq!(provider["skills"][0]["scope"], "repo", "{provider}");
    assert_eq!(
        provider["skills"][0]["displayName"], "Test Driven Development",
        "{provider}"
    );

    assert!(
        codex.arguments().starts_with("app-server"),
        "{}",
        codex.arguments()
    );
    assert!(
        codex.arguments().contains("--config"),
        "{}",
        codex.arguments()
    );
    assert!(
        codex.arguments().contains("model_reasoning_effort=high"),
        "{}",
        codex.arguments()
    );
    assert_eq!(codex.codex_home(), home.path().display().to_string());
    assert_eq!(
        codex.skill_cwds(),
        vec![std::env::current_dir()
            .expect("the test has a working directory")
            .display()
            .to_string()]
    );

    server.stop().await;
}

#[tokio::test]
async fn the_default_codex_instance_uses_its_generic_configuration() {
    let codex = harness::codex::ScriptedCodex::provider_probe();
    let home = tempfile::tempdir().expect("a CODEX_HOME");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .call(
            "server.updateSettings",
            json!({"patch": {"providerInstances": {"codex": {
                "driver": "codex",
                "displayName": "Codex Default",
                "enabled": true,
                "config": {
                    "binaryPath": codex.configured(),
                    "homePath": home.path().display().to_string(),
                    "launchArgs": "--config model_reasoning_effort=high",
                    "customModels": ["instance-codex-model"]
                }
            }}}}),
        )
        .await
        .expect_success();

    let provider = provider_named(&server, "codex").await;
    assert_eq!(provider["driver"], "codex", "{provider}");
    assert_eq!(provider["displayName"], "Codex Default", "{provider}");
    assert_eq!(provider["status"], "ready", "{provider}");
    assert!(slugs(&provider).iter().any(|slug| slug == "instance-codex-model"));
    assert_eq!(codex.codex_home(), home.path().display().to_string());
    assert!(codex.arguments().contains("model_reasoning_effort=high"));

    server.stop().await;
}

#[tokio::test]
async fn a_targeted_refresh_accepts_a_configured_codex_instance() {
    let codex = harness::codex::ScriptedCodex::provider_probe();
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    client
        .call(
            "server.updateSettings",
            json!({"patch": {"providerInstances": {"codexWork": {
                "driver": "codex",
                "displayName": "Codex Work",
                "enabled": true,
                "config": {"binaryPath": codex.configured()}
            }}}}),
        )
        .await
        .expect_success();

    let refreshed = client
        .call(
            "server.refreshProviders",
            json!({"instanceId": "codexWork"}),
        )
        .await
        .expect_success();
    let provider = refreshed["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["instanceId"] == "codexWork")
        .expect("the targeted Codex instance");
    assert_eq!(provider["displayName"], "Codex Work");
    assert_eq!(provider["status"], "ready");

    server.stop().await;
}

#[tokio::test]
async fn a_logged_out_codex_is_named_as_unauthenticated_without_losing_its_models() {
    // The transport fixture's account response is replaced while all other
    // responses, including the paged model catalogue, stay the same.
    let codex = harness::codex::ScriptedCodex::logged_out_provider_probe();
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;

    server.refresh_providers(Search::over(&[])).await;
    let provider = provider_named(&server, "codex").await;

    assert_eq!(provider["installed"], true, "{provider}");
    assert_eq!(provider["status"], "error", "{provider}");
    assert_eq!(provider["auth"]["status"], "unauthenticated", "{provider}");
    assert!(message(&provider).contains("codex login"), "{provider}");
    assert_eq!(slugs(&provider), vec!["gpt-5.6-sol", "gpt-5.6-luna"]);

    server.stop().await;
}

#[tokio::test]
async fn malformed_required_codex_responses_are_reported_as_broken_not_ready_and_empty() {
    for (codex, field) in [
        (harness::codex::ScriptedCodex::missing_user_agent(), "userAgent"),
        (harness::codex::ScriptedCodex::missing_model_data(), "data"),
        (harness::codex::ScriptedCodex::missing_skills_data(), "data"),
    ] {
        let mut config = ServerConfig::detect();
        config.settings.providers.codex.binary_path = codex.configured();
        let server = TestServer::start_with(config).await;

        server.refresh_providers(Search::over(&[])).await;
        let provider = provider_named(&server, "codex").await;

        assert_eq!(provider["status"], "error", "{provider}");
        assert_eq!(provider["auth"]["status"], "unknown", "{provider}");
        assert!(message(&provider).contains(field), "{provider}");
        codex.assert_reaped();
        server.stop().await;
    }
}

#[tokio::test]
async fn a_logged_out_codex_keeps_login_guidance_when_a_stale_path_falls_back() {
    let codex = harness::codex::ScriptedCodex::logged_out_provider_probe();
    let missing = tempfile::tempdir()
        .expect("a stale install directory")
        .path()
        .join(if cfg!(windows) { "codex.cmd" } else { "codex" });
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = missing.display().to_string();
    let server = TestServer::start_with(config).await;

    server
        .refresh_providers(Search::over(&[codex.directory()]))
        .await;
    let provider = provider_named(&server, "codex").await;

    assert_eq!(provider["auth"]["status"], "unauthenticated", "{provider}");
    let diagnostic = message(&provider);
    assert!(diagnostic.contains(&missing.display().to_string()), "{diagnostic}");
    assert!(diagnostic.contains("codex login"), "{diagnostic}");

    server.stop().await;
}

#[tokio::test]
async fn server_shutdown_cancels_and_reaps_an_in_flight_codex_probe() {
    let codex = harness::codex::ScriptedCodex::blocked_provider_probe_with_email(
        "blocked@example.com",
    );
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;

    server.probe_provider();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !codex.started() {
        assert!(
            std::time::Instant::now() < deadline,
            "the Codex probe never reached its blocked response"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    server.stop().await;

    #[cfg(not(windows))]
    {
        let survived_shutdown = codex.running();
        if survived_shutdown {
            codex.release();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while codex.running() && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        assert!(!survived_shutdown, "Codex outlived orderly server shutdown");
    }
    #[cfg(windows)]
    codex.assert_reaped();
}

fn message(provider: &Value) -> String {
    provider["message"]
        .as_str()
        .unwrap_or_else(|| panic!("a diagnostic: {provider}"))
        .to_string()
}

/// The model slugs a provider is offering, in the order it offers them — the
/// first is the one the UI will default to.
fn slugs(provider: &Value) -> Vec<String> {
    provider["models"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of models: {provider}"))
        .iter()
        .map(|model| model["slug"].as_str().expect("a slug").to_string())
        .collect()
}

/// Before anything has looked, there is no provider instance — and that is the
/// answer rather than a gap.
///
/// The tempting alternative was a placeholder saying "not checked yet", and it is
/// wrong in a way only the UI shows: `getProviderSummary` tests `!installed`
/// *before* it looks at `status`, so a placeholder reads as **"Not found"**, and
/// `shouldShowProviderStatusBanner` returns true for anything that is not `ready`
/// or `disabled`, so every launch would flash a warning alert in the chat view.
/// An absent instance is what upstream's own copy is written for — "Checking
/// provider status / Waiting for the server to report installation and
/// authentication details."
#[tokio::test]
async fn there_is_no_provider_instance_until_something_has_looked_for_one() {
    let server = TestServer::start().await;

    assert_eq!(providers_over_the_socket(&server).await, Vec::<Value>::new());

    server.stop().await;
}

/// What the instance says about itself once there is one. These are the fields
/// the UI routes on, and three of them are branded slugs the client decodes
/// against a pattern, so an empty one costs the whole configuration payload.
#[tokio::test]
async fn the_resolved_instance_names_the_driver_the_ui_routes_through() {
    let agent = FakeAgent::reporting("2.1.220");
    let server = TestServer::start().await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["instanceId"], json!("claudeAgent"));
    assert_eq!(provider["driver"], json!("claudeAgent"));
    assert_eq!(provider["displayName"], json!("Claude"));
    assert_eq!(provider["auth"]["status"], json!("unknown"));
    assert!(provider["checkedAt"].is_string(), "{provider}");

    server.stop().await;
}

/// The default configuration is a bare `claude`, and finding it takes no
/// configuration at all: it is on `PATH`, so it is found, its version is read
/// back, and the UI is shown a ready instance.
///
/// This is criterion one and criteria five and six together, which is the point
/// — "ready" without a version would be a claim this server cannot support.
#[tokio::test]
async fn the_binary_on_path_is_found_without_configuration_and_reported_ready() {
    let agent = FakeAgent::reporting("2.1.220");
    let server = TestServer::start().await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("ready"));
    assert_eq!(provider["installed"], json!(true));
    assert_eq!(provider["enabled"], json!(true));
    assert_eq!(provider["version"], json!("2.1.220"));
    assert_eq!(
        provider.get("message"),
        None,
        "a working provider owes the developer no sentence: {provider}"
    );

    // Ready, enabled and available is exactly what the UI's picker requires of
    // an instance before it will offer it — and it has a model to offer with it.
    assert!(
        provider["models"]
            .as_array()
            .is_some_and(|models| !models.is_empty()),
        "{provider}"
    );

    server.stop().await;
}

/// A configured path wins over a binary on `PATH`. Both are startable and both
/// report a version, so the version is what says which one ran — asserting on
/// the path would be asserting on the resolver rather than on the outcome.
#[tokio::test]
async fn an_explicitly_configured_path_takes_precedence_over_the_path_lookup() {
    let configured_agent = FakeAgent::reporting("1.2.3");
    let on_path = FakeAgent::reporting("9.9.9");
    let server = TestServer::start_with(configured(&configured_agent.configured())).await;

    server
        .refresh_providers(Search::over(&[on_path.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("ready"));
    assert_eq!(provider["version"], json!("1.2.3"));

    server.stop().await;
}

/// A configured path that has gone away falls through to `PATH`, and the
/// developer is told — otherwise the version above would be from a binary they
/// never named, with nothing anywhere to say so.
#[tokio::test]
async fn a_configured_path_that_is_gone_falls_back_to_path_and_says_which_binary_answered() {
    let agent = FakeAgent::reporting("2.1.220");
    let stale = agent.stale_path();
    let server = TestServer::start_with(configured(&stale)).await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("ready"));
    assert_eq!(provider["version"], json!("2.1.220"));

    let message = message(&provider);
    assert!(message.contains(&stale), "{message}");
    assert!(
        message.contains(&agent.path().display().to_string()),
        "{message}"
    );

    server.stop().await;
}

/// The diagnostic the ticket exists for: enough to fix the problem without
/// opening a log file. It names the configured path and every directory that was
/// searched, and it arrives over the same socket the UI is already reading.
#[tokio::test]
async fn a_missing_binary_names_the_configured_path_and_the_directories_searched() {
    let searched = tempfile::tempdir().expect("a temporary directory");
    let elsewhere = tempfile::tempdir().expect("a temporary directory");
    let missing = elsewhere.path().join("nowhere").join("claude.exe");
    let server = TestServer::start_with(configured(&missing.to_string_lossy())).await;

    server
        .refresh_providers(Search::over(&[searched.path()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("error"));
    assert_eq!(provider["installed"], json!(false));
    assert_eq!(provider["version"], Value::Null);

    let message = message(&provider);
    assert!(message.contains(&missing.display().to_string()), "{message}");
    assert!(
        message.contains(&searched.path().display().to_string()),
        "{message}"
    );
    assert!(message.contains("PATH"), "{message}");

    server.stop().await;
}

/// A configured path that exists and is not a program is its own diagnosis. The
/// two sentences have to differ, because the two fixes do: one is a path to
/// correct, the other is a file to replace.
#[tokio::test]
async fn a_configured_path_that_is_not_executable_is_reported_distinctly() {
    let agent = FakeAgent::reporting("2.1.220");
    let not_a_program = agent.directory().join("claude-notes.txt");
    std::fs::write(&not_a_program, "not a program").expect("writes the file");

    let broken = TestServer::start_with(configured(&not_a_program.to_string_lossy())).await;
    broken
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let broken_provider = provider_over_the_socket(&broken).await;

    let absent = TestServer::start_with(configured(&agent.stale_path())).await;
    absent.refresh_providers(Search::over(&[])).await;
    let absent_provider = provider_over_the_socket(&absent).await;

    assert_eq!(broken_provider["status"], json!("error"));
    assert_eq!(broken_provider["installed"], json!(false));
    assert!(
        message(&broken_provider).contains(&not_a_program.display().to_string()),
        "{broken_provider}"
    );
    assert!(
        message(&broken_provider).contains("exists but is not a program"),
        "{broken_provider}"
    );

    assert_ne!(
        message(&broken_provider),
        message(&absent_provider),
        "the two ways of having no binary have to read differently"
    );

    // And a startable binary *was* on PATH, which this deliberately did not use:
    // a file the developer named and got wrong is reported, not worked around.
    assert_eq!(broken_provider["version"], Value::Null);

    broken.stop().await;
    absent.stop().await;
}

/// A directory where a program should be. The same refusal as a file that is not
/// a program, and worth driving separately because it is the mistake a developer
/// actually makes — configuring the install directory rather than the binary in
/// it.
#[tokio::test]
async fn a_configured_path_that_is_a_directory_is_refused_rather_than_started() {
    let agent = FakeAgent::reporting("2.1.220");
    let server = TestServer::start_with(configured(&agent.directory().to_string_lossy())).await;

    server.refresh_providers(Search::over(&[])).await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("error"));
    assert!(
        message(&provider).contains("is not a program"),
        "{provider}"
    );

    server.stop().await;
}

/// An install that is there and does not work. `installed` stays true, because
/// "not found" and "found and broken" send a developer to different places.
#[tokio::test]
async fn a_binary_that_fails_to_answer_is_installed_but_not_ready() {
    let agent = FakeAgent::failing();
    let server = TestServer::start().await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("error"));
    assert_eq!(provider["installed"], json!(true));
    assert_eq!(provider["version"], Value::Null);
    assert!(message(&provider).contains("status 1"), "{provider}");

    server.stop().await;
}

/// An install that runs and says nothing this server can read is a *warning*
/// rather than an error: it may well run a turn, so the instance stays usable —
/// but with only the models every version supports, since nothing established
/// which version this is.
#[tokio::test]
async fn a_binary_whose_version_cannot_be_read_is_a_warning_with_a_shorter_model_list() {
    let agent = FakeAgent::saying("echo ready when you are");
    let server = TestServer::start().await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("warning"));
    assert_eq!(provider["installed"], json!(true));
    assert_eq!(provider["version"], Value::Null);
    assert!(
        message(&provider).contains("ready when you are"),
        "the diagnostic quotes what it actually said: {provider}"
    );
    assert!(
        !slugs(&provider).contains(&"claude-opus-5".to_string()),
        "a gated slug must not be offered for a version nothing established: {provider}"
    );

    server.stop().await;
}

/// A driver the developer switched off is not looked for at all, and says so
/// with the contract's own `disabled` — which is the one state the UI hides the
/// instance for rather than raising a banner about.
#[tokio::test]
async fn a_disabled_provider_is_reported_as_disabled_rather_than_missing() {
    let agent = FakeAgent::reporting("2.1.220");
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent.enabled = false;
    let server = TestServer::start_with(config).await;

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(provider["status"], json!("disabled"));
    assert_eq!(provider["enabled"], json!(false));
    assert_eq!(provider["installed"], json!(false));
    assert!(message(&provider).contains("switched off"), "{provider}");

    server.stop().await;
}

/// The UI does not poll for this. It subscribes to the configuration during boot
/// and is *told* — so what a subscriber receives has to be the same provider
/// `server.getConfig` would answer with, and it has to arrive without asking
/// again.
#[tokio::test]
async fn the_resolved_provider_reaches_an_open_subscriber() {
    let agent = FakeAgent::reporting("2.1.220");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe("subscribeServerConfig", json!({})).await;
    let snapshot = client.next_event(&subscription).await;
    assert_eq!(
        snapshot["config"]["providers"],
        json!([]),
        "the snapshot describes the world before anything looked"
    );

    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;

    let events = client
        .values_until(&subscription, |event| {
            event["payload"]["providers"]
                .as_array()
                .is_some_and(|providers| {
                    providers
                        .iter()
                        .any(|provider| provider["instanceId"] == "codex")
                })
        })
        .await;
    let event = events.last().expect("the provider update");
    assert_eq!(event["type"], json!("providerStatuses"));
    let streamed = event["payload"]["providers"]
        .as_array()
        .and_then(|providers| {
            providers
                .iter()
                .find(|provider| provider["instanceId"] == "claudeAgent")
        })
        .expect("the streamed Claude provider");
    assert_eq!(streamed["status"], json!("ready"));
    assert_eq!(streamed["version"], json!("2.1.220"));

    client.close().await;

    // The same value, not merely a similar one: a client that had cached the
    // snapshot and applied the update must end up where a client that connected
    // afterwards starts.
    assert_eq!(*streamed, provider_over_the_socket(&server).await);

    server.stop().await;
}

/// `Server::probe_provider` is the method the app's startup calls and the one a
/// later refresh will call again — the production trigger, driven here rather
/// than argued about.
///
/// It reads the machine's own `PATH`, which a test has no business changing, so
/// the stand-in is injected the way the spec says: through `binaryPath`. Nothing
/// on the machine can change the answer, because a configured path that exists is
/// never fallen back from.
#[tokio::test]
async fn the_provider_is_resolved_by_the_call_the_app_makes_at_startup() {
    let agent = FakeAgent::reporting("2.1.220");
    let server = TestServer::start_with(configured(&agent.configured())).await;

    server.probe_provider();

    let provider = server.await_provider_state(ProviderState::Ready).await;
    assert_eq!(provider["version"], json!("2.1.220"));

    // And over the socket, which is where the UI would see it.
    assert_eq!(provider, provider_over_the_socket(&server).await);

    server.stop().await;
}

/// The models on offer are the ones the version that answered supports. This is
/// what makes the version load-bearing rather than decorative: an old CLI is
/// never offered a slug it would reject.
#[tokio::test]
async fn the_models_offered_follow_the_version_that_answered() {
    let old = FakeAgent::reporting("2.1.100");
    let current = FakeAgent::reporting("2.1.220");
    let server = TestServer::start().await;

    server
        .refresh_providers(Search::over(&[old.directory()]))
        .await;
    let behind = provider_over_the_socket(&server).await;

    server
        .refresh_providers(Search::over(&[current.directory()]))
        .await;
    let ahead = provider_over_the_socket(&server).await;

    assert!(!slugs(&behind).contains(&"claude-opus-5".to_string()), "{behind}");
    assert!(slugs(&ahead).contains(&"claude-opus-5".to_string()), "{ahead}");
    assert!(
        slugs(&ahead).len() > slugs(&behind).len(),
        "a newer CLI is offered more, not different: {behind} then {ahead}"
    );

    // The gate is silent, so the old CLI is *told* why its list is short — and
    // the current one, having everything, is told nothing. Without this the
    // developer sees fewer models than the release notes promised and has no way
    // to find out why.
    assert_eq!(behind["status"], json!("ready"), "still usable: {behind}");
    let advice = message(&behind);
    assert!(advice.contains("Claude Opus 5"), "{advice}");
    assert!(advice.contains("v2.1.219"), "{advice}");
    assert_eq!(ahead.get("message"), None, "{ahead}");

    server.stop().await;
}

/// The machine's own Claude Code, resolved off its real `PATH`.
///
/// Every other test here proves the resolver against a file it wrote itself,
/// which is what makes the suite offline and deterministic — and also means not
/// one of them would notice if the real binary were, say, a launcher script this
/// server cannot start. This is the test that would, and it is the only claim in
/// the ticket that a fixture cannot stand in for.
///
/// Skipped unless asked for, the same way
/// `editor::tests::the_file_manager_can_be_started` is: a suite that failed on a
/// machine without the agent installed would be a suite that could not run on the
/// machine it is meant to prove things about.
#[tokio::test]
async fn the_real_agent_on_this_machine_is_found_and_reports_its_version() {
    if std::env::var_os("LAPLUS_TEST_REAL_AGENT").is_none() {
        eprintln!(
            "skipped: set LAPLUS_TEST_REAL_AGENT=1 to resolve the Claude Code \
             actually installed here"
        );
        return;
    }

    let server = TestServer::start().await;
    server.refresh_providers(Search::from_environment()).await;
    let provider = provider_over_the_socket(&server).await;

    assert_eq!(
        provider["status"],
        json!("ready"),
        "the installed agent did not resolve: {provider}"
    );
    assert!(
        provider["version"].as_str().is_some_and(|version| version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))),
        "{provider}"
    );

    server.stop().await;
}

/// Every state the provider can be in decodes against one schema on the client,
/// so every state has to send the same fields. A field that only appears when
/// things are going well is a decode failure waiting for the day they stop.
#[tokio::test]
async fn every_provider_state_sends_the_same_fields_over_the_socket() {
    let ready = FakeAgent::reporting("2.1.220");
    let broken = FakeAgent::failing();
    let quiet = FakeAgent::saying("echo hello");

    let mut disabled_config = ServerConfig::detect();
    disabled_config.settings.providers.claude_agent.enabled = false;

    // Every state a provider can reach, each with the search that reaches it.
    let states: [(ServerConfig, Search); 5] = [
        (ServerConfig::detect(), Search::over(&[ready.directory()])),
        (ServerConfig::detect(), Search::over(&[broken.directory()])),
        (ServerConfig::detect(), Search::over(&[quiet.directory()])),
        (ServerConfig::detect(), Search::over(&[])),
        (disabled_config, Search::over(&[ready.directory()])),
    ];

    let fields = |provider: &Value| -> Vec<String> {
        let mut fields: Vec<String> = provider
            .as_object()
            .unwrap_or_else(|| panic!("an object: {provider}"))
            .keys()
            // The one field the contract lets come and go, and a provider that
            // resolved cleanly on a current CLI has nothing to say.
            .filter(|field| *field != "message")
            .cloned()
            .collect();
        fields.sort();
        fields
    };

    let mut expected: Option<Vec<String>> = None;
    let mut seen: Vec<Value> = Vec::new();
    for (config, search) in states {
        let server = TestServer::start_with(config).await;
        server.refresh_providers(search).await;
        let provider = provider_over_the_socket(&server).await;
        server.stop().await;

        match &expected {
            None => expected = Some(fields(&provider)),
            Some(expected) => assert_eq!(&fields(&provider), expected, "{provider}"),
        }
        seen.push(provider["status"].clone());
    }

    // And they really were five different states, not one repeated — otherwise
    // the assertion above is a comparison of a snapshot with itself.
    seen.dedup();
    assert_eq!(
        seen,
        vec![
            json!("ready"),
            json!("error"),
            json!("warning"),
            json!("error"),
            json!("disabled"),
        ]
    );
}

/// The `/` menu, end to end: the agent is asked what commands it knows and the
/// answer reaches the client.
///
/// The composer opens its slash menu over `provider.slashCommands`
/// (`ChatComposer.tsx`, `composerMenuItems`), so an empty array there is an
/// empty menu — which is what this server sent in every snapshot until the
/// handshake below was added. `/clear` is the one in the assertion because it is
/// the built-in a developer notices missing first, and because it exists nowhere
/// on disk: it is compiled into the CLI, and asking is the only way to learn it.
///
/// Driven against [`ScriptedAgent`] rather than [`FakeAgent`] because only the
/// scripted one speaks the control protocol, which is the thing being tested.
#[tokio::test]
async fn the_agent_is_asked_what_commands_it_knows_and_the_answer_reaches_the_client() {
    let agent = harness::agent::ScriptedAgent::emitting(&[
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn"}"#,
    ]);
    let server = TestServer::start_with(configured(&agent.configured())).await;
    server.refresh_providers(Search::over(&[])).await;

    let provider = provider_over_the_socket(&server).await;
    let commands = provider["slashCommands"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of commands: {provider}"))
        .clone();

    let named: Vec<&str> = commands
        .iter()
        .map(|command| command["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(named, vec!["clear", "compact", "context"], "{provider}");

    // The two optional fields, which the menu shows as a row's subtitle. Absent
    // rather than empty where the agent sent nothing: the contract trims them to
    // non-empty, and `""` would fail the client's decode of the whole payload.
    assert_eq!(commands[0]["description"], "Clear conversation history");
    assert_eq!(commands[0].get("input"), None, "an empty hint is not a hint");
    assert_eq!(commands[1]["input"]["hint"], "instructions");
    assert_eq!(commands[2].get("description"), None);

    // Asking is not a session. A developer who has taken no turn has started no
    // agent, and a probe counted as one would put a conversation in the app's
    // accounting that nobody had.
    assert_eq!(agent.starts(), 0, "{:?}", agent.arguments());

    server.stop().await;
}

/// The `$` menu, end to end: the developer's own skills reach the client.
///
/// Read off the filesystem rather than asked for, because the handshake lists a
/// skill's name and not the path or scope the picker shows — see
/// `laplus_server::catalogue`. Both scopes are here because the rule that
/// decides collisions is the interesting part: the project's copy wins, which is
/// the CLI's own most-specific-wins resolution.
#[tokio::test]
async fn the_developers_skills_reach_the_client_with_the_project_scope_winning() {
    let home = tempfile::tempdir().expect("a home");
    let project = harness::workspace::Workspace::with(&["src/"]);
    for (root, name, manifest) in [
        (
            home.path().join("skills"),
            "tdd",
            "---\ndescription: Red, green, refactor.\n---\n",
        ),
        (
            project.path().join(".claude").join("skills"),
            "tdd",
            "---\ndescription: This repository's own.\n---\n",
        ),
    ] {
        std::fs::create_dir_all(root.join(name)).expect("a skill directory");
        std::fs::write(root.join(name).join("SKILL.md"), manifest).expect("a manifest");
    }

    let agent = FakeAgent::reporting("2.1.220");
    let mut config = configured(&agent.configured());
    // The one place a test asks for the developer's home to be somewhere real:
    // the harness otherwise points it at a directory that does not exist, so
    // that no other test depends on which skills the machine happens to have.
    config.settings.providers.claude_agent.home_path = home.path().display().to_string();

    let server = TestServer::start_with(config).await;

    // Probed with no projects registered, which is the state a server boots in:
    // only the developer's own skills can be found.
    server.refresh_providers(Search::over(&[])).await;
    let provider = provider_over_the_socket(&server).await;
    assert_eq!(provider["skills"][0]["scope"], "user", "{provider}");
    assert_eq!(
        provider["skills"][0]["description"], "Red, green, refactor.",
        "{provider}"
    );

    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            harness::conversation::create_project("project-1", project.path()),
        )
        .await
        .expect_success();
    client.close().await;

    // And now the project's own, without a second probe and without a restart.
    // Adding a project is what changes the answer, so it is what has to trigger
    // the rescan — see `provider::rescan_skills`. Polled because that runs off
    // the call, which answers as soon as the registry is written.
    // The deadline is a hang guard rather than a budget: what is being waited
    // for is a `readdir`, and any duration this test could assert on would be a
    // measurement of the machine it runs on.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let skills = loop {
        let provider = provider_over_the_socket(&server).await;
        let skills = provider["skills"].clone();
        if skills[0]["scope"] == json!("project") {
            break skills;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the project's skills never reached the snapshot: {provider}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };

    assert_eq!(
        skills.as_array().map(Vec::len),
        Some(1),
        "the project's copy replaces the user's rather than joining it: {skills}"
    );
    assert_eq!(skills[0]["name"], "tdd");
    assert_eq!(skills[0]["description"], "This repository's own.");
    assert_eq!(skills[0]["enabled"], json!(true));

    server.stop().await;
}
