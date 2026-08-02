//! Provider maintenance at the client boundary, with fake commands and an
//! external OpenCode peer. No installed tool or network service is used.

mod harness;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use axum::{routing::get, Json, Router};
use harness::TestServer;
use laplus_server::config::ServerConfig;
use laplus_server::provider_maintenance::{
    Action, CommandOutcome, CommandRunner, ProviderMaintenance,
};
use serde_json::json;

#[derive(Debug)]
struct FakeCommand {
    outcome: Mutex<Option<Result<CommandOutcome, String>>>,
    seen: Mutex<Vec<Action>>,
}

#[derive(Debug)]
struct BarrierCommand {
    gate: (Mutex<bool>, Condvar),
    entered: std::sync::mpsc::Sender<()>,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl CommandRunner for BarrierCommand {
    fn run(&self, _action: &Action) -> Result<CommandOutcome, String> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let _ = self.entered.send(());
        let (lock, wake) = &self.gate;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = wake.wait(released).unwrap();
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(CommandOutcome {
            exit_code: Some(0),
            output: String::new(),
        })
    }
}

impl CommandRunner for FakeCommand {
    fn run(&self, action: &Action) -> Result<CommandOutcome, String> {
        self.seen.lock().unwrap().push(action.clone());
        self.outcome
            .lock()
            .unwrap()
            .take()
            .expect("one command outcome")
    }
}

async fn external(version: &'static str) -> String {
    let app = Router::new()
        .route(
            "/global/health",
            get(move || async move { Json(json!({"healthy":true,"version":version})) }),
        )
        .route(
            "/provider",
            get(|| async { Json(json!({"providers":[],"connected":[]})) }),
        )
        .route("/agent", get(|| async { Json(json!([])) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

fn native_installation() -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(".opencode/bin/opencode");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (root, path.to_string_lossy().into_owned())
}

fn configured(endpoint: String, binary_path: &str) -> ServerConfig {
    let mut config = ServerConfig::detect();
    config.settings.provider_instances.insert(
        "openExternal".into(),
        json!(
            {"driver":"opencode", "displayName":"External OpenCode", "enabled":true, "config":{
            "binaryPath":binary_path, "serverUrl":endpoint,
                "serverPassword":"", "customModels":[]
            }}
        ),
    );
    config
}

#[tokio::test]
async fn explicit_update_reports_an_unchanged_external_snapshot() {
    let (_installation, binary_path) = native_installation();
    let runner = Arc::new(FakeCommand {
        outcome: Mutex::new(Some(Ok(CommandOutcome {
            exit_code: Some(0),
            output: "done".into(),
        }))),
        seen: Mutex::new(Vec::new()),
    });
    let server = TestServer::start_with_maintenance(
        configured(external("1.20.0").await, &binary_path),
        ProviderMaintenance::with_runner(runner.clone()),
    )
    .await;
    let mut client = server.connect().await;
    client
        .call(
            "server.refreshProviders",
            json!({"instanceId":"openExternal"}),
        )
        .await
        .expect_success();
    assert!(runner.seen.lock().unwrap().is_empty(), "refresh must not run maintenance");
    let result = client
        .call(
            "server.updateProvider",
            json!({"provider":"opencode","instanceId":"openExternal"}),
        )
        .await
        .expect_success();
    let provider = &result["providers"][0];
    assert_eq!(provider["version"], "1.20.0");
    assert_eq!(provider["updateState"]["status"], "unchanged");
    assert_eq!(provider["updateState"]["beforeVersion"], "1.20.0");
    assert_eq!(provider["updateState"]["afterVersion"], "1.20.0");
    assert_eq!(runner.seen.lock().unwrap()[0].display(), "opencode upgrade");
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn failed_command_is_reported_after_refresh_and_routing_is_instance_strict() {
    let (_installation, binary_path) = native_installation();
    let runner = Arc::new(FakeCommand {
        outcome: Mutex::new(Some(Ok(CommandOutcome {
            exit_code: Some(7),
            output: "permission denied".into(),
        }))),
        seen: Mutex::new(Vec::new()),
    });
    let server = TestServer::start_with_maintenance(
        configured(external("1.21.0").await, &binary_path),
        ProviderMaintenance::with_runner(runner),
    )
    .await;
    let mut client = server.connect().await;
    client
        .call(
            "server.refreshProviders",
            json!({"instanceId":"openExternal"}),
        )
        .await
        .expect_success();
    let missing = client
        .call("server.updateProvider", json!({"provider":"opencode"}))
        .await
        .expect_declared("ServerProviderUpdateError");
    assert!(missing["reason"].as_str().unwrap().contains("instanceId"));
    let mismatch = client
        .call(
            "server.updateProvider",
            json!({"provider":"codex","instanceId":"openExternal"}),
        )
        .await
        .expect_declared("ServerProviderUpdateError");
    assert!(mismatch["reason"]
        .as_str()
        .unwrap()
        .contains("belongs to driver"));
    let result = client
        .call(
            "server.updateProvider",
            json!({"provider":"opencode","instanceId":"openExternal"}),
        )
        .await
        .expect_success();
    assert_eq!(result["providers"][0]["updateState"]["status"], "failed");
    assert_eq!(
        result["providers"][0]["updateState"]["afterVersion"],
        "1.21.0"
    );
    client.close().await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_requests_are_serialized_by_instance_and_package_manager() {
    let (_installation, binary_path) = native_installation();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let runner = Arc::new(BarrierCommand {
        gate: (Mutex::new(false), Condvar::new()),
        entered: entered_tx,
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    });
    let endpoint = external("1.22.0").await;
    let mut config = configured(endpoint.clone(), &binary_path);
    let mut second = config.settings.provider_instances["openExternal"].clone();
    second["displayName"] = json!("External OpenCode Two");
    second["config"]["serverUrl"] = json!(endpoint);
    config
        .settings
        .provider_instances
        .insert("openExternalTwo".into(), second);
    let server = TestServer::start_with_maintenance(
        config,
        ProviderMaintenance::with_runner(runner.clone()),
    )
    .await;
    let mut setup = server.connect().await;
    setup
        .call(
            "server.refreshProviders",
            json!({"instanceId":"openExternal"}),
        )
        .await
        .expect_success();
    setup
        .call(
            "server.refreshProviders",
            json!({"instanceId":"openExternalTwo"}),
        )
        .await
        .expect_success();
    setup.close().await;

    let mut first = server.connect().await;
    let mut second = server.connect().await;
    let mut same_instance = server.connect().await;
    let first_call = tokio::spawn(async move {
        let result = first
            .call(
                "server.updateProvider",
                json!({"provider":"opencode","instanceId":"openExternal"}),
            )
            .await;
        first.close().await;
        result
    });
    tokio::task::spawn_blocking(move || {
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("first command reaches barrier")
    })
    .await
    .unwrap();
    let second_call = tokio::spawn(async move {
        let result = second
            .call(
                "server.updateProvider",
                json!({"provider":"opencode","instanceId":"openExternalTwo"}),
            )
            .await;
        second.close().await;
        result
    });
    let same_instance_call = tokio::spawn(async move {
        let result = same_instance
            .call(
                "server.updateProvider",
                json!({"provider":"opencode","instanceId":"openExternal"}),
            )
            .await;
        same_instance.close().await;
        result
    });
    // Both calls have been launched; releasing the fake lets the lock ordering
    // itself decide whether their command bodies overlap.
    *runner.gate.0.lock().unwrap() = true;
    runner.gate.1.notify_all();
    tokio::time::timeout(std::time::Duration::from_secs(3), first_call)
        .await
        .expect("first update exits after barrier release")
        .unwrap()
        .expect_success();
    tokio::time::timeout(std::time::Duration::from_secs(3), second_call)
        .await
        .expect("second update exits after barrier release")
        .unwrap()
        .expect_success();
    tokio::time::timeout(std::time::Duration::from_secs(3), same_instance_call)
        .await
        .expect("same-instance update exits after barrier release")
        .unwrap()
        .expect_success();
    assert_eq!(runner.max_active.load(Ordering::SeqCst), 1);
    server.stop().await;
}
