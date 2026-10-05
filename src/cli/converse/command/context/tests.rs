use super::*;

use crucible_client_api::{Model, Percent};

use super::super::{Command, MidTurn};

/// A long session on a large window, with nothing from a Model Context
/// Protocol server.
fn spent() -> api::Context {
    api::Context {
        model: Model::new("gpt-6-sol"),
        window: Some(872_000),
        left: Percent::new(62),
        system: 3_100,
        instructions: 9_800,
        tools: 4_200,
        mcp: 0,
        messages: 184_600,
        reserve: 128_000,
        free: 542_300,
    }
}

/// The same request on a model that never said how large its window is.
fn unmeasured() -> api::Context {
    api::Context {
        model: Model::new("my-local-model"),
        window: None,
        left: None,
        free: 0,
        ..spent()
    }
}

/// The panel's rows at `columns` where the whole of it stands: a rule, the
/// body, and how to close it.
fn panel(context: &api::Context, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    framed(
        body(context, columns, glyphs),
        columns,
        glyphs,
        "esc to close".to_owned(),
    )
}

fn art(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// The tone of the cell at `column` of `row`.
fn tone(row: &Row, column: usize) -> Option<Slot> {
    let mut at = 0;
    for (slot, text) in row.spans() {
        for one in text.chars() {
            if at == column {
                return Some(slot);
            }
            at += crucible_tui::columns(one.encode_utf8(&mut [0; 4]));
        }
    }
    None
}

#[test]
fn context_of_a_known_window_is_its_bar_and_a_row_for_every_category() {
    let rows = panel(&spent(), 80, Glyphs::Unicode);

    // The bar draws a part per category: a cell for each with tokens, however
    // few; none for MCP, which has none; the shaded rest is free.
    let bar = format!("{}{}", "█".repeat(31), "░".repeat(49));
    assert_eq!(
        art(&rows),
        [
            "─".repeat(80).as_str(),
            "",
            "Context · gpt-6-sol · 872k window",
            "",
            &bar,
            "",
            "  ██ system prompt           3.1k     0.4%",
            "  ██ project instructions    9.8k     1.1%",
            "  ██ tool schemas            4.2k     0.5%",
            "  ██ MCP tool schemas           0       0%",
            "  ██ messages              184.6k    21.2%",
            "  ██ reserve               128.0k    14.7%",
            "  ░░ free                  542.3k      62%",
            "",
            "esc to close",
        ]
    );
}

#[test]
fn context_rows_wear_the_tone_their_part_of_the_bar_is_drawn_in() {
    let rows = panel(&spent(), 80, Glyphs::Unicode);
    let tones = [
        Slot::Plain,
        Slot::DoneMark,
        Slot::DoingMark,
        Slot::Trouble,
        Slot::Accent,
        Slot::Quiet,
        Slot::Quiet,
    ];
    let bar = rows.get(4).expect("the bar");

    // The bar's first cells, in the order the window fills: one apiece for
    // the three small parts, then messages.
    for (column, slot) in [Slot::Plain, Slot::DoneMark, Slot::DoingMark, Slot::Accent]
        .into_iter()
        .enumerate()
    {
        assert_eq!(tone(bar, column), Some(slot), "bar column {column}");
    }
    assert_eq!(tone(bar, 79), Some(Slot::Quiet));

    for (row, slot) in rows.iter().skip(6).zip(tones) {
        assert_eq!(tone(row, 2), Some(slot), "{:?}", row.text());
        assert_eq!(tone(row, 3), Some(slot), "{:?}", row.text());
        // The figures are quiet from column 26, where they start.
        assert_eq!(tone(row, 26), Some(Slot::Quiet), "{:?}", row.text());
        assert_eq!(tone(row, 32), Some(Slot::Quiet), "{:?}", row.text());
    }
    assert_eq!(tone(rows.get(2).expect("the title"), 0), Some(Slot::Strong));
    assert_eq!(tone(rows.last().expect("the footer"), 0), Some(Slot::Quiet));
}

#[test]
fn context_of_a_window_not_known_has_no_bar_no_free_row_and_no_percent() {
    let rows = panel(&unmeasured(), 80, Glyphs::Unicode);

    assert_eq!(
        art(&rows),
        [
            "─".repeat(80).as_str(),
            "",
            "Context · my-local-model · window not known",
            "",
            "  system prompt              3.1k",
            "  project instructions       9.8k",
            "  tool schemas               4.2k",
            "  MCP tool schemas              0",
            "  messages                 184.6k",
            "  reserve                  128.0k",
            "",
            "esc to close",
        ]
    );
}

#[test]
fn context_at_forty_columns_keeps_tokens_and_percent_behind_one_swatch_cell() {
    let rows = panel(&spent(), 40, Glyphs::Unicode);
    let bar = format!("{}{}", "█".repeat(16), "░".repeat(24));

    assert_eq!(
        art(&rows),
        [
            "─".repeat(40).as_str(),
            "",
            "Context · gpt-6-sol · 872k window",
            "",
            &bar,
            "",
            "█ system prompt          3.1k   0.4%",
            "█ project instructions   9.8k   1.1%",
            "█ tool schemas           4.2k   0.5%",
            "█ MCP tool schemas          0     0%",
            "█ messages             184.6k  21.2%",
            "█ reserve              128.0k  14.7%",
            "░ free                 542.3k    62%",
            "",
            "esc to close",
        ]
    );
}

#[test]
fn context_without_unicode_is_drawn_in_hashes_and_dots() {
    let rows = panel(&spent(), 80, Glyphs::Ascii);
    let text = art(&rows);

    assert_eq!(text.first(), Some(&"-".repeat(80)));
    assert_eq!(
        text.get(2).map(String::as_str),
        Some("Context - gpt-6-sol - 872k window")
    );
    assert_eq!(
        text.get(4),
        Some(&format!("{}{}", "#".repeat(31), ".".repeat(49)))
    );
    assert_eq!(
        text.get(6).map(String::as_str),
        Some("  ## system prompt           3.1k     0.4%")
    );
    assert_eq!(
        text.get(12).map(String::as_str),
        Some("  .. free                  542.3k      62%")
    );
}

#[test]
fn context_shows_free_as_the_figure_the_prompt_line_reads() {
    // The prompt line reads what is left of the room before compaction, in
    // whole percent. Free here is 62.2% of the whole window, and the line
    // above the prompt would say 64: the row says what the line says.
    let context = api::Context {
        left: Percent::new(64),
        ..spent()
    };
    let rows = panel(&context, 80, Glyphs::Unicode);
    let free = rows.get(12).map(Row::text).unwrap_or_default();

    assert!(free.ends_with(" 64%"), "{free:?}");
    assert_eq!(
        free,
        format!(
            "  ░░ free                  542.3k {:>8}",
            format!("{}%", context.left.map_or(0, Percent::get))
        )
    );
}

#[test]
fn context_fits_every_width_down_to_one_column() {
    let enormous = api::Context {
        model: Model::new(&"a-model-with-a-very-long-name-".repeat(4)),
        window: Some(u64::MAX),
        system: u64::MAX / 8,
        messages: u64::MAX / 8,
        free: u64::MAX / 2,
        ..spent()
    };
    for context in [spent(), unmeasured(), enormous] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for columns in 1..=200 {
                for row in panel(&context, columns, glyphs) {
                    assert!(
                        row.columns() <= columns,
                        "{} at {columns} with {glyphs:?}: {:?}",
                        row.columns(),
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn context_opens_while_a_turn_runs() {
    // It reads and changes nothing, so it stands over the running turn with
    // the figures that turn last reported, which include anything it has
    // recorded since its last request.
    assert_eq!(Command::Context.mid_turn(), MidTurn::Live);
}

#[test]
fn context_closes_on_escape_and_the_keys_every_panel_closes_on() {
    // Escape is the key the footer names. Interrupt and end of input close
    // every panel, so they close this one too; enter leaves the panel
    // standing, as any other key does.
    for pressed in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        assert_eq!(closing(&pressed), Moved::Left, "{pressed:?}");
    }
    assert_eq!(closing(&Pressed::Resized), Moved::Redraw);
    for key in [Key::Enter, Key::Char('q')] {
        assert_eq!(closing(&Pressed::Key(key)), Moved::Still, "{key:?}");
    }
}

#[test]
fn context_follows_the_colour_rule() {
    // The messages part is the bar's one accent, and its row below the bar
    // is the legend's.
    for (name, context) in [("known", spent()), ("unknown", unmeasured())] {
        for (columns, glyphs) in [(80, Glyphs::Unicode), (40, Glyphs::Ascii)] {
            let rows = panel(&context, columns, glyphs);

            crate::cli::colour_rule::holds(&format!("context {name} at {columns}"), &rows, |_| {
                false
            });
        }
    }
}

#[test]
fn context_taller_than_its_room_stands_in_it_and_scrolls_with_the_arrows() {
    let mut scrolled = Scrolled::default();
    // The body is 11 rows, and with the rule, the blank rows and the footer
    // the panel would be 15: a 12-row room holds 7 of them and a row saying
    // how many more are below.
    let stood = art(&scrolled.laid(&spent(), 40, 12, Glyphs::Unicode));
    assert_eq!(stood.len(), 12, "{stood:#?}");
    assert_eq!(
        stood.first().map(String::as_str),
        Some("─".repeat(40).as_str())
    );
    assert!(
        stood
            .get(2)
            .is_some_and(|row| row.starts_with("Context · gpt-6-sol")),
        "{stood:#?}"
    );
    assert_eq!(
        stood.get(9).map(String::as_str),
        Some("  ↓ 4 more"),
        "{stood:#?}"
    );
    assert_eq!(
        stood.last().map(String::as_str),
        Some("esc to close · ↑↓ to see more")
    );

    assert_eq!(scrolled.pressed(&Pressed::Up), Moved::Still);
    assert_eq!(scrolled.pressed(&Pressed::Down), Moved::Redraw);
    let moved = art(&scrolled.laid(&spent(), 40, 12, Glyphs::Unicode));
    assert_eq!(moved.len(), 12, "{moved:#?}");
    assert!(
        !moved.iter().any(|row| row.starts_with("Context")),
        "{moved:#?}"
    );
    assert_eq!(
        moved.get(9).map(String::as_str),
        Some("  ↓ 3 more"),
        "{moved:#?}"
    );

    // At the foot the free row stands over the footer, and ↓ is spent.
    for _ in 0..3 {
        assert_eq!(scrolled.pressed(&Pressed::Down), Moved::Redraw);
    }
    assert_eq!(scrolled.pressed(&Pressed::Down), Moved::Still);
    let foot = art(&scrolled.laid(&spent(), 40, 12, Glyphs::Unicode));
    assert!(
        !foot.iter().any(|row| row.trim_start().starts_with('↓')),
        "{foot:#?}"
    );
    assert!(
        foot.iter()
            .rev()
            .nth(2)
            .is_some_and(|row| row.contains(" free ")),
        "{foot:#?}"
    );
    assert_eq!(scrolled.pressed(&Pressed::Up), Moved::Redraw);
    assert_eq!(scrolled.pressed(&Pressed::Escape), Moved::Left);

    // Where the whole panel fits, it stands whole and says only how to close.
    for (context, rows) in [(spent(), 15), (unmeasured(), 12)] {
        let whole = art(&scrolled.laid(&context, 40, rows, Glyphs::Unicode));
        assert_eq!(whole, art(&panel(&context, 40, Glyphs::Unicode)));
        assert_eq!(whole.last().map(String::as_str), Some("esc to close"));
    }

    // In ASCII the arrows and the dot are drawn as the font has them.
    let ascii = art(&scrolled.laid(&spent(), 40, 12, Glyphs::Ascii));
    assert_eq!(
        ascii.last().map(String::as_str),
        Some("esc to close - ^v to see more")
    );

    // Where even the rule, the blank rows, the footer and a row of the body
    // with the row under it have no room, nothing stands, and the caller
    // prints.
    for room in [0, 4, 5] {
        assert!(
            scrolled
                .laid(&spent(), 40, room, Glyphs::Unicode)
                .is_empty(),
            "at {room}"
        );
    }
    assert_eq!(scrolled.laid(&spent(), 40, 6, Glyphs::Unicode).len(), 6);
}
