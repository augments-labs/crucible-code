//! What taking a model, by name or off the shelf, leaves the session asking.

use std::sync::Arc;

use crucible_runner::{Agent, Model as RunnerModel, Tools};
use crucible_session::Session;
use crucible_tui::{Glyphs, Recording, Renderer};
use crucible_types::AgentId;

use crate::cli::converse::tests::{keeping, plain};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;

use crucible_app::Conversation;
use crucible_app::providers::Providers;

use super::{Asked, Effort, Selected, answered, applied, in_force, keys, offered, taken, titled};

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

/// What the status row under the box says of the model, at a width that holds
/// every fact on it: what follows the last run of two spaces, which at this
/// width is the label, itself spaced singly.
fn under_the_box(provider: &str, model: &str, effort: Option<&str>, glyphs: Glyphs) -> String {
    let prompt = crucible_tui::Prompt {
        draft: crucible_tui::Draft::at("", 0),
        left: crucible_tui::Remaining::new(None),
        history: crucible_tui::Recalled::default(),
        mode: "ask mode on",
        tone: crucible_tui::Slot::Quiet,
        hint: "",
        model,
        provider,
        effort,
        speed: None,
        asking: None,
        commands: crucible_tui::CommandCount::new(0, false),
        room: 10,
        named: &[],
    };
    let rows: Vec<String> = prompt
        .rows(200, glyphs)
        .iter()
        .map(crucible_tui::Row::text)
        .collect();
    let status = rows
        .iter()
        .find(|row| row.starts_with("ask mode on"))
        .unwrap_or_else(|| panic!("no status row in {rows:#?}"));
    status
        .trim_end()
        .rsplit("  ")
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn every_place_the_model_is_drawn_names_it_the_same_way() {
    // The status row, the shelf's title, the row answering `/model` and the
    // list printed where no shelf fits each say which model the next turn is
    // asked of. For one state they say it in one form, for every model the
    // registry holds, so a model added later is held to it with no new test.
    // This holds the status row as drawn and the builder each other site
    // calls; which rung the sites hand those builders is held by the tests
    // that take a model below, and the drawn rows by the pictures.
    let providers = catalogue();
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let dot = glyphs.dot();
        for served in offered(&providers) {
            for model in served.models {
                let rungs = std::iter::once(None).chain(model.rungs.iter().copied().map(Some));
                for effort in rungs {
                    let wanted = match effort {
                        Some(effort) => format!(
                            "{} {dot} {} {dot} {}",
                            served.name,
                            model.name,
                            effort.as_str()
                        ),
                        None => format!("{} {dot} {}", served.name, model.name),
                    };
                    let at = format!("{} {} {effort:?} {glyphs:?}", served.name, model.name);
                    assert_eq!(
                        under_the_box(served.name, model.name, effort.map(Effort::as_str), glyphs),
                        wanted,
                        "status row, {at}"
                    );
                    let current = Asked {
                        provider: Some(served.name),
                        model: model.name,
                        effort: effort.map(Effort::as_str),
                        pace: super::super::Pace::default(),
                    };
                    assert_eq!(
                        titled(current, glyphs),
                        format!("now  {wanted}"),
                        "shelf title, {at}"
                    );
                    assert_eq!(
                        answered(served.name, model.name, effort, glyphs),
                        wanted,
                        "answer row, {at}"
                    );
                    assert_eq!(
                        in_force(Some(served.name), model.name, effort, glyphs),
                        wanted,
                        "list, {at}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_accepted_picture_draws_a_model_the_way_it_is_typed() {
    // The label reads `provider · model`; `provider/model` is the form typed
    // after `/model` and `--model`, and a line printed for somebody to type
    // keeps it. So a picture may hold the slash only right after one of those,
    // or where a built-in provider's name is only the tail of a longer word.
    // What is read is every accepted `.snap` picture of the tree.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let providers = catalogue();
    let names: Vec<&str> = offered(&providers).map(|served| served.name).collect();
    let mut pictures = Vec::new();
    let mut folders = vec![root.join("tests"), root.join("crates"), root.join("src")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("a folder of the tree") {
            let path = entry.expect("an entry").path();
            if path.is_dir() && !path.ends_with("target") {
                folders.push(path);
            } else if path.extension().is_some_and(|kind| kind == "snap") {
                pictures.push(path);
            }
        }
    }
    assert!(pictures.len() > 50, "{} pictures found", pictures.len());

    let mut drawn = Vec::new();
    for picture in &pictures {
        let text = std::fs::read_to_string(picture).expect("a picture");
        for line in text.lines() {
            for name in &names {
                let slashed = format!("{name}/");
                let mut from = 0;
                while let Some(found) = line.get(from..).and_then(|rest| rest.find(&slashed)) {
                    let at = from + found;
                    let before = line.get(..at).unwrap_or_default();
                    let typed = before.ends_with("/model ") || before.ends_with("--model ");
                    let word = before.chars().last().is_some_and(char::is_alphanumeric);
                    if !typed && !word {
                        drawn.push(format!("{}: {}", picture.display(), line.trim()));
                    }
                    from = at + slashed.len();
                }
            }
        }
    }
    assert!(drawn.is_empty(), "{drawn:#?}");
}

/// What `renderer` has said, a row a line.
fn said(renderer: &Renderer<Recording>) -> Vec<String> {
    renderer
        .tail(24)
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

#[test]
fn a_model_taken_off_the_shelf_is_answered_with_the_rung_taken_with_it() {
    // The rung marked under the row is the one the next turn is asked on, so
    // the answer names it rather than the one in force before the Enter.
    let sample = Sample::new("model-answer-rung");
    let terms = keeping(&sample);
    let mut conversation = conversing(Some("anthropic"), "old", Some(99), Some(Effort::High));
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let selected = row("anthropic", "claude-sonnet-5");
    let at = selected
        .model
        .rungs
        .iter()
        .position(|rung| *rung == Effort::Low)
        .expect("a model that serves low");

    applied(selected, Some(at), &mut renderer, &mut conversation, &terms)
        .expect("the row to be taken");

    let said = said(&renderer);
    assert!(
        said.iter()
            .any(|row| row == "anthropic · claude-sonnet-5 · low"),
        "{said:#?}"
    );
    assert_eq!(conversation.runner().effort(), Some(Effort::Low));
}

#[test]
fn a_model_named_with_a_rung_in_force_is_answered_with_that_rung() {
    let sample = Sample::new("model-typed-rung");
    let terms = keeping(&sample);
    let mut conversation = conversing(Some("anthropic"), "old", Some(99), Some(Effort::High));
    let mut renderer = Renderer::new(Recording::new(80, 24));

    taken(
        row("anthropic", "claude-sonnet-5").provider,
        ("claude-sonnet-5", None),
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the name to be taken");

    let said = said(&renderer);
    assert!(
        said.iter()
            .any(|row| row == "anthropic · claude-sonnet-5 · high"),
        "{said:#?}"
    );
}

#[test]
fn the_shelf_stood_while_a_turn_runs_names_the_rung_in_force() {
    // No rung may be taken while the turn runs, but one is still in force,
    // and the row under the box says so: the title says the same.
    let current = Asked {
        provider: Some("anthropic"),
        model: "claude-sonnet-5",
        effort: Some("high"),
        pace: super::super::Pace::default(),
    };

    assert_eq!(
        titled(current, Glyphs::Unicode),
        "now  anthropic · claude-sonnet-5 · high"
    );
    assert_eq!(
        under_the_box(
            "anthropic",
            "claude-sonnet-5",
            Some("high"),
            Glyphs::Unicode
        ),
        "anthropic · claude-sonnet-5 · high"
    );
}

#[test]
fn a_model_with_no_provider_answering_is_named_on_its_own() {
    // A name taken from a file whose vendor has no credential yet: the label
    // says the model and the rung, and no word stands in for the vendor.
    let current = Asked {
        provider: None,
        model: "claude-sonnet-5",
        effort: Some("high"),
        pace: super::super::Pace::default(),
    };

    assert_eq!(
        titled(current, Glyphs::Unicode),
        "now  claude-sonnet-5 · high"
    );
    assert_eq!(
        in_force(None, "claude-sonnet-5", Some(Effort::High), Glyphs::Unicode),
        "claude-sonnet-5 · high"
    );
    assert_eq!(
        under_the_box("", "claude-sonnet-5", Some("high"), Glyphs::Unicode),
        "claude-sonnet-5 · high"
    );
}

#[test]
fn a_row_says_no_rung_before_it_says_fast_and_fast_for_either_kind_of_form() {
    let cost = crucible_models::Cost {
        price: "2x the price",
        speed: None,
        caveat: None,
    };
    let rungs = [Effort::High];

    assert_eq!(
        super::note(&rungs, crucible_models::FastForm::Field(cost)),
        "fast"
    );
    assert_eq!(
        super::note(&rungs, crucible_models::FastForm::Own(cost)),
        "fast"
    );
    assert_eq!(super::note(&rungs, crucible_models::FastForm::None), "");
    assert_eq!(
        super::note(&[], crucible_models::FastForm::Field(cost)),
        "no rung"
    );
}

#[test]
fn the_shelf_title_says_fast_only_after_an_answer_served_fast() {
    let asked = |served| Asked {
        provider: Some("openai"),
        model: "gpt-6-astra",
        effort: Some("high"),
        pace: super::super::Pace {
            served,
            ..super::super::Pace::default()
        },
    };

    assert_eq!(
        titled(asked(true), Glyphs::Unicode),
        "now  openai · gpt-6-astra · high · fast"
    );
    assert_eq!(
        titled(asked(false), Glyphs::Unicode),
        "now  openai · gpt-6-astra · high"
    );
}

#[test]
fn a_row_has_the_fast_form_of_the_route_its_provider_is_served_on() {
    // A sign-in serves fast on fewer models than a key, and a configured
    // address serves none: the note says what taking the row would ask.
    let catalogue = catalogue();
    let openai = offered(&catalogue)
        .find(|served| served.name == "openai")
        .expect("openai is offered");

    assert!(super::routed(openai, "gpt-5.5", false, false).switched());
    assert_eq!(
        super::routed(openai, "gpt-5.5", false, true),
        crucible_models::FastForm::None
    );
    assert!(super::routed(openai, "gpt-5.6-sol", false, true).switched());
    assert_eq!(
        super::routed(openai, "gpt-6-astra", true, false),
        crucible_models::FastForm::None
    );
}

#[test]
fn a_row_reads_its_route_off_the_settings_and_the_store_in_force() {
    // What the shelf hands the route: a `baseUrl` from the settings, and a
    // sign-in from the store, each changing what a row says.
    let catalogue = catalogue();
    let openai = offered(&catalogue)
        .find(|served| served.name == "openai")
        .expect("openai is offered");
    let plain = crucible_config::Settings::default();
    let nothing = crucible_auth::StoredCredentials::default();
    assert!(super::row_form(openai, "gpt-5.5", &plain, &nothing).switched());

    let sample = Sample::new("row-route");
    let based =
        sample.user(r#"{"providers": {"openai": {"baseUrl": "https://gateway.example/v1"}}}"#);
    assert_eq!(
        super::row_form(openai, "gpt-5.5", &based, &nothing),
        crucible_models::FastForm::None
    );

    let home = sample.found();
    std::fs::create_dir_all(home.path()).expect("a home");
    std::fs::write(
        home.path().join("auth.json"),
        r#"{"version":2,"keys":{},"subscriptions":{"openai":{"access_token":"fabricated-openai-access","refresh_token":"fabricated-openai-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#,
    )
    .expect("a store");
    let signed = sample.store().read();
    assert_eq!(
        super::row_form(openai, "gpt-5.5", &plain, &signed),
        crucible_models::FastForm::None
    );
    assert!(super::row_form(openai, "gpt-5.6-sol", &plain, &signed).switched());
}
