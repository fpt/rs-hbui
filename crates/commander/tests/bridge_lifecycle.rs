//! The success scenario of `docs/MCP_BRIDGE.md`, with sessions, end to end.
//!
//! Real `commander` processes are started, killed and restarted. A bridge
//! runs in this process — one `Server`, standing in for one MCP connection
//! that is never re-established — and a second bridge stands in for a second
//! agent watching the same applications.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hbui_ipc::Bridge;
use hbui_mcp_bridge::Server;
use serde_json::{json, Value};

/// A session directory and a directory of files, removed on drop. Short, for
/// the socket-path limit.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("hbui-lc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("files/docs")).unwrap();
        std::fs::write(p.join("files/notes.txt"), "hi").unwrap();
        Self(p)
    }

    fn sessions(&self) -> PathBuf {
        self.0.join("s")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A commander process, killed on drop so a failing test leaves none behind.
struct App(Child);

impl App {
    fn start(sessions: &Path, session: &str, dir: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_commander"))
            .env("HBUI_DIR", sessions)
            .args(["--headless", "--hbui-session", session])
            .arg(dir)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn kill(mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Call a tool the way an MCP client would, through JSON-RPC.
fn tool(server: &Server, name: &str, args: Value) -> (bool, Value) {
    let req = serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": name, "arguments": args},
    }))
    .unwrap();
    let resp = serde_json::to_value(server.handle(req).unwrap()).unwrap();
    assert!(resp.get("error").is_none(), "protocol error: {resp}");
    let result = &resp["result"];
    let text = result["content"][0]["text"].as_str().unwrap();
    (
        result["isError"] == true,
        serde_json::from_str(text).unwrap_or(json!(text)),
    )
}

fn sessions(server: &Server) -> Vec<Value> {
    tool(server, "list_sessions", json!({})).1["sessions"]
        .as_array()
        .unwrap()
        .clone()
}

fn status(server: &Server, session: &str) -> Value {
    sessions(server)
        .into_iter()
        .find(|s| s["session"] == session)
        .map_or(Value::Null, |s| s["status"].clone())
}

fn until(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn the_agent_outlives_the_application() {
    let scratch = Scratch::new();
    let files = scratch.0.join("files");
    let dir = scratch.sessions();

    // 1. The agent's connection: one bridge, one server, for the whole test.
    let bridge = Bridge::new(&dir);
    bridge.watch();
    let server = Server::new(bridge, "test");
    assert!(sessions(&server).is_empty());

    // 2. Two applications, each in its own session.
    let app = App::start(&dir, "main", &files);
    let _other = App::start(&dir, "other", &files);
    until("both sessions", || {
        status(&server, "main") == "ready" && status(&server, "other") == "ready"
    });
    let row = sessions(&server)
        .into_iter()
        .find(|s| s["session"] == "main")
        .unwrap();
    assert_eq!(row["application"], "commander");
    assert_eq!(row["pid"], app.pid());
    assert_eq!(row["instance"], 1);

    // 3. get_view, by session and by pid.
    let (_, view) = tool(&server, "get_view", json!({"session": "main"}));
    assert_eq!(
        (view["session"].clone(), view["instance"].clone()),
        (json!("main"), json!(1))
    );
    let (_, by_pid) = tool(&server, "get_view", json!({"pid": app.pid()}));
    assert_eq!(by_pid["session"], "main");
    let revision = view["revision"].clone();

    // 4. dispatch.
    let (err, out) = tool(
        &server,
        "dispatch",
        json!({"session": "main", "type": "select", "target": "left.files", "item": "notes.txt",
               "expected_instance": 1, "expected_revision": revision}),
    );
    assert!(!err, "{out}");
    assert_eq!(out["changes"][0]["path"], "/widgets/left.files/selected");

    // A second agent's bridge sees the same application, and the change.
    let second = Server::new(Bridge::new(&dir), "test");
    let (_, seen) = tool(&second, "get_view", json!({"session": "main"}));
    assert_eq!(seen["widgets"]["left.files"]["selected"], "notes.txt");

    // 5. The application goes away — killed, as a crash would. (First let
    // its pushes catch up: the second bridge attaching changed its status
    // line, so the revision may already be past `before`.)
    let before = out["revision"].as_u64().unwrap();
    until("the pushed view to catch up", || {
        sessions(&server)
            .into_iter()
            .any(|s| s["session"] == "main" && s["revision"].as_u64() >= Some(before))
    });
    app.kill();

    // 6-7. The MCP side still answers, says the application is gone, and
    // shows its last state marked stale. The other session is untouched.
    until("disconnect", || status(&server, "main") == "disconnected");
    assert_eq!(status(&server, "other"), "ready");
    let (err, down) = tool(&server, "get_view", json!({"session": "main"}));
    assert!(!err, "an absent application is not a tool failure");
    assert_eq!(down["status"], "application_disconnected");
    assert_eq!(down["stale"], true);
    assert_eq!(down["last_view"]["instance"], 1);
    assert_eq!(
        down["last_view"]["widgets"]["left.files"]["selected"],
        "notes.txt"
    );
    let (err, refused) = tool(
        &server,
        "dispatch",
        json!({"session": "main", "type": "focus", "target": "left.files"}),
    );
    assert!(err);
    assert_eq!(refused["error"], "application_disconnected");

    // 8-10. A new build starts under the same session and is found by
    // itself, as a new instance with a new pid.
    let app = App::start(&dir, "main", &files);
    until("second instance", || status(&server, "main") == "ready");
    let (_, view) = tool(&server, "get_view", json!({"session": "main"}));
    assert_eq!(view["instance"], 2);
    let row = sessions(&server)
        .into_iter()
        .find(|s| s["session"] == "main")
        .unwrap();
    assert_eq!(row["pid"], app.pid());

    // 11. Everything works again, on the same MCP connection.
    let (err, out) = tool(
        &server,
        "dispatch",
        json!({"session": "main", "type": "activate", "target": "left.files", "item": "docs",
               "expected_instance": 2, "expected_revision": view["revision"]}),
    );
    assert!(!err, "{out}");
    let (_, view) = tool(&server, "get_view", json!({"session": "main"}));
    assert!(
        view["layout"].to_string().contains("files/docs"),
        "the pane moved into docs: {}",
        view["layout"]
    );

    // 12. An action planned against the old instance is refused.
    let (err, stale) = tool(
        &server,
        "dispatch",
        json!({"session": "main", "type": "select", "target": "left.files", "item": "notes.txt",
               "expected_instance": 1, "expected_revision": before}),
    );
    assert!(err);
    assert_eq!(stale["error"], "stale_instance");
    assert_eq!(stale["current_instance"], 2);

    // A session name held by a live process cannot be taken twice.
    let taken = Command::new(env!("CARGO_BIN_EXE_commander"))
        .env("HBUI_DIR", &dir)
        .args(["--headless", "--hbui-session", "main"])
        .arg(&files)
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!taken.success());

    // 13. Nothing on the agent's side was restarted: every call above went
    // through the one `server` made in step 1.
}
