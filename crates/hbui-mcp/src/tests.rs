use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use hbui_core::*;
use serde_json::{json, Value};

use crate::{serve_http, stdio, Server};

fn server() -> Server {
    let ui = UiState::new(Layout::pane("left", "Left", "files")).with(
        "files",
        Widget::List(List::new(
            "Files",
            vec![Item::new("a", "a.txt"), Item::new("b", "b.txt")],
        )),
    );
    Server::new(Shared::new(Session::new(ui, NoController)), "test", "0")
}

/// Run a whole stdio session and return one parsed response per line.
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
fn initialize_list_and_notifications() {
    let r = session(
        &server(),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ],
    );
    assert_eq!(r.len(), 2, "a notification gets no reply");
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-03-26");
    let names: Vec<&str> = r[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["get_view", "dispatch"],
        "no capture_view without a renderer"
    );
}

#[test]
fn get_view_then_dispatch_then_get_view_since() {
    let r = session(
        &server(),
        &[
            call(1, "get_view", json!({})),
            call(
                2,
                "dispatch",
                json!({"type": "select", "target": "files", "item": "b", "expected_revision": 1}),
            ),
            call(
                3,
                "dispatch",
                json!({"type": "select", "target": "files", "item": "a", "expected_revision": 1}),
            ),
            call(4, "get_view", json!({"since": 1})),
        ],
    );
    let view = tool_json(&r[0]);
    assert_eq!(view["revision"], 1);
    assert_eq!(view["widgets"]["files"]["selected"], "a");

    let out = tool_json(&r[1]);
    assert_eq!(out["revision"], 2);
    assert_eq!(out["changes"][0]["path"], "/widgets/files/selected");

    assert_eq!(r[2]["result"]["isError"], true);
    assert_eq!(tool_json(&r[2])["error"], "stale_view");
    assert_eq!(tool_json(&r[2])["current_revision"], 2);

    assert_eq!(tool_json(&r[3])["changes"][0]["value"], "b");
}

#[test]
fn a_malformed_action_is_a_tool_error_not_a_protocol_error() {
    let r = session(
        &server(),
        &[call(1, "dispatch", json!({"type": "teleport"}))],
    );
    assert!(r[0].get("error").is_none());
    assert_eq!(r[0]["result"]["isError"], true);
    assert_eq!(tool_json(&r[0])["error"], "bad_action");
}

#[test]
fn capture_view_is_offered_when_a_renderer_is() {
    let server = server().with_capture(Arc::new(|ui: &UiState, w, h| {
        format!("{w}x{h} focus={:?}", ui.focus())
    }));
    let r = session(&server, &[call(1, "capture_view", json!({"width": 40}))]);
    let text = r[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, "revision 1\n40x24 focus=Some(WidgetId(\"files\"))");
}

#[test]
fn http_answers_a_post_on_the_same_session() {
    let server = Arc::new(server());
    let addr = serve_http(server.clone(), 0).unwrap();
    let body = call(
        7,
        "dispatch",
        json!({"type": "select", "target": "files", "item": "b"}),
    )
    .to_string();
    let mut s = TcpStream::connect(addr).unwrap();
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    s.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    // The change landed on the shared session the person's terminal draws.
    assert_eq!(
        server.shared().lock().view()["widgets"]["files"]["selected"],
        "b"
    );
}
