//! One configuration file, read and checked against the shape.

use crucible_core::Rules;
use serde_json::Value;

use crate::check::{Reader, Spot};
use crate::error::ConfigError;
use crate::rules;
use crate::shape::DOCUMENT;

/// Which of the layers a document was read from.
///
/// Carried by the document rather than decided by the reader, because one rule
/// depends on it: the layer that travels with a clone may not carry `env`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The configuration file in the user's home directory.
    User,
    /// `.crucible/config.json` — checked in, and read by everyone who clones.
    Project,
    /// `.crucible/config.local.json` — git ignores it.
    ProjectLocal,
}

impl Origin {
    /// How near this layer is to the working directory. Higher wins.
    ///
    /// Precedence belongs to the origin rather than to the order documents are
    /// handed over, so resolving them cannot be got wrong by passing them in
    /// the wrong sequence.
    pub(crate) fn nearness(self) -> u8 {
        match self {
            Self::User => 0,
            Self::Project => 1,
            Self::ProjectLocal => 2,
        }
    }
}

/// A configuration file that parsed and that crucible understood.
///
/// Holding one is the proof that every key in it is a key crucible has and
/// every value is the kind of thing that key accepts, so the layers above only
/// have to decide precedence.
#[derive(Debug, Clone)]
pub struct Document {
    value: Value,
    origin: Origin,

    /// The rules this file states, read here rather than after the layers are
    /// resolved: a rule that will not parse has to be reported with the file it
    /// is in, and by then nothing could say which file that was.
    rules: Rules,
}

impl Document {
    /// Reads one document.
    ///
    /// `file` is what the reader will be told to open, so it is the path as the
    /// user would name it rather than a canonicalised one.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Malformed`] when the text is not JSON, one of the shape
    /// errors when it is JSON crucible does not understand, and
    /// [`ConfigError::BadRule`] or [`ConfigError::Relative`] when the
    /// permissions block holds text that is the right shape and says nothing.
    pub fn parse(text: &str, file: &str, origin: Origin) -> Result<Self, ConfigError> {
        let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
            file: file.into(),
            line: source.line(),
            column: source.column(),
            problem: without_position(&source.to_string()).into(),
        })?;

        let reader = Reader { file, text };
        reader.check(&value, &DOCUMENT, Spot::ROOT)?;
        reader.variables(&value, origin)?;
        reader.directories(&value)?;
        let rules = rules::read(&reader, &value)?;

        Ok(Self {
            value,
            origin,
            rules,
        })
    }

    /// The checked value, for the layering above to merge.
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    /// Which layer this came from, which is what decides precedence.
    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }

    /// The rules this file stated, for the layering above to concatenate.
    pub(crate) fn rules(self) -> Rules {
        self.rules
    }

    /// A document from text that the test author knows is one.
    ///
    /// Here rather than in each test module that wants one, so that the tests
    /// about layering are not each carrying their own copy of how a document is
    /// made. Panics on text that will not parse, which in a test is the report.
    #[cfg(test)]
    pub(crate) fn sample(text: &str, origin: Origin) -> Self {
        Self::parse(text, "config.json", origin).expect("the test wrote a document")
    }
}

/// What the JSON parser said, without the position crucible says itself.
///
/// The parser ends its sentence with ` at line N column M`, and the error this
/// becomes has already given the same two numbers in crucible's own words. Both
/// of them, punctuated differently, is the reader's first clue that nobody read
/// the message. The line and column are taken from the parser separately, so
/// nothing is lost by cutting the tail off the text.
fn without_position(said: &str) -> &str {
    said.rsplit_once(" at line ")
        .map_or(said, |(problem, _)| problem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_crucible_does_not_have_is_refused() {
        let err = Document::parse(
            r#"{"colour": "always"}"#,
            ".crucible/config.json",
            Origin::Project,
        )
        .unwrap_err();

        // The reader has the file open. Telling them the key is wrong without
        // telling them what is accepted leaves them guessing at the spelling.
        let said = err.to_string();
        assert!(matches!(err, ConfigError::UnknownKey { .. }), "got {err:?}");
        assert!(said.contains("colour"), "got {said}");
        assert!(said.contains("output"), "got {said}");
    }
}
