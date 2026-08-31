//! Typed, retained facts assembled around a provider request.
//!
//! A context section is not a second instruction channel. It projects one
//! bounded family of facts the model is sent, remembers the serialized state
//! behind those words, and can recognize its own retained fragments after the
//! transcript has been rewritten. The state and history are both necessary:
//! either one alone can claim the model knows words compaction removed.

use std::fmt;

use serde_json::Value;

/// What the retained transcript proves the model has seen for one section.
///
/// `Stale` and `Fresh` both require a complete rendering, but they remain
/// separate because the cause matters: one is history rewriting and the other
/// is a section speaking for the first time. `Unknown` is different again. It
/// means words from the section remain while the typed state that explained
/// them does not, so the replacement must say it supersedes those words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen<T> {
    /// A recorded state still has a recognized rendering in retained history.
    Known(T),
    /// A recorded state exists, but history no longer establishes it.
    Stale,
    /// Section words remain without the typed state that produced them.
    Unknown,
    /// This section has never spoken.
    Fresh,
}

/// One model-visible rendering owned by a context section.
///
/// The section identifier travels with the words because recognition after
/// compaction must not depend on prose that can legitimately change. The text
/// itself is redacted from [`Debug`]: workspace paths, tool names, and granted
/// scopes are user data even when their section name is not.
#[derive(Clone, PartialEq, Eq)]
pub struct Fragment {
    section: &'static str,
    text: Box<str>,
}

impl Fragment {
    /// Takes the words one stable section produced.
    #[must_use]
    pub fn new(section: &'static str, text: impl Into<Box<str>>) -> Self {
        Self {
            section,
            text: text.into(),
        }
    }

    /// The stable persistence identity of the section that produced this.
    #[must_use]
    pub const fn section(&self) -> &'static str {
        self.section
    }

    /// The exact words the model reads.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl fmt::Debug for Fragment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fragment")
            .field("section", &self.section)
            .field("text", &"[redacted]")
            .finish()
    }
}

/// One independently changing family of model-visible context.
///
/// `ID` is part of the session format, not a display label. It is persisted in
/// context records and retained fragments; renaming it is a breaking replay
/// change in the same class as changing a session-log format.
pub trait ContextSection {
    /// Stable, persisted section identity. Never rename a shipped value.
    const ID: &'static str;

    /// What is true now, as JSON state suitable for an RFC 7386 merge patch.
    ///
    /// Callers use [`ContextSection::checked_snapshot`] rather than retaining
    /// this value directly, so JSON `null` cannot acquire its merge-patch
    /// meaning of removal by accident.
    fn snapshot(&self) -> Value;

    /// The words needed given what retained history establishes.
    ///
    /// `None` means the model already knows the complete current state.
    fn render(&self, prior: Seen<&Value>) -> Option<Fragment>;

    /// Whether one retained fragment belongs to this section.
    fn recognizes(&self, fragment: &Fragment) -> bool;

    /// Serializes this section at the one boundary that admits snapshots.
    ///
    /// # Errors
    ///
    /// [`ContextError::NullSnapshot`] when `snapshot` returned JSON `null`.
    fn checked_snapshot(&self) -> Result<Value, ContextError> {
        let snapshot = self.snapshot();
        if snapshot.is_null() {
            Err(ContextError::NullSnapshot { section: Self::ID })
        } else {
            Ok(snapshot)
        }
    }
}

/// Why context state could not cross its typed boundary.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ContextError {
    /// JSON null is reserved by RFC 7386 to remove an object member.
    #[error("context section {section} serialized to null")]
    NullSnapshot {
        /// The stable section whose state was defective.
        section: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ContextError, ContextSection, Fragment, Seen};

    struct Workspace;

    impl ContextSection for Workspace {
        const ID: &'static str = "workspace";

        fn snapshot(&self) -> Value {
            json!({ "root": "/work" })
        }

        fn render(&self, prior: Seen<&Value>) -> Option<Fragment> {
            match prior {
                Seen::Known(_) => None,
                Seen::Stale | Seen::Unknown | Seen::Fresh => {
                    Some(Fragment::new(Self::ID, "workspace is /work"))
                }
            }
        }

        fn recognizes(&self, fragment: &Fragment) -> bool {
            fragment.section() == Self::ID
        }
    }

    struct NullSection;

    impl ContextSection for NullSection {
        const ID: &'static str = "permissions";

        fn snapshot(&self) -> Value {
            Value::Null
        }

        fn render(&self, _prior: Seen<&Value>) -> Option<Fragment> {
            None
        }

        fn recognizes(&self, fragment: &Fragment) -> bool {
            fragment.section() == Self::ID
        }
    }

    #[test]
    fn the_four_seen_states_keep_stale_fresh_and_unknown_distinct() {
        let known = json!({ "root": "/before" });
        assert!(matches!(Seen::Known(&known), Seen::Known(_)));
        assert!(matches!(Seen::<&Value>::Stale, Seen::Stale));
        assert!(matches!(Seen::<&Value>::Unknown, Seen::Unknown));
        assert!(matches!(Seen::<&Value>::Fresh, Seen::Fresh));
    }

    #[test]
    fn a_fragment_keeps_the_stable_section_id_that_recognizes_it() {
        let section = Workspace;
        let fragment = section
            .render(Seen::Fresh)
            .expect("fresh context renders in full");

        assert_eq!(fragment.section(), "workspace");
        assert_eq!(fragment.text(), "workspace is /work");
        assert!(section.recognizes(&fragment));
        assert!(!section.recognizes(&Fragment::new("model", "different")));
    }

    #[test]
    fn null_is_rejected_at_the_section_boundary_and_names_its_owner() {
        let problem = NullSection.checked_snapshot().unwrap_err();

        assert_eq!(
            problem,
            ContextError::NullSnapshot {
                section: "permissions"
            }
        );
        assert_eq!(
            problem.to_string(),
            "context section permissions serialized to null"
        );
    }

    #[test]
    fn a_non_null_snapshot_crosses_the_boundary_unchanged() {
        assert_eq!(
            Workspace.checked_snapshot().unwrap(),
            json!({ "root": "/work" })
        );
    }
}
