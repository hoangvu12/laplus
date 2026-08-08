use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    Router,
};
use laplus_server::{
    config::OpenCodeSettings,
    provider::{ConfiguredInstance, OpenCodeInstance, ProviderIdentity},
    text_generation::{idle_decision, IdleDecision, Operation, ResultText, Service},
};
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::oneshot};

type Requests = Arc<Mutex<Vec<(String, String, Value)>>>;

#[derive(Clone)]
struct Peer {
    requests: Requests,
    answer: Value,
    prompt_delay: std::time::Duration,
}

async fn handler(State(peer): State<Peer>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    if path == "/session/temporary/message" {
        tokio::time::sleep(peer.prompt_delay).await;
    }
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    peer.requests
        .lock()
        .unwrap()
        .push((method.clone(), path.clone(), body));
    let value = match (method.as_str(), path.as_str()) {
        ("POST", "/session") => json!({"id":"temporary"}),
        ("POST", "/session/temporary/message") => peer.answer.clone(),
        ("DELETE", "/session/temporary") => json!(true),
        _ => json!({"name":"NotFoundError"}),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

#[test]
fn local_generation_reaps_on_the_thirty_second_idle_decision() {
    assert_eq!(
        idle_decision(std::time::Duration::from_secs(29)),
        IdleDecision::Keep
    );
    assert_eq!(
        idle_decision(std::time::Duration::from_secs(30)),
        IdleDecision::Reap
    );
}

#[tokio::test]
async fn every_destination_returns_its_sanitized_contract_shape() {
    let cases = [
        (
            Operation::CommitMessage {
                context: "diff".into(),
            },
            json!({"parts":[{"type":"text","text":"{\"subject\":\"  Add   search  \",\"body\":\"Details\\r\\n\"}"}]}),
            ResultText::CommitMessage {
                subject: "Add search".into(),
                body: Some("Details".into()),
            },
        ),
        (
            Operation::PullRequest {
                context: "diff".into(),
            },
            json!({"parts":[{"type":"text","text":"{\"title\":\"  Add search  \",\"body\":\"## Summary\\r\\nDone\\n\"}"}]}),
            ResultText::PullRequest {
                title: "Add search".into(),
                body: "## Summary\nDone".into(),
            },
        ),
        (
            Operation::ThreadTitle {
                context: "chat".into(),
            },
            json!({"parts":[{"type":"text","text":"{\"title\":\"  Search   design  \"}"}]}),
            ResultText::ThreadTitle("Search design".into()),
        ),
    ];
    for (operation, answer, expected) in cases {
        let (url, _, stop) = peer(answer).await;
        assert_eq!(
            Service::new()
                .generate(&external(url), "/workspace", None, operation)
                .await
                .unwrap(),
            expected
        );
        let _ = stop.send(());
    }
}

async fn peer(answer: Value) -> (String, Requests, oneshot::Sender<()>) {
    peer_delayed(answer, std::time::Duration::ZERO).await
}

async fn peer_delayed(
    answer: Value,
    prompt_delay: std::time::Duration,
) -> (String, Requests, oneshot::Sender<()>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback(handler).with_state(Peer {
        requests: requests.clone(),
        answer,
        prompt_delay,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await;
    });
    (format!("http://{address}"), requests, stop_tx)
}

#[tokio::test]
async fn a_timed_out_generation_still_cleans_up_its_temporary_session() {
    let (url, requests, stop) = peer_delayed(
        json!({"parts":[{"type":"text","text":"{\"title\":\"late\"}"}]}),
        std::time::Duration::from_millis(100),
    )
    .await;
    let error = Service::with_request_timeout(std::time::Duration::from_millis(10))
        .generate(
            &external(url),
            "/workspace",
            None,
            Operation::ThreadTitle {
                context: "chat".into(),
            },
        )
        .await
        .expect_err("deadline");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.0 == "DELETE"));
    let _ = stop.send(());
}

fn external(url: String) -> ConfiguredInstance {
    ConfiguredInstance::OpenCode(OpenCodeInstance {
        identity: ProviderIdentity {
            instance_id: "work".into(),
            driver: "opencode".into(),
        },
        display_name: "Work".into(),
        settings: OpenCodeSettings {
            enabled: true,
            binary_path: "opencode".into(),
            server_url: url,
            server_password: String::new(),
            custom_models: vec![],
        },
    })
}

struct FakeLocal {
    _directory: tempfile::TempDir,
    binary: PathBuf,
    log: PathBuf,
}

impl FakeLocal {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join(if cfg!(windows) {
            "opencode.cmd"
        } else {
            "opencode"
        });
        let log = directory.path().join("requests.jsonl");
        let executable = std::env::current_exe().unwrap();
        let launch = if cfg!(windows) {
            format!("@echo off\r\nset T17_PORT=%~3\r\nset T17_LOG={}\r\n\"{}\" --exact local_generation_peer_child --ignored --nocapture\r\n", log.display(), executable.display())
        } else {
            format!("#!/bin/sh\nT17_PORT=\"$3\" T17_LOG='{}' exec '{}' --exact local_generation_peer_child --ignored --nocapture\n", log.display(), executable.display())
        };
        std::fs::write(&binary, launch).unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            _directory: directory,
            binary,
            log,
        }
    }
    async fn log(&self, count: usize) -> Vec<Value> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
                    let values = contents
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect::<Vec<_>>();
                    if values.len() >= count {
                        return values;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }
}

fn local(fake: &FakeLocal) -> ConfiguredInstance {
    let ConfiguredInstance::OpenCode(mut instance) = external(String::new()) else {
        unreachable!()
    };
    instance.identity.instance_id = "local".into();
    instance.settings.binary_path = fake.binary.display().to_string();
    ConfiguredInstance::OpenCode(instance)
}

#[derive(Clone)]
struct LocalPeer {
    log: Arc<PathBuf>,
    sessions: Arc<AtomicUsize>,
}

async fn local_handler(State(peer): State<LocalPeer>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let id = if path == "/session" {
        format!(
            "temporary-{}",
            peer.sessions.fetch_add(1, Ordering::SeqCst) + 1
        )
    } else {
        String::new()
    };
    let record = json!({"method":method,"path":path,"body":String::from_utf8_lossy(&body)});
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&*peer.log)
            .unwrap(),
        "{record}"
    )
    .unwrap();
    let value = if path == "/global/health" {
        json!({"healthy":true,"version":"1.18.10"})
    } else if path == "/session" {
        json!({"id":id})
    } else if path.ends_with("/message") {
        json!({"parts":[{"type":"text","text":"{\"title\":\"Shared server\"}"}]})
    } else {
        json!(true)
    };
    Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn local_generation_peer_child() {
    let port = std::env::var("T17_PORT")
        .unwrap()
        .trim_start_matches("--port=")
        .parse::<u16>()
        .unwrap();
    let log = Arc::new(PathBuf::from(std::env::var("T17_LOG").unwrap()));
    let app = Router::new().fallback(local_handler).with_state(LocalPeer {
        log,
        sessions: Arc::new(AtomicUsize::new(0)),
    });
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[tokio::test]
async fn local_requests_reuse_one_server_but_never_one_temporary_session() {
    let fake = FakeLocal::new();
    let service = Service::new();
    let workspace = fake._directory.path().display().to_string();
    for context in ["first", "second"] {
        assert_eq!(
            service
                .generate(
                    &local(&fake),
                    &workspace,
                    None,
                    Operation::ThreadTitle {
                        context: context.into()
                    }
                )
                .await
                .unwrap(),
            ResultText::ThreadTitle("Shared server".into())
        );
    }
    let log = fake.log(7).await;
    assert_eq!(
        log.iter()
            .filter(|record| record["path"] == "/global/health")
            .count(),
        1,
        "one owned server launch"
    );
    assert_eq!(
        log.iter()
            .filter(|record| record["path"] == "/session")
            .count(),
        2,
        "one isolated session per request"
    );
    assert!(log
        .iter()
        .any(|record| record["method"] == "DELETE" && record["path"] == "/session/temporary-1"));
    assert!(log
        .iter()
        .any(|record| record["method"] == "DELETE" && record["path"] == "/session/temporary-2"));
    assert_eq!(
        service
            .reap_idle_at(tokio::time::Instant::now() + std::time::Duration::from_secs(30))
            .await,
        1
    );
}

#[tokio::test]
async fn external_generation_is_isolated_deny_all_structured_and_cleaned_up() {
    let (url, requests, stop) = peer(
        json!({"parts":[{"type":"text","text":"{\"branchName\":\"  feature/My Branch  \"}"}]}),
    )
    .await;
    let service = Service::new();
    let result = service
        .generate(
            &external(url),
            "/workspace",
            Some("openai/model-x"),
            Operation::BranchName {
                context: "Add search".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(result, ResultText::BranchName("feature/my-branch".into()));

    let requests = requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.0.as_str(), request.1.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("POST", "/session"),
            ("POST", "/session/temporary/message"),
            ("DELETE", "/session/temporary")
        ]
    );
    assert_eq!(
        requests[0].2["permission"],
        json!([{"permission":"*","pattern":"*","action":"deny"}])
    );
    assert!(requests[1].2["tools"]
        .as_object()
        .is_some_and(|tools| tools.is_empty()));
    assert_eq!(
        requests[1].2["model"],
        json!({"providerID":"openai","modelID":"model-x"})
    );
    assert!(requests[1].2["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("branchName"));
    let _ = stop.send(());
}

#[tokio::test]
async fn malformed_or_tool_output_is_rejected_but_the_temporary_session_is_deleted() {
    for answer in [
        json!({"parts":[{"type":"text","text":"not json"}]}),
        json!({"parts":[{"type":"tool","tool":"bash"},{"type":"text","text":"{\"title\":\"ok\"}"}]}),
    ] {
        let (url, requests, stop) = peer(answer).await;
        let error = Service::new()
            .generate(
                &external(url),
                "/workspace",
                None,
                Operation::ThreadTitle {
                    context: "discussion".into(),
                },
            )
            .await
            .expect_err("invalid output");
        assert!(error.to_string().contains("OpenCode"));
        let requests = requests.lock().unwrap();
        assert_eq!(
            (&requests.last().unwrap().0, &requests.last().unwrap().1),
            (&"DELETE".to_string(), &"/session/temporary".to_string())
        );
        let _ = stop.send(());
    }
}
