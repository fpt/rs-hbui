//! The application's end: connect to the bridge, and keep connecting.
//!
//! An application calls [`Endpoint::spawn`] once and forgets about it. The
//! endpoint retries until a bridge is listening, answers its requests against
//! the shared session, pushes the view whenever it changes, and when the
//! bridge goes away, goes back to retrying. Nothing about this blocks or
//! fails the application: a bridge is something a developer runs, not
//! something the application needs.

use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hbui_core::{ActionRequest, Shared, UiState};
use serde_json::{json, Value};

use crate::protocol::{read_line, write_line, Hello, Welcome, VERSION};

/// Renders a state as text at a size, for `capture_view`.
pub type Capture = Arc<dyn Fn(&UiState, u16, u16) -> String + Send + Sync>;

/// What the link to the bridge is doing, for an application that wants to
/// show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connected {
        instance: u64,
    },
    Disconnected,
    /// The bridge speaks another protocol version. Rebuild one side.
    Rejected {
        reason: String,
    },
}

type OnLink = Arc<dyn Fn(&Link) + Send + Sync>;

/// How long between connection attempts while no bridge is listening. Short,
/// so that a restarted application is back in front of the agent almost at
/// once.
const RETRY: Duration = Duration::from_millis(250);
/// After a refusal, retrying fast would only be refused fast.
const RETRY_REJECTED: Duration = Duration::from_secs(3);
/// How often to look for a revision change to push.
const WATCH: Duration = Duration::from_millis(50);

pub struct Endpoint {
    shared: Shared,
    path: PathBuf,
    application: String,
    capture: Option<Capture>,
    on_link: Option<OnLink>,
}

impl Endpoint {
    pub fn new(shared: Shared, path: impl Into<PathBuf>, application: impl Into<String>) -> Self {
        Self {
            shared,
            path: path.into(),
            application: application.into(),
            capture: None,
            on_link: None,
        }
    }

    /// Answer `capture_view` by rendering with `capture`.
    pub fn with_capture(mut self, capture: Capture) -> Self {
        self.capture = Some(capture);
        self
    }

    /// Be told when the link comes and goes. Called on the endpoint's thread.
    pub fn on_link(mut self, f: impl Fn(&Link) + Send + Sync + 'static) -> Self {
        self.on_link = Some(Arc::new(f));
        self
    }

    /// Run on a background thread for the life of the process.
    pub fn spawn(self) -> JoinHandle<()> {
        thread::Builder::new()
            .name("hbui-ipc".into())
            .spawn(move || self.run())
            .expect("spawning the ipc thread")
    }

    fn run(self) {
        let mut last: Option<Link> = None;
        let mut report = |link: Link| {
            if last.as_ref() != Some(&link) {
                if let Some(f) = &self.on_link {
                    f(&link);
                }
                last = Some(link);
            }
        };
        loop {
            let Ok(stream) = UnixStream::connect(&self.path) else {
                thread::sleep(RETRY);
                continue;
            };
            match self.session(stream, &mut report) {
                Ok(Some(reason)) => {
                    report(Link::Rejected { reason });
                    thread::sleep(RETRY_REJECTED);
                }
                Ok(None) | Err(_) => {
                    report(Link::Disconnected);
                    thread::sleep(RETRY);
                }
            }
        }
    }

    /// One connection, start to end. `Ok(Some(reason))` if the bridge
    /// refused the handshake.
    fn session(
        &self,
        stream: UnixStream,
        report: &mut impl FnMut(Link),
    ) -> io::Result<Option<String>> {
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream.try_clone()?);

        write_line(&mut writer, &Hello::new(&self.application))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let Some(welcome) = read_line(&mut reader)? else {
            return Ok(None);
        };
        stream.set_read_timeout(None)?;
        let welcome: Welcome = serde_json::from_value(welcome).map_err(io::Error::other)?;
        if !welcome.accepted {
            let reason = match (welcome.error.as_deref(), welcome.bridge_version) {
                (Some("protocol_mismatch"), Some(v)) => {
                    format!("protocol_mismatch: bridge speaks v{v}, this application v{VERSION}")
                }
                (e, _) => e.unwrap_or("refused").to_string(),
            };
            return Ok(Some(reason));
        }
        report(Link::Connected {
            instance: welcome.instance.unwrap_or(0),
        });

        let writer = Arc::new(Mutex::new(writer));
        let stop = Arc::new(AtomicBool::new(false));
        let watcher = self.watch(writer.clone(), stop.clone());

        let result = (|| -> io::Result<()> {
            while let Some(msg) = read_line(&mut reader)? {
                let Some(id) = msg.get("id").cloned() else {
                    continue;
                };
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                let reply = match self.handle(method, &params) {
                    Ok(result) => json!({ "id": id, "result": result }),
                    Err(error) => json!({ "id": id, "error": error }),
                };
                write_line(&mut *lock(&writer), &reply)?;
            }
            Ok(())
        })();

        stop.store(true, Ordering::Relaxed);
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = watcher.join();
        result.map(|()| None)
    }

    /// Push the view to the bridge now, and again whenever the revision moves.
    fn watch(&self, writer: Arc<Mutex<UnixStream>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
        let shared = self.shared.clone();
        thread::spawn(move || {
            let mut pushed = None;
            while !stop.load(Ordering::Relaxed) {
                let view = {
                    let session = shared.lock();
                    (pushed != Some(session.revision()))
                        .then(|| (session.revision(), session.view()))
                };
                if let Some((revision, view)) = view {
                    let msg = json!({ "method": "view_changed", "params": { "view": view } });
                    if write_line(&mut *lock(&writer), &msg).is_err() {
                        return;
                    }
                    pushed = Some(revision);
                }
                thread::sleep(WATCH);
            }
        })
    }

    fn handle(&self, method: &str, params: &Value) -> Result<Value, Value> {
        match method {
            "get_view" => {
                let session = self.shared.lock();
                Ok(match params.get("since").and_then(Value::as_u64) {
                    Some(since) => session.view_since(since),
                    None => session.view(),
                })
            }
            "dispatch" => {
                let req: ActionRequest = serde_json::from_value(params.clone()).map_err(|e| {
                    json!({ "ok": false, "error": "bad_action", "message": format!("not an action: {e}") })
                })?;
                match self.shared.lock().dispatch(req) {
                    Ok(outcome) => Ok(json!(outcome)),
                    Err(e) => Err(e.to_json()),
                }
            }
            "capture_view" => {
                let Some(capture) = &self.capture else {
                    return Err(json!({
                        "ok": false,
                        "error": "not_supported",
                        "message": "this application does not render captures",
                    }));
                };
                let dim = |k: &str, d: u16| {
                    params
                        .get(k)
                        .and_then(Value::as_u64)
                        .map_or(d, |v| v.clamp(10, 500) as u16)
                };
                let session = self.shared.lock();
                Ok(json!({
                    "revision": session.revision(),
                    "text": capture(session.ui(), dim("width", 80), dim("height", 24)),
                }))
            }
            other => Err(json!({
                "ok": false,
                "error": "unknown_method",
                "message": format!("no method {other:?}"),
            })),
        }
    }
}

/// A poisoned writer is still a writer: the panic that poisoned it was in
/// some other message's code, not in the socket.
fn lock(m: &Mutex<UnixStream>) -> std::sync::MutexGuard<'_, UnixStream> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
