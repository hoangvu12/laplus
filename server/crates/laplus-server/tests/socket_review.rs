//! The current sidebar RPC, not just the saved-turn checkpoint methods.
//! Real repositories and sockets; no provider is needed to review disk edits.

mod harness;

use harness::workspace::Workspace;
use harness::TestServer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn repository() -> Workspace {
    let workspace = Workspace::with(&[]);
    workspace.put("README.md", "before\n");
    workspace.put(".gitignore", "ignored/\n");
    workspace.init_repository().commit("baseline");
    workspace
}

fn source<'a>(preview: &'a Value, kind: &str) -> &'a Value {
    assert!(chrono::DateTime::parse_from_rfc3339(
        preview["generatedAt"].as_str().expect("a wire timestamp")
    )
    .is_ok());
    let sources = preview["sources"].as_array().expect("review sources");
    assert_eq!(sources.len(), 2, "{preview}");
    for source in sources {
        assert!(source["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(source["title"]
            .as_str()
            .is_some_and(|title| !title.is_empty()));
        for key in ["baseRef", "headRef"] {
            assert!(source.get(key).is_some(), "missing {key}: {source}");
            assert!(source[key].is_null() || source[key].is_string());
        }
        let diff = source["diff"].as_str().expect("a patch string");
        assert_eq!(
            source["diffHash"],
            format!("{:x}", Sha256::digest(diff.as_bytes()))
        );
        assert!(source["truncated"].is_boolean());
    }
    sources
        .iter()
        .find(|source| source["kind"] == kind)
        .expect("selected source")
}

#[tokio::test]
async fn working_tree_preview_includes_tracked_staged_and_untracked_edits_without_staging_them() {
    let workspace = repository();
    workspace.put("README.md", "staged\n");
    workspace.git(&["add", "README.md"]);
    workspace.put("README.md", "after editing\n");
    workspace.put("integration note.md", "new file from a tool\n");
    workspace.put("ignored/output.txt", "not part of the review\n");
    let head = workspace.git(&["rev-parse", "HEAD"]);
    let index = workspace.git(&["ls-files", "--stage"]);
    let status = workspace.git(&["status", "--porcelain"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let input = json!({"cwd": workspace.cwd(), "ignoreWhitespace": false});
    let preview = client
        .call("review.getDiffPreview", input.clone())
        .await
        .expect_success();
    assert_eq!(preview["cwd"], workspace.cwd());
    let working = source(&preview, "working-tree");
    let diff = working["diff"].as_str().unwrap();
    assert!(
        diff.contains("-before") && diff.contains("+after editing"),
        "{diff}"
    );
    assert!(
        diff.contains("integration note.md") && diff.contains("+new file from a tool"),
        "{diff}"
    );
    assert!(diff.contains("new file mode"), "{diff}");
    assert!(
        !diff.contains("ignored/") && !diff.contains("+staged"),
        "{diff}"
    );
    assert_eq!(working["truncated"], false);
    let repeated = client
        .call("review.getDiffPreview", input)
        .await
        .expect_success();
    assert_eq!(
        source(&repeated, "working-tree")["diffHash"],
        working["diffHash"]
    );
    workspace.put("integration note.md", "updated tool output\n");
    let updated = client
        .call("review.getDiffPreview", json!({"cwd":workspace.cwd()}))
        .await
        .expect_success();
    assert_ne!(
        source(&updated, "working-tree")["diffHash"],
        working["diffHash"]
    );
    assert_eq!(workspace.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(workspace.git(&["ls-files", "--stage"]), index);
    assert_eq!(workspace.git(&["status", "--porcelain"]), status);
    assert_eq!(workspace.git(&["for-each-ref", "refs/laplus"]), "");
    server.stop().await;
}

#[tokio::test]
async fn branch_preview_uses_the_merge_base_and_keeps_uncommitted_work_separate() {
    let workspace = repository();
    workspace.git(&["switch", "-c", "feature"]);
    workspace.put("feature.txt", "committed feature\n");
    workspace.commit("feature");
    workspace.git(&["switch", "main"]);
    workspace.put("upstream.txt", "only on main\n");
    workspace.commit("main advanced independently");
    workspace.git(&["switch", "feature"]);
    workspace.put("README.md", "uncommitted work\n");
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    for input in [
        json!({"cwd":workspace.cwd(), "baseRef":"main", "ignoreWhitespace":false}),
        json!({"cwd":workspace.cwd(), "ignoreWhitespace":false}),
    ] {
        let preview = client
            .call("review.getDiffPreview", input)
            .await
            .expect_success();
        let branch = source(&preview, "branch-range");
        assert_eq!(branch["baseRef"], "main");
        assert_eq!(branch["headRef"], "feature");
        let diff = branch["diff"].as_str().unwrap();
        assert!(diff.contains("+committed feature"), "{diff}");
        assert!(
            !diff.contains("upstream.txt") && !diff.contains("uncommitted work"),
            "{diff}"
        );
        let working = source(&preview, "working-tree")["diff"].as_str().unwrap();
        assert!(working.contains("+uncommitted work"), "{working}");
        assert!(!working.contains("feature.txt"), "{working}");
    }
    server.stop().await;
}

#[tokio::test]
async fn review_handles_clean_unborn_and_non_repository_folders() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let input = json!({"cwd":workspace.cwd()});
    let none = client
        .call("review.getDiffPreview", input.clone())
        .await
        .expect_success();
    assert_eq!(none["sources"], json!([]));
    workspace.init_repository();
    workspace.put("new.txt", "before the first commit\n");
    let unborn = client
        .call("review.getDiffPreview", input.clone())
        .await
        .expect_success();
    assert!(source(&unborn, "working-tree")["diff"]
        .as_str()
        .unwrap()
        .contains("+before the first commit"));
    assert_eq!(source(&unborn, "branch-range")["diff"], "");
    assert_eq!(
        workspace.git(&["ls-files"]),
        "",
        "preview must not create the user's index"
    );
    workspace.commit("first commit");
    let clean = client
        .call("review.getDiffPreview", input)
        .await
        .expect_success();
    assert_eq!(source(&clean, "working-tree")["diff"], "");
    assert_eq!(source(&clean, "branch-range")["diff"], "");
    server.stop().await;
}

#[tokio::test]
async fn review_honors_whitespace_in_a_linked_worktree() {
    let repository = repository();
    let workspace = repository.worktree("other");
    workspace.put("README.md", "  before  \n");
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let shown = client
        .call(
            "review.getDiffPreview",
            json!({"cwd":workspace.cwd(), "ignoreWhitespace":false}),
        )
        .await
        .expect_success();
    assert!(!source(&shown, "working-tree")["diff"]
        .as_str()
        .unwrap()
        .is_empty());
    let hidden = client
        .call(
            "review.getDiffPreview",
            json!({"cwd":workspace.cwd(), "ignoreWhitespace":true}),
        )
        .await
        .expect_success();
    assert_eq!(source(&hidden, "working-tree")["diff"], "");
    assert_eq!(repository.read("README.md"), "before\n");
    server.stop().await;
}

#[tokio::test]
async fn invalid_review_inputs_have_decodable_git_errors() {
    let workspace = repository();
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    for input in [
        json!({}),
        json!({"cwd":" "}),
        json!({"cwd":workspace.cwd(), "ignoreWhitespace":"yes"}),
        json!({"cwd":workspace.cwd(), "baseRef":" "}),
        json!({"cwd":workspace.cwd(), "baseRef":"--output=not-a-ref"}),
        json!({"cwd":workspace.cwd(), "baseRef":"no-such-branch"}),
        json!({"cwd":workspace.path().join("missing")}),
    ] {
        let error = client
            .call("review.getDiffPreview", input)
            .await
            .expect_declared("GitCommandError");
        assert_eq!(error["operation"], "review.getDiffPreview");
        assert_eq!(error["command"], "git");
        assert!(error["cwd"].is_string());
        assert!(error["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()));
    }
    server.stop().await;
}

#[tokio::test]
async fn oversized_review_patch_reports_truncation_and_hashes_the_returned_patch() {
    let workspace = repository();
    workspace.put(
        "large.txt",
        &"a line of generated content\n".repeat(400_000),
    );
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let preview = client
        .call("review.getDiffPreview", json!({"cwd":workspace.cwd()}))
        .await
        .expect_success();
    let working = source(&preview, "working-tree");
    assert_eq!(working["truncated"], true);
    let diff = working["diff"].as_str().unwrap();
    assert!(diff.contains("truncated by laplus"));
    assert!(
        diff.len() < 10_001_000,
        "bounded while reading git, not after encoding it"
    );
    server.stop().await;
}

/// A project may be one package in a monorepo. Both live sources must name
/// files relative to that project, as the file viewer's read RPC expects.
#[tokio::test]
async fn subdirectory_review_paths_open_in_the_project_and_exclude_siblings() {
    let workspace = repository();
    workspace.put("packages/widget/tracked.txt", "before editing\n");
    workspace.commit("package baseline");
    workspace.git(&["switch", "-c", "feature"]);
    workspace.put("packages/widget/committed.txt", "committed package work\n");
    workspace.put("packages/sibling/committed.txt", "committed sibling work\n");
    workspace.commit("changes in two packages");
    workspace.put("packages/widget/tracked.txt", "modified package work\n");
    workspace.put("packages/widget/new.txt", "untracked package work\n");
    workspace.put("packages/sibling/new.txt", "untracked sibling work\n");
    let cwd = workspace.path().join("packages/widget");
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let preview = client
        .call(
            "review.getDiffPreview",
            json!({"cwd":cwd, "baseRef":"main", "ignoreWhitespace":false}),
        )
        .await
        .expect_success();
    assert_eq!(preview["cwd"], cwd.to_string_lossy().as_ref());
    for (kind, expected) in [
        ("working-tree", vec!["new.txt", "tracked.txt"]),
        ("branch-range", vec!["committed.txt"]),
    ] {
        let diff = source(&preview, kind)["diff"].as_str().unwrap();
        let paths: Vec<&str> = diff
            .lines()
            .filter_map(|line| line.strip_prefix("+++ b/"))
            .collect();
        assert_eq!(
            paths, expected,
            "{kind} must be scoped and relative to cwd: {diff}"
        );
        assert!(
            !diff.contains("sibling"),
            "{kind} leaked another package: {diff}"
        );
        for path in paths {
            assert!(
                diff.contains(&format!("diff --git a/{path} b/{path}")),
                "{diff}"
            );
            let opened = client
                .call("projects.readFile", json!({"cwd":cwd, "relativePath":path}))
                .await
                .expect_success();
            assert_eq!(opened["relativePath"], path);
            assert_eq!(
                opened["contents"],
                workspace.read(&format!("packages/widget/{path}"))
            );
        }
    }
    server.stop().await;
}
