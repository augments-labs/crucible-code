//! What taking a model, by name or off the shelf, leaves the session asking.

use std::sync::Arc;

use crucible_core::AgentId;
use crucible_runner::{Agent, Model as RunnerModel, Tools};
use crucible_session::Session;
use crucible_tui::{Glyphs, Recording, Renderer};

use crate::cli::converse::tests::{keeping, plain};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;

use crucible_app::Conversation;
use crucible_app::providers::Providers;

use super::{Effort, Selected, applied, keys, offered, taken};

/// The built-in providers, as one generation the rows are read off.
fn catalogue() -> Providers {
    crucible_app::providers::providers()
        .expect("the built-in providers register")
        .snapshot()
}

#[test]
fn the_keys_under_the_panes_come_out_of_the_glyph_set() {
    // The row naming the keys is the whole of what teaches somebody
    // standing at the shelf how to walk it and how to leave it. A terminal
    // without the arrows draws four hollow squares on the one row that
    // exists to be read by somebody who does not yet know.
    assert_eq!(
        keys(Glyphs::Unicode),
        (
            "tab pane \u{b7} \u{2191}\u{2193} model \u{b7} \u{2190}\u{2192} effort \u{b7} enter takes both \u{b7} esc to cancel"
                .to_owned(),
            "tab \u{b7} \u{2191}\u{2193} \u{b7} \u{2190}\u{2192} \u{b7} enter \u{b7} esc".to_owned(),
        )
    );
    assert_eq!(
        keys(Glyphs::Ascii),
        (
            "tab pane - ^v model - <> effort - enter takes both - esc to cancel".to_owned(),
            "tab - ^v - <> - enter - esc".to_owned(),
        )
    );
}

/// A conversation `serving` answers, asking `model` at `effort` from an
/// empty script.
fn conversing(
    serving: Option<&'static str>,
    model: &str,
    window: Option<u32>,
    effort: Option<Effort>,
) -> Conversation {
    Conversation::recording(Arc::new(Session::nowhere()), serving, |session| {
        let mut runner = crucible_runner::Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Agent::new(
                AgentId::new("test"),
                RunnerModel {
                    name: model.into(),
                    max_tokens: 17,
                    window,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_context::ContextInputs::new(std::env::temp_dir()),
            session,
        );
        if let Some(effort) = effort {
            runner.think(effort);
        }
        runner
    })
}

/// A conversation asking for `old` and nothing else, to take a row against.
fn asking() -> Conversation {
    conversing(Some("anthropic"), "old", Some(99), None)
}

/// A conversation with nothing to ask, as a run with no credential anywhere
/// gets.
fn unasked() -> Conversation {
    conversing(None, "", None, None)
}

/// The row for one model of one provider, by both names.
fn row(provider: &str, model: &str) -> Selected {
    let provider = offered(&catalogue())
        .find(|one| one.name == provider)
        .expect("a served provider");
    let model = provider
        .models
        .iter()
        .find(|one| one.name == model)
        .copied()
        .expect("a served model");

    Selected { provider, model }
}

#[test]
fn a_row_whose_provider_cannot_be_reached_takes_nothing_and_says_it_once() {
    // One sentence, and the one that names what is actually missing. Going
    // on to the rung reaches `/effort`, which finds no provider set and
    // says the session has no model at all -- a second warning, about a
    // different missing thing, printed under the first and contradicting
    // the model still in force.
    // The machine the reader is on: no key for anything, so no provider was
    // resolved and no model was ever asked for.
    let terms = plain();
    let mut conversation = unasked();
    let mut renderer = Renderer::new(Recording::new(80, 24));

    applied(
        row("moonshot", "k3"),
        Some(0),
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the row to be answered");

    let written = renderer.terminal().written().to_string();
    assert!(written.contains("! "), "{written}");
    assert!(!written.contains("No model selected"), "{written}");
    assert!(!written.contains("No models available"), "{written}");
    assert!(
        conversation.runner().model().is_empty(),
        "{}",
        conversation.runner().model()
    );
}

#[test]
fn taking_a_row_asks_for_the_model_and_then_the_rung_marked_under_it() {
    // Both halves, in that order. A rung is asked of a model, so a shelf
    // that applied the rung first would be asking it of the model being
    // left behind.
    let sample = Sample::new("model-row-and-rung");
    let terms = keeping(&sample);
    let mut conversation = asking();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let selected = row("anthropic", "claude-sonnet-5");
    let at = selected
        .model
        .rungs
        .iter()
        .position(|rung| *rung == Effort::Xhigh)
        .expect("a model that serves xhigh");

    applied(selected, Some(at), &mut renderer, &mut conversation, &terms)
        .expect("the row to be taken");

    assert_eq!(conversation.runner().model(), "claude-sonnet-5");
    assert_eq!(conversation.runner().effort(), Some(Effort::Xhigh));
}

#[test]
fn google_model_switch_requires_an_explicit_compatible_effort() {
    let sample = Sample::new("model-google-effort");
    let mut terms = keeping(&sample);
    terms.serving = Box::new(|_, _| {
        Ok(crucible_app::providers::Resolved {
            provider: Box::new(Script::new(Vec::new())),
            source: crucible_app::providers::CredentialSource::StoredKey,
        })
    });
    let mut conversation = conversing(Some("anthropic"), "old", Some(99), Some(Effort::Max));
    let mut renderer = Renderer::new(Recording::new(100, 24));
    let google = row("google", "gemini-3.8-flash");
    super::run(
        "google/gemini-3.8-flash",
        &mut renderer,
        &mut conversation,
        &terms,
        false,
    )
    .unwrap();
    assert_eq!(
        conversation.runner().model(),
        "old",
        "an incompatible inherited rung must not silently cross providers"
    );
    assert_eq!(conversation.runner().effort(), Some(Effort::Max));
    assert_eq!(conversation.serving(), Some("anthropic"));
    assert!(renderer.terminal().written().contains("effort"));
    applied(google, Some(2), &mut renderer, &mut conversation, &terms).unwrap();
    assert_eq!(conversation.runner().model(), "gemini-3.8-flash");
    assert_eq!(conversation.runner().effort(), Some(Effort::High));
    assert_eq!(conversation.serving(), Some("google"));
    super::super::effort::run("xhigh", &mut renderer, &mut conversation, &terms, false).unwrap();
    assert_eq!(
        conversation.runner().effort(),
        Some(Effort::High),
        "an unsupported typed rung must leave the selected rung unchanged"
    );
}

#[test]
fn taking_a_model_that_serves_no_rung_leaves_the_rung_exactly_as_it_was() {
    // Not an error and nothing said about it. The row carried `no rung`
    // and the strip carried the same sentence, so a session that took it
    // has already been told.
    let sample = Sample::new("model-no-rung");
    let terms = keeping(&sample);
    let mut conversation = conversing(Some("anthropic"), "old", Some(99), Some(Effort::High));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let selected = row("anthropic", "claude-haiku-4-5");
    assert!(selected.model.rungs.is_empty());

    applied(selected, None, &mut renderer, &mut conversation, &terms).expect("the row to be taken");

    assert_eq!(conversation.runner().model(), "claude-haiku-4-5");
    assert_eq!(conversation.runner().effort(), Some(Effort::High));
}

#[test]
fn taking_a_model_replaces_name_output_and_startup_resolved_window_together() {
    let sample = Sample::new("model-runtime-limits");
    let mut terms = keeping(&sample);
    terms.settings = sample
        .settings(r#"{"providers":{"anthropic":{"contextWindow":{"claude-haiku-4-5":345678}}}}"#);
    let mut conversation = asking();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let anthropic = offered(&catalogue())
        .find(|provider| provider.name == "anthropic")
        .expect("anthropic is served");

    taken(
        anthropic,
        ("claude-haiku-4-5", None),
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the model to be taken");

    assert_eq!(conversation.runner().model(), "claude-haiku-4-5");
    assert_eq!(conversation.runner().maximum_output(), 16_000);
    assert_eq!(conversation.runner().context_window(), Some(345_678));
}
