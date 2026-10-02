//! A prompt with nobody to ask it of.
//!
//! Interactively each of these is a warning and the session carries on,
//! because `/model` or `/login` is a key away. With input and output both
//! redirected nobody can type either, so the run says why once, takes no
//! turn, and ends with an error rather than `Ok`. Neither records the prompt.

use crucible_types::Message;

use crate::cli::converse::{Answers, Held, queueing};
use crate::cli::sample::Sample;
use crate::cli::style::Style;

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

/// A conversation recording into `session`, on `model` and the provider a
/// machine with nothing set up stands in: it reaches no model.
fn standing_in(session: &Arc<Session>, model: &str) -> Conversation {
    Conversation::recording(Arc::clone(session), None, |session| {
        Runner::new(
            Box::new(crucible_provider::Unavailable::new(
                crucible_app::providers::NOTHING_TO_ASK,
            )),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                Model {
                    name: model.into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        )
    })
}

/// The prompts `session` recorded, read back off the disk once it is closed.
fn recorded(sample: &Sample, session: Arc<Session>) -> Vec<Message> {
    assert_eq!(session.finish(), None);
    drop(session);
    let (_, transcript) =
        Session::resume(&sample.logs(), &sample.workspace()).expect("the session");
    transcript
        .messages()
        .iter()
        .filter(|message| matches!(message, Message::User { .. }))
        .cloned()
        .collect()
}

/// Whether the screen warned of a prompt and took no turn for it.
///
/// The warning alone does not tell the two apart: the stand-in refuses a turn
/// with the same sentence. A turn is what draws the thinking row, so its
/// absence is the turn that was never taken.
fn warned_without_a_turn(written: &str) -> bool {
    written.contains("No models available") && !written.contains("thinking")
}

#[test]
fn a_typed_prompt_for_a_model_nothing_serves_is_warned_of_as_one_with_no_model_is() {
    // At a terminal a session with no model answers a prompt with the warning
    // and takes no turn. `--model foo` with nothing set up is the same session
    // with a name in it: the provider standing in would refuse the turn, but
    // only after the prompt was recorded as said to a model nobody asked.
    let sample = Sample::new("unserved-typed");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let conversation = standing_in(&session, "foo");

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"what is 2+2\n".to_vec());

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the session to carry on past the warning");

    let said = recorded(&sample, session);
    assert!(said.is_empty(), "a prompt was recorded: {said:?}");
    let written = renderer.terminal().written();
    assert!(warned_without_a_turn(written), "{written}");
}

/// What a line queued behind the last turn comes to, on `model` and the
/// stand-in, with what was recorded and what was drawn.
fn queued_for(
    name: &str,
    model: &str,
    renderer: &mut Renderer<Recording>,
) -> (Result<Option<bool>, Fatal>, Vec<Message>) {
    let sample = Sample::new(name);
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let conversation = standing_in(&session, model);

    let card = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        crucible_builtins::Plan::new(),
        crucible_tui::Sending::default(),
        Answers {
            input: &mut input,
            keys: false,
        },
        &card,
    );
    let mut editor = typed("what is 2+2");
    assert_eq!(held.queued.accept(&mut editor), Retained::Accepted);

    let terms = plain();
    let taken = queueing::taken(conversation, renderer, &terms, &mut held, Style::plain())
        .map(|(_, leaving)| leaving);
    assert_eq!(held.queued.waiting_count(), 0, "the line is still waiting");

    (taken, recorded(&sample, session))
}

#[test]
fn a_queued_prompt_for_a_model_nothing_serves_is_warned_of_as_a_typed_one_is() {
    // A line typed while `/compact` made room is queued and taken as the next
    // turn without passing the box. It is owed what the same line typed at the
    // box gets: the warning, and nothing recorded.
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let (taken, said) = queued_for("unserved-queued", "foo", &mut renderer);

    assert!(said.is_empty(), "a prompt was recorded: {said:?}");
    assert!(matches!(taken, Ok(Some(false))), "{taken:?}");
    let written = renderer.terminal().written();
    assert!(warned_without_a_turn(written), "{written}");
}

#[test]
fn a_queued_prompt_with_no_model_is_warned_of_as_a_typed_one_is() {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let (taken, said) = queued_for("no-model-queued", "", &mut renderer);

    assert!(said.is_empty(), "a prompt was recorded: {said:?}");
    assert!(matches!(taken, Ok(Some(false))), "{taken:?}");
    let written = renderer.terminal().written();
    assert!(warned_without_a_turn(written), "{written}");
}

#[test]
fn a_queued_prompt_nothing_serves_down_a_pipe_fails_as_a_typed_one_does() {
    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let (taken, said) = queued_for("unserved-queued-piped", "foo", &mut renderer);

    assert!(said.is_empty(), "a prompt was recorded: {said:?}");
    assert!(
        matches!(
            taken,
            Err(Fatal::Unanswerable(crucible_app::providers::NOTHING_TO_ASK))
        ),
        "{taken:?}"
    );
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

#[test]
fn compact_with_no_model_to_ask_is_warned_of_as_a_prompt_is() {
    // Room is made by asking the model for a recap, so with nobody to ask it
    // is the same answer a prompt gets: the warning, and nothing recorded.
    let sample = Sample::new("unserved-compact");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let conversation = standing_in(&session, "foo");

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/compact\n".to_vec());

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the session to carry on past the warning");

    let said = recorded(&sample, session);
    assert!(said.is_empty(), "something was recorded: {said:?}");
    let written = renderer.terminal().written();
    assert!(written.contains("No models available"), "{written}");
    assert!(!written.contains("worth replacing"), "{written}");
}

#[test]
fn piped_compact_with_no_model_to_ask_fails_as_a_piped_prompt_does() {
    // Down a pipe nobody can type `/model` after the warning, so a request for
    // room with nobody to ask ends the run the way a prompt does, not `Ok`.
    let sample = Sample::new("unserved-compact-piped");
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let conversation = standing_in(&session, "foo");

    let mut renderer = Renderer::new(Recording::redirected(80, 24));
    let mut input = Cursor::new(b"/compact\n".to_vec());

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

    let said = recorded(&sample, session);
    assert!(said.is_empty(), "something was recorded: {said:?}");
    let problem = ended.expect_err("a run that answered nothing to fail");
    assert!(
        matches!(
            problem,
            Fatal::Unanswerable(crucible_app::providers::NOTHING_TO_ASK)
        ),
        "{problem:?}"
    );
}

/// A transcript long enough that a recap has an older middle to replace.
fn long_enough() -> crucible_types::Transcript {
    let mut transcript = crucible_types::Transcript::new();
    let said = "a long thing said ".repeat(2_000);
    for _ in 0..12 {
        transcript
            .push(Message::said(said.as_str()))
            .expect("valid fixture transcript");
        transcript
            .push(Message::Agent {
                continuation: None,
                text: said.as_str().into(),
                calls: Vec::new(),
                stop: Some(StopReason::Yielded),
            })
            .expect("valid fixture transcript");
    }
    transcript
}

#[test]
fn a_resumed_compaction_with_no_model_sends_nothing_and_says_what_is_missing() {
    // "Carry on from summary" on a session picked up with a provider served
    // and no model chosen is a recap request naming no model. It is answered
    // the way `/compact` is: the warning, and no request.
    let script = Script::new(vec![saying("a recap")]);
    let asked = script.asked();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        Runner::new(
            Box::new(script),
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
        .resuming(long_enough())
    });
    let terms = plain();
    let card = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        terms.plan.clone(),
        terms.sending.get(),
        Answers {
            input: &mut input,
            keys: false,
        },
        &card,
    );
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let ran = crate::cli::converse::ran(
        conversation,
        &mut renderer,
        &terms,
        crate::cli::converse::Work::Room(Compacting::Resumed),
        &mut held,
    );

    assert!(
        matches!(ran, Ok((_, false))),
        "{:?}",
        ran.map(|(_, left)| left)
    );
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 0);
    let written = renderer.terminal().written();
    assert!(written.contains("No model selected"), "{written}");
}
