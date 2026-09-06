//! The opt-in switch and the authority of each configuration file.

use crate::document::{Document, Origin};
use crate::{ConfigError, Settings};

#[test]
fn sandbox_mode_is_an_unknown_key_in_every_configuration_layer() {
    for origin in [Origin::User, Origin::Project, Origin::ProjectLocal] {
        for mode in [r#""required""#, r#""degraded""#, r#""off""#, "true", "null"] {
            for enabled in ["", r#""enabled":true,"#, r#""enabled":false,"#] {
                let text = format!(r#"{{"sandbox":{{{enabled}"mode":{mode}}}}}"#);
                let error = Document::parse(&text, "config.json", origin)
                    .expect_err("sandbox.enabled is the only confinement setting");
                assert!(
                    matches!(&error, ConfigError::UnknownKey { path, .. } if &**path == "sandbox.mode"),
                    "{origin:?}: {text}: {error}"
                );
                assert!(error.to_string().contains("enabled"), "{error}");
            }
        }
    }
}

#[test]
fn no_sandbox_choice_leaves_os_confinement_off() {
    assert!(!Settings::default().sandbox_enabled());
    assert!(!Settings::resolve(Vec::new()).sandbox_enabled());
    for text in [
        r"{}",
        r#"{"sandbox":{}}"#,
        r#"{"sandbox":{"$comment":"inert"}}"#,
    ] {
        for origin in [Origin::User, Origin::Project, Origin::ProjectLocal] {
            assert!(!Settings::resolve(vec![Document::sample(text, origin)]).sandbox_enabled());
        }
    }
}

#[test]
fn a_user_can_enable_required_confinement_or_explicitly_disable_it() {
    for (text, expected) in [
        (r#"{"sandbox":{"enabled":true}}"#, true),
        (r#"{"sandbox":{"enabled":false}}"#, false),
        (
            r#"{"sandbox":{"enabled":true,"$comment":"explicit","$schema":"https://example.test/schema"}}"#,
            true,
        ),
    ] {
        assert_eq!(
            Settings::resolve(vec![Document::sample(text, Origin::User)]).sandbox_enabled(),
            expected
        );
    }
}

#[test]
fn enabling_does_not_accept_a_string_number_or_null() {
    for value in [r#""true""#, "1", "null", "[]", "{}"] {
        let text = format!(r#"{{"sandbox":{{"enabled":{value}}}}}"#);
        assert!(
            matches!(Document::parse(&text, "config.json", Origin::User), Err(ConfigError::WrongType { path, .. }) if &*path == "sandbox.enabled")
        );
    }
}

#[test]
fn a_workspace_cannot_disable_confinement() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        assert!(matches!(
            Document::parse(r#"{"sandbox":{"enabled":false}}"#, "config.json", origin),
            Err(ConfigError::Widening { path, .. }) if &*path == "sandbox.enabled"
        ));
    }
}

#[test]
fn workspace_requirement_wins_regardless_of_document_order() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        let user = Document::sample(r#"{"sandbox":{"enabled":false}}"#, Origin::User);
        let project = Document::sample(r#"{"sandbox":{"enabled":true}}"#, origin);
        for documents in [
            vec![user.clone(), project.clone()],
            vec![project.clone(), user.clone()],
        ] {
            assert!(Settings::resolve(documents).sandbox_enabled());
        }
    }
}
