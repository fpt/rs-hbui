//! `commander` — a two-pane file browser that a person and an agent can use
//! at the same time.
//!
//! ```text
//! commander [LEFT [RIGHT]]              the terminal UI
//! commander --mcp[=PORT] [LEFT [RIGHT]] ... also serving MCP on 127.0.0.1:PORT/mcp
//! commander mcp [LEFT [RIGHT]]          headless, MCP over stdio
//! ```

mod app;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hbui_core::{Session, Shared};
use hbui_mcp::Server;

const NAME: &str = "commander";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PORT: u16 = 8740;

const USAGE: &str = "\
usage: commander [--mcp[=PORT]] [LEFT [RIGHT]]
       commander mcp [LEFT [RIGHT]]

  --mcp[=PORT]  also serve MCP over HTTP on 127.0.0.1:PORT/mcp (default 8740)
  mcp           no terminal UI; serve MCP over stdio

keys: Tab switch pane, Enter open, F2 rename, F5 copy, F7 mkdir, ^R refresh, ^Q quit";

struct Args {
    stdio: bool,
    http: Option<u16>,
    dirs: Vec<PathBuf>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        stdio: false,
        http: None,
        dirs: Vec::new(),
    };
    for (i, arg) in args.enumerate() {
        match arg.as_str() {
            "mcp" if i == 0 => out.stdio = true,
            "--mcp" => out.http = Some(DEFAULT_PORT),
            "-h" | "--help" => return Err(String::new()),
            a if a.starts_with("--mcp=") => {
                let port = a["--mcp=".len()..]
                    .parse()
                    .map_err(|_| format!("bad port in {a}"))?;
                out.http = Some(port);
            }
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            a => out.dirs.push(PathBuf::from(a)),
        }
    }
    if out.dirs.len() > 2 {
        return Err("at most two directories".into());
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{NAME}: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(if e.is_empty() { 0 } else { 2 });
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolve = |p: Option<&PathBuf>| {
        let p = p.cloned().unwrap_or_else(|| cwd.clone());
        p.canonicalize().unwrap_or(p)
    };
    let left = resolve(args.dirs.first());
    let right = resolve(args.dirs.get(1).or(args.dirs.first()));

    let (ui, app) = app::build(left, right);
    let shared = Shared::new(Session::new(ui, app));
    let server = Server::new(shared.clone(), NAME, VERSION).with_capture(Arc::new(|ui, w, h| {
        hbui_terminal::render(ui, w, h).surface.to_text()
    }));

    if args.stdio {
        eprintln!("{NAME} {VERSION}: serving MCP on stdio");
        hbui_mcp::serve_stdio(&server);
        return ExitCode::SUCCESS;
    }

    if let Some(port) = args.http {
        match hbui_mcp::serve_http(Arc::new(server), port) {
            Ok(addr) => {
                // The terminal is about to be taken over, so the address goes
                // where the person will see it: the status line.
                shared.lock().update(|ui| {
                    ui.set_text(app::STATUS, format!("MCP: http://{addr}/mcp  ^Q: quit"))
                });
            }
            Err(e) => {
                eprintln!("{NAME}: cannot listen on 127.0.0.1:{port}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if let Err(e) = hbui_terminal::run(&shared) {
        eprintln!("{NAME}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
