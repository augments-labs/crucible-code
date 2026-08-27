use super::*;
use crate::dump::dump;

/// Long enough to break at any width in the sweep, and to need clipping in a
/// column sized for a word.
const CROWDED: &str = "a model whose name nobody would type twice, kept here because a layout that \
                       fits short words fits nothing";

const RUNGS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

const KEYS: (&str, &str) = (
    "tab pane · ↑↓ model · ←→ effort · enter takes both · esc to cancel",
    "tab · ↑↓ · ←→ · enter · esc",
);

/// What the shelf holds unnarrowed, which the fixtures are a query's worth of.
const HELD: usize = 12;

const NAMES: [&str; 12] = [
    "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth",
    "eleventh", "twelfth",
];

/// The row a pane's header is on, which every geometry assertion counts from.
const HEADER: usize = 7;

/// The first row of a pane's body.
const BODY: usize = 9;

/// One row of what a shelf drew, as text.
fn said(rows: &[Row], at: usize) -> String {
    rows.get(at).expect("a row the shelf drew").text()
}

/// A snapshot drawing without the row-end cells `dump` pads to the width.
fn picture(rows: &[Row], columns: usize) -> String {
    dump(rows, columns)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn serving() -> Vec<Serving<'static>> {
    vec![
        Serving {
            name: "All",
            count: Some(4),
        },
        Serving {
            name: "Anthropic",
            count: Some(3),
        },
        Serving {
            name: "MoonshotAI",
            count: Some(1),
        },
        Serving {
            name: "OpenAI",
            count: None,
        },
    ]
}

fn stocked() -> Vec<Stocked<'static>> {
    vec![
        Stocked {
            name: "claude-sonnet-5",
            by: "Anthropic",
            window: "1M",
            note: "",
            now: true,
        },
        Stocked {
            name: "claude-opus-5",
            by: "Anthropic",
            window: "200K",
            note: "",
            now: false,
        },
        Stocked {
            name: "claude-haiku-4-5",
            by: "Anthropic",
            window: "200K",
            note: "no rung",
            now: false,
        },
        Stocked {
            name: CROWDED,
            by: "MoonshotAI",
            window: "—",
            note: CROWDED,
            now: false,
        },
    ]
}

/// More models than any room in the sweep leaves rows for.
fn many() -> Vec<Stocked<'static>> {
    (0..12)
        .map(|which| Stocked {
            name: NAMES.get(which).copied().unwrap_or_default(),
            by: "Anthropic",
            window: "200K",
            note: "",
            now: which == 0,
        })
        .collect()
}

fn shelf<'a>(providers: &'a [Serving<'a>], models: &'a [Stocked<'a>]) -> Shelf<'a> {
    Shelf {
        title: "Model",
        now: "now  anthropic/claude-sonnet-5 · high",
        query: "",
        typed: 0,
        hint: "a model, or a vendor",
        providers,
        provider: 0,
        models,
        held: HELD,
        model: 0,
        rungs: &RUNGS,
        rung: 2,
        nothing: "nothing matches — backspace to widen it",
        pane: Pane::Models,
        keys: KEYS,
        norung: "no rung",
        pointer: None,
    }
}

#[test]
fn a_room_short_of_the_chrome_answers_with_nothing_at_all() {
    // A pane with no rows in it is a border around nothing, and a caller reads
    // an empty answer as there having been no room to stand one.
    let providers = serving();
    let models = stocked();
    let shelf = shelf(&providers, &models);

    for room in 0..=CHROME {
        assert!(
            shelf.within(100, room, Glyphs::Unicode).is_empty(),
            "drew something into a room of {room}"
        );
    }

    assert_eq!(
        shelf.within(100, CHROME + 1, Glyphs::Unicode).len(),
        CHROME + 1
    );
}

#[test]
fn the_panes_pad_to_the_bottom_rather_than_stopping_at_the_last_model() {
    // The one bargain this inverts. Four models in room for twenty-six rows of
    // pane still answers with every row, because the band it stands in has no
    // share and the frame has to close at the bottom of it.
    let providers = serving();
    let models = stocked();

    let rows = shelf(&providers, &models).within(100, 40, Glyphs::Unicode);

    assert_eq!(rows.len(), 40);
}

#[test]
fn the_divider_between_the_panes_is_the_mark_the_frame_is_drawn_with() {
    // Three uprights on a row of two panes: the two edges and the one between
    // them. A divider spelled any other way is a second answer to a question
    // the glyph set already answers in one word.
    let providers = serving();
    let models = stocked();
    let shelf = shelf(&providers, &models);

    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = shelf.within(100, 30, glyphs);

        for at in [HEADER, BODY] {
            assert_eq!(
                said(&rows, at).matches(glyphs.vertical()).count(),
                3,
                "{glyphs:?} row {at}: {:?}",
                said(&rows, at)
            );
        }
    }
}

#[test]
fn the_ascii_frame_is_one_mark_at_every_joint_and_is_the_same_width_as_the_other() {
    // A set that spells a joint narrower or wider than the other draws a shelf
    // that fits at one setting and not at the other, and the setting is the
    // reader's.
    let providers = serving();
    let models = stocked();
    let shelf = shelf(&providers, &models);

    let unicode = shelf.within(100, 30, Glyphs::Unicode);
    let ascii = shelf.within(100, 30, Glyphs::Ascii);

    assert_eq!(unicode.len(), ascii.len());
    for (at, (one, other)) in unicode.iter().zip(&ascii).enumerate() {
        assert_eq!(one.columns(), other.columns(), "row {at}");
    }

    for at in [2, 4, 6, 8, 30 - 5] {
        let drawn = said(&ascii, at);
        assert!(
            drawn
                .chars()
                .all(|mark| mark == '+' || mark == '-' || mark == ' '),
            "row {at} is not made of ascii joints: {drawn:?}"
        );
    }
}

#[test]
fn the_marked_model_is_on_screen_on_every_rung_of_the_scroll() {
    // A list scrolled to its top would leave the keys about to act on something
    // off screen.
    let providers = serving();
    let models = many();

    for (mark, name) in NAMES.iter().enumerate() {
        let mut shelf = shelf(&providers, &models);
        shelf.model = mark;

        let rows = shelf.within(100, 20, Glyphs::Unicode);

        assert!(
            rows.iter().any(|row| row.text().contains(name)),
            "{name} is off screen with the mark on it"
        );
    }
}

#[test]
fn what_the_last_row_counts_is_models_left_rather_than_rows_left() {
    // Twelve models into a body of six rows: five are drawn and seven are not.
    // A count of rows would say six and be wrong by the row doing the saying.
    let providers = serving();
    let models = many();

    let rows = shelf(&providers, &models).within(100, 20, Glyphs::Unicode);

    let last = said(&rows, 20 - 6);
    assert!(last.contains("7 more"), "{last:?}");
}

#[test]
fn below_fifty_nine_columns_the_providers_fold_into_the_header_and_the_border_stays_one() {
    // Two panes cost more in borders and padding than they return down there.
    // What must not happen is the fold leaving a second frame behind it.
    let providers = serving();
    let models = stocked();
    let shelf = shelf(&providers, &models);

    let folded = shelf.within(FOLDS_AT - 1, 30, Glyphs::Unicode);
    assert_eq!(said(&folded, HEADER).matches('│').count(), 2);
    assert_eq!(said(&folded, BODY).matches('│').count(), 2);
    assert!(
        said(&folded, HEADER).contains("Anthropic"),
        "{:?}",
        said(&folded, HEADER)
    );

    let apart = shelf.within(FOLDS_AT, 30, Glyphs::Unicode);
    assert_eq!(said(&apart, HEADER).matches('│').count(), 3);
}

#[test]
fn a_query_longer_than_the_search_line_is_typed_at_its_end() {
    // The line is a field, and a field that goes on showing its front answers
    // every keystroke with the same picture — the reader is typing somewhere
    // they cannot see.
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    let typed = "sonnet with a much longer thing typed after it";
    shelf.query = typed;
    shelf.typed = typed.chars().count();

    let rows = shelf.within(40, 30, Glyphs::Unicode);
    let line = said(&rows, SEARCHING);
    assert!(
        line.contains("typed after it"),
        "the end of the query is not on the line: {line:?}"
    );
}

#[test]
fn the_caret_lands_inside_the_search_line_at_every_width_the_line_is_drawn_at() {
    // The one row here whose column is a fact about the terminal cursor rather
    // than about a span, so it is the one that can be put outside the frame.
    let providers = serving();
    let models = stocked();

    for columns in NARROWEST..=200 {
        let mut shelf = shelf(&providers, &models);
        shelf.query = "sonnet";
        shelf.typed = 6;

        let rows = shelf.within(columns, 30, Glyphs::Unicode);
        let caret = shelf.caret(columns, Glyphs::Unicode);

        assert!(caret.row < rows.len(), "{columns}: past the last row");
        assert!(
            said(&rows, caret.row).starts_with('│'),
            "{columns}: row {} is not the search line: {:?}",
            caret.row,
            said(&rows, caret.row)
        );
        assert!(
            (1..columns - 1).contains(&caret.column),
            "{columns}: caret at column {}",
            caret.column
        );
    }
}

#[test]
fn a_query_that_left_nothing_says_so_where_a_model_row_would_have_been() {
    // An empty pane that says nothing at all reads as a shelf that is still
    // loading. The reason it emptied is the query in the line above it.
    let providers = serving();

    let rows = shelf(&providers, &[]).within(100, 30, Glyphs::Unicode);

    assert!(
        said(&rows, BODY).contains("nothing matches"),
        "{:?}",
        said(&rows, BODY)
    );
}

#[test]
fn a_model_that_takes_no_rung_says_so_on_the_track() {
    // An empty track is indistinguishable from a track that failed to draw.
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    shelf.rungs = &[];

    let rows = shelf.within(100, 30, Glyphs::Unicode);

    assert!(
        said(&rows, 30 - 3).contains("no rung"),
        "{:?}",
        said(&rows, 30 - 3)
    );
}

#[test]
fn a_provider_the_query_emptied_keeps_its_row_and_a_dot_where_its_count_was() {
    // A row that is not there says nothing at all, and a zero reads as a
    // provider that serves nothing rather than one this query emptied.
    let providers = serving();
    let models = stocked();

    let rows = shelf(&providers, &models).within(100, 30, Glyphs::Unicode);

    let openai = rows
        .iter()
        .find(|row| row.text().contains("OpenAI"))
        .expect("the provider kept its row");
    assert!(openai.text().contains('·'), "{:?}", openai.text());
}

#[test]
fn the_row_in_force_says_so_in_words_and_not_by_colour() {
    // Nothing is ever said by colour alone: the row in force says so in words
    // at the end of it, because a terminal with no colour still has to say
    // which model the next turn will be asked with. It wins that column
    // outright -- a note about a rung is still true tomorrow, and this is the
    // one thing on the row that is only true now.
    let providers = serving();
    let models = stocked();

    let rows = shelf(&providers, &models).within(100, 30, Glyphs::Unicode);

    let force = rows
        .iter()
        .find(|row| row.text().contains("claude-sonnet-5") && row.text().contains('│'))
        .expect("the row in force is drawn");
    assert!(force.kinds().any(|slot| slot == Slot::DoneMark));
    assert!(force.text().contains("← now"), "{:?}", force.text());
}

#[test]
fn nothing_is_drawn_into_a_window_narrower_than_the_search_line_needs() {
    // Under this the line has no room left for what was typed into it, and a
    // search line that cannot show the query is the one row here that has to.
    let providers = serving();
    let models = stocked();
    let shelf = shelf(&providers, &models);

    // Written out rather than read from the constant, which would make this a
    // test of nothing: a sweep bounded by the number it is checking moves with
    // it and stays green wherever it is put.
    for columns in 0..24 {
        assert!(
            shelf.within(columns, 30, Glyphs::Unicode).is_empty(),
            "drew something into {columns} columns"
        );
    }

    assert_eq!(shelf.within(24, 30, Glyphs::Unicode).len(), 30);
}

#[test]
fn the_whole_shelf() {
    let providers = serving();
    let models = stocked();

    insta::assert_snapshot!(picture(
        &shelf(&providers, &models).within(100, 30, Glyphs::Unicode),
        100
    ));
}

#[test]
fn the_whole_shelf_folded() {
    let providers = serving();
    let models = stocked();

    insta::assert_snapshot!(picture(
        &shelf(&providers, &models).within(58, 24, Glyphs::Unicode),
        58
    ));
}

#[test]
fn a_provider_under_the_pointer_is_lit_across_the_whole_pane() {
    // The whole width of the pane, not the width of the name in it. A ground
    // that stopped where the text stopped would be a ragged edge down the
    // middle of the panel, and what the reader is being told is which row their
    // hand is on -- which is the row, all of it.
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY + 1, 5));

    let rows = shelf.within(100, 30, Glyphs::Unicode);
    let lit = rows
        .get(BODY + 1)
        .expect("the row the pointer is on")
        .clipped(SERVES + 1);

    assert_eq!(lit.columns(), SERVES + 1);
    let mut slots = lit.kinds();
    assert_eq!(slots.next(), Some(Slot::Quiet), "{:?}", lit.text());
    assert!(slots.all(|slot| slot == Slot::Pointed), "{:?}", lit.text());
}

#[test]
fn a_lit_row_leaves_the_pane_beside_it_dark() {
    // Two panes, and the pointer is in one of them. A hand resting on a model
    // says nothing about the provider drawn level with it, and lighting both
    // would be the panel claiming a pair that nothing has chosen.
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY, 40));

    let rows = shelf.within(100, 30, Glyphs::Unicode);
    let row = rows.get(BODY).expect("the row the pointer is on");
    let beside = row.clipped(SERVES + 1);

    assert!(
        beside.kinds().all(|slot| slot != Slot::Pointed),
        "{:?}",
        beside.text()
    );
    assert!(
        row.kinds().any(|slot| slot == Slot::Pointed),
        "{:?}",
        row.text()
    );
}

#[test]
fn the_rows_padding_a_pane_to_the_bottom_are_not_something_to_point_at() {
    // A pane is padded to the bottom of the window whatever it holds, and those
    // rows are the frame's inside rather than anything a reader can take. A
    // ground under one would offer a row that answers to nothing.
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY + providers.len(), 5));

    let rows = shelf.within(100, 30, Glyphs::Unicode);
    let row = rows
        .get(BODY + providers.len())
        .expect("a row padding the pane");

    assert!(
        row.kinds().all(|slot| slot != Slot::Pointed),
        "{:?}",
        row.text()
    );
}

#[test]
fn the_panes_are_framed_quietly_wherever_the_pointer_is() {
    // The frame says where one pane stops and the other starts, and it has
    // nothing else to say at any moment of the reading. The accent is spent on
    // the two things that do: the mark walking a pane, and the field a pointer
    // is resting in.
    let providers = serving();
    let models = stocked();

    for pointer in [
        None,
        Some((BODY, 5)),
        Some((BODY, 40)),
        Some((SEARCHING, 8)),
    ] {
        let mut shelf = shelf(&providers, &models);
        shelf.pointer = pointer;
        let rows = shelf.within(100, 30, Glyphs::Unicode);

        for at in [HEADER - 1, HEADER + 1, 25] {
            let row = rows.get(at).expect("a rule of the panes' frame");
            assert!(
                row.kinds().all(|slot| slot == Slot::Quiet),
                "{pointer:?} at {at}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn the_search_field_is_framed_quietly_until_the_pointer_is_in_it() {
    // The one frame that answers to the pointer, because it is the one a reader
    // reaches for with the mouse rather than the arrows: a field is a place to
    // click, and nothing else on the shelf is.
    let providers = serving();
    let models = stocked();

    for at in [SEARCHING - 1, SEARCHING, SEARCHING + 1] {
        let mut shelf = shelf(&providers, &models);
        let rows = shelf.within(100, 30, Glyphs::Unicode);
        let quiet = rows.get(at).expect("a row of the search frame");
        assert!(
            quiet.kinds().all(|slot| slot != Slot::Accent),
            "{at}: {:?}",
            quiet.text()
        );

        shelf.pointer = Some((at, 8));
        let rows = shelf.within(100, 30, Glyphs::Unicode);
        let row = rows.get(at).expect("a row of the search frame");
        assert!(
            row.kinds().any(|slot| slot == Slot::Accent),
            "{at}: {:?}",
            row.text()
        );
    }
}

#[test]
fn the_whole_shelf_under_the_pointer() {
    let providers = serving();
    let models = stocked();
    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY + 2, 40));

    insta::assert_snapshot!(picture(&shelf.within(100, 30, Glyphs::Unicode), 100));
}

#[test]
fn nothing_a_pane_says_about_itself_is_something_to_point_at() {
    // Three rows that are not models: the line saying a query matched nothing,
    // the count of what is scrolled past, and the blanks below either. A ground
    // under one of them offers a row that answers to nothing, which is a worse
    // lie than no ground at all.
    let providers = serving();
    let crowded = many();

    for (models, at) in [(Vec::new(), 0), (crowded.clone(), 0), (crowded, 20)] {
        let mut shelf = shelf(&providers, &models);
        shelf.pointer = Some((BODY + at, 40));
        let rows = shelf.within(100, 30, Glyphs::Unicode);
        let row = rows.get(BODY + at).expect("a row of the pane");

        // The one case that is a model: a crowded shelf's first row.
        let takeable = !models.is_empty() && at == 0;
        assert_eq!(
            row.kinds().any(|slot| slot == Slot::Pointed),
            takeable,
            "{at}: {:?}",
            row.text()
        );
    }
}

#[test]
fn the_folded_strip_of_providers_is_read_rather_than_pointed_at() {
    // Folded, the providers are a row of words rather than a pane of rows, and
    // a ground under one word would be a rectangle the width of that word --
    // which is the ragged edge the whole-column rule exists to refuse. It is
    // walked with tab and the arrows there, as it always was.
    let providers = serving();
    let models = stocked();

    for at in [HEADER, HEADER + 1] {
        let mut shelf = shelf(&providers, &models);
        shelf.pointer = Some((at, 5));
        let rows = shelf.within(58, 24, Glyphs::Unicode);
        let row = rows.get(at).expect("a row of the folded shelf");

        assert!(
            row.kinds().all(|slot| slot != Slot::Pointed),
            "{at}: {:?}",
            row.text()
        );
    }
}

#[test]
fn a_pointer_anywhere_at_any_width_draws_a_shelf_that_still_fits() {
    // The pointer is a place in a window that has just been resized, so every
    // width has to survive being pointed at everywhere -- including the widths
    // where a pane is too narrow to hold one and the columns it would have been
    // in belong to something else.
    let providers = serving();
    let models = many();

    // The widths either side of every decision the layout makes, rather than
    // every width: what a pointer changes is which slot a span takes and never
    // how wide it is, so what is being watched here is the arithmetic that
    // decides which pane a column is in -- and that only turns over at the
    // folding width and at the edges of the panes.
    let widths = [
        NARROWEST,
        NARROWEST + 1,
        FOLDS_AT - 1,
        FOLDS_AT,
        FOLDS_AT + 1,
        100,
        120,
    ];

    for columns in widths {
        for at in 0..34 {
            for column in [0, 1, SERVES, SERVES + 1, SERVES + 2, columns] {
                let mut shelf = shelf(&providers, &models);
                shelf.pointer = Some((at, column));
                for row in shelf.within(columns, 30, Glyphs::Unicode) {
                    assert!(
                        row.columns() <= columns,
                        "{columns} columns, pointed at {at}:{column}: {:?}",
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn what_is_resting_under_the_pointer_is_what_the_shelf_lit() {
    // One reading behind both answers. A caller that worked the place out a
    // second time would be a second reading of a picture that has been
    // narrowed, scrolled and folded — and the day the two disagree is the day a
    // click takes a row other than the one the reader can see lit.
    let providers = serving();
    let models = stocked();

    for (at, resting) in [
        (0, Resting::Model(0)),
        (2, Resting::Model(2)),
        (3, Resting::Model(3)),
    ] {
        let mut shelf = shelf(&providers, &models);
        shelf.pointer = Some((BODY + at, 40));

        assert_eq!(shelf.resting(100, 30), Some(resting));
        let rows = shelf.within(100, 30, Glyphs::Unicode);
        assert!(
            rows.get(BODY + at)
                .expect("the row the pointer is on")
                .kinds()
                .any(|slot| slot == Slot::Pointed)
        );
    }

    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY + 2, 5));
    assert_eq!(shelf.resting(100, 30), Some(Resting::Provider(2)));
}

#[test]
fn a_pane_that_has_scrolled_rests_on_the_model_rather_than_the_row() {
    // The fourth row of a pane showing its ninth model. What a caller can take
    // is counted into what it handed over, because the rows are this shelf's
    // and it is the only party that knows how far they have moved.
    let providers = serving();
    let models = many();
    let mut shelf = shelf(&providers, &models);
    shelf.model = models.len() - 1;

    let room = CHROME + 5;
    shelf.pointer = Some((BODY, 40));
    let first = shelf
        .resting(100, room)
        .expect("the top row of a scrolled pane");

    assert_ne!(first, Resting::Model(0), "the pane has scrolled past it");
    let Resting::Model(first) = first else {
        panic!("a model, not a provider")
    };

    // And the row beside the row is the model beside the model.
    shelf.pointer = Some((BODY + 1, 40));
    assert_eq!(shelf.resting(100, room), Some(Resting::Model(first + 1)));
}

#[test]
fn nothing_rests_on_a_row_that_is_not_a_row_to_take() {
    // Exactly the rows that carry no ground, said the other way round: a
    // caller asking what a click would take is owed nothing wherever a reader
    // was shown nothing.
    let providers = serving();
    let models = stocked();

    for pointer in [
        None,
        Some((0, 5)),
        Some((SEARCHING, 8)),
        Some((HEADER, 5)),
        Some((BODY + models.len(), 40)),
        Some((BODY + providers.len(), 5)),
        Some((27, 20)),
    ] {
        let mut shelf = shelf(&providers, &models);
        shelf.pointer = pointer;
        assert_eq!(shelf.resting(100, 30), None, "{pointer:?}");
    }

    // And a shelf with no room to stand rests on nothing at any place at all.
    let mut shelf = shelf(&providers, &models);
    shelf.pointer = Some((BODY, 40));
    assert_eq!(shelf.resting(100, CHROME), None);
    assert_eq!(shelf.resting(NARROWEST - 1, 30), None);
}
