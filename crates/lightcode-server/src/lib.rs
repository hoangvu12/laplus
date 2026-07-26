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
//! and [`server`] is the endpoint and the connection loop that ties them
//! together.

pub mod auth;
pub mod config;
pub mod config_store;
pub mod http;
pub mod protocol;
pub mod rpc;
pub mod server;
pub mod subscriptions;
pub mod wire;

pub use server::{Server, ServerState};
