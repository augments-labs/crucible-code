//! A prompt arriving down a pipe with nobody to ask it of.
//!
//! Interactively each of these is a warning and the session carries on,
//! because `/model` or `/login` is a key away. With input and output both
//! redirected nobody can type either, so the run says why once, takes no
//! turn, and ends with an error rather than `Ok`.

use crucible_types::Message;

use crate::cli::sample::Sample;

use super::*;

#[test]
fn a_prompt_that_cannot_be_answered_down_a_pipe_fails_rather_than_ending_quietly() {
    // Interactively this is a warning and the session carries on, because
    // `/model` is a key away. Down a pipe nobody can type it, so every line
    // after this one would be read and none of them answered — and the run
    // would end `Ok`, which is the one thing a script looks at. `echo ... |
    // crucible` reporting success while answering nothing is the "it does
    // nothing" report arriving as a zero exit.
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: String::new().into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
    });

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"what is 2+2\n".to_vec());

    let problem = converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect_err("a run that answered nothing to fail");

    assert!(matches!(problem, Fatal::Unanswerable(_)), "{problem:?}");
}
#[test]
fn a_piped_prompt_for_a_model_nothing_serves_fails_as_one_with_no_model_does() {
    // `--model foo` on a machine with nothing set up names a model and leaves
    // nobody to ask it of: the provider standing in refuses every turn. Down a
    // pipe that is the same unanswerable run as one with no model at all, and
    // it owes the same ending: said once, nothing recorded, and not `Ok`.
    let sample = Sample::new("unserved-piped");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let conversation = Conversation::recording(Arc::clone(&session), None, |session| {
        Runner::new(
            Box::new(crucible_provider::Unavailable::new(
                crucible_app::providers::NOTHING_TO_ASK,
            )),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: "foo".into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
    });

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"what is 2+2\nand 3+3\n".to_vec());

    let ended = converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    );

    // Read back before the outcome is judged, so that a run which took the
    // turn shows what it recorded rather than only that it ended `Ok`.
    assert_eq!(session.finish(), None);
    drop(session);
    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    let said: Vec<_> = transcript
        .messages()
        .iter()
        .filter(|message| matches!(message, Message::User { .. }))
        .collect();
    assert!(said.is_empty(), "a prompt was recorded: {said:?}");

    let problem = ended.expect_err("a run that answered nothing to fail");
    assert!(
        matches!(
            problem,
            Fatal::Unanswerable(crucible_app::providers::NOTHING_TO_ASK)
        ),
        "{problem:?}"
    );
    // Said once, by the error: no turn was refused on the screen before it.
    let written = renderer.terminal().written();
    assert!(!written.contains("No models available"), "{written}");
}

/// What a piped prompt ends the run with, in a session that chose no provider.
fn unanswered_without_a_provider(terms: &Terms) -> Fatal {
    let conversation = Conversation::recording(Arc::new(Session::nowhere()), None, |session| {
        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: String::new().into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
    });

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"what is 2+2\n".to_vec());

    converse(
        conversation,
        &mut renderer,
        terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect_err("a run that answered nothing to fail")
}

#[test]
fn a_piped_prompt_with_keys_for_two_providers_and_neither_chosen_says_choose_one() {
    // Two keys and nothing choosing between them is the state the welcome
    // calls "no provider selected". The prompt after it is the same state, so
    // it owes the same sentence: telling somebody holding two keys to go and
    // set one sends them to check the half that was never wrong.
    let sample = Sample::new("no-provider-piped");
    sample.stored("anthropic");
    sample.stored("openai");

    let problem = unanswered_without_a_provider(&Terms {
        logins: sample.store(),
        ..plain()
    });

    assert!(
        matches!(
            problem,
            Fatal::Unanswerable(crucible_app::providers::NO_PROVIDER_CHOSEN)
        ),
        "{problem:?}"
    );
}

#[test]
fn an_exported_key_counts_toward_which_warning_a_piped_prompt_gets() {
    // Nothing stored and one key exported is a provider set up, the way the
    // launch counts it; with no key anywhere the first warning is the one owed.
    let exported = unanswered_without_a_provider(&Terms {
        environment: Box::new(|name| (name == "OPENAI_API_KEY").then(|| "sk-sample".to_owned())),
        ..plain()
    });
    let bare = unanswered_without_a_provider(&plain());

    assert!(
        matches!(
            exported,
            Fatal::Unanswerable(crucible_app::providers::NO_PROVIDER_CHOSEN)
        ),
        "{exported:?}"
    );
    assert!(
        matches!(
            bare,
            Fatal::Unanswerable(crucible_app::providers::NOTHING_TO_ASK)
        ),
        "{bare:?}"
    );
}
