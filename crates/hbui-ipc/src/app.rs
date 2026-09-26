//! The application's end: claim a session, listen, and serve every bridge
//! that connects.
//!
//! [`Endpoint::start`] claims `<dir>/<session>.sock` and returns at once;
//! bridges come and go on a background thread. Several may be connected at
//! a time — one per agent — and each gets the same answers and the same
//! pushed views.
//!
//! A session is claimed with a lock file held for the life of the process.
//! The kernel drops that lock however the process ends, crash included, so
//! "is this session taken?" never depends on a stale socket file.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hbui_core::{ActionRequest, Shared, UiState};
use serde_json::{json, Value};

use crate::protocol::{
    default_dir, read_line, socket_path, valid_session, write_line, Hello, Welcome, PROTOCOL,
    VERSION,
};

/// Renders a state as text at a size, for `capture_view`.
pub type Capture = Arc<dyn Fn(&UiState, u16, u16) -> String + Send + Sync>;

/// What the endpoint is doing, for an application that wants to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// How many bridges (agents) are connected right now.
    Bridges { count: usize },
    /// A bridge speaks another protocol version. Rebuild one side.
    Rejected { reason: String },
}

type OnLink = Arc<dyn Fn(&Link) + Send + Sync>;

/// How often to look for a revision change to push.
const WATCH: Duration = Duration::from_millis(50);

pub struct Endpoint {
    shared: Shared,
    application: String,
    session: Option<String>,
    dir: PathBuf,
    capture: Option<Capture>,
    on_link: Option<OnLink>,
}

/// A claimed session. Dropping it removes the socket, so a clean exit leaves
/// nothing behind for a bridge to find.
pub struct Running {
    pub session: String,
    pub instance: u64,
    pub socket: PathBuf,
    _lock: File,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl Endpoint {
    pub fn new(shared: Shared, application: impl Into<String>) -> Self {
        Self {
            shared,
            application: application.into(),
            session: None,
            dir: default_dir(),
            capture: None,
            on_link: None,
        }
    }

    /// Claim exactly this session name, failing if another live process
    /// holds it. Without this, the application's name is used, or the first
    /// free `<application>-N`.
    pub fn session(mut self, name: impl Into<String>) -> Self {
        self.session = Some(name.into());
        self
    }

    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Answer `capture_view` by rendering with `capture`.
    pub fn with_capture(mut self, capture: Capture) -> Self {
        self.capture = Some(capture);
        self
    }

    /// Be told when bridges come and go. Called on the endpoint's threads.
    pub fn on_link(mut self, f: impl Fn(&Link) + Send + Sync + 'static) -> Self {
        self.on_link = Some(Arc::new(f));
        self
    }

    /// Claim a session and serve it on background threads.
    pub fn start(self) -> io::Result<Running> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        let (session, instance, lock) = self.claim()?;
        let socket = socket_path(&self.dir, &session);
        // The lock is ours, so any socket file here is a dead run's.
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        // Whoever can connect can drive the person's UI. Only its owner may.
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;

        let hello = Hello {
            protocol: PROTOCOL.into(),
            version: VERSION,
            application: self.application.clone(),
            session: session.clone(),
            instance,
            pid: std::process::id(),
            cwd: std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        let serving = Arc::new(Serving {
            endpoint: self,
            hello,
            bridges: AtomicUsize::new(0),
        });
        thread::Builder::new()
            .name("hbui-ipc-accept".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let serving = serving.clone();
                    thread::spawn(move || serving.connection(stream));
                }
            })?;
        Ok(Running {
            session,
            instance,
            socket,
            _lock: lock,
        })
    }

    /// Take the session's lock file and bump its instance counter.
    fn claim(&self) -> io::Result<(String, u64, File)> {
        let candidates: Vec<String> = match &self.session {
            Some(name) => {
                if !valid_session(name) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{name:?} is not a session name (letters, digits, - _ .)"),
                    ));
                }
                vec![name.clone()]
            }
            None => {
                let base: String = self
                    .application
                    .chars()
                    .map(|c| {
                        if valid_session(&c.to_string()) {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect();
                std::iter::once(base.clone())
                    .chain((2..100).map(|n| format!("{base}-{n}")))
                    .collect()
            }
        };
        for name in &candidates {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path(&self.dir, name))?;
            match file.try_lock() {
                Ok(()) => {
                    let mut text = String::new();
                    file.read_to_string(&mut text)?;
                    let instance = text.trim().parse::<u64>().unwrap_or(0) + 1;
                    file.set_len(0)?;
                    file.rewind()?;
                    write!(file, "{instance}")?;
                    file.flush()?;
                    return Ok((name.clone(), instance, file));
                }
                Err(std::fs::TryLockError::WouldBlock) => continue,
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            match &self.session {
                Some(name) => format!("session {name:?} is held by another running process"),
                None => "no free session name".to_string(),
            },
        ))
    }
}

fn lock_path(dir: &Path, session: &str) -> PathBuf {
    dir.join(format!("{session}.lock"))
}

/// Everything a connection thread needs.
struct Serving {
    endpoint: Endpoint,
    hello: Hello,
    bridges: AtomicUsize,
}

impl Serving {
    fn report(&self, link: Link) {
        if let Some(f) = &self.endpoint.on_link {
            f(&link);
        }
    }

    fn connection(&self, stream: UnixStream) {
        // One reader for the whole connection: a request can follow the
        // welcome line immediately, and a throwaway reader would swallow it.
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut reader = BufReader::new(read_half);
        match self.handshake(&stream, &mut reader) {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return self.report(Link::Rejected { reason }),
            Err(_) => return,
        }
        let count = self.bridges.fetch_add(1, Ordering::Relaxed) + 1;
        self.report(Link::Bridges { count });
        let _ = self.serve(&stream, reader);
        let count = self.bridges.fetch_sub(1, Ordering::Relaxed) - 1;
        self.report(Link::Bridges { count });
    }

    /// `Ok(Err(reason))` if the bridge refused us.
    fn handshake(
        &self,
        stream: &UnixStream,
        reader: &mut BufReader<UnixStream>,
    ) -> io::Result<Result<(), String>> {
        write_line(&mut &*stream, &self.hello)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let welcome =
            read_line(reader)?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        stream.set_read_timeout(None)?;
        let welcome: Welcome = serde_json::from_value(welcome).map_err(io::Error::other)?;
        if welcome.accepted {
            return Ok(Ok(()));
        }
        Ok(Err(
            match (welcome.error.as_deref(), welcome.bridge_version) {
                (Some("protocol_mismatch"), Some(v)) => {
                    format!("protocol_mismatch: bridge speaks v{v}, this application v{VERSION}")
                }
                (e, _) => e.unwrap_or("refused").to_string(),
            },
        ))
    }

    fn serve(&self, stream: &UnixStream, mut reader: BufReader<UnixStream>) -> io::Result<()> {
        let writer = Arc::new(Mutex::new(stream.try_clone()?));
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
        result
    }

    /// Push the view to this bridge now, and again whenever the revision
    /// moves — so the bridge holds the last state even after a crash.
    fn watch(&self, writer: Arc<Mutex<UnixStream>>, stop: Arc<AtomicBool>) -> JoinHandle<()> {
        let shared = self.endpoint.shared.clone();
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
        let shared = &self.endpoint.shared;
        match method {
            "get_view" => {
                let session = shared.lock();
                Ok(match params.get("since").and_then(Value::as_u64) {
                    Some(since) => session.view_since(since),
                    None => session.view(),
                })
            }
            "dispatch" => {
                let req: ActionRequest = serde_json::from_value(params.clone()).map_err(|e| {
                    json!({ "ok": false, "error": "bad_action", "message": format!("not an action: {e}") })
                })?;
                match shared.lock().dispatch(req) {
                    Ok(outcome) => Ok(json!(outcome)),
                    Err(e) => Err(e.to_json()),
                }
            }
            "capture_view" => {
                let Some(capture) = &self.endpoint.capture else {
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
                let session = shared.lock();
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
