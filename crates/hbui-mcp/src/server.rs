//! The tools, and the JSON-RPC methods around them.
//!
//! Two tools do the work: `get_view` reads the semantic view, `dispatch` runs
//! a semantic action. A third, `capture_view`, is there only if the host
//! supplies a renderer: it returns what the person's screen shows, as text,
//! for when an agent needs to check the *drawing* rather than the state. It
//! is an observation to reach for, not the view to work from.

use std::sync::Arc;

use hbui_core::{ActionRequest, Shared, UiState};
use serde_json::{json, Value};

use crate::wire::{
    negotiate_version, CallParams, CallResult, Request, Response, ToolInfo, INVALID_PARAMS,
    METHOD_NOT_FOUND,
};

/// Renders a state as text at a size, for `capture_view`.
pub type Capture = Arc<dyn Fn(&UiState, u16, u16) -> String + Send + Sync>;

const INSTRUCTIONS: &str = "\
This server is a live user interface that a person may be using at the same time as you.

Read it with get_view: the result is the UI as structured data — widgets by stable id, \
each with a role, its state and the actions it accepts; the layout; the focused widget; \
the open modal, if any; and the screen-level commands. Never guess coordinates or keys.

Change it with dispatch, one semantic action at a time, naming widgets and items by id. \
Pass expected_revision = the revision you last read: if the person changed the UI in \
between, the action is refused with stale_view and you should read again. The result \
lists exactly what changed; an empty list means the action had no visible effect.

While a modal is open it is the only thing that accepts actions. Act inside it, or \
close_modal. get_view with since=<revision> returns only what changed since then.";

pub struct Server {
    shared: Shared,
    name: String,
    version: String,
    capture: Option<Capture>,
}

impl Server {
    pub fn new(shared: Shared, name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            shared,
            name: name.into(),
            version: version.into(),
            capture: None,
        }
    }

    /// Offer `capture_view`, rendering with `capture`.
    pub fn with_capture(mut self, capture: Capture) -> Self {
        self.capture = Some(capture);
        self
    }

    pub fn shared(&self) -> &Shared {
        &self.shared
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
                "serverInfo": { "name": self.name, "version": self.version },
                "instructions": INSTRUCTIONS,
            }),
            "ping" => json!({}),
            "tools/list" => json!({ "tools": self.tools() }),
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
            "get_view" => {
                let session = self.shared.lock();
                match args.get("since").and_then(Value::as_u64) {
                    Some(since) => CallResult::json(&session.view_since(since)),
                    None => CallResult::json(&session.view()),
                }
            }
            "dispatch" => {
                let req: ActionRequest = match serde_json::from_value(args.clone()) {
                    Ok(r) => r,
                    Err(e) => {
                        return CallResult::failure(&json!({
                            "ok": false,
                            "error": "bad_action",
                            "message": format!("not an action: {e}"),
                        }))
                    }
                };
                match self.shared.lock().dispatch(req) {
                    Ok(outcome) => CallResult::json(&json!(outcome)),
                    Err(e) => CallResult::failure(&e.to_json()),
                }
            }
            "capture_view" if self.capture.is_some() => {
                let capture = self.capture.as_ref().expect("checked by the guard");
                let dim = |k: &str, d: u16| {
                    args.get(k)
                        .and_then(Value::as_u64)
                        .map_or(d, |v| v.clamp(10, 500) as u16)
                };
                let session = self.shared.lock();
                let text = capture(session.ui(), dim("width", 80), dim("height", 24));
                CallResult::text(format!("revision {}\n{text}", session.revision()))
            }
            other => CallResult::failure(&json!({
                "ok": false,
                "error": "unknown_tool",
                "message": format!("no tool {other:?}"),
            })),
        }
    }

    fn tools(&self) -> Vec<ToolInfo> {
        let mut tools = vec![
            ToolInfo {
                name: "get_view",
                description: "Read the UI as structured data: widgets by id (role, state, accepted actions), \
                              layout, focus, open modal, commands, and the current revision. With `since`, \
                              return only the changes after that revision.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "since": { "type": "integer", "description": "A revision you read before." }
                    },
                }),
            },
            ToolInfo {
                name: "dispatch",
                description: "Run one semantic action. Returns the new revision and the list of changes; \
                              refused with stale_view if expected_revision is not current.\n\
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
                        "expected_revision": { "type": "integer", "description": "The revision you last read." },
                    },
                    "required": ["type"],
                }),
            },
        ];
        if self.capture.is_some() {
            tools.push(ToolInfo {
                name: "capture_view",
                description:
                    "Render the UI as the person's terminal would draw it, as text. For checking \
                              what is drawn; work from get_view, not from this.",
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "width": { "type": "integer", "default": 80 },
                        "height": { "type": "integer", "default": 24 },
                    },
                }),
            });
        }
        tools
    }
}
