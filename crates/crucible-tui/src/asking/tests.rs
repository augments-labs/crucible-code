use super::*;

/// The three answers, in the words the binary uses.
const ANSWERS: [&str; 3] = [
    "Yes, once",
    "Yes, and don't ask again this session",
    "No, and end the turn",
];

/// The window the rest of this crate's tests measure against.
const WIDE: usize = 80;

/// One bash call waiting for a verdict, with the mark on the first answer.
fn asking<'a>(payload: &'a [&'a str]) -> Question<'a> {
    Question {
        subject: "Bash command",
        payload,
        description: "",
        attribution: "",
        explanation: &[],
        from: 0,
        more: "↑↓ to see more",
        statement: "This command needs your verdict.",
        question: "Do you want to proceed?",
        answers: &ANSWERS,
        marked: 0,
        footer: "esc to cancel · ctrl+e to explain",
    }
}

/// The paragraphs a reader sees after asking for them, long enough that the
/// three of them cannot fit a short window.
const PARAGRAPHS: [&str; 3] = [
    "Runs the workspace's whole test suite with every feature turned on, which \
     builds each crate in the workspace and then runs its tests in turn.",
    "I want to know the suite is green before I touch the module the next change \
     is in, so that a failure afterwards belongs to that change.",
    "It reads and compiles; nothing outside the target directory is written.",
];

/// What each row says with its side edges taken off, so a test reads the
/// rhythm rather than the frame.
fn inside(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let said = row.text();
            said.strip_prefix('│')
                .and_then(|rest| rest.strip_suffix('│'))
                .map_or_else(|| said.clone(), |inner| inner.trim_end().to_owned())
        })
        .collect()
}

/// The panel in a window of `room` rows, with the prose opened `from` rows
/// down. Scrolling is read off two of these rather than off one mutated
/// question, so what a test compares is two frames a reader could have seen.
fn scrolled(question: Question<'_>, room: usize, from: usize) -> Vec<String> {
    let opened = Question { from, ..question };
    inside(&opened.within(WIDE, room, Glyphs::Unicode))
}

/// The top and bottom edges at `WIDE`, which no test writes out by hand.
fn rule(corners: (&str, &str)) -> String {
    let (left, right) = corners;
    format!("{left}{}{right}", "─".repeat(WIDE - AROUND))
}

#[test]
fn the_common_case_is_thirteen_rows_and_every_one_of_them_reaches_the_edge() {
    let rows = asking(&["cargo test --workspace --all-features"]).within(WIDE, 24, Glyphs::Unicode);

    assert_eq!(rows.len(), 13);

    // Every row but the footer is exactly the window: a row a column short
    // leaves a notch in the frame, and one a column long wraps where this
    // process did not predict.
    let framed = rows.iter().take(rows.len() - 1);
    for row in framed {
        assert_eq!(row.columns(), WIDE, "{:?}", row.text());
    }
}

#[test]
fn the_rhythm_is_three_blanks_and_a_command_that_keeps_its_operators() {
    // The whole of what this component is for. The permission engine knows a
    // compound call is several commands and reasons about each of them, and
    // this row is still the call as it was written — `&&` is the difference
    // between three commands and three commands *if the one before worked*,
    // and a panel that quietly listed them would have dropped it.
    let rows = asking(&[
        "cargo fmt --all && cargo test --workspace --all-features && git push origin HEAD",
    ])
    .within(WIDE, 24, Glyphs::Unicode);

    let mut want = vec![rule(Glyphs::Unicode.top())];
    want.extend(
        [
            "  Bash command",
            "",
            "    cargo fmt --all && cargo test --workspace --all-features && git push",
            "    origin HEAD",
            "",
            "  This command needs your verdict.",
            "",
            "  Do you want to proceed?",
            "  › 1. Yes, once",
            "    2. Yes, and don't ask again this session",
            "    3. No, and end the turn",
        ]
        .map(str::to_owned),
    );
    want.push(rule(Glyphs::Unicode.bottom()));
    want.push("  esc to cancel · ctrl+e to explain".to_owned());

    assert_eq!(inside(&rows), want);
}

#[test]
fn a_command_wider_than_the_window_folds_and_every_row_is_indented_alike() {
    // The content extreme, and the one the wording of this component turns on.
    // A fold that left the first row shorter than the rest would put a whole
    // command's worth of leading columns where the reader looks for one.
    let long = "find . -name '*.rs' -newer target/.rustc_info.json -print0 | xargs -0 sed -i \
                s/Budget/Ceiling/g && cargo test --workspace --all-features -- --nocapture";
    let rows = asking(&[long]).within(WIDE, 24, Glyphs::Unicode);
    let said = inside(&rows);

    // The block between the blank under the subject and the blank under the
    // payload, which is all of what a command was folded into.
    let payload: Vec<&String> = said
        .iter()
        .skip_while(|row| !row.is_empty())
        .skip(1)
        .take_while(|row| !row.is_empty())
        .collect();

    assert!(payload.len() > 1, "{said:?}");
    for row in &payload {
        assert!(
            row.starts_with("    ") && !row.starts_with("     "),
            "{row:?}"
        );
    }

    // Nothing was dropped on the way: every word of the command is still on
    // screen, which is the property clipping would break silently.
    let whole: Vec<&str> = payload
        .iter()
        .flat_map(|row| row.split_whitespace())
        .collect();
    assert_eq!(whole, long.split_whitespace().collect::<Vec<&str>>());
}

#[test]
fn the_marked_answer_is_marked_as_well_as_coloured() {
    // Colour alone would be the one thing on screen a terminal without it
    // could not report, and the row a key is about to act on is the last thing
    // to leave to a hue.
    let mut question = asking(&["rm -rf build"]);
    question.marked = 2;
    let rows = question.within(WIDE, 24, Glyphs::Unicode);
    let said = inside(&rows);

    assert!(
        said.iter().any(|row| row == "  › 3. No, and end the turn"),
        "{said:?}"
    );
    assert!(said.iter().any(|row| row == "    1. Yes, once"), "{said:?}");

    let marked = rows
        .iter()
        .find(|row| row.text().contains("3. No"))
        .expect("the marked answer");
    let spans: Vec<(Slot, &str)> = marked.spans().collect();

    assert!(spans.contains(&(Slot::Accent, "›")), "{spans:?}");
    assert!(
        spans
            .iter()
            .any(|(slot, text)| *slot == Slot::Strong && text.contains("No, and end")),
        "{spans:?}"
    );
}

#[test]
fn the_ladder_gives_up_the_footer_then_the_statement_then_the_blank() {
    let question = asking(&["cargo test --workspace --all-features"]);
    let rungs = [24, 12, 11, 9].map(|room| inside(&question.within(WIDE, room, Glyphs::Unicode)));

    let held = |rows: &[String], said: &str| rows.iter().any(|row| row == said);
    let footer = "  esc to cancel · ctrl+e to explain";
    let statement = "  This command needs your verdict.";

    let [full, quiet, plain, tight] = rungs;

    assert_eq!(full.len(), 13);
    assert!(held(&full, footer) && held(&full, statement));

    assert_eq!(quiet.len(), 12);
    assert!(!held(&quiet, footer) && held(&quiet, statement));

    assert_eq!(plain.len(), 10);
    assert!(!held(&plain, statement));
    assert_eq!(plain.iter().filter(|row| row.is_empty()).count(), 2);

    // The last rung: the blank under the subject goes, and the one under the
    // payload stays. That one is what keeps the payload a block.
    assert_eq!(tight.len(), 9);
    assert_eq!(tight.iter().filter(|row| row.is_empty()).count(), 1);
}

#[test]
fn what_is_about_to_run_survives_every_rung() {
    let question = asking(&["cargo fmt --all", "git push origin HEAD"]);

    for room in 9..=24 {
        let said = inside(&question.within(WIDE, room, Glyphs::Unicode));
        if said.is_empty() {
            continue;
        }
        assert!(
            said.iter().any(|row| row == "    cargo fmt --all"),
            "{room}: {said:?}"
        );
        assert!(
            said.iter().any(|row| row == "    git push origin HEAD"),
            "{room}: {said:?}"
        );
    }
}

#[test]
fn below_the_floor_there_is_no_panel_rather_than_a_short_one() {
    let question = asking(&["cargo test --workspace --all-features"]);

    // A row short of the floor, and a window with no room to fold a command
    // to. Both are the caller's cue to ask in the scrollback, where nothing
    // bounds the height of the question.
    assert!(question.within(WIDE, 8, Glyphs::Unicode).is_empty());
    assert!(
        question
            .within(AROUND + PAYLOAD, 24, Glyphs::Unicode)
            .is_empty()
    );
    assert!(question.within(0, 24, Glyphs::Unicode).is_empty());

    // And a call with nothing to show is not a question anyone can answer.
    assert!(asking(&[]).within(WIDE, 24, Glyphs::Unicode).is_empty());
}

#[test]
fn the_description_is_a_caption_on_the_command_rather_than_a_block_under_it() {
    // A blank is what separates one block from the next in this panel, so
    // withholding one is the whole of what makes this row a caption. Getting it
    // wrong reads as a second paragraph that happens to be short.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.description = "Run the suite before touching the module";
    let said = inside(&question.within(WIDE, 24, Glyphs::Unicode));

    let at = said
        .iter()
        .position(|row| row == "    cargo test --workspace --all-features")
        .expect("the command");

    assert_eq!(
        said.get(at + 1).map(String::as_str),
        Some("    Run the suite before touching the module"),
        "{said:?}"
    );
    assert_eq!(said.get(at + 2).map(String::as_str), Some(""), "{said:?}");
}

#[test]
fn a_description_is_one_row_however_much_the_model_wrote() {
    // Provider-controlled text on the render path. A caption that folded would
    // let whatever wrote it decide how tall this panel is, and the row it would
    // push off the bottom is one of the answers.
    let mut question = asking(&["cargo fmt --all"]);
    question.description = "Format every crate in the workspace, and then the \
                            binary beside them, so the gate has nothing to say \
                            about whitespace when it runs afterwards";
    let rows = question.within(WIDE, 24, Glyphs::Unicode);

    assert_eq!(rows.len(), 14);
    let said = inside(&rows);
    assert!(
        said.iter()
            .any(|row| row.starts_with("    Format every crate")),
        "{said:?}"
    );
}

#[test]
fn each_paragraph_of_an_explanation_opens_with_a_blank_and_indents_alike() {
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;
    let said = inside(&question.within(WIDE, 40, Glyphs::Unicode));

    // Three blanks of the panel's own, and one opening each paragraph.
    assert_eq!(said.iter().filter(|row| row.is_empty()).count(), 6);

    for paragraph in PARAGRAPHS {
        let opening = paragraph.split_whitespace().take(3).collect::<Vec<&str>>();
        assert!(
            said.iter().any(|row| {
                row.starts_with("    ")
                    && row.split_whitespace().take(3).eq(opening.iter().copied())
            }),
            "{paragraph:?} is not in {said:?}"
        );
    }
}

#[test]
fn the_prose_opens_with_the_row_saying_whose_words_the_rest_of_it_is() {
    // The paragraphs are the model's and the panel around them is not, and a
    // reader deciding whether to allow a command is exactly the reader who has
    // to know which is which. So the block opens with a row of the panel's own,
    // drawn quiet like the row that counts what is below it, and it scrolls
    // with the prose because it is the first thing the prose says.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;
    question.attribution = "bash's own account of this call:";

    let said = inside(&question.within(WIDE, 40, Glyphs::Unicode));
    let at = said
        .iter()
        .position(|row| row == "    bash's own account of this call:")
        .unwrap_or_else(|| panic!("{said:?}"));

    // Above it the blank that parts it from the command, below it the blank
    // that opens the first paragraph — so the row reads as a heading over the
    // block rather than as one more sentence in it.
    assert_eq!(said.get(at - 1).map(String::as_str), Some(""));
    assert_eq!(said.get(at + 1).map(String::as_str), Some(""));
    assert!(
        said.get(at + 2)
            .is_some_and(|row| row.starts_with("    Runs the workspace's")),
        "{said:?}"
    );
}

#[test]
fn an_explanation_too_tall_for_the_window_is_cut_and_says_how_much_it_cut() {
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;

    let whole = question.within(WIDE, 40, Glyphs::Unicode);
    let short = question.within(WIDE, 20, Glyphs::Unicode);
    assert_eq!(short.len(), 20);
    assert!(whole.len() > short.len());

    // The row that says so takes one the prose would have had, so the count is
    // one more than the two panels differ by.
    let said = inside(&short);
    let dropped = whole.len() - short.len() + 1;
    let told = format!("    · {dropped} more rows of explanation");
    assert!(said.contains(&told), "{said:?}");
}

#[test]
fn the_footer_offers_the_arrows_only_while_something_is_off_screen() {
    // A key named on the footer has to do something when it is pressed. The
    // component is the only party that knows whether the prose was cut, so it
    // is the one that decides whether the item is drawn.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;

    let whole = inside(&question.within(WIDE, 40, Glyphs::Unicode));
    let short = inside(&question.within(WIDE, 20, Glyphs::Unicode));

    let plain = "  esc to cancel · ctrl+e to explain".to_owned();
    let scrolling = "  esc to cancel · ctrl+e to explain · ↑↓ to see more".to_owned();

    assert!(whole.contains(&plain), "{whole:?}");
    assert!(short.contains(&scrolling), "{short:?}");
}

#[test]
fn scrolling_moves_the_window_over_the_prose_and_leaves_the_panel_alone() {
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;

    // Two rows shorter than the window the rest of these tests use, so that
    // moving two rows down the prose still leaves rows below it — the shape
    // this test is about is the window mid-way, not the window at the end.
    let top = scrolled(question, 18, 0);
    let down = scrolled(question, 18, 2);

    assert_eq!(top.len(), down.len());

    // The two rows the window moved past are gone and two later ones have
    // arrived, and the count falls by the two that were read.
    assert_ne!(top, down);
    assert!(
        top.contains(&"    · 4 more rows of explanation".to_owned()),
        "{top:?}"
    );
    assert!(
        down.contains(&"    · 2 more rows of explanation".to_owned()),
        "{down:?}"
    );
    let opening = "    Runs the workspace's whole test suite with every feature turned on, which";
    assert!(top.contains(&opening.to_owned()), "{top:?}");
    assert!(!down.contains(&opening.to_owned()), "{down:?}");

    for said in [&top, &down] {
        assert!(said.contains(&"  › 1. Yes, once".to_owned()), "{said:?}");
    }
}

#[test]
fn the_marker_counts_the_rows_below_the_window_and_goes_when_there_are_none() {
    // What a reader presses ↓ for is the prose under the window, so that is
    // what the row above the answers counts. A constant count would still read
    // *5 more rows* at the end of the prose, where ↓ does nothing: a true
    // statement about the whole explanation, read as a false one about the
    // direction being pressed.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;

    let counted = |from| {
        scrolled(question, 20, from)
            .into_iter()
            .find_map(|row| row.strip_prefix("    · ").map(str::to_owned))
    };

    assert_eq!(counted(0).as_deref(), Some("2 more rows of explanation"));

    // And at the end there is nothing below, so the row that counted it goes
    // back to the prose — which is why the panel is exactly as tall either way,
    // and why the last press of the arrow uncovers the two rows it promised.
    assert_eq!(counted(1), None);
    assert_eq!(counted(usize::MAX), None);

    let end = scrolled(question, 20, 1);
    assert_eq!(end.len(), scrolled(question, 20, 0).len());
    let closing = "    It reads and compiles; nothing outside the target directory is written.";
    assert!(
        end.contains(&closing.to_owned()),
        "the last row of the prose is on screen at the end: {end:?}"
    );
    assert!(!scrolled(question, 20, 0).contains(&closing.to_owned()));
}

#[test]
fn scrolling_past_the_end_of_the_prose_stops_at_the_end() {
    // What a held key does. Clamped here rather than by the caller, because the
    // caller cannot see how many rows the prose folded into.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;
    let last = scrolled(question, 20, 1);

    for from in [2, 3, 9, usize::MAX] {
        assert_eq!(scrolled(question, 20, from), last);
    }
}

#[test]
fn the_panel_says_where_the_window_stops_so_a_key_held_past_it_costs_nothing() {
    // Clamping what it is given is only half of it. The offset lives with
    // whatever reads the arrows, and a caller whose own copy ran on past the
    // end would owe a press coming back for every press that moved nothing.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.explanation = &PARAGRAPHS;

    let last = question.end(WIDE, 20, Glyphs::Unicode);

    // Which is exactly the offset past which the picture stops changing, and
    // the first one at which it has stopped. Asserted against the frames
    // rather than against a number worked out here, because agreeing with the
    // panel is the whole of what the answer is for.
    assert_eq!(
        scrolled(question, 20, last),
        scrolled(question, 20, last + 9)
    );
    assert_ne!(
        scrolled(question, 20, last),
        scrolled(question, 20, last - 1)
    );

    // Nowhere to scroll to answers zero, at both of the reasons there can be
    // none: prose that fitted, and a window with no room to open any in. So a
    // caller clamps with it unconditionally rather than asking first whether
    // there was anything to clamp.
    assert_eq!(question.end(WIDE, 40, Glyphs::Unicode), 0);
    assert_eq!(question.end(WIDE, 14, Glyphs::Unicode), 0);
    assert_eq!(
        asking(&["cargo test"]).end(WIDE, 24, Glyphs::Unicode),
        0,
        "a call that carried no explanation"
    );
}

#[test]
fn what_is_about_to_run_outlives_an_explanation_that_will_not_fit() {
    // The order the two are given up in is the argument for the whole feature:
    // the command is what a verdict is about, and the prose is about the
    // command. The reader gets the prose back by pressing the key again.
    let mut question = asking(&["cargo test --workspace --all-features"]);
    question.description = "Run the suite";
    question.explanation = &PARAGRAPHS;

    for room in 14..=40 {
        let said = inside(&question.within(WIDE, room, Glyphs::Unicode));
        assert!(
            said.iter()
                .any(|row| row == "    cargo test --workspace --all-features"),
            "{room}: {said:?}"
        );
        assert!(
            said.iter().any(|row| row == "    3. No, and end the turn"),
            "{room}: {said:?}"
        );
    }
}

#[test]
fn both_glyph_sets_draw_the_same_panel_at_the_same_width() {
    let question = asking(&["cargo fmt --all", "git push origin HEAD"]);
    let unicode = question.within(WIDE, 24, Glyphs::Unicode);
    let ascii = question.within(WIDE, 24, Glyphs::Ascii);

    assert_eq!(unicode.len(), ascii.len());
    for (one, other) in unicode.iter().zip(&ascii) {
        assert_eq!(one.columns(), other.columns(), "{:?}", other.text());
    }

    // The ascii set draws a frame and a mark of its own rather than dropping
    // either: a terminal whose font has no box drawing still has to be able to
    // see which answer a key is about to take.
    let said = ascii
        .iter()
        .map(Row::text)
        .collect::<Vec<String>>()
        .join("\n");
    assert!(said.contains("+---"), "{said}");
    assert!(said.contains("|  > 1. Yes, once"), "{said}");
}
