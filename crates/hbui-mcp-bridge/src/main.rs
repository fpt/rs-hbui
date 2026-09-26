//! `hbui-mcp-bridge` — keep an agent connected while the application under
//! development restarts.
//!
//! ```text
//! hbui-mcp-bridge [--socket PATH]                 MCP over stdio
//! hbui-mcp-bridge [--socket PATH] --http[=PORT]   MCP on 127.0.0.1:PORT/mcp
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hbui_ipc::{default_socket, Bridge};
use hbui_mcp_bridge::{serve_http, serve_stdio, Server};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PORT: u16 = 8740;

const USAGE: &str = "\
usage: hbui-mcp-bridge [--socket PATH] [--http[=PORT]]

  --socket PATH  where applications connect (default: $HBUI_SOCKET, or
                 hbui-dev-$USER.sock in the temp dir)
  --http[=PORT]  serve MCP on 127.0.0.1:PORT/mcp (default 8740) instead of stdio";

fn main() -> ExitCode {
    let mut socket: Option<PathBuf> = None;
    let mut http: Option<u16> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => match args.next() {
                Some(p) => socket = Some(p.into()),
                None => return usage("--socket needs a path"),
            },
            "--http" => http = Some(DEFAULT_PORT),
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            a if a.starts_with("--http=") => match a["--http=".len()..].parse() {
                Ok(port) => http = Some(port),
                Err(_) => return usage(&format!("bad port in {a}")),
            },
            a => return usage(&format!("unknown argument {a}")),
        }
    }

    let socket = socket.unwrap_or_else(default_socket);
    let bridge = match Bridge::listen(&socket) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "hbui-mcp-bridge: cannot listen on {}: {e}",
                socket.display()
            );
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "hbui-mcp-bridge {VERSION}: applications connect to {}",
        socket.display()
    );
    let server = Arc::new(Server::new(bridge, VERSION));

    match http {
        None => {
            // The agent's client owns this process; when it closes stdin, the
            // session is over, and the socket goes with it.
            serve_stdio(&server);
            let _ = std::fs::remove_file(&socket);
        }
        Some(port) => match serve_http(server.clone(), port) {
            Ok(addr) => {
                eprintln!("hbui-mcp-bridge: MCP on http://{addr}/mcp");
                loop {
                    std::thread::park();
                }
            }
            Err(e) => {
                eprintln!("hbui-mcp-bridge: cannot listen on 127.0.0.1:{port}: {e}");
                return ExitCode::FAILURE;
            }
        },
    }
    ExitCode::SUCCESS
}

fn usage(problem: &str) -> ExitCode {
    eprintln!("hbui-mcp-bridge: {problem}\n{USAGE}");
    ExitCode::from(2)
}
