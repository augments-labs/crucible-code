//! `/cache` in a whole conversation over each provider this build added: it
//! answers with the vendor's own cache record, and with no error.
//!
//! Each provider is the one the registry builds from a key in its variable,
//! over a transport that sends nothing, so the test reaches no vendor.
//!
//! In a window too narrow for its lines, the answer wraps under the mark it
//! is hung from, on either screen.

use crucible_app::providers::{NOTHING_TO_ASK, offered, providers};
use crucible_app::startup::{self, ProviderAuth};
use crucible_app::subscription::Subscriptions;
use crucible_auth::{Renewals, StoredCredentials};
use crucible_config::Settings;
use crucible_models::Provider;
use crucible_provider::HttpTurns;
use crucible_runner::Runner;

use crate::cli::converse::command;

use super::*;

/// `named`'s provider, as the registry builds it from a key in its variable.
fn built(named: &str) -> Box<dyn Provider> {
    let catalogue = providers()
        .expect("the built-in providers register")
        .snapshot();
    let serving = offered(&catalogue)
        .find(|one| one.name == named)
        .expect("a provider this build serves");
    let settings = Settings::default();
    let from = |_: &str| Some("fabricated-cache-test-key".to_owned());
    let stored = StoredCredentials::default();
    let subscriptions = Subscriptions::production(&Renewals::new());
    startup::provider(
        Some(serving),
        NOTHING_TO_ASK,
        ProviderAuth {
            settings: &settings,
            from: &from,
            stored: &stored,
            subscriptions: &subscriptions,
        },
        &HttpTurns::unavailable(),
    )
    .expect("a provider from a key")
}

/// What the terminal holds after `/cache` over `named` asking `model`.
fn inspected(named: &'static str, model: &str) -> String {
    let provider = built(named);
    let conversation =
        Conversation::recording(Arc::new(Session::nowhere()), Some(named), |session| {
            Runner::new(
                provider,
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
        });
    let mut renderer = Renderer::new(Recording::new(200, 24));
    let mut input = Cursor::new(b"/cache\n".to_vec());

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
    .expect("the loop to finish");
    renderer.terminal().written().to_string()
}

/// `/cache` over `named` names the record reviewed at `source`, says nothing
/// is held, and says no line of trouble.
fn answers(named: &'static str, model: &str, source: &str) {
    let written = inspected(named, model);

    assert!(
        written.contains("cache policy: mode="),
        "{named}: {written}"
    );
    assert!(written.contains("declared support:"), "{named}: {written}");
    assert!(
        written.contains(&format!(
            "capability provenance: reviewed 2026-10-01 from {source}"
        )),
        "{named}: {written}"
    );
    assert!(
        written.contains("last attempt: none yet"),
        "{named}: {written}"
    );
    assert!(
        written.contains("persistent resources: none"),
        "{named}: {written}"
    );
    assert!(!written.contains("! cache"), "{named}: {written}");
    assert!(!written.contains("! /cache"), "{named}: {written}");
}

#[test]
fn cache_answers_for_meta() {
    answers(
        "meta",
        "muse-spark-1.3",
        "https://dev.meta.ai/docs/prompt-caching",
    );
}

#[test]
fn cache_answers_for_xai() {
    answers(
        "xai",
        "grok-4.7",
        "https://docs.x.ai/developers/advanced-api-usage/prompt-caching",
    );
}

#[test]
fn cache_answers_for_deepseek() {
    answers(
        "deepseek",
        "deepseek-flash",
        "https://api-docs.deepseek.com/guides/kv_cache",
    );
}

#[test]
fn cache_answers_for_zai() {
    answers(
        "zai",
        "glm-5.3",
        "https://docs.z.ai/guides/capabilities/cache",
    );
}

#[test]
fn cache_answers_for_qwen() {
    answers(
        "qwen",
        "qwen3.8-max",
        "https://www.alibabacloud.com/help/en/model-studio/context-cache",
    );
}

#[test]
fn cache_answers_for_mimo() {
    answers(
        "mimo",
        "mimo-v2.6-pro",
        "https://mimo.mi.com/static/docs/price/pay-as-you-go.md",
    );
}

#[test]
fn cache_answers_for_minimax() {
    answers(
        "minimax",
        "MiniMax-M3",
        "https://platform.minimax.io/docs/api-reference/text-prompt-caching",
    );
}

/// The rows a window forty columns wide drawn in `mode` shows once `/cache` has
/// been sent from the box and answered.
fn answered(mode: ScreenMode) -> Vec<String> {
    answered_in(mode, 40)
}

/// The rows a window `columns` wide drawn in `mode` shows once `/cache` has
/// been sent from the box and answered.
///
/// The line goes in the way the box leaves it, as one responsive prompt row,
/// because that is the row the answer hangs from on a run somebody is typing
/// at. Native mode's rows are read from the last frame it drew, which redraws
/// the whole region: nothing here waits for a key, so nothing has been sealed
/// into the scrollback above it.
fn answered_in(mode: ScreenMode, columns: usize) -> Vec<String> {
    let terms = plain();
    let opening = opening();
    let mut input = std::io::empty();
    let mut held = Held::new(
        terms.plan.clone(),
        terms.sending.get(),
        Answers {
            input: &mut input,
            keys: false,
        },
        &opening,
    );
    let mut conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(Script::new(vec![]), Tools::new(), session)
    });
    let mut renderer = Renderer::drawing(Recording::new(columns, 40), mode);
    let style = terms.style();

    renderer
        .responsive(
            "/cache".len(),
            Box::new(move |columns| {
                crucible_tui::Prompt::committed(
                    "/cache",
                    columns,
                    style.glyphs(),
                    style.palette().bands(),
                )
            }),
        )
        .expect("the prompt row to be committed");
    let wanted = command::wanted(&terms.commands.snapshot(), "/cache").expect("a command");
    command::run(wanted, &mut renderer, &mut conversation, &mut held, &terms)
        .expect("the command to be answered");

    match mode {
        ScreenMode::Fullscreen => renderer.terminal().picture().rows(),
        ScreenMode::Native => last_frame(renderer.terminal().written()),
    }
}

/// The rows the last native frame wrote, top first: what follows the erase
/// that opens it, with every control sequence read past.
fn last_frame(written: &str) -> Vec<String> {
    let drawn = written.rsplit("\x1b[J").next().unwrap_or_default();
    let mut text = String::new();
    let mut left = drawn.chars();
    while let Some(character) = left.next() {
        if character != '\x1b' {
            text.push(character);
            continue;
        }
        if left.next() == Some('[') {
            for byte in left.by_ref() {
                if ('@'..='~').contains(&byte) {
                    break;
                }
            }
        }
    }
    text.split("\r\n")
        .map(|row| row.trim_end().to_owned())
        .collect()
}

/// The rows of `/cache`'s reply among `rows`: those under the row that asked,
/// down to the first blank one.
fn reply_in(mode: ScreenMode, rows: &[String]) -> Vec<&String> {
    let asked = rows
        .iter()
        .position(|row| row.contains("/cache"))
        .unwrap_or_else(|| panic!("{mode:?}: /cache was never shown in {rows:#?}"));
    rows.iter()
        .skip(asked + 1)
        .take_while(|row| !row.is_empty())
        .collect()
}

/// The words of a reply, its rows joined, with the mark its first row is hung
/// from read past. The two columns the rest are hung by are whitespace.
fn words<'a>(reply: &[&'a String], hangs: &str) -> Vec<&'a str> {
    let (first, rest) = reply.split_first().expect("a reply");
    let first = first
        .strip_prefix(&format!("{hangs} "))
        .expect("a reply hung from the mark");
    std::iter::once(first)
        .chain(rest.iter().map(|row| row.as_str()))
        .flat_map(str::split_whitespace)
        .collect()
}

/// Every row of `/cache`'s reply, in a window forty columns wide drawn in
/// `mode`, after the first is hung under the mark.
///
/// Forty columns is narrower than the policy line and the line about the last
/// attempt, so both run over. A row they ran over onto that starts back at the
/// left edge reads as a line of its own under the command, rather than as part
/// of the answer to it.
fn keeps_its_indent(mode: ScreenMode) {
    let hangs = plain().style().glyphs().hangs();
    let rows = answered(mode);
    let reply = reply_in(mode, &rows);

    assert!(
        reply.len() > 4,
        "{mode:?}: four lines are said, so fewer rows means nothing wrapped, in {rows:#?}"
    );
    let (first, rest) = reply.split_first().expect("a reply");
    assert!(
        first.starts_with(&format!("{hangs} cache policy:")),
        "{mode:?}: the reply opened with {first:?}, in {rows:#?}"
    );
    for row in rest {
        assert!(
            row.starts_with("  ") && !row.starts_with("   "),
            "{mode:?}: {row:?} is not hung under the mark, in {rows:#?}"
        );
    }

    // The screen clips a row folded too wide for the mark rather than folding
    // it again, so a narrow reply must say every word a window wide enough for
    // every line says, in order.
    let wide = answered_in(mode, 200);
    assert_eq!(
        words(&reply, hangs),
        words(&reply_in(mode, &wide), hangs),
        "{mode:?}: the reply lost words at forty columns, in {rows:#?}"
    );
}

#[test]
fn cache_keeps_its_indent_where_its_lines_wrap() {
    keeps_its_indent(ScreenMode::Fullscreen);
}

#[test]
fn cache_keeps_its_indent_where_its_lines_wrap_in_native_mode() {
    keeps_its_indent(ScreenMode::Native);
}
