//! `hbui-mcp-bridge` — an MCP server (stdio) for every hbui application in
//! the session directory.
//!
//! ```text
//! hbui-mcp-bridge [--dir PATH]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use hbui_ipc::{default_dir, Bridge};
use hbui_mcp_bridge::{serve_stdio, Server};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
usage: hbui-mcp-bridge [--dir PATH]

Serves MCP on stdin/stdout, for an agent's client to start.

  --dir PATH  where applications put their session sockets (default: $HBUI_DIR,
              or hbui-$USER in the temp dir)";

fn main() -> ExitCode {
    let mut dir: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => match args.next() {
                Some(p) => dir = Some(p.into()),
                None => return usage("--dir needs a path"),
            },
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            a => return usage(&format!("unknown argument {a}")),
        }
    }

    let dir = dir.unwrap_or_else(default_dir);
    eprintln!("hbui-mcp-bridge {VERSION}: sessions in {}", dir.display());
    let bridge = Bridge::new(dir);
    bridge.watch();
    // The agent's client owns this process; when it closes stdin, we are done.
    serve_stdio(&Server::new(bridge, VERSION));
    ExitCode::SUCCESS
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("hbui-mcp-bridge: {problem}\n{USAGE}");
    ExitCode::from(2)
}
