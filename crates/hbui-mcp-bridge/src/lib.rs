//! hbui-mcp-bridge: the long-lived end of an agent's connection to a hbui
//! application.
//!
//! ```text
//! agent ──MCP── hbui-mcp-bridge ──hbui-ipc── application
//!              (long-lived)                 (short-lived)
//! ```
//!
//! The application is being developed: it gets rebuilt, restarted, and
//! sometimes crashes. The agent's MCP connection must not go with it. So MCP
//! lives here — transports, initialization, protocol versions — and the
//! application only speaks `hbui-ipc`. The bridge owns no UI state; it
//! forwards, labels each answer with the application instance it came from,
//! and says plainly when there is no application to ask.

pub mod http;
pub mod server;
pub mod stdio;
pub mod wire;

pub use http::serve_http;
pub use server::Server;
pub use stdio::serve_stdio;

#[cfg(test)]
mod tests;
