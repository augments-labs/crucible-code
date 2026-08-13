//! One configuration file, read and checked against the shape.

use std::fmt;

use crucible_core::Rules;
use serde_json::Value;

use crate::env;
use crate::error::ConfigError;
use crate::shape::DOCUMENT;

mod check;
mod rules;

use check::{Reader, Spot};

/// Which of the layers a document was read from.
///
/// Carried by the document rather than decided by the reader, because two rules
/// depend on it: what a file under the working directory may put in `env`, and
/// what the file that travels with a clone may say about permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The configuration file in the user's home directory.
    ///
    /// The only layer that arrived with the person rather than with a
    /// checkout, which is what makes it the only one allowed to widen anything.
    User,
    /// `.crucible/config.json` — checked in, and read by everyone who clones.
    Project,
    /// `.crucible/config.local.json` — the file crucible writes an answer to,
    /// and the one a `.gitignore` is expected to keep out of the repository.
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

    /// Whether this layer is a file that came with the working directory.
    ///
    /// Both project files do, and which of them git carries is not a fact
    /// crucible can check: `.crucible/config.local.json` is git-ignored by
    /// convention, and the convention is written in the `.gitignore` of the
    /// repository being cloned. A repository that simply commits one gets a
    /// file crucible reads and cannot tell from the user's own.
    ///
    /// So this is the question the `env` rule asks, rather than which of the
    /// two names the file has.
    pub(crate) fn in_the_workspace(self) -> bool {
        match self {
            Self::User => false,
            Self::Project | Self::ProjectLocal => true,
        }
    }
}

/// A configuration file that parsed and that crucible understood.
///
/// Holding one is the proof that every key in it is a key crucible has and
/// every value is the kind of thing that key accepts, so the layers above only
/// have to decide precedence.
#[derive(Clone)]
pub(crate) struct Document {
    value: Value,
    origin: Origin,

    /// The rules this file states, read here rather than after the layers are
    /// resolved: a rule that will not parse has to be reported with the file it
    /// is in, and by then nothing could say which file that was.
    rules: Rules,
}

impl fmt::Debug for Document {
    /// Written by hand so the `env` block is redacted. In two of the three
    /// layers a variable there may hold anything the user's commands need,
    /// and a derived `Debug` is what would print it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Document")
            .field("value", &env::Redacted(&self.value))
            .field("origin", &self.origin)
            .field("rules", &self.rules)
            .finish()
    }
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
    ///
    /// [`ConfigError::Widening`] and [`ConfigError::ProjectEnv`] for a document
    /// that says something its layer is not allowed to say — which is why the
    /// origin is an argument here rather than something the layering decides
    /// afterwards.
    pub(crate) fn parse(text: &str, file: &str, origin: Origin) -> Result<Self, ConfigError> {
        let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
            file: file.into(),
            line: source.line(),
            column: source.column(),
            problem: without_position(&source.to_string()).into(),
        })?;

        let reader = Reader { file, text, origin };
        reader.check(&value, &DOCUMENT, Spot::ROOT)?;
        reader.variables(&value)?;
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
pub(crate) fn without_position(said: &str) -> &str {
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

    #[test]
    fn printing_a_document_names_a_variable_and_shows_nothing_of_its_value() {
        // The home directory is where a variable outside crucible's own
        // namespace is allowed to live, so a document parsed from it holds
        // whatever the user put there.
        let document = Document::parse(
            r#"{"env": {"TOKEN": "hunter2"}}"#,
            "~/.crucible/config.json",
            Origin::User,
        )
        .expect("a document the user's own file accepts");

        let printed = format!("{document:?}");
        assert!(printed.contains("TOKEN"), "got {printed}");
        assert!(!printed.contains("hunter2"), "got {printed}");
    }
}
