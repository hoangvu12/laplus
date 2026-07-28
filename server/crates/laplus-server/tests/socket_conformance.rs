//! Does laplus answer the way the reference server answered?
//!
//! Ticket 01 recorded the real TypeScript server answering a real UI. This
//! holds laplus's answer next to the recording, for the calls both were
//! asked. It runs at the socket boundary rather than unit-testing an encoder,
//! so what is compared is observable protocol behaviour.
//!
//! Values cannot match — the capture carries another machine's hostname,
//! another checkout's `cwd`. Shape can, and where laplus's shape diverges
//! the divergence is declared with a reason and enforced by
//! [`harness::shape::assert_declared`]: an undeclared difference fails, and so
//! does a declaration that has gone stale.

mod harness;

use harness::agent::FakeAgent;
use harness::captures::Capture;
use harness::shape::{assert_declared, compare, Declared};
use harness::{ClientIdentity, TestServer};
use laplus_server::process::Search;
use serde_json::json;

/// The version the stand-in agent reports for these comparisons.
///
/// Current enough to clear every gate in `provider::BUILT_IN_MODELS`, which is
/// what makes the comparison the strict one: the capture's provider was ready on
/// a current CLI with the whole model table and no message, so a stand-in that
/// reported an older version would be compared as a *different state* and would
/// quietly need extra declarations to pass.
const AGENT_VERSION: &str = "2.1.220";

/// A server whose provider has been resolved against a stand-in agent.
///
/// The capture holds a `claudeAgent` instance that was found, ran, and reported a
/// version. Comparing our *unresolved* provider against it would compare two
/// different states — and comparing our resolved one against the machine's real
/// `claude` would make this suite pass or fail according to what the developer
/// happens to have installed. So the agent is the test's own file, and the
/// comparison is like for like.
async fn resolved_against_a_stand_in_agent(agent: &FakeAgent) -> TestServer {
    let server = TestServer::start().await;
    server
        .refresh_providers(Search::over(&[agent.directory()]))
        .await;
    server
}

/// The whole envelope, not just the payload: `_tag`, `requestId` and the exit
/// tag are what the client's protocol layer reads before it looks at anything
/// inside.
#[tokio::test]
async fn the_success_envelope_matches_the_capture() {
    let capture = Capture::load("02-request-response");
    let captured = capture.response_to("server.getConfig");

    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let live = client.call_raw("server.getConfig", json!({})).await;

    assert_eq!(live["_tag"], captured["_tag"]);
    assert_eq!(live["exit"]["_tag"], captured["exit"]["_tag"]);
    // The capture's client and ours both start their id space at "0".
    assert_eq!(live["requestId"], captured["requestId"]);
    assert_eq!(live["requestId"], json!("0"));

    // No key beyond the three the capture shows.
    let mut keys: Vec<&String> = live.as_object().expect("an object").keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["_tag", "exit", "requestId"]);

    client.close().await;
    server.stop().await;
}

/// The payload the UI decodes against its `ServerConfig` schema. A missing
/// required field here is not a cosmetic difference — it is a decode failure
/// the UI reports as a broken server.
#[tokio::test]
async fn the_server_config_payload_conforms_to_the_capture() {
    let capture = Capture::load("02-request-response");
    let captured = capture.response_to("server.getConfig");

    let agent = FakeAgent::reporting(AGENT_VERSION);
    let server = resolved_against_a_stand_in_agent(&agent).await;
    let mut client = server.connect().await;
    let live = client
        .call_raw("server.getConfig", json!({}))
        .await
        .get("exit")
        .and_then(|exit| exit.get("value"))
        .cloned()
        .expect("a success value");

    // The state being compared, asserted rather than assumed: every declaration
    // below is written against a provider that resolved cleanly, and a stand-in
    // that had stopped working would silently turn this into a comparison of
    // something else.
    assert_eq!(live["providers"][0]["status"], json!("ready"), "{live}");
    assert_eq!(live["providers"][0]["version"], json!(AGENT_VERSION));

    let differences = compare(&captured["exit"]["value"], &live);

    assert_declared("missing fields", &differences.missing, MISSING);
    assert_declared("added fields", &differences.added, ADDED);
    assert_declared("retyped fields", &differences.retyped, RETYPED);
    assert_declared("uncompared arrays", &differences.uncompared, UNCOMPARED);

    client.close().await;
    server.stop().await;
}

/// Fields the reference server sends and laplus does not. Each is either a
/// capability we have not built — where the contract reads absent as
/// unsupported — or a driver v1 does not ship.
const MISSING: &[Declared] = &[
    Declared {
        path: "/environment/capabilities/connectionProbe",
        because: "no server.probe method; the client falls back to probing with server.getConfig",
    },
    Declared {
        path: "/environment/capabilities/threadSettlement",
        because: "thread.settle / thread.unsettle are not implemented",
    },
    Declared {
        path: "/environment/capabilities/threadSnooze",
        because: "thread.snooze / thread.unsnooze are not implemented",
    },
    Declared {
        path: "/providers/0/continuation",
        because: "`groupKey` is how threads are grouped for resumption; ticket 11 \
                  owns multi-turn continuity",
    },
    Declared {
        path: "/providers/0/showInteractionModeToggle",
        because: "the plan / accept-edits toggle; ticket 13 owns permission modes, \
                  and offering the control before it means anything would be a \
                  switch that does nothing",
    },
    Declared {
        path: "/providers/0/versionAdvisory",
        because: "an update check is a network call on boot, which \
                  settings.enableProviderUpdateChecks is off to prevent",
    },
    Declared {
        path: "/providers/0/auth/type",
        because: "nothing reads a credential in ticket 09; auth.status is `unknown`, \
                  which is the contract's own literal for it",
    },
    Declared {
        path: "/providers/0/auth/label",
        because: "as auth/type — no credential is read, so there is no plan to label",
    },
    Declared {
        path: "/providers/0/auth/email",
        because: "as auth/type — no credential is read, so there is no account to name",
    },
    Declared {
        path: "/settings/providers/codex",
        because: "v1 ships one driver — Claude Code",
    },
    Declared {
        path: "/settings/providers/cursor",
        because: "v1 ships one driver — Claude Code",
    },
    Declared {
        path: "/settings/providers/grok",
        because: "v1 ships one driver — Claude Code",
    },
    Declared {
        path: "/settings/providers/opencode",
        because: "v1 ships one driver — Claude Code",
    },
    Declared {
        path: "/shellResumeCompletionMarker",
        because: "ticket 04 streams the configuration only; the shell subscription \
                  and its catch-up marker belong to the orchestration tickets",
    },
    Declared {
        path: "/threadResumeCompletionMarker",
        because: "ticket 04 streams the configuration only; the thread subscription \
                  and its catch-up marker belong to the orchestration tickets",
    },
];

/// Fields laplus sends and the reference server did not. There should never
/// be any: this payload is decoded against upstream's schema, and an unknown
/// key is at best ignored and at worst a decode failure.
///
/// `providers[0].message` came close to belonging here. It is the diagnostic
/// ticket 09 exists to produce, and the captured provider carries none — but that
/// is because a provider that resolved cleanly on a current CLI has nothing to
/// say, and so does the one compared here. A stand-in reporting an older version
/// would carry the model-gate advice and this list would stop being empty, which
/// is why `AGENT_VERSION` is current.
const ADDED: &[Declared] = &[];

/// Fields present in both but holding a different JSON type. Same reasoning as
/// added fields — the client decodes this, so a type change is a break.
const RETYPED: &[Declared] = &[Declared {
    path: "/providers/0/models/0/capabilities",
    because: "`null`, which the contract permits, until the ticket that sends \
              a turn can honour the reasoning and context-window options a \
              descriptor would advertise",
}];

/// Arrays whose element shape this comparison could not reach, because one
/// side was empty. Every one is a field a later ticket fills; when it does,
/// the declaration fails as stale and the element shape starts being compared.
const UNCOMPARED: &[Declared] = &[
    Declared {
        path: "/auth/bootstrapMethods[]",
        because: "laplus has no pairing flow to bootstrap through",
    },
    Declared {
        path: "/issues[]",
        because: "empty in the capture too — the reference server had no config issues either",
    },
    // Both are populated in a real server — see `laplus_server::catalogue` and
    // `socket_provider.rs`, which drives each end to end. They are empty *here*
    // because this test's world is deliberately bare, and that is what keeps the
    // conformance comparison a comparison of shapes rather than of the machine
    // the suite is running on.
    Declared {
        path: "/providers/0/slashCommands[]",
        because: "the stand-in this test resolves answers `--version` and nothing \
                  else, so it never answers the handshake the commands come from",
    },
    Declared {
        path: "/providers/0/skills[]",
        because: "the harness points the provider's Claude home at a directory \
                  that does not exist, so that no assertion here depends on which \
                  skills the developer running the suite has installed",
    },
    Declared {
        path: "/settings/providers/claudeAgent/customModels[]",
        because: "empty in the capture too — no custom models were configured",
    },
];

/// The whole subscription lifecycle, frame by frame, against
/// `04-streaming-subscription.ndjson` — the minimal recording of one: request,
/// chunk, acknowledgement, interrupt, terminal exit.
///
/// The captured subscription is `subscribeTerminalMetadata`, and since
/// ticket 17 laplus answers it — so this compares the whole frame rather
/// than only the envelope. The framing is the same for every subscription on
/// this wire and is the thing ticket 04 exists to prove; that the *payload*
/// inside now matches too is what makes this the strictest conformance
/// assertion in the file.
#[tokio::test]
async fn the_subscription_lifecycle_matches_the_capture() {
    let capture = Capture::load("04-streaming-subscription");
    let captured_chunk = capture.server_frame("Chunk");
    let captured_exit = capture.server_frame("Exit");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client
        .subscribe("subscribeTerminalMetadata", json!({}))
        .await;
    let live_chunk = client.next_frame_for(&subscription).await;
    client.ack(&subscription).await;
    client.interrupt(&subscription).await;
    let live_exit = client.next_frame_for(&subscription).await;

    // The capture's client and ours both open their subscription on id "0".
    assert_eq!(captured_chunk["requestId"], json!("0"));
    assert_eq!(live_chunk["requestId"], captured_chunk["requestId"]);
    assert_eq!(live_exit["requestId"], captured_exit["requestId"]);

    // No key beyond the ones the capture shows, either frame.
    assert_eq!(keys(&live_chunk), keys(&captured_chunk));
    assert_eq!(keys(&live_chunk), vec!["_tag", "requestId", "values"]);
    assert_eq!(keys(&live_exit), keys(&captured_exit));

    // Values batch — a client iterates them — and the capture's one chunk
    // carried a single value, as ours does here. Same method, same empty
    // server, so the value itself is comparable and is compared.
    assert_eq!(live_chunk["values"].as_array().map(Vec::len), Some(1));
    assert_eq!(live_chunk["values"], captured_chunk["values"]);
    assert_eq!(live_chunk, captured_chunk, "the whole frame, byte for byte");

    // A client-initiated unsubscribe ends as a *failure* with an interrupt
    // cause, not a success. The `fiberId` differs — it names a runtime object
    // on the machine that produced it — but its type does not.
    assert_eq!(live_exit["exit"]["_tag"], captured_exit["exit"]["_tag"]);
    assert_eq!(live_exit["exit"]["_tag"], json!("Failure"));
    let live_cause = &live_exit["exit"]["cause"];
    let captured_cause = &captured_exit["exit"]["cause"];
    assert_eq!(live_cause.as_array().map(Vec::len), Some(1));
    assert_eq!(keys(&live_cause[0]), keys(&captured_cause[0]));
    assert_eq!(live_cause[0]["_tag"], captured_cause[0]["_tag"]);
    assert_eq!(live_cause[0]["_tag"], json!("Interrupt"));
    assert!(live_cause[0]["fiberId"].is_u64() && captured_cause[0]["fiberId"].is_u64());

    client.close().await;
    server.stop().await;
}

/// The `subscribeServerConfig` snapshot, against the one the reference server
/// pushed to the real UI during its boot sequence.
///
/// The config inside diverges exactly as `server.getConfig` does — it is the
/// same payload — so the declared lists above are reused rather than
/// duplicated. That is the point: if a later ticket fills `providers` in one
/// place and not the other, one of these two tests fails.
#[tokio::test]
async fn the_config_snapshot_chunk_conforms_to_the_capture() {
    let capture = Capture::load("01-browser-session");
    let chunks = capture.chunks_to("subscribeServerConfig");
    assert_eq!(chunks.len(), 1, "the boot capture holds one config chunk");
    let captured = &chunks[0]["values"][0];

    let agent = FakeAgent::reporting(AGENT_VERSION);
    let server = resolved_against_a_stand_in_agent(&agent).await;
    let mut client = server.connect().await;
    let subscription = client.subscribe("subscribeServerConfig", json!({})).await;
    let live = client.next_event(&subscription).await;

    // The envelope around the config: the client dispatches on `type` and
    // refuses a `version` it does not know.
    assert_eq!(live["version"], captured["version"]);
    assert_eq!(live["type"], captured["type"]);
    assert_eq!(live["type"], json!("snapshot"));
    assert_eq!(keys(&live), keys(captured));

    let differences = compare(&captured["config"], &live["config"]);
    assert_declared("missing fields", &differences.missing, MISSING);
    assert_declared("added fields", &differences.added, ADDED);
    assert_declared("retyped fields", &differences.retyped, RETYPED);
    assert_declared("uncompared arrays", &differences.uncompared, UNCOMPARED);

    client.close().await;
    server.stop().await;
}

/// Divergences in the working tree status snapshot. Both are ticket 19's, and
/// both are named in `laplus_server::git`.
const STATUS_MISSING: &[Declared] = &[Declared {
    path: "/local/sourceControlProvider",
    because: "source-control hosting is out of v1's scope, and this field is a \
              label derived from the remote's URL for GitHub, GitLab and the \
              rest. Ticket 71 removed it from the contract, so the capture is \
              now the only place it appears at all.",
}];

const STATUS_RETYPED: &[Declared] = &[Declared {
    path: "/remote",
    because: "the reference server's remote half costs a network round trip, so \
              it sends the snapshot with a null remote and follows with a \
              remoteUpdated. laplus's ahead/behind come from the tracking \
              ref, which is local and read at the same instant as the rest — so \
              there is nothing to follow with and the snapshot carries it.",
}];

const STATUS_UNCOMPARED: &[Declared] = &[Declared {
    path: "/local/workingTree/files[]",
    because: "the captured repository was clean, so its file list is empty and \
              has no element shape to compare against.",
}];

/// The `subscribeVcsStatus` snapshot, against the one the reference server
/// pushed to the real UI during its boot sequence.
///
/// This is the only capture of a working tree status there is, which makes it
/// the only external check that ticket 19's payload is one the UI can decode.
/// The repository is this test's own, so what is compared is shape.
#[tokio::test]
async fn the_working_tree_status_snapshot_conforms_to_the_capture() {
    let capture = Capture::load("01-browser-session");
    let chunks = capture.chunks_to("subscribeVcsStatus");
    let captured = &chunks[0]["values"][0];
    assert_eq!(captured["_tag"], json!("snapshot"));

    let workspace = harness::workspace::Workspace::with(&["src/main.rs"]);
    workspace.init_repository().commit("first");
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let subscription = client
        .subscribe("subscribeVcsStatus", json!({"cwd": workspace.cwd()}))
        .await;
    let live = client.next_event(&subscription).await;

    // The client dispatches the stream's events on `_tag`, so the envelope has
    // to match before anything inside it is worth comparing.
    assert_eq!(live["_tag"], captured["_tag"]);
    assert_eq!(keys(&live), keys(captured));

    let differences = compare(captured, &live);
    assert_declared("missing fields", &differences.missing, STATUS_MISSING);
    assert_declared("added fields", &differences.added, &[]);
    assert_declared("retyped fields", &differences.retyped, STATUS_RETYPED);
    assert_declared(
        "uncompared arrays",
        &differences.uncompared,
        STATUS_UNCOMPARED,
    );

    client.close().await;
    server.stop().await;
}

/// The `vcs.listRefs` answer, against the one the reference server gave the
/// real UI when it opened the branch picker.
///
/// The captured repository was on `main`, checked out in its own worktree, with
/// an `origin` — and the answer named one ref. That last part is the load-bearing
/// one: `origin/main` and `origin/HEAD` both existed and neither was listed,
/// which is what fixes the default meaning of `includeMatchingRemoteRefs` and of
/// a symbolic remote ref. **There are no declared divergences here**, so any
/// difference at all fails.
#[tokio::test]
async fn the_branch_listing_conforms_to_the_capture() {
    let capture = Capture::load("01-browser-session");
    let captured = capture.response_to("vcs.listRefs");
    let captured = &captured["exit"]["value"];
    assert_eq!(captured["totalCount"], json!(1), "{captured}");

    let workspace = harness::workspace::Workspace::with(&["src/main.rs"]);
    workspace.init_repository().commit("first");
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let live = client
        .call("vcs.listRefs", json!({"cwd": workspace.cwd(), "limit": 100}))
        .await
        .expect_success();

    // The state being compared, asserted rather than assumed: a listing with no
    // refs in it would have no element shape and would pass by saying nothing.
    assert_eq!(live["totalCount"], json!(1), "{live}");

    let differences = compare(captured, &live);
    assert_declared("missing fields", &differences.missing, &[]);
    assert_declared("added fields", &differences.added, &[]);
    assert_declared("retyped fields", &differences.retyped, &[]);
    assert_declared("uncompared arrays", &differences.uncompared, &[]);

    client.close().await;
    server.stop().await;
}

fn keys(value: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("an object, got {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    keys
}

/// The keepalive, which the UI sends every ~5 s for the life of the
/// connection. Byte-for-byte, because there is nothing in it that could
/// legitimately differ.
#[tokio::test]
async fn pong_matches_the_capture_exactly() {
    let capture = Capture::load("02-request-response");
    let captured = capture.server_frame("Pong");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    assert_eq!(client.ping().await, captured);
    assert_eq!(captured, json!({"_tag": "Pong"}));

    client.close().await;
    server.stop().await;
}

/// **The one deliberate divergence in this ticket.**
///
/// The reference server answers an unknown tag with a bare `Defect`. laplus
/// answers with an `Exit`/`Failure` under the caller's `requestId` instead,
/// because a `Defect` fails every pending request and open subscription on the
/// socket rather than just the one — see `rpc::DispatchError::to_error` for the
/// source reading behind that. This test pins both halves: what the capture
/// holds, and what laplus does instead. If either moves, it fails.
#[tokio::test]
async fn an_unimplemented_method_diverges_from_the_captured_defect_on_purpose() {
    let capture = Capture::load("03-typed-error");
    let captured = capture.server_frame("Defect");

    // What the reference server did, still true of the capture.
    assert_eq!(
        captured,
        json!({"_tag": "Defect", "defect": "Unknown request tag: no.such.method"})
    );
    assert!(
        captured.get("requestId").is_none(),
        "the divergence exists because a Defect carries no requestId"
    );

    // What laplus does instead.
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let live = client.call_raw("no.such.method", json!({})).await;

    assert_eq!(live["_tag"], "Exit");
    assert_eq!(live["requestId"], json!("0"));
    assert_eq!(live["exit"]["_tag"], "Failure");
    assert_eq!(live["exit"]["cause"][0]["_tag"], "Fail");
    assert_eq!(
        live["exit"]["cause"][0]["error"]["_tag"],
        "ServerMethodNotImplementedError"
    );

    // The envelope it diverges *into* is itself captured — the typed-error
    // shape from the same fixture, which the client decodes routinely.
    let typed_error = capture.response_to("projects.readFile");
    assert_eq!(live["_tag"], typed_error["_tag"]);
    assert_eq!(live["exit"]["_tag"], typed_error["exit"]["_tag"]);
    assert_eq!(
        live["exit"]["cause"][0]["_tag"],
        typed_error["exit"]["cause"][0]["_tag"]
    );
    assert!(typed_error["exit"]["cause"][0]["error"]["_tag"].is_string());

    client.close().await;
    server.stop().await;
}

/// A refused upgrade. The status and the body's shape match the capture; the
/// `reason` deliberately does not describe what was actually wrong — see
/// `Rejection::body` for why the closed contract union leaves no better
/// answer.
#[tokio::test]
async fn a_refused_upgrade_matches_the_captured_401() {
    let capture = Capture::load("06-upgrade-rejected");
    let captured = capture.http_response_body();
    assert!(capture.http_status_line().contains("401"));

    let server = TestServer::start().await;
    let refusal = server
        .connect_as(ClientIdentity::browser().with_origin("https://evil.example"))
        .await
        .expect_err("a non-local origin is refused");

    assert_eq!(refusal.status, 401);

    let differences = compare(&captured, &refusal.body);
    assert!(
        differences.missing.is_empty()
            && differences.added.is_empty()
            && differences.retyped.is_empty(),
        "the refusal body diverges from the capture: {differences:#?}"
    );

    assert_eq!(refusal.body["_tag"], captured["_tag"]);
    assert_eq!(refusal.body["code"], captured["code"]);
    assert_eq!(
        refusal.body["traceId"].as_str().expect("a traceId").len(),
        captured["traceId"].as_str().expect("a traceId").len()
    );

    server.stop().await;
}

/// The comparison above passes; these show it would not pass on a payload
/// that had actually drifted. A structural check that cannot fail is worse
/// than no check, because it reads like one.
#[test]
fn the_comparison_catches_each_kind_of_drift() {
    let captured = json!({
        "environment": {"label": "DESKTOP", "capabilities": {"connectionProbe": true}},
        "providers": [{"instanceId": "claudeAgent"}],
        "issues": [],
    });

    let identical = compare(&captured, &captured);
    assert!(identical.missing.is_empty());
    assert!(identical.added.is_empty());
    assert!(identical.retyped.is_empty());
    assert_eq!(
        identical.uncompared.iter().collect::<Vec<_>>(),
        vec!["/issues[]"],
        "an empty array on both sides is reported as uncompared, not as agreement"
    );

    let drifted = json!({
        "environment": {"capabilities": {"connectionProbe": true}, "label": 7},
        "providers": [],
        "issues": [],
        "somethingNew": true,
    });
    let differences = compare(&captured, &drifted);

    assert!(differences.retyped.iter().any(|path| path.starts_with("/environment/label")));
    assert_eq!(differences.added.iter().collect::<Vec<_>>(), vec!["/somethingNew"]);
    assert!(differences.uncompared.contains("/providers[]"));

    // And a field the capture has but the payload lost.
    let stripped = json!({"environment": {}, "providers": [], "issues": []});
    assert!(compare(&captured, &stripped)
        .missing
        .contains("/environment/label"));
}

/// The captures are the contract, so a capture set that quietly stopped
/// holding the frames these tests read from would turn every check above into
/// a check of nothing.
#[test]
fn the_captures_still_hold_what_conformance_is_checked_against() {
    let request_response = Capture::load("02-request-response");
    assert!(request_response.response_to("server.getConfig")["exit"]["value"]
        .as_object()
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(
        request_response.server_frame("Pong"),
        json!({"_tag": "Pong"})
    );

    let browser_session = Capture::load("01-browser-session");
    assert_eq!(
        browser_session.chunks_to("subscribeVcsStatus")[0]["values"][0]["_tag"],
        "snapshot"
    );
    assert_eq!(
        browser_session.response_to("vcs.listRefs")["exit"]["value"]["refs"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the branch listing is compared element by element; a capture with no refs \
         in it would compare nothing"
    );

    let typed_error = Capture::load("03-typed-error");
    assert!(typed_error.server_frame("Defect")["defect"].is_string());
    assert_eq!(
        typed_error.response_to("projects.readFile")["exit"]["_tag"],
        "Failure"
    );

    let rejected = Capture::load("06-upgrade-rejected");
    assert!(rejected.http_status_line().contains("401"));
    assert_eq!(
        rejected.http_response_body()["_tag"],
        "EnvironmentAuthInvalidError"
    );
}
