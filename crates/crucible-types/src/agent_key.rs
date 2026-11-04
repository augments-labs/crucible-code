//! The name crucible gives an agent its catalog reviewed: the stem of the
//! agent's route, of the directory its releases are kept in and of its
//! credential names.
//!
//! A key is chosen once, when the agent is first reviewed, and is never reused
//! or renamed, so a name already written into somebody's configuration keeps
//! meaning the agent it meant. It is not the registry's id, so a rename
//! upstream never reaches a stored name. This type holds the spelling alone:
//! one to [`AGENT_KEY_BYTES`] lowercase ASCII letters, digits and hyphens.
//! Whether a key collides with a built-in driver's word is configuration's
//! question, since configuration holds those words, and whether the catalog
//! lists it is the catalog's.

use std::fmt;
use std::str::FromStr;

/// The longest an agent key is, in bytes; every byte of one is ASCII.
pub const AGENT_KEY_BYTES: usize = 32;

/// The crucible name of an agent the catalog reviewed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AgentKey(Box<str>);

/// Why text is not an agent key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentKeyError {
    /// It is empty or longer than [`AGENT_KEY_BYTES`].
    #[error("an agent key is 1 to {AGENT_KEY_BYTES} characters, not {0}")]
    Length(usize),
    /// It holds a character other than a lowercase letter, a digit or a
    /// hyphen.
    #[error("an agent key is lowercase letters, digits and hyphens, and {0:?} is none of them")]
    Character(char),
}

impl AgentKey {
    /// `text` as an agent key.
    ///
    /// # Errors
    ///
    /// [`AgentKeyError`] when `text` is not one to [`AGENT_KEY_BYTES`]
    /// lowercase letters, digits and hyphens.
    pub fn new(text: &str) -> Result<Self, AgentKeyError> {
        if let Some(refused) = text
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
        {
            return Err(AgentKeyError::Character(refused));
        }
        if text.is_empty() || text.len() > AGENT_KEY_BYTES {
            return Err(AgentKeyError::Length(text.len()));
        }
        Ok(Self(text.into()))
    }

    /// The key as it is spelled.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AgentKey {
    type Err = AgentKeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl fmt::Display for AgentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{AGENT_KEY_BYTES, AgentKey, AgentKeyError};

    #[test]
    fn lowercase_letters_digits_and_hyphens_are_a_key() {
        let longest = "a".repeat(AGENT_KEY_BYTES);
        for text in [
            "goose",
            "fast-agent",
            "a",
            "0",
            "-",
            "claude-acp-2",
            longest.as_str(),
        ] {
            assert_eq!(
                AgentKey::new(text).map(|key| key.as_str().to_owned()),
                Ok(text.to_owned()),
                "{text}"
            );
        }
    }

    #[test]
    fn an_empty_or_overlong_key_is_refused_by_its_length() {
        assert_eq!(AgentKey::new(""), Err(AgentKeyError::Length(0)));
        assert_eq!(
            AgentKey::new(&"a".repeat(AGENT_KEY_BYTES + 1)),
            Err(AgentKeyError::Length(AGENT_KEY_BYTES + 1))
        );
    }

    #[test]
    fn any_other_character_is_refused_by_name() {
        for (text, refused) in [
            ("Goose", 'G'),
            ("fast_agent", '_'),
            ("an agent", ' '),
            ("agent.key", '.'),
            ("agent/key", '/'),
            ("agént", 'é'),
        ] {
            assert_eq!(
                AgentKey::new(text),
                Err(AgentKeyError::Character(refused)),
                "{text}"
            );
        }
    }

    #[test]
    fn a_key_reads_and_shows_as_it_is_spelled() {
        let key: Result<AgentKey, _> = "goose".parse();
        assert_eq!(key.map(|key| key.to_string()), Ok("goose".to_owned()));
    }
}
