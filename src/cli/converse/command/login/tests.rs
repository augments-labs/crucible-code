//! What a credential given to `/login` leaves the session asking.

use std::cell::Cell;
use std::sync::Arc;

use crucible_auth::Store;
use crucible_builtins::{Ledger, Plan};
use crucible_core::{AgentId, Cancel, Revealed};
use crucible_runner::{Agent, Model, Runner, Tools};
use crucible_tui::Recording;

use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::*;

/// Terms whose session is already answered by `anthropic`, and whose
/// serving closure resolves any credential without reaching a network.
fn in_force(sample: &Sample) -> Terms {
    Terms {
        style: Cell::new(Style::plain()),
        chosen: Cell::new(None),
        reading: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        steer: crucible_core::Steer::new(),
        aside: crucible_core::Aside::new(),
        ledger: Ledger::new(),
        revealed: Revealed::new(),
        plan: Plan::new(),
        putting: crate::cli::seen::Putting::new(),
        client: crate::cli::client::Client::new(),
        leaving: crucible_builtins::Background::new(),
        pending_model: Cell::new(None),
        pending_mode: Cell::new(None),
        settings: crucible_config::Settings::default(),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        subscriptions: crucible_app::subscription::Subscriptions::production(),
        serving: Box::new(|named, _| {
            Ok(crucible_app::providers::Resolved {
                provider: Box::new(crucible_provider::Unavailable::new(
                    crucible_app::providers::NOTHING_TO_ASK,
                )),
                source: crucible_app::providers::CredentialSource::Environment(named.key.into()),
            })
        }),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        sending: crucible_tui::Sending::default(),
        commands: crate::cli::converse::command::builtins(&std::sync::Arc::default())
            .expect("the built-in commands register"),
        providers: crucible_app::providers::providers().expect("the built-in providers register"),
    }
}

/// A conversation `anthropic` already serves, asking `model` and answering
/// from an empty script.
fn asking(model: &str) -> Conversation {
    crate::cli::converse::tests::paired(Arc::new(crucible_session::Session::nowhere()), |session| {
        Runner::new(
            Box::new(Script::new(Vec::new())),
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

#[test]
fn a_credential_stored_while_a_provider_is_in_force_leaves_the_session_as_it_is() {
    // A credential says a provider can be reached and never which to ask.
    // The session in front of the reader is already answering, and a login
    // that pulled its provider and model out from under it would turn
    // "store a second key" into "lose the conversation's setup".
    let sample = Sample::new("login-in-force");
    let terms = in_force(&sample);
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "openai")
        .expect("a provider this build has an arm for");
    taken(named, &mut renderer, &mut conversation, &terms).expect("the terminal to be written");

    assert_eq!(conversation.runner().model(), "claude-test-1");
    assert_eq!(conversation.serving(), Some("anthropic"));

    let written = renderer.terminal().written().to_string();
    assert!(
        written.contains("/model"),
        "the way to switch is named: {written}"
    );
    assert!(
        !sample.root().join("unwritten-home.json").exists(),
        "which provider to open on is still the reader's standing choice"
    );
}

#[test]
fn a_store_that_cannot_be_written_is_said_without_its_path() {
    // The store's own error names the file and quotes the operating
    // system, which is right for a log and wrong for a row under a
    // command: the row says what stopped and the way back in, and the
    // reader's home stays off the screen.
    let sample = Sample::new("login-store-failed");
    let terms = in_force(&sample);
    std::fs::create_dir_all(sample.root().join("auth.json"))
        .expect("a directory where the store's file goes");
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");
    written(
        named,
        "sk-ant-not-a-key",
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the terminal to be written");

    // Folded at the window's width, so read back as rows joined by the
    // space a fold stands in for.
    let written = renderer.terminal().picture().said().join(" ");
    assert!(
            written.contains(
                "! the key could not be saved — crucible cannot write its login store; try /login again after fixing the permissions"
            ),
            "{written}"
        );
    assert!(!written.contains("auth.json"), "{written}");
    assert!(!written.contains("directory"), "{written}");
    assert!(
        !written.contains(&sample.root().display().to_string()),
        "{written}"
    );
    assert!(!written.contains("sk-ant"), "{written}");
}

#[test]
fn a_store_that_cannot_be_read_is_answered_with_moving_it_aside() {
    // The permissions are not what stopped this one: the store is there
    // and writable, and cannot be parsed. Telling the reader to fix the
    // permissions would send them to check a thing that is fine, and the
    // way back in is the one the store's own error gives — move it aside.
    let sample = Sample::new("login-store-unreadable");
    let terms = in_force(&sample);
    std::fs::write(sample.root().join("auth.json"), "not json {")
        .expect("a store that will not parse");
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");
    written(
        named,
        "sk-ant-not-a-key",
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the terminal to be written");

    let written = renderer.terminal().picture().said().join(" ");
    assert!(
            written.contains(
                "! the key could not be saved — crucible cannot read its login store; move it aside and try /login again"
            ),
            "{written}"
        );
    assert!(!written.contains("permissions"), "{written}");
    assert!(!written.contains("auth.json"), "{written}");
    assert!(!written.contains("sk-ant"), "{written}");
}

#[test]
fn a_store_past_its_byte_ceiling_is_answered_the_way_an_unreadable_one_is() {
    // Too large to parse is not read either, and the store refuses to
    // write over what it could not read. The reader is told the same
    // thing as for a store that will not parse: move it aside.
    let sample = Sample::new("login-store-too-large");
    let terms = in_force(&sample);
    std::fs::write(sample.root().join("auth.json"), "x".repeat(64 * 1024 + 1))
        .expect("a store past the ceiling");
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");
    written(
        named,
        "sk-ant-not-a-key",
        &mut renderer,
        &mut conversation,
        &terms,
    )
    .expect("the terminal to be written");

    let written = renderer.terminal().picture().said().join(" ");
    assert!(
            written.contains(
                "! the key could not be saved — crucible cannot read its login store; move it aside and try /login again"
            ),
            "{written}"
        );
    assert!(!written.contains("byte"), "{written}");
    assert!(!written.contains("auth.json"), "{written}");
    assert!(!written.contains("sk-ant"), "{written}");
}

#[test]
fn the_way_back_in_is_chosen_by_what_stopped_the_store() {
    // The one the store cannot be made to produce cheaply from here is a
    // lock held past its five seconds; the other three are here beside it
    // so the whole table is read in one place. Each arrives with a path,
    // and no sentence carries one.
    let path = std::path::PathBuf::from("/somebody/.crucible/auth.json");
    let busy = AuthError::Busy { path: path.clone() };
    let too_large = AuthError::TooLarge {
        path: path.clone(),
        maximum: 1,
    };
    let unreadable = AuthError::Unreadable { path: path.clone() };
    let unwritable = AuthError::Unwritable {
        path,
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    };

    assert_eq!(
        remedy(&busy),
        "another crucible is writing its login store; try /login again in a moment"
    );
    assert_eq!(remedy(&too_large), remedy(&unreadable));
    assert_eq!(
        remedy(&unreadable),
        "crucible cannot read its login store; move it aside and try /login again"
    );
    assert_eq!(remedy(&unwritable), STORE_UNWRITABLE);
    for failed in [&busy, &too_large, &unreadable, &unwritable] {
        assert!(!remedy(failed).contains("somebody"), "{}", remedy(failed));
    }
}

#[test]
fn a_window_too_short_for_the_key_box_says_so_rather_than_cancelled() {
    // Two rows: not even the box's own three. Nothing was asked and
    // nothing could have been typed, so "cancelled" would report a choice
    // the reader never made — and hide the one thing that would let them
    // in, which is a taller window.
    let sample = Sample::new("login-cramped");
    let terms = in_force(&sample);
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 2));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");
    given(named, &mut renderer, &mut conversation, &terms).expect("the terminal to be written");

    let written = renderer.terminal().picture().said().join(" ");
    assert!(
        written.contains(
            "the window has no room for the key box; make it taller and try /login again"
        ),
        "{written}"
    );
    assert!(!written.contains("cancelled"), "{written}");
}

#[test]
fn a_credential_stored_for_the_provider_in_force_keeps_its_model() {
    // Re-entering a key for the provider already answering is a renewal,
    // not a switch: the new credential signs the next request, and the
    // model in force goes on being the one asked.
    let sample = Sample::new("login-renewed");
    let terms = in_force(&sample);
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));

    let named = offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");
    taken(named, &mut renderer, &mut conversation, &terms).expect("the terminal to be written");

    assert_eq!(conversation.runner().model(), "claude-test-1");
    let written = renderer.terminal().written().to_string();
    assert!(written.contains("asking claude-test-1"), "{written}");
}

#[test]
fn browser_login_shows_the_short_page_and_masks_manual_input() {
    let mut view = LoginView::new(Glyphs::Unicode);
    view.page = Some(("http://localhost:1455/launch".into(), None));
    view.status = Cow::Borrowed("a browser should open; waiting for authorization…");
    view.accepts_manual = true;
    view.manual = "secret-callback".to_owned();
    let (rows, caret) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
    let text: Vec<_> = rows.iter().map(Row::text).collect();

    assert_eq!(text.first().map(String::as_str), Some("Log in to ChatGPT"));
    assert!(text.iter().any(|row| row.contains("localhost:1455/launch")));
    let input = text.iter().find(|row| row.starts_with("› ")).unwrap();
    assert_eq!(input.chars().skip(2).count(), "secret-callback".len());
    assert!(input.chars().skip(2).all(|character| character == '•'));
    assert!(!text.iter().any(|row| row.contains("secret-callback")));
    assert!(!text.iter().any(|row| row.contains("oauth/authorize")));
    assert_eq!(caret.row, rows.len() - 2);
}

#[test]
fn the_callback_box_takes_a_pasted_code_the_way_it_takes_typed_characters() {
    // A callback URL is copied out of a browser, and arrives with the
    // newline the address bar hands over with it. What is held is the code
    // alone, one mark per character, exactly as if it had been typed.
    let mut view = LoginView::new(Glyphs::Unicode);
    view.page = Some(("http://localhost:1455/launch".into(), None));
    view.accepts_manual = true;

    let redraw = view.pasted("  http://localhost:1455/callback?code=abc\n");

    assert!(redraw);
    assert_eq!(view.manual, "http://localhost:1455/callback?code=abc");
    assert!(!view.limited);

    let (rows, _) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
    let text: Vec<_> = rows.iter().map(Row::text).collect();
    let input = text.iter().find(|row| row.starts_with("› ")).unwrap();
    assert_eq!(input.chars().skip(2).count(), view.manual.chars().count());
    assert!(!text.iter().any(|row| row.contains("code=abc")));
}

#[test]
fn a_paste_past_the_callback_box_ceiling_is_refused_whole_and_the_row_beneath_says_so() {
    // Unlike the key box, this one has a status row beneath it, and the
    // row says why nothing grew. What was held before the paste is held
    // after it, untouched.
    let mut view = LoginView::new(Glyphs::Unicode);
    view.page = Some(("http://localhost:1455/launch".into(), None));
    view.accepts_manual = true;
    view.manual = "x".repeat(MAX_MANUAL - 1);

    let redraw = view.pasted("ab");

    assert!(redraw);
    assert!(view.limited);
    assert_eq!(view.manual.len(), MAX_MANUAL - 1);
    let (rows, _) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
    assert!(
        rows.iter()
            .map(Row::text)
            .any(|row| row.contains("limited")),
        "the row beneath says why"
    );

    assert!(view.pasted("a"), "one byte still fits");
    assert!(!view.limited);
    assert_eq!(view.manual.len(), MAX_MANUAL);
}

#[test]
fn the_paste_box_draws_its_mark_and_its_dots_out_of_the_glyph_set() {
    // A second box a line is typed into, so it takes the same mark the
    // prompt takes and hides what is typed with the same one the key box
    // hides a key with. A terminal whose font has neither would otherwise
    // get hollow squares on the row where the sign-in is asking for the one
    // thing it will not show back.
    for (glyphs, mark, hidden) in [(Glyphs::Unicode, "› ", '•'), (Glyphs::Ascii, "> ", '*')] {
        let mut view = LoginView::new(glyphs);
        view.page = Some(("http://localhost:1455/launch".into(), None));
        view.accepts_manual = true;
        view.manual = "pasted".to_owned();

        let (rows, caret) = view.frame(80, "Log in to ChatGPT", glyphs);
        let text: Vec<_> = rows.iter().map(Row::text).collect();
        let input = text
            .iter()
            .find(|row| row.starts_with(mark))
            .unwrap_or_else(|| panic!("{glyphs:?}: {text:?}"));

        assert!(
            input.chars().skip(2).all(|character| character == hidden),
            "{glyphs:?}: {input}"
        );
        assert_eq!(caret.column, 2 + "pasted".len(), "{glyphs:?}");
    }
}

#[test]
fn every_way_is_named_by_what_the_reader_holds_and_how_it_is_billed() {
    // The row under an account names the plan and whose it is; the row
    // under the key names how a key is billed. Neither is a sentence about
    // what pressing Enter does — the reader is choosing between things they
    // have, and a future account row inherits the same shape.
    let sample = Sample::new("login-ways");
    let terms = in_force(&sample);
    let ways = ways(&terms);

    let shown: Vec<&str> = ways.iter().map(|way| way.shown).collect();
    let says: Vec<&str> = ways.iter().map(|way| way.says.as_str()).collect();

    assert_eq!(shown, ["OpenAI", "MoonshotAI", KEY_ROUTE_SHOWN]);
    assert_eq!(
        says,
        [
            "ChatGPT plan with your subscription",
            "Kimi Code plan with your subscription",
            "API usage billing",
        ]
    );
}

#[test]
fn a_provider_row_says_which_variable_to_set() {
    // The one thing that differs between provider rows, and the whole of
    // what somebody who would rather not type a key needs to read.
    let terms = in_force(&Sample::new("login-variables"));
    let providers = terms.providers.snapshot();
    let anthropic = offered(&providers)
        .find(|served| served.name == "anthropic")
        .expect("a provider this build has an arm for");

    assert_eq!(variable_row(&anthropic), "set ANTHROPIC_API_KEY");
}

#[test]
fn the_row_saying_the_sign_in_is_waiting_takes_its_mark_from_the_set() {
    // The row under a sign-in that has not finished is a state and the key
    // that leaves it, parted by the same mark. It is the row somebody looks
    // at while nothing is happening, so it is the one that would sit there
    // with a hollow square in it the longest.
    for (glyphs, said) in [
        (Glyphs::Unicode, "waiting — esc to cancel"),
        (Glyphs::Ascii, "waiting -- esc to cancel"),
    ] {
        let view = LoginView::new(glyphs);
        let (rows, _) = view.frame(80, "Log in to ChatGPT", glyphs);
        let text: Vec<String> = rows.iter().map(Row::text).collect();

        assert!(text.iter().any(|row| row == said), "{glyphs:?}: {text:?}");
    }
}

#[test]
fn every_login_row_and_its_caret_fit_a_narrow_terminal() {
    let mut view = LoginView::new(Glyphs::Unicode);
    view.page = Some((
        "http://localhost:1455/launch".into(),
        Some("ABCD-EFGH".into()),
    ));
    view.status = Cow::Borrowed("waiting");
    view.browser_failed = true;
    view.accepts_manual = true;
    view.manual = "pasted-code".to_owned();
    view.limited = true;
    for columns in 0..=32 {
        let (rows, caret) = view.frame(columns, "Log in to ChatGPT", Glyphs::Unicode);
        assert!(rows.iter().all(|row| row.columns() <= columns));
        assert!(caret.column <= columns.saturating_sub(1));
        assert!(caret.row < rows.len());
    }
}
