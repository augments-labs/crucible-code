//! Typed, retained facts assembled around a provider request.
//!
//! These are the persisted facts alone: a fragment as it was retained, the
//! complete serialized state behind it, and the deterministic patch between two
//! such states. Replaying a session needs exactly this much and no assembler,
//! which is why it lives at the bottom of the graph. What projects a family of
//! facts into words, and what reads retained history back to decide whether the
//! model still knows them, belongs with the assembler above.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value};

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
    section: Box<str>,
    text: Box<str>,
}

impl Fragment {
    /// Takes the words one stable section produced.
    #[must_use]
    pub fn new(section: impl Into<Box<str>>, text: impl Into<Box<str>>) -> Self {
        Self {
            section: section.into(),
            text: text.into(),
        }
    }

    /// The stable persistence identity of the section that produced this.
    #[must_use]
    pub fn section(&self) -> &str {
        &self.section
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

/// The complete typed context state after zero or more section updates.
///
/// A [`BTreeMap`] owns the order. Serialization therefore cannot depend on
/// discovery order, hash seeds, or which section happened to refresh first.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ContextSnapshot {
    sections: BTreeMap<Box<str>, Value>,
}

impl fmt::Debug for ContextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextSnapshot")
            .field(
                "sections",
                &format_args!("{} values redacted", self.sections.len()),
            )
            .finish()
    }
}

impl ContextSnapshot {
    /// An empty state: no section has been sent yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one section's already-validated state, replacing any earlier
    /// state it had.
    ///
    /// The validation is here rather than at the caller because it is a fact
    /// about the set: a JSON `null` inside it would read as RFC 7386 removal
    /// the next time a patch was taken, so no route in may deposit one.
    ///
    /// # Errors
    ///
    /// [`ContextError::NullSnapshot`] before null can enter the set.
    pub fn record(&mut self, section: &str, state: Value) -> Result<(), ContextError> {
        if state.is_null() || contains_merge_removal(&state) {
            return Err(ContextError::NullSnapshot {
                section: section.into(),
            });
        }
        self.sections.insert(section.into(), state);
        Ok(())
    }

    /// Reconstructs a snapshot from its persisted JSON object.
    ///
    /// # Errors
    ///
    /// [`ContextError::SnapshotNotObject`] for any other root, or
    /// [`ContextError::NullSnapshot`] for a removed section or object member
    /// masquerading as present state.
    pub fn from_value(value: Value) -> Result<Self, ContextError> {
        let Value::Object(object) = value else {
            return Err(ContextError::SnapshotNotObject);
        };

        let mut sections = BTreeMap::new();
        for (section, state) in object {
            if state.is_null() || contains_merge_removal(&state) {
                return Err(ContextError::NullSnapshot {
                    section: section.into(),
                });
            }
            sections.insert(section.into(), state);
        }

        Ok(Self { sections })
    }

    /// One section's recorded state.
    #[must_use]
    pub fn get(&self, section: &str) -> Option<&Value> {
        self.sections.get(section)
    }

    /// Every section in stable identifier order.
    pub fn sections(&self) -> impl ExactSizeIterator<Item = (&str, &Value)> {
        self.sections
            .iter()
            .map(|(section, state)| (section.as_ref(), state))
    }

    /// The deterministic JSON object persisted and patched.
    #[must_use]
    pub fn value(&self) -> Value {
        let object: Map<String, Value> = self
            .sections
            .iter()
            .map(|(section, state)| (section.to_string(), state.clone()))
            .collect();
        Value::Object(object)
    }

    /// The RFC 7386 patch that turns `prior` into this state.
    #[must_use]
    pub fn patch_from(&self, prior: &Self) -> Option<ContextPatch> {
        merge_difference(&prior.value(), &self.value()).map(ContextPatch)
    }
}

/// Whether RFC 7386 would read snapshot state as an instruction to remove.
///
/// Arrays are replaced atomically by merge patch, so a null nested inside one
/// remains ordinary array data. Object members are recursively patched, and a
/// null in any of those positions can only mean deletion.
pub fn contains_merge_removal(value: &Value) -> bool {
    let Value::Object(object) = value else {
        return false;
    };
    object
        .values()
        .any(|value| value.is_null() || contains_merge_removal(value))
}

/// An RFC 7386 JSON merge patch over a [`ContextSnapshot`].
#[derive(Clone, PartialEq, Eq)]
pub struct ContextPatch(Value);

impl ContextPatch {
    /// Reads a persisted patch, whose root must be an object because a context
    /// snapshot's root is the section map and is never replaced wholesale.
    ///
    /// # Errors
    ///
    /// [`ContextError::PatchNotObject`] for a non-object root.
    pub fn from_value(value: Value) -> Result<Self, ContextError> {
        if value.is_object() {
            Ok(Self(value))
        } else {
            Err(ContextError::PatchNotObject)
        }
    }

    /// The exact deterministic JSON patch.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.0
    }

    /// Applies this patch to `prior` with RFC 7386 semantics.
    ///
    /// # Errors
    ///
    /// A derived patch cannot produce an invalid snapshot. A patch read from a
    /// session can, and is refused as a typed context error rather than
    /// accepted until some later section tries to use it.
    pub fn apply(&self, prior: &ContextSnapshot) -> Result<ContextSnapshot, ContextError> {
        let mut value = prior.value();
        merge_apply(&mut value, &self.0);
        ContextSnapshot::from_value(value)
    }
}

impl fmt::Debug for ContextPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ContextPatch([redacted])")
    }
}

/// Derives the smallest RFC 7386 patch that changes `prior` into `current`.
fn merge_difference(prior: &Value, current: &Value) -> Option<Value> {
    if prior == current {
        return None;
    }

    let (Value::Object(before), Value::Object(after)) = (prior, current) else {
        return Some(current.clone());
    };

    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let mut changed = BTreeMap::new();
    for key in keys {
        match (before.get(key), after.get(key)) {
            (Some(_), None) => {
                changed.insert(key.clone(), Value::Null);
            }
            (None, Some(value)) => {
                changed.insert(key.clone(), value.clone());
            }
            (Some(old), Some(new)) => {
                if let Some(value) = merge_difference(old, new) {
                    changed.insert(key.clone(), value);
                }
            }
            (None, None) => {}
        }
    }

    Some(Value::Object(changed.into_iter().collect()))
}

/// Applies one RFC 7386 merge patch in place.
fn merge_apply(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };

    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Some(object) = target.as_object_mut() else {
        return;
    };

    for (key, change) in changes {
        if change.is_null() {
            object.remove(key);
            continue;
        }

        if let Some(value) = object.get_mut(key) {
            merge_apply(value, change);
        } else {
            let mut value = Value::Null;
            merge_apply(&mut value, change);
            object.insert(key.clone(), value);
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
        section: Box<str>,
    },
    /// A complete context snapshot was not a section map.
    #[error("context snapshot is not a JSON object")]
    SnapshotNotObject,
    /// A persisted merge patch tried to replace the section map wholesale.
    #[error("context merge patch is not a JSON object")]
    PatchNotObject,
}
