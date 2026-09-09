//! Projecting typed facts into the words a request carries.
//!
//! A context section is not a second instruction channel. It projects one
//! bounded family of facts the model is sent, remembers the serialized state
//! behind those words, and can recognize its own retained fragments after the
//! transcript has been rewritten. The state and history are both necessary:
//! either one alone can claim the model knows words compaction removed.
//!
//! The facts themselves — a [`Fragment`], a [`ContextSnapshot`] and the patch
//! between two of them — are shared values and live below this crate. What is
//! here is the part that reads a live section or a retained transcript, so a
//! replay needs none of it.

use serde_json::Value;

use crucible_types::context::contains_merge_removal;
use crucible_types::{ContextError, ContextSnapshot, Fragment, Message, Seen, Transcript};

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
    /// [`ContextError::NullSnapshot`] when `snapshot` returned JSON `null` or
    /// placed one in an object member, where RFC 7386 would read it as removal
    /// rather than state.
    fn checked_snapshot(&self) -> Result<Value, ContextError> {
        let snapshot = self.snapshot();
        if snapshot.is_null() || contains_merge_removal(&snapshot) {
            Err(ContextError::NullSnapshot {
                section: self.id().into(),
            })
        } else {
            Ok(snapshot)
        }
    }
}

/// Captures one section into `snapshot`, replacing its earlier state if it had
/// one.
///
/// A free function rather than a method, because the snapshot is a shared value
/// owned below this crate and a section is a live thing owned here. Rust cannot
/// add an inherent method to a foreign type, and the direction is the point:
/// the value does not know what a section is.
///
/// # Errors
///
/// [`ContextError::NullSnapshot`] before null can enter the set.
pub fn capture(
    snapshot: &mut ContextSnapshot,
    section: &impl ContextSection,
) -> Result<(), ContextError> {
    let state = section.checked_snapshot()?;
    snapshot.record(section.id(), state)
}

/// Resolves what retained history proves for one section.
///
/// Both halves are load-bearing. Recorded state without a retained fragment is
/// stale after compaction; a retained fragment without its typed state is
/// unknown and must be superseded defensively.
#[must_use]
pub fn seen<'a>(
    snapshot: &'a ContextSnapshot,
    section: &impl ContextSection,
    transcript: &Transcript,
) -> Seen<&'a Value> {
    let recognized = transcript.messages().iter().any(
        |message| matches!(message, Message::Context(fragment) if section.recognizes(fragment)),
    );

    match (snapshot.get(section.id()), recognized) {
        (Some(state), true) => Seen::Known(state),
        (Some(_), false) => Seen::Stale,
        (None, true) => Seen::Unknown,
        (None, false) => Seen::Fresh,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Message, Transcript};

    use crucible_types::{ContextError, ContextPatch, ContextSnapshot, Fragment, Seen};

    use super::{ContextSection, capture, seen};

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

    struct NullMemberSection;

    impl ContextSection for NullMemberSection {
        const ID: &'static str = "environment";

        fn snapshot(&self) -> Value {
            json!({ "date": null })
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
    fn null_object_members_are_rejected_before_merge_patch_can_read_them_as_removal() {
        let problem = NullMemberSection.checked_snapshot().unwrap_err();

        assert_eq!(
            problem,
            ContextError::NullSnapshot {
                section: "environment".into()
            }
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
        capture(&mut first, &z).unwrap();
        capture(&mut first, &a).unwrap();
        let mut second = ContextSnapshot::new();
        capture(&mut second, &a).unwrap();
        capture(&mut second, &z).unwrap();

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
        capture(&mut recorded, &section).unwrap();
        let mut retained = Transcript::new();
        retained
            .push(Message::Context(Fragment::new(
                Workspace::ID,
                "workspace is /work",
            )))
            .expect("valid fixture transcript");

        assert!(matches!(
            seen(&recorded, &section, &retained),
            Seen::Known(state) if state == &json!({ "root": "/work" })
        ));

        assert!(matches!(
            seen(&recorded, &section, &Transcript::new()),
            Seen::Stale
        ));

        assert!(matches!(
            seen(&ContextSnapshot::new(), &section, &retained),
            Seen::Unknown
        ));

        assert!(matches!(
            seen(&ContextSnapshot::new(), &section, &Transcript::new()),
            Seen::Fresh
        ));
    }

    #[test]
    fn reconciliation_after_history_rewrite_detects_the_removed_fragment() {
        let section = Workspace;
        let mut recorded = ContextSnapshot::new();
        capture(&mut recorded, &section).unwrap();
        let mut retained = Transcript::new();
        retained
            .push(Message::Context(Fragment::new(
                Workspace::ID,
                "workspace is /work",
            )))
            .expect("valid fixture transcript");

        assert!(matches!(
            seen(&recorded, &section, &retained),
            Seen::Known(_)
        ));

        retained.behind(1);

        assert!(matches!(seen(&recorded, &section, &retained), Seen::Stale));
    }

    #[test]
    fn snapshot_debug_redacts_model_visible_state() {
        let section = State {
            id: "workspace",
            value: json!({ "root": "/private/context-debug-canary" }),
        };
        let mut snapshot = ContextSnapshot::new();
        capture(&mut snapshot, &section).unwrap();

        let shown = format!("{snapshot:?}");

        assert!(!shown.contains("context-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
