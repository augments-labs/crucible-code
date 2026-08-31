//! Typed, retained facts assembled around a provider request.
//!
//! A context section is not a second instruction channel. It projects one
//! bounded family of facts the model is sent, remembers the serialized state
//! behind those words, and can recognize its own retained fragments after the
//! transcript has been rewritten. The state and history are both necessary:
//! either one alone can claim the model knows words compaction removed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value};

use crate::transcript::{Message, Transcript};

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

/// One independently changing family of model-visible context.
///
/// `ID` is part of the session format, not a display label. It is persisted in
/// context records and retained fragments; renaming it is a breaking replay
/// change in the same class as changing a session-log format.
pub trait ContextSection {
    /// Stable, persisted section identity. Never rename a shipped value.
    const ID: &'static str;

    /// The stable identifier for this instance.
    ///
    /// Ordinary sections use [`ContextSection::ID`]. The method exists so a
    /// registry can expose several data-driven sections through one concrete
    /// implementation without turning their persisted identities into prose.
    fn id(&self) -> &'static str {
        Self::ID
    }

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
            Err(ContextError::NullSnapshot {
                section: self.id().into(),
            })
        } else {
            Ok(snapshot)
        }
    }
}

/// The complete typed context state after zero or more section updates.
///
/// A [`BTreeMap`] owns the order. Serialization therefore cannot depend on
/// discovery order, hash seeds, or which section happened to refresh first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextSnapshot {
    sections: BTreeMap<Box<str>, Value>,
}

impl ContextSnapshot {
    /// An empty state: no section has been sent yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Captures one section, replacing its earlier state if it had one.
    ///
    /// # Errors
    ///
    /// [`ContextError::NullSnapshot`] before null can enter the set.
    pub fn capture(&mut self, section: &impl ContextSection) -> Result<(), ContextError> {
        let state = section.checked_snapshot()?;
        self.sections.insert(section.id().into(), state);
        Ok(())
    }

    /// Reconstructs a snapshot from its persisted JSON object.
    ///
    /// # Errors
    ///
    /// [`ContextError::SnapshotNotObject`] for any other root, or
    /// [`ContextError::NullSnapshot`] for a removed section masquerading as
    /// present state.
    pub fn from_value(value: Value) -> Result<Self, ContextError> {
        let Value::Object(object) = value else {
            return Err(ContextError::SnapshotNotObject);
        };

        let mut sections = BTreeMap::new();
        for (section, state) in object {
            if state.is_null() {
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

    /// Resolves what retained history proves for one section.
    ///
    /// Both halves are load-bearing. Recorded state without a retained
    /// fragment is stale after compaction; a retained fragment without its
    /// typed state is unknown and must be superseded defensively.
    #[must_use]
    pub fn seen<'a>(
        &'a self,
        section: &impl ContextSection,
        transcript: &Transcript,
    ) -> Seen<&'a Value> {
        let recognized = transcript.messages().iter().any(
            |message| matches!(message, Message::Context(fragment) if section.recognizes(fragment)),
        );

        match (self.get(section.id()), recognized) {
            (Some(snapshot), true) => Seen::Known(snapshot),
            (Some(_), false) => Seen::Stale,
            (None, true) => Seen::Unknown,
            (None, false) => Seen::Fresh,
        }
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Message, Transcript};

    use super::{ContextError, ContextPatch, ContextSection, ContextSnapshot, Fragment, Seen};

    struct State {
        id: &'static str,
        value: Value,
    }

    impl ContextSection for State {
        const ID: &'static str = "state";

        fn snapshot(&self) -> Value {
            self.value.clone()
        }

        fn render(&self, _prior: Seen<&Value>) -> Option<Fragment> {
            None
        }

        fn recognizes(&self, fragment: &Fragment) -> bool {
            fragment.section() == self.id
        }

        fn id(&self) -> &'static str {
            self.id
        }
    }

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
                section: "permissions".into()
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

    #[test]
    fn section_order_is_deterministic_whatever_order_it_was_captured_in() {
        let a = State {
            id: "a",
            value: json!({ "value": 1 }),
        };
        let z = State {
            id: "z",
            value: json!({ "value": 2 }),
        };
        let mut first = ContextSnapshot::new();
        first.capture(&z).unwrap();
        first.capture(&a).unwrap();
        let mut second = ContextSnapshot::new();
        second.capture(&a).unwrap();
        second.capture(&z).unwrap();

        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first.value()).unwrap(),
            r#"{"a":{"value":1},"z":{"value":2}}"#
        );
    }

    #[test]
    fn merge_patch_derivation_and_application_round_trip_the_snapshot() {
        let prior = ContextSnapshot::from_value(json!({
            "environment": { "platform": "linux", "date": "2026-08-30" },
            "tools": { "generation": "one", "visible": { "read": true, "bash": true } }
        }))
        .unwrap();
        let current = ContextSnapshot::from_value(json!({
            "environment": { "platform": "linux", "date": "2026-08-31" },
            "tools": { "generation": "two", "visible": { "read": true, "grep": true } }
        }))
        .unwrap();

        let patch = current.patch_from(&prior).expect("the state changed");

        assert_eq!(
            patch.value(),
            &json!({
                "environment": { "date": "2026-08-31" },
                "tools": {
                    "generation": "two",
                    "visible": { "bash": null, "grep": true }
                }
            })
        );
        assert_eq!(patch.apply(&prior).unwrap(), current);
    }

    #[test]
    fn absence_and_present_empty_state_are_distinct_and_both_expressible() {
        let absent = ContextSnapshot::new();
        let present = ContextSnapshot::from_value(json!({ "skills": {} })).unwrap();

        let adding = present
            .patch_from(&absent)
            .expect("an empty section was added");
        assert_eq!(adding.value(), &json!({ "skills": {} }));
        assert_eq!(adding.apply(&absent).unwrap(), present);

        let removing = absent
            .patch_from(&present)
            .expect("the empty section was removed");
        assert_eq!(removing.value(), &json!({ "skills": null }));
        assert_eq!(removing.apply(&present).unwrap(), absent);
    }

    #[test]
    fn equal_snapshots_need_no_patch() {
        let snapshot = ContextSnapshot::from_value(json!({ "model": { "name": "one" } })).unwrap();

        assert_eq!(snapshot.patch_from(&snapshot), None);
    }

    #[test]
    fn a_persisted_patch_must_be_an_object() {
        let problem = ContextPatch::from_value(json!("replace everything")).unwrap_err();

        assert_eq!(problem, ContextError::PatchNotObject);
    }

    #[test]
    fn reconciliation_uses_recorded_state_and_retained_history_together() {
        let section = Workspace;
        let mut recorded = ContextSnapshot::new();
        recorded.capture(&section).unwrap();
        let mut retained = Transcript::new();
        retained.push(Message::Context(Fragment::new(
            Workspace::ID,
            "workspace is /work",
        )));

        assert!(matches!(
            recorded.seen(&section, &retained),
            Seen::Known(state) if state == &json!({ "root": "/work" })
        ));

        assert!(matches!(
            recorded.seen(&section, &Transcript::new()),
            Seen::Stale
        ));

        assert!(matches!(
            ContextSnapshot::new().seen(&section, &retained),
            Seen::Unknown
        ));

        assert!(matches!(
            ContextSnapshot::new().seen(&section, &Transcript::new()),
            Seen::Fresh
        ));
    }

    #[test]
    fn reconciliation_after_history_rewrite_detects_the_removed_fragment() {
        let section = Workspace;
        let mut recorded = ContextSnapshot::new();
        recorded.capture(&section).unwrap();
        let mut retained = Transcript::new();
        retained.push(Message::Context(Fragment::new(
            Workspace::ID,
            "workspace is /work",
        )));

        assert!(matches!(recorded.seen(&section, &retained), Seen::Known(_)));

        retained.behind(1);

        assert!(matches!(recorded.seen(&section, &retained), Seen::Stale));
    }
}
