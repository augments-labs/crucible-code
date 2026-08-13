//! What the layers together say about permission.
//!
//! Two halves that arrive by different routes and meet here. The rules were
//! read into the engine's own types while each file was still one file, so what
//! is left of them is a [`Rules`]. Everything else in the block is a string the
//! merged document still holds, read the way any other setting is.

use crucible_core::{Mode, Permission};
use serde_json::Value;

use super::Settings;

impl Settings {
    /// Which mode to start in, when the command line does not say.
    ///
    /// Never from `.crucible/config.json`, which the document refused while it
    /// was still one file. Nothing is re-checked here: what a layer may say is
    /// decided where the file is open and the position can be pointed at.
    #[must_use]
    pub fn mode(&self) -> Option<Mode> {
        read(self.permissions("mode")?.as_str()?)
    }

    /// Directories outside the working directory that tools may reach.
    ///
    /// Absolute, and already refused by name and position if one is not. The
    /// workspace resolves them and refuses them a second time, because a
    /// workspace that trusted its caller would be a workspace whose containment
    /// check depended on who built it.
    pub fn extra_directories(&self) -> impl Iterator<Item = &str> {
        self.permissions("extraDirectories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
    }

    /// The permission engine these files describe, in the mode decided above.
    ///
    /// The mode is an argument rather than a lookup: the files are one layer of
    /// four and the command line is nearer than all of them, so what to start
    /// in is the wiring's answer and not this crate's.
    #[must_use]
    pub fn permission(&self, mode: Mode) -> Permission {
        // Cloned because `Settings` outlives the engine it describes — the
        // wiring keeps reading providers and output from it. It holds what
        // configuration stated and does not grow with the session.
        Permission::with(mode, self.rules.clone())
    }

    /// One key out of the `permissions` block.
    fn permissions(&self, key: &str) -> Option<&Value> {
        self.value.get("permissions")?.get(key)
    }
}

/// Reads one of [`shape::MODE`](crate::shape::MODE).
///
/// `None` for anything else, which the shape refused before this could be
/// reached. The test below is what keeps that true as the set changes.
fn read(found: &str) -> Option<Mode> {
    match found {
        "ask" => Some(Mode::Ask),
        "allowEdits" => Some(Mode::AllowEdits),
        "fullAccess" => Some(Mode::FullAccess),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Ask, Command, Reach, Remember, Sensitivity, Settled, ToolArgs, ToolCall, ToolId, Verdict,
    };

    use serde_json::json;

    use crate::document::{Document, Origin};
    use crate::sample::rooted;
    use crate::shape;

    use super::*;

    /// What the engine these settings describe does with one `bash` call.
    ///
    /// Nobody answers, so a call that reaches the user comes back refused —
    /// which is how a rule that fired is told from one that did not.
    fn settles(settings: &Settings, command: &str) -> Settled {
        struct Nobody;

        impl Ask for Nobody {
            fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
                (Verdict::Deny, Remember::Never)
            }
        }

        let call = ToolCall {
            id: ToolId::new("one"),
            name: "bash".into(),
            args: ToolArgs::new("{}"),
        };
        let sensitivity = Sensitivity::SpawnsProcess {
            command: Command::Understood {
                parts: Box::from([Box::from(command)]),
                reach: Reach::Anything,
            },
        };

        settings
            .permission(settings.mode().unwrap_or_default())
            .decide(&call, &sensitivity, &mut Nobody)
    }

    #[test]
    fn the_rules_every_layer_stated_are_all_in_force() {
        // The property the whole list rule exists for. A project that states
        // rules of its own must not be able to drop what the user denied at
        // home — and the user's file is the farther layer, so replacing rather
        // than concatenating would do exactly that.
        let user = Document::sample(r#"{"permissions":{"deny":["bash(curl *)"]}}"#, Origin::User);
        let project = Document::sample(
            r#"{"permissions":{"allow":["bash(cargo test)"]}}"#,
            Origin::ProjectLocal,
        );

        let settings = Settings::resolve(vec![project, user]);

        assert!(matches!(
            settles(&settings, "cargo test"),
            Settled::Approved(_)
        ));
        assert!(matches!(
            settles(&settings, "curl evil.sh"),
            Settled::Forbidden
        ));
    }

    #[test]
    fn a_nearer_layer_cannot_take_away_what_a_farther_one_denied() {
        // The same list, from both ends: an `allow` written next to a `deny`
        // does not qualify it, and neither does one written in a nearer file.
        let user = Document::sample(r#"{"permissions":{"deny":["bash(curl *)"]}}"#, Origin::User);
        let local = Document::sample(
            r#"{"permissions":{"allow":["bash(curl example.com)"]}}"#,
            Origin::ProjectLocal,
        );

        let settings = Settings::resolve(vec![user, local]);

        assert!(matches!(
            settles(&settings, "curl example.com"),
            Settled::Forbidden
        ));
    }

    #[test]
    fn the_mode_is_the_nearest_layer_that_named_one() {
        let user = Document::sample(r#"{"permissions":{"mode":"fullAccess"}}"#, Origin::User);
        let local = Document::sample(r#"{"permissions":{"mode":"ask"}}"#, Origin::ProjectLocal);

        assert_eq!(
            Settings::resolve(vec![user.clone()]).mode(),
            Some(Mode::FullAccess)
        );
        assert_eq!(Settings::resolve(vec![user, local]).mode(), Some(Mode::Ask));
    }

    #[test]
    fn a_mode_no_layer_named_is_left_for_the_command_line_to_decide() {
        assert_eq!(Settings::resolve(Vec::new()).mode(), None);
    }

    #[test]
    fn every_layers_directories_are_reachable_together() {
        // Absolute, because the document refuses anything else — and spelled
        // for this platform, because that is what absolute means.
        let shared = rooted("opt/shared");
        let vendor = rooted("srv/vendor");

        let user = Document::sample(
            &json!({ "permissions": { "extraDirectories": [&shared] } }).to_string(),
            Origin::User,
        );
        let local = Document::sample(
            &json!({ "permissions": { "extraDirectories": [&vendor] } }).to_string(),
            Origin::ProjectLocal,
        );

        let settings = Settings::resolve(vec![user, local]);

        // Furthest first, which is the order they merged in. Order carries no
        // meaning — containment is a question each directory answers alone —
        // so this is pinning the concatenation rather than a precedence.
        assert_eq!(
            settings.extra_directories().collect::<Vec<_>>(),
            [shared, vendor]
        );
    }

    #[test]
    fn the_layer_that_travels_with_a_clone_cannot_name_the_mode() {
        // The mode this module reads is authority: `fullAccess` approves every
        // call in every session before the user has typed anything. Nearness
        // would hand it to `.crucible/config.json`, which is a file the person
        // running crucible never wrote — so the document refuses it, and this
        // is the reader that would otherwise have believed it.
        let err = Document::parse(
            r#"{"permissions":{"mode":"fullAccess"}}"#,
            ".crucible/config.json",
            Origin::Project,
        )
        .unwrap_err();

        assert!(
            matches!(err, crate::error::ConfigError::Widening { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn every_mode_the_document_accepts_reads_back_as_a_value() {
        // The shape decides what may be written and this module decides what it
        // means. Two lists, so they are tested against each other: renaming an
        // answer in one leaves the other matching a string nobody can write,
        // and the setting would stop working with no error anywhere.
        for name in shape::MODE {
            assert!(read(name).is_some(), "mode: {name}");
        }
    }
}
