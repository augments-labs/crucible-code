//! JSON-RPC 2.0, as the Agent Client Protocol carries it: one object a line.
//!
//! A frame is parsed here once into what it is, a response, a request or a
//! notification, and nothing past this module reads the raw line. An error an
//! agent responds with keeps its code and a clipped message, since the message
//! is the agent's own words and is shown, never followed.

use crucible_execution::{Failure, Text};
use serde_json::{Map, Value, json};

/// The most bytes an agent's error message keeps.
const MESSAGE_BYTES: usize = 1024;

/// One frame from the agent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Frame {
    /// The answer to a request this client sent.
    Response {
        /// The id of the request it answers.
        id: Value,
        /// What it carried: a result, or an error.
        outcome: Result<Value, Refused>,
    },
    /// A request the agent sends, which waits for an answer.
    Request {
        /// Its id, which the answer carries.
        id: Value,
        /// What it asks.
        method: String,
        /// With what.
        params: Value,
    },
    /// Something the agent says that waits for nothing.
    Notification {
        /// What it says.
        method: String,
        /// With what.
        params: Value,
    },
}

/// An error an agent responded with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused {
    /// Its code.
    pub(crate) code: i64,
    /// Its message, clipped.
    pub(crate) message: Text,
}

/// What `line` is.
///
/// # Errors
///
/// A [`Failure`] where it is not one JSON-RPC 2.0 object.
pub(crate) fn parse(line: &str) -> Result<Frame, Failure> {
    let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(line) else {
        return Err(Failure::new(
            "The agent sent a line that is not a JSON object.",
        ));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(Failure::new(
            "The agent sent a frame that is not JSON-RPC 2.0.",
        ));
    }
    let id = object.remove("id");
    let method = object.remove("method");
    let params = object.remove("params").unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(Value::String(method))) => Ok(Frame::Request { id, method, params }),
        (None, Some(Value::String(method))) => Ok(Frame::Notification { method, params }),
        (Some(id), None) => Ok(Frame::Response {
            id,
            outcome: outcome(&mut object)?,
        }),
        _ => Err(Failure::new(
            "The agent sent a frame that is neither a request nor a response.",
        )),
    }
}

/// A response's result or error.
fn outcome(object: &mut Map<String, Value>) -> Result<Result<Value, Refused>, Failure> {
    if let Some(result) = object.remove("result") {
        return Ok(Ok(result));
    }
    let Some(Value::Object(error)) = object.remove("error") else {
        return Err(Failure::new(
            "The agent sent a response with neither a result nor an error.",
        ));
    };
    let code = error
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(Err(Refused {
        code,
        message: Text::clipped(message, MESSAGE_BYTES),
    }))
}

/// The request `method` with `params`, under `id`.
pub(crate) fn request(id: u64, method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// The notification `method` with `params`.
pub(crate) fn notification(method: &str, params: &Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}
