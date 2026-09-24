use crucible_core::{
    Ask, Command, Remember, Sensitivity, Settled, ToolArgs, ToolCall, ToolId, Verdict,
};

use serde_json::json;

use crate::document::{Document, Origin};
use crate::sample::rooted;
use crate::shape;

use super::*;

/// Drives a future to its answer, the way a test drives a fake's future by
/// hand: this suite's [`Ask`] fake never really waits, so asking it once
/// always has one, and a fake that did not would fail here rather than hang.
///
/// A plain poll rather than `crucible_runtime::answered!`: this crate does
/// not depend on `crucible-runtime`, whose only shipped use here would be
/// this one test-only macro, and the workspace's crate layering keeps that
/// edge out of `crucible-config`. The manual poll belongs here, in a file
/// this workspace's bridge-ledger check reads as a test rather than as
/// shipped source, and not beside the settings it exercises.
fn answered_now<F: std::future::Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("the fake did not answer on the first poll"),
    }
}

/// What the engine these settings describe does with one `bash` call.
///
/// Nobody answers, so a call that reaches the user comes back refused — which
/// is how a rule that fired is told from one that did not.
fn settles(settings: &Settings, command: &str) -> Settled {
    struct Nobody;

    impl Ask for Nobody {
        fn ask<'a>(
            &'a mut self,
            _call: &'a ToolCall,
            _sensitivity: &'a Sensitivity,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (Verdict, Remember)> + Send + 'a>>
        {
            Box::pin(async { (Verdict::Deny, Remember::Never) })
        }
    }

    let call = ToolCall {
        id: ToolId::new("one"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    };
    let sensitivity = Sensitivity::SpawnsProcess {
        command: Command::Understood {
            sent: command.into(),
            parts: Box::from([Box::from(command)]),
        },
    };

    answered_now(
        settings
            .permission(settings.mode().unwrap_or_default())
            .decide(&call, &sensitivity, &mut Nobody),
    )
}

#[test]
fn narrowing_rules_from_every_layer_are_all_in_force() {
    // Project files can add refusals and questions but no silent allows.
    // Both kinds still concatenate with the user's own policy.
    let user = Document::sample(
        r#"{"permissions":{"mode":"fullAccess","allow":["bash(cargo test)"]}}"#,
        Origin::User,
    );
    let project = Document::sample(
        r#"{"permissions":{"deny":["bash(curl *)"]}}"#,
        Origin::Project,
    );
    let local = Document::sample(
        r#"{"permissions":{"ask":["bash(git push)"]}}"#,
        Origin::ProjectLocal,
    );

    let settings = Settings::resolve(vec![project, user, local]);

    assert!(matches!(
        settles(&settings, "cargo test"),
        Settled::Approved(_)
    ));
    assert!(matches!(
        settles(&settings, "curl evil.sh"),
        Settled::Forbidden
    ));
    assert!(matches!(settles(&settings, "git push"), Settled::Refused));
}

#[test]
fn a_nearer_denial_beats_a_farther_allow() {
    let user = Document::sample(
        r#"{"permissions":{"allow":["bash(curl example.com)"]}}"#,
        Origin::User,
    );
    let local = Document::sample(
        r#"{"permissions":{"deny":["bash(curl *)"]}}"#,
        Origin::ProjectLocal,
    );

    let settings = Settings::resolve(vec![user, local]);

    assert!(matches!(
        settles(&settings, "curl example.com"),
        Settled::Forbidden
    ));
}

#[test]
fn the_mode_is_read_from_the_user_layer() {
    let user = Document::sample(r#"{"permissions":{"mode":"fullAccess"}}"#, Origin::User);

    assert_eq!(Settings::resolve(vec![user]).mode(), Some(Mode::FullAccess));
}

#[test]
fn a_mode_no_layer_named_is_left_for_the_command_line_to_decide() {
    assert_eq!(Settings::resolve(Vec::new()).mode(), None);
}

#[test]
fn every_user_named_directory_is_reachable() {
    // Absolute, because the document refuses anything else — and spelled for
    // this platform, because that is what absolute means.
    let shared = rooted("opt/shared");
    let vendor = rooted("srv/vendor");

    let user = Document::sample(
        &json!({ "permissions": { "extraDirectories": [&shared, &vendor] } }).to_string(),
        Origin::User,
    );

    let settings = Settings::resolve(vec![user]);

    // Order carries no meaning — containment is a question each directory
    // answers alone — so this pins the list read rather than precedence.
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
    // running crucible never wrote — so the document refuses it, and this is
    // the reader that would otherwise have believed it.
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
    // answer in one leaves the other matching a string nobody can write, and
    // the setting would stop working with no error anywhere.
    for name in shape::MODE {
        assert!(read(name).is_some(), "mode: {name}");
    }
}

#[test]
fn the_default_the_schema_states_for_mode_is_the_one_it_falls_back_to() {
    assert_eq!(
        read(shape::usual(&["permissions", "mode"])),
        Some(Mode::default())
    );
}
