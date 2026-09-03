//! Saying hello, and reading back what the server offers.
//!
//! Two steps, and the first one gates the second in the type system. A
//! catalogue is only meaningful once both ends have agreed which version of the
//! protocol they are speaking, so [`tools`] takes the [`Greeting`] that
//! agreement produced and there is no way to ask for one before.
//!
//! Version agreement is the whole of what the handshake decides here. Crucible
//! offers the newest it speaks; the server answers with the one it chose; and
//! anything crucible does not speak ends the conversation rather than being
//! attempted. A client carrying on against a version it does not know is a
//! client guessing at what every later field means.
//!
//! Everything read back is somebody else's program's text. A name goes into
//! what the model calls and a schema goes into what the provider is shown, so
//! both are bounded on arrival, and a catalogue that runs past a bound is
//! refused whole rather than truncated into a shorter list that looks complete.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::talking::{Talking, Trouble};

/// The versions of MCP crucible speaks, newest first.
///
/// The whole of it. Crucible offers the first and accepts any of them back,
/// which is the negotiation the protocol describes; a version outside this list
/// is one crucible has no reader for, and treating it as near enough would mean
/// reading fields by their names in a document written to different rules.
pub const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The most tools crucible reads from one server.
///
/// Every one of them is a name and a schema the provider is shown, so this is a
/// bound on what one server can add to every request of every turn.
pub const TOOLS: usize = 256;

/// The most pages of a catalogue crucible will follow.
///
/// A cursor is the server's to choose, so a server can hand back one forever.
/// The page count is what makes that a sentence rather than a session that
/// never returns.
pub const PAGES: usize = 32;

/// The most bytes one tool's name may carry.
pub const NAME_BYTES: usize = 128;

/// The most bytes one tool's description may carry.
pub const ABOUT_BYTES: usize = 4 * 1024;

/// The most bytes one tool's input schema may carry.
pub const SCHEMA_BYTES: usize = 64 * 1024;

/// Why a server could not be greeted, or its catalogue read.
#[derive(Debug, thiserror::Error)]
pub enum Rebuffed {
    /// The conversation itself failed.
    #[error(transparent)]
    Talking(#[from] Trouble),

    /// The server chose a version of the protocol crucible does not speak.
    #[error(
        "the server answered with MCP version {found}, and crucible speaks {spoken} — \
         a client reading a version it does not know is a client guessing at what \
         every field in it means"
    )]
    Version {
        /// What the server chose.
        found: Box<str>,
        /// What crucible offered, as the list it would have accepted.
        spoken: Box<str>,
    },

    /// A member the protocol requires was absent, or was the wrong kind.
    #[error("the server answered without {field}, which every {said} has to carry")]
    Missing {
        /// Which member.
        field: &'static str,
        /// What it was an answer to.
        said: &'static str,
    },

    /// A retained spelling ran past its ceiling.
    #[error("the server offered a tool whose {field} is {actual} bytes; the maximum is {maximum}")]
    TooLong {
        /// Which field.
        field: &'static str,
        /// Its ceiling.
        maximum: usize,
        /// What arrived.
        actual: usize,
    },

    /// The catalogue was longer than crucible reads.
    ///
    /// Refused rather than cut short. A shorter list that looked complete would
    /// have the model told a server has these tools and not those, which is a
    /// statement crucible would be making up.
    #[error("the server offers more than {most} tools, which is more than crucible reads")]
    TooMany {
        /// The ceiling.
        most: usize,
    },

    /// The catalogue never ended.
    #[error("the server handed back a further page {most} times without finishing its catalogue")]
    Endless {
        /// How many pages were read.
        most: usize,
    },

    /// Two tools arrived under one name.
    ///
    /// Refused, because a name is what the model acts on: two meanings for one
    /// of them is a call whose outcome depends on which the reader happened to
    /// keep.
    #[error("the server offers two tools called {name}")]
    Twice {
        /// The name they share.
        name: Box<str>,
    },
}

/// What the handshake settled.
///
/// Holding one is the proof that both ends agreed a version, which is why
/// [`tools`] asks for it. What the server said it is called is kept because it
/// goes in the provenance of every tool read from it — a name the model sees
/// beside a tool has to say whose program answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greeting {
    /// The version both ends are speaking.
    version: Box<str>,
    /// What the server calls itself, where it said.
    named: Option<Box<str>>,
    /// Whether it said it has tools at all.
    offers: bool,
}

impl Greeting {
    /// The version both ends agreed.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// What the server calls itself, where it said so.
    #[must_use]
    pub fn named(&self) -> Option<&str> {
        self.named.as_deref()
    }

    /// Whether the server said it offers tools.
    #[must_use]
    pub const fn offers(&self) -> bool {
        self.offers
    }
}

/// One tool a server offers, as it was written down.
///
/// Inert. Holding one grants nothing and calls nothing; what turns it into
/// something the model can see is above this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    /// What the server calls it.
    name: Box<str>,
    /// What it says the tool is for, where it said.
    about: Option<Box<str>>,
    /// The schema for its arguments, carried and never read.
    schema: Value,
}

impl Offered {
    /// What the server calls it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What it says the tool is for.
    #[must_use]
    pub fn about(&self) -> Option<&str> {
        self.about.as_deref()
    }

    /// The schema for its arguments.
    #[must_use]
    pub const fn schema(&self) -> &Value {
        &self.schema
    }
}

/// Says hello, and agrees which version of MCP both ends are speaking.
///
/// # Errors
///
/// [`Rebuffed`] where the conversation fails, the answer carries no version, or
/// the version it carries is one crucible does not speak.
pub fn hello<R: BufRead, W: Write>(talking: &mut Talking<R, W>) -> Result<Greeting, Rebuffed> {
    let answer = talking.ask(
        "initialize",
        &json!({
            "protocolVersion": VERSIONS[0],
            // Nothing. Crucible offers a server no sampling, no roots and no
            // elicitation, and saying so is what stops a server building a plan
            // around asking for one.
            "capabilities": {},
            "clientInfo": { "name": "crucible", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;

    let Some(version) = answer.get("protocolVersion").and_then(Value::as_str) else {
        return Err(Rebuffed::Missing {
            field: "protocolVersion",
            said: "initialize answer",
        });
    };
    if !VERSIONS.contains(&version) {
        return Err(Rebuffed::Version {
            found: version.into(),
            spoken: VERSIONS.join(", ").into(),
        });
    }

    let greeting = Greeting {
        version: version.into(),
        named: answer
            .pointer("/serverInfo/name")
            .and_then(Value::as_str)
            .map(Into::into),
        offers: answer.pointer("/capabilities/tools").is_some(),
    };

    // The protocol's own order: nothing else may be asked until the server has
    // been told the handshake finished.
    talking.tell("notifications/initialized", &json!({}))?;
    Ok(greeting)
}

/// Reads every tool the server offers.
///
/// A server that did not say it has tools is not asked for any: the capability
/// is the server's own statement about itself, and a call past it would be
/// crucible ignoring an answer it already has.
///
/// # Errors
///
/// [`Rebuffed`] where the conversation fails, a page is not the shape the
/// protocol gives, a retained spelling runs past its ceiling, the catalogue is
/// longer than [`TOOLS`], it takes more than [`PAGES`] pages, or two tools
/// arrive under one name.
pub fn tools<R: BufRead, W: Write>(
    talking: &mut Talking<R, W>,
    greeting: &Greeting,
) -> Result<Vec<Offered>, Rebuffed> {
    if !greeting.offers() {
        return Ok(Vec::new());
    }

    let mut read: Vec<Offered> = Vec::new();
    let mut cursor: Option<Box<str>> = None;
    for _ in 0..PAGES {
        let params = cursor
            .as_deref()
            .map_or_else(|| json!({}), |held| json!({ "cursor": held }));
        let page = talking.ask("tools/list", &params)?;

        let Some(listed) = page.get("tools").and_then(Value::as_array) else {
            return Err(Rebuffed::Missing {
                field: "tools",
                said: "tools/list answer",
            });
        };
        for held in listed {
            let offered = one(held)?;
            if read.iter().any(|kept| kept.name() == offered.name()) {
                return Err(Rebuffed::Twice {
                    name: offered.name.clone(),
                });
            }
            if read.len() >= TOOLS {
                return Err(Rebuffed::TooMany { most: TOOLS });
            }
            read.push(offered);
        }

        match page.get("nextCursor").and_then(Value::as_str) {
            Some(next) => cursor = Some(next.into()),
            None => return Ok(read),
        }
    }
    Err(Rebuffed::Endless { most: PAGES })
}

/// One entry of a page.
fn one(held: &Value) -> Result<Offered, Rebuffed> {
    let Some(name) = held.get("name").and_then(Value::as_str) else {
        return Err(Rebuffed::Missing {
            field: "name",
            said: "tool",
        });
    };
    if name.is_empty() {
        return Err(Rebuffed::Missing {
            field: "name",
            said: "tool",
        });
    }
    bounded("name", name.len(), NAME_BYTES)?;

    let about = held.get("description").and_then(Value::as_str);
    if let Some(about) = about {
        bounded("description", about.len(), ABOUT_BYTES)?;
    }

    // Absent is an empty schema rather than a refusal: a tool that takes no
    // arguments is an ordinary tool, and the protocol lets a server leave the
    // member off to say so.
    let schema = held
        .get("inputSchema")
        .cloned()
        .unwrap_or_else(|| json!({}));
    bounded("inputSchema", schema.to_string().len(), SCHEMA_BYTES)?;

    Ok(Offered {
        name: name.into(),
        about: about.map(Into::into),
        schema,
    })
}

/// A retained spelling, held to its ceiling.
fn bounded(field: &'static str, actual: usize, maximum: usize) -> Result<(), Rebuffed> {
    if actual > maximum {
        return Err(Rebuffed::TooLong {
            field,
            maximum,
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
