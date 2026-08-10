mod harness;

use harness::conversation::{create_project, create_thread, kinds};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

async fn create(client: &mut SocketClient, workspace: &Workspace) {
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread("project-1", "thread-1"),
        )
        .await
        .expect_success();
}

fn command(kind: &str, extra: Value) -> Value {
    let mut value =
        json!({"type": kind, "commandId": format!("test:{kind}"), "threadId": "thread-1"});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

#[tokio::test]
async fn pin_reorder_unpin_cross_the_real_socket_and_survive_restart() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let pinned_at = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        create(&mut client, &workspace).await;
        let watch = client.watch_conversation("thread-1").await;
        client
            .call(
                "orchestration.dispatchCommand",
                command("thread.pin", json!({"orderKey": "a0"})),
            )
            .await
            .expect_success();
        let seen = client.next_chunk(&watch).await;
        assert_eq!(kinds(&seen), vec!["thread.pinned"]);
        assert_eq!(seen[0]["event"]["payload"]["pinOrderKey"], "a0");
        let pinned_at = seen[0]["event"]["payload"]["pinnedAt"].clone();
        client.ack(&watch).await;
        client.call("orchestration.dispatchCommand", command("thread.pin", json!({"orderKey": "must-be-ignored"}))).await.expect_success();
        let repeated = client.next_chunk(&watch).await;
        assert!(repeated[0]["event"]["payload"].get("pinOrderKey").is_none());
        let snapshot = server.connect().await.into_thread_snapshot("thread-1").await["thread"].clone();
        assert_eq!(snapshot["pinOrderKey"], "a0");
        assert_eq!(snapshot["pinnedAt"], pinned_at);
        client.close().await;
        server.stop().await;
        pinned_at
    };

    let server = TestServer::start_at(&database).await;
    let restored = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(restored["pinnedAt"], pinned_at);
    assert_eq!(restored["pinOrderKey"], "a0");

    let mut client = server.connect().await;
    let watch = client.watch_conversation("thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            command("thread.pin.reorder", json!({"orderKey": "z9"})),
        )
        .await
        .expect_success();
    let seen = client.next_chunk(&watch).await;
    assert_eq!(kinds(&seen), vec!["thread.pin-reordered"]);
    assert_eq!(seen[0]["event"]["payload"]["orderKey"], "z9");
    client.ack(&watch).await;
    client
        .call(
            "orchestration.dispatchCommand",
            command("thread.unpin", json!({})),
        )
        .await
        .expect_success();
    let seen = client.next_chunk(&watch).await;
    assert_eq!(kinds(&seen), vec!["thread.unpinned"]);
    let fresh = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(fresh["pinnedAt"], Value::Null);
    assert_eq!(fresh["pinOrderKey"], Value::Null);
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_unpinned_thread_cannot_be_reordered() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    create(&mut client, &workspace).await;
    client
        .call(
            "orchestration.dispatchCommand",
            command("thread.pin.reorder", json!({"orderKey": "z9"})),
        )
        .await
        .expect_declared("OrchestrationDispatchCommandError");
    client.close().await;
    server.stop().await;
}
