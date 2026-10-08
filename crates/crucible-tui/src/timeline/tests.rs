use super::*;

use crate::color::{Palette, Slot, Theme};
use crate::dump::dump;

/// A palette that writes colour and the addresses links carry.
fn for_tests() -> Palette {
    Palette::resolve(true, Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
    .addressing(true)
}

/// Twelve releases: two told in a row each, ten in full, the newest running.
const OLDER: [Brief<'static>; 2] = [
    Brief {
        version: "0.0.1",
        date: "2026-08-08",
        counted: &[("added", 18), ("known limits", 6)],
    },
    Brief {
        version: "0.0.2",
        date: "2026-08-09",
        counted: &[
            ("added", 1),
            ("fixed", 1),
            ("documented", 1),
            ("internal", 1),
        ],
    },
];

/// The words of the releases told in full, oldest first.
const WORDS: [&str; 10] = [
    "### Added\n\n- **A first command.** It lists what there is.",
    "### Fixed\n\n- **A crash on start is gone.** It read a file that was not there.\n- Another fix.",
    "### Changed\n\n**A paragraph under a heading.** It holds no bullet at all.",
    "### Security\n\n- **A TLS 1.3 handshake that breaks the protocol's encryption rules is refused.** \
     crucible now uses rustls 0.23.45, which rejects a handshake message a server sends in plaintext.",
    "### Added\n\n- **A table.**\n\n| Key | What it does |\n| --- | --- |\n| `Ctrl+O` | opens |\n",
    "### Fixed\n\n- **Code keeps its lines.**\n\n```\nfirst line\nsecond line\n```\n",
    "### Fixed\n\n- **A link.** See [the notes](https://example.invalid/notes) for more.",
    "### Removed\n\n- **An old flag.** It did nothing.",
    "### Added\n\n- **A number.** It was #123 upstream.",
    "**The newest summary.** Output redirected under `output.color` keeps the markdown.\n\n\
     ### Fixed\n\n- **Output keeps its markers.** That setting dropped them, so `**loud**` came out as loud.\n\
     - **A second fix.** With a longer sentence that has to wrap at forty columns and at eighty too, \
     so that the hanging indent shows.",
];

/// The versions of the releases told in full, oldest first.
const VERSIONS: [&str; 10] = [
    "0.36.0", "0.37.0", "0.38.0", "0.39.0", "0.40.0", "0.41.0", "0.41.1", "0.42.0", "0.43.0",
    "0.43.3",
];

/// The releases told in full.
fn told() -> Vec<Told<'static>> {
    VERSIONS
        .iter()
        .zip(WORDS)
        .map(|(version, text)| Told {
            brief: Brief {
                version,
                date: "2026-09-29",
                counted: &[("fixed", 1)],
            },
            text,
        })
        .collect()
}

/// The repository numbers are counted against.
fn forge() -> Forge {
    Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )
}

/// The whole list of the twelve at `columns`.
fn whole(
    told: &[Told<'_>],
    forge: &Forge,
    columns: usize,
    glyphs: Glyphs,
    most: usize,
) -> Vec<Row> {
    Timeline {
        older: &OLDER,
        told,
        running: Some("0.43.3"),
        forge: Some(forge),
        closing: Some("/release-notes <version> prints one"),
        most,
    }
    .rows(columns, glyphs)
}

/// The closing row, its folds joined: everything from its mark on.
fn closing(lines: &[String], glyphs: Glyphs) -> String {
    let from = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with(glyphs.hangs()))
        .expect("a closing row");
    lines
        .get(from..)
        .unwrap_or_default()
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// What each row says.
fn said(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

#[test]
fn the_whole_list_is_one_rail_from_the_first_release_to_the_running_one() {
    for (columns, glyphs) in [
        (80, Glyphs::Unicode),
        (40, Glyphs::Unicode),
        (80, Glyphs::Ascii),
        (40, Glyphs::Ascii),
    ] {
        let told = told();
        let rows = whole(&told, &forge(), columns, glyphs, 20_000);
        let lines = said(&rows);
        let (hollow, filled, rail) = match glyphs {
            Glyphs::Unicode => ("◇ ", "◆ ", "│"),
            Glyphs::Ascii => ("o ", "* ", "|"),
        };
        let at = format!("{columns} {glyphs:?}");

        // A hollow mark for each older release, on one line: the row after
        // each is the next release or the rail, never the rest of it.
        let briefs: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with(hollow))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(briefs, [0, 1], "{at}: {lines:#?}");
        assert!(
            lines.get(2).is_some_and(|line| line == rail),
            "{at}: {lines:#?}"
        );
        assert!(
            lines.iter().take(2).all(|line| line.contains("2026-08-0")),
            "{at}"
        );
        let dot = glyphs.dot();
        assert!(
            lines
                .get(1)
                .is_some_and(|line| line.contains(&format!("1 added {dot} 1 fi"))),
            "{at}: {lines:#?}"
        );

        // A filled mark for each release in full, in order.
        let heads: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with(filled))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(heads.len(), 10, "{at}: {lines:#?}");
        let first = *heads.first().expect("a release in full");
        let newest = *heads.last().expect("the newest");

        // The rail runs unbroken from the first release in full to the newest:
        // every row between is the rail, a mark, or words hanging off it.
        for line in lines.get(first..newest).unwrap_or_default() {
            assert!(
                line.starts_with(rail) || line.starts_with(filled),
                "{at}: a break in the rail: {line:?}"
            );
        }

        // The newest is the running version, marked so, with no rail under it.
        let head = lines.get(newest).expect("the newest");
        assert!(
            head.contains("0.43.3") && head.ends_with("this version"),
            "{at}: {head:?}"
        );
        let after = lines.get(newest + 1..).unwrap_or_default();
        assert!(
            after.iter().all(|line| !line.starts_with(rail)),
            "{at}: {after:#?}"
        );

        // The closing row stands last, folded where it has to be.
        let closing = closing(&lines, glyphs);
        assert!(
            closing.contains("12 releases")
                && closing.contains("/release-notes <version> prints one"),
            "{at}: {closing:?}"
        );

        for row in &rows {
            assert!(row.columns() <= columns, "{at}: {:?}", row.text());
        }
    }
}

#[test]
fn words_too_wide_for_any_row_are_left_out_and_never_drawn_past_the_edge() {
    // Two ideographs leave nothing a one column row can hold, and a bullet
    // whose words are only those leaves its mark and nothing after it.
    let bare = Spans::of(&Row::plain("\u{691c}\u{7d22}"));
    let marked = Spans::of(&Row::plain("\u{2022} \u{691c}\u{7d22}"));
    for (line, columns, hang) in [(&bare, 1, 0), (&marked, 3, 2)] {
        for row in hung(line, columns, hang) {
            assert!(row.columns() <= columns, "{:?}", row.text());
        }
    }
}

#[test]
fn notes_hang_off_the_rail_and_a_wrapped_bullet_hangs_under_its_words() {
    let told = told();
    let rows = whole(&told, &forge(), 40, Glyphs::Unicode, 20_000);
    let lines = said(&rows);

    let heading = lines
        .iter()
        .position(|line| line == "│  Security")
        .expect("the heading");
    let bullet = lines.get(heading + 1).expect("the bullet");
    assert!(bullet.starts_with("│  • A TLS 1.3"), "{bullet:?}");
    let wrapped = lines.get(heading + 2).expect("the bullet wrapped");
    assert!(
        wrapped.starts_with("│    ") && !wrapped.starts_with("│     "),
        "{wrapped:?}"
    );
}

#[test]
fn a_mark_the_rail_and_a_bold_lead_each_wear_their_slot() {
    let told = told();
    let rows = whole(&told, &forge(), 80, Glyphs::Unicode, 20_000);
    let spans = |row: &Row| {
        row.spans()
            .map(|(slot, text)| (slot, text.to_owned()))
            .collect::<Vec<_>>()
    };

    let brief = rows
        .iter()
        .find(|row| row.text().starts_with("◇ "))
        .expect("an older release");
    assert_eq!(
        spans(brief).first().map(|(slot, _)| *slot),
        Some(Slot::Quiet)
    );

    let head = rows
        .iter()
        .find(|row| row.text().starts_with("◆ 0.36.0"))
        .expect("a release in full");
    let head = spans(head);
    assert_eq!(
        head.first().map(|(slot, _)| *slot),
        Some(Slot::Accent),
        "{head:?}"
    );
    assert!(
        head.iter()
            .any(|(slot, text)| *slot == Slot::Strong && text == "0.36.0"),
        "{head:?}"
    );

    let rail = rows
        .iter()
        .find(|row| row.text().trim_end() == "│")
        .expect("the rail");
    assert!(
        rail.kinds().all(|slot| slot == Slot::Quiet),
        "{:?}",
        spans(rail)
    );

    let lead = rows
        .iter()
        .find(|row| row.text().contains("A first command."))
        .expect("a lead");
    assert!(
        spans(lead)
            .iter()
            .any(|(slot, text)| *slot == Slot::Bold && text.contains("A first command.")),
        "{:?}",
        spans(lead)
    );

    let code = rows
        .iter()
        .find(|row| row.text().contains("output.color"))
        .expect("inline code");
    assert!(
        spans(code)
            .iter()
            .any(|(slot, text)| *slot == Slot::Code && text == "output.color"),
        "{:?}",
        spans(code)
    );
}

#[test]
fn no_marker_of_the_markdown_reaches_the_screen_and_a_number_links_to_the_repository_given() {
    let told = told();
    let forge = forge();
    let rows = whole(&told, &forge, 80, Glyphs::Unicode, 20_000);
    let text = said(&rows).join("\n");

    assert!(!text.contains("**A") && !text.contains("###"), "{text}");
    let number = rows
        .iter()
        .find(|row| row.text().contains("#123"))
        .expect("the number");
    let painted = number.paint(&for_tests());
    assert!(
        painted.contains("https://github.com/augments-labs/crucible-code/issues/123"),
        "{painted:?}"
    );
}

#[test]
fn one_release_alone_has_no_rail_and_no_closing_row() {
    let told = told();
    let one = told.get(3..4).expect("one release");
    let rows = Timeline {
        older: &[],
        told: one,
        running: Some("0.43.3"),
        forge: None,
        closing: None,
        most: 20_000,
    }
    .rows(40, Glyphs::Unicode);
    let lines = said(&rows);

    assert!(
        lines
            .first()
            .is_some_and(|line| line == "◆ 0.39.0 · 2026-09-29"),
        "{lines:#?}"
    );
    assert!(
        lines.iter().all(|line| !line.starts_with('│')),
        "{lines:#?}"
    );
    assert!(lines.iter().all(|line| !line.contains('⎿')), "{lines:#?}");
    assert!(
        lines.iter().any(|line| line.starts_with("   • A TLS 1.3")),
        "{lines:#?}"
    );
}

#[test]
fn past_the_most_rows_what_is_left_out_goes_oldest_first_and_the_closing_row_counts_it() {
    let told = told();
    let forge = forge();
    for columns in [40, 80] {
        let everything = whole(&told, &forge, columns, Glyphs::Unicode, 20_000).len();

        for most in [everything - 1, everything / 2, 40] {
            let rows = whole(&told, &forge, columns, Glyphs::Unicode, most);
            let lines = said(&rows);
            let at = format!("{columns} columns, {most} rows");
            assert!(rows.len() <= most, "{at}: {} rows", rows.len());

            // Older rows go before any words do, and oldest first.
            let briefs = lines
                .iter()
                .filter(|line| line.starts_with("◇ 0.0."))
                .count();
            let heads = lines.iter().filter(|line| line.starts_with("◆ ")).count();
            let briefly = lines.iter().filter(|line| line.starts_with("◇ 0.")).count() - briefs;
            let older = 2 - briefs;
            assert!(
                briefly == 0 || older == 2,
                "{at}: words went before the older rows"
            );
            if briefs == 1 {
                assert!(
                    lines.iter().any(|line| line.contains("0.0.2")),
                    "{at}: {lines:#?}"
                );
            }
            assert_eq!(heads + briefly, 10, "{at}");

            // The newest is kept, and the closing row counts what went.
            assert!(
                lines.iter().any(|line| line.contains("A second fix.")),
                "{at}: {lines:#?}"
            );
            let closing = closing(&lines, Glyphs::Unicode);
            if older > 0 {
                assert!(
                    closing.contains(&format!("{older} of the older rows left out")),
                    "{at}: {closing:?}"
                );
            }
            if briefly > 0 {
                assert!(
                    closing.contains(&format!("{briefly} told in a row each")),
                    "{at}: {closing:?}"
                );
            }
            if older + briefly > 0 {
                assert!(closing.contains("incomplete"), "{at}: {closing:?}");
            }
        }
    }
}

#[test]
fn a_newest_release_that_alone_passes_the_most_is_cut_at_its_end_and_said_so() {
    let told = told();
    let newest = told.get(9..).expect("the newest");
    let rows = Timeline {
        older: &OLDER,
        told: newest,
        running: Some("0.43.3"),
        forge: None,
        closing: Some("/release-notes <version> prints one"),
        most: 6,
    }
    .rows(200, Glyphs::Unicode);
    let lines = said(&rows);

    assert!(rows.len() <= 6, "{lines:#?}");
    assert!(
        lines
            .first()
            .is_some_and(|line| line.starts_with("◆ 0.43.3")),
        "{lines:#?}"
    );
    let closing = closing(&lines, Glyphs::Unicode);
    assert!(
        closing.contains("the end of 0.43.3 left out"),
        "{closing:?}"
    );
    assert!(
        closing.contains("2 of the older rows left out"),
        "{closing:?}"
    );
}

#[test]
fn a_most_below_the_least_the_whole_can_be_told_in_gives_that_least_and_says_what_went() {
    // The releases told in full, each cut to a row, the rail, the newest's
    // first row and the closing row are the least the whole can be told in;
    // below that, that is what comes.
    let told = told();
    let rows = Timeline {
        older: &OLDER,
        told: &told,
        running: Some("0.43.3"),
        forge: None,
        closing: Some("/release-notes <version> prints one"),
        most: 6,
    }
    .rows(200, Glyphs::Unicode);
    let lines = said(&rows);
    let closing = closing(&lines, Glyphs::Unicode);

    // Nine in a row, the rail, the newest's head, a blank and the closing row.
    assert_eq!(rows.len(), 13, "{lines:#?}");
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("◆ 0.43.3") && line.ends_with("this version")),
        "{lines:#?}"
    );
    for brief in told.iter().map(|told| told.brief) {
        assert!(
            lines.iter().any(|line| line.contains(brief.version)),
            "{} went unsaid: {lines:#?}",
            brief.version
        );
    }
    assert!(
        closing.contains(&format!("{} of the older rows left out", OLDER.len())),
        "{closing:?}"
    );
    assert!(
        closing.contains(&format!("{} told in a row each", told.len() - 1)),
        "{closing:?}"
    );
    assert!(
        closing.contains("the end of 0.43.3 left out"),
        "{closing:?}"
    );
}

#[test]
fn with_no_release_told_in_full_the_closing_row_names_none() {
    let rows = Timeline {
        older: &OLDER,
        told: &[],
        running: None,
        forge: None,
        closing: Some("/release-notes <version> prints one"),
        most: 100,
    }
    .rows(80, Glyphs::Unicode);
    let closing = closing(&said(&rows), Glyphs::Unicode);

    assert!(
        closing.starts_with(&format!("⎿ {} releases · /release-notes", OLDER.len())),
        "{closing:?}"
    );
}

/// The twelve as a picture, for the four pictures kept beside this.
fn pictured(columns: usize, glyphs: Glyphs) -> String {
    let told = told();
    dump(&whole(&told, &forge(), columns, glyphs, 20_000), columns)
}

#[test]
fn the_whole_list_at_eighty_columns() {
    insta::assert_snapshot!(pictured(80, Glyphs::Unicode));
}

#[test]
fn the_whole_list_at_forty_columns() {
    insta::assert_snapshot!(pictured(40, Glyphs::Unicode));
}

#[test]
fn the_whole_list_at_eighty_columns_in_ascii() {
    insta::assert_snapshot!(pictured(80, Glyphs::Ascii));
}

#[test]
fn the_whole_list_at_forty_columns_in_ascii() {
    insta::assert_snapshot!(pictured(40, Glyphs::Ascii));
}

#[test]
fn the_release_notes_follow_the_colour_rule() {
    // Every heading's mark is the one thing on its row in the accent; the
    // version beside it is strong and the date quiet, at every width.
    let told = told();
    for (columns, glyphs) in [(80, Glyphs::Unicode), (40, Glyphs::Ascii)] {
        let rows = whole(&told, &forge(), columns, glyphs, 20_000);

        crate::colour_rule::holds("release notes", &rows, |_| false);

        let heading = rows
            .iter()
            .find(|row| row.text().contains("0.36.0"))
            .map(|row| row.spans().map(|(slot, _)| slot).collect::<Vec<_>>());
        assert_eq!(
            heading,
            Some(vec![Slot::Accent, Slot::Strong, Slot::Quiet]),
            "a release heading at {columns}: its mark, its version, its date"
        );
    }
}
