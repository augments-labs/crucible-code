use crate::color::Palette;

use super::*;

/// Three sessions, which is exactly what a column holds.
const WORKED_IN: [Recent<'static>; 3] = [
    Recent {
        title: "a search that stops partway",
        when: "2h ago",
    },
    Recent {
        title: "rule replacement on windows",
        when: "yesterday",
    },
    Recent {
        title: "column counting in the tail",
        when: "3d ago",
    },
];

/// The widths worth walking: every one the assemblies change at, and a stretch
/// on either side of each.
const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// A session in a directory that has these behind it.
fn welcome<'a>(sessions: &'a [Recent<'a>]) -> Welcome<'a> {
    Welcome {
        version: "v0.0.8",
        model: "claude-sonnet-5",
        effort: None,
        root: "~/code/crucible-code",
        sessions,
    }
}

/// What the component says, with no colour in it. The art on its own, which is
/// where a change to the layout shows up.
fn drawn(welcome: &Welcome<'_>, columns: usize, glyphs: Glyphs) -> String {
    welcome
        .rows(columns, glyphs)
        .iter()
        .map(Row::text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A palette that writes every hue it has.
fn colourful() -> Palette {
    Palette::resolve(true, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn a_framed_terminal_gets_rows_that_all_end_at_its_last_column() {
    // The property the whole component is built to hold. A row past the last
    // column is one the terminal wraps itself, which leaves the cursor a row
    // below where the next frame expects it -- so the next frame erases
    // somebody else's line. A row short of it leaves the frame open on one
    // side, which is only ugly.
    for columns in WIDTHS.filter(|columns| *columns >= FRAMED_AT) {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for sessions in [&WORKED_IN[..], &[]] {
                for row in welcome(sessions).rows(columns, glyphs) {
                    assert_eq!(
                        row.columns(),
                        columns,
                        "{glyphs:?} at {columns}: {:?}",
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn a_terminal_with_no_room_for_a_frame_is_still_never_drawn_past() {
    // Nothing pads out here -- there is no edge on the right to reach -- so
    // what is owed is only that no row goes over.
    for columns in WIDTHS.take_while(|columns| *columns < FRAMED_AT) {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for row in welcome(&WORKED_IN).rows(columns, glyphs) {
                assert!(
                    row.columns() <= columns,
                    "{glyphs:?} at {columns}: {:?}",
                    row.text()
                );
            }
        }
    }
}

#[test]
fn eighty_columns_draws_the_identity_beside_what_happened_here() {
    let drawn = drawn(&welcome(&WORKED_IN), 80, Glyphs::Unicode);

    assert_eq!(
        drawn,
        "\
╭─ crucible v0.0.8 ────────────────────────────────────────────────────────────╮
│                                  │ Recent sessions                           │
│  ▄▄▄ ▄▄▄ █  █ ▄▄▄ █ ▄▄▄ █   ▄▄▄  │                                           │
│  █   █▄▄ █  █ █   █ █▄█ █   █▄▄  │ 1  a search that stops partway     2h ago │
│  ▀▄▄ █ █ ▀▄▄▀ ▀▄▄ █ █▄█ █▄▄ █▄▄  │ 2  rule replacement on windows  yesterday │
│                                  │ 3  column counting in the tail     3d ago │
│  claude-sonnet-5                 │                                           │
│                                  │ ───────────────────────────────────────── │
│  ~/code/crucible-code            │                                           │
│                                  │ Tips                                      │
│                                  │                                           │
│                                  │ · /          opens the command list       │
│                                  │ · shift+tab  steps the permission mode    │
│                                  │ · ctrl-c     clears the line typed so far │
╰──────────────────────────────────────────────────────────────────────────────╯"
    );
}

#[test]
fn a_terminal_too_narrow_for_two_columns_stacks_them() {
    // Rather than squeezing: a thirty-two column identity cannot get narrower
    // without the wordmark breaking, and a wordmark that breaks is worse than
    // one that gives up its art.
    let drawn = drawn(&welcome(&WORKED_IN), FRAMED_AT, Glyphs::Unicode);

    assert_eq!(
        drawn,
        "\
╭─ crucible v0.0.8 ──────────────────────────╮
│                                            │
│ CRUCIBLE                                   │
│ claude-sonnet-5                            │
│ ~/code/crucible-code                       │
│                                            │
│ Recent sessions                            │
│                                            │
│ 1  a search that stops partway      2h ago │
│ 2  rule replacement on windows   yesterday │
│ 3  column counting in the tail      3d ago │
│                                            │
│ ────────────────────────────────────────── │
│                                            │
│ Tips                                       │
│                                            │
│ · /          opens the command list        │
│ · shift+tab  steps the permission mode     │
│ · ctrl-c     clears the line typed so far  │
╰────────────────────────────────────────────╯"
    );
}

#[test]
fn a_terminal_too_narrow_for_a_frame_keeps_only_what_is_a_fact() {
    // What this is, what it is asking, and where. A tip is an offer, and an
    // offer elided to three characters is not one.
    let drawn = drawn(&welcome(&WORKED_IN), FRAMED_AT - 1, Glyphs::Unicode);

    assert_eq!(
        drawn,
        "\
CRUCIBLE
claude-sonnet-5
~/code/crucible-code"
    );
}

#[test]
fn a_font_without_box_drawing_is_told_rather_than_guessed() {
    // A terminal showing hollow squares has a font problem, and a font is not
    // something this process can ask about. What it can do is take an answer --
    // and the answer has to reach every glyph, not only the frame.
    for columns in WIDTHS {
        let drawn = drawn(&welcome(&WORKED_IN), columns, Glyphs::Ascii);

        assert!(
            drawn.is_ascii(),
            "{columns} columns drew something a plain font may not have: {drawn:?}"
        );
    }
}

#[test]
fn a_directory_nobody_has_worked_in_still_says_so_under_the_heading() {
    // The heading stays. Its absence would be a different thing than its
    // emptiness, and only one of the two is worth reading.
    let drawn = drawn(&welcome(&[]), 80, Glyphs::Unicode);

    assert!(drawn.contains("Recent sessions"), "{drawn}");
    assert!(drawn.contains("No recent sessions"), "{drawn}");
    assert!(!drawn.contains("/resume"), "{drawn}");
}

#[test]
fn more_sessions_than_fit_puts_the_way_to_the_rest_on_screen() {
    let many = [WORKED_IN.as_slice(), &WORKED_IN[..1]].concat();
    let drawn = drawn(&welcome(&many), 80, Glyphs::Unicode);

    assert!(drawn.contains("/resume for more"), "{drawn}");
    // The fourth is not drawn as well as being counted.
    assert_eq!(drawn.matches("a search that stops partway").count(), 1);
}

#[test]
fn a_session_title_gives_up_columns_and_the_time_it_happened_never_does() {
    // Half a title still identifies a session. Half a date identifies nothing,
    // and the reader cannot tell which half they are looking at.
    let long = [Recent {
        title: "a search that stops partway through a directory nobody meant to open",
        when: "yesterday",
    }];
    let drawn = drawn(&welcome(&long), 80, Glyphs::Unicode);

    assert!(drawn.contains("yesterday"), "{drawn}");
    assert!(drawn.contains('…'), "the title was not elided: {drawn}");
}

#[test]
fn colour_says_what_a_row_means_and_never_changes_what_it_says() {
    // The frame lines up because every row was measured before the palette
    // touched it. A hue that cost a column would move the edge on its right by
    // one, on that row alone.
    for columns in [40, FRAMED_AT, 80, 120] {
        for row in welcome(&WORKED_IN).rows(columns, Glyphs::Unicode) {
            assert_eq!(
                crate::width::columns(&row.paint(colourful())),
                row.columns()
            );
            assert_eq!(row.paint(Palette::plain()), row.text());
        }
    }
}

#[test]
fn nothing_the_component_draws_reaches_above_the_screen() {
    // These rows are written straight to the terminal rather than through the
    // tail that drops escape sequences, so what they may carry is worth
    // asserting where they are built.
    let painted = welcome(&WORKED_IN)
        .rows(80, Glyphs::Unicode)
        .iter()
        .map(|row| row.paint(colourful()))
        .collect::<String>();

    for upward in [
        "\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H", "\x1b[A", "\x1b[48;",
    ] {
        assert!(
            !painted.contains(upward),
            "a welcome row carried {upward:?}: {painted:?}"
        );
    }
}

#[test]
fn the_wordmark_is_a_rectangle() {
    // A row a column short leans the whole mark; a row a column long pushes it
    // through the edge beside it, and only the second of those fails a test
    // about widths.
    let inset = IDENTITY - width::columns(INDENT);
    let art = parts::wordmark(inset, Glyphs::Unicode);
    let wide = art.first().expect("the wordmark has rows").columns();

    for row in &art {
        assert_eq!(row.columns(), wide, "{:?}", row.text());
    }
    assert!(wide <= inset, "the wordmark is wider than its own column");
}

#[test]
fn a_deep_directory_gives_up_its_middle_rather_than_its_name() {
    let deep = Welcome {
        root: "~/code/vendor/checkouts/2026/crucible-code",
        ..welcome(&WORKED_IN)
    };
    let drawn = drawn(&deep, 80, Glyphs::Unicode);

    assert!(drawn.contains("~/…/crucible-code"), "{drawn}");
}

#[test]
fn how_hard_a_model_is_thinking_is_drawn_only_where_there_is_one() {
    let asked = Welcome {
        effort: Some("high"),
        ..welcome(&WORKED_IN)
    };

    assert!(drawn(&asked, 80, Glyphs::Unicode).contains("claude-sonnet-5 · high effort"));
    assert!(!drawn(&welcome(&WORKED_IN), 80, Glyphs::Unicode).contains("effort"));
}
