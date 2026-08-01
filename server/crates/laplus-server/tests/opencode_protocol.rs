use std::{
    convert::Infallible,
    sync::{Arc, Mutex},
};

use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{Request, Response, StatusCode},
    Router,
};
use futures_util::StreamExt;
use laplus_server::opencode_protocol::{
    OpenCodeClient, OpenCodeError, OpenCodeEvent, SseDecodeError, SseDecoder,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::oneshot};

#[test]
fn client_rejects_a_base_url_that_cannot_be_a_http_endpoint() {
    let error = OpenCodeClient::new("not a url", "/workspace", None).expect_err("invalid endpoint");
    assert!(matches!(error, OpenCodeError::InvalidBaseUrl(_)));
}

#[derive(Clone)]
struct Script {
    requests: RecordedRequests,
    response: ScriptedResponse,
}

type RecordedRequest = (String, String, Option<String>, Vec<u8>);
type RecordedRequests = Arc<Mutex<Vec<RecordedRequest>>>;
type ScriptedResponse = Arc<dyn Fn(&str) -> Response<Body> + Send + Sync>;

async fn scripted_handler(State(script): State<Script>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or_default().to_owned();
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    script
        .requests
        .lock()
        .unwrap()
        .push((path.clone(), query, auth, body));
    (script.response)(&path)
}

async fn peer(
    response: impl Fn(&str) -> Response<Body> + Send + Sync + 'static,
) -> (String, RecordedRequests, oneshot::Sender<()>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new().fallback(scripted_handler).with_state(Script {
        requests: requests.clone(),
        response: Arc::new(response),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stop_rx.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{address}/api/"), requests, stop_tx)
}

fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

#[tokio::test]
async fn client_centralizes_prefix_directory_auth_json_and_operation_routes() {
    let (url, requests, stop) = peer(|path| match path {
        "/api/global/health" => {
            json_response(StatusCode::OK, json!({"healthy":true,"version":"1.18.10"}))
        }
        "/api/session" => json_response(StatusCode::OK, json!({"id":"ses_1"})),
        _ => json_response(StatusCode::OK, json!({"ok":true})),
    })
    .await;
    let client = OpenCodeClient::new(&url, "/work tree", Some("secret".into())).unwrap();

    assert_eq!(client.health().await.unwrap().version, "1.18.10");
    client.providers().await.unwrap();
    client.agents().await.unwrap();
    client
        .create_session(&json!({"title":"hello"}))
        .await
        .unwrap();
    client
        .prompt("ses_1", &json!({"parts":[{"type":"text","text":"hi"}]}))
        .await
        .unwrap();
    client.abort("ses_1").await.unwrap();
    client
        .revert("ses_1", &json!({"messageID":"m1"}))
        .await
        .unwrap();
    client
        .reply_permission("per_1", &json!({"reply":"once"}))
        .await
        .unwrap();
    client
        .reply_question("que_1", &json!({"answers":[["yes"]]}))
        .await
        .unwrap();
    client.reject_question("que_2").await.unwrap();

    let requests = requests.lock().unwrap();
    assert!(requests
        .iter()
        .all(|(_, query, auth, _)| query == "directory=%2Fwork+tree"
            && auth.as_deref() == Some("Basic b3BlbmNvZGU6c2VjcmV0")));
    assert_eq!(
        requests.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        vec![
            "/api/global/health",
            "/api/provider",
            "/api/agent",
            "/api/session",
            "/api/session/ses_1/prompt_async",
            "/api/session/ses_1/abort",
            "/api/session/ses_1/revert",
            "/api/permission/per_1/reply",
            "/api/question/que_1/reply",
            "/api/question/que_2/reject",
        ]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[3].3).unwrap(),
        json!({"title":"hello"})
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn missing_session_auth_transport_server_and_bad_json_are_distinct() {
    let (url, _, stop) = peer(|path| match path {
        "/api/session/missing" => json_response(
            StatusCode::NOT_FOUND,
            json!({"name":"NotFoundError","data":{"message":"Session missing not found"}}),
        ),
        "/api/session/auth" => json_response(
            StatusCode::UNAUTHORIZED,
            json!({"name":"UnauthorizedError","message":"bad password"}),
        ),
        "/api/session/broken" => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"name":"InternalError","message":"boom"}),
        ),
        "/api/global/health" => Response::builder()
            .status(200)
            .body(Body::from("not-json"))
            .unwrap(),
        _ => unreachable!(),
    })
    .await;
    let client = OpenCodeClient::new(&url, "/workspace", None).unwrap();
    assert!(matches!(
        client.session("missing").await,
        Err(OpenCodeError::MissingSession { .. })
    ));
    assert!(matches!(
        client.session("auth").await,
        Err(OpenCodeError::Authentication { .. })
    ));
    assert!(matches!(
        client.session("broken").await,
        Err(OpenCodeError::Server { .. })
    ));
    assert!(matches!(
        client.health().await,
        Err(OpenCodeError::MalformedJson { .. })
    ));
    let _ = stop.send(());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable = listener.local_addr().unwrap();
    drop(listener);
    let client = OpenCodeClient::new(&format!("http://{unavailable}"), "/workspace", None).unwrap();
    assert!(matches!(
        client.health().await,
        Err(OpenCodeError::Transport(_))
    ));
}

#[test]
fn sse_decoder_handles_chunks_multiline_heartbeats_unknowns_and_malformed_input() {
    let fixture = include_bytes!("../../../fixtures/opencode-http-sse/events.sse");
    let mut decoder = SseDecoder::default();
    let mut events = Vec::new();
    for chunk in fixture.chunks(7) {
        events.extend(decoder.push(chunk));
    }
    assert_eq!(decoder.finish(), None);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].as_ref().unwrap().envelope().kind,
        "session.status"
    );
    assert!(matches!(events[1], Ok(OpenCodeEvent::Unknown(_))));

    let mut malformed_json = SseDecoder::default();
    assert!(matches!(
        malformed_json.push(b"data: {nope}\n\n").as_slice(),
        [Err(SseDecodeError::MalformedJson(_))]
    ));
    let mut malformed_sse = SseDecoder::default();
    assert!(matches!(
        malformed_sse.push(b"wat\n").as_slice(),
        [Err(SseDecodeError::MalformedField(_))]
    ));
}

#[tokio::test]
async fn event_stream_retains_and_counts_unknown_events_and_cancels_promptly() {
    let (url, _, stop) = peer(move |_| {
        let stream = futures_util::stream::iter(vec![Ok::<_, Infallible>(Bytes::from_static(
            b"data: {\"type\":\"future.event\",\"properties\":{\"kept\":true}}\n\n",
        ))])
        .chain(futures_util::stream::pending());
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap()
    })
    .await;
    let client = OpenCodeClient::new(&url, "/workspace", None).unwrap();
    let mut events = client.subscribe().await.unwrap();
    assert!(events.next().await.unwrap().is_unknown());
    assert_eq!(events.unknown_count(), 1);
    tokio::time::timeout(std::time::Duration::from_millis(250), events.cancel())
        .await
        .expect("cancellation should release the response pump");
    let _ = stop.send(());
}

#[derive(Deserialize)]
struct GoldenCase {
    operation: String,
    response: Value,
}

#[test]
fn redacted_wire_fixture_covers_every_protocol_operation_and_error_family() {
    let cases: Vec<GoldenCase> = serde_json::from_str(include_str!(
        "../../../fixtures/opencode-http-sse/operations.json"
    ))
    .unwrap();
    let names = cases
        .iter()
        .map(|case| case.operation.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "health",
            "providers",
            "agents",
            "session.create",
            "session.get",
            "session.prompt_async",
            "session.abort",
            "session.revert",
            "permission.reply",
            "question.reply",
            "question.reject",
            "error.missing-session",
            "error.authentication",
            "error.server"
        ]
    );
    assert!(cases.iter().all(|case| !case.response.is_null()));
}
