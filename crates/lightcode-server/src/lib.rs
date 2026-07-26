//! lightcode's server: the Rust half of "same UI, tenth of the weight".
//!
//! Two wire formats meet here, and keeping them apart is the crate's main
//! organising idea:
//!
//! - [`protocol`] is what the **agent** speaks — the `claude` CLI's stdio
//!   NDJSON, lifted out of the STEP 1 spike (`spike-claude-protocol/README.md`)
//!   once it had answered its question. Pinned by `fixtures/claude-cli/`.
//! - [`wire`] is what the **UI** speaks — JSON messages inside WebSocket text
//!   frames. Pinned by `fixtures/socket-wire/` and described in
//!   `docs/socket-wire-format.md`.
//!
//! Both are pure: parsing and types, no I/O. That is what keeps a format
//! change to a blast radius of one file, and what lets the golden-file tests
//! act as drift detectors without standing a server up.
//!
//! Around them: [`auth`] decides whether a socket upgrade may proceed,
//! [`config`] builds the `server.getConfig` payload the UI fetches before it
//! can do anything else and [`config_store`] holds the one in force plus the
//! changes to it, [`http`] holds the two plain HTTP answers the UI needs
//! before it will open a socket at all, [`rpc`] maps a method tag to an
//! answer, [`subscriptions`] streams the answers that are not a single value,
//! [`filesystem`] enumerates names on disk for the folder picker, the file
//! tree and the search behind them, [`watcher`] tells it when something it has
//! already enumerated has changed underneath, [`files`] opens and saves what
//! those names point at, [`editor`] hands one to the developer's own editor,
//! [`provider`] finds the agent binary and reports what it found, [`process`]
//! is how all three of those start a program and where they look for one, and
//! [`server`] is the endpoint and the connection loop that ties them together.

pub mod auth;
pub mod config;
pub mod config_store;
pub mod editor;
pub mod files;
pub mod filesystem;
pub mod http;
pub mod orchestration;
pub mod process;
pub mod projects;
pub mod protocol;
pub mod provider;
pub mod rpc;
pub mod server;
pub mod store;
pub mod subscriptions;
pub mod watcher;
pub mod wire;

pub use server::{Server, ServerState, StartupFailure};
