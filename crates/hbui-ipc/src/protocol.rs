//! The wire between an application and a bridge.
//!
//! Every application listens on its own Unix socket, `<dir>/<session>.sock`,
//! and every bridge — one per agent — scans `<dir>` and connects to each. The
//! socket path is the logical session: an application restarted under the
//! same session name comes back on the same path, as a new instance.
//!
//! One JSON value per line. The application speaks first:
//!
//! ```text
//! app    → {"protocol":"hbui-ipc","version":2,"application":"commander",
//!           "session":"commander","instance":7,"pid":4242,"cwd":"/Users/me"}
//! bridge → {"accepted":true}
//! ```
//!
//! After that the bridge asks and the application answers, matched by `id`,
//! and the application pushes its view whenever the revision moves:
//!
//! ```text
//! bridge → {"id":1,"method":"get_view","params":{}}
//! app    → {"id":1,"result":{...}}          or {"id":1,"error":{...}}
//! app    → {"method":"view_changed","params":{"view":{...}}}
//! ```
//!
//! This is deliberately not MCP. MCP stays in the bridge; an application only
//! ever learns this.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL: &str = "hbui-ipc";

/// Bumped on any incompatible change. A bridge refuses a mismatched
/// application by name (`protocol_mismatch`) rather than half-working with it.
pub const VERSION: u32 = 2;

/// The application's first line on every connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: String,
    pub version: u32,
    pub application: String,
    /// The logical session: the socket's name, stable across restarts.
    pub session: String,
    /// Which run of this session this process is. Counted per session, so
    /// every bridge sees the same number for the same process.
    pub instance: u64,
    pub pid: u32,
    #[serde(default)]
    pub cwd: String,
}

/// The bridge's answer to [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_version: Option<u32>,
}

/// The directory sessions live in: `$HBUI_DIR`, or `hbui-$USER` in the temp
/// dir.
///
/// Per user, because `/tmp` is shared on Linux and one user's agent must not
/// find — let alone drive — another user's applications. Kept short, too: a
/// Unix socket path may not exceed about a hundred bytes.
pub fn default_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("HBUI_DIR") {
        return PathBuf::from(p);
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    std::env::temp_dir().join(format!("hbui-{user}"))
}

pub fn socket_path(dir: &Path, session: &str) -> PathBuf {
    dir.join(format!("{session}.sock"))
}

/// Session names become file names, so they are kept to a safe alphabet.
pub fn valid_session(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
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
