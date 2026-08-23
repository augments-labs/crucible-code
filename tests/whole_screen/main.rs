//! crucible, run the way a person runs it, and asserted on the screen it drew.
//!
//! Every other test in this tree sees rows. That is the right shape for a
//! component — a row is what it returns — but it means nothing above them ever
//! sees the arithmetic that turns rows into a screen: which band a frame writes
//! into, where the cursor parks, how tall the box is allowed to grow. crucible
//! shares the window out into bands and addresses every position in them
//! outright, so that arithmetic is the renderer, and it has been wrong in a
//! shipped release: what a turn was saying was bounded by the whole window
//! while rows stood under it, so once an answer filled the screen the box was
//! eaten away from the top as the answer got longer. No component test could
//! have caught it. This one can.
//!
//! So: a real pseudo terminal, the real binary, real keystrokes, and a
//! [`screen`] that understands exactly what the renderer promises to write and
//! reports anything else by name. Each case snapshots the picture, and every
//! frame on the way to it is checked for the guarantees crucible makes about
//! the screen rather than about a row — that no row is ever wider than the
//! terminal, and that no cell outside the window is ever addressed.
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
/// `(rows / 2) - 3` rows of a line that wraps at `columns - 6` — the reading
/// above the box takes its row out of the transcript's share, not the box's —
/// and this is comfortably past that at the size the case uses.
fn overlong() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(12)
}

/// An answer with more rows in it than the whole window has.
///
/// Past the window rather than merely past the box, and the difference is the
/// test: the transcript band holds what the bands under it leave it, so an
/// answer that fits on screen never reaches the bound and never exercises the
/// arithmetic that was wrong. This is long enough that rows go off the top of
/// that band while the box goes on standing under them.
fn taller_than_the_window() -> String {
    "the quick brown fox jumps over the lazy dog. ".repeat(40)
}

#[test]
fn a_first_run_with_nothing_set_up_draws_the_welcome_the_warning_and_the_box() {
    // Nothing typed: this is the whole of what crucible puts on screen before
    // it asks for anything, and the first frame is the one with nothing above
    // the box to share the window with.
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
    // The box grows on the keystroke that fills a row, which takes a row from
    // the transcript above it. The next frame has to lay the transcript out
    // against the band the taller box left and not the one the shorter one did.
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
    // Something standing that is suddenly much taller than the box on its own,
    // drawn over rows of the transcript that were on screen a frame ago.
    let mut window = Watched::open("commands", 80, 24);

    window.types("/");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_is_committed_above_a_box_that_is_still_where_it_was() {
    // The first case here that takes a turn. What it watches is the handover:
    // the answer joins the transcript, and the box is drawn again underneath in
    // the band it had before. The blank row between them is where a running
    // turn says what it is doing, empty now it is over — an empty band is still
    // a band, which is the whole of why the box did not move.
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::answering("answered", 80, 24, &vendor);

    window.types("what is 2+2\r");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn an_answer_longer_than_the_window_leaves_the_box_whole_under_it() {
    // The defect this whole file was written for, and the one no component test
    // could reach: what a turn was saying was bounded by the window rather than
    // by the rows left under it, so an answer this long ate the box from the
    // top as it grew. A short window, because what decides it is how much of
    // the screen the answer fills.
    let vendor = Vendor::answering(&taller_than_the_window());
    let mut window = Watched::answering("answered-long", 80, 16, &vendor);

    window.types("say something long\r");

    insta::assert_snapshot!(window.picture());
}

/// Where the count of what is still running lands, as a row of the window the
/// click below is aimed at. The picture carries its size and cursor on a
/// header line, so a line of it is one further down than the row it shows.
fn count_row(picture: &str) -> usize {
    picture
        .lines()
        .position(|line| line.contains("1 command"))
        .expect("the count row under the box")
        - 1
}

#[test]
fn a_click_on_the_count_opens_the_list_while_a_turn_is_still_running() {
    // The count is the one thing on the row under the box that can be acted
    // on, and the key that opens the list means backgrounding while a turn is
    // waiting — so the mouse is the door here, and a click that moved nothing
    // was the defect. The command is backgrounded by the model, so the count
    // is up from the moment the call is answered, with the next answer still
    // arriving over it.
    // The answer after the call is long enough to still be arriving when the
    // click lands, which is what keeps the turn running over it: a short one
    // ends the turn first, and the click finds the at-rest box instead.
    let answer = taller_than_the_window();
    let vendor = Vendor::calling(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        &answer,
    );
    let mut window = Watched::allowing("click-count-mid-turn", 60, 24, &vendor, "bash(*)");

    // The first word of the answer after the call is what the catch waits
    // for: the command is backgrounded and counted by then, the turn is
    // provably still running, and the count under the box names the row to
    // click. The spinner keeps the screen beating, so this catches the text
    // rather than waiting for a stillness that does not come. A narrow
    // window, so the model's name is the fact that gives way rather than the
    // count.
    window.types_and_catches("start it\r", "the quick brown fox");
    let at = count_row(&window.picture());

    // The same reason: the answer is still arriving over the list, so opening
    // it is caught by its heading rather than waited out to a still screen.
    window.clicks_catching(at, 0, "Still running");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_command_that_cannot_run_mid_turn_says_so_on_a_panel() {
    // The answer is long enough that the turn is still running when the
    // command is sent: a short one ends first and the command lands at the
    // at-rest box, which is the between-turns path rather than this one.
    let answer = taller_than_the_window();
    let vendor = Vendor::calling(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        &answer,
    );
    let mut window = Watched::allowing("refuse-command-mid-turn", 60, 24, &vendor, "bash(*)");

    // The first word of the answer proves the turn is running: the command is
    // backgrounded and counted, and the answer to it is still arriving. The
    // spinner keeps the screen beating, so the text is caught rather than
    // waited out to a stillness that does not come.
    window.types_and_catches("start it\r", "the quick brown fox");

    // `/logout` removes the key the request now in flight is signed with, so
    // it is the command that must not run mid-turn. What stands instead names
    // it and says why, over the box and the working row.
    window.types_and_catches("/logout\r", "/logout");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_theme_panel_opens_while_a_turn_is_still_running() {
    // The answer is long enough that the turn is still running when the
    // command is sent. `/theme` moves nothing but the screen, so its picker
    // opens over the box with the turn going on behind it.
    let answer = taller_than_the_window();
    let vendor = Vendor::calling(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        &answer,
    );
    let mut window = Watched::allowing("theme-mid-turn", 60, 24, &vendor, "bash(*)");

    // The first word of the answer proves the turn is running before the
    // command is sent.
    window.types_and_catches("start it\r", "the quick brown fox");

    // The picker stands over the box: its title row is the stable mark, and
    // the theme list under it is what choosing moves through.
    window.types_and_catches("/theme\r", "Theme");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_transcript_map_drags_a_long_answer_back_to_its_first_retained_row() {
    // A real SGR mouse click opens the control at the bottom right, then a
    // second gesture drags its current place to the first cell. The
    // transcript jumps from the answer's foot to the opening while the box
    // stays on the same rows underneath it.
    let vendor = Vendor::answering(&taller_than_the_window());
    let mut window = Watched::answering("transcript-map", 80, 16, &vendor);
    window.types("say something long\r");

    // The padded control begins at column 62; the open map track begins at 6.
    window.clicks(15, 62);
    window.drags((15, 69), (15, 6));

    insta::assert_snapshot!(window.picture());
}

/// An answer with something of every shape the reader knows in it.
///
/// Written as one string rather than assembled, because what it is here to
/// carry is the blank rows between the blocks as much as the blocks.
const IN_MARKDOWN: &str = "## What I found\n\n\
    The **loud** part, a `span` of code and a [page](https://example.invalid) \
    beside them.\n\n\
    - one\n- two\n\n\
    > and a line somebody else said\n\n\
    ```rust\nfn main() {}\n```\n";

#[test]
fn an_answer_reaches_the_screen_with_its_markers_read_rather_than_drawn() {
    // The only case here that runs in colour, and the only one that can see
    // this at all: `NO_COLOR` is what keeps every other picture in this file
    // readable, and a run with no colour to put a marker into has no reason to
    // take the marker out — so the reader that turns `##` into a heading and
    // `-` into a bullet had never once been reached through a terminal.
    //
    // What the picture is worth is the text. The screen keeps no colour, so the
    // heading and the bold word are told apart from prose only by the markers
    // being gone; that they are gone, and that the rows and the blank rows
    // between them are the ones a person would count, is the whole assertion.
    let vendor = Vendor::answering(IN_MARKDOWN);
    let mut window = Watched::in_colour("answered-markdown", 80, 32, &vendor);

    window.types_until("say something in markdown\r", "fn main");

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
    // variable each reads from — three rows into the transcript, for somebody
    // who had just said they did not want to be asked. One line is what it owes:
    // enough that the record says the question was asked, and no more.
    let mut window = Watched::open("login-left", 80, 24);

    window.types("/login\r");
    window.types("\x1b");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_window_that_narrows_mid_session_redraws_what_is_live_at_the_new_width() {
    // The size changes under a screen laid out for the old one, and every row
    // of it is drawn again at the new width — the box, what stands over it, and
    // the transcript above them both, since this process owns all three now.
    //
    // What is drawn again is not the same as what is laid out again. Text is
    // folded at whatever the window is now, so the line being typed re-wraps
    // inside a narrower box, and a card arranged into columns by something that
    // has since gone is clipped rather than folded.
    //
    // The opening is the one card that has not gone: it is drawn from facts
    // read once at launch and held for the session, so it is arranged again for
    // the window there is. Which is why it comes back below in one column
    // instead of showing the left half of two.
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
    let vendor = Vendor::recapping_after(
        &[
            "Notes on everything that came before.",
            "Notes on everything that came before.",
            "Notes on everything that came before.",
        ],
        "Notes on everything that came before.",
        None,
    );
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
    let vendor =
        Vendor::recapping_after(&["one answer", "two answer", "three answer"], &notes, None);
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
    //
    // Once, which is the other half of what the picture pins. The line is
    // offered to the running turn and queued behind it, and a turn that ended
    // without reaching it leaves it in both places: sent as its own prompt here
    // and worked into this turn as well, so the record said it twice.
    let notes = "notes to self about everything that has happened so far ".repeat(24);
    let vendor = Vendor::recapping_after(
        &["one answer", "two answer", "three answer"],
        &notes,
        Some("the answer to what was queued"),
    );
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
