//! laplus's server: the Rust half of "same UI, tenth of the weight".
//!
//! Agent and UI wire formats meet here, and keeping them apart is the crate's main
//! organising idea:
//!
//! - [`protocol`] is what the **agent** speaks — the `claude` CLI's stdio
//!   NDJSON, lifted out of the STEP 1 spike (`spike-claude-protocol/README.md`)
//!   once it had answered its question. Pinned by `fixtures/claude-cli/`.
//! - `codex_protocol` is the pure JSON-RPC subset spoken by `codex app-server`,
//!   pinned in both directions by `fixtures/codex-app-server/`; [`codex`] owns
//!   the child process, pipes and deadlines around it.
//! - [`opencode_protocol`] is the pure JSON/SSE subset spoken by OpenCode,
//!   pinned by `fixtures/opencode-http-sse/`; [`opencode`] owns HTTP requests
//!   and the event-stream lifetime around it.
//! - [`wire`] is what the **UI** speaks — JSON messages inside WebSocket text
//!   frames. Pinned by `fixtures/socket-wire/` and described in
//!   `docs/socket-wire-format.md`.
//!
//! The protocol modules are pure: parsing and types, no I/O. That is what keeps a format
//! change to a blast radius of one file, and what lets the golden-file tests
//! act as drift detectors without standing a server up.
//!
//! Around them: [`auth`] decides whether a socket upgrade may proceed,
//! [`config`] builds the `server.getConfig` payload the UI fetches before it
//! can do anything else and [`config_store`] holds the one in force plus the
//! changes to it, [`http`] holds the two plain HTTP answers the UI needs
//! before it will open a socket at all, [`rpc`] maps a method tag to an
//! answer and [`refusals`] says how it declines the ones it has no answer for
//! yet, [`subscriptions`] streams the answers that are not a single value,
//! [`filesystem`] enumerates names on disk for the folder picker, the file
//! tree and the search behind them, [`watcher`] tells it when something it has
//! already enumerated has changed underneath, [`files`] opens and saves what
//! those names point at, [`git`] says which of them have changed and keeps
//! saying it while the agent works, [`checkpoints`] records what the working
//! tree looked like at each turn boundary so that a turn and a whole
//! conversation can be read as diffs, [`refs`] lists the branches, moves between
//! them and makes the repository a project has not got yet, [`editor`] hands
//! one to the developer's own editor, [`provider`] finds the agent binary and
//! reports what it found, [`process`] is how all of those start a program and
//! where they look for one,
//! [`terminal`] runs a shell in a project's folder and pipes it to the pane the
//! developer typed into, and [`server`] is the endpoint and the connection loop
//! that ties them together.
//!
//! [`terminal`] is the one subsystem here that is deliberately *not* a
//! translation. The two formats above are parsed; a terminal's bytes are not
//! read at all, because the thing that reads them is the emulator in the UI.
//! The only exception is the copy kept for a client that reconnects, and the
//! module says why.
//!
//! The Claude and UI formats meet in exactly one place. [`agent`] runs the `claude`
//! subprocess, [`settling`] holds the contract's two lifecycle vocabularies and
//! the one rule that reads a session's status as how its turn went — the third
//! copy of a rule upstream keeps in both its server and its client, and the only
//! one this repository controls — [`threads`] holds a conversation as the UI reads one,
//! [`transcripts`] writes one down behind the stream that is producing it,
//! [`worklog`] says how a tool call and a pause to reason look to the UI, and
//! [`turn`] is the join: it folds what the agent said with [`protocol`] and
//! answers with what the UI reads. Keeping the join to one file is what makes
//! "the agent's format moved" and "the UI's contract moved" separate failures.
//!
//! [`session`] is the lifetime around that join, and it is deliberately on the
//! other side of it: a conversation's baselines, checkpoints, epochs, settling
//! and session events are written once, over the `session::Driver` trait, so a
//! second agent brings a protocol and an encoder rather than a second copy of
//! all of that. [`turn`] drives Claude and [`codex`] drives Codex app-server.

pub mod agent;
pub mod approval;
pub mod assets;
pub mod auth;
pub mod catalogue;
pub mod checkpoints;
pub mod clock;
pub mod codes;
pub mod codex;
pub mod codex_protocol;
pub mod config;
pub mod config_store;
pub mod editor;
pub mod endpoints;
pub mod files;
pub mod filesystem;
pub mod git;
pub mod http;
pub mod keybindings;
pub mod launch;
pub mod opencode;
pub mod opencode_protocol;
pub mod orchestration;
pub mod pairing;
pub mod process;
pub mod project_favicon;
pub mod projects;
pub mod protocol;
pub mod provider;
pub mod provider_maintenance;
pub mod qr;
pub mod refs;
pub mod refusals;
pub mod remote_access;
pub mod rpc;
pub mod server;
pub mod service;
pub mod session;
pub mod settings;
pub mod settling;
pub mod startup;
pub mod store;
pub mod subscriptions;
pub mod terminal;
pub mod threads;
pub mod transcripts;
pub mod turn;
pub mod ui;
pub mod version;
pub mod watcher;
pub mod wire;
pub mod worklog;

pub use server::{Server, ServerState, StartupFailure};
