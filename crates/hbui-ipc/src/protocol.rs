//! The wire between an application and the bridge.
//!
//! One JSON value per line over a Unix socket. The application connects and
//! speaks first:
//!
//! ```text
//! app    → {"protocol":"hbui-ipc","version":1,"application":"commander","pid":4242}
//! bridge → {"accepted":true,"instance":27}
//! ```
//!
//! After that the bridge asks and the application answers, matched by `id`:
//!
//! ```text
//! bridge → {"id":1,"method":"get_view","params":{}}
//! app    → {"id":1,"result":{...}}          or {"id":1,"error":{...}}
//! ```
//!
//! and the application also pushes its view whenever the revision moves, so
//! the bridge always holds the last state it saw — including the one just
//! before a crash:
//!
//! ```text
//! app    → {"method":"view_changed","params":{"view":{...}}}
//! ```
//!
//! This is deliberately not MCP. MCP's transports, sessions and versions stay
//! in the bridge; an application only ever learns this.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: &str = "hbui-ipc";

/// Bumped on any incompatible change. The bridge refuses a mismatched
/// application by name (`protocol_mismatch`) rather than half-working with it.
pub const VERSION: u32 = 1;

/// The application's first line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: String,
    pub version: u32,
    pub application: String,
    pub pid: u32,
}

impl Hello {
    pub fn new(application: impl Into<String>) -> Self {
        Self {
            protocol: PROTOCOL.into(),
            version: VERSION,
            application: application.into(),
            pid: std::process::id(),
        }
    }
}

/// The bridge's answer to [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub accepted: bool,
    /// Which generation of the application this connection is. Assigned by
    /// the bridge, one higher on every accepted connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_version: Option<u32>,
}

/// Where the bridge listens and the application connects when neither is
/// told otherwise: `$HBUI_SOCKET`, or `hbui-dev-$USER.sock` in the temp dir.
///
/// Per user, because `/tmp` is shared on Linux and a second user's bridge
/// must not collide with — or be reachable by — the first's.
pub fn default_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("HBUI_SOCKET") {
        return PathBuf::from(p);
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("hbui-dev-{user}.sock"))
}

pub fn write_line(out: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

/// Read one JSON line. `None` at a clean end of stream.
pub fn read_line(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
