//! Reading one subagent's work stream back off the wire.
//!
//! Both provider suites opened `orchestration.subscribeSubagent` and folded what
//! it sent the way a client does, and both wrote the fold out for themselves
//! while the other was in flight. It is one operation and it belongs in one
//! place: the rule under test — **upsert by entry id, order by sequence** — is
//! the shared model's, not any provider's, and a second copy is a second chance
//! for one suite to prove something the other does not.

#![allow(dead_code)]

use serde_json::{json, Value};

use super::TestServer;

/// One child's work stream as the tab that opens it reads it: the opening
/// snapshot, and nothing else.
pub async fn child_stream(server: &TestServer, thread_id: &str, child_id: &str) -> Value {
    let mut inspector = server.connect().await;
    let subscription = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread_id, "childId": child_id}),
        )
        .await;
    let replayed = inspector.next_chunk(&subscription).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .unwrap_or_else(|| panic!("no stream for {child_id}: {replayed:#?}"))["snapshot"]
        .clone();
    inspector.close().await;
    snapshot
}

/// Replay and live continuation folded the way a client folds them: upsert by
/// entry id, order by sequence.
pub fn folded_entries(snapshot: &Value, live: &[Value]) -> Vec<Value> {
    let mut folded: Vec<Value> = snapshot["entries"]
        .as_array()
        .expect("the snapshot carries the entries so far")
        .clone();
    for item in live {
        let Some(entry) = item.get("entry").filter(|entry| entry.is_object()) else {
            continue;
        };
        match folded.iter().position(|held| held["id"] == entry["id"]) {
            Some(index) => folded[index] = entry.clone(),
            None => folded.push(entry.clone()),
        }
    }
    folded.sort_by_key(|entry| entry["sequence"].as_i64().unwrap_or_default());
    folded
}

/// The compact row one child owns in its parent's transcript.
///
/// The **last** row drawn for that child, because the row is redrawn as the
/// child moves and what the developer is left looking at is whatever was drawn
/// last — which is the whole of what "the row said it completed" and "the row
/// said it was stopped" are claims about.
///
/// Found by `data.childId`, the stream reference every driver's compact child
/// row carries ([`laplus_server::worklog::subagent`],
/// `codex::collaboration_agent_row`), rather than by a provider's own id: the
/// three suites are asking one question about one shared surface, and a
/// provider-shaped lookup would let one of them pass on a row the others would
/// not accept.
pub async fn child_row(server: &TestServer, thread_id: &str, child_id: &str) -> Value {
    let snapshot = server.connect().await.into_thread_snapshot(thread_id).await;
    snapshot["thread"]["activities"]
        .as_array()
        .expect("a thread snapshot carries its activities")
        .iter()
        .rfind(|activity| activity["payload"]["data"]["childId"] == child_id)
        .unwrap_or_else(|| panic!("no compact row for {child_id}: {snapshot:#?}"))
        .clone()
}
