//! crucible, run the way a person runs it, and asserted on the screen it drew.
//!
//! Every other test in this tree sees rows. That is the right shape for a
//! component — a row is what it returns — but it means nothing above them ever
//! sees the arithmetic that turns rows into a screen: how far a frame rewinds,
//! where the cursor parks, how tall the live region is allowed to be. crucible
//! writes escape sequences inline into the terminal's own scrollback and owns no
//! cell buffer, so that arithmetic is the renderer, and it has been wrong in a
//! shipped release: the live tail was bounded by the whole window while rows
//! stood under it, so once an answer filled the screen every frame erased rows
//! the terminal had already taken, and the box was eaten away from the top as
//! the answer got longer. No component test could have caught it. This one can.
//!
//! So: a real pseudo terminal, the real binary, real keystrokes, and a
//! [`screen`] that understands exactly what the renderer promises to write and
//! reports anything else by name. Each case snapshots the picture, and every
//! frame on the way to it is checked for the two guarantees crucible makes
//! about the screen rather than about a row — that no row is ever wider than
//! the terminal, and that nothing ever moves the cursor above the top of it.
//!
//! Linux only, and the reason is in [`window`]: the child needs this pty as its
//! controlling terminal or it reads the developer's window size instead of this
//! one, and claiming a controlling terminal without `unsafe` means handing the
//! job to `setsid --ctty`, which is util-linux. Nothing about the renderer is
//! Linux-specific; the way to watch it is.
#![cfg(target_os = "linux")]
// This is test code all the way down, but the exemption `clippy.toml` grants
// tests reaches only the body of a `#[test]` function, and the pty, the child
// process and the settle loop all live in helpers beside them. A failure here
// is meant to stop the case that met it, and a `Result` threaded back to every
// case would say less about what went wrong than the message on the `expect`.
#![allow(clippy::expect_used, clippy::panic)]

mod screen;
mod vendor;
mod window;

use vendor::Vendor;
use window::Window;

/// A line long enough to need more rows than the box is allowed to grow to.
///
/// Built rather than written out so the arithmetic is visible: the box shows
/// `(rows / 2) - 3` rows of a line that wraps at `columns - 6`, and this is
/// comfortably past that at the size the case uses.
fn overlong() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(12)
}

/// An answer with more rows in it than the whole window has.
///
/// Past the window rather than merely past the box, and the difference is the
/// test: the tail may hold what the footing leaves it, so an answer that fits on
/// screen never reaches the bound and never exercises the arithmetic that was
/// wrong. This is long enough that rows leave the tail for the terminal's
/// scrollback while the box goes on standing under them.
fn taller_than_the_window() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(40)
}

#[test]
fn a_first_run_with_nothing_set_up_draws_the_welcome_the_warning_and_the_box() {
    // Nothing typed: this is the whole of what crucible puts on screen before
    // it asks for anything, and the first frame is the one with no committed
    // row above it to rewind over.
    //
    // It is also the screen a first run meets, and the reason this case is the
    // gate on that: nothing holds a key here, so the warning is the one naming
    // both `/login` and `/model` — and what stands under it is the prompt box,
    // not a panel. A run that opened on a panel instead would put the reader in
    // front of a question before it had said where they were.
    let window = Window::open("welcome", 80, 24);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_same_session_in_a_narrow_window_is_the_same_screen_at_its_width() {
    // Half the width, where the welcome drops to one column and the wordmark
    // has to go. Two widths rather than one because a row that fits at eighty
    // and overflows at forty is the failure this is watching for.
    let window = Window::open("narrow", 40, 24);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_typed_line_that_reaches_the_edge_wraps_and_grows_the_box() {
    // The box grows on the keystroke that fills a row, which pushes everything
    // above it up the screen. The rewind on the next frame has to stop at the
    // top of the taller box and not at the top of the shorter one it replaced.
    let mut window = Window::open("wrapped", 80, 24);

    window.types(&"the quick brown fox jumps over the lazy dog. ".repeat(3));

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_line_past_what_the_box_has_room_for_scrolls_inside_it() {
    // Past the ceiling the box stops growing and the line scrolls under its top
    // edge. A short window, because the ceiling is worked out from the height:
    // a box that went on growing here would be taller than the screen and could
    // not be taken back at all.
    let mut window = Window::open("scrolled", 80, 16);

    window.types(&overlong());

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_slash_opens_the_command_list_above_the_box() {
    // A live region that is suddenly much taller than the box on its own, drawn
    // over rows the terminal has already been given.
    let mut window = Window::open("commands", 80, 24);

    window.types("/");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_is_committed_above_a_box_that_is_still_where_it_was() {
    // The first case here that takes a turn. What it watches is the handover:
    // the answer becomes rows the terminal owns, and the box and its footing
    // are drawn again underneath, once, at the bottom of the screen.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::answering("answered", 80, 24, &vendor);

    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_longer_than_the_window_leaves_the_box_whole_at_the_foot_of_it() {
    // The defect this whole file was written for, and the one no component test
    // could reach: the live tail was bounded by the window rather than by the
    // rows left under it, so an answer this long ate the box from the top as it
    // grew. A short window, because what decides it is how much of the screen
    // the answer fills.
    let vendor = Vendor::answering(&taller_than_the_window());
    let mut window = Window::answering("answered-long", 80, 16, &vendor);

    window.types("say something long\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_key_given_to_login_is_what_the_turn_after_it_is_sent_with() {
    // The first minute on a machine that has never logged in, end to end: the
    // welcome says there is nothing to ask, `/login` takes a key into a box that
    // does not echo it, and the next thing typed is answered. Nothing restarts
    // in between, which is the whole of what this case is here to prove — and
    // only a real run can, since what a key has to reach is a socket.
    //
    // Named on the line, which is what skips both panels: this is the way in for
    // somebody who already knows whose key they hold.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::keyless("logged-in", 80, 24, &vendor);

    window.types("/login anthropic\r");
    window.types("not-a-key-and-nothing-reads-it\r");
    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_two_panels_of_login_reach_a_turn_without_a_provider_being_named() {
    // `/login` with nothing after it, walked the whole way: how crucible is paid
    // for, then whose console the key is from, then the key. Two panels because
    // they are two questions, and this is the case that proves the second one is
    // reached and that what comes off the pair signs the turn after it.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::keyless("login-walked", 80, 24, &vendor);

    window.types("/login\r");
    // Down twice to the console account, past the two plans; Enter opens the
    // panel asking whose console, and Enter again takes the one under the mark.
    window.types("\x1b[B\x1b[B\r");
    window.types("\r");
    window.types("not-a-key-and-nothing-reads-it\r");
    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn logging_in_writes_down_which_provider_to_ask_from_the_next_run_on() {
    // The half of `/login` a picture cannot show. A key says a provider can be
    // reached and never which to ask, so a run that logged in and wrote only
    // the key would meet the same question at the next launch — with one more
    // key in hand to be undecided between.
    //
    // Here rather than beside the command's own tests because the walk needs a
    // keyboard: the key goes into a box that does not echo it, and a loop
    // driven off a pipe has nothing to type into one. It is the one case in
    // this suite with no snapshot, because what it asserts is a file.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::keyless("login-written", 80, 24, &vendor);

    window.types("/login\r");
    window.types("\x1b[B\x1b[B\r");
    window.types("\r");
    window.types("not-a-key-and-nothing-reads-it\r");

    let held = std::fs::read_to_string(window.home().join("config.json"))
        .expect("the configuration file this case was given");

    assert!(held.contains("\"provider\": \"anthropic\""), "{held}");
    // What was already in it, byte for byte: the file is spliced rather than
    // rewritten, and a `/login` that dropped the address this case reaches its
    // vendor at would have taken the rest of the suite with it.
    assert!(held.contains("\"baseUrl\""), "{held}");
}

#[test]
fn a_plan_crucible_cannot_sign_with_yet_says_so_rather_than_asking_for_a_key() {
    // The honest half of a panel that draws more than it can do. A plan row is
    // there so somebody holding one learns crucible knows about it — and the
    // moment they choose it, it has to say that it is unbuilt rather than open
    // a key box, which is the thing a plan holder has no key for.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::keyless("unbuilt", 80, 24, &vendor);

    window.types("/login\r");
    window.types("\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_effort_ladder_stands_in_a_window_a_panel_of_the_same_five_would_fill() {
    // Five one-word rungs, drawn the way a choice between waiting and thinking
    // reads: one track, one mark, both ends named. The panel this replaced spent
    // two rows on each rung under a three-row paragraph and came to twenty-four
    // — the whole window, for five words. This is the case that says it fits,
    // and it is a whole-screen one rather than a component one because fitting
    // is a fact about the window and the box underneath, not about the rows.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Window::keyless("effort-ladder", 80, 24, &vendor);

    window.types("/login anthropic\r");
    window.types("not-a-key-and-nothing-reads-it\r");
    window.types("/effort\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_panel_that_was_left_writes_one_line_and_not_the_list_under_it() {
    // Escape is an answer, and the answer is "the screen I had". `/login` left
    // this way used to fall through to the list of every provider and the
    // variable each reads from — three rows into the scrollback, for somebody
    // who had just said they did not want to be asked. One line is what it owes:
    // enough that the record says the question was asked, and no more.
    let mut window = Window::open("login-left", 80, 24);

    window.types("/login\r");
    window.types("\x1b");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_window_that_narrows_mid_session_redraws_what_is_live_at_the_new_width() {
    // The size changes under a line that was laid out for the old one. What was
    // committed stays where the terminal put it; what is live is drawn again,
    // and the row count it rewinds over is the count from before the resize.
    let mut window = Window::open("resized", 80, 24);

    window.types("the quick brown fox jumps over the lazy dog");
    window.resize(52, 20);

    insta::assert_snapshot!(window.picture());
}
