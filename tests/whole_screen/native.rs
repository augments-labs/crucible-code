//! Native mode, watched on a terminal that keeps a scrollback.
//!
//! Every case here starts crucible with `output.screen` set to `native`, so
//! it draws at the foot of the terminal's own buffer and lets finished rows
//! scroll off the top. What proves a case ran there is not its rows, which look
//! the same on either screen until something scrolls, but that the alternate
//! screen was never entered — so every case asks the window that outright.
//!
//! A reader of a native session has two things in front of them: the window,
//! and what they can scroll back to. A case that asserts a line went out once
//! reads both.

use crate::vendor::Vendor;
use crate::watched::Watched;

/// Everything a reader could scroll to: the scrollback, then the window.
fn everything(window: &Watched) -> String {
    let back = window.scrollback();
    let picture = window.picture();
    if back.is_empty() {
        picture
    } else {
        format!("{back}\n{picture}")
    }
}

#[test]
fn a_turn_is_written_once_and_the_box_stands_under_it_in_native_mode() {
    // The handover a native frame is built around: the prompt and the answer
    // go out to the terminal once each, and the box is drawn again under them
    // rather than at a row of its own. A terminal that was sent the turn twice
    // would show it twice somewhere a reader could scroll to, which is why the
    // count is taken over the scrollback and the window together.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::native("native-answered", 80, 24, &vendor);

    window.types_until("what is 2+2\r", "Two plus two is four.");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(read.matches("what is 2+2").count(), 1, "{read}");
    assert_eq!(read.matches("Two plus two is four.").count(), 1, "{read}");
    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_wheel_or_click_report_changes_nothing_in_native_mode() {
    // Native mode asks the terminal for no mouse mode, so a wheel notch scrolls
    // the terminal's own buffer and a click is the terminal's to select with.
    // A terminal that reports them anyway — one left in a mouse mode by another
    // program — is sent reports crucible has nothing to do with, and the screen
    // it drew is the screen it keeps.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::native("native-reported", 80, 24, &vendor);
    window.types_until("what is 2+2\r", "Two plus two is four.");
    let before = window.picture();

    // A notch up and a notch down over the answer, then a click on it.
    window.reports("\x1b[<64;5;3M");
    window.reports("\x1b[<65;5;3M");
    window.reports("\x1b[<0;5;3M\x1b[<0;5;3m");

    window.assert_never_alternate();
    assert_eq!(window.picture(), before);
    insta::assert_snapshot!(window.picture());
}

// `/resume` and `/clear` in native mode. Nothing written into the terminal's
// own buffer can be taken back, so the session that follows is put under the
// one before it with a single divider row between them, rather than under a
// second welcome card that would read as a second launch.

/// What a session picked up is told when the window can hold it.
const RESUMED: &str = "session resumed";

/// What a cleared session is told.
const STARTED: &str = "new session";

/// How many welcome cards a reader could scroll to.
///
/// A framed card's top edge names the release, and nothing else on screen
/// does; a bare one, drawn where a frame does not fit, is the wordmark alone on
/// its row. The rows are read as one run of cells, because a card drawn wider
/// than the window it is read in was rewrapped by the terminal, and its edge
/// with it.
fn cards(read: &str) -> usize {
    let cells = |row: &str| {
        row.strip_prefix('|')
            .and_then(|row| row.strip_suffix('|'))
            .map(str::to_owned)
    };
    let rows: Vec<String> = read.lines().filter_map(cells).collect();
    let framed = rows.concat().matches(" crucible v").count();
    let bare = rows.iter().filter(|row| row.trim() == "CRUCIBLE").count();
    framed + bare
}

/// How many warning lines a reader could scroll to.
fn warnings(read: &str) -> usize {
    read.lines()
        .filter(|row| row.trim_start_matches('|').starts_with("! "))
        .count()
}

/// How many divider rows a reader could scroll to, drawn in `rule`: two rule
/// cells and a space open one, which no other row does.
fn dividers(read: &str, rule: &str) -> usize {
    let opens = format!("|{rule}{rule} ");
    read.lines().filter(|row| row.starts_with(&opens)).count()
}

/// The session the run in `window` recorded, by the id `/resume` takes.
fn recorded_id(window: &Watched) -> String {
    fn found(directory: &std::path::Path) -> Option<String> {
        for entry in std::fs::read_dir(directory).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(id) = found(&path) {
                    return Some(id);
                }
            } else if path.extension().is_some_and(|kind| kind == "jsonl") {
                return path.file_stem()?.to_str().map(str::to_owned);
            }
        }
        None
    }
    found(&window.home().join("sessions")).expect("a session log the first run left")
}

/// The window with the session id typed into `/resume` written over.
///
/// A session's id is new on every run, so a picture holding it would be a
/// picture of one run. The box wraps a long line onto rows of its own, so a
/// row holding nothing but a piece of the id is written over as well.
fn without_id(window: &Watched, id: &str) -> String {
    window
        .picture()
        .lines()
        .map(|line| {
            let line = line.replace(id, &"#".repeat(id.len()));
            let cells = line.trim_matches('|').trim();
            if cells.len() >= 4 && id.contains(cells) {
                line.replace(cells, &"#".repeat(cells.len()))
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A native session that said something and then ended, and a second launch
/// in the same home that can resume it.
///
/// Two launches rather than a `/clear` before the `/resume`, so that the
/// second run's screen holds one launch and one resume and nothing else: a
/// clear would write a divider of its own. The second is given the first's
/// case name, which is what gives it the same home; the first is handed back
/// beside it so that its directory outlives the second run.
fn ended_then_relaunched(case: &str, columns: u16, vendor: &Vendor) -> (Watched, Watched, String) {
    let mut first = Watched::native(case, 80, 24, vendor);
    first.types_until("say something\r", "The first thing");
    first.ends_on("TERM");
    let id = recorded_id(&first);

    let second = Watched::native(case, columns.max(80), 24, vendor);
    (first, second, id)
}

#[test]
fn resume_writes_one_divider_and_no_second_card_in_native_mode() {
    let vendor = Vendor::answering("The first thing this session said.");
    let (_first, mut window, id) = ended_then_relaunched("native-resume", 80, &vendor);
    let launched = warnings(&everything(&window));

    window.types_until(&format!("/resume {id}\r"), "The first thing");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(cards(&read), 1, "{read}");
    assert_eq!(warnings(&read), launched, "{read}");
    assert_eq!(dividers(&read, "─"), 1, "{read}");
    assert!(read.contains(&format!("── {RESUMED} ─")), "{read}");
    insta::assert_snapshot!(without_id(&window, &id));
}

#[test]
fn resume_divider_is_clipped_at_16_columns_in_native_mode() {
    // Sixteen columns is narrower than the twenty-one the label and two rule
    // cells each side of it need, and than the seventeen of the other label.
    let vendor = Vendor::answering("The first thing this session said.");
    let (_first, mut window, id) = ended_then_relaunched("native-resume-narrow", 16, &vendor);
    let launched = warnings(&everything(&window));
    window.resize(16, 24);

    window.types_until(&format!("/resume {id}\r"), "first");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(cards(&read), 1, "{read}");
    assert_eq!(warnings(&read), launched, "{read}");
    assert_eq!(dividers(&read, "─"), 1, "{read}");
    assert!(read.contains("|── session r… ──|"), "{read}");
    insta::assert_snapshot!(without_id(&window, &id));
}

#[test]
fn clear_writes_one_divider_and_no_second_card_in_native_mode() {
    let vendor = Vendor::answering("Done.");
    let mut window = Watched::native("native-clear", 80, 24, &vendor);
    let launched = warnings(&everything(&window));
    window.types_until("say something\r", "Done.");

    window.types_until("/clear\r", "ask mode on");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(cards(&read), 1, "{read}");
    assert_eq!(warnings(&read), launched, "{read}");
    assert_eq!(dividers(&read, "─"), 1, "{read}");
    assert!(read.contains(&format!("── {STARTED} ─")), "{read}");
    insta::assert_snapshot!(window.picture());
}

#[test]
fn clear_of_a_silent_session_writes_no_divider_in_native_mode() {
    // Nothing was said, so there is no session to put a divider under: the
    // row saying so is all a clear writes.
    let vendor = Vendor::answering("Done.");
    let mut window = Watched::native("native-clear-silent", 80, 24, &vendor);
    let launched = warnings(&everything(&window));

    window.types_until("/clear\r", "nothing had been said");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(cards(&read), 1, "{read}");
    assert_eq!(warnings(&read), launched, "{read}");
    assert_eq!(dividers(&read, "─"), 0, "{read}");
    assert_eq!(read.matches("nothing had been said").count(), 1, "{read}");
    insta::assert_snapshot!(window.picture());
}

#[test]
fn clear_divider_is_drawn_in_ascii_glyphs_in_native_mode() {
    // `Watched::native` writes its own configuration, so the glyph set is
    // changed the way a reader changes it mid-session: through `/settings`,
    // which draws with the new set from that moment.
    let vendor = Vendor::answering("Done.");
    let mut window = Watched::native("native-clear-ascii", 80, 24, &vendor);
    let launched = warnings(&everything(&window));
    window.types_until("say something\r", "Done.");
    window.types_until("/settings\r", "esc to close");
    window.types_until("/glyphs", "Glyphs");
    window.types_until("\r\r", "ascii");
    window.types_until("\x1b", "> Theme");
    window.types_until("\x1b", "ask mode on");

    window.types_until("/clear\r", "ask mode on");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(cards(&read), 1, "{read}");
    assert_eq!(warnings(&read), launched, "{read}");
    assert_eq!(dividers(&read, "-"), 1, "{read}");
    assert_eq!(dividers(&read, "─"), 0, "{read}");
    assert!(read.contains(&format!("-- {STARTED} -")), "{read}");
    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_o_stands_at_the_foot_and_escape_leaves_no_row_of_it_in_native_mode() {
    // The view is the way to a cut result in native mode, where the transcript
    // is the terminal's own and nothing in it can be clicked open. It stands
    // at the foot with the same keys as fullscreen, and is taken back when it
    // closes: a row of it left in the scrollback would be a result written
    // into the record a second time, which is the thing standing it avoids.
    let vendor = Vendor::calling_batches(&crate::reading_three(), "All three are read.");
    let mut window = Watched::native("native-results", 80, 24, &vendor);
    crate::three_files(&window);
    window.types_until("read all three\r", "All three are read.");

    window.types_until("\x0f", "result 1 of 3");
    let opened = window.picture();
    assert_eq!(
        opened.lines().last().map(str::trim_end),
        Some("|esc to close · ↑↓ pgup pgdn to see more · ←→ result 1 of 3                      |"),
        "{opened}"
    );
    assert!(opened.contains("gamma line 02"), "{opened}");
    insta::assert_snapshot!(opened);

    window.types("\x1b[6~");
    window.types_until("\x1b[C", "result 2 of 3");
    assert!(
        window.picture().contains("beta line 01"),
        "{}",
        window.picture()
    );

    window.types_until("\x1b", "ask mode on");

    window.assert_never_alternate();
    let read = everything(&window);
    for row in [
        "pgup pgdn",
        "result 2 of 3",
        "gamma line 02",
        "beta line 02",
    ] {
        assert!(!read.contains(row), "{row:?} was left behind:\n{read}");
    }
    assert!(
        !read.contains(&"─".repeat(80)),
        "the view's rule was left behind:\n{read}"
    );
    for row in ["All three are read.", "1 gamma line 01 (+29 lines"] {
        assert_eq!(read.matches(row).count(), 1, "{row:?}:\n{read}");
    }
}
