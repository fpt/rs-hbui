//! `commander` — a two-pane file browser that a person and an agent can use
//! at the same time.
//!
//! The agent reaches it through `hbui-mcp-bridge`, never directly. The
//! commander claims a hbui session — a name, and a socket in the session
//! directory — which every bridge finds by itself. Restart it under the same
//! session and the agent sees the same session with a new instance.
//!
//! ```text
//! commander [--hbui-session NAME] [LEFT [RIGHT]]   the terminal UI
//! commander --headless [LEFT [RIGHT]]              no terminal; only agents drive it
//! commander --no-hbui [LEFT [RIGHT]]               the terminal UI, invisible to agents
//! ```

mod app;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock};

use hbui_core::{Session, Shared};
use hbui_ipc::{Endpoint, Link};

const NAME: &str = "commander";

const USAGE: &str = "\
usage: commander [--hbui-session NAME | --no-hbui] [--headless] [LEFT [RIGHT]]

  --hbui-session NAME  the session agents address this as (default: commander,
                       or commander-2, ... if that is taken). Sessions live in
                       $HBUI_DIR, or hbui-$USER in the temp dir.
  --no-hbui            do not open a session; no agent can reach this
  --headless           no terminal UI; run until killed, driven only by agents

keys: Tab switch pane, Enter open, F2 rename, F5 copy, F7 mkdir, ^R refresh, ^Q quit";

struct Args {
    session: Option<String>,
    hbui: bool,
    headless: bool,
    dirs: Vec<PathBuf>,
}

fn parse(args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        session: None,
        hbui: true,
        headless: false,
        dirs: Vec::new(),
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hbui-session" => {
                out.session = Some(args.next().ok_or("--hbui-session needs a name")?)
            }
            "--no-hbui" => out.hbui = false,
            "--headless" => out.headless = true,
            "-h" | "--help" => return Err(String::new()),
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            a => out.dirs.push(PathBuf::from(a)),
        }
    }
    if out.dirs.len() > 2 {
        return Err("at most two directories".into());
    }
    if out.headless && !out.hbui {
        return Err("--headless with --no-hbui would be driven by nobody".into());
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

    // Held to the end of `main`: dropping it removes the session's socket.
    let mut _running = None;
    if args.hbui {
        let label: Arc<OnceLock<String>> = Arc::default();
        let status = shared.clone();
        let headless = args.headless;
        let shown = label.clone();
        let say = move |text: String| {
            if headless {
                eprintln!("{NAME}: {text}");
            }
            // The terminal belongs to the UI, so the person reads this in the
            // status line rather than on stderr.
            status
                .lock()
                .update(|ui| ui.set_text(app::STATUS, format!("{text}  ^Q: quit")));
        };
        let say = Arc::new(say);
        let on_link = say.clone();
        let mut endpoint = Endpoint::new(shared.clone(), NAME)
            .with_capture(Arc::new(|ui, w, h| {
                hbui_terminal::render(ui, w, h).surface.to_text()
            }))
            .on_link(move |link| {
                let who = shown.get().cloned().unwrap_or_default();
                on_link(match link {
                    Link::Bridges { count: 1 } => format!("{who}, 1 agent"),
                    Link::Bridges { count } => format!("{who}, {count} agents"),
                    Link::Rejected { reason } => format!("{who}, {reason}"),
                })
            });
        if let Some(name) = &args.session {
            endpoint = endpoint.session(name);
        }
        match endpoint.start() {
            Ok(running) => {
                let _ = label.set(format!(
                    "hbui: session {} (instance {})",
                    running.session, running.instance
                ));
                say(label.get().cloned().unwrap_or_default());
                _running = Some(running);
            }
            // Asked for a name and could not have it: say so and stop,
            // rather than run under a name nobody asked for.
            Err(e) if args.session.is_some() || args.headless => {
                eprintln!("{NAME}: {e}");
                return ExitCode::FAILURE;
            }
            Err(e) => say(format!("hbui: unavailable ({e})")),
        }
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
