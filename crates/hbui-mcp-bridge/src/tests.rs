use std::path::PathBuf;

use hbui_core::*;
use hbui_ipc::{Bridge, Endpoint, Running};
use serde_json::{json, Value};

use crate::{stdio, Server};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("hbui-b-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(dir: &Dir, session: &str) -> (Shared, Running) {
    let ui = UiState::new(Layout::pane("left", "Left", "files")).with(
        "files",
        Widget::List(List::new(
            "Files",
            vec![Item::new("a", "a.txt"), Item::new("b", "b.txt")],
        )),
    );
    let shared = Shared::new(Session::new(ui, NoController));
    let running = Endpoint::new(shared.clone(), "test")
        .session(session)
        .dir(&dir.0)
        .start()
        .unwrap();
    (shared, running)
}

/// Run a stdio session and return one parsed response per line.
fn session(server: &Server, requests: &[Value]) -> Vec<Value> {
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let mut out = Vec::new();
    stdio::serve(server, input.as_bytes(), &mut out);
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn tool_json(response: &Value) -> Value {
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_and_list_work_with_no_application() {
    let dir = Dir::new("init");
    let r = session(
        &Server::new(Bridge::new(&dir.0), "0"),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            call(3, "list_sessions", json!({})),
            call(4, "get_view", json!({})),
        ],
    );
    assert_eq!(r.len(), 4, "a notification gets no reply");
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-03-26");
    let names: Vec<&str> = r[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["list_sessions", "get_view", "dispatch", "capture_view"]
    );
    assert_eq!(tool_json(&r[2]), json!({"sessions": []}));
    assert_eq!(r[3]["result"]["isError"], true);
    assert_eq!(tool_json(&r[3])["error"], "session_required");
}

#[test]
fn every_call_names_its_session() {
    let dir = Dir::new("sessions");
    let (left, _l) = app(&dir, "left");
    let (_right, _r) = app(&dir, "right");
    let server = Server::new(Bridge::new(&dir.0), "0");
    let r = session(
        &server,
        &[
            call(1, "list_sessions", json!({})),
            call(
                2,
                "dispatch",
                json!({"session": "left", "type": "select", "target": "files",
                                       "item": "b", "expected_instance": 1, "expected_revision": 0}),
            ),
            call(3, "get_view", json!({"session": "right"})),
            call(4, "get_view", json!({"session": "nope"})),
            call(5, "get_view", json!({"pid": std::process::id()})),
        ],
    );
    let sessions = tool_json(&r[0])["sessions"].clone();
    let names: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["left", "right"]);
    assert_eq!(sessions[0]["status"], "ready");
    assert_eq!(sessions[0]["pid"], std::process::id());

    let out = tool_json(&r[1]);
    assert_eq!(
        (out["session"].clone(), out["instance"].clone()),
        (json!("left"), json!(1))
    );
    assert_eq!(left.lock().view()["widgets"]["files"]["selected"], "b");
    // Only the named session changed.
    assert_eq!(tool_json(&r[2])["widgets"]["files"]["selected"], "a");

    assert_eq!(tool_json(&r[3])["error"], "unknown_session");
    assert_eq!(tool_json(&r[3])["sessions"], json!(["left", "right"]));
    assert_eq!(tool_json(&r[4])["status"], "ready");
}

#[test]
fn stale_expectations_are_refused() {
    let dir = Dir::new("stale");
    let (_app, _running) = app(&dir, "s");
    let server = Server::new(Bridge::new(&dir.0), "0");
    let r = session(
        &server,
        &[
            call(
                1,
                "dispatch",
                json!({"session": "s", "type": "select", "target": "files", "item": "b",
                                       "expected_instance": 1, "expected_revision": 0}),
            ),
            call(
                2,
                "dispatch",
                json!({"session": "s", "type": "select", "target": "files", "item": "a",
                                       "expected_instance": 1, "expected_revision": 0}),
            ),
            call(
                3,
                "dispatch",
                json!({"session": "s", "type": "select", "target": "files", "item": "a",
                                       "expected_instance": 7}),
            ),
            call(
                4,
                "get_view",
                json!({"session": "s", "since": 0, "instance": 1}),
            ),
            call(
                5,
                "get_view",
                json!({"session": "s", "since": 0, "instance": 7}),
            ),
            call(6, "capture_view", json!({"session": "s"})),
        ],
    );
    assert_eq!(tool_json(&r[1])["error"], "stale_revision");
    assert_eq!(tool_json(&r[2])["error"], "stale_instance");
    assert_eq!(tool_json(&r[2])["current_instance"], 1);
    assert_eq!(tool_json(&r[3])["changes"][0]["value"], "b");
    // `since` from another instance is meaningless: the whole view instead.
    assert_eq!(tool_json(&r[4])["widgets"]["files"]["selected"], "b");
    assert_eq!(tool_json(&r[5])["error"], "not_supported");
}

/// Clients that ask for resources or prompts get empty lists and carry on;
/// anything else unknown gets an error that says what is here.
#[test]
fn unknown_methods_point_at_the_tools() {
    let dir = Dir::new("methods");
    let r = session(
        &Server::new(Bridge::new(&dir.0), "0"),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "resources/templates/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "prompts/list"}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "resources/read", "params": {"uri": "x"}}),
            call(5, "screenshot", json!({})),
        ],
    );
    assert_eq!(r[0]["result"], json!({"resources": []}));
    assert_eq!(r[1]["result"], json!({"resourceTemplates": []}));
    assert_eq!(r[2]["result"], json!({"prompts": []}));
    let message = r[3]["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("tools only") && message.contains("list_sessions"),
        "{message}"
    );
    assert!(tool_json(&r[4])["message"]
        .as_str()
        .unwrap()
        .contains("list_sessions"));
}
