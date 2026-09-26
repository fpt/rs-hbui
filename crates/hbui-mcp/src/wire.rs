//! MCP / JSON-RPC 2.0 wire types for a tools-only server.
//!
//! Hand-rolled rather than taken from an SDK, for the reason rs-voxeler's are:
//! an SDK's value is its macros over typed Rust functions, these tools are
//! dispatched against hand-written schemas, and an SDK would bring an async
//! runtime into a library whose event loop is a plain blocking one.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The revision this server implements. The tools-only surface is the same
/// across recent revisions, so [`negotiate_version`] echoes the client's.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const JSONRPC_VERSION: &str = "2.0";

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_PARAMS: i32 = -32602;
pub const METHOD_NOT_FOUND: i32 = -32601;

/// A JSON-RPC request, or a notification when `id` is absent.
#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

#[derive(Debug, Serialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
}

impl Response {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
            id,
            result: None,
            error: Some(ErrorObject {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// The result of a `tools/call`.
///
/// `isError` is not a JSON-RPC error: a tool that ran and was refused says so
/// here, where the model reads it and can correct itself. JSON-RPC errors are
/// for protocol faults.
#[derive(Debug, Clone, Serialize)]
pub struct CallResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text { text: String },
}

impl CallResult {
    pub fn json(value: &Value) -> Self {
        Self {
            content: vec![Content::Text {
                text: value.to_string(),
            }],
            is_error: None,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: None,
        }
    }

    pub fn failure(value: &Value) -> Self {
        Self {
            is_error: Some(true),
            ..Self::json(value)
        }
    }
}

pub fn negotiate_version(params: Option<&Value>) -> String {
    params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION)
        .to_string()
}
