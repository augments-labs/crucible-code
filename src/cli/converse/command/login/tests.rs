//! What a credential given to `/login` leaves the session asking.

use std::cell::Cell;
use std::sync::Arc;

use crucible_auth::Store;
use crucible_builtins::{Ledger, Plan};
use crucible_runner::{Agent, Model, Runner, Tools};
use crucible_runtime::Cancel;
use crucible_tools::Revealed;
use crucible_tui::Recording;
use crucible_types::AgentId;

use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::*;

/// Terms whose session is already answered by `anthropic`, and whose
/// serving closure resolves any credential without reaching a network.
fn in_force(sample: &Sample) -> Terms {
    Terms {
        consent: crucible_app::content_use::Consent::new(
            crucible_app::content_use::Routes::production(),
        ),
        style: Cell::new(Style::plain()),
        chosen: Cell::new(None),
        reading: std::cell::RefCell::default(),
        cancel: Cancel::new(),
        runtime: crate::cli::fake::runtime(),
        ending: crate::cli::ending::Ending::deaf(),
        steer: crucible_runtime::Steer::new(),
        aside: crucible_runtime::Aside::new(),
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
        subscriptions: crucible_app::subscription::Subscriptions::production(
            &crucible_auth::Renewals::new(),
        ),
        serving: Box::new(|named, _| {
            Ok(crucible_app::providers::Resolved {
                provider: Box::new(crucible_provider::Unavailable::new(
                    crucible_app::providers::NOTHING_TO_ASK,
                )),
                source: crucible_app::providers::CredentialSource::Environment(named.key.into()),
            })
        }),
        environment: Box::new(|_| None),
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
    // Two ways to pay, whatever the store holds: an account whose plan
    // includes the usage, or a key billed by what is sent. Neither row is a
    // sentence about what pressing Enter does, and neither names a vendor:
    // the vendors are the lists each leads to.
    let shown: Vec<&str> = FIRST.iter().map(|way| way.name).collect();
    let says: Vec<&str> = FIRST.iter().map(|way| way.says).collect();

    assert_eq!(shown, ["Your account with subscription", KEY_ROUTE_SHOWN]);
    assert_eq!(
        says,
        ["Usage included in your paid plan", "API usage billing"]
    );
    assert_eq!(HOW, "Choose how usage is paid for.");
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

/// The rows this build ships.
fn production() -> Rows {
    Rows::production()
}

/// The row of `production` named `shown` in `list`.
fn way(rows: &Rows, list: List, shown: &str) -> Way {
    rows.listed(list)
        .find(|way| way.shown == shown)
        .cloned()
        .unwrap_or_else(|| panic!("a row named {shown}"))
}

/// The names of the rows `words` leave.
fn left(words: &str, rows: &Rows) -> Vec<String> {
    let words: Vec<&str> = words.split_whitespace().collect();
    matching(&words, rows)
        .iter()
        .map(|way| format!("{} {}", way.shown, credential(way)))
        .collect()
}

#[test]
fn words_after_login_narrow_the_rows_to_those_every_word_matches() {
    let rows = production();

    assert_eq!(left("openai", &rows), ["OpenAI sign-in", "OpenAI API key"]);
    for words in ["kimi", "moonshot", "KIMI", "Moonshot"] {
        assert_eq!(
            left(words, &rows),
            [
                "Kimi Code · kimi.ai sign-in",
                "Kimi Code · kimi.com sign-in",
                "MoonshotAI · kimi.ai API key",
                "MoonshotAI · kimi.com API key",
            ],
            "{words}"
        );
    }
    assert_eq!(
        left("kimi code", &rows),
        [
            "Kimi Code · kimi.ai sign-in",
            "Kimi Code · kimi.com sign-in"
        ]
    );
    assert_eq!(
        left("kimi.ai", &rows),
        [
            "Kimi Code · kimi.ai sign-in",
            "MoonshotAI · kimi.ai API key"
        ]
    );
    assert_eq!(
        left("kimi code kimi.ai", &rows),
        ["Kimi Code · kimi.ai sign-in"]
    );
    assert_eq!(left("openai subscription", &rows), ["OpenAI sign-in"]);
    assert_eq!(left("openai key", &rows), ["OpenAI API key"]);
    assert_eq!(left("anthropic", &rows), ["Anthropic API key"]);
    assert_eq!(left("google", &rows), ["Google API key"]);
    assert!(left("nope", &rows).is_empty());
    assert_eq!(left("", &rows).len(), rows.all().len());
}

#[test]
fn every_row_is_left_alone_by_the_words_it_is_typed_with() {
    let rows = production();
    for way in rows.all() {
        let words = reaching(way, &rows);
        let split: Vec<&str> = words.split_whitespace().collect();

        assert_eq!(matching(&split, &rows), [way], "{words}");
    }
}

/// A registry holding one row this build has never shipped of each kind.
fn fabricated() -> Rows {
    let mut rows = production().all().to_vec();
    rows.insert(
        0,
        Way {
            list: List::Subscription,
            shown: "Fabricated Plan",
            provider: "fabricated",
            site: None,
            says: Some("Fabricated plan usage"),
            kind: Kind::Account,
            mark: None,
            stored: "fabricated",
            environment: false,
            address: None,
        },
    );
    rows.push(Way {
        list: List::Key,
        shown: "Fabricated",
        provider: "fabricated",
        site: None,
        says: Some("a fabricated console key"),
        kind: Kind::Key,
        mark: None,
        stored: "fabricated",
        environment: false,
        address: None,
    });
    Rows::new(rows)
}

#[test]
fn a_row_added_to_the_registry_is_listed_with_no_edit_here() {
    let rows = fabricated();
    let providers = crucible_app::providers::providers()
        .expect("the built-in providers register")
        .snapshot();

    for (list, shown, says) in [
        (
            List::Subscription,
            "Fabricated Plan",
            "Fabricated plan usage",
        ),
        (List::Key, "Fabricated", "a fabricated console key"),
    ] {
        let listed: Vec<(&str, String)> = listed(&rows, list)
            .into_iter()
            .map(|way| {
                let says = described(way, &[], &providers, Glyphs::Unicode);
                (way.shown, says)
            })
            .collect();
        assert!(listed.contains(&(shown, says.to_owned())), "{listed:?}");

        let way = way(&rows, list, shown);
        let line = line(&way, &rows, &providers, Glyphs::Unicode);
        assert!(line.starts_with("/login fabricated "), "{line}");
    }
}

/// Terms over `sample`'s home, whose store names what the rows do.
fn named(sample: &Sample) -> Terms {
    let mut terms = in_force(sample);
    terms.logins = Store::in_home(&sample.root()).naming(production().names());
    terms
}

/// A store holding a kimi.ai key and an OpenAI sign-in, and nothing else.
const HELD: &str = r#"{"version":2,"keys":{"moonshot@kimi.ai":"fabricated-kimi-ai-key"},"subscriptions":{"openai":{"access_token":"fabricated-openai-access","refresh_token":"fabricated-openai-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;

#[test]
fn a_row_holding_its_providers_credential_says_so_and_no_secret_reaches_the_screen() {
    let sample = Sample::new("login-signed-in");
    std::fs::write(sample.root().join("auth.json"), HELD).expect("a store");
    let terms = named(&sample);
    let rows = production();
    let providers = terms.providers.snapshot();

    let held = holding(&rows, &terms).expect("a store that reads whole");
    let said: Vec<(&str, String)> = rows
        .all()
        .iter()
        .map(|way| {
            (
                way.shown,
                described(way, &held, &providers, Glyphs::Unicode),
            )
        })
        .collect();

    assert_eq!(
        said,
        [
            (
                "OpenAI",
                "signed in · ChatGPT plan usage with Plus, Pro, Business and Enterprise".to_owned()
            ),
            (
                "Kimi Code · kimi.ai",
                "Kimi Code plan usage, accounts outside mainland China".to_owned()
            ),
            (
                "Kimi Code · kimi.com",
                "Kimi Code plan usage, mainland China accounts".to_owned()
            ),
            ("Anthropic", "set ANTHROPIC_API_KEY".to_owned()),
            ("Google", "set GEMINI_API_KEY".to_owned()),
            (
                "MoonshotAI · kimi.ai",
                "signed in with a stored key".to_owned()
            ),
            ("MoonshotAI · kimi.com", "set MOONSHOT_API_KEY".to_owned()),
            ("OpenAI", "set OPENAI_API_KEY".to_owned()),
        ]
    );
    let everything = format!("{said:?} {held:?}");
    assert!(!everything.contains("fabricated"), "{everything}");
}

#[test]
fn signed_in_is_kept_at_forty_columns_where_the_plan_words_are_cut() {
    let sample = Sample::new("login-signed-in-narrow");
    std::fs::write(sample.root().join("auth.json"), HELD).expect("a store");
    let terms = named(&sample);
    let rows = production();
    let providers = terms.providers.snapshot();
    let held = holding(&rows, &terms).expect("a store that reads whole");

    let says: Vec<String> = rows
        .listed(List::Subscription)
        .map(|way| described(way, &held, &providers, Glyphs::Unicode))
        .collect();
    let shown: Vec<Offered<'_>> = rows
        .listed(List::Subscription)
        .zip(&says)
        .map(|(way, says)| Offered {
            name: way.shown,
            says,
        })
        .collect();
    let panel = Panel {
        source: None,
        title: TITLE,
        said: Some(ACCOUNTS),
        shown: &shown,
        chosen: 0,
        footer: BACK,
    };
    let drawn: Vec<String> = panel
        .rows(40, Glyphs::Unicode)
        .iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect();

    assert!(
        drawn.contains(&"  signed in · ChatGPT plan usage with P…".to_owned()),
        "{drawn:?}"
    );
}

#[test]
fn an_unreadable_store_is_said_before_any_row_is_drawn() {
    for (case, text) in [
        ("login-open-unreadable", "not json {".to_owned()),
        ("login-open-too-large", "x".repeat(64 * 1024 + 1)),
    ] {
        let sample = Sample::new(case);
        std::fs::write(sample.root().join("auth.json"), text).expect("a store");
        let terms = named(&sample);
        let mut conversation = asking("claude-test-1");
        let mut renderer = Renderer::new(Recording::new(80, 24));

        run("", &mut renderer, &mut conversation, &terms, true)
            .expect("the terminal to be written");

        let written = renderer.terminal().picture().said().join(" ");
        assert!(
            written.contains(
                "! crucible cannot read its login store; move it aside and try /login again"
            ),
            "{written}"
        );
        assert!(!written.contains(TITLE), "{written}");
    }
}

#[test]
fn what_a_choice_replaces_is_named_by_the_row_it_was_given_on() {
    let rows = production();
    let held = |list, shown| vec![way(&rows, list, shown)];

    let openai_key = way(&rows, List::Key, "OpenAI");
    let openai_plan = way(&rows, List::Subscription, "OpenAI");
    let kimi_ai = way(&rows, List::Subscription, "Kimi Code · kimi.ai");
    let anthropic = way(&rows, List::Key, "Anthropic");

    assert_eq!(
        replaced(
            &openai_key,
            &held(List::Subscription, "OpenAI"),
            Glyphs::Unicode
        )
        .as_deref(),
        Some("the sign-in held for OpenAI")
    );
    assert_eq!(
        replaced(&openai_plan, &held(List::Key, "OpenAI"), Glyphs::Unicode).as_deref(),
        Some("the API key held for OpenAI")
    );
    assert_eq!(
        replaced(
            &kimi_ai,
            &held(List::Subscription, "Kimi Code · kimi.com"),
            Glyphs::Unicode
        )
        .as_deref(),
        Some("the sign-in held for Kimi Code · kimi.com")
    );
    assert_eq!(
        replaced(&anthropic, &held(List::Key, "Anthropic"), Glyphs::Unicode).as_deref(),
        Some("the API key held for Anthropic")
    );
    assert_eq!(
        replaced(&anthropic, &held(List::Key, "OpenAI"), Glyphs::Unicode),
        None
    );

    assert_eq!(
        unchanged(&openai_plan, &held(List::Key, "OpenAI"), Glyphs::Unicode),
        "! sign-in did not complete; the API key stored for OpenAI is unchanged"
    );
    assert_eq!(
        unchanged(&kimi_ai, &[], Glyphs::Unicode),
        "! sign-in did not complete; nothing was stored"
    );
}

#[test]
fn a_sign_in_chosen_from_a_list_says_what_it_replaces_and_that_escape_goes_back() {
    let view = LoginView::new(Glyphs::Unicode).opened(
        BACK,
        Some("the API key held for OpenAI".to_owned()),
        Glyphs::Unicode,
    );
    let (rows, _) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
    let text: Vec<String> = rows.iter().map(Row::text).collect();

    assert_eq!(
        text.get(1).map(String::as_str),
        Some("Signing in replaces the API key held for OpenAI once it completes."),
        "{text:?}"
    );
    assert!(
        text.iter().any(|row| row == "waiting — esc to go back"),
        "{text:?}"
    );
}

#[test]
fn the_key_route_is_worded_the_same_on_the_first_panel_and_the_key_box() {
    assert_eq!(
        KEY_ROUTE_SHOWN.to_lowercase(),
        crucible_tui::KEY_ROUTE.to_lowercase()
    );
}

/// What `/login` with nothing after it writes into a window `columns` by
/// `height`, with a keyboard or without one.
fn opened_into(columns: usize, height: usize, keys: bool) -> String {
    let sample = Sample::new("login-no-room");
    let terms = named(&sample);
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(columns, height));

    run("", &mut renderer, &mut conversation, &terms, keys).expect("the terminal to be written");

    renderer.terminal().written().to_string()
}

#[test]
fn a_window_with_no_room_for_a_panel_is_given_every_row_as_the_line_to_type() {
    // A run with no keyboard stands no panel: it is given one line per row,
    // `/login` and the words that leave that row alone, whole at forty
    // columns too.
    let rows = production();
    for columns in [80, 40] {
        let written = opened_into(columns, 40, false);
        for way in rows.all() {
            let typed = format!("/login {} —", reaching(way, &rows));
            assert!(written.contains(&typed), "{columns}: {typed}: {written}");
        }
        assert_eq!(
            written.matches("/login ").count(),
            rows.all().len(),
            "{written}"
        );
    }

    // Three rows hold no panel either, keyboard or not: the same lines come
    // out, the last of them on the screen.
    for columns in [80, 40] {
        let written = opened_into(columns, 3, true);
        assert!(
            written.contains("/login openai key —"),
            "{columns}: {written}"
        );
        assert!(!written.contains(TITLE), "{columns}: {written}");
    }
}

#[test]
fn row_names_are_drawn_with_the_glyph_sets_own_dot() {
    // A terminal set to ASCII is sent no middle dot: not in a row's name, not
    // in the sentence naming what a choice replaces.
    let rows = production();
    let providers = crucible_app::providers::providers()
        .expect("the built-in providers register")
        .snapshot();
    let kimi_com = way(&rows, List::Subscription, "Kimi Code · kimi.com");
    let kimi_ai = way(&rows, List::Subscription, "Kimi Code · kimi.ai");

    let listed: Vec<&Way> = rows.all().iter().collect();
    let ascii = entries(
        &listed,
        std::slice::from_ref(&kimi_com),
        &providers,
        Glyphs::Ascii,
    );
    for (name, says) in &ascii {
        assert!(name.is_ascii(), "{name}");
        assert!(says.is_ascii(), "{says}");
    }
    assert!(
        ascii.iter().any(|(name, _)| name == "Kimi Code - kimi.ai"),
        "{ascii:?}"
    );
    assert_eq!(
        replaced(&kimi_ai, std::slice::from_ref(&kimi_com), Glyphs::Ascii).as_deref(),
        Some("the sign-in held for Kimi Code - kimi.com")
    );
    assert!(unchanged(&kimi_ai, &[kimi_com], Glyphs::Ascii).is_ascii());

    let unicode = entries(&listed, &[], &providers, Glyphs::Unicode);
    assert!(
        unicode
            .iter()
            .any(|(name, _)| name == "Kimi Code · kimi.ai"),
        "{unicode:?}"
    );
}

#[test]
fn rows_narrowed_by_words_stand_under_the_heading_of_their_own_kind() {
    // A registry that lists a sign-in row after key rows, as one a later
    // release adds a row to may: each still stands under its own kind.
    let mut listed = production().all().to_vec();
    let late = listed.remove(0);
    listed.push(late);
    let rows = Rows::new(listed);
    let matched = matching(&["openai"], &rows);

    let (ordered, headings) = kinds(&matched);

    let names: Vec<(&str, List)> = ordered.iter().map(|way| (way.shown, way.list)).collect();
    assert_eq!(
        names,
        [("OpenAI", List::Subscription), ("OpenAI", List::Key)]
    );
    let at: Vec<(usize, &str)> = headings.iter().map(|one| (one.before, one.name)).collect();
    assert_eq!(at, [(0, "Subscription"), (1, "API key")]);
}

/// A sign-in that is refused the moment it starts: the one way a test can
/// see what the sign-in view says when a flow does not complete.
struct Refused {
    slot: crucible_auth::LoginSlot,
}

impl std::fmt::Debug for Refused {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Refused")
    }
}

impl crucible_auth::SubscriptionLogin for Refused {
    fn provider(&self) -> &'static str {
        "openai"
    }

    fn start(
        &self,
        _method: crucible_auth::LoginMethod,
        _store: Store,
    ) -> Result<crucible_auth::LoginAttempt, crucible_auth::OAuthError> {
        self.slot
            .start(&crate::cli::fake::runtime(), |updates| async move {
                let _ = updates.send(Err(crucible_auth::OAuthError::Denied));
            })
    }

    fn credential(
        &self,
        _stored: &crucible_auth::StoredCredentials,
    ) -> Option<Box<dyn crucible_credentials::Credential>> {
        None
    }
}

/// The one route to [`Refused`].
const REFUSED: Route = Route::new(
    "openai",
    crucible_auth::LoginMethod::new("refused"),
    "Log in to ChatGPT",
    "Refused",
    "refused at once",
);

#[test]
fn a_refused_sign_in_names_what_it_would_have_replaced_and_says_it_is_unchanged() {
    let sample = Sample::new("login-refused");
    let mut terms = named(&sample);
    terms.subscriptions = crucible_app::subscription::Subscriptions::new(
        vec![(
            Arc::new(Refused {
                slot: crucible_auth::LoginSlot::new(),
            }),
            crucible_provider::OpenAi::SUBSCRIPTION,
        )],
        vec![REFUSED],
    );
    let rows = production();
    let held = [way(&rows, List::Key, "OpenAI")];
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let plan = way(&rows, List::Subscription, "OpenAI");
    let mut walk = Walk {
        renderer: &mut renderer,
        conversation: &mut conversation,
        terms: &terms,
        held: &held,
    };

    let closed = subscribed(REFUSED, &plan, Opened::Directly, &mut walk).expect("the terminal");

    assert_eq!(closed, Closed::Done);
    let written = renderer.terminal().written().to_string();
    assert!(
        written.contains("Signing in replaces the API key held for OpenAI once it completes."),
        "{written}"
    );
    assert!(
        written.contains("! account login was not authorized"),
        "{written}"
    );
    assert!(
        written.contains("! sign-in did not complete; the API key stored for OpenAI is unchanged"),
        "{written}"
    );
}

#[test]
fn a_sign_in_row_nothing_is_registered_for_says_so_rather_than_standing_an_empty_panel() {
    let sample = Sample::new("login-unregistered");
    let mut terms = named(&sample);
    terms.subscriptions = crucible_app::subscription::Subscriptions::new(Vec::new(), Vec::new());
    // The row's route said yes to, so what stands is the screen this is about
    // rather than the question before it.
    terms.consent.record("subscription:moonshot@kimi.ai");
    let rows = production();
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let plan = way(&rows, List::Subscription, "Kimi Code · kimi.ai");
    let mut walk = Walk {
        renderer: &mut renderer,
        conversation: &mut conversation,
        terms: &terms,
        held: &[],
    };

    let closed = screen(&plan, Opened::Directly, &mut walk).expect("the terminal");

    assert_eq!(closed, Closed::Done);
    let said = renderer.terminal().picture().said().join(" ");
    assert!(
        said.contains("! no subscription login for Kimi Code · kimi.ai"),
        "{said}"
    );
}

#[test]
fn a_key_only_the_environment_holds_marks_no_row() {
    // The marks are read from the store and nothing else: a key exported in
    // the shell is not one this command stored or can replace.
    let sample = Sample::new("login-environment-only");
    let mut terms = named(&sample);
    terms.environment = Box::new(|_| Some("fabricated-exported-key".to_owned()));
    let rows = production();
    let providers = terms.providers.snapshot();

    let held = holding(&rows, &terms).expect("an empty store");
    let listed: Vec<&Way> = rows.all().iter().collect();
    let drawn = entries(&listed, &held, &providers, Glyphs::Unicode);

    assert!(held.is_empty(), "{held:?}");
    assert!(
        drawn.iter().all(|(_, says)| !says.starts_with("signed in")),
        "{drawn:?}"
    );
}

#[test]
fn what_is_said_after_a_stop_is_what_the_store_holds() {
    use crucible_auth::Stopped;

    for opened in [Opened::Below, Opened::Directly] {
        assert_eq!(after_stop(Stopped::Written, opened), AfterStop::Take);
        assert_eq!(after_stop(Stopped::Unsettled, opened), AfterStop::Unsettled);
    }
    assert_eq!(
        after_stop(Stopped::Unwritten, Opened::Below),
        AfterStop::Back
    );
    assert_eq!(
        after_stop(Stopped::Unwritten, Opened::Directly),
        AfterStop::Left
    );
}

#[test]
fn a_sign_in_being_stopped_says_so_while_it_waits() {
    let mut view = LoginView::new(Glyphs::Unicode);
    view.stopping();
    let (rows, _) = view.frame(80, "Log in to ChatGPT", Glyphs::Unicode);
    let text: Vec<String> = rows.iter().map(Row::text).collect();

    assert!(
        text.iter()
            .any(|row| row == "stopping; waiting for anything being stored…"),
        "{text:?}"
    );
}
