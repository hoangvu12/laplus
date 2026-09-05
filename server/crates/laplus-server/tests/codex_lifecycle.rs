//! Regression for Codex starting a successor without another turn/start RPC.
use laplus_server::codex_protocol::{ConversationFold, ConversationState};
use serde_json::json;

#[test]
fn automatic_root_successor_accepts_its_own_completion() {
    let mut state = ConversationState::new();
    state.fold_message(json!({"result": {"thread": {"id": "root"}}}));
    state.fold_message(json!({"result": {"turn": {"id": "first", "status": "inProgress"}}}));
    state.fold_message(json!({"method": "turn/completed", "params": {
        "threadId": "root", "turn": {"id": "first", "status": "completed"}
    }}));
    state.fold_message(json!({"method": "turn/started", "params": {
        "threadId": "root", "turn": {"id": "automatic", "status": "inProgress"}
    }}));
    assert_eq!(
        state.fold_message(json!({"method": "turn/completed", "params": {
            "threadId": "root", "turn": {"id": "first", "status": "completed"}
        }})),
        ConversationFold::Nothing,
        "a late terminal for the previous turn must not settle its successor"
    );
    let ended = state.fold_message(json!({"method": "turn/completed", "params": {
        "threadId": "root", "turn": {"id": "automatic", "status": "completed"}
    }}));
    assert!(
        matches!(ended, ConversationFold::TurnCompleted(_)),
        "the automatic turn's terminal event was discarded"
    );
}

#[test]
fn root_identity_is_not_a_child_even_when_a_descendant_reports_it() {
    for source in ["root", "child"] {
        for method in ["item/started", "item/completed"] {
            let mut state = ConversationState::new();
            state.fold_message(json!({"result":{"thread":{"id":"root"}}}));
            let folded = state.fold_message(json!({"method":method,"params":{
                "threadId":source,"item":{"type":"subAgentActivity","id":"activity",
                    "kind":"interacted","agentThreadId":"root","agentPath":"/root"}
            }}));
            assert_eq!(folded, ConversationFold::Nothing);
            let child = state.fold_message(json!({"method":method,"params":{
                "threadId":source,"item":{"type":"subAgentActivity","id":"child-activity",
                    "kind":"interacted","agentThreadId":"real-child","agentPath":"/root/child"}
            }}));
            assert!(matches!(
                child,
                ConversationFold::SubagentActivity(_) | ConversationFold::NestedSubagentActivity(_)
            ));
        }
    }
}

#[test]
fn collaboration_receivers_exclude_only_the_root_identity() {
    let mut state = ConversationState::new();
    state.fold_message(json!({"result":{"thread":{"id":"root"}}}));
    let folded = state.fold_message(json!({"method":"item/completed","params":{
        "threadId":"root","item":{"id":"call","type":"collabAgentToolCall","tool":"sendInput",
            "status":"completed","senderThreadId":"child","receiverThreadIds":["root","child"],
            "agentsStates":{"root":{"status":"running"},"child":{"status":"running"}}}
    }}));
    let ConversationFold::CollaborationCompleted(call) = folded else {
        panic!("collaboration call");
    };
    assert_eq!(call.receiver_thread_ids, ["child"]);
    assert_eq!(call.agents.len(), 1);
    assert_eq!(call.agents[0].thread_id, "child");
}

#[test]
fn a_duplicate_start_does_not_reopen_its_completed_turn() {
    let mut state = ConversationState::new();
    state.fold_message(json!({"result":{"thread":{"id":"root"}}}));
    let started = json!({"method":"turn/started","params":{"threadId":"root","turn":{"id":"one","status":"inProgress"}}});
    state.fold_message(started.clone());
    state.fold_message(json!({"method":"turn/completed","params":{"threadId":"root","turn":{"id":"one","status":"completed"}}}));
    assert_eq!(state.fold_message(started), ConversationFold::Nothing);
}
