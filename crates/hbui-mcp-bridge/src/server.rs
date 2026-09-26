//! The MCP tools, forwarded to the session each call names.
//!
//! `list_sessions` is the bridge's own. `get_view`, `dispatch` and
//! `capture_view` are the application's: the bridge adds the `session` and
//! `instance` they came from, refuses `stale_instance`, and answers for the
//! application when it is not there. Answers, not fails: an application that
//! is down is a state the agent reads in a normal result, never an error
//! that looks like the bridge broke.

use std::sync::Arc;

use hbui_ipc::{Bridge, RequestError, Target, VERSION};
use serde_json::{json, Value};

use crate::wire::{
    negotiate_version, CallParams, CallResult, Request, Response, ToolInfo, INVALID_PARAMS,
    METHOD_NOT_FOUND,
};

pub const NAME: &str = "hbui-mcp-bridge";

const INSTRUCTIONS: &str = "\
This server bridges to live user interfaces — hbui applications — that a person may be \
using at the same time as you. Applications may be restarted at any moment (they are being \
developed); this connection stays up regardless.

Start with list_sessions: one row per application, named by `session`. Name the session \
on every other call; there is no current session. `pid` works as a selector too, but \
sessions survive restarts and pids do not.

Read a UI with get_view: widgets by stable id, each with a role, its state and the actions \
it accepts; the layout; the focused widget; the open modal; the commands. Every view \
carries an `instance` (which run of the application) and a `revision` (which state of \
that run). Never guess coordinates or keys.

Change it with dispatch, one semantic action at a time, naming widgets and items by id. \
Pass expected_instance and expected_revision from the view you acted on. stale_instance \
means the application restarted; stale_revision means the person changed something. In \
both cases read the view again. The result lists exactly what changed; an empty list means \
the action had no visible effect.

If get_view reports status application_disconnected, that application is not running — \
the bridge is fine. A last_view marked stale is its state just before it went away, for \
diagnosis only; do not act on it.";

pub struct Server {
    bridge: Arc<Bridge>,
    version: String,
}

impl Server {
    pub fn new(bridge: Arc<Bridge>, version: impl Into<String>) -> Self {
        Self {
            bridge,
            version: version.into(),
        }
    }

    pub fn bridge(&self) -> &Arc<Bridge> {
        &self.bridge
    }

    /// Answer one JSON-RPC message. `None` for a notification.
    pub fn handle(&self, req: Request) -> Option<Response> {
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let result = match req.method.as_str() {
            "initialize" => json!({
                "protocolVersion": negotiate_version(req.params.as_ref()),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": NAME, "version": self.version },
                "instructions": INSTRUCTIONS,
            }),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": tools() }),
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                match serde_json::from_value::<CallParams>(params) {
                    Ok(call) => json!(self.call(&call.name, &call.arguments)),
                    Err(e) => {
                        return Some(Response::error(
                            id,
                            INVALID_PARAMS,
                            format!("bad tools/call: {e}"),
                        ))
                    }
                }
            }
            other => {
                return Some(Response::error(
                    id,
                    METHOD_NOT_FOUND,
                    format!("no method {other:?}"),
                ))
            }
        };
        Some(Response::success(id, result))
    }

    pub fn call(&self, name: &str, args: &Value) -> CallResult {
        // Look before answering, so an application started a moment ago is
        // already in the list.
        self.bridge.scan();
        match name {
            "list_sessions" => CallResult::json(&json!({
                "sessions": self.bridge.list().iter().map(|s| s.to_json()).collect::<Vec<_>>(),
            })),
            "get_view" => self.with_session(args, |s| self.get_view(s, args)),
            "dispatch" => self.with_session(args, |s| self.dispatch(s, args)),
            "capture_view" => self.with_session(args, |s| self.capture_view(s, args)),
            other => CallResult::failure(&json!({
                "ok": false,
                "error": "unknown_tool",
                "message": format!("no tool {other:?}"),
            })),
        }
    }

    /// Resolve the `session` or `pid` a call names, or refuse it with the
    /// sessions that do exist.
    fn with_session(&self, args: &Value, f: impl FnOnce(&str) -> CallResult) -> CallResult {
        let target = match (args.get("session").and_then(Value::as_str), args.get("pid")) {
            (Some(s), _) => Target::Session(s.to_string()),
            (None, Some(p)) => match p.as_u64().and_then(|p| u32::try_from(p).ok()) {
                Some(p) => Target::Pid(p),
                None => return self.no_session("session_required", "pid must be a number"),
            },
            (None, None) => {
                return self.no_session(
                    "session_required",
                    "name the application with `session` (see list_sessions)",
                )
            }
        };
        match self.bridge.resolve(&target) {
            Some(session) => f(&session),
            None => self.no_session("unknown_session", &format!("no session matches {target:?}")),
        }
    }

    fn no_session(&self, error: &str, message: &str) -> CallResult {
        let names: Vec<String> = self.bridge.list().into_iter().map(|s| s.session).collect();
        CallResult::failure(&json!({
            "ok": false,
            "error": error,
            "message": message,
            "sessions": names,
        }))
    }

    fn get_view(&self, session: &str, args: &Value) -> CallResult {
        let since = args.get("since").and_then(Value::as_u64);
        let instance = args.get("instance").and_then(Value::as_u64);
        // A revision only means something within its instance: revision 3 of
        // the last run is not revision 3 of this one. So `since` is honoured
        // only when the caller says which instance it belongs to, and that
        // instance is still the one running.
        let attempt = match (since, instance) {
            (Some(since), Some(instance)) => {
                match self.bridge.request(
                    session,
                    Some(instance),
                    "get_view",
                    json!({ "since": since }),
                ) {
                    Err(RequestError::StaleInstance { .. }) => None,
                    other => Some(other),
                }
            }
            _ => None,
        };
        let result =
            attempt.unwrap_or_else(|| self.bridge.request(session, None, "get_view", json!({})));
        match result {
            Ok((instance, Ok(view))) => {
                let mut view = labelled(view, session, instance);
                view["status"] = json!("ready");
                CallResult::json(&view)
            }
            Ok((instance, Err(e))) => CallResult::failure(&labelled(e, session, instance)),
            Err(e) => match self.unreachable(session, &e) {
                // An absent application is a state to report, not a failure.
                v if v["status"].is_string() => CallResult::json(&v),
                v => CallResult::failure(&v),
            },
        }
    }

    fn dispatch(&self, session: &str, args: &Value) -> CallResult {
        let mut action = args.clone();
        let expected = action.as_object_mut().and_then(|o| {
            o.remove("session");
            o.remove("pid");
            o.remove("expected_instance")
        });
        let expected = expected.and_then(|v| v.as_u64());
        match self.bridge.request(session, expected, "dispatch", action) {
            Ok((instance, Ok(outcome))) => CallResult::json(&labelled(outcome, session, instance)),
            Ok((instance, Err(e))) => CallResult::failure(&labelled(e, session, instance)),
            Err(e) => CallResult::failure(&refusal(self.unreachable(session, &e))),
        }
    }

    fn capture_view(&self, session: &str, args: &Value) -> CallResult {
        let size = json!({ "width": args.get("width"), "height": args.get("height") });
        match self.bridge.request(session, None, "capture_view", size) {
            Ok((instance, Ok(v))) => CallResult::text(format!(
                "session {session} instance {instance} revision {}\n{}",
                v["revision"],
                v["text"].as_str().unwrap_or("")
            )),
            Ok((instance, Err(e))) => CallResult::failure(&labelled(e, session, instance)),
            Err(e) => CallResult::failure(&refusal(self.unreachable(session, &e))),
        }
    }

    /// Describe why `session` could not be asked. States — the application
    /// is down, or speaks another version — carry a `status`; failures of
    /// the call itself carry an `error`.
    fn unreachable(&self, session: &str, e: &RequestError) -> Value {
        match e {
            RequestError::Unavailable { snapshot } => {
                let mut v = json!({
                    "status": "application_disconnected",
                    "session": session,
                    "application": { "connected": false },
                });
                match snapshot {
                    // It was here. Show what it last looked like — flagged,
                    // so it is never taken for the present.
                    Some((instance, view)) => {
                        v["stale"] = json!(true);
                        v["last_view"] = labelled(view.clone(), session, *instance);
                        v["message"] = json!(
                            "the application is not running; last_view is its state before it went away"
                        );
                    }
                    None => v["message"] = json!("the application is not running"),
                }
                v
            }
            RequestError::Mismatch {
                application_version,
            } => json!({
                "status": "protocol_mismatch",
                "session": session,
                "bridge_version": VERSION,
                "application_version": application_version,
                "message": "the application speaks another hbui-ipc version; rebuild it or restart the bridge",
            }),
            RequestError::UnknownSession => json!({
                "ok": false,
                "error": "unknown_session",
                "message": format!("no session {session:?}"),
            }),
            RequestError::StaleInstance { current } => json!({
                "ok": false,
                "error": "stale_instance",
                "session": session,
                "current_instance": current,
                "message": format!("the application restarted (now instance {current}); read the view again before acting"),
            }),
            RequestError::Disconnected => json!({
                "ok": false,
                "error": "application_disconnected",
                "session": session,
                "outcome": "unknown",
                "message": "the application went away while handling this; it may or may not have been applied",
            }),
            RequestError::Timeout => json!({
                "ok": false,
                "error": "application_timeout",
                "session": session,
                "outcome": "unknown",
                "message": "the application did not answer in time; it may or may not have been applied",
            }),
        }
    }
}

/// An unreachable state turned into a refusal, for calls that needed the
/// application to act: the state's name becomes the error, and a stale view
/// is left out — it cannot help an action that was not taken.
fn refusal(mut v: Value) -> Value {
    if let Some(status) = v.get("status").cloned() {
        if let Some(o) = v.as_object_mut() {
            o.remove("status");
            o.remove("last_view");
            o.remove("stale");
        }
        v["ok"] = json!(false);
        v["error"] = status;
    }
    v
}

fn labelled(mut v: Value, session: &str, instance: u64) -> Value {
    if v.is_object() {
        v["session"] = json!(session);
        v["instance"] = json!(instance);
    }
    v
}

fn target_props() -> Value {
    json!({
        "session": { "type": "string", "description": "The session to act on (see list_sessions)." },
        "pid": { "type": "integer", "description": "Alternatively, the application's process id." },
    })
}

fn with_target(mut schema: Value) -> Value {
    let props = schema["properties"]
        .as_object_mut()
        .expect("an object schema");
    for (k, v) in target_props().as_object().unwrap() {
        props.insert(k.clone(), v.clone());
    }
    schema
}

fn tools() -> Vec<ToolInfo> {
    vec![
        ToolInfo {
            name: "list_sessions",
            description: "List the hbui applications this bridge can reach: session, application, \
                          pid, cwd, instance, status (ready / disconnected / protocol_mismatch) \
                          and the last revision seen.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolInfo {
            name: "get_view",
            description: "Read a session's UI as structured data: widgets by id (role, state, \
                          accepted actions), layout, focus, open modal, commands, plus `instance` \
                          and `revision`. With `since` and `instance`, return only the changes after \
                          that revision; far back, they come `merged: true` (replace / remove with \
                          current values). If the application is not running, `status` says so.",
            input_schema: with_target(json!({
                "type": "object",
                "properties": {
                    "since": { "type": "integer", "description": "A revision you read before." },
                    "instance": { "type": "integer", "description": "The instance that revision belongs to." },
                },
            })),
        },
        ToolInfo {
            name: "dispatch",
            description: "Run one semantic action in a session. Returns the instance, the new \
                          revision and the list of changes. Refused with stale_instance / \
                          stale_revision if the expectations are not current.\n\
                          focus{target} · select{target,item} · activate{target,item?} · \
                          set_text{target,value} · set_checked{target,checked} · expand{target,item} · \
                          collapse{target,item} · \
                          invoke{command} · close_modal{}",
            input_schema: with_target(json!({
                "type": "object",
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["focus", "select", "activate", "set_text", "set_checked", "expand", "collapse", "invoke", "close_modal"],
                    },
                    "target": { "type": "string", "description": "Widget id (or tab-set id for select)." },
                    "item": { "type": "string", "description": "Item id within the target." },
                    "value": { "type": "string", "description": "The full new text, for set_text." },
                    "checked": { "type": "boolean", "description": "On or off, for set_checked." },
                    "command": { "type": "string", "description": "Command id, for invoke." },
                    "expected_instance": { "type": "integer", "description": "The instance you last read." },
                    "expected_revision": { "type": "integer", "description": "The revision you last read." },
                },
                "required": ["type"],
            })),
        },
        ToolInfo {
            name: "capture_view",
            description: "Render a session's UI as the person's screen would draw it, as text. \
                          For checking what is drawn; work from get_view, not from this.",
            input_schema: with_target(json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "default": 80 },
                    "height": { "type": "integer", "default": 24 },
                },
            })),
        },
    ]
}
