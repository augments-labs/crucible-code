//! What a credential given to `/login` leaves the session asking.

use std::cell::Cell;
use std::sync::Arc;

use crucible_auth::Store;
use crucible_builtins::{Ledger, Plan};
use crucible_client_api::{Capabilities, Capability};
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
        settled: std::cell::RefCell::default(),
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
        pending_speed: Cell::new(None),
        settings: crucible_config::Settings::default(),
        choosing: sample.root().join("unwritten-home.json"),
        logins: Store::in_home(&sample.root()),
        rewritten: crucible_app::access::Rewritten::new(),
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
        sourcing: Box::new(|_, _, _| crucible_app::startup::Reaching::nothing()),
        environment: Box::new(|_| None),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        sending: std::cell::Cell::default(),
        pinning: std::cell::Cell::default(),
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

    assert_eq!(conversation.settled().model(), "claude-test-1");
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

    assert_eq!(conversation.settled().model(), "claude-test-1");
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
    // includes the usage, or a key billed by what is sent; and an installed
    // agent's own sign-in third, where one is offered. No row is a sentence
    // about what pressing Enter does, and none names a vendor: the vendors
    // are the lists each leads to.
    let shown: Vec<&str> = FIRST.iter().map(|way| way.name).collect();
    let says: Vec<&str> = FIRST.iter().map(|way| way.says).collect();

    assert_eq!(
        shown,
        [
            "Your account with subscription",
            KEY_ROUTE_SHOWN,
            "Use an installed coding agent"
        ]
    );
    assert_eq!(
        says,
        [
            "Usage included in your paid plan",
            "API usage billing",
            "Sign in through an official harness"
        ]
    );
    assert_eq!(FIRST.get(..PAID).map(<[_]>::len), Some(2));
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
            refuses: &[],
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
        refuses: &[],
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
                "signed in · may train on what is sent · ChatGPT plan usage with Plus, Pro, \
                 Business and Enterprise"
                    .to_owned()
            ),
            (
                "Kimi Code · kimi.ai",
                "may train on what is sent · Kimi Code plan usage, accounts outside mainland \
                 China"
                    .to_owned()
            ),
            (
                "Kimi Code · kimi.com",
                "may use what is sent · Kimi Code plan usage, mainland China accounts".to_owned()
            ),
            (
                "MiniMax Token Plan · minimax.io",
                "may use what is sent · MiniMax Token Plan usage, accounts outside mainland China"
                    .to_owned()
            ),
            (
                "MiniMax Token Plan · minimaxi.com",
                "may use what is sent · MiniMax Token Plan usage, mainland China accounts"
                    .to_owned()
            ),
            (
                "Qwen Coding Plan · alibabacloud.com",
                "Qwen Coding Plan usage, the international site".to_owned()
            ),
            (
                "Qwen Coding Plan · aliyun.com",
                "uses what is sent · Qwen Coding Plan usage, the mainland China site".to_owned()
            ),
            (
                "Qwen Token Plan · alibabacloud.com",
                "Qwen Token Plan usage, the international site".to_owned()
            ),
            (
                "Qwen Token Plan · aliyun.com",
                "uses what is sent · Qwen Token Plan usage, the mainland China site".to_owned()
            ),
            ("Anthropic", "set ANTHROPIC_API_KEY".to_owned()),
            ("DeepSeek", "set DEEPSEEK_API_KEY".to_owned()),
            (
                "Google",
                "uses what is sent · set GEMINI_API_KEY".to_owned()
            ),
            ("Meta", "set META_API_KEY".to_owned()),
            ("MiMo", "set MIMO_API_KEY".to_owned()),
            (
                "MiniMax · minimax.io",
                "may use what is sent · set MINIMAX_API_KEY".to_owned()
            ),
            (
                "MiniMax · minimaxi.com",
                "may use what is sent · a pay-as-you-go key, mainland China accounts".to_owned()
            ),
            (
                "MoonshotAI · kimi.ai",
                "signed in · may train on what is sent".to_owned()
            ),
            (
                "MoonshotAI · kimi.com",
                "may use what is sent · set MOONSHOT_API_KEY".to_owned()
            ),
            ("OpenAI", "set OPENAI_API_KEY".to_owned()),
            (
                "Qwen · alibabacloud.com",
                "set DASHSCOPE_API_KEY".to_owned()
            ),
            (
                "Qwen · aliyun.com",
                "a pay-as-you-go key of the mainland China site".to_owned()
            ),
            ("xAI", "set XAI_API_KEY".to_owned()),
            (
                "Z.ai · bigmodel.cn",
                "may train on what is sent · a key of bigmodel.cn, mainland China accounts"
                    .to_owned()
            ),
            ("Z.ai · z.ai", "set ZAI_API_KEY".to_owned()),
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
        told: &[],
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
        drawn.contains(&"  signed in · may train on what is sent…".to_owned()),
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
    // columns too, folded only where the words are wider than the window.
    let rows = production();
    for columns in [80, 40] {
        let written = opened_into(columns, 40, false);
        let unfolded = crucible_tui::Picture::of(&written, columns, 200)
            .said()
            .join(" ");
        for way in rows.all() {
            let typed = format!("/login {} —", reaching(way, &rows));
            assert!(unfolded.contains(&typed), "{columns}: {typed}: {written}");
            // Narrower than the window, it is never broken at all.
            if typed.chars().count() < columns {
                assert!(written.contains(&typed), "{columns}: {typed}: {written}");
            }
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
    terms.consent.keeps_in(sample.user_file());
    let warned = crucible_app::content_use::WARNED
        .iter()
        .find(|one| one.route == "subscription:moonshot@kimi.ai")
        .expect("the kimi.ai sign-in row is warned");
    terms.consent.accept(warned).expect("a yes written down");
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

/// A warned row says its caution after `signed in` and before its own words,
/// so a window of forty columns cuts those rather than it; a row whose vendor
/// says nothing carries none.
#[test]
fn a_warned_row_keeps_its_caution_at_forty_columns() {
    let sample = Sample::new("login-caution-narrow");
    std::fs::write(sample.root().join("auth.json"), HELD).expect("a store");
    let terms = named(&sample);
    let rows = production();
    let providers = terms.providers.snapshot();
    let held = holding(&rows, &terms).expect("a store that reads whole");

    for list in [List::Subscription, List::Key] {
        let listed = listed(&rows, list);
        let drawn = entries(&listed, &held, &providers, Glyphs::Unicode);
        let shown: Vec<Offered<'_>> = drawn
            .iter()
            .map(|(name, says)| Offered { name, says })
            .collect();
        let panel = Panel {
            source: None,
            title: TITLE,
            said: None,
            told: &[],
            shown: &shown,
            chosen: 0,
            footer: BACK,
        };
        let picture: Vec<String> = panel
            .rows(40, Glyphs::Unicode)
            .iter()
            .map(|row| row.text().trim_end().to_owned())
            .collect();
        for way in &listed {
            let route = crucible_app::content_use::row_route(way);
            let at = picture
                .iter()
                .position(|row| row.trim_start().trim_start_matches("› ") == way.shown)
                .unwrap_or_else(|| panic!("{} is not drawn: {picture:?}", way.shown));
            let under = picture.get(at + 1).cloned().unwrap_or_default();
            match terms.consent.routes().warned(&route) {
                Some(warned) => {
                    assert!(
                        under.contains(warned.warning.caution.as_ref()),
                        "{route}: {under}"
                    );
                    let signed = under.find("signed in").unwrap_or(0);
                    assert!(
                        under.find(warned.warning.caution.as_ref()) >= Some(signed),
                        "{under}"
                    );
                }
                None => assert!(!under.contains("sent"), "{route}: {under}"),
            }
        }
    }
}

#[test]
fn the_new_vendors_rows_are_narrowed_and_replaced_by_their_own_words() {
    let rows = production();

    assert_eq!(left("qwen", &rows).len(), 6, "{:?}", left("qwen", &rows));
    assert_eq!(
        left("qwen coding aliyun.com", &rows),
        ["Qwen Coding Plan · aliyun.com API key"]
    );
    assert!(left("qwen tokyo", &rows).is_empty());

    // A key held on one row of a vendor is named when another of its rows is
    // chosen: the plans of one site, and the two sites of one vendor.
    let coding = way(&rows, List::Subscription, "Qwen Coding Plan · aliyun.com");
    let token = way(&rows, List::Subscription, "Qwen Token Plan · aliyun.com");
    assert_eq!(
        replaced(&token, &[coding], Glyphs::Unicode).as_deref(),
        Some("the API key held for Qwen Coding Plan · aliyun.com")
    );
    let international = way(&rows, List::Key, "Z.ai · z.ai");
    let mainland = way(&rows, List::Key, "Z.ai · bigmodel.cn");
    assert_eq!(
        replaced(&mainland, &[international], Glyphs::Unicode).as_deref(),
        Some("the API key held for Z.ai · z.ai")
    );

    // A MiniMax plan key and a pay-as-you-go key of one site are told apart
    // by the plan's name.
    assert_eq!(
        left("minimax token plan", &rows),
        [
            "MiniMax Token Plan · minimax.io API key",
            "MiniMax Token Plan · minimaxi.com API key"
        ]
    );
    let plan = way(
        &rows,
        List::Subscription,
        "MiniMax Token Plan · minimaxi.com",
    );
    let key = way(&rows, List::Key, "MiniMax · minimaxi.com");
    assert_eq!(
        replaced(&key, &[plan], Glyphs::Unicode).as_deref(),
        Some("the API key held for MiniMax Token Plan · minimaxi.com")
    );
}

#[test]
fn a_refused_key_is_answered_by_its_mark_and_never_its_text() {
    use crucible_app::providers::Misfit;
    use crucible_tui::Glyphs;

    for (misfit, said) in [
        (
            Misfit::Unmarked("sk-api-"),
            "! not a key for this row; its keys start sk-api-",
        ),
        (
            Misfit::Refused("tp-"),
            "! not a key for this row; keys starting tp- are another kind",
        ),
        (
            Misfit::Shared("sk-sp-"),
            "! keys starting sk-sp- are for another row; choose it by its plan and site",
        ),
        (
            Misfit::Another("MiniMax · minimax.io"),
            "! that is a key for MiniMax - minimax.io; choose that row",
        ),
    ] {
        assert_eq!(super::unfitting(&misfit, Glyphs::Ascii), said);
    }
}

#[test]
fn a_fact_an_agent_gave_reaches_the_panel_with_none_of_its_control_bytes() {
    let drawn = drawn_facts(&[
        "Reason: \u{1b}]0;title\u{7}status\tunavailable\nretry".to_owned(),
        "Agent: work, run as work.".to_owned(),
    ]);

    assert!(
        drawn.iter().all(|fact| !fact.chars().any(char::is_control)),
        "{drawn:?}"
    );
    assert_eq!(
        drawn.get(1).map(String::as_str),
        Some("Agent: work, run as work.")
    );
}

#[test]
fn the_login_agent_connect_claims_install_and_the_connect_logout_sends_withholds_it() {
    // `/login` carries a move on an agent out, so its connect claims install;
    // `/logout`'s Connect cannot, so the connect it sends claims every other
    // capability and is never offered one.
    let sample = Sample::new("login-connect-claims");
    let mut terms = in_force(&sample);
    let (client, claims) = crate::cli::client::Client::noting_claims();
    terms.client = client;
    let mut conversation = asking("claude-test-1");
    let mut asked = 0;

    let performed = connecting(&terms, &mut conversation, |_| {
        asked += 1;
        Ok::<_, Fatal>(None)
    })
    .expect("nobody answering fails nothing");
    let elsewhere = terms
        .operate(
            &mut conversation,
            Command::Connect { instance: None },
            |_| {
                asked += 1;
                Ok::<_, Fatal>(None)
            },
        )
        .expect("nobody answering fails nothing");

    for performed in [&performed, &elsewhere] {
        assert!(
            matches!(performed, Performed::Account(Account::Unconnected)),
            "{:?}",
            performed.outcome().kind()
        );
    }
    assert_eq!(asked, 2, "the agent question was put to each");
    let taken = claims.taken();
    let [(login, carried), (logout, withheld)] = taken.as_slice() else {
        panic!("two requests answered, not {}", taken.len());
    };
    assert_eq!(*login, Command::Connect { instance: None });
    assert_eq!(*logout, Command::Connect { instance: None });
    assert_eq!(*carried, Capabilities::every(), "/login claims install too");
    assert!(!withheld.has(Capability::Install), "{withheld:?}");
    assert_eq!(
        *withheld,
        Capabilities::every().without(Capability::Install),
        "every other capability is still claimed"
    );
}

/// What the agents door was doing, as the panel standing over it draws it:
/// `done` of `total` bytes of a download, where it is downloading, the link a
/// sign-in waits on, where it shows one, and whether a cancel was asked for.
fn working(download: Option<(u64, u64)>, link: Option<&str>, cancelling: bool) -> Working {
    let agent = crucible_client_api::Name::new("antigravity").expect("a name the contract takes");
    Working {
        title: TITLE,
        doing: "installing antigravity 1.3.0".to_owned(),
        progress: download.and_then(|(done, total)| {
            crucible_client_api::InstallProgress::new(agent.clone(), done, total)
        }),
        link: link.map(|link| (link.to_owned(), false)),
        cancel: Cancel::new(),
        cancelling,
    }
}

#[test]
fn every_state_of_the_working_panel_fits_every_window_it_is_given() {
    // The panel's own fit sweep, since it is laid out here and not among the
    // components the crate's sweep walks: downloading with a size and without,
    // being cancelled, showing a sign-in link short and long, and working with
    // nothing to show, each at every width and room it decides anything at,
    // in either glyph set.
    let long = format!(
        "https://accounts.example.com/o/oauth2/auth?{}",
        "state=x&".repeat(60)
    );
    let states = [
        ("sized", working(Some((512, 2048)), None, false)),
        ("unsized", working(Some((0, 0)), None, false)),
        ("cancelling", working(Some((1024, 2048)), None, true)),
        (
            "short link",
            working(None, Some("https://example.com/sign-in"), false),
        ),
        ("long link", working(None, Some(&long), false)),
        ("waiting", working(None, None, false)),
    ];
    for (state, panel) in &states {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for columns in 1..=200 {
                for room in 0..=24 {
                    let laid = panel.laid(columns, room, glyphs);
                    let at = format!("{state} {glyphs:?} {columns}x{room}");
                    assert!(laid.len() <= room, "{at}: {} rows", laid.len());
                    for row in &laid {
                        assert!(row.columns() <= columns, "{at}: {:?}", row.text());
                    }
                }
            }
        }
    }
}

#[test]
fn before_the_door_reports_anything_the_panel_says_which_move_is_under_way() {
    let name = |text: &str| crucible_client_api::Name::new(text).expect("a contract name");
    let moves = [
        (
            AgentAction::Install {
                agent: name("antigravity"),
                version: name("1.3.0"),
            },
            "installing antigravity 1.3.0",
        ),
        (
            AgentAction::Update {
                agent: name("antigravity"),
            },
            "updating antigravity",
        ),
        (
            AgentAction::Rollback {
                agent: name("antigravity"),
            },
            "rolling back antigravity",
        ),
        (
            AgentAction::SignIn {
                instance: name("work"),
                method: name("oauth-personal"),
            },
            "signing \"work\" in",
        ),
    ];
    for (action, says) in &moves {
        assert_eq!(doing(action), *says);
    }
    // Nothing reported yet: the panel names the move, and claims nothing of
    // who is at work, since an install is still fetching the catalog.
    let text: Vec<String> = working(None, None, false)
        .laid(80, 24, Glyphs::Unicode)
        .iter()
        .map(Row::text)
        .collect();
    assert!(
        text.iter()
            .any(|row| row == "Log in · installing antigravity 1.3.0"),
        "{text:?}"
    );
    assert!(text.iter().all(|row| !row.contains("agent")), "{text:?}");
}

#[test]
fn a_download_shows_its_share_only_where_its_size_is_known() {
    let text = |panel: &Working| -> String {
        panel
            .laid(80, 24, Glyphs::Unicode)
            .iter()
            .map(|row| row.text() + "\n")
            .collect()
    };

    let sized = text(&working(Some((512, 2048)), None, false));
    // A size of 0 is one the door does not know, and counts nothing done.
    let unknown = text(&working(Some((0, 0)), None, false));

    assert!(sized.contains("25%"), "{sized}");
    assert!(sized.contains("antigravity"), "{sized}");
    assert!(!unknown.contains('%'), "{unknown}");
    assert!(unknown.contains("0 bytes so far"), "{unknown}");
}

#[test]
fn escape_cancels_a_download_once_and_nothing_else_is_cancelled_by_it() {
    let mut downloading = working(Some((512, 2048)), None, false);
    assert_eq!(downloading.pressed(&Pressed::Escape), Moved::Redraw);
    assert!(downloading.cancel.requested() && downloading.cancelling);
    assert_eq!(downloading.pressed(&Pressed::Escape), Moved::Still);

    let mut signing_in = working(None, Some("https://example.com/sign-in"), false);
    assert_eq!(signing_in.pressed(&Pressed::Escape), Moved::Still);
    assert!(!signing_in.cancel.requested());
}

#[test]
fn a_download_every_byte_of_which_has_arrived_is_not_offered_for_cancelling() {
    let mut finished = working(Some((2048, 2048)), None, false);

    assert_eq!(finished.pressed(&Pressed::Escape), Moved::Still);
    assert_eq!(
        finished.pressed(&Pressed::Key(Key::Interrupt)),
        Moved::Still
    );
    assert!(!finished.cancel.requested() && !finished.cancelling);
    assert_eq!(footer_of(&finished), "waiting for it to finish");

    let cancelled_then_finished = working(Some((2048, 2048)), None, true);
    assert_eq!(
        footer_of(&cancelled_then_finished),
        "waiting for it to finish"
    );

    let mut sizeless = working(Some((0, 0)), None, false);
    assert_eq!(footer_of(&sizeless), "esc cancels the download");
    assert_eq!(sizeless.pressed(&Pressed::Escape), Moved::Redraw);
    assert!(sizeless.cancel.requested());
}

/// The last row `panel` draws in a window with room for all of it.
fn footer_of(panel: &Working) -> String {
    panel
        .laid(80, 24, Glyphs::Ascii)
        .last()
        .map(Row::text)
        .unwrap_or_default()
}

#[test]
fn a_refusal_quotes_every_instance_name_it_gives_with_its_controls_escaped() {
    use crucible_app::client::agents::AgentsError;
    use crucible_app::remember::RememberError;
    use crucible_config::{At, ConfigError};

    let hostile = || Box::<str>::from("space\u{1b}[2J\u{202e}");
    let pathed = || -> Box<str> { format!("externalAgents.{}.executable", hostile()).into() };
    let unspliceable = || ConfigError::Unspliceable {
        file: "config.json".into(),
        at: pathed(),
        written: "\"/agents/agy\"".into(),
    };
    for (refusal, quoted) in [
        (
            AgentsError::Running(vec![hostile()]),
            "\"space\\u{1b}[2J\\u{202e}\"",
        ),
        (
            AgentsError::Unstopped(vec![hostile()]),
            "\"space\\u{1b}[2J\\u{202e}\"",
        ),
        (
            AgentsError::Unlocated(hostile()),
            "\"space\\u{1b}[2J\\u{202e}\"",
        ),
        (
            AgentsError::Unclouded {
                method: "vertex".into(),
                instance: hostile(),
            },
            "\"space\\u{1b}[2J\\u{202e}\"",
        ),
        (
            AgentsError::Remember(RememberError::Unusable(unspliceable())),
            "\"externalAgents.space\\u{1b}[2J\\u{202e}.executable\"",
        ),
        (
            AgentsError::HoldUnkept(RememberError::Unusable(unspliceable())),
            "\"externalAgents.space\\u{1b}[2J\\u{202e}.executable\"",
        ),
        (
            AgentsError::Config(ConfigError::WrongType {
                file: "config.json".into(),
                path: pathed(),
                wanted: "a string",
                at: At::Ambiguous,
            }),
            "\"externalAgents.space\\u{1b}[2J\\u{202e}.executable\" wants a string",
        ),
    ] {
        let said = refused(&refusal);

        assert!(said.contains(quoted), "{said:?}");
        assert!(!said.contains("space\u{1b}"), "{said:?}");
        assert!(
            !said.chars().any(
                |character| character.is_control() || crucible_types::shown::unshown(character)
            ),
            "{said:?}"
        );
    }
}

#[test]
fn a_refusal_passed_on_in_the_doors_words_is_escaped_and_cut_with_the_cut_marked() {
    use crucible_app::client::agents::AgentsError;

    let refusal = AgentsError::Program(format!("agy\u{202e}{}", "x".repeat(4 * REFUSAL)).into());

    let said = refused(&refusal);

    assert!(
        said.starts_with("! the release lists no runnable agy\\u{202e}x"),
        "{said:?}"
    );
    assert!(said.ends_with('…'), "{said:?}");
    assert_eq!(said.chars().count(), "! ".len() + REFUSAL + 1);
    assert!(!said.contains('\u{202e}'), "{said:?}");
}

#[test]
fn a_download_given_up_is_said_with_no_mark_and_one_that_failed_is_marked() {
    use crucible_app::agent_download::DownloadError;
    use crucible_app::client::agents::AgentsError;

    let given_up = AgentsError::Download(DownloadError::Cancelled);
    let failed = AgentsError::Download(DownloadError::Tls);

    assert_eq!(refused(&given_up), GIVEN_UP);
    assert_eq!(sentence(&given_up), GIVEN_UP);
    assert_eq!(refused(&failed), format!("! {}", sentence(&failed)));
}

#[test]
fn a_move_whose_smoke_test_or_pruning_did_not_happen_says_why_with_no_mark_mid_line() {
    use crucible_app::client::agents::{AgentsError, Outcome as Door};

    let said = carried_out(&Door::Moved {
        agent: crucible_types::AgentKey::new("antigravity").expect("an agent key"),
        how: How::Installed,
        version: "1.2.3".into(),
        instances: vec!["work".into()],
        left: Vec::new(),
        downloaded: true,
        standing: false,
        unsmoked: Some("its program\u{202e} did not start".into()),
        unpruned: Some(Box::new(AgentsError::TooLarge)),
    })
    .expect("a move says what came of it");

    assert!(said.starts_with("antigravity 1.2.3 installed; "), "{said}");
    assert!(
        said.contains("; it was not smoke-tested: its program\\u{202e} did not start"),
        "{said}"
    );
    assert!(
        said.contains("; older versions were kept: the release is larger than"),
        "{said}"
    );
    assert!(!said.contains("! "), "{said}");
}

#[test]
fn a_key_with_nothing_in_it_or_longer_than_the_door_keeps_is_refused_before_anything() {
    use crucible_app::client::agents::KEY_BYTES;

    assert!(kept_key("").is_none());
    assert!(kept_key(" \t ").is_none());
    assert!(kept_key(&"k".repeat(KEY_BYTES + 1)).is_none());
    assert!(kept_key(&"k".repeat(KEY_BYTES)).is_some());
}

#[test]
fn a_sign_in_reads_as_the_agent_opening_a_session_and_promises_nothing_more() {
    use crucible_app::client::agents::{Outcome as Door, Signed};

    for by in [Signed::Agent, Signed::Kept] {
        let said = carried_out(&Door::SignedIn {
            instance: "space".into(),
            method: "google".into(),
            by,
        })
        .expect("a sign-in says what came of it");

        assert!(said.contains("opened a session"), "{said}");
        assert!(!said.contains("signed in to"), "{said}");
        assert!(said.contains("\"space\""), "{said}");
    }
}

/// Writes the configuration the keyed sign-in cases run under into
/// `sample`'s home: one instance of the agent the build knows, whose methods
/// are one that takes a key and one that does not.
fn signing(sample: &Sample) {
    let profile = sample.root().join("agy-profile");
    let document = serde_json::json!({"externalAgents": {"work": {
        "driver": "antigravity",
        "executable": "agy",
        "profileDir": profile.to_string_lossy(),
    }}});
    sample.user(&document.to_string());
}

/// What a key typed into the box in these cases is; it is never written into
/// an assertion or a message.
const TYPED: &str = "fabricated-agent-key-0123456789";

/// An agents door scripted to say each of `said` in turn, keeping what is
/// sent and how often the key box stood, and keeping a key in `home` as the
/// agent screen's door does.
struct Scripted {
    said: std::collections::VecDeque<Result<door::Outcome, AgentsError>>,
    /// What the key box is answered with: the key typed, or Escape.
    answer: Option<&'static str>,
    sent: Vec<Command>,
    asked: usize,
    home: crucible_config::Home,
}

impl Scripted {
    fn new(
        sample: &Sample,
        said: Vec<Result<door::Outcome, AgentsError>>,
        answer: Option<&'static str>,
    ) -> Self {
        Self {
            said: said.into(),
            answer,
            sent: Vec::new(),
            asked: 0,
            home: sample.found(),
        }
    }
}

impl<T: Terminal> Door<T> for Scripted {
    fn carry(
        &mut self,
        _: &mut Walk<'_, T>,
        action: &AgentAction,
    ) -> Result<Option<Result<door::Outcome, AgentsError>>, Fatal> {
        self.sent.push(action.command());
        Ok(self.said.pop_front())
    }

    fn ask(&mut self, _: &mut Walk<'_, T>, _: &str) -> Result<Asked, Fatal> {
        self.asked += 1;
        Ok(self
            .answer
            .map_or(Asked::Left, |typed| Asked::Key(typed.to_owned())))
    }

    fn keep(&mut self, instance: &str, method: &str, key: Typed) -> Result<(), AgentsError> {
        keep_key(&self.home, instance, method, key)
    }
}

/// A sign-in of the `work` instance by `method`.
fn signing_in(method: &str) -> AgentAction {
    AgentAction::SignIn {
        instance: crucible_client_api::Name::new("work").expect("a name the contract takes"),
        method: crucible_client_api::Name::new(method).expect("a name the contract takes"),
    }
}

/// What the door says of a sign-in by the key method with no key kept.
fn unstored() -> Result<door::Outcome, AgentsError> {
    Err(AgentsError::Unstored {
        agent: crucible_types::AgentKey::new("antigravity").expect("an agent key"),
        method: "\"gemini-api-key\"".into(),
    })
}

/// What the door says of `work` signed in by `method`.
fn signed_in(method: &str) -> door::Outcome {
    door::Outcome::SignedIn {
        instance: "work".into(),
        method: method.into(),
        by: Signed::Kept,
    }
}

/// Whether `sample`'s store holds a key for the agent's key method.
fn holds_the_key(sample: &Sample) -> bool {
    let agent = crucible_types::AgentKey::new("antigravity").expect("an agent key");
    let name = production()
        .signing_in(&agent, "gemini-api-key")
        .expect("the method's credential has a name");
    sample.store().read().has_key(&name)
}

/// `scripted` driven over `action` on a screen of its own, the walk's close
/// and the lines said handed back.
fn driven(
    sample: &Sample,
    action: &AgentAction,
    scripted: &mut Scripted,
) -> (Option<Closed>, String) {
    let terms = named(sample);
    let mut conversation = asking("claude-test-1");
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut walk = Walk {
        renderer: &mut renderer,
        conversation: &mut conversation,
        terms: &terms,
        held: &[],
    };
    let closed = carried(&mut walk, action, scripted).expect("the terminal");
    let written = renderer.terminal().written().to_owned();
    (closed, written)
}

#[test]
fn a_key_method_with_none_kept_asks_for_the_key_once_keeps_it_and_signs_in_again() {
    let sample = Sample::new("login-keyed-sign-in");
    signing(&sample);
    let action = signing_in("gemini-api-key");
    let mut door = Scripted::new(
        &sample,
        vec![unstored(), Ok(signed_in("gemini-api-key"))],
        Some(TYPED),
    );

    let (closed, written) = driven(&sample, &action, &mut door);

    // Before any message shows the screen, so none can show the key.
    assert!(!written.contains(TYPED), "the key typed reached the screen");
    assert_eq!(closed, None);
    assert_eq!(door.asked, 1, "the key box did not stand exactly once");
    assert_eq!(door.sent, vec![action.command(), action.command()]);
    assert!(holds_the_key(&sample), "the key typed was not kept");
    assert!(
        written.contains("the agent of \"work\" opened a session by gemini-api-key"),
        "{written}"
    );
}

#[test]
fn a_second_refusal_for_want_of_a_key_is_said_and_not_asked_about_again() {
    let sample = Sample::new("login-keyed-twice");
    signing(&sample);
    let action = signing_in("gemini-api-key");
    let mut door = Scripted::new(&sample, vec![unstored(), unstored()], Some(TYPED));

    let (closed, written) = driven(&sample, &action, &mut door);

    // Before any message shows the screen, so none can show the key.
    assert!(!written.contains(TYPED), "the key typed reached the screen");
    assert_eq!(closed, None);
    assert_eq!(
        door.asked, 1,
        "the key box stood again after the second refusal"
    );
    assert_eq!(door.sent.len(), 2, "the sign-in was not sent exactly twice");
    assert!(holds_the_key(&sample), "the key typed was not kept");
    assert!(
        written.contains("! no key is kept for antigravity to sign in by"),
        "{written}"
    );
}

#[test]
fn escape_at_the_key_box_keeps_nothing_and_sends_nothing_more() {
    let sample = Sample::new("login-keyed-left");
    signing(&sample);
    let action = signing_in("gemini-api-key");
    let mut door = Scripted::new(
        &sample,
        vec![unstored(), Ok(signed_in("gemini-api-key"))],
        None,
    );

    let (closed, written) = driven(&sample, &action, &mut door);

    assert_eq!(closed, None);
    assert_eq!(door.asked, 1);
    assert_eq!(door.sent.len(), 1, "a sign-in was sent after Escape");
    assert!(!holds_the_key(&sample), "a key was kept after Escape");
    assert!(written.contains(LEFT), "{written}");
}

#[test]
fn a_method_that_takes_no_key_is_never_asked_for_one() {
    let sample = Sample::new("login-keyless");
    signing(&sample);
    let action = signing_in("oauth-personal");
    let mut door = Scripted::new(&sample, vec![Ok(signed_in("oauth-personal"))], Some(TYPED));

    let (closed, written) = driven(&sample, &action, &mut door);

    assert_eq!(closed, None);
    assert_eq!(
        door.asked, 0,
        "the key box stood for a method that takes no key"
    );
    assert_eq!(door.sent, vec![action.command()]);
    assert!(
        !holds_the_key(&sample),
        "a key was kept for a method that takes none"
    );
    assert!(
        written.contains("the agent of \"work\" opened a session by oauth-personal"),
        "{written}"
    );
}
