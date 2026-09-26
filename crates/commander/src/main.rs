//! `commander` — a two-pane file browser that a person and an agent can use
//! at the same time.
//!
//! The agent reaches it through `hbui-mcp-bridge`, never directly: the
//! commander connects out to the bridge's socket (and keeps retrying while
//! there is none), so it can be rebuilt and restarted while the agent stays
//! connected to the bridge.
//!
//! ```text
//! commander [--socket PATH] [LEFT [RIGHT]]   the terminal UI
//! commander --headless [LEFT [RIGHT]]        no terminal; only the agent drives it
//! commander --no-bridge [LEFT [RIGHT]]       the terminal UI, not reachable by an agent
//! ```

mod app;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hbui_core::{Session, Shared};
use hbui_ipc::{default_socket, Endpoint, Link};

const NAME: &str = "commander";

const USAGE: &str = "\
usage: commander [--socket PATH | --no-bridge] [--headless] [LEFT [RIGHT]]

  --socket PATH  the hbui-mcp-bridge socket to connect to (default: $HBUI_SOCKET,
                 or hbui-dev-$USER.sock in the temp dir)
  --no-bridge    do not connect to a bridge
  --headless     no terminal UI; run until killed, driven only through the bridge

keys: Tab switch pane, Enter open, F2 rename, F5 copy, F7 mkdir, ^R refresh, ^Q quit";

struct Args {
    socket: Option<PathBuf>,
    bridge: bool,
    headless: bool,
    dirs: Vec<PathBuf>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        socket: None,
        bridge: true,
        headless: false,
        dirs: Vec::new(),
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => out.socket = Some(args.next().ok_or("--socket needs a path")?.into()),
            "--no-bridge" => out.bridge = false,
            "--headless" => out.headless = true,
            "-h" | "--help" => return Err(String::new()),
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            a => out.dirs.push(PathBuf::from(a)),
        }
    }
    if out.dirs.len() > 2 {
        return Err("at most two directories".into());
    }
    if out.headless && !out.bridge {
        return Err("--headless with --no-bridge would be driven by nobody".into());
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

    if args.bridge {
        let socket = args.socket.unwrap_or_else(default_socket);
        let status = shared.clone();
        let headless = args.headless;
        Endpoint::new(shared.clone(), &socket, NAME)
            .with_capture(Arc::new(|ui, w, h| {
                hbui_terminal::render(ui, w, h).surface.to_text()
            }))
            .on_link(move |link| {
                let text = match link {
                    Link::Connected { instance } => {
                        format!("bridge: connected, instance {instance}")
                    }
                    Link::Disconnected => "bridge: waiting".to_string(),
                    Link::Rejected { reason } => format!("bridge: {reason}"),
                };
                if headless {
                    eprintln!("{NAME}: {text}");
                }
                // The terminal belongs to the UI, so the person reads this in
                // the status line rather than on stderr.
                status
                    .lock()
                    .update(|ui| ui.set_text(app::STATUS, format!("{text}  ^Q: quit")));
            })
            .spawn();
    }

    if args.headless {
        eprintln!("{NAME}: headless (pid {})", std::process::id());
        loop {
            std::thread::park();
        }
    }

    if let Err(e) = hbui_terminal::run(&shared) {
        eprintln!("{NAME}: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
