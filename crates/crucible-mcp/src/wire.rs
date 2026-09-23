//! What one MCP frame says.
//!
//! [`Frames`](crucible_transport::Frames) finds the boundaries; this reads what is
//! inside them. MCP is JSON-RPC 2.0 over a pipe, one document per line, and the
//! part that matters here is that every field arriving is a field somebody
//! else's program wrote. Nothing is believed because it is well-formed.
//!
//! Crucible numbers its own calls and never accepts an answer to a call it did
//! not make, so an identifier is read as a number and a frame carrying anything
//! else in that place is refused rather than matched loosely. The standard
//! allows a string there; crucible never sends one, so a string can only be an
//! answer to somebody else's question.
//!
//! What rides inside `params` and `result` is carried and never read. Those
//! names belong to whoever wrote the method, and crucible refusing a spelling
//! out of a vocabulary it was never shown would be crucible deciding what a
//! protocol it is a client of meant.
//!
//! What a frame says that crucible keeps as words — a failure's message, and
//! the spelling of an identifier it could not have issued, digits included —
//! has what the server was given in confidence hidden in it as it is read; see
//! [`Withheld`]. Everything else is read as sent: the version member, every
//! number crucible reads, a result, which is hidden by whoever reads it, a
//! notification, which is dropped, and the name of a question, which only goes
//! back to the server that asked it.

use std::fmt;

use serde_json::{Map, Value, json};

use crate::withheld::{Indistinct, Withheld};

/// The version of JSON-RPC every MCP frame states.
pub const RPC: &str = "2.0";

/// The most bytes a method name, or the words of a failure, may carry.
///
/// Far past any method the protocol names and any sentence a person reads, and
/// held all the same. Both are text off a pipe that crucible keeps: a method
/// name goes into whatever this crate is matching on, and a failure's words go
/// on a screen.
pub const SAID_BYTES: usize = 4 * 1024;

/// The code JSON-RPC reserves for a method the receiver does not implement.
///
/// Crucible implements none of what a server may ask it, and says so with the
/// number the standard gives rather than by going quiet — a server waiting on
/// an answer that will never come is a server that never gets to its own work.
pub const NO_SUCH_METHOD: i64 = -32601;

/// Which call an exchange belongs to.
///
/// Crucible numbers its own, so two of these are equal only among the calls
/// crucible made. Matching an answer to a request is the caller's job, because
/// only the caller knows what it asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Call(u64);

impl Call {
    /// The call this number names.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    #[must_use]
    pub const fn number(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Call {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(form, "{}", self.0)
    }
}

/// Why a frame was not a message crucible could act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Garbled {
    /// The frame was not JSON at all.
    #[error("the server sent a frame that is not JSON: {said}")]
    Unparsed {
        /// What the parser made of it.
        said: Box<str>,
    },

    /// The frame was JSON, but not an object.
    #[error("the server sent {found} where a JSON-RPC message belongs")]
    NotAMessage {
        /// What arrived instead.
        found: &'static str,
    },

    /// The frame did not say it was JSON-RPC 2.0.
    ///
    /// Refused rather than assumed. The member is one byte of agreement about
    /// which protocol is being spoken, and a frame that leaves it out is a
    /// frame crucible cannot say it understood.
    #[error("the server sent a frame with no jsonrpc: \"{RPC}\" member")]
    Unversioned,

    /// A frame was none of a request, an answer or a notification.
    ///
    /// A message carries a method or settles an identifier. One carrying
    /// neither, or carrying a result and a failure at once, is not a shape the
    /// standard has and is not one to guess at.
    #[error("the server sent a frame that is neither a request, an answer nor a notification")]
    Shapeless,

    /// A field held the wrong kind of value.
    #[error("the server sent {field} as {found} rather than {wanted}")]
    WrongKind {
        /// Which field.
        field: &'static str,
        /// What arrived.
        found: &'static str,
        /// What the standard says belongs there.
        wanted: &'static str,
    },

    /// A retained spelling ran past its ceiling.
    #[error("the server sent {field} as {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its ceiling.
        maximum: usize,
        /// What arrived.
        actual: usize,
    },

    /// An identifier was not a number crucible could have issued.
    ///
    /// Crucible only ever sends a non-negative integer, so anything else in
    /// that place settles a call crucible did not make.
    #[error("the server answered a call identified as {found}, which crucible does not issue")]
    NotACall {
        /// The identifier as it was written.
        found: Box<str>,
    },
}

/// What one frame from a server said.
///
/// Its `Debug` says what kind of frame it was and how much it carried, and
/// never a word of what the server wrote there that crucible has not hidden.
#[derive(Clone, PartialEq, Eq)]
pub enum Heard {
    /// An answer to a call crucible made.
    Answer {
        /// Which call it settles.
        call: Call,
        /// What it says.
        reply: Reply,
    },

    /// A question the server is waiting on crucible to answer.
    ///
    /// Kept as its identifier and its name, and no further. Crucible answers
    /// every one of them the same way, so the parameters are never read.
    Asked {
        /// Which call an answer has to settle.
        call: Call,
        /// What it asked for.
        method: Box<str>,
    },

    /// Something the server said that expects nothing back.
    Told {
        /// What it was about.
        method: Box<str>,
        /// What it carried, unread.
        params: Value,
    },
}

/// What an answer settled a call with.
///
/// Its `Debug` names a result's kind and never its contents, which are read
/// and hidden by whoever asked.
#[derive(Clone, PartialEq, Eq)]
pub enum Reply {
    /// It worked, and this is what came back, unread.
    Worked(Value),

    /// It did not, and this is what the server said about it.
    Failed {
        /// The code the server gave.
        code: i64,
        /// The words it gave, which are its own and go on a screen.
        said: Box<str>,
    },
}

impl fmt::Debug for Heard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Answer { call, reply } => f
                .debug_struct("Answer")
                .field("call", call)
                .field("reply", reply)
                .finish(),
            Self::Asked { call, method } => f
                .debug_struct("Asked")
                .field("call", call)
                .field("method_bytes", &method.len())
                .finish(),
            Self::Told { method, params } => f
                .debug_struct("Told")
                .field("method_bytes", &method.len())
                .field("params", &kind(params))
                .finish(),
        }
    }
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worked(result) => f.debug_tuple("Worked").field(&kind(result)).finish(),
            // Hidden as it was read, and the same words a failure shows.
            Self::Failed { code, said } => f
                .debug_struct("Failed")
                .field("code", code)
                .field("said", said)
                .finish(),
        }
    }
}

impl Heard {
    /// Reads one frame.
    ///
    /// # Errors
    ///
    /// [`Garbled`] where the text is not a JSON-RPC message crucible can act
    /// on. Every one of them is about this frame alone: the framing is intact,
    /// so the next frame is still worth reading.
    pub fn read(frame: &str) -> Result<Self, Garbled> {
        Self::read_withholding(frame, &Withheld::nothing())
    }

    /// Reads one frame from a server given `withheld`, hiding it in what the
    /// frame says that crucible keeps as words: a failure's message, and the
    /// spelling of an identifier crucible could not have issued. A method
    /// name, a notification's parameters and a result are carried as sent,
    /// for the reasons the module doc gives.
    ///
    /// # Errors
    ///
    /// As [`Self::read`], with the same values hidden in what an error says.
    pub fn read_withholding(frame: &str, withheld: &Withheld) -> Result<Self, Garbled> {
        let value: Value = serde_json::from_str(frame).map_err(|err| Garbled::Unparsed {
            said: err.to_string().into(),
        })?;
        let Value::Object(written) = value else {
            return Err(Garbled::NotAMessage {
                found: kind(&value),
            });
        };

        if written.get("jsonrpc").and_then(Value::as_str) != Some(RPC) {
            return Err(Garbled::Unversioned);
        }

        match (
            written.get("method"),
            written.get("result"),
            written.get("error"),
        ) {
            (Some(method), None, None) => asked(&written, method, withheld),
            (None, Some(result), None) => Ok(Self::Answer {
                call: call(&written, withheld)?,
                reply: Reply::Worked(result.clone()),
            }),
            (None, None, Some(trouble)) => Ok(Self::Answer {
                call: call(&written, withheld)?,
                reply: failed(trouble, withheld)?,
            }),
            _ => Err(Garbled::Shapeless),
        }
    }
}

/// A request or a notification, told apart by whether it carries an identifier.
fn asked(
    written: &Map<String, Value>,
    method: &Value,
    withheld: &Withheld,
) -> Result<Heard, Garbled> {
    // Not hidden: a notification is dropped unread and a question's name only
    // goes back to the server that asked it, so neither is ever shown.
    let method: Box<str> = text("method", method)?.into();
    bounded("method", &method)?;
    let Some(id) = written.get("id") else {
        let params = written.get("params").cloned().unwrap_or(Value::Null);
        return Ok(Heard::Told { method, params });
    };
    Ok(Heard::Asked {
        call: numbered(id, withheld)?,
        method,
    })
}

/// The call an answer settles.
fn call(written: &Map<String, Value>, withheld: &Withheld) -> Result<Call, Garbled> {
    let Some(id) = written.get("id") else {
        return Err(Garbled::Shapeless);
    };
    numbered(id, withheld)
}

/// An identifier, as the number crucible issues and nothing else.
fn numbered(id: &Value, withheld: &Withheld) -> Result<Call, Garbled> {
    if let Some(call) = id.as_u64() {
        return Ok(Call::new(call));
    }

    // Held to the same ceiling as everything else retained off a pipe. The
    // spelling is kept so the sentence can say what arrived instead, and a
    // frame is a megabyte: without this the sentence could be one.
    //
    // Hidden twice: in the strings the identifier holds, so an escaped value
    // is found as the value it decodes to, and then in the spelling, since an
    // identifier crucible could not have issued is kept as text, and a number
    // spelled there is as much a server's words as a string. Where hiding
    // would leave two of its member names the same, the identifier is named
    // by its kind instead: quoting it would quote one member where the server
    // wrote two.
    let mut written = id.clone();
    let found = match withheld.hide_in(&mut written) {
        Ok(()) => withheld.hide(&written.to_string()).into_owned(),
        Err(Indistinct) => kind(id).to_owned(),
    };
    bounded("id", &found)?;
    Err(Garbled::NotACall {
        found: found.into(),
    })
}

/// The `error` member, which the standard says is an object with a code and a
/// message.
fn failed(trouble: &Value, withheld: &Withheld) -> Result<Reply, Garbled> {
    let Some(written) = trouble.as_object() else {
        return Err(Garbled::WrongKind {
            field: "error",
            found: kind(trouble),
            wanted: "an object",
        });
    };
    let Some(code) = written.get("code").and_then(Value::as_i64) else {
        return Err(Garbled::WrongKind {
            field: "error.code",
            found: written.get("code").map_or("nothing", kind),
            wanted: "an integer",
        });
    };
    let said: Box<str> = withheld
        .hide(text(
            "error.message",
            written.get("message").unwrap_or(&Value::Null),
        )?)
        .into();
    bounded("error.message", &said)?;
    Ok(Reply::Failed { code, said })
}

/// A member that has to be a string.
fn text<'a>(field: &'static str, value: &'a Value) -> Result<&'a str, Garbled> {
    value.as_str().ok_or_else(|| Garbled::WrongKind {
        field,
        found: kind(value),
        wanted: "a string",
    })
}

/// A retained spelling, held to its ceiling.
fn bounded(field: &'static str, said: &str) -> Result<(), Garbled> {
    if said.len() > SAID_BYTES {
        return Err(Garbled::TooLong {
            field,
            maximum: SAID_BYTES,
            actual: said.len(),
        });
    }
    Ok(())
}

/// What arrived, named the way a sentence about it would name it.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// One frame crucible is about to send.
///
/// Built here rather than by whoever is speaking, so every outgoing frame
/// carries the version member and nothing composes a message by hand.
///
/// It carries what a server said as sent — the name of a tool being called,
/// a page cursor, the name of a question being refused — so its `Debug` says
/// how long the frame is and never a word of it.
#[derive(Clone, PartialEq, Eq)]
pub struct Sent(Value);

impl fmt::Debug for Sent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sent")
            .field("frame_bytes", &self.frame().len())
            .finish()
    }
}

impl Sent {
    /// A call crucible wants an answer to.
    #[must_use]
    pub fn asking(call: Call, method: &str, params: &Value) -> Self {
        Self(json!({
            "jsonrpc": RPC,
            "id": call.number(),
            "method": method,
            "params": params,
        }))
    }

    /// Something crucible is saying that expects nothing back.
    #[must_use]
    pub fn telling(method: &str, params: &Value) -> Self {
        Self(json!({ "jsonrpc": RPC, "method": method, "params": params }))
    }

    /// The refusal crucible sends every question a server asks it.
    ///
    /// Crucible is a client here and offers a server nothing to call back into.
    /// Saying so is what lets the server stop waiting and get on with the
    /// request it was actually asked.
    #[must_use]
    pub fn refusing(call: Call, method: &str) -> Self {
        Self(json!({
            "jsonrpc": RPC,
            "id": call.number(),
            "error": {
                "code": NO_SUCH_METHOD,
                "message": format!("crucible offers no {method}"),
            },
        }))
    }

    /// The frame, as the line it goes out as.
    #[must_use]
    pub fn frame(&self) -> String {
        self.0.to_string()
    }
}

#[cfg(test)]
mod tests;
