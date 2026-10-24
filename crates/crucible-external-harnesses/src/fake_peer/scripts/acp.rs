//! Replies in the Agent Client Protocol, as an agent speaking it would give.
//!
//! The client numbers its requests from zero in each launch: initializing is
//! `0`, opening the session `1` and the first prompt `2`, so a reply answers
//! by those numbers. An agent described here is the release a dialect admits,
//! with every capability a client relies on.

use serde_json::{Value, json};

use crate::conformance::Set;
use crate::fake_peer::Reply;
use crucible_execution::Decision;

/// The session every opened session is called.
pub const SESSION: &str = "s";

/// A response to request `id` carrying `result`.
#[must_use]
pub fn response(id: u64, result: &Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// An error response to request `id`.
#[must_use]
pub fn refused(id: u64, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// The answer to initializing of an agent called `name` at `version`, which
/// signs in with `sign_in`.
#[must_use]
pub fn initialized(name: &str, version: &str, sign_in: &str) -> String {
    response(
        0,
        &json!({
            "protocolVersion": 1,
            "agentInfo": {"name": name, "version": version},
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}},
                "auth": {"logout": {}},
            },
            "authMethods": [{"id": sign_in, "name": "Sign in"}],
        }),
    )
}

/// The answer to initializing of the agent the catalog intake's fixtures
/// describe: it signs in in a browser or at a terminal.
#[must_use]
pub fn probed() -> String {
    response(
        0,
        &json!({
            "protocolVersion": 1,
            "agentInfo": {"name": "fake-agent", "version": "1.0.0"},
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}},
                "auth": {"logout": {}},
            },
            "authMethods": [
                {"id": "browser", "name": "Sign in with a browser"},
                {
                    "type": "terminal",
                    "id": "terminal",
                    "name": "Sign in at a terminal",
                    "command": "agent",
                    "args": ["login"],
                    "env": {"LOGIN_MODE": "1"},
                },
            ],
        }),
    )
}

/// The answer to opening a session, as request `id`.
#[must_use]
pub fn opened(id: u64) -> String {
    response(id, &json!({"sessionId": SESSION}))
}

/// The refusal of request `id` for want of a sign-in.
#[must_use]
pub fn sign_in_required(id: u64) -> String {
    refused(id, -32000, "Authentication required")
}

/// The refusal of request `id`, which asked for a session the agent kept, for
/// want of any session by that name: the protocol's code for a resource not
/// found, as the reference adapter refuses a session it cannot load.
#[must_use]
pub fn session_gone(id: u64) -> String {
    refused(id, -32002, "Resource not found")
}

/// The end of prompt `id`, for `reason`.
#[must_use]
pub fn stopped(id: u64, reason: &str) -> String {
    response(id, &json!({"stopReason": reason}))
}

/// The session update `update`.
#[must_use]
pub fn update(update: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": SESSION, "update": update},
    })
    .to_string()
}

/// More of the answer: `text`.
#[must_use]
pub fn said(text: &str) -> String {
    update(&json!({
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": text},
    }))
}

/// The session's running cost, `amount` dollars.
#[must_use]
pub fn spent(amount: f64) -> String {
    update(&json!({
        "sessionUpdate": "usage_update",
        "used": 0,
        "size": 0,
        "cost": {"amount": amount, "currency": "USD"},
    }))
}

/// A question, under the request id `native`, whether to run a command.
#[must_use]
pub fn asked(native: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": native,
        "method": "session/request_permission",
        "params": {
            "sessionId": SESSION,
            "toolCall": {
                "toolCallId": "t",
                "title": "Read the notes",
                "kind": "read",
                "rawInput": {"path": "notes.md"},
            },
            "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
            ],
        },
    })
    .to_string()
}

/// What the agent must hear when the question `native` is decided
/// `decision`: the option it offered for an allow, and a cancelled outcome
/// for a deny, since the suite denies only a question asked after its turn
/// was stopped.
#[must_use]
pub fn answer(native: &str, decision: &Decision) -> String {
    let outcome = match decision {
        Decision::Allow(crucible_execution::Lasting::Once) => {
            json!({"outcome": "selected", "optionId": "allow"})
        }
        Decision::Allow(_) => json!({"outcome": "selected", "optionId": "always"}),
        Decision::Deny | Decision::Answer(_) => json!({"outcome": "cancelled"}),
    };
    json!({"jsonrpc": "2.0", "id": native, "result": {"outcome": outcome}}).to_string()
}

/// A reply that opens a turn for an agent `initialized` describes: it hears
/// initializing and answers, hears the session opened and answers, then hears
/// the prompt.
#[must_use]
pub fn opening(initialized: &str) -> Reply {
    Reply::new()
        .hear(1)
        .say(initialized)
        .hear(1)
        .say(opened(1))
        .hear(1)
}

/// The conformance set of an agent `initialized` describes.
#[must_use]
pub fn set(initialized: String) -> Set {
    Set {
        opening: vec![
            (1, vec![initialized]),
            (1, vec![opened(1)]),
            (1, Vec::new()),
        ],
        effect: update(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t",
            "title": "Edit src/lib.rs",
            "kind": "edit",
            "status": "in_progress",
        })),
        ask: asked,
        answer,
        answered: stopped(2, "end_turn"),
        cancelled: stopped(2, "cancelled"),
        stop: Some(
            json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": SESSION}})
                .to_string(),
        ),
    }
}
