use std::path::PathBuf;
use std::time::{Duration, Instant};

use hbui_core::*;
use hbui_ipc::{Bridge, Endpoint};
use serde_json::{json, Value};

use crate::{stdio, Server};

fn socket(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hbui-bridge-{name}-{}.sock", std::process::id()))
}

fn app() -> Shared {
    let ui = UiState::new(Layout::pane("left", "Left", "files")).with(
        "files",
        Widget::List(List::new(
            "Files",
            vec![Item::new("a", "a.txt"), Item::new("b", "b.txt")],
        )),
    );
    Shared::new(Session::new(ui, NoController))
}

fn server(name: &str) -> Server {
    Server::new(Bridge::listen(socket(name)).unwrap(), "0")
}

fn connect(server: &Server, app: &Shared) {
    Endpoint::new(app.clone(), server.bridge().path(), "test").spawn();
    let start = Instant::now();
    while server.bridge().status().connected.is_none() {
        assert!(start.elapsed() < Duration::from_secs(3), "no connection");
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let r = session(
        &server("init"),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            call(3, "get_view", json!({})),
        ],
    );
    assert_eq!(r.len(), 3, "a notification gets no reply");
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-03-26");
    let names: Vec<&str> = r[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["get_view", "dispatch", "capture_view"]);
    // Not an error: the bridge is fine, the application is simply absent.
    assert!(r[2]["result"].get("isError").is_none());
    assert_eq!(tool_json(&r[2])["status"], "application_unavailable");
}

#[test]
fn views_and_actions_carry_the_instance() {
    let server = server("instance");
    connect(&server, &app());
    let r = session(
        &server,
        &[
            call(1, "get_view", json!({})),
            call(
                2,
                "dispatch",
                json!({"type": "select", "target": "files", "item": "b",
                                       "expected_instance": 1, "expected_revision": 0}),
            ),
            call(
                3,
                "dispatch",
                json!({"type": "select", "target": "files", "item": "a",
                                       "expected_instance": 1, "expected_revision": 0}),
            ),
            call(
                4,
                "dispatch",
                json!({"type": "select", "target": "files", "item": "a",
                                       "expected_instance": 7}),
            ),
            call(5, "get_view", json!({"since": 0, "instance": 1})),
            call(6, "get_view", json!({"since": 0, "instance": 7})),
        ],
    );
    let view = tool_json(&r[0]);
    assert_eq!(
        (
            view["status"].clone(),
            view["instance"].clone(),
            view["revision"].clone()
        ),
        (json!("ready"), json!(1), json!(0))
    );

    let out = tool_json(&r[1]);
    assert_eq!(
        (out["instance"].clone(), out["revision"].clone()),
        (json!(1), json!(1))
    );

    assert_eq!(r[2]["result"]["isError"], true);
    assert_eq!(tool_json(&r[2])["error"], "stale_revision");

    assert_eq!(r[3]["result"]["isError"], true);
    assert_eq!(tool_json(&r[3])["error"], "stale_instance");
    assert_eq!(tool_json(&r[3])["current_instance"], 1);

    assert_eq!(tool_json(&r[4])["changes"][0]["value"], "b");
    // `since` from another instance is meaningless: the whole view instead.
    assert_eq!(tool_json(&r[5])["widgets"]["files"]["selected"], "b");
}

#[test]
fn capture_view_is_the_applications_to_answer() {
    let server = server("capture");
    connect(&server, &app());
    let r = session(&server, &[call(1, "capture_view", json!({}))]);
    assert_eq!(r[0]["result"]["isError"], true);
    assert_eq!(tool_json(&r[0])["error"], "not_supported");
}
