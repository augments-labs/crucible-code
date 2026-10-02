use super::*;

const fn part(slot: Slot, fill: Fill, size: u64) -> Part {
    Part { slot, fill, size }
}

/// Each run of one tone in `row`, as the tone and how many cells it covers.
fn runs(row: &Row) -> Vec<(Slot, usize)> {
    let mut runs: Vec<(Slot, usize)> = Vec::new();
    for (slot, text) in row.spans() {
        let cells = crate::columns(text);
        match runs.last_mut() {
            Some((last, count)) if *last == slot => *count += cells,
            _ => runs.push((slot, cells)),
        }
    }
    runs
}

#[test]
fn a_bar_spans_its_width_and_splits_it_by_size() {
    let parts = [
        part(Slot::Plain, Fill::Solid, 1),
        part(Slot::Accent, Fill::Solid, 1),
        part(Slot::Quiet, Fill::Shaded, 2),
    ];
    let row = Bar { parts: &parts }.row(8, Glyphs::Unicode);

    assert_eq!(row.text(), "████░░░░");
    assert_eq!(
        runs(&row),
        [(Slot::Plain, 2), (Slot::Accent, 2), (Slot::Quiet, 4)]
    );
}

#[test]
fn a_part_with_any_size_has_a_cell_and_an_empty_part_has_none() {
    let parts = [
        part(Slot::Plain, Fill::Solid, 1),
        part(Slot::DoneMark, Fill::Solid, 0),
        part(Slot::Accent, Fill::Solid, 499),
        part(Slot::Quiet, Fill::Shaded, 500),
    ];
    let row = Bar { parts: &parts }.row(80, Glyphs::Unicode);

    assert_eq!(row.columns(), 80);
    assert_eq!(
        runs(&row),
        [(Slot::Plain, 1), (Slot::Accent, 39), (Slot::Quiet, 40)]
    );
}

#[test]
fn a_bar_without_unicode_is_hashes_and_dots() {
    let parts = [
        part(Slot::Accent, Fill::Solid, 3),
        part(Slot::Quiet, Fill::Shaded, 1),
    ];

    assert_eq!(Bar { parts: &parts }.row(4, Glyphs::Ascii).text(), "###.");
}

#[test]
fn more_parts_than_columns_leave_the_smallest_out() {
    let parts = [
        part(Slot::Plain, Fill::Solid, 1),
        part(Slot::DoneMark, Fill::Solid, 50),
        part(Slot::DoingMark, Fill::Solid, 2),
        part(Slot::Quiet, Fill::Shaded, 40),
    ];
    let row = Bar { parts: &parts }.row(2, Glyphs::Unicode);

    assert_eq!(runs(&row), [(Slot::DoneMark, 1), (Slot::Quiet, 1)]);
}

#[test]
fn a_bar_of_nothing_draws_nothing() {
    let parts = [part(Slot::Plain, Fill::Solid, 0)];

    assert!(Bar { parts: &parts }.row(40, Glyphs::Unicode).is_empty());
    assert!(Bar { parts: &[] }.row(40, Glyphs::Unicode).is_empty());
}
