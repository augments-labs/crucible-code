use super::*;

/// How many rows the window in the two tests below has: more than everything
/// they draw, so every row drawn is a row of the window and none has scrolled
/// out of it.
const TALL: usize = 60;

/// A call line longer than a heading is given room for, so the row of the
/// result under it is the one that offers the whole of it.
fn long_heading() -> String {
    format!("Edit({}release.yml)", "deep/".repeat(40))
}

/// What one result came to, drawn under a call line too long for its heading.
struct Landing {
    /// How many lines the record held just before the result was drawn, which
    /// is the number of the first line the result wrote.
    actual: usize,
    /// How many lines of the record the result took.
    lines: usize,
    /// How many rows of the window the result took.
    rows: usize,
    /// The line of the record a click lands on, for every row of the window
    /// that names the key. Asked of the renderer, never of what is held.
    clicked: Vec<usize>,
    /// What was held for the key to open.
    kept: Kept,
}

/// Draws `output` the way a turn draws a result, on a terminal, with twenty
/// lines already above it so a row counted back from the end has somewhere to
/// land that is not the top of the record.
fn landing(output: ToolOutput) -> Landing {
    let style = Style::plain();
    let mut renderer = Renderer::new(Recording::new(WIDE, TALL));
    renderer.wears(style.palette());
    let mut kept = Kept::default();

    for line in 1..=20 {
        renderer
            .commit(&format!("earlier line {line}"))
            .expect("an earlier line to draw");
    }

    let call = call("edit", "{}");
    let heading = long_heading();
    kept.calling(call.id.clone(), heading.clone());
    returned(
        &mut renderer,
        &Called::new(heading, crucible_tools::Argument::Path),
        style,
    ).expect("the call line to draw");

    let actual = renderer.lines();
    let above = renderer.tail(TALL).len();

    came_back(
        &mut renderer,
        &mut kept,
        &call.id,
        Shown::live(output),
        style,
    )
    .expect("the result to draw");

    let lines = renderer.lines() - actual;
    let shown = renderer.tail(TALL);
    assert!(
        shown.len() < TALL,
        "the window is not tall enough for the test to read every row: {}",
        shown.len()
    );
    let rows = shown.len() - above;

    let clicked = shown
        .iter()
        .enumerate()
        .filter(|(_, row)| row.text().contains("ctrl+o to expand"))
        .map(|(at, row)| match renderer.aimed(at) {
            Some(crucible_tui::Aimed::Line(line)) => line,
            _ => panic!("window row {at} is no line of the record: {:?}", row.text()),
        })
        .collect();

    Landing {
        actual,
        lines,
        rows,
        clicked,
        kept,
    }
}

#[test]
fn a_result_that_changed_a_file_is_held_at_the_row_it_was_written_on() {
    // A click becomes a line of the record, and the line has to be the one the
    // result is held against, or the row that names the key opens nothing and
    // a row that names nothing opens the result.
    assert!(
        Renderer::new(Recording::new(WIDE, TALL)).is_terminal(),
        "the recording has to claim to be a terminal"
    );
    let landing = landing(ToolOutput::ok("changed release.yml, 3 replacements").showing(changed()));

    // What the test stands on, none of it read from what is held.
    assert_eq!(
        landing.clicked,
        vec![landing.actual],
        "one row names the key, and a click on it lands on the first line the result wrote"
    );
    assert_eq!(landing.rows, changed().lines().len() + 1, "a header and the change");

    let under = landing.rows - 1;
    let stored = landing.kept.newest().next().and_then(Whole::at);
    let earlier = landing.actual.saturating_sub(under);
    println!(
        "H2: actual line {}, record lines taken {}, window rows taken {}, rows of change under \
         the header {under}, stored line {stored:?}, offered({}) = {}, offered({earlier}) = {}",
        landing.actual,
        landing.lines,
        landing.rows,
        landing.actual,
        landing.kept.offered(landing.actual),
        landing.kept.offered(earlier),
    );

    assert!(
        landing.kept.offered(landing.actual),
        "the result was written on line {actual} of the record, taking {lines} line of it and \
         {rows} rows of the window, {under} of them the change under its header; a click on the \
         row that names the key lands on line {clicked:?}; it is held at {stored:?}, so \
         offered({actual}) is {at_actual} and offered({earlier}) is {at_earlier}, and {earlier} \
         is {under} lines before the line it was written on",
        actual = landing.actual,
        lines = landing.lines,
        rows = landing.rows,
        clicked = landing.clicked,
        at_actual = landing.kept.offered(landing.actual),
        at_earlier = landing.kept.offered(earlier),
    );
}

#[test]
fn a_result_that_changed_no_file_is_held_at_the_row_it_was_written_on() {
    // The same call line and the same terminal, and nothing under the row: what
    // the test above is compared with.
    let landing = landing(ToolOutput::ok("ran, and said one line"));

    assert_eq!(
        landing.clicked,
        vec![landing.actual],
        "one row names the key, and a click on it lands on the first line the result wrote"
    );
    assert_eq!(landing.rows, 1, "a result with no change under it is one row");
    assert_eq!(landing.lines, 1, "and one line of the record");

    let stored = landing.kept.newest().next().and_then(Whole::at);
    println!(
        "H2 control: actual line {}, record lines taken {}, window rows taken {}, stored line \
         {stored:?}, offered({}) = {}",
        landing.actual,
        landing.lines,
        landing.rows,
        landing.actual,
        landing.kept.offered(landing.actual),
    );

    assert!(
        landing.kept.offered(landing.actual),
        "the result was written on line {} of the record and is held at {stored:?}",
        landing.actual
    );
    assert_eq!(stored, Some(landing.actual));
}
