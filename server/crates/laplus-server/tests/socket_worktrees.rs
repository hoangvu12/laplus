//! A conversation that lives in a worktree, driven the way the UI drives one.
//!
//! Ticket 01 of the vcs effort, at the seam its spec calls primary: a real
//! socket, a real repository in a temporary folder, a real second checkout of it
//! made with `git worktree add`, and a real turn that edits files.
//!
//! ## Why this file exists at all
//!
//! There is one rule for where a conversation's work happens — the worktree when
//! it has one, the project's folder otherwise — and until this ticket it was
//! written in the review path and *described in a comment* on the turn path. The
//! two disagreed: a developer who picked a ref that was current in a worktree got
//! an agent editing the project's own folder, while the diff and the revert ran
//! in the worktree.
//!
//! **The revert is what that cost.** A checkpoint is a ref and a patch is a diff
//! between two of them, and refs are shared with a linked worktree — so the diff
//! panel resolved checkpoints captured from the project's folder and showed the
//! agent's own changes after all. It was right by accident. A revert writes a
//! tree, and it wrote the tree recorded from the project's folder into the
//! worktree, over a checkout the agent had never touched. Nothing in the UI said
//! so.
//!
//! No method here made that reachable and none was needed to reach it — a
//! worktree made by hand at a terminal is enough, which is exactly what
//! [`Workspace::worktree`] does.
//!
//! ## What every test here asserts, and why it is two folders
//!
//! **Both of them, every time.** A test that only looked at the worktree would
//! pass against a server that wrote to both; a test that only looked at the
//! project would pass against one that wrote to neither. The pair is what says
//! the work happened in one place, and it is `socket_branches.rs`'s doctrine —
//! what happened on disk, and whether the developer can tell.

mod harness;

use harness::agent::{writes, ScriptedAgent, WORKING_DIRECTORY_MARKER};
use harness::conversation::{revert_checkpoint, start_turn};
use harness::workspace::Workspace;
use harness::TestServer;

/// The lines a scripted turn is made of, either side of what it does to the
/// tree. The same three `socket_diffs.rs` and `socket_revert.rs` use, and for the
/// same reason: no recording contains a turn that edited *these* files.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Write"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// A repository with one committed file, and the marker ignored.
///
/// The `.gitignore` is committed rather than written, which is what carries it
/// into the worktree — the marker the scripted agent drops into whatever
/// directory it was started in is how these tests observe where the agent ran,
/// and a checkpoint that carried it would put a file no test is about into every
/// diff below.
fn a_repository() -> Workspace {
    let workspace = Workspace::with(&[]);
    workspace.put("kept.txt", "one\ntwo\n");
    workspace.put(".gitignore", &format!("{WORKING_DIRECTORY_MARKER}\n"));
    workspace.init_repository().commit("the beginning");
    workspace
}

/// The ticket's own test: one turn, and both halves of it asserted together.
///
/// **The diff assertion is not redundant, and it is not sufficient on its own.**
/// A patch is ref-to-ref, so it would have come out identical before this ticket
/// — the checkpoint recorded the project's folder and the patch was run in the
/// worktree, and shared refs made those the same commits. What makes it evidence
/// here is the assertion above it: the project's own folder never changed, so a
/// patch that shows `+three` can only have been taken of the worktree. The pair
/// is the claim; either alone is satisfied by the broken server.
///
/// Two files, because they are read out of git by different routes: one the
/// repository already tracks, and one it has never seen. The untracked one is
/// what an agent mostly produces, and it is the case a diff of a working tree has
/// nothing to say about on its own.
#[tokio::test]
async fn a_conversation_in_a_worktree_runs_its_agent_there_and_records_that_tree() {
    let modified = writes("kept.txt", "one\ntwo\nthree\n");
    let created = writes("src/new.txt", "brand new\n");
    let agent = ScriptedAgent::emitting(&[INIT, modified.as_str(), created.as_str(), SAID, DONE]);
    let project = a_repository();
    let worktree = project.worktree("feature");
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client
        .open_conversation_at(&project, "project-1", "thread-1", Some(worktree.path()))
        .await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "make a change"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // Half one: the agent worked in the folder the conversation was pointed at.
    // The marker says where the child was started; the files say what it did once
    // it was there, which is the thing the developer actually cares about.
    assert!(
        worktree.path().join(WORKING_DIRECTORY_MARKER).exists(),
        "the agent ran somewhere other than {}",
        worktree.path().display()
    );
    assert!(
        !project.path().join(WORKING_DIRECTORY_MARKER).exists(),
        "the agent ran in the project's own folder as well as, or instead of, the worktree"
    );
    assert_eq!(
        worktree.read("kept.txt"),
        "one\ntwo\nthree\n",
        "the agent's edit did not land in the worktree"
    );
    assert!(
        worktree.path().join("src/new.txt").exists(),
        "the file the agent created is not in the worktree"
    );
    assert_eq!(
        project.read("kept.txt"),
        "one\ntwo\n",
        "the agent edited a tree the conversation was never pointed at"
    );
    assert!(
        !project.path().join("src").exists(),
        "the agent created a file in a tree the conversation was never pointed at"
    );

    // Half two: and the developer can see it. The checkpoint taken for the turn
    // is of the same tree, so reviewing the turn is reviewing the agent's work.
    let diff = client.turn_diff("thread-1", 1).await;
    assert!(
        diff.contains("kept.txt") && diff.contains("+three"),
        "the turn's diff does not show the file the agent changed: {diff}"
    );
    assert!(
        diff.contains("src/new.txt") && diff.contains("+brand new"),
        "the turn's diff does not show the file the agent created: {diff}"
    );

    client.close().await;
    server.stop().await;
}

/// A revert puts back the tree the agent edited, and leaves the other one alone.
///
/// The damaging half of the old behaviour: a revert of a conversation in a
/// worktree restored the worktree, which the agent had never touched — so undoing
/// a turn wrote over a checkout the developer may have had their own work in,
/// while the tree the agent really changed was left as it was. Both trees are
/// asserted for that reason.
#[tokio::test]
async fn a_revert_puts_back_the_worktree_the_agent_edited() {
    let modified = writes("kept.txt", "one\ntwo\nthree\n");
    let created = writes("src/new.txt", "brand new\n");
    let agent = ScriptedAgent::emitting(&[INIT, modified.as_str(), created.as_str(), SAID, DONE]);
    let project = a_repository();
    let worktree = project.worktree("feature");
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client
        .open_conversation_at(&project, "project-1", "thread-1", Some(worktree.path()))
        .await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "make a change"),
        )
        .await
        .expect_success();
    client.events_through_the_checkpoint(&subscription, 1).await;

    // Turn zero is the tree before the first turn ran, which is what the panel
    // names when the developer reverts the conversation's first message.
    client
        .call("orchestration.dispatchCommand", revert_checkpoint("thread-1", 0))
        .await
        .expect_success();
    client.events_through_the_revert(&subscription, 0).await;

    assert_eq!(
        worktree.read("kept.txt"),
        "one\ntwo\n",
        "the tree the agent edited was not put back"
    );
    assert!(
        !worktree.path().join("src/new.txt").exists(),
        "the file the agent created in the worktree is still there"
    );
    assert_eq!(
        project.read("kept.txt"),
        "one\ntwo\n",
        "the project's folder was written to by a revert of a conversation that never ran there"
    );

    client.close().await;
    server.stop().await;
}
