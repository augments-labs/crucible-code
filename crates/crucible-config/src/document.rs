//! One configuration file, read and checked against the shape.

use serde_json::Value;

use crate::check::{Reader, Spot};
use crate::error::ConfigError;
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
}

impl Document {
    /// Reads one document.
    ///
    /// `file` is what the reader will be told to open, so it is the path as the
    /// user would name it rather than a canonicalised one.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Malformed`] when the text is not JSON, and one of the
    /// shape errors when it is JSON crucible does not understand.
    pub fn parse(text: &str, file: &str, origin: Origin) -> Result<Self, ConfigError> {
        let value: Value = serde_json::from_str(text).map_err(|source| ConfigError::Malformed {
            file: file.into(),
            line: source.line(),
            column: source.column(),
            problem: source.to_string().into(),
        })?;

        let reader = Reader { file, text };
        reader.check(&value, &DOCUMENT, Spot::ROOT)?;
        reader.secrets(&value, origin)?;

        Ok(Self { value, origin })
    }

    /// The checked value, for the layering above to merge.
    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    /// Which layer this came from, which is what decides precedence.
    pub(crate) fn origin(&self) -> Origin {
        self.origin
    }
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
