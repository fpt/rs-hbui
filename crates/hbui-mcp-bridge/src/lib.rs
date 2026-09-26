//! hbui-mcp-bridge: an agent's MCP connection to every hbui application on
//! the machine.
//!
//! ```text
//! agent ──MCP (stdio)── hbui-mcp-bridge ──hbui-ipc── commander, voxeler, ...
//!                       (lives as long as the agent)  (restart at will)
//! ```
//!
//! The applications are being developed: they get rebuilt, restarted, and
//! sometimes crash. The agent's MCP connection must not go with them. So MCP
//! lives here and the applications only speak `hbui-ipc`. The bridge owns no
//! UI state; it finds applications, forwards to the session each call names,
//! labels every answer with its session and instance, and says plainly when
//! an application is not there.
//!
//! Stdio only: the agent's client starts the bridge and owns it. Several
//! agents each start their own, and all of them reach the same applications.

pub mod server;
pub mod stdio;
pub mod wire;

pub use server::Server;
pub use stdio::serve_stdio;

#[cfg(test)]
mod tests;
