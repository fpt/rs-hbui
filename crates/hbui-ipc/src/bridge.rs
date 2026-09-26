//! The bridge's end: listen, accept one application at a time, number each
//! one, and forward requests to whichever is connected.
//!
//! The bridge owns no UI state. What it does own is what has to outlive the
//! application: the socket, the instance counter, and the last view the
//! application pushed — kept for when the application is gone, and always
//! reported as stale.

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::{read_line, write_line, Hello, Welcome, PROTOCOL, VERSION};

/// How long a request waits for the application. Long enough for a slow
/// capture; short enough that a wedged application answers the agent with
/// an error instead of hanging it.
const TIMEOUT: Duration = Duration::from_secs(10);

type Reply = Result<Value, Value>;
type Pending = Arc<Mutex<HashMap<u64, SyncSender<Reply>>>>;

/// The application connection currently in use.
#[derive(Clone)]
struct Conn {
    instance: u64,
    application: String,
    writer: Arc<Mutex<UnixStream>>,
    pending: Pending,
}

/// The last view an application pushed, and which instance pushed it.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub instance: u64,
    pub application: String,
    pub view: Value,
}

/// Why the bridge could not refuse or answer a request itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// No application is connected.
    Unavailable,
    /// The caller named an instance that is not the connected one.
    StaleInstance { current: u64 },
    /// The application went away before answering. It may or may not have
    /// acted on the request.
    Disconnected,
    /// The application did not answer in time.
    Timeout,
}

/// The last handshake the bridge refused for a version mismatch — kept so an
/// agent asking why nothing is connected hears the real reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch {
    pub application_version: u32,
}

#[derive(Default)]
struct State {
    conn: Option<Conn>,
    last_instance: u64,
    snapshot: Option<Snapshot>,
    mismatch: Option<Mismatch>,
}

pub struct Bridge {
    path: PathBuf,
    state: Mutex<State>,
    next_id: AtomicU64,
}

/// What an agent is told about the link when there is nothing better to say.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// `(instance, application)` of the connected application, if any.
    pub connected: Option<(u64, String)>,
    pub snapshot: Option<Snapshot>,
    pub mismatch: Option<Mismatch>,
}

impl Bridge {
    /// Listen on `path` and accept applications on a background thread.
    ///
    /// A socket file left behind by a bridge that died is removed; one that a
    /// live bridge is still answering on is an error, not something to steal.
    pub fn listen(path: impl Into<PathBuf>) -> io::Result<Arc<Bridge>> {
        let path = path.into();
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another bridge is listening on {}", path.display()),
                ));
            }
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        // Whoever can connect can drive the person's UI. Only its owner may.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        let bridge = Arc::new(Bridge {
            path,
            state: Mutex::new(State::default()),
            next_id: AtomicU64::new(1),
        });
        let accepting = bridge.clone();
        thread::Builder::new()
            .name("hbui-bridge-accept".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let bridge = accepting.clone();
                    thread::spawn(move || {
                        let _ = bridge.serve(stream);
                    });
                }
            })?;
        Ok(bridge)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        let s = self.state();
        Status {
            connected: s.conn.as_ref().map(|c| (c.instance, c.application.clone())),
            snapshot: s.snapshot.clone(),
            mismatch: s.mismatch,
        }
    }

    /// Send `method` to the connected application and wait for its answer.
    ///
    /// With `expected_instance`, refuse up front unless that is the instance
    /// connected right now — checked against the same connection the request
    /// then goes to, so a restart in between cannot slip past.
    pub fn request(
        &self,
        expected_instance: Option<u64>,
        method: &str,
        params: Value,
    ) -> Result<(u64, Reply), RequestError> {
        let conn = self.state().conn.clone().ok_or(RequestError::Unavailable)?;
        if let Some(expected) = expected_instance {
            if expected != conn.instance {
                return Err(RequestError::StaleInstance {
                    current: conn.instance,
                });
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        lock(&conn.pending).insert(id, tx);
        let msg = json!({ "id": id, "method": method, "params": params });
        if write_line(&mut *lock(&conn.writer), &msg).is_err() {
            lock(&conn.pending).remove(&id);
            return Err(RequestError::Disconnected);
        }
        match rx.recv_timeout(TIMEOUT) {
            Ok(reply) => Ok((conn.instance, reply)),
            Err(RecvTimeoutError::Timeout) => {
                lock(&conn.pending).remove(&id);
                Err(RequestError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(RequestError::Disconnected),
        }
    }

    /// One application connection: handshake, then read until it ends.
    fn serve(&self, stream: UnixStream) -> io::Result<()> {
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream.try_clone()?);

        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let Some(hello) = read_line(&mut reader)? else {
            return Ok(());
        };
        stream.set_read_timeout(None)?;
        let hello: Hello = serde_json::from_value(hello).map_err(io::Error::other)?;
        if hello.protocol != PROTOCOL || hello.version != VERSION {
            self.state().mismatch = Some(Mismatch {
                application_version: hello.version,
            });
            let refusal = Welcome {
                accepted: false,
                instance: None,
                error: Some("protocol_mismatch".into()),
                bridge_version: Some(VERSION),
            };
            return write_line(&mut writer, &refusal);
        }

        let pending: Pending = Arc::default();
        let instance = {
            let mut s = self.state();
            s.last_instance += 1;
            let conn = Conn {
                instance: s.last_instance,
                application: hello.application.clone(),
                writer: Arc::new(Mutex::new(writer.try_clone()?)),
                pending: pending.clone(),
            };
            // A new instance replaces the old: the old one is almost always a
            // process on its way out. Cut it off so nothing reaches it.
            if let Some(old) = s.conn.replace(conn) {
                let _ = lock(&old.writer).shutdown(std::net::Shutdown::Both);
            }
            s.mismatch = None;
            s.last_instance
        };
        eprintln!(
            "hbui-mcp-bridge: {} (pid {}) connected as instance {instance}",
            hello.application, hello.pid
        );
        let welcome = Welcome {
            accepted: true,
            instance: Some(instance),
            error: None,
            bridge_version: Some(VERSION),
        };
        let result = write_line(&mut writer, &welcome).and_then(|()| {
            while let Some(msg) = read_line(&mut reader)? {
                if let Some(id) = msg.get("id").and_then(Value::as_u64) {
                    let reply = match msg.get("error") {
                        Some(e) => Err(e.clone()),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    if let Some(tx) = lock(&pending).remove(&id) {
                        let _ = tx.send(reply);
                    }
                } else if msg.get("method").and_then(Value::as_str) == Some("view_changed") {
                    if let Some(view) = msg.pointer("/params/view") {
                        self.state().snapshot = Some(Snapshot {
                            instance,
                            application: hello.application.clone(),
                            view: view.clone(),
                        });
                    }
                }
            }
            Ok(())
        });

        // Gone. Anyone still waiting on an answer from it gets Disconnected
        // when their sender drops here.
        lock(&pending).clear();
        let mut s = self.state();
        if s.conn.as_ref().is_some_and(|c| c.instance == instance) {
            s.conn = None;
            eprintln!("hbui-mcp-bridge: instance {instance} disconnected");
        }
        result
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
