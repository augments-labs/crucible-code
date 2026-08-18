//! What the list of running commands says, and what it gives up first.

use super::*;

/// A window wide enough that nothing is cut for width.
const WIDE: usize = 80;

fn one(number: usize, called: &str, seconds: u64, lines: usize, bytes: usize) -> Command<'_> {
    Command {
        number,
        called,
        running: Duration::from_secs(seconds),
        lines,
        bytes,
    }
}

fn text(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

#[test]
fn every_command_is_listed_with_how_long_and_how_much() {
    let shown = [
        one(1, "Bash(npm run dev)", 252, 84, 6_400),
        one(2, "Bash(cargo watch -x test)", 63, 512, 48_000),
    ];
    let rows = text(
        &Running {
            shown: &shown,
            at: 0,
        }
        .rows(WIDE, 24, Glyphs::Unicode),
    );

    assert!(rows.iter().any(|row| row.contains(TITLE)), "{rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.contains("1. Bash(npm run dev)") && row.contains("4m 12s")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("2. Bash(cargo watch -x test)") && row.contains("1m 03s")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|row| row.contains("84 lines")), "{rows:?}");
    assert!(rows.iter().any(|row| row.contains("6.4 kB")), "{rows:?}");
    assert!(rows.iter().any(|row| row.contains(KEYS)), "{rows:?}");
}

#[test]
fn the_row_a_key_acts_on_is_marked_as_well_as_coloured() {
    // The convention every list here keeps: a terminal drawing no colour at all
    // still says which row `enter` and `x` are about.
    let shown = [one(1, "Bash(one)", 1, 1, 1), one(2, "Bash(two)", 1, 1, 1)];
    let rows = text(
        &Running {
            shown: &shown,
            at: 1,
        }
        .rows(WIDE, 24, Glyphs::Unicode),
    );

    let marked: Vec<&String> = rows
        .iter()
        .filter(|row| {
            row.contains("Bash(") && row.trim_start().starts_with(|c: char| !c.is_numeric())
        })
        .collect();

    assert_eq!(marked.len(), 1, "{rows:?}");
    assert!(
        marked.first().is_some_and(|row| row.contains("Bash(two)")),
        "the mark was not on the row a key would act on: {rows:?}"
    );
}

#[test]
fn a_mark_past_the_end_of_the_list_still_lands_on_a_row() {
    // The list is drawn from a copy that may be a frame old, and a command that
    // exited between the frame and the key is one row shorter than the mark
    // expects. It lands on the last row rather than nowhere.
    let shown = [one(1, "Bash(one)", 1, 1, 1)];
    let rows = Running {
        shown: &shown,
        at: 9,
    }
    .rows(WIDE, 24, Glyphs::Unicode);

    assert_eq!(rows.len(), shown.len() + CHROME);
}

#[test]
fn nothing_running_draws_nothing_at_all() {
    let rows = Running { shown: &[], at: 0 }.rows(WIDE, 24, Glyphs::Unicode);

    assert!(rows.is_empty());
    assert_eq!(Running { shown: &[], at: 0 }.height(), 0);
}

#[test]
fn a_window_too_short_for_the_whole_list_draws_none_of_it() {
    // Half a list of processes is worse than the count that was already on the
    // row below, and a region taller than the window is one the renderer cannot
    // rewind over.
    let shown = [one(1, "Bash(one)", 1, 1, 1), one(2, "Bash(two)", 1, 1, 1)];
    let panel = Running {
        shown: &shown,
        at: 0,
    };

    assert!(
        panel
            .rows(WIDE, panel.height() - 1, Glyphs::Unicode)
            .is_empty()
    );
    assert!(!panel.rows(WIDE, panel.height(), Glyphs::Unicode).is_empty());
}

#[test]
fn a_narrow_window_keeps_the_way_out_and_gives_up_the_rest() {
    let shown = [one(1, "Bash(one)", 1, 1, 1)];
    let rows = text(
        &Running {
            shown: &shown,
            at: 0,
        }
        .rows(20, 24, Glyphs::Unicode),
    );
    let last = rows.last().cloned().unwrap_or_default();

    assert!(last.contains(CLOSE), "{last:?}");
    assert!(!last.contains("stops it"), "{last:?}");
}

#[test]
fn how_long_and_how_much_are_read_the_way_the_row_above_the_box_reads_them() {
    assert_eq!(elapsed(Duration::from_secs(9)), "9s");
    assert_eq!(elapsed(Duration::from_secs(63)), "1m 03s");
    assert_eq!(counted(1, "line"), "1 line");
    assert_eq!(counted(0, "line"), "0 lines");
    assert_eq!(sized(238), "238 B");
    assert_eq!(sized(6_400), "6.4 kB");
    assert_eq!(sized(1_000), "1 kB");
    assert_eq!(sized(48_000), "48 kB");
    assert_eq!(sized(1_200_000), "1.2 MB");
}
