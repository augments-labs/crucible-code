//! The opt-in switch, its compatibility spelling, and the authority of each file.

use crucible_core::SandboxMode;

use crate::document::{Document, Origin};
use crate::{ConfigError, Settings};

#[test]
fn no_sandbox_choice_leaves_os_confinement_off() {
    assert_eq!(Settings::default().sandbox_mode(), SandboxMode::Off);
    assert_eq!(
        Settings::resolve(Vec::new()).sandbox_mode(),
        SandboxMode::Off
    );
    for text in [
        r"{}",
        r#"{"sandbox":{}}"#,
        r#"{"sandbox":{"$comment":"inert"}}"#,
    ] {
        for origin in [Origin::User, Origin::Project, Origin::ProjectLocal] {
            assert_eq!(
                Settings::resolve(vec![Document::sample(text, origin)]).sandbox_mode(),
                SandboxMode::Off
            );
        }
    }
}

#[test]
fn a_user_can_enable_required_confinement_or_explicitly_disable_it() {
    for (text, expected) in [
        (r#"{"sandbox":{"enabled":true}}"#, SandboxMode::Required),
        (r#"{"sandbox":{"enabled":false}}"#, SandboxMode::Off),
        (r#"{"sandbox":{"mode":"required"}}"#, SandboxMode::Required),
        (r#"{"sandbox":{"mode":"degraded"}}"#, SandboxMode::Degraded),
        (r#"{"sandbox":{"mode":"off"}}"#, SandboxMode::Off),
        (
            r#"{"sandbox":{"enabled":true,"$comment":"explicit","$schema":"https://example.test/schema"}}"#,
            SandboxMode::Required,
        ),
    ] {
        assert_eq!(
            Settings::resolve(vec![Document::sample(text, Origin::User)]).sandbox_mode(),
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
fn a_workspace_cannot_disable_confinement_with_either_spelling() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        for text in [
            r#"{"sandbox":{"enabled":false}}"#,
            r#"{"sandbox":{"mode":"off"}}"#,
            r#"{"sandbox":{"mode":"degraded"}}"#,
        ] {
            assert!(matches!(
                Document::parse(text, "config.json", origin),
                Err(ConfigError::Widening { .. })
            ));
        }
    }
}

#[test]
fn workspace_requirement_wins_across_spellings_and_document_order() {
    for origin in [Origin::Project, Origin::ProjectLocal] {
        for project in [
            r#"{"sandbox":{"enabled":true}}"#,
            r#"{"sandbox":{"mode":"required"}}"#,
        ] {
            for user in [
                r#"{"sandbox":{"enabled":false}}"#,
                r#"{"sandbox":{"mode":"degraded"}}"#,
                r#"{"sandbox":{"mode":"off"}}"#,
            ] {
                let user = Document::sample(user, Origin::User);
                let project = Document::sample(project, origin);
                for documents in [
                    vec![user.clone(), project.clone()],
                    vec![project.clone(), user.clone()],
                ] {
                    assert_eq!(
                        Settings::resolve(documents).sandbox_mode(),
                        SandboxMode::Required
                    );
                }
            }
        }
    }
}

#[test]
fn neither_spelling_can_hide_a_second_choice_in_the_same_document() {
    for enabled in [true, false] {
        for mode in ["required", "degraded", "off"] {
            let text = format!(r#"{{"sandbox":{{"enabled":{enabled},"mode":"{mode}"}}}}"#);
            let error = Document::parse(&text, "config.json", Origin::User)
                .expect_err("two choices must be refused");
            let said = error.to_string();
            assert!(
                said.contains("sandbox.enabled")
                    && said.contains("sandbox.mode")
                    && said.contains("cannot be combined"),
                "{said}"
            );
        }
    }
}
