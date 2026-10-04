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
