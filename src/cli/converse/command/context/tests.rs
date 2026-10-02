use super::*;

use crucible_client_api::{Model, Percent};

use super::super::{Command, MidTurn};

/// The window the mockup draws: a long session on a large window, with
/// nothing from a Model Context Protocol server.
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

    // The bar a part a category: a cell for each with tokens however small,
    // none for MCP, which has none, and the shaded rest is free.
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
        // The figures are quiet from the column the mockup starts them in.
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
    // the figures of the last request that turn built.
    assert_eq!(Command::Context.mid_turn(), MidTurn::Live);
}

#[test]
fn context_closes_on_escape_and_the_keys_every_panel_closes_on() {
    // Escape is the key the footer names. Interrupt and end of input close
    // every panel, so they close this one too; enter, as the mockup draws it,
    // leaves it standing, as any other key does.
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
