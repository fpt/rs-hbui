//! The local link between hbui applications and the bridges that serve them
//! to agents.
//!
//! ```text
//! agent A ──MCP── bridge A ──┐                  ┌── commander  (session "commander")
//!                            ├── <dir>/*.sock ──┤
//! agent B ──MCP── bridge B ──┘                  └── voxeler    (session "voxeler")
//! ```
//!
//! An application uses [`Endpoint`]: it claims a session — a name, and the
//! socket `<dir>/<session>.sock` — and serves every bridge that connects. A
//! bridge uses [`Bridge`]: it scans `<dir>`, connects to each application,
//! and forwards requests to the session they name.
//!
//! Three identities, kept apart:
//!
//! - **session** — the logical application, stable across restarts;
//! - **instance** — which run of that session, one higher on each restart;
//! - **pid** — the current process, a convenient selector and nothing more.
//!
//! Neither side knows anything about MCP. Unix only, for now.

pub mod app;
pub mod bridge;
pub mod protocol;

pub use app::{Capture, Endpoint, Link, Running};
pub use bridge::{Bridge, RequestError, SessionInfo, Target};
pub use protocol::{default_dir, socket_path, valid_session, Hello, Welcome, PROTOCOL, VERSION};

#[cfg(test)]
mod tests;
