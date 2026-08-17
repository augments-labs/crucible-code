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
        statement: "This command needs your verdict.",
        question: "Do you want to proceed?",
        answers: &ANSWERS,
        marked: 0,
        footer: "esc to cancel · ctrl+e to explain",
    }
}

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
fn the_rhythm_is_three_blanks_and_one_row_for_each_simple_command() {
    // The whole of what this component is for: the permission engine already
    // knows a compound call is several commands, and each of them gets a row.
    let rows = asking(&[
        "cargo fmt --all",
        "cargo test --workspace --all-features",
        "git push origin HEAD",
    ])
    .within(WIDE, 24, Glyphs::Unicode);

    let mut want = vec![rule(Glyphs::Unicode.top())];
    want.extend(
        [
            "  Bash command",
            "",
            "    cargo fmt --all",
            "    cargo test --workspace --all-features",
            "    git push origin HEAD",
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
