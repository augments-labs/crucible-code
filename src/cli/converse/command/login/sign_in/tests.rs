//! What the sign-in panel draws, keeps and sends, and that it fits.

use crucible_app::client::agents::SaidLine;

use crate::cli::client::HEARD_LIMIT;

use super::*;

/// A value standing for a line that is hidden: its letters are in nothing the
/// panel says itself.
const HIDDEN: &str = "zqxjkvbw";

/// A panel on a queue of `room` lines, what it draws having been told `said`,
/// and the far end of that queue, as the door has it.
fn signing_in_on(said: &[&str], room: usize) -> (SigningIn, mpsc::Receiver<Typing>) {
    let mut heard = Heard::default();
    for line in said {
        heard.take(SaidLine::new(line));
    }
    let (to, from) = mpsc::channel(room);
    let panel = SigningIn::new(
        "Log in",
        "signing \"goose\" in",
        Arc::new(Mutex::new(heard)),
        to,
        Cancel::new(),
    );
    (panel, from)
}

/// A panel told `said`, for the sweep: the queue's far end is dropped, which
/// the rows it draws do not depend on.
fn signing_in(said: &[&str]) -> SigningIn {
    signing_in_on(said, 1).0
}

/// Everything the panel draws at 80 columns, as one text.
fn drawn(panel: &SigningIn, room: usize) -> String {
    panel
        .laid(80, room, Glyphs::Unicode)
        .0
        .iter()
        .map(Row::text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Types `text` into `panel` a key at a time.
fn typed(panel: &mut SigningIn, text: &str) {
    for character in text.chars() {
        panel.pressed(Pressed::Key(Key::Char(character)));
    }
}

#[test]
fn a_terminal_is_lent_only_where_every_stream_is_one() {
    let terminal = Renderer::new(crucible_tui::Recording::new(80, 24));
    for stdin in [false, true] {
        for stdout in [false, true] {
            for stderr in [false, true] {
                assert_eq!(
                    handover_if(&terminal, [stdin, stdout, stderr]).is_some(),
                    (stdin, stdout, stderr) == (true, true, true),
                    "stdin {stdin}, stdout {stdout}, stderr {stderr}"
                );
            }
        }
    }
    let redirected = Renderer::new(crucible_tui::Recording::redirected(80, 24));
    assert!(handover_if(&redirected, [true; 3]).is_none());
    let native = Renderer::drawing(
        crucible_tui::Recording::new(80, 24),
        crucible_tui::ScreenMode::Native,
    );
    assert!(handover_if(&native, [true; 3]).is_some());
}

#[test]
fn every_state_of_the_sign_in_panel_fits_every_window_it_is_given() {
    // The panel's own fit sweep, since it is laid out here and not among the
    // components the crate's sweep walks: with nothing said, a few lines, a
    // line wider than any window, wide characters, more lines than are kept,
    // a line typed shown and hidden, a refusal standing, and being stopped,
    // each at every width and room it decides anything at, in either glyph set.
    let wide = "x".repeat(4096);
    let many: Vec<String> = (0..400).map(|at| format!("line {at}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let mut typing = signing_in(&["your code: "]);
    typed(&mut typing, &"y".repeat(300));
    let mut hiding = signing_in(&["password: "]);
    hiding.pressed(Pressed::Tab);
    typed(&mut hiding, &"y".repeat(300));
    let mut refusing = signing_in(&[]);
    refusing.pressed(Pressed::Pasted("one\ntwo".into()));
    let mut stopping = signing_in(&["waiting"]);
    stopping.pressed(Pressed::Escape);
    let states = [
        ("nothing", signing_in(&[])),
        ("few", signing_in(&["hello", "your code: "])),
        ("wide", signing_in(&[&wide])),
        ("cjk", signing_in(&["認証コードを入力してください"])),
        ("many", signing_in(&many)),
        ("typing", typing),
        ("hiding", hiding),
        ("refusing", refusing),
        ("stopping", stopping),
    ];
    for (state, panel) in &states {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for width in 1..=200 {
                for room in 0..=24 {
                    let (laid, caret) = panel.laid(width, room, glyphs);
                    let at = format!("{state} {glyphs:?} {width}x{room}");
                    assert!(laid.len() <= room, "{at}: {} rows", laid.len());
                    for row in &laid {
                        assert!(row.columns() <= width, "{at}: {:?}", row.text());
                    }
                    if let Some(caret) = caret {
                        assert!(caret.row < laid.len(), "{at}: caret row {}", caret.row);
                        assert!(caret.column < width, "{at}: caret column {}", caret.column);
                    }
                }
            }
        }
    }
}

#[test]
fn what_the_panel_keeps_is_bounded_oldest_first_and_it_says_so() {
    let many: Vec<String> = (0..HEARD_LIMIT + 10)
        .map(|at| format!("said {at}"))
        .collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let (panel, _from) = signing_in_on(&many, 1);
    let picture = drawn(&panel, 24);
    assert!(picture.contains("10 earlier lines dropped"), "{picture}");
    let newest = format!("said {}", HEARD_LIMIT + 9);
    assert!(picture.contains(&newest), "{picture}");
    let heard = panel.heard.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(heard.lines().count(), HEARD_LIMIT);
    assert_eq!(heard.dropped(), 10);
}

#[test]
fn what_cannot_be_seen_is_said_and_the_newest_stays_on_the_screen() {
    let (panel, _from) = signing_in_on(&["first", "second", "third", "fourth"], 1);
    let picture = drawn(&panel, 8);
    assert!(picture.contains("3 earlier rows"), "{picture}");
    assert!(picture.contains("fourth"), "{picture}");
    assert!(!picture.contains("first"), "{picture}");
}

#[test]
fn a_line_the_door_cut_says_it_was_cut() {
    let mut heard = Heard::default();
    heard.take(SaidLine::clipped("the start of it", 5));
    let (to, _from) = mpsc::channel(1);
    let panel = SigningIn::new(
        "Log in",
        "signing in",
        Arc::new(Mutex::new(heard)),
        to,
        Cancel::new(),
    );
    assert!(drawn(&panel, 24).contains(CUT));
}

#[test]
fn a_line_the_command_wrote_cannot_reach_the_terminal_as_it_was_written() {
    let (panel, _from) = signing_in_on(&["before\u{1b}[2J\u{1b}[1;1H\u{7}\tafter"], 1);
    let picture = drawn(&panel, 24);
    assert!(!picture.contains('\u{1b}'), "{picture:?}");
    assert!(!picture.contains('\u{7}'), "{picture:?}");
    assert!(picture.contains("before\\u{1b}[2J"), "{picture}");
}

#[test]
fn a_line_is_shown_as_it_is_typed_and_sent_when_entered() {
    let (mut panel, mut from) = signing_in_on(&[], 1);
    typed(&mut panel, "yes");
    assert!(drawn(&panel, 24).contains("› yes"));
    panel.pressed(Pressed::Key(Key::Enter));
    assert!(matches!(from.try_recv(), Ok(Typing::Line(line)) if line == "yes"));
    assert!(!drawn(&panel, 24).contains("› yes"));
}

#[test]
fn an_empty_line_is_an_answer() {
    let (mut panel, mut from) = signing_in_on(&[], 1);
    panel.pressed(Pressed::Key(Key::Enter));
    assert!(matches!(from.try_recv(), Ok(Typing::Line(line)) if line.is_empty()));
}

#[test]
fn a_hidden_line_is_drawn_as_marks_sent_as_a_secret_and_kept_no_longer() {
    let (mut panel, mut from) = signing_in_on(&[], 1);
    panel.pressed(Pressed::Tab);
    typed(&mut panel, HIDDEN);
    let picture = drawn(&panel, 24);
    assert!(!picture.contains(HIDDEN), "the hidden line was drawn");
    assert!(picture.contains("hidden › ••••••••"), "{picture}");
    panel.pressed(Pressed::Key(Key::Enter));
    let sent = from.try_recv();
    let Ok(Typing::Secret(sent)) = sent else {
        panic!("the hidden line was not sent as a secret");
    };
    assert!(sent.applied() == HIDDEN, "the secret sent was not the line");
    assert!(panel.held.is_empty(), "the hidden line was kept");
    assert!(!panel.hidden, "the next line stayed hidden");
    typed(&mut panel, "shown");
    assert!(drawn(&panel, 24).contains("› shown"));
}

#[test]
fn an_empty_secret_is_nothing_to_send() {
    let (mut panel, mut from) = signing_in_on(&[], 1);
    panel.pressed(Pressed::Tab);
    assert!(matches!(
        panel.pressed(Pressed::Key(Key::Enter)),
        Moved::Still
    ));
    assert!(from.try_recv().is_err());
    assert!(panel.hidden);
}

#[test]
fn what_was_typed_does_not_cross_from_one_box_to_the_other() {
    let (mut panel, _from) = signing_in_on(&[], 1);
    typed(&mut panel, "half a line");
    panel.pressed(Pressed::Tab);
    assert!(panel.held.is_empty());
    typed(&mut panel, HIDDEN);
    panel.pressed(Pressed::Tab);
    assert!(panel.held.is_empty());
    assert!(
        !drawn(&panel, 24).contains(HIDDEN),
        "the hidden line was drawn"
    );
}

#[test]
fn a_paste_holding_a_line_ending_is_refused_and_nothing_is_sent_or_held() {
    for ending in ["\n", "\r", "\r\n"] {
        for hidden in [false, true] {
            let (mut panel, mut from) = signing_in_on(&[], 1);
            if hidden {
                panel.pressed(Pressed::Tab);
            }
            panel.pressed(Pressed::Pasted(format!("one{ending}two").into()));
            assert!(from.try_recv().is_err(), "the paste was sent");
            assert!(panel.held.is_empty(), "the paste was held");
            let picture = drawn(&panel, 24);
            assert!(picture.contains(PASTE_REFUSED), "{picture}");
            assert!(!picture.contains("two"), "{picture}");
        }
    }
}

#[test]
fn a_paste_on_one_line_is_held_without_its_control_characters() {
    let (mut panel, _from) = signing_in_on(&[], 1);
    panel.pressed(Pressed::Pasted("a\u{1b}b".into()));
    assert_eq!(panel.held, "ab");
}

#[test]
fn a_line_longer_than_the_box_is_refused_whole() {
    let (mut panel, _from) = signing_in_on(&[], 1);
    panel.pressed(Pressed::Pasted("y".repeat(LINE_BYTES + 1).into()));
    assert!(panel.held.is_empty());
    assert!(drawn(&panel, 24).contains(TOO_LONG));
}

#[test]
fn a_line_with_nowhere_to_go_is_kept_and_said_to_be_waiting() {
    let (mut panel, mut from) = signing_in_on(&[], 1);
    typed(&mut panel, "first");
    panel.pressed(Pressed::Key(Key::Enter));
    typed(&mut panel, "second");
    panel.pressed(Pressed::Key(Key::Enter));
    assert_eq!(panel.held, "second");
    assert!(drawn(&panel, 24).contains(NOT_READING));
    assert!(matches!(from.try_recv(), Ok(Typing::Line(line)) if line == "first"));
    panel.pressed(Pressed::Key(Key::Enter));
    assert!(matches!(from.try_recv(), Ok(Typing::Line(line)) if line == "second"));
}

#[test]
fn escape_raises_the_cancel_and_the_panel_stands_taking_no_more() {
    for key in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        let (mut panel, mut from) = signing_in_on(&["waiting"], 1);
        assert!(!panel.cancel.requested());
        assert!(!matches!(panel.pressed(key), Moved::Took | Moved::Left));
        assert!(panel.cancel.requested());
        assert!(drawn(&panel, 24).contains("stopping the command"));
        assert!(drawn(&panel, 24).contains("waiting"));
        typed(&mut panel, "late");
        panel.pressed(Pressed::Key(Key::Enter));
        assert!(panel.held.is_empty());
        assert!(
            from.try_recv().is_err(),
            "a line went to a command stopping"
        );
    }
}

#[test]
fn a_panel_stood_again_over_a_command_being_stopped_says_so() {
    let (to, _from) = mpsc::channel(1);
    let cancel = Cancel::new();
    cancel.request();
    let panel = SigningIn::new(
        "Log in",
        "signing in",
        Arc::new(Mutex::new(Heard::default())),
        to,
        cancel,
    );
    assert!(panel.stopping);
}

#[test]
fn the_footer_names_the_keys_at_any_width_they_fit() {
    let panel = signing_in(&[]);
    let at = |width| panel.footer(width, Glyphs::Unicode);
    assert!(at(80).contains("tab hides the next line"), "{}", at(80));
    assert!(at(30).contains("tab hides"), "{}", at(30));
    assert!(at(30).contains("esc stops"), "{}", at(30));
}
