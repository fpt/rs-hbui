//! The MCP tools, forwarded to whichever application is connected.
//!
//! `get_view`, `dispatch` and `capture_view` are the application's; the bridge
//! adds the `instance` they came from, refuses `stale_instance`, and answers
//! for the application when it is not there. Answers, not fails: an
//! application that is down is a state the agent reads in a normal result,
//! never a transport error that looks like the bridge broke.

use std::sync::Arc;

use hbui_ipc::{Bridge, RequestError, VERSION};
use serde_json::{json, Value};

use crate::wire::{
    negotiate_version, CallParams, CallResult, Request, Response, ToolInfo, INVALID_PARAMS,
    METHOD_NOT_FOUND,
};

pub const NAME: &str = "hbui-mcp-bridge";

const INSTRUCTIONS: &str = "\
This server is a bridge to a live user interface that a person may be using at the same \
time as you. The application behind it may be restarted at any moment (it is being \
developed); this connection stays up regardless.

Read the UI with get_view: widgets by stable id, each with a role, its state and the \
actions it accepts; the layout; the focused widget; the open modal; the commands. Every \
view carries an `instance` (which run of the application) and a `revision` (which state of \
that run). Never guess coordinates or keys.

Change it with dispatch, one semantic action at a time, naming widgets and items by id. \
Pass expected_instance and expected_revision from the view you acted on. stale_instance \
means the application restarted; stale_revision means the person changed something. In \
both cases read the view again. The result lists exactly what changed; an empty list means \
the action had no visible effect.

If get_view reports status application_unavailable or application_disconnected, the \
application is not running — the bridge is fine. A last_view marked stale is the state \
just before it went away, for diagnosis only; do not act on it.";

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
        match name {
            "get_view" => self.get_view(args),
            "dispatch" => self.dispatch(args),
            "capture_view" => self.capture_view(args),
            other => CallResult::failure(&json!({
                "ok": false,
                "error": "unknown_tool",
                "message": format!("no tool {other:?}"),
            })),
        }
    }

    fn get_view(&self, args: &Value) -> CallResult {
        let since = args.get("since").and_then(Value::as_u64);
        let instance = args.get("instance").and_then(Value::as_u64);
        // A revision only means something within its instance: revision 3 of
        // the last run is not revision 3 of this one. So `since` is honoured
        // only when the caller says which instance it belongs to, and that
        // instance is still the one connected.
        let attempt = match (since, instance) {
            (Some(since), Some(instance)) => {
                match self
                    .bridge
                    .request(Some(instance), "get_view", json!({ "since": since }))
                {
                    Err(RequestError::StaleInstance { .. }) => None,
                    other => Some(other),
                }
            }
            _ => None,
        };
        let result = attempt.unwrap_or_else(|| self.bridge.request(None, "get_view", json!({})));
        match result {
            Ok((instance, Ok(mut view))) => {
                view["instance"] = json!(instance);
                view["status"] = json!("ready");
                CallResult::json(&view)
            }
            Ok((instance, Err(e))) => CallResult::failure(&with_instance(e, instance)),
            Err(e) => CallResult::json(&self.unavailable(&e)),
        }
    }

    fn dispatch(&self, args: &Value) -> CallResult {
        let mut action = args.clone();
        let expected = action
            .as_object_mut()
            .and_then(|o| o.remove("expected_instance"))
            .and_then(|v| v.as_u64());
        match self.bridge.request(expected, "dispatch", action) {
            Ok((instance, Ok(outcome))) => CallResult::json(&with_instance(outcome, instance)),
            Ok((instance, Err(e))) => CallResult::failure(&with_instance(e, instance)),
            Err(e) => CallResult::failure(&self.refusal(&e)),
        }
    }

    fn capture_view(&self, args: &Value) -> CallResult {
        match self.bridge.request(None, "capture_view", args.clone()) {
            Ok((instance, Ok(v))) => CallResult::text(format!(
                "instance {instance} revision {}\n{}",
                v["revision"],
                v["text"].as_str().unwrap_or("")
            )),
            Ok((instance, Err(e))) => CallResult::failure(&with_instance(e, instance)),
            Err(e) => CallResult::failure(&self.refusal(&e)),
        }
    }

    /// What `get_view` says when it cannot reach the application.
    fn unavailable(&self, e: &RequestError) -> Value {
        let status = self.bridge.status();
        if let Some(m) = status.mismatch {
            return json!({
                "status": "protocol_mismatch",
                "bridge_version": VERSION,
                "application_version": m.application_version,
                "message": "the application speaks another hbui-ipc version; rebuild it or restart the bridge",
            });
        }
        match status.snapshot {
            // It was here. Show what it last looked like — flagged, so it is
            // never taken for the present.
            Some(snap) => {
                let mut last = snap.view;
                last["instance"] = json!(snap.instance);
                json!({
                    "status": "application_disconnected",
                    "application": { "connected": false, "name": snap.application },
                    "stale": true,
                    "last_view": last,
                    "message": match e {
                        RequestError::Timeout => "the application stopped answering",
                        _ => "the application is not running; last_view is its state before it went away",
                    },
                })
            }
            None => json!({
                "status": "application_unavailable",
                "application": { "connected": false },
                "message": "no application has connected to the bridge yet",
            }),
        }
    }

    /// The refusal for an action or capture that could not reach the
    /// application.
    fn refusal(&self, e: &RequestError) -> Value {
        match e {
            RequestError::StaleInstance { current } => json!({
                "ok": false,
                "error": "stale_instance",
                "current_instance": current,
                "message": format!("the application restarted (now instance {current}); read the view again before acting"),
            }),
            RequestError::Unavailable => {
                let mut v = self.unavailable(e);
                v.as_object_mut().map(|o| o.remove("last_view"));
                v["ok"] = json!(false);
                v["error"] = v["status"].clone();
                v
            }
            RequestError::Disconnected => json!({
                "ok": false,
                "error": "application_disconnected",
                "outcome": "unknown",
                "message": "the application went away while handling this; it may or may not have been applied",
            }),
            RequestError::Timeout => json!({
                "ok": false,
                "error": "application_timeout",
                "outcome": "unknown",
                "message": "the application did not answer in time; it may or may not have been applied",
            }),
        }
    }
}

fn with_instance(mut v: Value, instance: u64) -> Value {
    if v.is_object() {
        v["instance"] = json!(instance);
    }
    v
}

fn tools() -> Vec<ToolInfo> {
    vec![
        ToolInfo {
            name: "get_view",
            description: "Read the UI as structured data: widgets by id (role, state, accepted actions), \
                          layout, focus, open modal, commands, plus `instance` and `revision`. With \
                          `since` and `instance`, return only the changes after that revision. If the \
                          application is not running, `status` says so.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "since": { "type": "integer", "description": "A revision you read before." },
                    "instance": { "type": "integer", "description": "The instance that revision belongs to." },
                },
            }),
        },
        ToolInfo {
            name: "dispatch",
            description: "Run one semantic action. Returns the instance, the new revision and the list \
                          of changes. Refused with stale_instance / stale_revision if the expectations \
                          are not current.\n\
                          focus{target} · select{target,item} · activate{target,item?} · \
                          set_text{target,value} · expand{target,item} · collapse{target,item} · \
                          invoke{command} · close_modal{}",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": ["focus", "select", "activate", "set_text", "expand", "collapse", "invoke", "close_modal"],
                    },
                    "target": { "type": "string", "description": "Widget id (or tab-set id for select)." },
                    "item": { "type": "string", "description": "Item id within the target." },
                    "value": { "type": "string", "description": "The full new text, for set_text." },
                    "command": { "type": "string", "description": "Command id, for invoke." },
                    "expected_instance": { "type": "integer", "description": "The instance you last read." },
                    "expected_revision": { "type": "integer", "description": "The revision you last read." },
                },
                "required": ["type"],
            }),
        },
        ToolInfo {
            name: "capture_view",
            description: "Render the UI as the person's screen would draw it, as text. For checking what \
                          is drawn; work from get_view, not from this.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "default": 80 },
                    "height": { "type": "integer", "default": 24 },
                },
            }),
        },
    ]
}
