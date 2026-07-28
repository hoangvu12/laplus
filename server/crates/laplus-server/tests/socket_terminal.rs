//! A terminal, opened, typed into and resized through the socket boundary.
//!
//! Ticket 17's last requirement is that open, write and resize are driven
//! through the socket and the *streamed output* asserted, and this file is that.
//! Nothing here reaches into `laplus_server::terminal`: a shell is started by
//! sending the request the terminal drawer sends, and what it says is read off
//! the subscription the drawer subscribes to.
//!
//! ## The shell is real, and it has to be
//!
//! Everything else agent-facing in this suite runs against a scripted stand-in,
//! and the spec says so. A terminal cannot: the thing under test *is* the pty,
//! and a fake shell would prove that a fake shell works. So these tests start
//! `cmd.exe` — or `/bin/sh` — in a temporary directory and assert on what it
//! prints. That stays inside the spec's rule about tests, which is about the
//! Anthropic API rather than about subprocesses: this is offline, free, and
//! deterministic because every assertion is on a marker the test itself asked
//! for rather than on timing or on the shell's own decoration.
//!
//! Which shell is a value the client sends, not a switch: `ComSpec` and `SHELL`
//! are the platform's conventional names for the command interpreter and the
//! server consults the session's environment for them. See
//! `harness::terminal::shell_choice`.
//!
//! ## The test is the emulator
//!
//! The server is a wire between a pty and an emulator and does not interpret
//! what crosses it. That makes these tests the emulator, and it is not
//! ceremony: ConPTY opens by asking where the cursor is and **blocks until it is
//! told**, so a harness that only read would watch a shell that never printed a
//! prompt. `harness::terminal::Pane` answers. It is also the reason
//! `terminal.write` is not only "what the developer typed" — a good deal of
//! what a real emulator writes is the answers to questions.

mod harness;

use harness::terminal::{
    colour_command, echo, quit, report_size, reported_size, shell_choice, Pane, RED,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

const THREAD: &str = "thread-1";
const TERMINAL: &str = "term-1";

/// `terminal.open` as the drawer sends it.
async fn open(client: &mut SocketClient, workspace: &Workspace, cols: u64, rows: u64) -> Value {
    client
        .call(
            "terminal.open",
            json!({
                "threadId": THREAD,
                "terminalId": TERMINAL,
                "cwd": workspace.cwd(),
                "cols": cols,
                "rows": rows,
                "env": shell_choice(),
            }),
        )
        .await
        .expect_success()
}

/// A terminal opens where the developer's project is, and what is typed into it
/// reaches a shell that answers.
///
/// The first two acceptance lines of the ticket, and they are one test because
/// they are one fact: the answer proves the shell is running *and* that it is
/// running in the right place, since the marker is a file that only exists
/// there.
#[tokio::test]
async fn a_terminal_opens_in_the_project_and_answers_what_is_typed_into_it() {
    let workspace = Workspace::with(&["only-here.txt"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let snapshot = open(&mut client, &workspace, 120, 30).await;
    assert_eq!(snapshot["threadId"], THREAD);
    assert_eq!(snapshot["terminalId"], TERMINAL);
    assert_eq!(snapshot["status"], "running");
    assert_eq!(snapshot["cwd"], workspace.cwd());
    assert_eq!(snapshot["label"], "Terminal 1");
    assert!(snapshot["pid"].as_u64().expect("a process id") > 0);
    assert_eq!(snapshot["exitCode"], Value::Null);
    assert_eq!(server.live_terminals(), 1);

    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    // Listing the directory is the working-directory assertion: the file is in
    // the temporary workspace and nowhere else on the machine.
    pane.run(&mut client, &directory_listing()).await;
    pane.wait_for(&mut client, "only-here.txt").await;

    // …and an arbitrary marker, to say that this is a shell taking commands
    // rather than one directory listing that happened to work.
    pane.run(&mut client, &echo("laplus-was-here")).await;
    pane.wait_for(&mut client, "laplus-was-here").await;

    client.close().await;
    server.stop().await;
}

/// The pty is really resized, and the proof is the shell's own account of how
/// big it thinks it is.
///
/// This is the only assertion available for "output rewraps" that does not need
/// an emulator: rewrapping is what the *program* does with the size it is given,
/// so asking the program what size it was given is the honest test, and the
/// answer arrives on the same stream everything else does.
#[tokio::test]
async fn resizing_a_terminal_resizes_the_pty_the_shell_is_running_in() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 97, 24).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;

    pane.run(&mut client, &report_size()).await;
    pane.wait_for(&mut client, size_marker()).await;
    assert!(
        reported_size(&pane.text(), 97, 24),
        "the shell did not see the size it was opened at:\n{}",
        pane.text()
    );

    pane.resize(&mut client, 131, 40).await.expect_success();
    let before = pane.text().len();
    pane.run(&mut client, &report_size()).await;
    loop {
        pane.pump(&mut client).await;
        if reported_size(&pane.text()[before..], 131, 40) {
            break;
        }
        assert!(
            pane.text().len() < before + 100_000,
            "the shell never reported the new size:\n{}",
            &pane.text()[before..]
        );
    }

    client.close().await;
    server.stop().await;
}

/// Colour reaches the client as colour — a control sequence — rather than as
/// text or as nothing.
///
/// This is what "colour output renders correctly" and "interactive full-screen
/// programs render correctly" mean on *this* side of the wire. Neither is
/// something the server implements; they are things it must not break, and the
/// two ways it could are covered here: a shell on a pipe rather than a pty emits
/// no colour at all, and a server that read the stream instead of forwarding it
/// could strip or mangle what the shell did emit.
///
/// Deliberately not a byte-for-byte comparison against what the *program*
/// wrote — see `harness::terminal::colour_command` for why that was never the
/// server's to promise.
#[tokio::test]
async fn colour_reaches_the_client_as_a_control_sequence_rather_than_as_text() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;

    pane.run(&mut client, &colour_command("laplus")).await;
    pane.wait_for(&mut client, RED).await;

    // The colour is there, and the text it colours is there…
    assert!(pane.text().contains("laplus"), "{}", pane.text());
    // …and the colour is not *in* the text, which is what makes it a control
    // sequence rather than characters that happen to look like one.
    assert!(
        !pane.text().contains(RED) && !pane.text().contains('\u{1b}'),
        "the escape reached the reader as text: {:?}",
        pane.text()
    );

    client.close().await;
    server.stop().await;
}

/// The shell's exit is reported, and the terminal remembers how it went.
#[tokio::test]
async fn a_shell_that_exits_says_so_and_leaves_a_terminal_that_says_how() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Subscribed before the exit rather than after it. A list that only told
    // the truth to whoever asked *next* would leave a tab that was already open
    // looking busy forever.
    let listing = metadata(&mut client).await;
    client.next_event(&listing).await;

    open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    // Waited for, so the shell is certainly reading input before it is asked to
    // leave — otherwise this races the prompt and proves nothing about exiting.
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;

    pane.run(&mut client, &quit()).await;
    let exit = pane.wait_for_exit(&mut client).await;
    assert_eq!(exit["threadId"], THREAD);
    assert_eq!(exit["terminalId"], TERMINAL);
    assert_eq!(exit["exitCode"], 0);
    assert_eq!(exit["exitSignal"], Value::Null);
    server.await_live_terminals(0).await;

    // And the terminal is still there to be looked at, saying what happened —
    // a pane that vanished on exit would take the output with it. The list is
    // *told*, on the subscription that was already open: first that the
    // terminal started, then that it exited.
    let started = client.next_event(&listing).await;
    assert_eq!(started["terminal"]["status"], "running");
    let ended = client.next_event(&listing).await;
    assert_eq!(ended["type"], "upsert");
    assert_eq!(ended["terminal"]["status"], "exited");
    assert_eq!(ended["terminal"]["exitCode"], 0);
    assert_eq!(ended["terminal"]["pid"], Value::Null);

    // Typing at it is refused by name rather than silently dropped.
    let refusal = pane
        .type_in(&mut client, "still there?\r")
        .await
        .expect_declared("TerminalNotRunningError");
    assert_eq!(refusal["threadId"], THREAD);

    client.close().await;
    server.stop().await;
}

/// Opening the same terminal twice is the same terminal, not a second shell.
///
/// The UI opens and attaches from two different places on the same mount, so a
/// second open is ordinary traffic. A server that started a fresh shell for one
/// would replace whatever the developer was running with a blank prompt.
#[tokio::test]
async fn opening_a_terminal_that_is_already_open_returns_the_one_that_is_running() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("first-shell")).await;
    pane.wait_for(&mut client, "first-shell").await;

    let second = open(&mut client, &workspace, 120, 30).await;
    assert_eq!(first["pid"], second["pid"], "a second shell was started");
    assert_eq!(server.live_terminals(), 1);
    assert!(
        second["history"]
            .as_str()
            .expect("scrollback")
            .contains("first-shell"),
        "the second open lost what the terminal had said"
    );

    client.close().await;
    server.stop().await;
}

/// An attach that arrives before the open carries enough to be the open.
///
/// Both calls are made by the reused UI from different places on the same
/// mount, so neither can be sure it went first. This is the ordering that has
/// nothing to attach to.
#[tokio::test]
async fn attaching_first_opens_the_terminal_it_was_going_to_attach_to() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let mut pane = Pane::attach(
        &mut client,
        THREAD,
        TERMINAL,
        Pane::opening(workspace.path(), 120, 30),
    )
    .await;
    assert_eq!(server.live_terminals(), 1);

    pane.run(&mut client, &echo("opened-by-attaching")).await;
    pane.wait_for(&mut client, "opened-by-attaching").await;

    client.close().await;
    server.stop().await;
}

/// A terminal opened before anything attached to it still works.
///
/// The hazard this pins is specific and was measured rather than guessed: a
/// shell's *first* output is a question, the answer is what unblocks it, and a
/// question does not belong in scrollback because scrollback is replayed. So a
/// question with nothing attached to answer it has to be remembered and asked
/// again — otherwise this exact ordering produces a terminal that is running,
/// looks healthy, and never prints a prompt.
#[tokio::test]
async fn a_terminal_opened_before_anything_attached_still_reaches_a_prompt() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    // Long enough that the shell has certainly asked its question and is
    // certainly blocked on it before anything is listening.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("asked-and-answered")).await;
    pane.wait_for(&mut client, "asked-and-answered").await;

    client.close().await;
    server.stop().await;
}

/// Output the client has already been given as scrollback is not given again as
/// output.
///
/// Every other subscription on this wire delivers replacements, so seeing one
/// twice is harmless. A terminal's output is appended, so an overlap is text on
/// the screen twice and nothing later corrects it. Driven by re-describing the
/// world — which is what the server does whenever a subscriber falls behind.
#[tokio::test]
async fn a_re_described_terminal_does_not_repeat_what_the_snapshot_already_held() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("said-once")).await;
    pane.wait_for(&mut client, "said-once").await;

    // A second attachment on the same connection, which opens with a snapshot
    // of everything said so far and must not then be told it again.
    let mut second = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    second.run(&mut client, &echo("said-twice")).await;
    second.wait_for(&mut client, "said-twice").await;

    assert_eq!(
        second.text().matches("said-once").count(),
        // Twice: the shell echoes the command line and then runs it. Which is
        // the point — it is a stable number, and a repeated snapshot would make
        // it four.
        2,
        "output arrived both in the snapshot and again as an event:\n{}",
        second.text()
    );

    client.close().await;
    server.stop().await;
}

/// A great deal of output does not stall the connection or lose the socket.
///
/// The bound is real rather than nominal: a terminal's feed holds 64 events and
/// the pane deliberately reads slowly, so this drives the path where the server
/// gives up on catching up and re-describes the world instead. What must
/// survive is the connection — the socket still answers, and the terminal is
/// still usable afterwards.
#[tokio::test]
async fn a_flood_of_output_neither_stalls_the_socket_nor_loses_the_terminal() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;

    pane.run(&mut client, &flood()).await;
    pane.wait_for(&mut client, "flood-finished").await;

    // The connection is still the connection: a plain call is answered, and
    // the terminal still takes commands.
    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));
    pane.run(&mut client, &echo("still-here")).await;
    pane.wait_for(&mut client, "still-here").await;

    client.close().await;
    server.stop().await;
}

/// Stopping the server takes the shells with it.
///
/// A terminal that outlived the app would hold the project's files open with
/// nothing left able to show it to the developer, which is the same leak an
/// orphaned agent would be.
#[tokio::test]
async fn stopping_the_server_reaps_every_terminal() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;
    assert_eq!(server.live_terminals(), 1);

    client.abandon();
    server.stop().await;
}

/// The terminal list, which is the first thing the UI subscribes to about
/// terminals and the one subscription in this ticket a capture pins whole.
#[tokio::test]
async fn the_terminal_list_opens_empty_and_gains_what_is_opened() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = metadata(&mut client).await;
    assert_eq!(
        client.next_event(&listing).await,
        json!({"type": "snapshot", "terminals": []}),
        "the captured snapshot for an empty server"
    );

    open(&mut client, &workspace, 120, 30).await;
    let upsert = client.next_event(&listing).await;
    assert_eq!(upsert["type"], "upsert");

    let terminal = &upsert["terminal"];
    for key in [
        "threadId",
        "terminalId",
        "cwd",
        "worktreePath",
        "status",
        "pid",
        "exitCode",
        "exitSignal",
        "hasRunningSubprocess",
        "label",
        "updatedAt",
    ] {
        assert!(
            terminal.get(key).is_some(),
            "a summary missing {key} fails the client's decode: {terminal}"
        );
    }
    assert_eq!(terminal["terminalId"], TERMINAL);
    assert_eq!(terminal["status"], "running");
    assert_eq!(terminal["label"], "Terminal 1");

    client.close().await;
    server.stop().await;
}

/// A working directory that is not one is refused by what is wrong with it, and
/// nothing is started.
#[tokio::test]
async fn a_terminal_is_refused_by_what_is_wrong_with_where_it_was_asked_to_open() {
    let workspace = Workspace::with(&["a-file.txt"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for (cwd, expected) in [
        (workspace.path().join("not-here"), "TerminalCwdNotFoundError"),
        (
            workspace.path().join("a-file.txt"),
            "TerminalCwdNotDirectoryError",
        ),
    ] {
        let refusal = client
            .call(
                "terminal.open",
                json!({
                    "threadId": THREAD,
                    "terminalId": TERMINAL,
                    "cwd": cwd.to_string_lossy(),
                }),
            )
            .await
            .expect_declared(expected);
        assert_eq!(refusal["cwd"], cwd.to_string_lossy().into_owned());
    }

    assert_eq!(server.live_terminals(), 0);
    client.close().await;
    server.stop().await;
}

/// An attach that offered to open a terminal and named a directory that is not
/// one is told *that*, rather than "no such terminal".
///
/// Both are true of the same call. Only one of them is the thing to go and fix,
/// and the contract has a separate error class for it precisely because they are
/// different facts.
#[tokio::test]
async fn an_attach_naming_a_directory_that_is_not_one_says_so() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let missing = workspace.path().join("not-here");
    let refusal = client
        .call(
            "terminal.attach",
            json!({
                "threadId": THREAD,
                "terminalId": TERMINAL,
                "cwd": missing.to_string_lossy(),
            }),
        )
        .await
        .expect_declared("TerminalCwdNotFoundError");
    assert_eq!(refusal["cwd"], missing.to_string_lossy().into_owned());

    // …and one that offered nothing is still a plain lookup failure.
    client
        .call(
            "terminal.attach",
            json!({"threadId": THREAD, "terminalId": TERMINAL}),
        )
        .await
        .expect_declared("TerminalSessionLookupError");

    client.close().await;
    server.stop().await;
}

/// More input than the contract allows in one call is refused rather than
/// truncated.
///
/// Truncating would silently drop keystrokes, and the queue in front of the
/// shell is bounded in *slots* — so without a bound on each one, sixty-four
/// slots is not a bound on anything.
#[tokio::test]
async fn an_oversized_write_is_refused_rather_than_truncated() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace, 120, 30).await;
    let pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;

    let refusal = pane
        .type_in(&mut client, &"x".repeat(65_537))
        .await
        .expect_declared("TerminalWriteError");
    assert_eq!(refusal["threadId"], THREAD);
    assert!(refusal["terminalPid"].as_u64().expect("a process id") > 0);

    // The one below the limit is accepted, so this is a limit rather than a
    // refusal to take anything large.
    pane.type_in(&mut client, &"x".repeat(65_536))
        .await
        .expect_success();

    client.close().await;
    server.stop().await;
}

/// Open a `subscribeTerminalMetadata` and hand back its request id.
async fn metadata(client: &mut SocketClient) -> String {
    client.subscribe("subscribeTerminalMetadata", json!({})).await
}

fn directory_listing() -> String {
    match cfg!(windows) {
        true => "dir /b".to_string(),
        false => "ls".to_string(),
    }
}

/// Enough output that a subscriber reading one chunk at a time cannot keep up.
fn flood() -> String {
    match cfg!(windows) {
        true => "for /l %i in (1,1,400) do @echo line-%i & echo flood-finished".to_string(),
        false => "for i in $(seq 1 400); do echo line-$i; done; echo flood-finished".to_string(),
    }
}

/// A string `report_size` prints on the way to the numbers, so a test can wait
/// for the command to have run before reading them.
fn size_marker() -> &'static str {
    match cfg!(windows) {
        true => "Columns:",
        false => " ",
    }
}
