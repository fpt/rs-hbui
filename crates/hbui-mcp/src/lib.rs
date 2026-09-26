//! The AI half of hbui: a session served over MCP.
//!
//! The agent gets the same state the person's screen is drawn from, as data,
//! and changes it with the same semantic actions a person's keys become. It
//! never sees pixels unless it asks for them (`capture_view`), and never
//! sends a key.
//!
//! Two transports: [`serve_stdio`] for a headless session an agent spawns for
//! itself, and [`serve_http`] for a session a person is also using in a
//! terminal.

pub mod http;
pub mod server;
pub mod stdio;
pub mod wire;

pub use http::serve_http;
pub use server::{Capture, Server};
pub use stdio::serve_stdio;

#[cfg(test)]
mod tests;
