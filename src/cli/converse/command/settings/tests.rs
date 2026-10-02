use crucible_client_api::{Context, Cost, Limits, Model, Percent, Used};
use crucible_tui::{Glyphs, Key, Pressed, Recording, Renderer, Row};

use super::*;
use crate::cli::converse::tests::{asking_plan, keeping, plain};
use crate::cli::sample::Sample;

/// A session that has used nothing yet.
fn counted() -> Counted {
    Counted {
        usage: api::Usage {
            used: Used {
                cost: Cost::NotPriced,
                api_ms: 0,
                wall_ms: 0,
                added: 0,
                removed: 0,
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            context: Context {
                model: Model::new("gpt-6-sol"),
                window: Some(872_000),
                left: Percent::new(100),
                system: 0,
                instructions: 0,
                tools: 0,
                mcp: 0,
                messages: 0,
                reserve: 0,
                free: 872_000,
            },
            limits: Limits::default(),
        },
        serving: None,
        mode: Mode::Ask,
        session: None,
    }
}

fn text(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// What the panel draws at `columns` by `room`, as text.
fn drawn(panel: &mut Panel, columns: usize, room: usize) -> Vec<String> {
    text(&panel.laid(columns, room, Glyphs::Unicode).0)
}

fn key(panel: &mut Panel, pressed: Pressed) -> Moved {
    panel.walked(pressed)
}

fn typed(panel: &mut Panel, word: &str) {
    for letter in word.chars() {
        key(panel, Pressed::Key(Key::Char(letter)));
    }
}

/// Walks the mark to the row labelled `label`, from the top.
fn walk_to(panel: &mut Panel, label: &str) {
    for _ in 0..crucible_config::rows().len() {
        key(panel, Pressed::Up);
    }
    for _ in 0..crucible_config::rows().len() {
        if panel.marked_label() == Some(label) {
            return;
        }
        key(panel, Pressed::Down);
    }
    panic!("no row is labelled {label}");
}

#[test]
fn settings_tabs_switch_with_tab_shift_tab_and_the_arrows() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    let tabs = |panel: &mut Panel| drawn(panel, 80, 40).get(2).cloned().unwrap_or_default();
    assert!(
        tabs(&mut panel).contains("‹ Config ›"),
        "{:?}",
        tabs(&mut panel)
    );

    assert_eq!(key(&mut panel, Pressed::Tab), Moved::Redraw);
    assert!(tabs(&mut panel).contains("‹ Usage ›"));
    assert!(
        drawn(&mut panel, 80, 40)
            .iter()
            .any(|row| row.starts_with("Usage"))
    );

    key(&mut panel, Pressed::Tab);
    assert!(tabs(&mut panel).contains("‹ Status ›"));
    let status = drawn(&mut panel, 80, 40);
    assert!(
        status.iter().any(|row| row.contains("Permission mode")),
        "{status:#?}"
    );
    assert!(
        status.iter().any(|row| row.contains("gpt-6-sol")),
        "{status:#?}"
    );

    key(&mut panel, Pressed::Cycle);
    assert!(tabs(&mut panel).contains("‹ Usage ›"));
    key(&mut panel, Pressed::Key(Key::Left));
    assert!(tabs(&mut panel).contains("‹ Config ›"));
    key(&mut panel, Pressed::Key(Key::Right));
    assert!(tabs(&mut panel).contains("‹ Usage ›"));

    assert_eq!(key(&mut panel, Pressed::Escape), Moved::Left);
}

#[test]
fn settings_search_filters_the_rows_by_label() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    key(&mut panel, Pressed::Key(Key::Char('/')));
    typed(&mut panel, "them");
    let rows = drawn(&mut panel, 80, 40);
    assert!(rows.iter().any(|row| row.contains("│ them")), "{rows:#?}");
    assert!(rows.iter().any(|row| row.contains("Theme")), "{rows:#?}");
    assert!(
        rows.iter().any(|row| row.contains("Syntax theme")),
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|row| row.contains("Glyphs")), "{rows:#?}");

    // Escape in the search clears it, and a second closes the panel.
    assert_eq!(key(&mut panel, Pressed::Escape), Moved::Redraw);
    let rows = drawn(&mut panel, 80, 40);
    assert!(rows.iter().any(|row| row.contains("Glyphs")), "{rows:#?}");
    assert_eq!(key(&mut panel, Pressed::Escape), Moved::Left);
}

#[test]
fn settings_enter_flips_a_flag_cycles_a_short_choice_and_steps_the_number() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    walk_to(&mut panel, "Scroll rail");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Took);
    assert_eq!(panel.asked_word(), Some("false"));

    walk_to(&mut panel, "Glyphs");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Char(' '))), Moved::Took);
    assert_eq!(panel.asked_word(), Some("ascii"));

    walk_to(&mut panel, "Mouse scroll speed");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Right)), Moved::Took);
    let stepped = panel.asked_word().map(str::to_owned);
    let usual: u16 = crucible_config::row("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED")
        .and_then(crucible_config::Row::usual)
        .and_then(|word| word.parse().ok())
        .expect("the speed has a default");
    assert_eq!(stepped, Some((usual + 1).to_string()));

    // A choice of more than three opens its options under the row.
    walk_to(&mut panel, "Theme");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Redraw);
    let rows = drawn(&mut panel, 80, 40);
    assert!(
        rows.iter().any(|row| row.trim() == "colourblind-dark"),
        "{rows:#?}"
    );
    key(&mut panel, Pressed::Down);
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Took);
    assert_eq!(panel.asked_word(), Some("dark"));
}

#[test]
fn a_settings_toggle_changes_the_running_value_and_the_file() {
    let sample = Sample::new("settings-toggle");
    let terms = keeping(&sample);
    let counted = counted();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer.rails(true);
    let before = renderer.transcript_columns();
    let mut panel = Panel::new(&terms, &counted);

    walk_to(&mut panel, "Scroll rail");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Took);
    settle(&mut renderer, &terms, &mut panel);

    assert_eq!(
        renderer.transcript_columns(),
        before + 1,
        "the rail is gone now"
    );
    let written = std::fs::read_to_string(sample.user_file()).expect("the file was written");
    assert!(written.contains("\"scrollRail\": false"), "{written}");
    let rows = drawn(&mut panel, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("Scroll rail") && row.ends_with("off")),
        "{rows:#?}"
    );

    walk_to(&mut panel, "Theme");
    key(&mut panel, Pressed::Key(Key::Enter));
    key(&mut panel, Pressed::Down);
    key(&mut panel, Pressed::Key(Key::Enter));
    settle(&mut renderer, &terms, &mut panel);
    assert_eq!(terms.chosen.get(), Some(crucible_config::ThemeChoice::Dark));
    let written = std::fs::read_to_string(sample.user_file()).expect("the file was written");
    assert!(written.contains("\"theme\": \"dark\""), "{written}");

    // A row that only a new start reads is written and says so.
    walk_to(&mut panel, "Tone");
    key(&mut panel, Pressed::Key(Key::Enter));
    settle(&mut renderer, &terms, &mut panel);
    let rows = drawn(&mut panel, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("explanatory · applies at next start")),
        "{rows:#?}"
    );

    // And a panel opened again shows what was taken, not what the start read.
    let again = Panel::new(&terms, &counted);
    let mut again = again;
    let rows = drawn(&mut again, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("Tone") && row.ends_with("explanatory · applies at next start")),
        "{rows:#?}"
    );
}

#[test]
fn a_settings_row_the_project_forces_is_locked_and_cannot_change() {
    let sample = Sample::new("settings-forced");
    let terms = Terms {
        settings: sample.settings(r#"{"output": {"glyphs": "ascii"}}"#),
        ..keeping(&sample)
    };
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    let rows = drawn(&mut panel, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("Glyphs") && row.ends_with("ascii · set by project config")),
        "{rows:#?}"
    );

    walk_to(&mut panel, "Glyphs");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Redraw);
    assert_eq!(panel.asked_word(), None);
    let rows = drawn(&mut panel, 80, 40);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("this value is set by the project config"),
        "{rows:#?}"
    );
    assert!(!sample.user_file().exists(), "nothing was written");
}

#[test]
fn every_settings_row_is_listed_on_the_config_tab() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    let rows = drawn(&mut panel, 80, 200);
    for row in crucible_config::rows() {
        assert!(
            rows.iter().any(|drawn| drawn.contains(row.label())),
            "{} has no row: {rows:#?}",
            row.label()
        );
    }
}

#[test]
fn the_settings_panel_fits_every_width_and_height_or_draws_nothing() {
    let terms = plain();
    let counted = counted();

    // Every view a reader can reach: each tab, a search, an opened choice.
    let views: [&dyn Fn(&mut Panel); 5] = [
        &|_| {},
        &|panel| {
            key(panel, Pressed::Tab);
        },
        &|panel| {
            key(panel, Pressed::Cycle);
        },
        &|panel| {
            key(panel, Pressed::Key(Key::Char('/')));
            typed(panel, "cache");
        },
        &|panel| {
            walk_to(panel, "Syntax theme");
            key(panel, Pressed::Key(Key::Enter));
        },
    ];

    let mut drew_narrow = false;
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for view in &views {
            for columns in 1..=200 {
                for room in [1, 4, 8, 12, 16, 24, 60] {
                    let mut panel = Panel::new(&terms, &counted);
                    view(&mut panel);
                    let (rows, caret) = panel.laid(columns, room, glyphs);
                    assert!(rows.len() <= room, "{columns}x{room}: {:#?}", text(&rows));
                    for row in &rows {
                        assert!(
                            row.columns() <= columns,
                            "{columns}x{room}: {:?} is too wide",
                            row.text()
                        );
                    }
                    if let Some(caret) = caret {
                        assert!(caret.row < rows.len() && caret.column <= columns);
                    }
                    drew_narrow |= columns == NARROWEST && room == 24 && !rows.is_empty();
                }
            }
        }
    }
    assert!(drew_narrow, "the narrowest width the panel names draws it");
}

#[test]
fn a_settings_panel_in_a_window_too_small_lists_the_values_instead() {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let terms = plain();

    listed(&mut renderer, &terms).expect("the listing to be written");

    let written = renderer.terminal().picture().said().join("\n");
    assert!(written.contains("Scroll rail"), "{written}");
    assert!(written.contains("Theme"), "{written}");
}

#[test]
fn the_settings_tab_row_shows_the_open_tab_whole_at_the_narrowest_width() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    for open in ["‹ Config ›", "‹ Usage ›", "‹ Status ›"] {
        let tabs = drawn(&mut panel, NARROWEST, 24)
            .get(2)
            .cloned()
            .unwrap_or_default();
        assert!(tabs.contains(open), "{open} is cut from {tabs:?}");
        key(&mut panel, Pressed::Tab);
    }
}

/// What the running session has for `id`, read back from where it lives,
/// where the running session reads it at all.
fn running(
    id: crucible_config::RowId,
    renderer: &Renderer<Recording>,
    terms: &Terms,
) -> Option<String> {
    use crucible_config::RowId;
    match id {
        RowId::Theme => theme::worn(terms).map(str::to_owned),
        RowId::SyntaxTheme => terms.reading.borrow().clone(),
        RowId::ScrollRail => Some(renderer.transcript_columns().to_string()),
        RowId::ScrollSpeed => Some(renderer.scroll_rows().to_string()),
        RowId::Glyphs => Some(format!("{:?}", terms.style().glyphs())),
        // The detail is how wide a call's arguments may run in a wide window.
        RowId::ToolDetail => Some(terms.style().args(1000).to_string()),
        RowId::Send => Some(format!("{:?}", terms.sending.get())),
        RowId::Colour
        | RowId::Tone
        | RowId::Compaction
        | RowId::UpdateCheck
        | RowId::CacheMode
        | RowId::CacheIsolation
        | RowId::CacheRetention
        | RowId::CachePersistent => None,
    }
}

/// A value `line`'s row takes that is not the one it has.
fn other(line: &Line) -> String {
    match line.row.values() {
        Values::Flag => Some(
            if line.value == "true" {
                "false"
            } else {
                "true"
            }
            .to_owned(),
        ),
        Values::Choice(words) => words
            .iter()
            .copied()
            .find(|word| *word != line.value)
            .map(str::to_owned),
        Values::Whole { least, most } => Some(
            if line.value == least.to_string() {
                most
            } else {
                least
            }
            .to_string(),
        ),
        Values::Named => crucible_tui::syntax::every_theme()
            .into_iter()
            .find(|name| *name != line.value && *name != crucible_tui::syntax::THEME_UNLESS_SAID),
    }
    .unwrap_or_else(|| panic!("{} offers one value", line.row.label()))
}

#[test]
fn every_settings_row_that_applies_at_once_changes_the_running_value() {
    let sample = Sample::new("settings-live");
    let terms = keeping(&sample);
    let counted = counted();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    // As the start leaves it, where no file says otherwise: the rail on and
    // the wheel at its usual speed.
    renderer.rails(true);
    let speed = crucible_config::row("env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED")
        .and_then(crucible_config::Row::usual)
        .and_then(|word| word.parse().ok())
        .expect("the speed has a default");
    renderer.rolls(speed);
    let mut panel = Panel::new(&terms, &counted);

    for at in 0..panel.lines.len() {
        let line = panel.lines.get(at).expect("a row is there");
        let (id, label) = (line.row.id(), line.row.label());
        if takes(id) != Takes::Now {
            continue;
        }
        let before = running(id, &renderer, &terms);
        let word = other(line);
        panel.asked = Some((at, word.clone()));
        settle(&mut renderer, &terms, &mut panel);
        let after = running(id, &renderer, &terms);
        assert!(
            after.is_some(),
            "{label} is read again but nothing shows it"
        );
        assert_ne!(
            before, after,
            "{label} set to {word} left the running value"
        );
        assert!(
            panel.lines.get(at).is_some_and(|line| !line.later),
            "{label} says it waits for a start"
        );
    }
}

#[test]
fn the_settings_rows_that_apply_at_once_are_these() {
    let live: Vec<&str> = crucible_config::rows()
        .iter()
        .filter(|row| takes(row.id()) == Takes::Now)
        .map(crucible_config::Row::label)
        .collect();
    assert_eq!(
        live,
        [
            "Theme",
            "Syntax theme",
            "Glyphs",
            "Tool detail",
            "Scroll rail",
            "Mouse scroll speed",
            "Send with"
        ]
    );
}

#[test]
fn a_settings_row_only_a_new_start_reads_says_so_before_it_is_changed() {
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);
    let rows = drawn(&mut panel, 100, 40);
    let row = |label: &str| {
        rows.iter()
            .find(|row| row.contains(label))
            .cloned()
            .unwrap_or_else(|| panic!("no {label} row in {rows:#?}"))
    };

    for label in [
        "Colour",
        "Tone",
        "Compaction",
        "Prompt caching",
        "Cache isolation",
        "Cache retention",
        "Persistent cache",
    ] {
        assert!(
            row(label).ends_with("· applies at next start"),
            "{label}: {rows:#?}"
        );
    }
    // Read again by the running session, or only when it is next used: bare.
    for label in [
        "Theme",
        "Glyphs",
        "Tool detail",
        "Scroll rail",
        "Send with",
        "Check for updates",
    ] {
        assert!(!row(label).contains("applies"), "{label}: {rows:#?}");
    }
}

#[test]
fn a_settings_change_the_user_file_cannot_take_is_still_in_force_and_says_so() {
    let sample = Sample::new("settings-unwritable");
    let terms = keeping(&sample);
    // Something that is not a file stands where the file goes, so no write
    // can land there whoever runs the test.
    std::fs::create_dir_all(sample.user_file()).expect("a directory in the file's place");
    let counted = counted();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer.rails(true);
    let before = renderer.transcript_columns();
    let mut panel = Panel::new(&terms, &counted);

    walk_to(&mut panel, "Scroll rail");
    assert_eq!(key(&mut panel, Pressed::Key(Key::Enter)), Moved::Took);
    let left = settle(&mut renderer, &terms, &mut panel);

    assert_eq!(left, None, "nothing is left in the transcript");
    assert_eq!(
        renderer.transcript_columns(),
        before + 1,
        "the rail is gone"
    );
    let note = panel.note.clone().unwrap_or_default();
    assert!(note.starts_with("! not written: "), "{note}");
    assert!(
        note.ends_with(" · in force for this session only"),
        "{note}"
    );
    let rows = drawn(&mut panel, 80, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("Scroll rail") && row.ends_with("off")),
        "{rows:#?}"
    );
}

#[test]
fn the_settings_status_tab_shows_a_session_by_the_first_eight_of_its_id() {
    let terms = plain();
    let id: crucible_types::SessionId = "0198f2a4-7c3e-7b21-9d4f-3a6c5e8b1f20"
        .parse()
        .expect("a session id");
    let counted = Counted {
        session: Some(id),
        ..counted()
    };
    let mut panel = Panel::new(&terms, &counted);
    key(&mut panel, Pressed::Tab);
    key(&mut panel, Pressed::Tab);

    let status = drawn(&mut panel, 40, 40);
    let session = status
        .iter()
        .find(|row| row.contains("Session"))
        .cloned()
        .unwrap_or_default();
    assert!(
        session.ends_with("Session              0198f2a4"),
        "{status:#?}"
    );
    assert!(
        !status.iter().any(|row| row.contains("7c3e")),
        "{status:#?}"
    );
}

#[test]
fn a_glyph_set_chosen_in_settings_is_the_one_the_next_answer_is_drawn_in() {
    let sample = Sample::new("settings-glyphs");
    let terms = keeping(&sample);
    let counted = counted();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    // In colour, where the markdown reader puts its own bullet in place of
    // the model's dash rather than keeping the dash.
    renderer.wears(crucible_tui::Palette::resolve(
        true,
        crucible_tui::Theme::Dark,
        None,
        &|name| (name == "COLORTERM").then(|| "truecolor".to_owned()),
    ));
    let mut panel = Panel::new(&terms, &counted);
    let at = panel
        .lines
        .iter()
        .position(|line| line.row.id() == crucible_config::RowId::Glyphs)
        .expect("a Glyphs row");
    panel.asked = Some((at, "ascii".to_owned()));
    settle(&mut renderer, &terms, &mut panel);

    renderer.stream("- next\n").expect("streamed");
    renderer.settle().expect("settled");
    let said = renderer.terminal().picture().said();
    assert!(
        said.iter().any(|row| row == "- next"),
        "the answer is not in the set chosen: {said:#?}"
    );
    assert!(
        !said
            .iter()
            .any(|row| row.contains(Glyphs::Unicode.bullet())),
        "{said:#?}"
    );
}

#[test]
fn every_settings_view_follows_the_colour_rule() {
    /// What brings the panel to one of the views checked.
    type Opening = fn(&mut Panel);

    // The marked row is the one line allowed its caret and its value both in
    // the accent; every other row, on every tab, in a search and in an opened
    // choice, lands the eye on one thing at most.
    let terms = plain();
    let counted = counted();
    let views: [(&str, Opening); 5] = [
        ("config", |_| {}),
        ("usage", |panel| {
            key(panel, Pressed::Tab);
        }),
        ("status", |panel| {
            key(panel, Pressed::Cycle);
        }),
        ("search", |panel| {
            key(panel, Pressed::Key(Key::Char('/')));
            typed(panel, "the");
        }),
        ("choice", |panel| {
            walk_to(panel, "Theme");
            key(panel, Pressed::Key(Key::Enter));
        }),
    ];

    for (name, view) in views {
        for (columns, glyphs) in [(80, Glyphs::Unicode), (40, Glyphs::Ascii)] {
            let mut panel = Panel::new(&terms, &counted);
            view(&mut panel);
            let rows = panel.laid(columns, 40, glyphs).0;

            crate::cli::colour_rule::holds(
                &format!("settings {name} at {columns}"),
                &rows,
                crate::cli::colour_rule::marked,
            );
        }
    }
}

#[test]
fn usage_tab_asks_the_plan_when_turned_to_and_draws_its_answer() {
    let (mut conversation, asked) = asking_plan();
    let terms = plain();
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);

    // Nothing is asked while another tab is shown.
    assert_eq!(panel.watched(&terms, &mut conversation), Moved::Still);
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 0);

    panel.turn(Tab::Usage);
    assert_eq!(panel.watched(&terms, &mut conversation), Moved::Redraw);
    let drawn_now = drawn(&mut panel, 80, 40);
    assert!(
        drawn_now
            .iter()
            .any(|row| row.ends_with("asking anthropic…")),
        "{drawn_now:#?}"
    );

    // The answer comes off the drawing thread, and the next look draws it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !panel
        .out
        .as_ref()
        .is_some_and(crate::cli::client::Out::ended)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the plan never answered"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(panel.watched(&terms, &mut conversation), Moved::Redraw);
    let answered = drawn(&mut panel, 80, 40);
    assert!(
        answered.iter().any(|row| row == "  gpt-5.3-codex-spark"),
        "{answered:#?}"
    );
    assert!(
        !answered.iter().any(|row| row.contains("asking")),
        "{answered:#?}"
    );

    // Turned away and back within the minute, it is not asked again.
    panel.turn(Tab::Config);
    panel.watched(&terms, &mut conversation);
    panel.turn(Tab::Usage);
    panel.watched(&terms, &mut conversation);
    assert!(panel.out.is_none());
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn usage_tab_closed_with_the_question_out_answers_it_and_the_next_opening_asks_nothing() {
    let mut conversation = crate::cli::converse::tests::stalled_plan();
    let (client, journal) = crate::cli::client::Client::noting();
    let mut terms = plain();
    terms.client = client;
    let counted = counted();
    let mut panel = Panel::new(&terms, &counted);
    panel.turn(Tab::Usage);
    assert_eq!(panel.watched(&terms, &mut conversation), Moved::Redraw);
    assert!(panel.out.is_some(), "the plan was not asked");

    usage::closed(panel.out.take(), &terms, &mut conversation);

    // The request is answered, with what was known, as the panel closes.
    assert!(
        matches!(
            journal.noted().as_slice(),
            [crate::cli::client::tests::Noted::Answered {
                asked: api::Command::AskLimits,
                outcome: api::Outcome::Usage(_),
                ..
            }]
        ),
        "{:#?}",
        journal.noted()
    );
    // The question was out, so it counts toward the minute: the tab opened
    // again at once asks nothing.
    let mut again = Panel::new(&terms, &counted);
    again.turn(Tab::Usage);
    again.watched(&terms, &mut conversation);
    assert!(
        again.out.is_none(),
        "the plan was asked again within the minute"
    );
}
