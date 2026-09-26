use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hbui_core::*;
use serde_json::json;

use crate::protocol::{read_line, write_line};
use crate::*;

/// A fresh session directory, removed on drop. Short, for the socket-path
/// limit.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("hbui-t-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

fn status(bridge: &Bridge, session: &str) -> Option<&'static str> {
    bridge
        .list()
        .into_iter()
        .find(|s| s.session == session)
        .map(|s| s.status)
}

#[test]
fn a_bridge_finds_an_application_and_forwards_to_it() {
    let dir = Dir::new("forward");
    let app = shared();
    let running = Endpoint::new(app.clone(), "test")
        .dir(&dir.0)
        .start()
        .unwrap();
    assert_eq!((running.session.as_str(), running.instance), ("test", 1));

    let bridge = Bridge::new(&dir.0);
    bridge.scan();
    let info = &bridge.list()[0];
    assert_eq!(
        (info.session.as_str(), info.status, info.pid),
        ("test", "ready", std::process::id())
    );

    let (instance, view) = bridge.request("test", None, "get_view", json!({})).unwrap();
    assert_eq!(instance, 1);
    assert_eq!(view.unwrap()["widgets"]["files"]["selected"], "a");

    let (_, out) = bridge
        .request(
            "test",
            Some(1),
            "dispatch",
            json!({"type": "select", "target": "files", "item": "b", "expected_revision": 0}),
        )
        .unwrap();
    assert_eq!(out.unwrap()["revision"], 1);
    assert_eq!(app.lock().view()["widgets"]["files"]["selected"], "b");

    let (_, out) = bridge
        .request(
            "test",
            None,
            "dispatch",
            json!({"type": "select", "target": "files", "item": "zz"}),
        )
        .unwrap();
    assert_eq!(out.unwrap_err()["error"], "unknown_item");

    assert_eq!(
        bridge.request("test", Some(9), "get_view", json!({})),
        Err(RequestError::StaleInstance { current: 1 })
    );
    assert_eq!(
        bridge.request("nope", None, "get_view", json!({})),
        Err(RequestError::UnknownSession)
    );
    assert_eq!(
        bridge.resolve(&Target::Pid(std::process::id())),
        Some("test".into())
    );
}

#[test]
fn sessions_are_claimed_once_and_named_after_the_application() {
    let dir = Dir::new("claim");
    let first = Endpoint::new(shared(), "app").dir(&dir.0).start().unwrap();
    let second = Endpoint::new(shared(), "app").dir(&dir.0).start().unwrap();
    assert_eq!(first.session, "app");
    assert_eq!(second.session, "app-2");

    let taken = Endpoint::new(shared(), "other")
        .session("app")
        .dir(&dir.0)
        .start()
        .err()
        .unwrap();
    assert_eq!(taken.kind(), std::io::ErrorKind::AddrInUse);

    // Released and reclaimed: the same session, the next instance.
    drop(first);
    let again = Endpoint::new(shared(), "app")
        .session("app")
        .dir(&dir.0)
        .start()
        .unwrap();
    assert_eq!((again.session.as_str(), again.instance), ("app", 2));
}

#[test]
fn several_bridges_serve_one_application() {
    let dir = Dir::new("multi");
    let app = shared();
    let _running = Endpoint::new(app.clone(), "test")
        .dir(&dir.0)
        .start()
        .unwrap();
    let (a, b) = (Bridge::new(&dir.0), Bridge::new(&dir.0));
    a.scan();
    b.scan();
    a.request(
        "test",
        None,
        "dispatch",
        json!({"type": "select", "target": "files", "item": "b"}),
    )
    .unwrap()
    .1
    .unwrap();
    let (_, view) = b.request("test", None, "get_view", json!({})).unwrap();
    assert_eq!(view.unwrap()["widgets"]["files"]["selected"], "b");
}

#[test]
fn the_bridge_keeps_the_view_the_application_pushes() {
    let dir = Dir::new("snapshot");
    let app = shared();
    let _running = Endpoint::new(app.clone(), "test")
        .dir(&dir.0)
        .start()
        .unwrap();
    let bridge = Bridge::new(&dir.0);
    bridge.scan();

    // A person's change, which the bridge never asked about, still arrives.
    // (What happens when the process then dies is covered, with a real
    // process, by commander's bridge_lifecycle test.)
    app.lock()
        .dispatch(
            Action::Select {
                target: "files".into(),
                item: "b".into(),
            }
            .into(),
        )
        .unwrap();
    eventually("the pushed view", || bridge.list()[0].revision == Some(1));
}

#[test]
fn a_mismatched_application_is_refused_by_name() {
    let dir = Dir::new("mismatch");
    std::fs::create_dir_all(&dir.0).unwrap();
    let listener = UnixListener::bind(socket_path(&dir.0, "future")).unwrap();
    let fake = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let hello = Hello {
            protocol: PROTOCOL.into(),
            version: VERSION + 1,
            application: "future".into(),
            session: "future".into(),
            instance: 1,
            pid: 1,
            cwd: String::new(),
        };
        write_line(&mut &stream, &hello).unwrap();
        read_line(&mut BufReader::new(&stream)).unwrap().unwrap()
    });
    let bridge = Bridge::new(&dir.0);
    bridge.scan();
    let welcome = fake.join().unwrap();
    assert_eq!(welcome["accepted"], false);
    assert_eq!(welcome["error"], "protocol_mismatch");
    assert_eq!(status(&bridge, "future"), Some("protocol_mismatch"));
    assert_eq!(
        bridge.request("future", None, "get_view", json!({})),
        Err(RequestError::Mismatch {
            application_version: VERSION + 1
        })
    );
}
