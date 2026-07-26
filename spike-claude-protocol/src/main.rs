//! THROWAWAY TUI SHELL — not production code.
//!
//! A thin harness over `protocol.rs` so the state model can be driven by hand.
//! It spawns the user's installed `claude` binary, streams its NDJSON stdio,
//! folds it through the reducer, and re-renders the whole frame after every
//! event. The reducer is the part worth keeping; this file is not.

mod protocol;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use protocol::{parse_line, SessionState};

enum Msg {
    /// A parsed protocol event, or a parse failure (drift signal).
    Proto(Box<Result<protocol::Event, ()>>),
    /// A raw line, kept for the `/raw` view.
    Raw(String),
    User(String),
    ChildEof,
    StdinEof,
}

fn main() {
    let mut child = match spawn_claude() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to spawn `claude`: {e}\nIs it on PATH?");
            std::process::exit(1);
        }
    };

    let mut child_stdin = child.stdin.take().expect("piped stdin");
    let child_stdout = child.stdout.take().expect("piped stdout");

    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();

    // Reader thread: child stdout -> parsed events.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(child_stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let _ = tx.send(Msg::Raw(line.clone()));
                let parsed = parse_line(&line).map_err(|_| ());
                if tx.send(Msg::Proto(Box::new(parsed))).is_err() {
                    break;
                }
            }
            let _ = tx.send(Msg::ChildEof);
        });
    }

    // Reader thread: our stdin -> user commands.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(Msg::User(line)).is_err() {
                    break;
                }
            }
            let _ = tx.send(Msg::StdinEof);
        });
    }

    let mut state = SessionState::new();
    let mut raw_log: Vec<String> = Vec::new();
    let mut show_raw = false;
    let mut status = String::from("ready — type a prompt and press Enter");
    let mut child_alive = true;

    render(&state, &raw_log, show_raw, &status, child_alive);

    for msg in rx {
        match msg {
            Msg::Raw(line) => {
                raw_log.push(line);
                if raw_log.len() > 200 {
                    raw_log.remove(0);
                }
                continue; // Raw always arrives paired with Proto; render on that.
            }
            Msg::Proto(result) => match *result {
                Ok(event) => state.reduce(event),
                Err(()) => state.note_parse_error(),
            },
            Msg::User(line) => {
                let line = line.trim().to_string();
                match line.as_str() {
                    "/q" | "/quit" => break,
                    "/raw" => {
                        show_raw = !show_raw;
                        status = format!("raw event log: {}", if show_raw { "on" } else { "off" });
                    }
                    "" => {}
                    _ => {
                        if !child_alive {
                            status = "child exited — /q to quit".into();
                        } else if let Err(e) = send_user(&mut child_stdin, &line) {
                            status = format!("send failed: {e}");
                        } else {
                            status = format!("sent: {line}");
                        }
                    }
                }
            }
            Msg::ChildEof => {
                child_alive = false;
                status = "claude exited (stdout closed) — /q to quit".into();
            }
            Msg::StdinEof => break,
        }
        render(&state, &raw_log, show_raw, &status, child_alive);
    }

    let _ = child.kill();
    let _ = child.wait();
    println!("\x1b[0m");
}

fn spawn_claude() -> std::io::Result<Child> {
    // The flags that constitute the protocol, confirmed against `claude --help`
    // and a live capture. `--verbose` is required alongside `--print` for
    // stream-json output.
    Command::new("claude")
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--verbose",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

fn send_user(stdin: &mut ChildStdin, text: &str) -> std::io::Result<()> {
    let line = protocol::user_message_line(text);
    stdin.write_all(line.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()
}

// ---------------------------------------------------------------------------
// Rendering — replace the frame, never append.
// ---------------------------------------------------------------------------

const B: &str = "\x1b[1m";
const D: &str = "\x1b[2m";
const R: &str = "\x1b[0m";

fn render(state: &SessionState, raw: &[String], show_raw: bool, status: &str, alive: bool) {
    print!("\x1b[2J\x1b[H");

    println!("{B}spike-claude-protocol{R} {D}— throwaway prototype; question: does the CLI's stdio protocol bend to Rust?{R}");
    println!("{D}{}{R}", "-".repeat(78));

    println!("{B}session{R}    {}", opt(&state.session_id));
    println!("{B}model{R}      {}", opt(&state.model));
    println!("{B}cwd{R}        {}", opt(&state.cwd));
    println!(
        "{B}perm mode{R}  {}   {B}tools{R} {}",
        opt(&state.permission_mode),
        state.tool_count
    );
    println!(
        "{B}child{R}      {}",
        if alive { "running" } else { "exited" }
    );

    println!();
    println!(
        "{B}transcript{R} {D}({} turns){R}",
        state.transcript.len()
    );
    let start = state.transcript.len().saturating_sub(6);
    for turn in &state.transcript[start..] {
        let flag = if turn.from_deltas {
            format!(" {D}(deltas matched buffered message){R}")
        } else {
            String::new()
        };
        println!("  {B}{}{R}: {}{}", turn.role, truncate(&turn.text, 60), flag);
    }

    println!();
    if state.streaming {
        println!("{B}streaming{R}  {}", truncate(state.visible_text(), 66));
    } else {
        println!("{D}streaming  (idle){R}");
    }

    if let Some(r) = &state.last_result {
        println!();
        println!(
            "{B}last result{R} stop={} turns={} {D}{}ms  ${:.4}  error={}{R}",
            r.stop_reason.clone().unwrap_or_else(|| "-".into()),
            r.num_turns.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
            r.duration_ms.unwrap_or(0),
            r.total_cost_usd.unwrap_or(0.0),
            r.is_error
        );
    }

    println!();
    println!(
        "{B}protocol drift{R} unknown events: {}   parse errors: {}",
        state.unknown_events, state.parse_errors
    );
    let counts: Vec<String> = state
        .counts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    println!("{D}{}{R}", truncate(&counts.join("  "), 76));

    if show_raw {
        println!();
        println!("{B}raw events{R} {D}(last 6){R}");
        let start = raw.len().saturating_sub(6);
        for line in &raw[start..] {
            println!("  {D}{}{R}", truncate(line, 74));
        }
    }

    println!();
    println!("{D}{}{R}", "-".repeat(78));
    println!("{D}status:{R} {status}");
    println!(
        "{B}<text>{R}{D} send prompt{R}   {B}/raw{R}{D} toggle raw log{R}   {B}/q{R}{D} quit{R}"
    );
    print!("> ");
    let _ = std::io::stdout().flush();
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "-".into())
}

fn truncate(s: &str, n: usize) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if one_line.chars().count() <= n {
        one_line
    } else {
        let head: String = one_line.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}
