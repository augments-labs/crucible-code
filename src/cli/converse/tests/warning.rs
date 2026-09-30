//! A send on a route whose vendor says it uses what is sent, in a run with
//! no terminal to put the question on.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crucible_app::content_use::{Consent, Routes, Serving, WARNED};
use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::{Recording, Renderer};

use crate::cli::Fatal;
use crate::cli::converse::{First, Terms, converse};
use crate::cli::fake::Script;

use super::{opening, paired, plain, saying, scripted};

/// Consent that serves the fake provider on the Google key row's route.
fn serving_google() -> Consent {
    let consent = Consent::new(Routes::production());
    consent.served(
        "anthropic",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    consent
}

/// What typing `typed` down a pipe came to, and how many requests the
/// provider was asked.
fn piped(consent: Consent, typed: &str) -> (Result<(), Fatal>, usize, String) {
    let terms = Terms { consent, ..plain() };
    let script = Script::new(vec![saying("answered")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(script, Tools::new(), session)
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    let ended = converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .map(|_| ());
    let written = renderer.terminal().written().to_string();
    (ended, asked.load(Ordering::SeqCst), written)
}

#[test]
fn down_a_pipe_a_warned_route_with_no_yes_ends_the_run_and_sends_nothing() {
    let (ended, asked, _) = piped(serving_google(), "hello\n");

    assert_eq!(asked, 0, "a request was sent");
    let Err(Fatal::Unanswered(said)) = ended else {
        panic!("the run went on: {ended:?}");
    };
    let google = WARNED
        .iter()
        .find(|warned| warned.route == "key:google")
        .expect("the Google key row is warned");
    assert!(said.contains(google.warning.sentence), "{said}");
    assert!(said.ends_with("answer it once in a terminal."), "{said}");
    assert!(
        crate::cli::Fatal::Unanswered(said.clone())
            .to_string()
            .contains("Nothing was sent")
    );
}

#[test]
fn a_route_already_said_yes_to_is_sent_on_without_a_question() {
    let consent = serving_google();
    consent.recorded(["key:google".to_owned()]);

    let (ended, asked, written) = piped(consent, "hello\n");

    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(asked, 1);
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn a_route_its_vendor_says_nothing_about_is_never_asked_about() {
    let consent = Consent::new(Routes::production());
    consent.served(
        "anthropic",
        Some(Serving {
            route: Some("key:anthropic".to_owned()),
            at: None,
        }),
    );

    let (ended, asked, _) = piped(consent, "hello\n");

    assert!(ended.is_ok(), "{ended:?}");
    assert_eq!(asked, 1);
}
