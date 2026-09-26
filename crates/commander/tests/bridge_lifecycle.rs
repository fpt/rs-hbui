//! The success scenario of `docs/MCP_BRIDGE.md`, end to end.
//!
//! A real bridge runs in this process — one `Server`, standing in for one
//! MCP connection that is never re-established — while real `commander`
//! processes are started, killed and restarted behind it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hbui_ipc::Bridge;
use hbui_mcp_bridge::Server;
use serde_json::{json, Value};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("hbui-lifecycle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("files/docs")).unwrap();
        std::fs::write(p.join("files/notes.txt"), "hi").unwrap();
        Self(p)
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
    fn start(socket: &Path, dir: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_commander"))
            .arg("--headless")
            .arg("--socket")
            .arg(socket)
            .arg(dir)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
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
    let dir = scratch.0.join("files");
    let socket = scratch.0.join("bridge.sock");

    // 1. The agent's connection: one bridge, one server, for the whole test.
    let server = Server::new(Bridge::listen(&socket).unwrap(), "test");
    let status = |s: &Server| tool(s, "get_view", json!({})).1["status"].clone();
    assert_eq!(status(&server), "application_unavailable");

    // 2. The application connects.
    let app = App::start(&socket, &dir);
    until("first instance", || status(&server) == "ready");

    // 3. get_view.
    let (_, view) = tool(&server, "get_view", json!({}));
    let instance = view["instance"].as_u64().unwrap();
    let revision = view["revision"].as_u64().unwrap();
    assert_eq!(instance, 1);
    let items: Vec<&str> = view["widgets"]["left.files"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert!(items.contains(&"notes.txt"), "{items:?}");

    // 4. dispatch.
    let (err, out) = tool(
        &server,
        "dispatch",
        json!({"type": "select", "target": "left.files", "item": "notes.txt",
               "expected_instance": instance, "expected_revision": revision}),
    );
    assert!(!err, "{out}");
    assert_eq!(out["instance"], 1);
    assert_eq!(out["changes"][0]["path"], "/widgets/left.files/selected");

    // 5. The application goes away — killed, as a crash would.
    let before = out["revision"].as_u64().unwrap();
    until("the pushed view to catch up", || {
        server
            .bridge()
            .status()
            .snapshot
            .is_some_and(|s| s.view["revision"] == before)
    });
    app.kill();

    // 6-7. The MCP side still answers, and says the application is gone,
    // with its last state marked stale.
    until("disconnect", || {
        status(&server) == "application_disconnected"
    });
    let (err, down) = tool(&server, "get_view", json!({}));
    assert!(!err, "an absent application is not a tool failure");
    assert_eq!(down["stale"], true);
    assert_eq!(down["last_view"]["instance"], 1);
    assert_eq!(
        down["last_view"]["widgets"]["left.files"]["selected"],
        "notes.txt"
    );
    let (err, refused) = tool(
        &server,
        "dispatch",
        json!({"type": "focus", "target": "left.files"}),
    );
    assert!(err);
    assert_eq!(refused["error"], "application_disconnected");

    // 8-10. A new build starts, reconnects by itself, as a new instance.
    let _app = App::start(&socket, &dir);
    until("second instance", || status(&server) == "ready");
    let (_, view) = tool(&server, "get_view", json!({}));
    assert_eq!(view["instance"], 2);

    // 11. Everything works again, on the same MCP connection.
    let (err, out) = tool(
        &server,
        "dispatch",
        json!({"type": "activate", "target": "left.files", "item": "docs",
               "expected_instance": 2, "expected_revision": view["revision"]}),
    );
    assert!(!err, "{out}");
    let (_, view) = tool(&server, "get_view", json!({}));
    assert!(
        view["layout"].to_string().contains("files/docs"),
        "the pane moved into docs: {}",
        view["layout"]
    );

    // 12. An action planned against the old instance is refused.
    let (err, stale) = tool(
        &server,
        "dispatch",
        json!({"type": "select", "target": "left.files", "item": "notes.txt",
               "expected_instance": 1, "expected_revision": before}),
    );
    assert!(err);
    assert_eq!(stale["error"], "stale_instance");
    assert_eq!(stale["current_instance"], 2);

    // 13. Nothing on the agent's side was restarted: every call above went
    // through the one `server` made in step 1.
}
