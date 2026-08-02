//! Generic MCP transport at the real HTTP seam.

mod harness;

use harness::TestServer;
use reqwest::StatusCode;
use serde_json::{json, Value};

#[tokio::test]
async fn mcp_http_authentication_origin_protocol_and_lifetime() {
    let server = TestServer::start().await;
    let session = server.open_mcp_session("thread-mcp-http");
    let client = reqwest::Client::new();

    assert_eq!(
        client
            .get(session.endpoint())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        client
            .post(session.endpoint())
            .body("{")
            .header("Authorization", session.authorization())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        client
            .post(session.endpoint())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(session.endpoint())
            .header("Authorization", session.authorization())
            .header("Origin", "http://localhost:123@evil.example")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    let initialized = client
        .post(session.endpoint())
        .header("Authorization", session.authorization())
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    assert_eq!(
        initialized.json::<Value>().await.unwrap()["result"]["protocolVersion"],
        "2025-06-18"
    );

    for accepted in [
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{}}),
        json!({"jsonrpc":"2.0","id":1,"result":{}}),
    ] {
        let response = client
            .post(session.endpoint())
            .header("Authorization", session.authorization())
            .json(&accepted)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.content_length(), Some(0));
    }

    let tools = client
        .post(session.endpoint())
        .header("Authorization", session.authorization())
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(tools, json!({"jsonrpc":"2.0","id":2,"result":{"tools":[]}}));
    let missing = client.post(session.endpoint()).header("Authorization", session.authorization()).json(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"missing","arguments":{}}})).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(missing["error"]["code"], -32602);

    let other = server.open_mcp_session("thread-mcp-other");
    assert_eq!(
        client
            .post(session.endpoint())
            .header("Authorization", other.authorization())
            .json(&json!({"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(other);

    let endpoint = session.endpoint().to_string();
    let authorization = session.authorization().to_string();
    drop(session);
    assert_eq!(
        client
            .post(endpoint)
            .header("Authorization", authorization)
            .json(&json!({"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    server.stop().await;
}
