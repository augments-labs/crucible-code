//! What one frame says.
//!
//! [`Frames`](crate::Frames) finds the boundaries; this reads what is inside
//! them. The two are apart on purpose: a boundary can be found in bytes nobody
//! has trusted yet, and everything here is a document somebody else's program
//! wrote.
//!
//! Three things can be said. A request expects exactly one answer, an answer
//! settles exactly one request, and a telling expects nothing back. Both ends
//! send all three — an extension granted `askTheOperator` asks crucible
//! questions, and crucible tells an extension what has happened without waiting
//! on it — so there is one shape here and not one per direction.
//!
//! What rides inside a request or an answer is carried and never read. The
//! names in there were chosen by whoever wrote the method, for the reason
//! `extensions.<id>.config` is carried unread: crucible refusing a spelling out
//! of a vocabulary it was never shown is crucible deleting a line the protocol
//! told its author to write.

use std::fmt;

use serde_json::{Map, Value, json};

/// The most bytes a method name, or the words of a failure, may carry.
///
/// Far past any method a protocol names and any sentence a person reads, and
/// held all the same. Both are text off a pipe that crucible keeps: a method
/// name goes into whatever the host is matching on, and a failure's words go on
/// a screen. Neither is a place to let the far end choose the size.
pub const EXTENSION_SAID_BYTES: usize = 4 * 1024;

/// Which call an exchange belongs to.
///
/// Each end numbers its own calls, so two of these are equal only within one
/// direction. Nothing here matches an answer to a request; a call nobody made
/// is the host's to notice, because only the host knows what it asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallId(u64);

impl CallId {
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

impl fmt::Display for CallId {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(form, "{}", self.0)
    }
}

/// Why a frame was not a message crucible could act on.
///
/// Each of these is a document that parsed and still said nothing usable. They
/// are named rather than collapsed into one refusal because the person reading
/// the report is usually the extension's author, and which of these it was is
/// the whole of what they need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malformed {
    /// It named no method and carried no answer.
    Silent,
    /// It named a method and answered at the same time.
    Muddled,
    /// It answered with a result and a failure at once.
    Doubled,
    /// It answered with no call to answer.
    Unasked,
}

impl Malformed {
    /// Every way a parsed frame can still be nothing.
    ///
    /// Listed for the reason [`ExtensionCapability::EVERY`] is, with the same
    /// unchecked completeness: the exhaustive match below is what puts an
    /// author in this file, and the line above it is theirs to remember.
    ///
    /// [`ExtensionCapability::EVERY`]: crate::ExtensionCapability::EVERY
    pub const EVERY: &'static [Self] = &[Self::Silent, Self::Muddled, Self::Doubled, Self::Unasked];

    /// What was wrong with it, as a report would put it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "names no method and carries no answer",
            Self::Muddled => "names a method and answers at the same time",
            Self::Doubled => "answers with a result and a failure at once",
            Self::Unasked => "answers with no call to answer",
        }
    }
}

impl fmt::Display for Malformed {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        form.write_str(self.as_str())
    }
}

/// Why a frame was not a message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpokenError {
    /// It was not a document at all.
    #[error("a frame was not JSON: {said}")]
    NotJson {
        /// What the parser reported.
        said: Box<str>,
    },

    /// It parsed into something that was not an object.
    #[error("a frame was {found} rather than an object")]
    NotAnObject {
        /// What it was instead.
        found: &'static str,
    },

    /// It was an object and still said nothing usable.
    #[error("a frame {problem}")]
    Malformed {
        /// Which way.
        problem: Malformed,
    },

    /// A field was there and was the wrong shape.
    #[error("a frame's {field} was {found} rather than {wanted}")]
    WrongShape {
        /// Which one.
        field: &'static str,
        /// What arrived.
        found: &'static str,
        /// What has to be there.
        wanted: &'static str,
    },

    /// Its call identifier was not one.
    #[error("a frame's call identifier was not a whole number")]
    NotACall,

    /// A retained spelling was empty.
    #[error("a frame's {field} was empty")]
    Empty {
        /// Which one.
        field: &'static str,
    },

    /// A retained spelling crossed its boundary.
    #[error("a frame's {field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which one.
        field: &'static str,
        /// Its boundary.
        maximum: usize,
        /// What arrived.
        actual: usize,
    },
}

/// What went wrong, in the far end's own words.
///
/// Its own type so those words cannot be mistaken for crucible's anywhere they
/// are printed. An extension is a program somebody else wrote, and a line it
/// composed that reads as crucible's own voice is how a person comes to be
/// asked for a credential by something that is not crucible.
///
/// Which is why there is no [`Display`](fmt::Display) here. Reaching the words
/// means calling [`said`](Self::said), and whoever calls it has to put them
/// somewhere that says whose they are:
///
/// ```compile_fail,E0277
/// // Named error code on purpose: `compile_fail` alone passes for any compile
/// // error, so a rename elsewhere could make this snippet fail for a reason
/// // that has nothing to do with the seam it guards.
/// use crucible_core::Trouble;
///
/// let trouble = Trouble::new("could not reach the index").unwrap();
/// println!("crucible: {trouble}");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    /// The far end's own words.
    said: Box<str>,
}

impl Trouble {
    /// What the far end said went wrong.
    ///
    /// # Errors
    ///
    /// [`SpokenError`] where the words are empty or past
    /// [`EXTENSION_SAID_BYTES`].
    pub fn new(said: impl Into<Box<str>>) -> Result<Self, SpokenError> {
        let said = said.into();
        bounded("failure", &said)?;
        Ok(Self { said })
    }

    /// The far end's own words, to be shown as the far end's own.
    #[must_use]
    pub fn said(&self) -> &str {
        &self.said
    }
}

/// What a request was answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It worked, carrying whatever the method answers with.
    Worked(Value),
    /// It did not.
    Failed(Trouble),
}

/// One thing said on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spoken {
    /// A request expecting exactly one answer.
    Request {
        /// Which call this starts.
        id: CallId,
        /// What is being asked for.
        method: Box<str>,
        /// What rides with it, carried unread.
        params: Value,
    },

    /// The answer to exactly one request.
    Answer {
        /// Which call this settles.
        id: CallId,
        /// How it went.
        outcome: Outcome,
    },

    /// Something said that expects nothing back.
    Told {
        /// What happened.
        method: Box<str>,
        /// What rides with it, carried unread.
        params: Value,
    },
}

impl Spoken {
    /// Reads one frame.
    ///
    /// # Errors
    ///
    /// [`SpokenError`] where the frame is not JSON, is not an object, says
    /// nothing this can act on, or carries a spelling that is empty or past
    /// [`EXTENSION_SAID_BYTES`].
    pub fn read(frame: &str) -> Result<Self, SpokenError> {
        let value: Value = serde_json::from_str(frame).map_err(|err| SpokenError::NotJson {
            said: err.to_string().into(),
        })?;
        let Value::Object(written) = value else {
            return Err(SpokenError::NotAnObject {
                found: kind(&value),
            });
        };

        // The three keys are the whole discriminator, and exactly one of them
        // may be there. A frame carrying two of them has said two things at
        // once, and picking one would be crucible deciding which the author
        // meant on evidence that says they meant both.
        match (
            written.get("method"),
            written.get("result"),
            written.get("error"),
        ) {
            (Some(method), None, None) => Self::asked(&written, method),
            (None, Some(result), None) => Self::answered(&written, Outcome::Worked(result.clone())),
            (None, None, Some(failed)) => {
                let said = text("failure", failed)?;
                Self::answered(&written, Outcome::Failed(Trouble::new(said)?))
            }
            (None, None, None) => Err(Malformed::Silent.into()),
            (Some(_), _, _) => Err(Malformed::Muddled.into()),
            (None, Some(_), Some(_)) => Err(Malformed::Doubled.into()),
        }
    }

    /// A frame that named a method: a request where it also named a call.
    fn asked(written: &Map<String, Value>, method: &Value) -> Result<Self, SpokenError> {
        let method: Box<str> = text("method", method)?.into();
        bounded("method", &method)?;
        // Whatever rides along, carried and not read. Absent is not empty and
        // not an error: a method that takes nothing is answered by sending
        // nothing, and an object invented here would be crucible adding a word.
        let params = written.get("params").cloned().unwrap_or(Value::Null);
        match written.get("id") {
            None => Ok(Self::Told { method, params }),
            Some(id) => Ok(Self::Request {
                id: call(id)?,
                method,
                params,
            }),
        }
    }

    /// A frame that answered, which has to say what it is answering.
    fn answered(written: &Map<String, Value>, outcome: Outcome) -> Result<Self, SpokenError> {
        let Some(id) = written.get("id") else {
            return Err(Malformed::Unasked.into());
        };
        Ok(Self::Answer {
            id: call(id)?,
            outcome,
        })
    }

    /// Writes one frame.
    ///
    /// Never a newline, whatever rides inside: the boundary belongs to
    /// [`Written`](crate::Written), and a payload that could put one here would
    /// be a payload choosing where crucible's frames end.
    #[must_use]
    pub fn written(&self) -> String {
        let mut written = Map::new();
        match self {
            Self::Request { id, method, params } => {
                written.insert("id".to_owned(), json!(id.number()));
                written.insert("method".to_owned(), json!(method));
                written.insert("params".to_owned(), params.clone());
            }
            Self::Told { method, params } => {
                written.insert("method".to_owned(), json!(method));
                written.insert("params".to_owned(), params.clone());
            }
            Self::Answer { id, outcome } => {
                written.insert("id".to_owned(), json!(id.number()));
                match outcome {
                    Outcome::Worked(result) => {
                        written.insert("result".to_owned(), result.clone());
                    }
                    Outcome::Failed(trouble) => {
                        written.insert("error".to_owned(), json!(trouble.said()));
                    }
                }
            }
        }
        // Escaped by the writer, which is what keeps the promise above: a
        // newline inside a string arrives as two characters that are not one.
        Value::Object(written).to_string()
    }
}

impl From<Malformed> for SpokenError {
    fn from(problem: Malformed) -> Self {
        Self::Malformed { problem }
    }
}

/// The call a value names, where it names one.
///
/// A whole number that fits, and nothing else. A negative or fractional
/// identifier is one crucible cannot hand back in an answer the far end will
/// recognise, so it is refused where it arrives rather than rounded into a call
/// somebody else made.
fn call(id: &Value) -> Result<CallId, SpokenError> {
    id.as_u64().map(CallId::new).ok_or(SpokenError::NotACall)
}

/// The words a value holds, where it holds words.
fn text<'a>(field: &'static str, value: &'a Value) -> Result<&'a str, SpokenError> {
    value.as_str().ok_or(SpokenError::WrongShape {
        field,
        found: kind(value),
        wanted: "a string",
    })
}

/// What a value is, in the words a refusal uses.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "nothing",
        Value::Bool(_) => "a true or false",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// One retained spelling, held to [`EXTENSION_SAID_BYTES`].
fn bounded(field: &'static str, value: &str) -> Result<(), SpokenError> {
    if value.is_empty() {
        return Err(SpokenError::Empty { field });
    }
    if value.len() > EXTENSION_SAID_BYTES {
        return Err(SpokenError::TooLong {
            field,
            maximum: EXTENSION_SAID_BYTES,
            actual: value.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
