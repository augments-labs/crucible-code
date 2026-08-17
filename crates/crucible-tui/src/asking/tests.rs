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
        explanation: &[],
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
