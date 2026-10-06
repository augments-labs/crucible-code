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
    // count is taken over the scrollback and the window together. The box
    // stands at the foot: the region grew while the turn ran, and keeps the
    // height it grew to once the turn is over.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::native("native-answered", 80, 24, &vendor);

    window.types_until("what is 2+2\r", "Two plus two is four.");

    window.assert_never_alternate();
    let read = everything(&window);
    assert_eq!(read.matches("what is 2+2").count(), 1, "{read}");
    assert_eq!(read.matches("Two plus two is four.").count(), 1, "{read}");
    let picture = window.picture();
    assert!(
        drawn(&picture)
            .last()
            .is_some_and(|row| row.contains("ask mode on")),
        "the box does not stand at the foot:\n{picture}"
    );
    insta::assert_snapshot!(picture);
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
///
/// Both open at 80 by 24. A case that wants a narrower window resizes the
/// second once it is up: one opened at sixteen columns never draws the line
/// that says it has settled.
fn ended_then_relaunched(case: &str, vendor: &Vendor) -> (Watched, Watched, String) {
    let mut first = Watched::native(case, 80, 24, vendor);
    first.types_until("say something\r", "The first thing");
    first.ends_on("TERM");
    let id = recorded_id(&first);

    let second = Watched::native(case, 80, 24, vendor);
    (first, second, id)
}

#[test]
fn resume_writes_one_divider_and_no_second_card_in_native_mode() {
    let vendor = Vendor::answering("The first thing this session said.");
    let (_first, mut window, id) = ended_then_relaunched("native-resume", &vendor);
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
    let (_first, mut window, id) = ended_then_relaunched("native-resume-narrow", &vendor);
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

    window.types_until("/clear\r", &format!("── {STARTED} ─"));

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

    window.types_until("/clear\r", &format!("-- {STARTED} -"));

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

/// The rows of a picture or a scrollback, without the header a picture opens
/// with.
fn drawn(picture: &str) -> Vec<&str> {
    picture
        .lines()
        .filter(|line| line.starts_with('|'))
        .collect()
}

/// How many rows at the top of `opened` are the transcript's: the longest run
/// of them that stood, in that order, somewhere in `before`, and under it the
/// row echoing a command `keys` sent, which the transcript wrote as the
/// command went and keeps once the panel is gone.
///
/// A panel's first row is a rule, a title or a list entry, none of which the
/// transcript wrote, so the run ends where the panel begins.
fn transcript_rows(before: &str, opened: &str, keys: &str) -> usize {
    let before = drawn(before);
    let opened = drawn(opened);
    let kept = (1..=opened.len())
        .rev()
        .find(|&count| {
            opened
                .get(..count)
                .is_some_and(|top| before.windows(count).any(|run| run == top))
        })
        .unwrap_or(0);
    kept + usize::from(echoed(&opened, kept, keys))
}

/// Whether the row of `opened` at `at` echoes the command `keys` sent: `› `
/// and the command, which only keys ending in Enter leave behind.
fn echoed(opened: &[&str], at: usize, keys: &str) -> bool {
    let Some(command) = keys.strip_suffix('\r') else {
        return false;
    };
    opened
        .get(at)
        .is_some_and(|row| row.trim_end_matches(['|', ' ']) == format!("|› {command}"))
}

/// The rows of the window the box takes: the `window left` row the prompt
/// draws over its top border, the box, its status row and whatever the
/// region leaves under them. The blank row above them is the list's, not the
/// box's.
fn box_rows(picture: &str) -> usize {
    let rows = drawn(picture);
    rows.iter()
        .position(|row| row.contains("% window left"))
        .map_or(0, |at| rows.len() - at)
}

/// What a panel opened over a full window took, read against the window as it
/// was before it opened.
struct Stood {
    /// The window's rows that are still the transcript's.
    transcript: usize,
    /// The window's rows under them: the panel, or the list and the box.
    under: usize,
    /// How many rows the panel pushed into the scrollback: what the terminal
    /// was handed while the panel opened, less the one the command's echo
    /// took for itself.
    pushed: usize,
    /// The rows of the panel: every row of the window the transcript never
    /// wrote, blank ones aside.
    rows: Vec<String>,
}

/// Reads [`Stood`] off `window` with a panel `keys` opened.
///
/// Fails, showing the window, where the panel took every row of it: no row of
/// the transcript is left to stand the echo under, and the picture is what
/// says which panel grew.
fn stood(window: &Watched, before: &str, back_before: &str, keys: &str) -> Stood {
    let opened = window.picture();
    let transcript = transcript_rows(before, &opened, keys);
    let Some(last) = transcript.checked_sub(1) else {
        panic!(
            "the panel took all {} rows of the window and left none of the transcript's:\n{opened}",
            drawn(&opened).len()
        );
    };
    let echo = usize::from(echoed(&drawn(&opened), last, keys));
    let written: Vec<&str> = drawn(before)
        .into_iter()
        .chain(drawn(back_before))
        .collect();
    let rows = drawn(&opened)
        .into_iter()
        .skip(transcript)
        .filter(|row| !row.trim_matches(|c| c == '|' || c == ' ').is_empty())
        .filter(|row| !written.contains(row))
        .map(str::to_owned)
        .collect();
    Stood {
        transcript,
        under: drawn(&opened).len() - transcript,
        pushed: drawn(&window.scrollback()).len() - drawn(back_before).len() - echo,
        rows,
    }
}

/// Fails where a row the panel drew can still be scrolled back to.
fn left_nothing(window: &Watched, panel: &[String]) {
    let back = window.scrollback();
    for row in panel {
        assert!(
            !back.contains(row.as_str()),
            "{row:?} was left behind:\n{back}"
        );
    }
}

/// A native window at 80x24 filled by an answer taller than it.
fn filled(case: &str, vendor: &Vendor) -> Watched {
    let mut window = Watched::native(case, 80, 24, vendor);
    window.types_until("say something\r", crate::ANSWER_END);
    assert!(
        !window.scrollback().is_empty(),
        "the window is not full:\n{}",
        window.picture()
    );
    window
}

/// Opens a panel with `keys` over a full window, checks it against the cap
/// from `mockups.md` §3, closes it with Esc and checks nothing of it stayed.
///
/// `height` is what the panel may take: half the window, or its smallest
/// drawable height where that is more.
fn capped(window: &mut Watched, keys: &str, opens: &str, height: usize) -> String {
    let before = window.picture();
    let back_before = window.scrollback();

    window.types_until(keys, opens);
    let opened = window.picture();
    let Stood {
        transcript,
        under,
        pushed,
        rows,
    } = stood(window, &before, &back_before, keys);
    assert!(
        under <= height,
        "{under} rows under the transcript:\n{opened}"
    );
    assert!(
        transcript >= 24 - height,
        "{transcript} rows of the transcript:\n{opened}"
    );
    assert!(
        pushed <= under.saturating_sub(box_rows(&before)),
        "{pushed} rows pushed into the scrollback:\n{opened}"
    );

    window.types_until("\x1b", "ask mode on");
    window.assert_never_alternate();
    left_nothing(window, &rows);
    opened
}

#[test]
fn settings_takes_at_most_half_the_window_in_native_mode() {
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-settings", &vendor);
    let opened = capped(&mut window, "/settings\r", "esc to close", 12);
    insta::assert_snapshot!(opened);
}

#[test]
fn settings_scrolls_inside_its_room_in_native_mode() {
    // Capped at half of 24 rows, the Config tab has room for one of its rows,
    // so a step down past it has to move the list inside the panel: the panel
    // growing to show the next row would push the transcript off the top.
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-settings-scrolled", &vendor);
    let before = window.picture();
    let back_before = window.scrollback();
    window.types_until("/settings\r", "esc to close");
    let opened = window.picture();
    assert!(opened.contains("› Theme"), "{opened}");
    assert!(opened.contains("↓ 17 more"), "{opened}");

    window.types_until("\x1b[B", "› Syntax theme");
    let scrolled = window.picture();
    let Stood {
        transcript,
        under,
        pushed,
        rows,
    } = stood(&window, &before, &back_before, "/settings\r");
    assert!(
        under <= 12,
        "{under} rows under the transcript:\n{scrolled}"
    );
    assert!(
        transcript >= 12,
        "{transcript} rows of the transcript:\n{scrolled}"
    );
    assert!(
        pushed <= under.saturating_sub(box_rows(&before)),
        "{pushed} rows pushed into the scrollback:\n{scrolled}"
    );
    assert!(!scrolled.contains("› Theme"), "{scrolled}");
    assert!(scrolled.contains("↓ 16 more"), "{scrolled}");

    window.types_until("\x1b", "ask mode on");
    window.assert_never_alternate();
    left_nothing(&window, &rows);
    insta::assert_snapshot!(scrolled);
}

#[test]
fn theme_takes_at_most_half_the_window_in_native_mode() {
    // The theme picture's smallest drawable height at 80 columns is 19: the
    // specimen stands under the list there, so the panel stands at that height
    // rather than at the cap.
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-theme", &vendor);
    let opened = capped(&mut window, "/theme\r", "Theme", 19);
    insta::assert_snapshot!(opened);
}

#[test]
fn model_takes_at_most_half_the_window_in_native_mode() {
    // The shelf's smallest drawable height is 15, so it stands at that height
    // rather than at the cap.
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-model", &vendor);
    let opened = capped(&mut window, "/model\r", "Search", 15);
    insta::assert_snapshot!(opened);
}

#[test]
fn resume_takes_at_most_half_the_window_in_native_mode() {
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut first = Watched::native("native-cap-resume", 80, 24, &vendor);
    first.types_until("say something\r", crate::ANSWER_END);
    first.ends_on("TERM");

    let mut window = filled("native-cap-resume", &vendor);
    let opened = capped(&mut window, "/resume\r", "a session, or a branch", 12);
    insta::assert_snapshot!(opened);
}

#[test]
fn ctrl_o_takes_at_most_half_the_window_in_native_mode() {
    let vendor = Vendor::calling_batches(&crate::reading_three(), "All three are read.");
    let mut window = Watched::native("native-cap-results", 80, 24, &vendor);
    crate::three_files(&window);
    window.types_until("read all three\r", "All three are read.");
    assert!(!window.scrollback().is_empty(), "{}", window.picture());

    let opened = capped(&mut window, "\x0f", "result 1 of 3", 12);
    assert!(opened.contains("pgup pgdn to see more"), "{opened}");
    insta::assert_snapshot!(opened);
}

#[test]
fn slash_list_takes_at_most_half_the_window_in_native_mode() {
    // The list stands above the box with one blank row between them, so the
    // cap is on the three together, and the list is closed by taking the `/`
    // back rather than with Esc.
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-slash", &vendor);
    let before = window.picture();
    let back_before = window.scrollback();

    window.types("/");
    let opened = window.picture();
    let Stood {
        transcript,
        under,
        pushed,
        rows,
    } = stood(&window, &before, &back_before, "/");
    let list = under - box_rows(&opened) - 1;
    assert!(list > 0, "{opened}");
    assert!(under <= 12, "{under} rows under the transcript:\n{opened}");
    assert!(
        transcript >= 12,
        "{transcript} rows of the transcript:\n{opened}"
    );
    assert!(
        pushed <= list + 1,
        "{pushed} rows pushed into the scrollback:\n{opened}"
    );
    assert!(opened.contains(" more"), "{opened}");

    window.types("\x7f");
    assert!(
        !window.picture().contains("/settings"),
        "{}",
        window.picture()
    );
    window.assert_never_alternate();
    left_nothing(&window, &rows);
    insta::assert_snapshot!(opened);
}

/// Fails where a row of the `/settings` panel, at either width it was drawn
/// at, can be scrolled back to: its tab row, its footer, or its rule, the one
/// row made of nothing but `─`.
fn settings_left_nothing(window: &Watched) {
    let back = window.scrollback();
    for text in ["esc to close", "Config", "Usage"] {
        assert!(!back.contains(text), "{text:?} was left behind:\n{back}");
    }
    let rule = drawn(&back).into_iter().find(|row| {
        let inside = row.trim_matches(|c| c == '|' || c == ' ');
        !inside.is_empty() && inside.chars().all(|c| c == '─')
    });
    assert!(rule.is_none(), "{rule:?} was left behind:\n{back}");
}

#[test]
fn a_resize_with_a_panel_open_leaves_no_panel_row_in_the_scrollback_in_native_mode() {
    let vendor = Vendor::answering(&crate::a_long_answer());
    let mut window = filled("native-cap-resized", &vendor);
    let before = window.picture();
    let back_before = window.scrollback();
    window.types_until("/settings\r", "esc to close");
    let rows = stood(&window, &before, &back_before, "/settings\r").rows;

    // Shorter: the cap would be 8, and the panel's smallest drawable height is
    // 12, so it stands at 12 of the 16 rows.
    window.resize(80, 16);
    let shorter = window.picture();
    assert!(shorter.contains("esc to close"), "{shorter}");
    assert_eq!(
        16 - transcript_rows(&before, &shorter, "/settings\r"),
        12,
        "{shorter}"
    );
    left_nothing(&window, &rows);
    insta::assert_snapshot!("a_resize_with_a_panel_open_80x16", shorter);

    // Narrower and tall again: the rows fold at the new width and the panel is
    // capped as it was at 80x24, except that its footer folds into two rows at
    // 60 columns, so its smallest drawable height there is 13 and it stands at
    // that. The region's redraw leaves the rows under its new foot blank, so
    // the panel is measured from its rule to its last written row.
    window.resize(60, 24);
    let narrower = window.picture();
    assert!(narrower.contains("esc to close"), "{narrower}");
    let rows = drawn(&narrower);
    let rule = rows
        .iter()
        .position(|row| row.starts_with(&format!("|{}", "─".repeat(60))))
        .unwrap_or(0);
    let footer = rows
        .iter()
        .rposition(|row| row.contains("esc to close"))
        .unwrap_or(0);
    assert!(footer + 1 - rule <= 13, "{narrower}");
    settings_left_nothing(&window);
    insta::assert_snapshot!("a_resize_with_a_panel_open_60x24", narrower);

    window.types_until("\x1b", "ask mode on");
    window.assert_never_alternate();
    settings_left_nothing(&window);
}

#[test]
fn ctrl_b_opens_the_running_list_during_a_turn_in_native_mode() {
    // Native mode asks for no mouse, so the click on the count that opens the
    // `Still running` list in fullscreen is no door here, and before Ctrl+B
    // opened it a running turn left the list out of reach. The model
    // backgrounds its own command, so the turn has nothing for the key to
    // background, and the list is what it opens.
    //
    // A native launch carries no rule for the call, so the call is asked
    // about and allowed once.
    let vendor = crate::a_turn_still_running();
    let mut window = Watched::native("native-ctrl-b-mid-turn", 60, 24, &vendor);
    window.types_until("start it\r", "Do you want to proceed?");
    window.types_and_catches("\r", crate::HELD_LAST_WORD);

    // Caught by its heading: the spinner of a turn still running keeps the
    // screen beating.
    window.types_and_catches("\x02", "Still running");

    window.assert_never_alternate();
    insta::assert_snapshot!(window.picture());
}

// A command's reply in native mode. Fullscreen hangs it under the line that
// asked, with the mark on its first row and its text column kept on the rest,
// and a native reply is held to that. A row goes out to the scrollback once and
// cannot be marked after, so a reply that leaves the region before its command
// ends has to leave it already marked.

/// A fullscreen window configured as [`Watched::native`] configures one, with
/// the scroll rail off, so its transcript is as wide as the native one.
fn fullscreen(case: &str, columns: u16, rows: u16, vendor: &Vendor) -> Watched {
    let document = format!(
        "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
         \"output\": {{\"scrollRail\": false}},\n  \
         \"providers\": {{\n    \"anthropic\": {{\n      \
         \"model\": \"claude-test-1\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
        vendor.address()
    );
    Watched::configured(case, columns, rows, &document, true)
}

/// The reply to `command` in `read`: the rows under the one that echoes it,
/// through the first that says `last`, without the edges or trailing blanks.
fn reply(read: &str, command: &str, last: &str) -> Vec<String> {
    let echo = format!("› {command}");
    let rows: Vec<&str> = drawn(read)
        .into_iter()
        .map(|row| row.trim_matches('|').trim_end())
        .collect();
    let at = rows
        .iter()
        .position(|row| *row == echo)
        .unwrap_or_else(|| panic!("no row reads {echo}:\n{read}"));
    let mut said = Vec::new();
    for row in rows.into_iter().skip(at + 1) {
        said.push(row.to_owned());
        if row.contains(last) {
            return said;
        }
    }
    panic!("no row under {echo} says {last:?}:\n{read}")
}

/// Opens `/login` and leaves it with Esc at its first panel.
fn leaves_login(window: &mut Watched) {
    window.types_until("/login\r", "esc to cancel");
    window.types_until("\x1b", "signed in");
}

#[test]
fn a_one_row_reply_carries_its_mark_in_native_mode() {
    // The reply follows the panel's key wait, which seals what is above it.
    let vendor = Vendor::answering("Done.");
    let mut window = Watched::native("native-reply-one-row", 80, 24, &vendor);
    leaves_login(&mut window);
    let native = reply(&everything(&window), "/login", "signed in");
    let mut other = fullscreen("native-reply-one-row-fullscreen", 80, 24, &vendor);
    leaves_login(&mut other);
    let full = reply(&other.picture(), "/login", "signed in");

    window.assert_never_alternate();
    let all = everything(&window);
    assert_eq!(all.matches("signed in").count(), 1, "{all}");
    assert_eq!(native, ["⎿ cancelled, nothing signed in"]);
    assert_eq!(native, full);
    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_reply_that_wraps_carries_its_mark_and_indent_in_native_mode() {
    // Thirty cells with its mark, so two rows at 24 columns: the mark on the
    // first and the text column it opens kept on the second.
    let vendor = Vendor::answering("Done.");
    let mut window = Watched::native("native-reply-wrapped", 24, 24, &vendor);
    leaves_login(&mut window);
    let native = reply(&everything(&window), "/login", "signed in");
    let mut other = fullscreen("native-reply-wrapped-fullscreen", 24, 24, &vendor);
    leaves_login(&mut other);
    let full = reply(&other.picture(), "/login", "signed in");

    window.assert_never_alternate();
    let all = everything(&window);
    assert_eq!(all.matches("signed in").count(), 1, "{all}");
    assert_eq!(native, ["⎿ cancelled, nothing", "  signed in"]);
    assert_eq!(native, full);
    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_reply_taller_than_the_window_carries_its_mark_on_its_first_row_in_native_mode() {
    // Five rows leave `/usage` no room to stand, so it prints, and most of what
    // it prints has gone out to the scrollback before the command ends. The
    // fullscreen window is made tall again afterwards, which lays out nothing
    // again, so that the whole of its reply is on screen to read.
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::native("native-reply-tall", 80, 24, &vendor);
    window.resize(80, 5);
    window.types_until("/usage\r", "limits not reported");
    let native = reply(
        &crate::timeless(&everything(&window)),
        "/usage",
        "limits not reported",
    );

    let mut other = fullscreen("native-reply-tall-fullscreen", 80, 24, &vendor);
    other.resize(80, 5);
    other.types_until("/usage\r", "limits not reported");
    other.resize(80, 24);
    let full = reply(
        &crate::timeless(&other.picture()),
        "/usage",
        "limits not reported",
    );

    window.assert_never_alternate();
    assert!(native.len() > 5, "{native:#?}");
    assert!(
        native.first().is_some_and(|row| row.starts_with("⎿ Usage")),
        "{native:#?}"
    );
    assert_eq!(native, full);
    insta::assert_snapshot!(crate::timeless(&everything(&window)));
}

/// The answer the window is narrowed under, one word of every row of it
/// standing in no other row, so that a count of the word is a count of the row.
const NARROWED_UNDER: &str = "Row alfa opens an answer the window is narrowed under \
    while it is still arriving. Row bravo follows it, as wide as the row above it, \
    so that both fold again at sixty. Row charlie is the third, worded so that no \
    word of it stands in any other row. Row delta is the fourth, and the window may \
    well be narrow by the time it lands. Row foxtrot is the fifth, long enough that \
    the resize has somewhere to fall. Row golf closes the first answer, and the box \
    is drawn again under it at sixty.";

/// The answer the window is widened under again, worded the same way.
const WIDENED_UNDER: &str = "Row hotel opens the second answer, the one the window \
    is widened under again. Row india follows it at sixty columns, folded by the \
    renderer and not by the terminal. Row juliett is the third of the second, and \
    reads the same at either width. Row kilo is the fourth, and the window may well \
    be wide again by the time it lands. Row lima is the fifth, long enough that this \
    resize has somewhere to fall too. Row mike closes the second answer, and the box \
    is drawn again under it at eighty.";

/// The one word of each row of [`NARROWED_UNDER`] and [`WIDENED_UNDER`].
const ROW_WORDS: [&str; 12] = [
    "alfa", "bravo", "charlie", "delta", "foxtrot", "golf", "hotel", "india", "juliett", "kilo",
    "lima", "mike",
];

#[test]
fn a_resize_while_an_answer_streams_leaves_no_ghost_row_in_native_mode() {
    // A resize is taken while the answer is still arriving: the window is
    // narrowed as soon as the first row of one answer is on screen, and
    // widened again as soon as the first row of the next is. The harness
    // settles a resize on a quiet screen, and a streaming answer is never
    // quiet until it ends, so the two resizes fall in two answers rather than
    // one. Each answer is short enough that, rewrapped at sixty columns, the
    // region still fits the window: what is read is the renderer's own
    // reckoning of how far back its region now starts, not what a terminal
    // does with a region taller than the window.
    let vendor = Vendor::answering_each(&[NARROWED_UNDER, WIDENED_UNDER]);
    let mut window = Watched::native("native-resized-mid-answer", 80, 24, &vendor);

    window.types_and_catches("say the first\r", "alfa");
    window.resize(60, 24);
    window.types_and_catches("say the second\r", "hotel");
    window.resize(80, 24);

    window.assert_never_alternate();
    let all = everything(&window);
    for word in ROW_WORDS {
        assert_eq!(all.matches(word).count(), 1, "{word:?} in\n{all}");
    }
    insta::assert_snapshot!(window.picture());
}

/// Seventy-eight cells: one row at eighty columns, and two once folded at forty.
const SEVENTY_EIGHT: &str =
    "This row is seventy-eight cells wide: one row at eighty and two rows at forty.";

#[test]
fn a_row_sealed_after_narrowing_is_folded_not_clipped_in_native_mode() {
    // The answer is whole and the turn is held open behind it, so the row is
    // still in the region when the window narrows. The resize settles only
    // once the turn ends, which is what seals the row: it goes out after the
    // narrowing, at the width the window has by then.
    let vendor = Vendor::holding(SEVENTY_EIGHT);
    let mut window = Watched::native("native-sealed-after-narrowing", 80, 24, &vendor);
    window.types_and_catches("say it\r", "forty.");
    window.resize(40, 24);

    window.assert_never_alternate();
    let all = everything(&window);
    let said: Vec<String> = reply(&all, "say it", "forty.")
        .into_iter()
        .filter(|row| !row.is_empty())
        .collect();
    assert!(
        said.iter().all(|row| row.chars().count() <= 40),
        "{said:#?}"
    );
    assert_eq!(said.join(" "), SEVENTY_EIGHT, "{all}");
    // A held turn that runs out of keep-alives ends as an answer ends, so
    // nothing on screen reports a stream cut short.
    assert!(!all.contains("the response ended before"), "{all}");
    insta::assert_snapshot!(window.picture());
}
