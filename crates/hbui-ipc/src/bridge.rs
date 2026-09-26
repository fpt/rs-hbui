//! The bridge's end: find every application in the session directory, stay
//! connected to each, and forward requests to the one named.
//!
//! The bridge owns no UI state. What it keeps per session is what has to
//! outlive a process: which instance was last seen, and the last view that
//! instance pushed — for when it is gone, and always reported as stale.
//!
//! Sessions are named explicitly on every request. There is no "current
//! session": an agent that selected one and then reasoned for a while would
//! be acting on an assumption nothing re-checks.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol::{read_line, write_line, Hello, Welcome, PROTOCOL, VERSION};

/// How long a request waits for the application. Long enough for a slow
/// capture; short enough that a wedged application answers the agent with
/// an error instead of hanging it.
const TIMEOUT: Duration = Duration::from_secs(10);
/// How often the background scan looks for new or restarted applications.
const SCAN_EVERY: Duration = Duration::from_millis(250);
/// After refusing a mismatched application, how long before trying it again.
const MISMATCH_BACKOFF: Duration = Duration::from_secs(5);

type Reply = Result<Value, Value>;
type Pending = Arc<Mutex<HashMap<u64, SyncSender<Reply>>>>;

#[derive(Clone)]
struct Conn {
    instance: u64,
    writer: Arc<Mutex<UnixStream>>,
    pending: Pending,
}

/// Everything the bridge knows about one logical session.
struct Entry {
    application: String,
    pid: u32,
    cwd: String,
    instance: u64,
    conn: Option<Conn>,
    /// The last view pushed, with the instance that pushed it.
    snapshot: Option<(u64, Value)>,
    mismatch: Option<u32>,
    retry_after: Option<Instant>,
}

impl Entry {
    fn from_hello(h: &Hello) -> Self {
        Self {
            application: h.application.clone(),
            pid: h.pid,
            cwd: h.cwd.clone(),
            instance: h.instance,
            conn: None,
            snapshot: None,
            mismatch: None,
            retry_after: None,
        }
    }
}

/// One row of `list_sessions`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub session: String,
    pub application: String,
    pub pid: u32,
    pub cwd: String,
    pub instance: u64,
    /// `ready`, `disconnected` or `protocol_mismatch`.
    pub status: &'static str,
    /// The revision of the last view seen, if any.
    pub revision: Option<u64>,
}

impl SessionInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "session": self.session,
            "application": self.application,
            "pid": self.pid,
            "cwd": self.cwd,
            "instance": self.instance,
            "status": self.status,
            "revision": self.revision,
        })
    }
}

/// How a request names its application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Session(String),
    /// A convenience selector. PIDs change on every restart; sessions do not.
    Pid(u32),
}

/// Why a request got no answer from the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    UnknownSession,
    /// The session is known but its application is not running. Carries
    /// the last view it pushed, if any: `(instance, view)`.
    Unavailable {
        snapshot: Option<(u64, Value)>,
    },
    /// The application speaks another protocol version.
    Mismatch {
        application_version: u32,
    },
    /// The caller named an instance that is not the running one.
    StaleInstance {
        current: u64,
    },
    /// The application went away before answering. It may or may not have
    /// acted on the request.
    Disconnected,
    /// The application did not answer in time.
    Timeout,
}

pub struct Bridge {
    dir: PathBuf,
    sessions: Mutex<BTreeMap<String, Entry>>,
    scanning: Mutex<()>,
    next_id: AtomicU64,
}

impl Bridge {
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Bridge> {
        Arc::new(Bridge {
            dir: dir.into(),
            sessions: Mutex::new(BTreeMap::new()),
            scanning: Mutex::new(()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Keep scanning in the background, so a restarted application is
    /// reconnected — and its views cached — before anyone asks for it.
    pub fn watch(self: &Arc<Self>) {
        let bridge = Arc::downgrade(self);
        thread::Builder::new()
            .name("hbui-bridge-scan".into())
            .spawn(move || {
                while let Some(b) = bridge.upgrade() {
                    b.scan();
                    drop(b);
                    thread::sleep(SCAN_EVERY);
                }
            })
            .expect("spawning the scan thread");
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn sessions(&self) -> MutexGuard<'_, BTreeMap<String, Entry>> {
        lock(&self.sessions)
    }

    /// Connect to every application in the directory not already connected.
    /// A socket nobody answers on is a dead run's, and is left alone.
    pub fn scan(self: &Arc<Self>) {
        let _one_at_a_time = lock(&self.scanning);
        let Ok(dir) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "sock") {
                continue;
            }
            let Some(name) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let skip = self.sessions().get(&name).is_some_and(|e| {
                e.conn.is_some() || e.retry_after.is_some_and(|t| Instant::now() < t)
            });
            if skip {
                continue;
            }
            if let Ok(stream) = UnixStream::connect(&path) {
                let _ = self.attach(&name, stream);
            }
        }
    }

    /// Handshake with a freshly connected application and start reading it.
    fn attach(self: &Arc<Self>, name: &str, stream: UnixStream) -> io::Result<()> {
        let mut writer = stream.try_clone()?;
        let mut reader = BufReader::new(stream.try_clone()?);
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let hello = read_line(&mut reader)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        stream.set_read_timeout(None)?;
        let hello: Hello = serde_json::from_value(hello).map_err(io::Error::other)?;

        if hello.protocol != PROTOCOL || hello.version != VERSION {
            {
                let mut sessions = self.sessions();
                let e = sessions
                    .entry(name.to_string())
                    .or_insert_with(|| Entry::from_hello(&hello));
                e.mismatch = Some(hello.version);
                e.retry_after = Some(Instant::now() + MISMATCH_BACKOFF);
            }
            let refusal = Welcome {
                accepted: false,
                error: Some("protocol_mismatch".into()),
                bridge_version: Some(VERSION),
            };
            return write_line(&mut writer, &refusal);
        }

        let pending: Pending = Arc::default();
        {
            let mut sessions = self.sessions();
            let e = sessions
                .entry(name.to_string())
                .or_insert_with(|| Entry::from_hello(&hello));
            e.application = hello.application.clone();
            e.pid = hello.pid;
            e.cwd = hello.cwd.clone();
            e.instance = hello.instance;
            e.mismatch = None;
            e.retry_after = None;
            e.conn = Some(Conn {
                instance: hello.instance,
                writer: Arc::new(Mutex::new(writer.try_clone()?)),
                pending: pending.clone(),
            });
        }
        let welcome = Welcome {
            accepted: true,
            error: None,
            bridge_version: Some(VERSION),
        };
        write_line(&mut writer, &welcome)?;

        let bridge = self.clone();
        let name = name.to_string();
        thread::spawn(move || bridge.read(&name, hello.instance, reader, pending));
        Ok(())
    }

    /// Route answers to their waiters and keep the snapshot current, until
    /// the application goes away.
    fn read(&self, name: &str, instance: u64, mut reader: BufReader<UnixStream>, pending: Pending) {
        while let Ok(Some(msg)) = read_line(&mut reader) {
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
                    if let Some(e) = self.sessions().get_mut(name) {
                        e.snapshot = Some((instance, view.clone()));
                    }
                }
            }
        }
        // Anyone still waiting hears Disconnected when their sender drops.
        lock(&pending).clear();
        if let Some(e) = self.sessions().get_mut(name) {
            if e.conn.as_ref().is_some_and(|c| c.instance == instance) {
                e.conn = None;
            }
        }
    }

    /// Every session this bridge has seen, running or not.
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions()
            .iter()
            .map(|(name, e)| SessionInfo {
                session: name.clone(),
                application: e.application.clone(),
                pid: e.pid,
                cwd: e.cwd.clone(),
                instance: e.instance,
                status: match (&e.conn, e.mismatch) {
                    (Some(_), _) => "ready",
                    (None, Some(_)) => "protocol_mismatch",
                    (None, None) => "disconnected",
                },
                revision: e
                    .snapshot
                    .as_ref()
                    .and_then(|(_, v)| v.get("revision"))
                    .and_then(Value::as_u64),
            })
            .collect()
    }

    /// The session a target names. A PID prefers a running session, since a
    /// dead one may share it with some unrelated later process.
    pub fn resolve(&self, target: &Target) -> Option<String> {
        let sessions = self.sessions();
        match target {
            Target::Session(name) => sessions.contains_key(name).then(|| name.clone()),
            Target::Pid(pid) => sessions
                .iter()
                .filter(|(_, e)| e.pid == *pid)
                .max_by_key(|(_, e)| e.conn.is_some())
                .map(|(name, _)| name.clone()),
        }
    }

    /// Send `method` to a session's application and wait for its answer.
    ///
    /// With `expected_instance`, refuse up front unless that is the instance
    /// connected right now — checked against the same connection the request
    /// then goes to, so a restart in between cannot slip past.
    pub fn request(
        &self,
        session: &str,
        expected_instance: Option<u64>,
        method: &str,
        params: Value,
    ) -> Result<(u64, Reply), RequestError> {
        let conn = {
            let sessions = self.sessions();
            let e = sessions.get(session).ok_or(RequestError::UnknownSession)?;
            match (&e.conn, e.mismatch) {
                (Some(c), _) => c.clone(),
                (None, Some(v)) => {
                    return Err(RequestError::Mismatch {
                        application_version: v,
                    })
                }
                (None, None) => {
                    return Err(RequestError::Unavailable {
                        snapshot: e.snapshot.clone(),
                    })
                }
            }
        };
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
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
