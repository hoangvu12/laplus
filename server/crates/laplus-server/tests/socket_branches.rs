//! Branches through the socket boundary, against real repositories.
//!
//! Ticket 21's last line asks for exactly this, and nothing here reaches into
//! `laplus_server::refs`: a repository is made with the `git` binary and then
//! listed, switched, branched and initialised by sending the four requests the
//! UI sends — `vcs.listRefs` for the branch picker, `vcs.switchRef` for choosing
//! from it, `vcs.createRef` for starting work, and `vcs.init` for a project that
//! is not a repository yet.
//!
//! Two things are checked about every call that changes something, because
//! either one alone would pass while the feature was broken: **what happened on
//! disk**, which is whether the developer's files actually moved, and **what the
//! status panel now says**, which is whether they can tell.

mod harness;

use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// Ask for the branch list — the picker.
async fn list(client: &mut SocketClient, workspace: &Workspace, extra: Value) -> Value {
    let mut payload = json!({"cwd": workspace.cwd()});
    for (key, value) in extra.as_object().expect("an object") {
        payload[key] = value.clone();
    }
    client.call("vcs.listRefs", payload).await.expect_success()
}

/// The `name` of every ref in a listing, in the order it was sent.
fn names(listing: &Value) -> Vec<&str> {
    listing["refs"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of refs: {listing}"))
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a name: {entry}"))
        })
        .collect()
}

/// One ref out of a listing, or a failure naming what was there instead.
fn named<'a>(listing: &'a Value, name: &str) -> &'a Value {
    listing["refs"]
        .as_array()
        .expect("an array of refs")
        .iter()
        .find(|entry| entry["name"] == json!(name))
        .unwrap_or_else(|| panic!("{name} is not in {:?}", names(listing)))
}

/// A repository with one commit behind it, which is what a branch is made from.
fn committed(paths: &[&str]) -> Workspace {
    let workspace = Workspace::with(paths);
    workspace.init_repository().commit("first");
    workspace
}

/// A clone, and the repository it was cloned from.
///
/// The clone is where the remote half of this ticket lives, and a real one is
/// the only way to get at it: `refs/remotes/origin/*`, the symbolic
/// `refs/remotes/origin/HEAD` that must never be a row, and a branch that
/// exists on the remote and not locally are all things `git clone` produces and
/// nothing else does.
fn cloned() -> (Workspace, Workspace) {
    let origin = committed(&["src/main.rs"]);
    origin.git(&["checkout", "-b", "only-on-the-remote"]);
    origin.put("src/remote-only.rs", "fn there() {}\n");
    origin.commit("on the remote");
    origin.git(&["checkout", "main"]);

    // `Workspace::with(&[])` is an empty directory, which is what clone wants.
    let clone = Workspace::with(&[]);
    origin.git(&["clone", &origin.cwd(), &clone.cwd()]);
    // The same four settings `init_repository` pins, and for the same reasons —
    // a clone inherits the machine's global configuration, not the fixture's.
    clone.git(&["config", "user.name", "laplus tests"]);
    clone.git(&["config", "user.email", "tests@laplus.invalid"]);
    clone.git(&["config", "commit.gpgsign", "false"]);
    clone.git(&["config", "core.autocrlf", "false"]);

    (clone, origin)
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// The ticket's first line. A developer cannot choose a branch they cannot see,
/// and cannot choose *sensibly* without knowing which one they are already on.
#[tokio::test]
async fn the_branches_are_listed_with_the_current_one_indicated() {
    let workspace = committed(&["src/main.rs"]);
    workspace.git(&["branch", "feature/one"]);
    workspace.git(&["branch", "feature/two"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = list(&mut client, &workspace, json!({})).await;

    assert_eq!(listing["isRepo"], json!(true));
    assert_eq!(listing["totalCount"], json!(3));
    assert_eq!(listing["nextCursor"], Value::Null, "three fit on one page");
    assert_eq!(
        names(&listing),
        ["main", "feature/one", "feature/two"],
        "the branch we are on leads, and it is also the default one here"
    );

    let current = named(&listing, "main");
    assert_eq!(current["current"], json!(true));
    assert_eq!(current["isDefault"], json!(true));
    assert_eq!(current["isRemote"], json!(false));
    assert!(
        current["worktreePath"]
            .as_str()
            .expect("the branch we are on is checked out somewhere")
            .replace('\\', "/")
            .ends_with(
                &workspace
                    .path()
                    .file_name()
                    .expect("a directory name")
                    .to_string_lossy()
                    .to_string()
            ),
        "{current}"
    );

    // Every other branch is a branch nobody is on.
    for name in ["feature/one", "feature/two"] {
        assert_eq!(named(&listing, name)["current"], json!(false), "{name}");
        assert_eq!(named(&listing, name)["worktreePath"], Value::Null, "{name}");
    }

    server.stop().await;
}

/// A project with no repository is a normal thing to have open — it is what
/// `vcs.init` exists for — so the picker has to describe one rather than fail.
#[tokio::test]
async fn a_project_that_is_not_a_repository_lists_no_branches() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = list(&mut client, &workspace, json!({})).await;

    assert_eq!(listing["isRepo"], json!(false));
    assert_eq!(names(&listing), [] as [&str; 0]);
    assert_eq!(listing["totalCount"], json!(0));
    assert_eq!(listing["nextCursor"], Value::Null);
    assert_eq!(listing["hasPrimaryRemote"], json!(false));

    server.stop().await;
}

/// The picker's own two controls: the box the developer types into, and the
/// scrolling that fetches the next page.
///
/// `totalCount` is over the whole filtered list rather than the page, which is
/// what lets the picker say "12 branches" while showing four of them.
#[tokio::test]
async fn a_listing_is_filtered_by_the_query_and_paged_by_the_cursor() {
    let workspace = committed(&["src/main.rs"]);
    for index in 0..5 {
        workspace.git(&["branch", &format!("feature/branch-{index}")]);
    }
    workspace.git(&["branch", "unrelated"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let matching = list(&mut client, &workspace, json!({"query": "FEATURE/"})).await;
    assert_eq!(matching["totalCount"], json!(5), "case is not a filter");
    assert!(
        !names(&matching).contains(&"unrelated"),
        "{:?}",
        names(&matching)
    );

    let first = list(&mut client, &workspace, json!({"limit": 2})).await;
    assert_eq!(first["totalCount"], json!(7));
    assert_eq!(names(&first).len(), 2);
    assert_eq!(first["nextCursor"], json!(2));

    let second = list(&mut client, &workspace, json!({"limit": 2, "cursor": 2})).await;
    assert_eq!(names(&second).len(), 2);
    assert!(
        names(&second)
            .iter()
            .all(|name| !names(&first).contains(name)),
        "the second page repeats the first: {:?} then {:?}",
        names(&first),
        names(&second)
    );

    let last = list(&mut client, &workspace, json!({"limit": 2, "cursor": 6})).await;
    assert_eq!(names(&last).len(), 1);
    assert_eq!(last["nextCursor"], Value::Null, "seven branches, seven rows");

    server.stop().await;
}

/// The remote half, against a real clone — which is the only place
/// `refs/remotes/*` and the symbolic `origin/HEAD` a clone leaves behind
/// actually exist.
///
/// Three rules, and the capture is what fixed all three: a remote ref whose
/// branch has a local counterpart is folded away, `origin/HEAD` is never a row
/// because it is a pointer at one, and a branch that is only on the remote is
/// listed so it can be switched to.
#[tokio::test]
async fn a_remote_branch_is_listed_only_when_no_local_branch_stands_for_it() {
    let (clone, _origin) = cloned();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = list(&mut client, &clone, json!({})).await;

    assert_eq!(listing["hasPrimaryRemote"], json!(true));
    assert_eq!(
        names(&listing),
        ["main", "origin/only-on-the-remote"],
        "origin/main is folded against the local main, and origin/HEAD is a \
         pointer rather than a branch"
    );

    let remote = named(&listing, "origin/only-on-the-remote");
    assert_eq!(remote["isRemote"], json!(true));
    assert_eq!(remote["remoteName"], json!("origin"));
    assert_eq!(remote["current"], json!(false));
    assert_eq!(remote["worktreePath"], Value::Null);

    // …unless the client asks for the folded ones, which is what the flag is
    // for. `origin/HEAD` stays out even then.
    let all = list(
        &mut client,
        &clone,
        json!({"includeMatchingRemoteRefs": true}),
    )
    .await;
    assert_eq!(
        names(&all),
        ["main", "origin/main", "origin/only-on-the-remote"]
    );

    server.stop().await;
}

/// Asking for the remote side asks "what is on the remote", and the answer is
/// not "the branches nobody has checked out".
///
/// The fold is about a *pair* of rows, so it has no business in a listing that
/// only has one side of every pair — and a clone where every remote branch has
/// a local counterpart is the ordinary case, where folding would answer with
/// nothing at all.
#[tokio::test]
async fn asking_for_the_remote_side_is_not_answered_by_the_fold() {
    let (clone, _origin) = cloned();
    clone.git(&["switch", "--create", "only-on-the-remote", "--track", "origin/only-on-the-remote"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Every remote branch now has a local branch of the same name.
    let both = list(&mut client, &clone, json!({})).await;
    assert_eq!(names(&both), ["only-on-the-remote", "main"]);

    let remotes = list(&mut client, &clone, json!({"refKind": "remote"})).await;
    assert_eq!(
        names(&remotes),
        ["origin/main", "origin/only-on-the-remote"],
        "the remote side went missing because every branch has a local one"
    );

    let locals = list(&mut client, &clone, json!({"refKind": "local"})).await;
    assert_eq!(names(&locals), ["only-on-the-remote", "main"]);

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------

/// A picker that lists remote branches has to be able to switch to one, and a
/// working tree cannot be *on* `origin/x` — so this is the one call here that
/// creates a ref the developer did not name.
#[tokio::test]
async fn switching_to_a_remote_branch_makes_the_local_branch_that_tracks_it() {
    let (clone, _origin) = cloned();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let switched = client
        .call(
            "vcs.switchRef",
            json!({"cwd": clone.cwd(), "refName": "origin/only-on-the-remote"}),
        )
        .await
        .expect_success();

    // The branch the developer is on is the local one, not the ref they picked.
    assert_eq!(switched["refName"], json!("only-on-the-remote"));
    assert_eq!(
        clone.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "only-on-the-remote"
    );
    assert!(
        clone.path().join("src/remote-only.rs").exists(),
        "the working tree did not move"
    );
    // …and it tracks the ref it came from, which is what makes the next pull
    // or push mean anything.
    assert_eq!(
        clone
            .git(&["rev-parse", "--abbrev-ref", "only-on-the-remote@{upstream}"])
            .trim(),
        "origin/only-on-the-remote"
    );

    // Switching to it a second time by its remote name finds the local branch
    // rather than trying to create it again.
    client
        .call("vcs.switchRef", json!({"cwd": clone.cwd(), "refName": "main"}))
        .await
        .expect_success();
    let again = client
        .call(
            "vcs.switchRef",
            json!({"cwd": clone.cwd(), "refName": "origin/only-on-the-remote"}),
        )
        .await
        .expect_success();
    assert_eq!(again["refName"], json!("only-on-the-remote"));

    server.stop().await;
}

/// The ticket's second line, in both halves: the files on disk change, and the
/// panel the developer is looking at says so without being asked.
#[tokio::test]
async fn switching_moves_the_working_tree_and_the_status_follows() {
    let workspace = committed(&["src/main.rs"]);
    workspace.git(&["checkout", "-b", "feature/branches"]);
    workspace.put("src/only-on-the-branch.rs", "fn only() {}\n");
    workspace.commit("on the branch");
    workspace.git(&["checkout", "main"]);
    assert!(
        !workspace.path().join("src/only-on-the-branch.rs").exists(),
        "the fixture is not set up: the branch's file is on main"
    );

    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let watching = client
        .subscribe("subscribeVcsStatus", json!({"cwd": workspace.cwd()}))
        .await;
    client.status_until(&watching, |status| {
        status["refName"] == json!("main")
    })
    .await;

    let switched = client
        .call(
            "vcs.switchRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/branches"}),
        )
        .await
        .expect_success();

    // What happened on disk.
    assert_eq!(switched["refName"], json!("feature/branches"));
    assert!(
        workspace.path().join("src/only-on-the-branch.rs").exists(),
        "the working tree did not move"
    );

    // …and what the developer can see. Nothing asked for this.
    let told = client.status_until(&watching, |status| {
        status["refName"] == json!("feature/branches")
    })
    .await;
    assert_eq!(told["isRepo"], json!(true));
    assert_eq!(told["isDefaultRef"], json!(false));

    // The picker agrees about where we ended up.
    let listing = list(&mut client, &workspace, json!({})).await;
    assert_eq!(named(&listing, "feature/branches")["current"], json!(true));
    assert_eq!(named(&listing, "main")["current"], json!(false));

    client.interrupt(&watching).await;
    server.stop().await;
}

/// The ticket's fifth line, and the one that says what kind of tool this is. A
/// switch that would throw away work the developer has not committed is refused
/// — not forced, and not silently skipped.
#[tokio::test]
async fn a_switch_that_would_lose_uncommitted_work_is_refused_with_an_explanation() {
    let workspace = committed(&["shared.txt"]);
    workspace.git(&["checkout", "-b", "theirs"]);
    workspace.put("shared.txt", "their version\n");
    workspace.commit("theirs");
    workspace.git(&["checkout", "main"]);

    // Work in progress, of the kind an agent produces and nobody has reviewed.
    workspace.put("shared.txt", "half-finished, not committed\n");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.switchRef",
            json!({"cwd": workspace.cwd(), "refName": "theirs"}),
        )
        .await
        .expect_declared("GitCommandError");

    assert_eq!(error["operation"], json!("vcs.switchRef"));
    let detail = error["detail"].as_str().expect("a detail");
    assert!(
        detail.contains("shared.txt"),
        "the refusal must name what is in the way: {detail}"
    );
    assert!(
        detail.contains("commit") || detail.contains("stash"),
        "the refusal must say what to do about it: {detail}"
    );
    assert!(error["exitCode"].is_number(), "{error}");

    // Nothing was lost and nothing moved, which is the half of this that
    // matters more than the sentence.
    assert_eq!(workspace.read("shared.txt"), "half-finished, not committed\n");
    assert_eq!(
        workspace.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );

    server.stop().await;
}

/// A branch that is not there is refused before git is asked, because the
/// listing this name came from is the server's own answer — so a name that is
/// not in it is a stale picker, and saying so is more use than passing git's
/// "invalid reference" through.
#[tokio::test]
async fn switching_to_a_branch_that_does_not_exist_is_refused_by_name() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.switchRef",
            json!({"cwd": workspace.cwd(), "refName": "never-existed"}),
        )
        .await
        .expect_declared("GitCommandError");

    assert!(
        error["detail"]
            .as_str()
            .expect("a detail")
            .contains("never-existed"),
        "{error}"
    );
    assert_eq!(
        workspace.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );

    server.stop().await;
}

/// A remote whose *name* has a slash in it, which git allows and which breaks
/// every "the branch is what follows the first slash" shortcut.
///
/// The listing and the switch have to agree about where the split is, because
/// a switch is asked for by a name the listing produced. Reading it as
/// `origin` + `mirror/main` would leave the developer on a local branch called
/// `mirror/main` — a branch that tracks the right thing under a name nobody
/// chose.
#[tokio::test]
async fn a_remote_with_a_slash_in_its_name_is_split_where_the_remote_ends() {
    let origin = committed(&["src/main.rs"]);
    let clone = committed(&["other.txt"]);
    // Renamed so that the remote's `main` has no local branch standing for it,
    // which is what makes the switch below take the tracking path.
    clone.git(&["branch", "--move", "trunk"]);
    clone.git(&["remote", "add", "origin/mirror", &origin.cwd()]);
    clone.git(&["fetch", "origin/mirror"]);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = list(&mut client, &clone, json!({})).await;
    let remote = named(&listing, "origin/mirror/main");
    assert_eq!(remote["remoteName"], json!("origin/mirror"));

    let switched = client
        .call(
            "vcs.switchRef",
            json!({"cwd": clone.cwd(), "refName": "origin/mirror/main"}),
        )
        .await
        .expect_success();

    assert_eq!(
        switched["refName"],
        json!("main"),
        "the remote's own name was mistaken for part of the branch"
    );
    assert_eq!(
        clone
            .git(&["rev-parse", "--abbrev-ref", "main@{upstream}"])
            .trim(),
        "origin/mirror/main"
    );

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Creating
// ---------------------------------------------------------------------------

/// The ticket's third line. "From the current position" is the whole of it: a
/// branch made to start work has to start where the developer is standing.
#[tokio::test]
async fn a_branch_is_created_from_the_current_position() {
    let workspace = committed(&["src/main.rs"]);
    workspace.put("src/main.rs", "second commit\n");
    workspace.commit("second");
    let here = workspace.git(&["rev-parse", "HEAD"]).trim().to_string();

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let created = client
        .call(
            "vcs.createRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/new"}),
        )
        .await
        .expect_success();

    assert_eq!(created, json!({"refName": "feature/new"}));
    assert_eq!(
        workspace.git(&["rev-parse", "feature/new"]).trim(),
        here,
        "the branch was not made where we were standing"
    );
    // Made, but not moved to — that is what `switchRef` is for, and it was not
    // asked for.
    assert_eq!(
        workspace.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "main"
    );

    let listing = list(&mut client, &workspace, json!({})).await;
    assert!(names(&listing).contains(&"feature/new"), "{listing}");

    server.stop().await;
}

/// The same call with `switchRef`, which is what the picker's "create branch"
/// actually sends: a developer naming a branch means to start working on it.
#[tokio::test]
async fn a_branch_can_be_created_and_switched_to_in_one_call() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = client
        .subscribe("subscribeVcsStatus", json!({"cwd": workspace.cwd()}))
        .await;
    client.status_until(&watching, |status| {
        status["refName"] == json!("main")
    })
    .await;

    let created = client
        .call(
            "vcs.createRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/started", "switchRef": true}),
        )
        .await
        .expect_success();

    assert_eq!(created, json!({"refName": "feature/started"}));
    assert_eq!(
        workspace.git(&["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "feature/started"
    );

    let told = client.status_until(&watching, |status| {
        status["refName"] == json!("feature/started")
    })
    .await;
    assert_eq!(told["hasWorkingTreeChanges"], json!(false));

    client.interrupt(&watching).await;
    server.stop().await;
}

/// The ticket's sixth line. A developer who reaches for a name they already
/// used needs to be told which name, not handed git's ref vocabulary.
#[tokio::test]
async fn creating_a_branch_whose_name_is_taken_is_refused_and_changes_nothing() {
    let workspace = committed(&["src/main.rs"]);
    workspace.git(&["branch", "feature/taken"]);
    let before = workspace.git(&["rev-parse", "feature/taken"]).trim().to_string();
    workspace.put("src/main.rs", "moved on\n");
    workspace.commit("second");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.createRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/taken"}),
        )
        .await
        .expect_declared("GitCommandError");

    assert_eq!(error["operation"], json!("vcs.createRef"));
    let detail = error["detail"].as_str().expect("a detail");
    assert!(detail.contains("feature/taken"), "{detail}");
    assert!(detail.contains("already exists"), "{detail}");
    assert!(error.get("message").is_none(), "{error}");

    // The branch that was there is still where it was — a refusal that moved
    // it would be worse than one that said nothing.
    assert_eq!(
        workspace.git(&["rev-parse", "feature/taken"]).trim(),
        before
    );

    server.stop().await;
}

/// "From the current position" has no answer in a repository that has no
/// commits — which is exactly what `vcs.init` leaves behind, so it is the
/// sequence the ticket itself sets up. The refusal says why; git's own
/// `not a valid object name: 'HEAD'` would not.
///
/// Switching to the new branch still works there, because an unborn branch is
/// only a name in `HEAD` and renaming it costs nothing.
#[tokio::test]
async fn a_branch_cannot_be_made_from_a_position_a_fresh_repository_has_not_got() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .call("vcs.init", json!({"cwd": workspace.cwd()}))
        .await
        .expect_success();

    let error = client
        .call(
            "vcs.createRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/too-early"}),
        )
        .await
        .expect_declared("GitCommandError");
    let detail = error["detail"].as_str().expect("a detail");
    assert!(detail.contains("no commits"), "{detail}");
    assert!(detail.contains("feature/too-early"), "{detail}");

    // The half that does work, so that the refusal above is a statement about
    // git rather than about this server.
    let created = client
        .call(
            "vcs.createRef",
            json!({"cwd": workspace.cwd(), "refName": "feature/started", "switchRef": true}),
        )
        .await
        .expect_success();
    assert_eq!(created, json!({"refName": "feature/started"}));
    assert_eq!(
        workspace
            .git(&["symbolic-ref", "--short", "HEAD"])
            .trim(),
        "feature/started"
    );

    server.stop().await;
}

/// The ticket's seventh line. Every one of these is a name `git check-ref-format`
/// would refuse; the point is that none of them reaches a `git`, and that what
/// comes back describes a *branch name* rather than a ref.
#[tokio::test]
async fn an_invalid_branch_name_is_rejected_before_it_reaches_git() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for (name, expected) in [
        ("my branch", "space"),
        ("feature~1", "'~'"),
        ("-rf", "start with '-'"),
        ("a..b", "'..'"),
        ("trailing/", "'/'"),
        (".hidden", "start with '.'"),
        ("feature.lock", "'.lock'"),
    ] {
        for tag in ["vcs.createRef", "vcs.switchRef"] {
            let error = client
                .call(tag, json!({"cwd": workspace.cwd(), "refName": name}))
                .await
                .expect_declared("GitCommandError");

            assert_eq!(error["operation"], json!(tag), "{name}");
            let detail = error["detail"].as_str().expect("a detail");
            assert!(
                detail.contains("branch name") && detail.contains(expected),
                "{tag} refused {name:?} with {detail:?}, which does not say {expected:?} about a \
                 branch name"
            );
        }
    }

    // Nothing was created and nothing moved.
    let listing = list(&mut client, &workspace, json!({})).await;
    assert_eq!(names(&listing), ["main"]);

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Initialising
// ---------------------------------------------------------------------------

/// The ticket's fourth line, in both halves: the repository appears, and the
/// status the developer was already looking at becomes a real one.
///
/// This is the reason `crate::git` reports a folder with no repository as a
/// status rather than as an error — the panel is on screen *before* this call,
/// showing `isRepo: false`, and this is the button on it.
#[tokio::test]
async fn a_project_with_no_repository_can_be_initialised_and_then_has_a_status() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = client
        .subscribe("subscribeVcsStatus", json!({"cwd": workspace.cwd()}))
        .await;
    let before = client.status_until(&watching, |_| true).await;
    assert_eq!(before["isRepo"], json!(false), "the fixture already had one");
    server.await_watched_workspaces(1).await;

    let initialised = client
        .call("vcs.init", json!({"cwd": workspace.cwd()}))
        .await
        .expect_success();
    // `vcs.init` declares no success value, and `Schema.Void` is `null` on this
    // wire — the same answer `terminal.write` gives.
    assert_eq!(initialised, Value::Null);

    assert!(workspace.path().join(".git").is_dir(), "no repository was made");

    // The panel finds out without being asked, which is what "after which
    // status works" has to mean for a panel that is already open.
    let after = client.status_until(&watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;
    assert!(
        after["refName"].is_string(),
        "a fresh repository is on a branch, even with nothing committed: {after}"
    );
    // Everything in the project is new, because there is nothing to compare it
    // against yet.
    assert_eq!(after["hasWorkingTreeChanges"], json!(true));

    // And the branch picker agrees, which it could not before.
    let listing = list(&mut client, &workspace, json!({})).await;
    assert_eq!(listing["isRepo"], json!(true));
    assert_eq!(
        names(&listing).len(),
        1,
        "the branch HEAD names, even with no commit behind it: {listing}"
    );
    assert_eq!(listing["refs"][0]["current"], json!(true));

    client.interrupt(&watching).await;
    server.stop().await;
}

/// `vcs.init` declares a different error union from the other three —
/// `VcsError`, which has no `GitCommandError` in it — so a folder that is not
/// there has to be refused under one of *its* tags or the client cannot decode
/// the failure.
#[tokio::test]
async fn initialising_a_folder_that_is_not_there_is_refused_under_the_declared_union() {
    let workspace = Workspace::with(&[]);
    let missing = workspace
        .path()
        .join("not-there")
        .to_string_lossy()
        .into_owned();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call("vcs.init", json!({"cwd": missing}))
        .await
        .expect_declared("VcsRepositoryDetectionError");

    assert_eq!(error["operation"], json!("vcs.init"));
    assert_eq!(error["cwd"], json!(missing));
    assert!(
        error["detail"]
            .as_str()
            .expect("a detail")
            .contains("does not exist"),
        "{error}"
    );
    // `message` is a getter on the client's own error class.
    assert!(error.get("message").is_none(), "{error}");

    server.stop().await;
}

/// The contract's `kind` has three values and this server drives one of them.
/// The union has an error that says exactly that, so it is used rather than
/// quietly making a git repository somebody asked jj for.
#[tokio::test]
async fn initialising_a_kind_this_server_does_not_drive_is_refused_rather_than_substituted() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call("vcs.init", json!({"cwd": workspace.cwd(), "kind": "jj"}))
        .await
        .expect_declared("VcsUnsupportedOperationError");

    assert_eq!(error["kind"], json!("jj"));
    assert!(
        !workspace.path().join(".git").exists(),
        "a git repository was made for a call that asked for jj"
    );

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Refusals shared by the three git-shaped methods
// ---------------------------------------------------------------------------

/// A workspace root the server cannot use is refused with the error these
/// methods declare — `GitCommandError` — rather than with the unknown-method
/// error, which would tell the client the server cannot do branches at all.
#[tokio::test]
async fn a_workspace_root_that_is_not_there_is_refused_with_the_declared_error() {
    let workspace = Workspace::with(&[]);
    let missing = workspace
        .path()
        .join("not-there")
        .to_string_lossy()
        .into_owned();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for (tag, payload) in [
        ("vcs.listRefs", json!({"cwd": missing})),
        (
            "vcs.createRef",
            json!({"cwd": missing, "refName": "feature/x"}),
        ),
        (
            "vcs.switchRef",
            json!({"cwd": missing, "refName": "feature/x"}),
        ),
    ] {
        let error = client
            .call(tag, payload)
            .await
            .expect_declared("GitCommandError");

        assert_eq!(error["operation"], json!(tag));
        assert_eq!(error["cwd"], json!(missing));
        assert!(
            error["detail"]
                .as_str()
                .expect("a detail")
                .contains("does not exist"),
            "{tag}: {error}"
        );
        assert!(error.get("message").is_none(), "{tag}: {error}");
    }

    server.stop().await;
}
