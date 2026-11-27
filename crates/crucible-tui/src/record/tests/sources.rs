//! The source a drawn row gives back, walked newest first, and a drawn
//! block laid out again for a new width, glyph set or theme.

use super::*;

#[test]
fn every_row_of_a_drawn_block_gives_its_source_and_prose_rows_give_none() {
    let mut record = Record::new(60);
    record.write(Slot::Plain, "before\n", None);
    record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
    record.write(Slot::Plain, "after\n", None);

    let rows = rows_of(&record, 1);
    assert!(rows.len() > 1, "a table draws more than a row: {rows:?}");
    for row in rows {
        assert_eq!(
            record.source_of(row),
            Some(table("alpha").source.as_str()),
            "row {row}"
        );
    }
    assert_eq!(record.source_of(0), None);
    assert_eq!(record.source_of(rows_of(&record, 2).start), None);
    assert_eq!(record.source_of(9_999), None);
}

#[test]
fn a_rule_has_no_source_to_give() {
    let mut record = Record::new(40);
    record.drawn(
        block(markdown::Kind::Rule, "---\n"),
        Glyphs::Unicode,
        Theme::Dark,
    );

    assert_eq!(record.lines(), 1);
    assert_eq!(record.source_of(0), None);
    assert_eq!(record.sources().count(), 0);
}

#[test]
fn the_sources_are_walked_newest_first_by_the_word_they_are_called() {
    let mut record = Record::new(60);
    record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
    record.write(Slot::Plain, "between\n", None);
    record.drawn(
        block(markdown::Kind::DisplayMath, "$$\nx^2\n$$\n"),
        Glyphs::Unicode,
        Theme::Dark,
    );
    record.drawn(
        block(markdown::Kind::Diff, "```diff\n-a\n+b\n```\n"),
        Glyphs::Unicode,
        Theme::Dark,
    );

    let walked: Vec<(usize, &str, &str)> = record
        .sources()
        .map(|source| (source.line, source.word, source.text))
        .collect();

    assert_eq!(
        walked,
        [
            (3, "diff", "```diff\n-a\n+b\n```\n"),
            (2, "math", "$$\nx^2\n$$\n"),
            (0, "table", table("alpha").source.as_str()),
        ]
    );
}

/// Every row the record holds, as plain text.
fn all(record: &Record) -> Vec<String> {
    (0..record.lines())
        .flat_map(|line| record.folded(line))
        .map(|row| row.text())
        .collect()
}

#[test]
fn a_drawn_table_is_laid_out_again_for_a_new_width_and_a_new_glyph_set() {
    let fresh = |columns, glyphs| {
        let mut record = Record::new(columns);
        record.drawn(table("alpha"), glyphs, Theme::Dark);
        all(&record)
    };
    let mut record = Record::new(60);
    record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
    assert_eq!(all(&record), fresh(60, Glyphs::Unicode));

    record.relay(20, Glyphs::Unicode, Theme::Dark);
    assert_eq!(all(&record), fresh(20, Glyphs::Unicode));
    assert_ne!(fresh(20, Glyphs::Unicode), fresh(60, Glyphs::Unicode));

    record.relay(20, Glyphs::Ascii, Theme::Dark);
    assert_eq!(all(&record), fresh(20, Glyphs::Ascii));
    assert_ne!(fresh(20, Glyphs::Ascii), fresh(20, Glyphs::Unicode));
    assert_eq!(record.rows, counted(&record));
}

#[test]
fn a_new_theme_paints_a_drawn_block_without_laying_it_out_again() {
    let mut record = Record::new(40);
    record.drawn(table("alpha"), Glyphs::Unicode, Theme::Dark);
    let Some(Line::Drawn(before)) = record.lines.back() else {
        panic!("the table is not a drawn block: {:?}", record.lines);
    };
    let rows = before.rows.clone();

    record.relay(40, Glyphs::Unicode, Theme::Light);

    let Some(Line::Drawn(after)) = record.lines.back() else {
        panic!("the table is not a drawn block: {:?}", record.lines);
    };
    assert_eq!(after.rows, rows);
    assert_eq!(after.laid, (40, Glyphs::Unicode, Theme::Light));
}
