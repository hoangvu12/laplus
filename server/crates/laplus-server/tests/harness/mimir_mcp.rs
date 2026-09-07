//! Mimir's bridge boundary against the real Laplus authenticated MCP HTTP host.
use super::*;
use laplus_server::mcp::{Host, Platform, Toolkit};
use std::{future::Future, pin::Pin};

fn connection(request: &Value) -> (String, String) {
    assert_eq!(request["id"], SESSION);
    assert_eq!(request["servers"].as_array().unwrap().len(), 1);
    assert_eq!(request["servers"][0]["name"], "laplus");
    let http = &request["servers"][0]["transport"]["http"];
    assert_eq!(http["headers"].as_array().unwrap().len(), 1);
    assert_eq!(http["headers"][0][0], "Authorization");
    let endpoint = http["url"].as_str().unwrap().to_string();
    let authorization = http["headers"][0][1].as_str().unwrap().to_string();
    assert!(endpoint.starts_with("http://127.0.0.1:"));
    assert!(authorization.starts_with("Bearer "));
    (endpoint, authorization)
}

async fn rpc(endpoint: &str, authorization: &str, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(endpoint)
        .header("Authorization", authorization)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

pub(super) async fn attach(shared: &Shared, request: Value) -> Json<Value> {
    let mode = {
        let mut state = shared.state.lock().unwrap();
        state.requests.push(request.clone());
        state.mcp_mode
    };
    let (endpoint, authorization) = connection(&request);
    let initialized = rpc(&endpoint, &authorization, "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"bridge-peer","version":"1"}})).await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "laplus");
    let tools = rpc(&endpoint, &authorization, "tools/list", json!({})).await;
    assert!(tools["result"]["tools"].is_array());
    if mode == McpMode::RefuseAttach {
        // A peer error is untrusted, including its code; neither credential form
        // may survive in activities, snapshots, or logs.
        return Json(
            json!({"version":1,"error":{"code":authorization,"message":authorization.strip_prefix("Bearer ").unwrap()}}),
        );
    }
    if mode == McpMode::MalformedAttach {
        return Json(json!({"version":1,"result":{}}));
    }
    Json(json!({"version":1,"result":{"attachment_id":"host-attachment-1"}}))
}

#[derive(Debug)]
struct ConversationToolkit;
impl Toolkit for ConversationToolkit {
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"name":"conversation_identity","description":"Return the trusted host conversation identity","inputSchema":{"type":"object"}}),
        ]
    }
    fn call<'a>(
        &'a self,
        thread_id: &'a str,
        name: &'a str,
        _: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(name, "conversation_identity");
            Ok(json!([{"type":"text","text":thread_id}]))
        })
    }
}

async fn start_thread(
    server: &TestServer,
    peer: &Peer,
    thread_id: &str,
    instance_id: &str,
) -> (SocketClient, String) {
    let mut client = server.connect().await;
    let project = format!("project-{thread_id}");
    dispatch(
        &mut client,
        create_project(&project, &peer.shared.workspace),
    )
    .await;
    let mut thread = create_thread(&project, thread_id);
    thread["modelSelection"] = json!({"instanceId":instance_id,"model":"test/org/model"});
    dispatch(&mut client, thread).await;
    let subscription = client.watch_conversation(thread_id).await;
    dispatch(
        &mut client,
        follow_up(thread_id, &format!("message-{thread_id}"), "Hello"),
    )
    .await;
    (client, subscription)
}

fn attached_connection(peer: &Peer) -> (String, String) {
    let state = peer.shared.state.lock().unwrap();
    let attachments: Vec<_> = state
        .requests
        .iter()
        .filter(|r| r["action"] == "attach_mcp")
        .collect();
    assert_eq!(attachments.len(), 1);
    connection(attachments[0])
}

fn assert_released(peer: &Peer, detached: bool) {
    let state = peer.shared.state.lock().unwrap();
    let actions: Vec<_> = state
        .requests
        .iter()
        .map(|r| r["action"].as_str().unwrap())
        .collect();
    let release = actions
        .iter()
        .position(|a| *a == "release")
        .expect("SDK session released");
    if detached {
        assert_eq!(actions.iter().filter(|a| **a == "detach_mcp").count(), 1);
        assert!(actions.iter().position(|a| *a == "detach_mcp").unwrap() < release);
    } else {
        assert!(!actions.contains(&"detach_mcp"));
    }
}

#[tokio::test]
async fn mimir_mcp_host_calls_are_scoped_to_thread_and_provider_instance_and_revoked_on_shutdown() {
    let first = Peer::with_mcp(McpMode::Ready).await;
    let second = Peer::with_mcp(McpMode::Ready).await;
    let mut config = first.config();
    config.settings.provider_instances.insert(
        "mimirOther".into(),
        second
            .config()
            .settings
            .provider_instances
            .remove("mimirLocal")
            .unwrap(),
    );
    let host = Host::with_toolkits(vec![Arc::new(ConversationToolkit)]);
    let server = TestServer::start_with_mcp(config, Arc::new(host.clone())).await;
    let (mut a, a_sub) = start_thread(&server, &first, "thread-a", "mimirLocal").await;
    a.events_until_user_input(&a_sub).await;
    let (mut b, b_sub) = start_thread(&server, &second, "thread-b", "mimirOther").await;
    b.events_until_user_input(&b_sub).await;
    assert_eq!(host.live_sessions(), 2);
    let first_grant = attached_connection(&first);
    let second_grant = attached_connection(&second);
    assert_ne!(first_grant, second_grant);
    for (thread, (endpoint, authorization)) in
        [("thread-a", &first_grant), ("thread-b", &second_grant)]
    {
        let result = rpc(endpoint, authorization, "tools/call", json!({"name":"conversation_identity","arguments":{"thread_id":"cannot-override-trusted-identity"}})).await;
        assert_eq!(result["result"]["content"][0]["text"], thread);
        let snapshot = server
            .connect()
            .await
            .into_thread_snapshot(thread)
            .await
            .to_string();
        assert!(!snapshot.contains(authorization.strip_prefix("Bearer ").unwrap()));
        assert!(!snapshot.contains(endpoint));
        assert!(snapshot.contains("provider.mcp-attached"));
    }
    assert!(!host.authorizes(&first_grant.0, &second_grant.1));
    assert!(!host.authorizes(&second_grant.0, &first_grant.1));
    a.close().await;
    b.close().await;
    server.stop().await;
    assert_eq!(host.live_sessions(), 0);
    assert!(!host.authorizes(&first_grant.0, &first_grant.1));
    assert!(!host.authorizes(&second_grant.0, &second_grant.1));
    assert_released(&first, true);
    assert_released(&second, true);
}

#[tokio::test]
async fn mimir_mcp_old_bridge_still_chats_with_explicit_unavailable_diagnostic() {
    let peer = Peer::start().await;
    let host = Host::new();
    let server = TestServer::start_with_mcp(peer.config(), Arc::new(host.clone())).await;
    let (server, client, _) = open_on(&peer, server).await;
    assert_eq!(host.live_sessions(), 0);
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await
        .to_string();
    assert!(snapshot.contains("provider.mcp-unavailable"));
    assert!(snapshot.contains("Root answer"));
    client.close().await;
    server.stop().await;
    assert!(!peer
        .shared
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|r| r["action"] == "attach_mcp"));
}

async fn failed_start(mode: McpMode) {
    let peer = Peer::with_mcp(mode).await;
    let host = Host::new();
    let server = TestServer::start_with_mcp(peer.config(), Arc::new(host.clone())).await;
    let (mut client, subscription) =
        start_thread(&server, &peer, "thread-failed", "mimirLocal").await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "session.failed"
        })
        .await;
    let (endpoint, authorization) = attached_connection(&peer);
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-failed")
        .await
        .to_string();
    assert!(!snapshot.contains(authorization.strip_prefix("Bearer ").unwrap()));
    assert!(!snapshot.contains(&endpoint));
    if mode == McpMode::RefuseAttach {
        assert!(snapshot.contains("[redacted]"));
    }
    client.close().await;
    server.stop().await;
    assert_eq!(host.live_sessions(), 0);
    assert!(!host.authorizes(&endpoint, &authorization));
    assert_released(&peer, mode == McpMode::RefusePrompt);
}

#[tokio::test]
async fn mimir_mcp_attach_failure_releases_session_and_redacts_host_credentials() {
    failed_start(McpMode::RefuseAttach).await;
}
#[tokio::test]
async fn mimir_mcp_malformed_attachment_revokes_grant_and_releases_session() {
    failed_start(McpMode::MalformedAttach).await;
}
#[tokio::test]
async fn mimir_mcp_prompt_failure_after_attachment_detaches_and_revokes() {
    failed_start(McpMode::RefusePrompt).await;
}
#[tokio::test]
async fn mimir_mcp_failed_detach_still_revokes_local_grant_and_releases_session() {
    let peer = Peer::with_mcp(McpMode::RefuseDetach).await;
    let host = Host::new();
    let server = TestServer::start_with_mcp(peer.config(), Arc::new(host.clone())).await;
    let (server, client, _) = open_on(&peer, server).await;
    let (endpoint, authorization) = attached_connection(&peer);
    assert_eq!(host.live_sessions(), 1);
    client.close().await;
    server.stop().await;
    assert_eq!(host.live_sessions(), 0);
    assert!(!host.authorizes(&endpoint, &authorization));
    assert_released(&peer, true);
}
