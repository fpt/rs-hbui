//! The local link that lets an application restart without its agent
//! noticing more than a new instance id.
//!
//! ```text
//! agent ──MCP── hbui-mcp-bridge ──hbui-ipc── application ── hbui-core
//!              (long-lived)                 (short-lived)
//! ```
//!
//! The application uses [`Endpoint`]: it connects out to the bridge, keeps
//! retrying while there is none, and answers `get_view`, `dispatch` and
//! `capture_view` from its own [`hbui_core::Session`]. The bridge uses
//! [`Bridge`]: it listens, numbers each connection as a new instance, and
//! keeps the last pushed view for when the application is gone.
//!
//! Neither side knows anything about MCP. Unix only, for now: the link is a
//! Unix domain socket.

pub mod app;
pub mod bridge;
pub mod protocol;

pub use app::{Capture, Endpoint, Link};
pub use bridge::{Bridge, Mismatch, RequestError, Snapshot, Status};
pub use protocol::{default_socket, Hello, Welcome, PROTOCOL, VERSION};

#[cfg(test)]
mod tests;
