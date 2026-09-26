use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hbui_core::*;
use serde_json::json;

use crate::protocol::{read_line, write_line};
use crate::*;

fn socket(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hbui-ipc-{name}-{}.sock", std::process::id()))
}

fn shared() -> Shared {
    let ui = UiState::new(Layout::Widget("files".into())).with(
        "files",
        Widget::List(List::new(
            "Files",
            vec![Item::new("a", "a"), Item::new("b", "b")],
        )),
    );
    Shared::new(Session::new(ui, NoController))
}

/// Poll until `f` holds, or fail after a couple of seconds.
fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_application_connects_and_answers() {
    let path = socket("answers");
    let bridge = Bridge::listen(&path).unwrap();
    let app = shared();
    Endpoint::new(app.clone(), &path, "test").spawn();
    eventually("connection", || bridge.status().connected.is_some());

    let (instance, view) = bridge.request(None, "get_view", json!({})).unwrap();
    assert_eq!(instance, 1);
    assert_eq!(view.unwrap()["widgets"]["files"]["selected"], "a");

    let (_, out) = bridge
        .request(
            Some(1),
            "dispatch",
            json!({"type": "select", "target": "files", "item": "b", "expected_revision": 0}),
        )
        .unwrap();
    assert_eq!(out.unwrap()["revision"], 1);
    assert_eq!(app.lock().view()["widgets"]["files"]["selected"], "b");

    // Refusals come back as the application's error, not a transport error.
    let (_, out) = bridge
        .request(
            None,
            "dispatch",
            json!({"type": "select", "target": "files", "item": "zz"}),
        )
        .unwrap();
    assert_eq!(out.unwrap_err()["error"], "unknown_item");

    assert_eq!(
        bridge.request(Some(9), "get_view", json!({})),
        Err(RequestError::StaleInstance { current: 1 })
    );
}

#[test]
fn the_bridge_keeps_the_last_pushed_view() {
    let path = socket("snapshot");
    let bridge = Bridge::listen(&path).unwrap();
    let app = shared();
    Endpoint::new(app.clone(), &path, "test").spawn();
    eventually("connection", || bridge.status().connected.is_some());

    // A person's change, which the bridge never asked about, still arrives.
    app.lock()
        .dispatch(
            Action::Select {
                target: "files".into(),
                item: "b".into(),
            }
            .into(),
        )
        .unwrap();
    eventually("the pushed view", || {
        bridge
            .status()
            .snapshot
            .is_some_and(|s| s.view["widgets"]["files"]["selected"] == "b")
    });
}

#[test]
fn a_mismatched_application_is_refused_by_name() {
    let path = socket("mismatch");
    let bridge = Bridge::listen(&path).unwrap();
    let stream = UnixStream::connect(&path).unwrap();
    let mut hello = Hello::new("future");
    hello.version = VERSION + 1;
    write_line(&mut &stream, &hello).unwrap();
    let welcome = read_line(&mut BufReader::new(&stream)).unwrap().unwrap();
    assert_eq!(welcome["accepted"], false);
    assert_eq!(welcome["error"], "protocol_mismatch");
    assert_eq!(welcome["bridge_version"], VERSION);
    assert_eq!(
        bridge.status().mismatch,
        Some(Mismatch {
            application_version: VERSION + 1
        })
    );
    assert_eq!(
        bridge.request(None, "get_view", json!({})),
        Err(RequestError::Unavailable)
    );
}

#[test]
fn a_live_bridge_is_not_replaced_but_a_dead_one_is() {
    let path = socket("twice");
    let first = Bridge::listen(&path).unwrap();
    let err = Bridge::listen(&path).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
    // A stale file with nobody behind it is cleaned up.
    std::mem::forget(first); // keep the file, as a crash would
    let stale = socket("stale");
    std::os::unix::net::UnixListener::bind(&stale).unwrap(); // bound, then dropped
    Bridge::listen(&stale).unwrap();
}
