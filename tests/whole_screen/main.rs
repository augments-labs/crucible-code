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
mod watched;

use vendor::Vendor;
use watched::Watched;

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
    let window = Watched::open("welcome", 80, 24);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_remembered_provider_without_its_credential_still_opens_the_session() {
    // `/model` persists all three names, but the credential may belong only to
    // the shell that selected them. Once that variable is unset, the remembered
    // names become dormant setup rather than an error before the prompt exists.
    let window = Watched::unavailable("remembered-without-key", 80, 24);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_same_session_in_a_narrow_window_is_the_same_screen_at_its_width() {
    // Half the width, where the welcome drops to one column and the wordmark
    // has to go. Two widths rather than one because a row that fits at eighty
    // and overflows at forty is the failure this is watching for.
    let window = Watched::open("narrow", 40, 24);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_typed_line_that_reaches_the_edge_wraps_and_grows_the_box() {
    // The box grows on the keystroke that fills a row, which pushes everything
    // above it up the screen. The rewind on the next frame has to stop at the
    // top of the taller box and not at the top of the shorter one it replaced.
    let mut window = Watched::open("wrapped", 80, 24);

    window.types(&"the quick brown fox jumps over the lazy dog. ".repeat(3));

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_paste_of_several_lines_is_one_prompt_and_not_the_first_line_of_one() {
    // Bracketed, so the terminal says where the paste starts and ends and the
    // breaks inside it are structure. Unbracketed the same bytes are keystrokes,
    // and the first break is a Return: the prompt would be sent one line in with
    // the rest typed into whatever came up next.
    //
    // The breaks are carriage returns because that is what a terminal puts
    // inside the brackets — Return's own byte, not a newline.
    let mut window = Watched::open("pasted", 80, 24);

    window.types("\x1b[200~first line\rsecond line\rthird line\x1b[201~");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn every_way_a_terminal_spells_a_newline_grows_the_box_and_sends_nothing() {
    // Shift+Return in the encoding that has room for the modifier, then
    // Alt+Return, then Ctrl+J — which is a byte of its own and needs no
    // encoding asked for. Three rows added to the box and no turn taken.
    let mut window = Watched::open("newlines", 80, 24);

    window.types("one\x1b[13;2utwo\x1b\rthree\nfour");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_line_past_what_the_box_has_room_for_scrolls_inside_it() {
    // Past the ceiling the box stops growing and the line scrolls under its top
    // edge. A short window, because the ceiling is worked out from the height:
    // a box that went on growing here would be taller than the screen and could
    // not be taken back at all.
    let mut window = Watched::open("scrolled", 80, 16);

    window.types(&overlong());

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_slash_opens_the_command_list_above_the_box() {
    // A live region that is suddenly much taller than the box on its own, drawn
    // over rows the terminal has already been given.
    let mut window = Watched::open("commands", 80, 24);

    window.types("/");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_is_committed_above_a_box_that_is_still_where_it_was() {
    // The first case here that takes a turn. What it watches is the handover:
    // the answer becomes rows the terminal owns, and the box and its footing
    // are drawn again underneath, once. The blank rows below them are the three
    // the running turn had and has handed back: a live region that grew scrolled
    // the terminal to make room, and shrinking it again cannot scroll back.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::answering("answered", 80, 24, &vendor);

    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_longer_than_the_window_leaves_the_box_whole_under_it() {
    // The defect this whole file was written for, and the one no component test
    // could reach: the live tail was bounded by the window rather than by the
    // rows left under it, so an answer this long ate the box from the top as it
    // grew. A short window, because what decides it is how much of the screen
    // the answer fills.
    let vendor = Vendor::answering(&taller_than_the_window());
    let mut window = Watched::answering("answered-long", 80, 16, &vendor);

    window.types("say something long\r");

    insta::assert_snapshot!(window.picture());
}

/// The file the call in [`a_call_that_changed_a_file_is_drawn_with_the_change`]
/// is about, and the one line of it that moves.
const BEFORE: &str = "# trend data\nbudgets:\n  name: release budgets\n";

#[test]
fn a_call_that_changed_a_file_is_drawn_with_the_change() {
    // A whole turn with a call in it, which no case here could take before: the
    // model asks for a tool, a rule lets it through without a question, the
    // file changes, and what the reader is left with is the call, what it did,
    // and the lines it moved. Every part of that has a test beside the rows it
    // returns. This is the one place they are in the same picture, at a real
    // width, in the order somebody watching a turn meets them.
    let vendor = Vendor::calling(
        "edit",
        r##"{"path":"release.yml","find":"# trend data","replace":"# what stops a tag"}"##,
        "Renamed. Nothing else in the file moved.",
    );
    let mut window = Watched::allowing("tool-called", 80, 24, &vendor, "edit(*)");
    let file = window.workspace().join("release.yml");
    std::fs::write(&file, BEFORE).expect("a file for the call to change");

    window.types_until("rename that comment\r", "Nothing else in the file moved");

    // The screen says a line moved; this is whether it did. A picture of a
    // block drawn from a change nobody made would look exactly the same.
    let after = std::fs::read_to_string(&file).expect("the file the call changed");
    assert_eq!(after, BEFORE.replace("# trend data", "# what stops a tag"));

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_environment_authenticated_session_never_claims_logout_removed_it() {
    // The key belongs to the shell that launched this real process. `/logout`
    // can remove only Crucible's protected store, so this screen must name the
    // inherited source and leave the selected provider and model in force.
    let vendor = Vendor::answering("still authenticated");
    let mut window = Watched::answering("environment-logout", 80, 24, &vendor);

    window.types("/logout\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_key_given_to_login_is_what_the_turn_after_it_is_sent_with() {
    // The first minute on a machine that has never logged in, end to end: the
    // welcome says there is nothing to ask, `/login` takes a key into a box that
    // does not echo it, `/model` explicitly chooses what answers, and the next
    // thing typed is answered. Nothing restarts in between, which is the whole
    // of what this case is here to prove — and only a real run can, since what
    // a key has to reach is a socket.
    //
    // Named on the line, which is what skips the provider panel: this is the
    // way in for somebody who already knows whose key they hold.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::keyless("logged-in", 80, 24, &vendor);

    window.types("/login anthropic\r");
    window.types_until("not-a-key-and-nothing-reads-it\r", "logged in to anthropic");
    window.types("/model claude-test-1\r");
    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_provider_panel_reaches_a_turn_without_a_provider_being_named() {
    // `/login` with nothing after it, walked the whole way: past the two
    // account rows to the console account, then whose key this is, then the
    // key. `/model` remains a separate explicit choice; this proves what comes
    // off the login panel signs the next turn after that choice.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::keyless("login-walked", 80, 24, &vendor);

    window.types("/login\r");
    // Down twice to the console account, past the two plans; Enter opens the
    // panel asking whose console, and Enter again takes the one under the mark.
    window.types("\x1b[B\x1b[B\r");
    window.types("\r");
    window.types_until("not-a-key-and-nothing-reads-it\r", "logged in to anthropic");
    window.types("/model claude-test-1\r");
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
    let mut window = Watched::keyless("login-written", 80, 24, &vendor);

    window.types("/login\r");
    window.types("\x1b[B\x1b[B\r");
    window.types("\r");
    window.types_until("not-a-key-and-nothing-reads-it\r", "logged in to anthropic");

    let held = std::fs::read_to_string(window.home().join("config.json"))
        .expect("the configuration file this case was given");

    assert!(held.contains("\"provider\": \"anthropic\""), "{held}");
    // What was already in it, byte for byte: the file is spliced rather than
    // rewritten, and a `/login` that dropped the address this case reaches its
    // vendor at would have taken the rest of the suite with it.
    assert!(held.contains("\"baseUrl\""), "{held}");
}

#[test]
fn openai_account_login_offers_browser_and_device_code_methods() {
    // Provider first, method second. Browser sign-in is the ordinary local
    // path; device code stays visible for a remote terminal or another device.
    // This stops before either network flow starts and snapshots Crucible's
    // own inline panel rather than a provider's interface.
    let mut window = Watched::open("openai-login-methods", 80, 24);

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
    let mut window = Watched::keyless("effort-ladder", 80, 24, &vendor);

    window.types("/login anthropic\r");
    window.types_until("not-a-key-and-nothing-reads-it\r", "logged in to anthropic");
    window.types("/model claude-test-1\r");
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
    let mut window = Watched::open("login-left", 80, 24);

    window.types("/login\r");
    window.types("\x1b");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_window_that_narrows_mid_session_redraws_what_is_live_at_the_new_width() {
    // The size changes under a line that was laid out for the old one. What was
    // committed stays where the terminal put it; what is live is drawn again,
    // and the row count it rewinds over is the count from before the resize.
    let mut window = Watched::open("resized", 80, 24);

    window.types("the quick brown fox jumps over the lazy dog");
    window.resize(52, 20);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn picking_a_session_up_asks_before_carrying_it_whole() {
    // The panel in the binary rather than in a component test: it stands where
    // the box was, and what it says has to be readable against a real screen.
    let vendor = Vendor::answering("The first thing this session said.");
    let mut window = Watched::asking_on_resume("resume-asks", 80, 24, &vendor);

    window.types_until("say something\r", "The first thing");
    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume 1\r", "This session is large");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn choosing_notes_makes_room_and_says_what_it_took() {
    // Three turns, because a recap stands in place of what is behind the two
    // this keeps whole: a shorter session has no middle to replace, and the
    // choice would spend nothing.
    let vendor = Vendor::answering("Notes on everything that came before.");
    let mut window = Watched::compacting("resume-notes", 80, 24, &vendor);

    window.types_until("the first thing\r", "Notes on");
    window.types_until("the second thing\r", "ask mode on");
    window.types_until("the third thing\r", "ask mode on");
    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume 1\r", "This session is large");

    // Enter takes the first answer, which is the one that spends a request.
    window.types_until("\r", "compacted");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn escape_while_room_is_being_made_stops_it_and_replaces_nothing() {
    // Escape, with the notes half written. Two things are owed and neither used
    // to arrive: a line saying it stopped, and a session left exactly as it was
    // — half a memory of one, stood in place of the messages it was meant to
    // replace, loses the rest for good.
    //
    // The recap answer is long so the key lands while it is still arriving; the
    // vendor writes a word every few milliseconds, which is what makes the
    // middle of a stream somewhere a test can press a key.
    let notes = "notes to self about everything that has happened so far ".repeat(24);
    let vendor = Vendor::answering_each(&["one answer", "two answer", "three answer", &notes]);
    let mut window = Watched::compacting("compact-stopped", 80, 24, &vendor);

    window.types_until("the first thing\r", "one answer");
    window.types_until("the second thing\r", "two answer");
    window.types_until("the third thing\r", "three answer");
    window.types_and_catches("/compact\r", "compacting");
    window.types_until("\x1b", "! stopped");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_prompt_typed_while_room_is_being_made_is_sent_once_there_is_room() {
    // The box under a compaction is a box, not a picture of one: keys reach it
    // while the notes are being written, and the line finished there is sent
    // afterwards — against the session that has just been made smaller, which
    // is the whole reason it waits rather than going first.
    let notes = "notes to self about everything that has happened so far ".repeat(24);
    let vendor = Vendor::answering_each(&[
        "one answer",
        "two answer",
        "three answer",
        &notes,
        "the answer to what was queued",
    ]);
    let mut window = Watched::compacting("compact-typing", 80, 24, &vendor);

    window.types_until("the first thing\r", "one answer");
    window.types_until("the second thing\r", "two answer");
    window.types_until("the third thing\r", "three answer");

    // Typed into the box while the row above it still says what is happening.
    window.types_and_catches("/compact\r", "compacting");
    window.types_and_catches("what next", "what next");
    window.types_until("\r", "the answer to what was queued");

    insta::assert_snapshot!(window.picture());
}
