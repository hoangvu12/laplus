//! A terminal that outlives the pane looking at it, and the three ways a
//! developer ends one.
//!
//! Ticket 17 proved a terminal works while somebody is watching it. This file
//! is the other half: that it keeps working when nobody is, that it comes back
//! the way it was left, and that when it is finally closed nothing of it is
//! still running.
//!
//! ## Detaching is an unsubscribe, and that is the point
//!
//! There is no `terminal.detach` on this wire. Navigating away from a pane
//! cancels the `terminal.attach` subscription and touches nothing else, so what
//! these tests do is exactly what the app does — `Interrupt`, then a fresh
//! `terminal.attach` when the developer comes back. A server that reaped a
//! terminal when its last subscriber left would pass every test in ticket 17
//! and lose the developer's build.
//!
//! ## The evidence for reaping is outside the terminal
//!
//! "Closing a terminal terminates and reaps its process and child processes"
//! cannot be asserted from the terminal, because the terminal is the thing
//! being closed. So the child leaves its evidence on disk — a file it appends
//! to once a second — and the assertion is that the file **stops growing**.
//! That is a fact about the operating system rather than about this server's
//! own bookkeeping, which is what makes it worth the seconds it costs.
//!
//! Everything else is the gauges the rest of the suite uses:
//! `live_terminals` for shells and `live_subscriptions` for attachments, both
//! of which have to reach zero.

mod harness;

use std::time::Duration;

use harness::terminal::{
    echo, endless_child, length, quit, report_size, reported_size, shell_choice, slow_work, Pane,
    TICK,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

const THREAD: &str = "thread-1";
const TERMINAL: &str = "term-1";
const SECOND_TERMINAL: &str = "term-2";

/// Navigating away and back is the same terminal, with everything it said still
/// on it.
///
/// The first three acceptance lines of ticket 18 minus the slow one, and they
/// are one test because they are one claim: the terminal did not notice.
#[tokio::test]
async fn navigating_away_and_back_reattaches_to_the_same_shell_and_its_scrollback() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let opened = open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("said-before-leaving")).await;
    pane.wait_for(&mut client, "said-before-leaving").await;

    // Away. The subscription is gone and the shell is not, which is the whole
    // distinction this ticket rests on.
    pane.detach(&mut client).await;
    server.await_live_subscriptions(0).await;
    assert_eq!(server.live_terminals(), 1);

    // …and back.
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    let snapshot = pane.snapshots.first().expect("a reattachment describes it");
    assert_eq!(snapshot["status"], "running");
    assert_eq!(
        snapshot["pid"], opened["pid"],
        "reattaching started a second shell instead of finding the first"
    );
    assert!(
        pane.text().contains("said-before-leaving"),
        "the scrollback from before detaching did not come back:\n{}",
        pane.text()
    );

    // And it is a terminal rather than a transcript of one: it still takes
    // commands, and the shell answering is the one that was there all along.
    pane.run(&mut client, &echo("said-after-returning")).await;
    pane.wait_for(&mut client, "said-after-returning").await;

    client.close().await;
    server.stop().await;
}

/// A command that is still running when the pane goes away finishes anyway, and
/// what it printed is there when the developer comes back.
///
/// The acceptance this ticket exists for — "a long-running process is never
/// lost by clicking elsewhere". The marker is deliberately not on the screen
/// when the pane detaches, so its presence afterwards is evidence the shell
/// kept working with nothing subscribed rather than evidence of a replay.
#[tokio::test]
async fn a_process_that_outlives_the_pane_keeps_running_and_its_output_is_kept() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;

    pane.run(&mut client, &slow_work("finished-while-away", 3))
        .await;
    assert!(
        !pane.text().contains("finished-while-away"),
        "the work had already finished, so this proves nothing about detaching"
    );
    pane.detach(&mut client).await;
    server.await_live_subscriptions(0).await;

    // Nothing is listening, and the shell is working. Waited out rather than
    // polled, because there is no subscription left to hear anything on.
    tokio::time::sleep(TICK * 5).await;

    let pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    assert!(
        pane.text().contains("finished-while-away"),
        "work done while detached was lost:\n{}",
        pane.text()
    );

    client.close().await;
    server.stop().await;
}

/// A cleared terminal forgets what it showed and keeps the shell that showed
/// it.
#[tokio::test]
async fn a_cleared_terminal_forgets_what_it_showed_and_keeps_its_shell() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let opened = open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("before-the-clear")).await;
    pane.wait_for(&mut client, "before-the-clear").await;

    pane.clear(&mut client).await.expect_success();
    let cleared = pane.wait_for_notice(&mut client, "cleared").await;
    assert_eq!(cleared["threadId"], THREAD);
    assert_eq!(cleared["terminalId"], TERMINAL);

    // The shell is untouched: same process, and it still answers.
    assert_eq!(server.live_terminals(), 1);
    pane.run(&mut client, &echo("after-the-clear")).await;
    pane.wait_for(&mut client, "after-the-clear").await;
    assert!(
        !pane.text().contains("before-the-clear"),
        "what was cleared came back:\n{}",
        pane.text()
    );

    // …and a pane arriving afterwards is not sent it either, which is the half
    // that would go wrong if only the event had been published.
    let arriving = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    let snapshot = arriving.snapshots.first().expect("a description");
    assert_eq!(snapshot["pid"], opened["pid"]);
    assert!(
        !arriving.text().contains("before-the-clear"),
        "a later attachment was given the cleared scrollback:\n{}",
        arriving.text()
    );

    client.close().await;
    server.stop().await;
}

/// Restarting replaces the shell in the pane, and the pane is the same pane.
///
/// Three things at once, and the third is the one an implementation gets wrong:
/// a *new* process, an *empty* scrollback, and only *one* terminal — a restart
/// that opened a second shell without reaping the first would satisfy the first
/// two and leak the process.
#[tokio::test]
async fn a_restarted_terminal_gets_a_new_shell_in_the_same_pane() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let opened = open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("said-by-the-first-shell")).await;
    pane.wait_for(&mut client, "said-by-the-first-shell").await;

    let restarted = pane
        .restart(&mut client, workspace.path(), 120, 30)
        .await
        .expect_success();
    assert_eq!(restarted["status"], "running");
    assert_ne!(
        restarted["pid"], opened["pid"],
        "the restart handed back the shell it was asked to replace"
    );
    server.await_live_terminals(1).await;

    // The pane that was already attached is told, and told with a snapshot —
    // which is what lets it replace its buffer rather than append to it.
    let announced = pane.wait_for_notice(&mut client, "restarted").await;
    assert_eq!(announced["snapshot"]["pid"], restarted["pid"]);
    // Emptiness is asserted on *this* snapshot rather than on the call's own
    // answer, and the difference is not pedantry: this one is taken under the
    // lock, before the reader thread exists, so it cannot contain a byte of the
    // new shell. The call's answer is taken afterwards and by then the shell
    // has usually said something, so the same assertion there would fail
    // whenever the machine was quick.
    assert_eq!(
        announced["snapshot"]["history"], "",
        "the new shell inherited a screen"
    );
    assert_eq!(
        announced["snapshot"]["sequence"], announced["sequence"],
        "the snapshot and the event that carried it must agree, or a \
         reattachment will replay what this already delivered"
    );
    assert!(
        !pane.text().contains("said-by-the-first-shell"),
        "the replaced shell's output survived the restart:\n{}",
        pane.text()
    );

    // …and the new shell is a shell.
    pane.run(&mut client, &echo("said-by-the-second-shell")).await;
    pane.wait_for(&mut client, "said-by-the-second-shell").await;

    client.close().await;
    server.stop().await;
}

/// A call that says nothing about the size leaves the size alone.
///
/// The contract makes `cols`/`rows` optional on an open and an attach, and the
/// pane that sends one of those is not always the pane that opened the
/// terminal — so a missing size read as *the default* rather than as "leave it"
/// shrinks a terminal somebody else is working in, on traffic the UI sends every
/// time a second client mounts a pane. Asserted the only honest way: by asking
/// the shell how big it thinks it is.
#[tokio::test]
async fn a_call_that_names_no_size_does_not_resize_the_terminal() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open_at(&mut client, &workspace, 97, 41).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;

    // A second open and a restart, neither carrying a size. The restart also
    // replaces the shell, so what it must preserve is the *terminal's* size
    // rather than anything the old process knew.
    for tag in ["terminal.open", "terminal.restart"] {
        client
            .call(
                tag,
                json!({
                    "threadId": THREAD,
                    "terminalId": TERMINAL,
                    "cwd": workspace.cwd(),
                    "env": shell_choice(),
                }),
            )
            .await
            .expect_success();
    }

    // Asked, and then asked for a marker that arrives whatever the answer was —
    // so a terminal that *was* resized fails with the size it reported rather
    // than by waiting for a number that is never coming.
    let before = pane.text().len();
    pane.run(&mut client, &report_size()).await;
    pane.run(&mut client, &echo("size-reported")).await;
    pane.wait_for(&mut client, "size-reported").await;
    assert!(
        reported_size(&pane.text()[before..], 97, 41),
        "the terminal was resized by a call that named no size:\n{}",
        &pane.text()[before..]
    );

    client.close().await;
    server.stop().await;
}

/// A terminal whose shell has gone gets one back — but only because the client
/// asked for one by name.
///
/// `restartIfNotRunning` is the mobile client's way of saying "I am arriving at
/// a terminal I expect to be able to type in". The default is the interesting
/// half: an attach is a read, and one that quietly replaced a working shell
/// would lose whatever was running in it.
#[tokio::test]
async fn an_attach_puts_a_shell_back_only_when_it_asked_to() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let opened = open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &quit()).await;
    pane.wait_for_exit(&mut client).await;
    server.await_live_terminals(0).await;

    // An ordinary reattachment finds it as it stands: exited, and still
    // readable.
    let plain = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    assert_eq!(plain.snapshots[0]["status"], "exited");
    assert_eq!(server.live_terminals(), 0);
    plain.detach(&mut client).await;

    let asking = {
        let mut asking = Pane::opening(workspace.path(), 120, 30);
        asking["restartIfNotRunning"] = json!(true);
        asking
    };
    let mut revived = Pane::attach(&mut client, THREAD, TERMINAL, asking.clone()).await;
    assert_eq!(revived.snapshots[0]["status"], "running");
    assert_ne!(revived.snapshots[0]["pid"], opened["pid"]);
    server.await_live_terminals(1).await;
    revived.run(&mut client, &echo("running-again")).await;
    revived.wait_for(&mut client, "running-again").await;

    // …and asking again, of a terminal that is now running, changes nothing.
    let running = Pane::attach(&mut client, THREAD, TERMINAL, asking).await;
    assert_eq!(
        running.snapshots[0]["pid"], revived.snapshots[0]["pid"],
        "a terminal that was already running was restarted out from under itself"
    );

    client.close().await;
    server.stop().await;
}

/// Closing a terminal ends it: the shell is reaped, the pane is told, and the
/// list stops carrying it.
#[tokio::test]
async fn closing_a_terminal_reaps_its_shell_and_takes_it_off_the_list() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listing = client
        .subscribe("subscribeTerminalMetadata", json!({}))
        .await;
    client.next_event(&listing).await;

    open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;
    assert_eq!(client.next_event(&listing).await["type"], "upsert");

    pane.close(&mut client).await.expect_success();
    assert_eq!(server.live_terminals(), 0);

    let closed = pane.wait_for_notice(&mut client, "closed").await;
    assert_eq!(closed["threadId"], THREAD);
    assert_eq!(closed["terminalId"], TERMINAL);

    // The shell's *own* exit is the evidence it was reaped rather than
    // forgotten, and it is the only evidence there is: the gauge above counts
    // what the registry holds, which a close empties whether or not it killed
    // anything. This event is published by the reaper, after the process has
    // gone and the threads reading and writing its pty have been joined — so
    // its arrival, before the `closed`, is the whole promise of this call.
    let exited = pane.wait_for_notice(&mut client, "exited").await;
    assert!(exited["exitCode"].is_i64() || exited["exitCode"].is_null());
    let order = |kind: &str| {
        pane.notices
            .iter()
            .position(|notice| notice["type"] == kind)
            .unwrap_or_else(|| panic!("a {kind} notice"))
    };
    assert!(
        order("exited") < order("closed"),
        "the terminal was reported closed before the shell in it had gone:\n{:#?}",
        pane.notices
    );

    // The list is told separately, because a client watching the list rather
    // than the terminal has no other way to learn the tab has gone.
    let removed = client
        .values_until(&listing, |value| value["type"] == "remove")
        .await;
    let removed = removed.last().expect("a removal");
    assert_eq!(removed["threadId"], THREAD);
    assert_eq!(removed["terminalId"], TERMINAL);

    // And it is gone rather than idle: naming it now is a lookup failure.
    pane.type_in(&mut client, "still there?\r")
        .await
        .expect_declared("TerminalSessionLookupError");

    client.close().await;
    server.stop().await;
}

/// A close with no terminal named ends every terminal on the thread. What the
/// client sends when a whole conversation goes away rather than one pane.
#[tokio::test]
async fn closing_a_thread_closes_every_terminal_on_it() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for terminal in [TERMINAL, SECOND_TERMINAL] {
        open_named(&mut client, &workspace, terminal).await;
    }
    // A terminal on another thread, which must survive: the call names a
    // thread, and a server that read it as "everything" would take the
    // developer's other conversation with it.
    open_on(&mut client, &workspace, "thread-2", TERMINAL).await;
    server.await_live_terminals(3).await;

    client
        .call("terminal.close", json!({"threadId": THREAD}))
        .await
        .expect_success();
    assert_eq!(server.live_terminals(), 1);

    let survivor = client
        .call(
            "terminal.write",
            json!({"threadId": "thread-2", "terminalId": TERMINAL, "data": "\r"}),
        )
        .await;
    survivor.expect_success();

    client.close().await;
    server.stop().await;
}

/// Closing a terminal takes the processes running *inside* it with it.
///
/// The acceptance line no gauge can answer, because the evidence is a process
/// this server never had a handle on: the shell started it, and closing the
/// terminal has to reach it anyway. So it leaves its trace on disk and the
/// assertion is that the trace stops.
#[tokio::test]
async fn closing_a_terminal_reaps_the_child_processes_running_in_it() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    open(&mut client, &workspace).await;
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;

    let log = workspace.path().join("child.log");
    pane.run(&mut client, &endless_child(workspace.path(), "child.log"))
        .await;
    // Waited for rather than assumed: a child that had not started yet would
    // make the assertion below true for the wrong reason.
    let running = await_growth(&log, Duration::from_secs(30)).await;
    assert!(running, "the child never started, so nothing was reaped");

    pane.close(&mut client).await.expect_success();
    assert_eq!(server.live_terminals(), 0);

    // Long enough that a child still ticking would certainly have ticked.
    tokio::time::sleep(TICK).await;
    let after = length(&log);
    tokio::time::sleep(TICK * 4).await;
    assert_eq!(
        length(&log),
        after,
        "a process started inside the terminal outlived it"
    );

    client.close().await;
    server.stop().await;
}

/// Stopping the server reaps the terminals of every thread on it, not only the
/// ones somebody was watching.
///
/// Ticket 17 asserted this for a single attached terminal. What is added here is
/// the plural and the detached: a server that only reaped what had a subscriber
/// would leave a shell per abandoned pane.
#[tokio::test]
async fn stopping_the_server_reaps_the_terminals_of_every_thread() {
    let workspace = Workspace::with(&[]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let log = workspace.path().join("child.log");
    for (thread, terminal) in [(THREAD, TERMINAL), (THREAD, SECOND_TERMINAL), ("thread-2", TERMINAL)] {
        open_on(&mut client, &workspace, thread, terminal).await;
    }
    server.await_live_terminals(3).await;

    // One of them is running something, and nothing is attached to any of them.
    let mut pane = Pane::attach(&mut client, THREAD, TERMINAL, json!({})).await;
    pane.run(&mut client, &echo("ready")).await;
    pane.wait_for(&mut client, "ready").await;
    pane.run(&mut client, &endless_child(workspace.path(), "child.log"))
        .await;
    assert!(
        await_growth(&log, Duration::from_secs(30)).await,
        "the child never started, so nothing was reaped"
    );
    pane.detach(&mut client).await;

    client.abandon();
    server.stop().await;

    tokio::time::sleep(TICK).await;
    let after = length(&log);
    tokio::time::sleep(TICK * 4).await;
    assert_eq!(
        length(&log),
        after,
        "stopping the server left a process running in a terminal"
    );
}

/// `terminal.open` as the drawer sends it.
async fn open(client: &mut SocketClient, workspace: &Workspace) -> Value {
    open_named(client, workspace, TERMINAL).await
}

async fn open_named(client: &mut SocketClient, workspace: &Workspace, terminal: &str) -> Value {
    open_on(client, workspace, THREAD, terminal).await
}

async fn open_on(
    client: &mut SocketClient,
    workspace: &Workspace,
    thread: &str,
    terminal: &str,
) -> Value {
    open_full(client, workspace, thread, terminal, 120, 30).await
}

async fn open_at(
    client: &mut SocketClient,
    workspace: &Workspace,
    cols: u64,
    rows: u64,
) -> Value {
    open_full(client, workspace, THREAD, TERMINAL, cols, rows).await
}

async fn open_full(
    client: &mut SocketClient,
    workspace: &Workspace,
    thread: &str,
    terminal: &str,
    cols: u64,
    rows: u64,
) -> Value {
    client
        .call(
            "terminal.open",
            json!({
                "threadId": thread,
                "terminalId": terminal,
                "cwd": workspace.cwd(),
                "cols": cols,
                "rows": rows,
                "env": shell_choice(),
            }),
        )
        .await
        .expect_success()
}

/// Wait until `path` has grown twice, which is the only honest way to know a
/// process is *still* running rather than to know it once ran.
async fn await_growth(path: &std::path::Path, patience: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + patience;
    let mut seen = length(path);
    let mut growths = 0;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(TICK / 4).await;
        let now = length(path);
        if now > seen {
            seen = now;
            growths += 1;
            if growths == 2 {
                return true;
            }
        }
    }
    false
}
