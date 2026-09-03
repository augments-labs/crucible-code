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

use std::fmt::Write as _;

use vendor::Vendor;
use watched::Watched;

/// `picture` with the mark on the running turn's row turned back to its first
/// face.
///
/// The mark turns on the wall clock, a face every quarter second from the
/// moment the turn began, and a case that catches the screen mid-turn reaches
/// it however long starting the call took on this machine today. What such a
/// case is about is the rows around that mark, so the mark is steadied rather
/// than the timing. Every face is one column wide, so nothing else on the row
/// moves.
fn on_the_first_beat(picture: &str) -> String {
    let mut steadied = picture.to_owned();
    for face in ["\u{273b}", "\u{273a}", "\u{2731}"] {
        steadied = steadied.replace(face, "\u{2733}");
    }
    steadied
}

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

/// The word [`a_long_answer`] ends with.
///
/// Every sentence of [`taller_than_the_window`] is the same sentence, and the
/// window holds fewer of them than the answer has, so no phrase from the
/// transcript tells an answer that is all here from one that is nearly here.
/// This is the one word only the end can put on screen.
const ANSWER_END: &str = "done.";

/// [`taller_than_the_window`], with an end a case can wait for.
///
/// Waiting for the screen to go still is not the same thing. A stall on a
/// loaded machine is quiet, and quiet is all `settle` has to go on, so a case
/// that only waits for stillness can be handed a transcript the stream had not
/// finished writing — and then draws a map, or a box, around however much of it
/// had arrived.
fn a_long_answer() -> String {
    format!("{}{ANSWER_END}", taller_than_the_window())
}

/// What [`a_turn_still_running`] answers once the call is away.
const HELD_ANSWER: &str = "It is started. That is all.";

/// The last word of [`HELD_ANSWER`].
///
/// Waiting on it is what makes the rows above a mid-turn panel the same rows
/// every run: the answer is whole by then, so nothing further can arrive to
/// move them.
const HELD_LAST_WORD: &str = "all.";

/// A backgrounded call, and a turn still running behind an answer already whole.
///
/// Every case that acts mid-turn wants the same two things: a call in the
/// transcript to act beside, and a turn that has not ended. An answer long
/// enough to still be arriving gives the second by accident and takes the first
/// away — what is drawn is however far the deltas had got when the step landed,
/// so the picture moves with the machine and a loaded one draws a word fewer of
/// it. Holding the message open behind a finished answer pins the screen and
/// leaves the turn exactly where these cases want it.
fn a_turn_still_running() -> Vendor {
    Vendor::calling_then_holding(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        HELD_ANSWER,
    )
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
    let vendor = Vendor::answering(&a_long_answer());
    let mut window = Watched::answering("answered-long", 80, 16, &vendor);

    window.types_until("say something long\r", ANSWER_END);

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
    // is up from the moment the call is answered.
    //
    // The turn is held open behind a finished answer rather than made long
    // enough to still be arriving. A still-arriving one put the pace of the
    // stream into the picture: what stood above the list was however far the
    // deltas had got when the click landed, so a loaded machine drew one word
    // fewer of it and the case failed on its own timing rather than on its
    // subject. Short, because the call has to stay on screen beside the list —
    // the row the click is aimed at is the one under that box — and an answer
    // taller than the window pushes it off the top.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("click-count-mid-turn", 60, 24, &vendor, "bash(*)");

    // Waited for by its last word, so every row the click is measured against
    // is drawn: the command is backgrounded and counted by then, the turn is
    // provably still running behind the keep-alives, and the count under the
    // box names the row to click. A narrow window, so the model's name is the
    // fact that gives way rather than the count.
    window.types_and_catches("start it\r", HELD_LAST_WORD);
    let at = count_row(&window.picture());

    // Caught by its heading rather than waited out to a still screen: the
    // spinner of a turn that is still running keeps the screen beating.
    window.clicks_catching(at, 0, "Still running");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_command_that_cannot_run_mid_turn_says_so_on_a_panel() {
    // The turn is still running when the command is sent; at an at-rest box
    // this would be the between-turns path rather than this one.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("refuse-command-mid-turn", 60, 24, &vendor, "bash(*)");

    // The last word of the answer proves the turn got that far: the command is
    // backgrounded and counted by then. Caught rather than settled for, because
    // the spinner of a turn still running never lets the screen go quiet.
    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // `/logout` removes the key the request now in flight is signed with, so
    // it is the command that must not run mid-turn. What stands instead names
    // it and says why, over the box and the working row.
    window.types_and_catches("/logout\r", "/logout");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn exit_mid_turn_is_refused_with_its_reason() {
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("exit-mid-turn", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // `/exit` ends the session a running turn owns, so it is refused and says
    // why — it is not a word that names no command.
    window.types_and_catches("/exit\r", "ends the session");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_refusal_closed_leaves_a_clean_box() {
    // The turn runs on. `/exit` is refused, the panel closes on esc, and what
    // the box comes back to is a clean one: no command list left standing over
    // it, no `/` to erase, and Enter on a fresh `/` picks a command again.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("refusal-close", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/exit\r", "ends the session");

    // Esc closes the panel. The box it comes back to is empty, and the list is
    // gone with it — nothing of the refused line is left standing.
    window.types_and_catches("\x1b", "esc to interrupt");
    let clean = window.picture();
    assert!(
        !clean.contains("/exit"),
        "the refused line is gone:\n{clean}"
    );
    assert!(!clean.contains("/clear"), "no list is left open:\n{clean}");
    // No snapshot: what this case is about is what is *absent* after esc, and
    // absence is what an assertion says and a picture cannot.
}

#[test]
fn a_command_picked_off_the_list_runs_the_marked_one() {
    // `/ex` typed, the down arrow walked to a row, and Enter runs the row the
    // mark is on — not the half-typed word. The list's mark is what a reader
    // has chosen, and a running turn changes nothing about that.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("marked-command", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // `/ex` filters to `/exit`; Enter runs the marked row, which the refusal
    // names. A bare-typed-word submission would name no command instead.
    window.types_and_catches("/ex", "/exit");
    window.types_and_catches("\r", "ends the session");
}

#[test]
fn a_bare_slash_is_the_list_opener_not_a_command() {
    // Enter on a box holding only `/` is a reader still choosing, not a
    // submission: the line stays, the list stays open, and nothing is refused.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("bare-slash", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/", "/clear");

    // Enter on the bare slash: the list is still open and the box still holds
    // the slash — nothing was submitted.
    window.types_and_catches("\r", "/clear");
    let still = window.picture();
    assert!(!still.contains("names no command"), "no refusal:\n{still}");
}

#[test]
fn a_theme_panel_opens_while_a_turn_is_still_running() {
    // The turn is held open behind a finished answer — not made long enough to
    // still be arriving, which would put the pace of the stream in the
    // snapshot. `/theme` moves nothing but the screen, so its picker opens
    // over the box with the turn going on behind it.
    //
    // The answer is the long one here, because what the picker has to stand
    // over is a transcript taller than the window. Waiting for its end is what
    // makes the row above the panel the same row every run.
    let answer = a_long_answer();
    let vendor = Vendor::calling_then_holding(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        &answer,
    );
    let mut window = Watched::allowing("theme-mid-turn", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", ANSWER_END);

    // The picker stands over the box, covering every transcript row but the
    // first: its title row is the stable mark, and the theme list under it is
    // what choosing moves through.
    window.types_and_catches("/theme\r", "Theme");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_shift_tab_mid_turn_steps_the_mode_the_next_turn_runs_under() {
    // The turn is still running when shift+tab is pressed. The mode the running
    // turn is decided under cannot change mid-turn — the runner holding it is
    // on the worker — so the step is held for the next turn, and the row under
    // the box says which mode that is.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("mode-mid-turn", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // Shift+Tab: from the running mode one step on.
    window.types_and_catches("\x1b[Z", "allow edits on");

    insta::assert_snapshot!(on_the_first_beat(&window.picture()));
}

#[test]
fn a_mode_command_mid_turn_steps_the_mode_the_next_turn_runs_under() {
    // `/mode` typed mid-turn is the shift+tab step made by name: the mode the
    // running turn is decided under cannot change, so the step is held for the
    // next turn and the row under the box says which mode it reached.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("mode-command-mid-turn", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // One step on from ask, the same as one shift+tab.
    window.types_and_catches("/mode\r", "allow edits on");
}

#[test]
fn a_slash_typed_mid_turn_opens_the_command_list() {
    // The turn is still running when `/` is typed. The command list the line
    // opens is the same one the prompt would open between turns, stood above
    // the box while the turn goes on writing behind it.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("list-mid-turn", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // `/` typed into the box opens the list above it.
    window.types_and_catches("/", "/clear");

    insta::assert_snapshot!(on_the_first_beat(&window.picture()));
}

#[test]
fn a_model_picked_mid_turn_is_confirmed_then_held() {
    // The answer arrives whole and the turn goes on running behind it, which is
    // what the command is sent into. `/model` cannot reach the runner on the
    // worker, so its picker opens over the turn, the consequence of a switch is
    // said and agreed to, and the pick is held for the turn the loop starts
    // next.
    //
    // Waited for by its last word: everything under the panel is in this
    // picture, so a step taken while the answer was still arriving would pin
    // the rows to how far the stream had got — and the case would then fail on
    // a loaded machine for a reason that is not its subject. Short, now that
    // holding the turn open is what keeps it running rather than the answer
    // going on long enough to: the command it was sent into stays on screen
    // beside the picker, where a picture of a turn still running wants it.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("model-mid-turn", 100, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // The picker opens, then a step down and Enter picks a model that is not
    // the one in force. What stands next is the consequence, said and asked
    // about before anything is held.
    window.types_and_catches("/model\r", "Model");
    window.types_and_catches("\x1b[B\r", "cached for the current model");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_transcript_map_drags_a_long_answer_back_to_its_first_retained_row() {
    // A real SGR mouse click opens the control at the bottom right, then a
    // second gesture drags its current place to the first cell. The
    // transcript jumps from the answer's foot to the opening while the box
    // stays on the same rows underneath it.
    let vendor = Vendor::answering(&a_long_answer());
    let mut window = Watched::answering("transcript-map", 80, 16, &vendor);
    window.types_until("say something long\r", ANSWER_END);

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
    | what | how |\n| --- | --- |\n| one | first |\n| two | after |\n\n\
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

#[test]
fn markdown_rows_survive_chunking() {
    // How the wire was cut is the vendor's business. A reader holds the opening
    // bytes of a row across a delta — a `- ` that is about to become a bullet,
    // a fence that has not said what it is written in yet — and what asks for a
    // row boundary between two deltas cannot see that it is holding one. So the
    // same answer arriving in pieces gains rows the same answer arriving whole
    // does not, mid-block, where a reader counting bullets would notice.
    //
    // Both sides are drawn rather than one being written down, because what is
    // asserted is that they agree: a snapshot of either would freeze whichever
    // was current and say nothing about the other.
    let arriving = pictured("markdown-chunked", &Vendor::answering(IN_MARKDOWN));
    let at_once = pictured("markdown-whole", &Vendor::answering_whole(IN_MARKDOWN));

    assert_eq!(
        arriving, at_once,
        "the same answer drew differently for having been cut differently"
    );
}

/// The screen an answer from `vendor` leaves behind, in colour.
fn pictured(case: &str, vendor: &Vendor) -> String {
    let mut window = Watched::in_colour(case, 80, 32, vendor);
    window.types_until("say something in markdown\r", "fn main");
    window.picture()
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

/// How many lines the file in [`change_header_survives_resume`] has, on each
/// side of the call that rewrites it.
///
/// Two of these is more lines than a block may draw, which is the whole of why
/// the number is this one: the header then has something to say that the block
/// cannot show, and that sentence is the one thing the live screen and a resumed
/// one are meant to differ on.
const REWRITTEN: usize = 40;

/// The header that call leaves on the row answering it.
const CHANGED: &str = "Added 40 lines, removed 40 lines";

/// What the live screen adds to it, and a resumed screen has no lines to earn.
const UNSHOWN: &str = "16 of them not shown";

/// Every line of one version of that file, each saying which version it is.
fn spelling(tense: &str) -> String {
    (1..=REWRITTEN).fold(String::new(), |mut file, at| {
        let _ = writeln!(file, "the line that {tense} here, number {at}");
        file
    })
}

#[test]
fn change_header_survives_resume() {
    // One call, drawn twice: as it came back, and again off the log once the
    // session was picked up. The header is what the reader is owed both times —
    // a session put back on the screen that forgot what a call changed reads as
    // though nothing happened in it.
    //
    // The lines under the header are the reader's alone and never reach the log,
    // so the block is live-only by construction. The sentence counting what the
    // block could not fit goes with them: on a screen with no block, a header
    // still claiming lines nobody is being shown would be the header lying.
    let input = serde_json::json!({
        "path": "notes.md",
        "find": spelling("was"),
        "replace": spelling("is now"),
    })
    .to_string();
    let vendor = Vendor::calling("edit", &input, "Rewrote it whole.");

    // Tall, because the live screen draws the block. A window the block scrolled
    // the header off the top of would be comparing what fitted rather than what
    // was drawn.
    let mut window = Watched::allowing("resume-change", 80, 100, &vendor, "edit(*)");
    std::fs::write(window.workspace().join("notes.md"), spelling("was"))
        .expect("a file for the call to rewrite");

    window.types_until("rewrite that file\r", "Rewrote it whole");
    let live = window.picture();

    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\r", "Rewrote it whole");
    let again = window.picture();

    assert!(live.contains(CHANGED), "the live header: {live}");
    assert!(live.contains(UNSHOWN), "the live header's tail: {live}");
    assert!(
        live.contains("the line that is now here, number 1"),
        "the live block: {live}"
    );

    assert!(
        again.contains(CHANGED),
        "the resumed screen forgot what the call changed: {again}"
    );
    assert!(
        !again.contains("the line that"),
        "the resumed screen drew lines the log never held: {again}"
    );
    assert!(
        !again.contains(UNSHOWN),
        "the resumed header counted lines nothing is showing: {again}"
    );
}

/// How many files the turn in [`a_resumed_session_says_what_the_reader_watched`]
/// reads before it changes one.
///
/// Enough that the oldest of them fall outside the window of recent output a
/// pruning protects, and that what falls outside is worth clearing. Both are
/// figures the runner holds, and this is the smallest count that clears the
/// first two whatever the reader's own cap does to each result.
/// The mark a result hangs under the call that made it.
///
/// Written out here rather than read off `Glyphs`, so a test asserting where it
/// may not appear cannot be satisfied by the set changing under it.
const HANGS: &str = "\u{23bf}";

const READ: usize = 5;

/// What the four reads after the change come to, drawn as one line.
///
/// Four rather than five because the first read is on the other side of the
/// change, and a call with nothing beside it is not a run.
const GATHERED: &str = "Read 4 files";

/// How many lines each of those files has.
///
/// More than the reader returns, so every result comes back at its ceiling
/// rather than at the file's length: what the pruning is measured against is
/// bytes, and a case whose results were short would clear nothing.
const LONG: usize = 2_000;

/// The first line of the file numbered `at`, which is the line its row says.
///
/// A result's row is its first line, so this is the whole of what a reader sees
/// of a thirty-kilobyte answer — and therefore the whole of what a pruning
/// takes away and a resumed screen has to give back.
fn top(at: usize) -> String {
    format!("the top of the file numbered {at}")
}

/// That file, whole.
fn filled(at: usize) -> String {
    let mut file = top(at);
    for line in 2..=LONG {
        let _ = writeln!(file);
        let _ = write!(file, "line {line} of the file numbered {at}");
    }
    file.push('\n');
    file
}

#[test]
fn a_resumed_session_says_what_the_reader_watched() {
    // Everything this change is about, in one session, drawn twice. A turn that
    // reads five long files and rewrites a sixth, answered in markdown a word at
    // a time on a terminal taking colour; then room asked for, which finds no
    // middle to recap and clears the oldest results instead; then the session
    // put down and picked up again.
    //
    // What the resumed screen owes the reader is what the live one showed: the
    // header saying what the call changed, and the results the pruning cleared
    // saying again what they said. Neither is in the transcript a request is
    // built from — the header is drawn from counts recorded beside the result,
    // and the words come from beside the transcript rather than out of it.
    //
    // The two pictures are not the same picture, and the assertions below say
    // which rows are one screen's alone rather than pretending otherwise. Every
    // other row matches, the cleared results' among them. What differs is the
    // block of lines the call moved, which reaches no log; the sentence counting
    // what that block could not fit, which would be a lie on a screen with no
    // block; and the note saying room was made together with the line that asked
    // for it, which are things that happened to the session rather than messages
    // in it.
    let file = |at: usize| {
        (
            "read",
            serde_json::json!({ "path": format!("file-{at}.txt") }).to_string(),
        )
    };

    // One read, then the change, then the rest, each its own round trip. The
    // change is what parts them, and parting them is the point: the first read
    // stands alone and keeps the row a pruning is measured against, and the
    // four after it come in one batch and so come to one line. Both shapes are
    // in the one picture, which is the only place they can be compared.
    let batches: Vec<Vec<(&str, String)>> = vec![
        vec![file(1)],
        vec![(
            "edit",
            serde_json::json!({
                "path": "notes.md",
                "find": spelling("was"),
                "replace": spelling("is now"),
            })
            .to_string(),
        )],
        (2..=READ).map(file).collect(),
    ];

    let vendor = Vendor::calling_batches(&batches, IN_MARKDOWN);

    // Tall enough to hold the whole session at once. The results a pruning
    // clears are the oldest rows on the screen, so a window that scrolled them
    // away would be comparing what fitted rather than what was drawn.
    let mut window =
        Watched::pruning_in_colour("resume-parity", 100, 200, &vendor, &["read(*)", "edit(*)"]);

    for at in 1..=READ {
        std::fs::write(
            window.workspace().join(format!("file-{at}.txt")),
            filled(at),
        )
        .expect("a file for the call to read");
    }
    std::fs::write(window.workspace().join("notes.md"), spelling("was"))
        .expect("a file for the call to rewrite");

    window.types_until("read those files and rewrite the notes\r", "fn main");
    window.types_until("/compact\r", "old tool output was cleared");
    let live = window.picture();

    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\r", "fn main");
    let again = window.picture();

    // The live screen first, because everything asserted of the resumed one is
    // only worth asserting if this is what the reader was actually shown.
    assert!(live.contains(CHANGED), "the live header: {live}");
    assert!(live.contains(UNSHOWN), "the live header's tail: {live}");
    assert!(
        live.contains("the line that is now here, number 1"),
        "the live block: {live}"
    );
    assert!(
        live.contains(&top(1)),
        "the live row for the read that stood alone: {live}"
    );
    assert!(
        live.contains(GATHERED),
        "the live line for the run of reads: {live}"
    );
    assert!(
        !live.contains(&top(2)),
        "a call in a folded run kept a row of its own: {live}"
    );
    assert!(
        live.contains("old tool output was cleared"),
        "nothing was cleared, so there is no pruning to replay: {live}"
    );

    // And then the same session, off its own log.
    assert!(
        again.contains(CHANGED),
        "the resumed screen forgot what the call changed: {again}"
    );
    assert!(
        again.contains(&top(1)),
        "the resumed screen kept the placeholder where the reader saw an answer: {again}"
    );
    assert!(
        again.contains(GATHERED) && !again.contains(&top(2)),
        "the resumed screen unfolded a run the reader was shown as one line: {again}"
    );
    assert!(
        !again.contains("cleared to make room"),
        "the resumed screen showed the model's placeholder to a person: {again}"
    );
    assert!(
        again.contains("What I found") && again.contains("fn main"),
        "the resumed screen lost the answer: {again}"
    );

    // The three rows that are the live screen's alone, named rather than
    // stumbled over: a resumed screen drawing any of them would be drawing
    // something the log does not hold.
    assert!(
        !again.contains("the line that"),
        "the resumed screen drew lines the log never held: {again}"
    );
    assert!(
        !again.contains(UNSHOWN),
        "the resumed header counted lines nothing is showing: {again}"
    );
    assert!(
        !again.contains("old tool output was cleared"),
        "the resumed screen reported a compaction as though it had just run: {again}"
    );
}

/// How many files the turn in [`a_run_of_lookups_is_one_line_that_opens_it_all`]
/// reads.
///
/// Three, because two is the least a run is folded at and a count that could be
/// mistaken for the threshold proves less than one that cannot.
const GLANCED: usize = 3;

/// One of those files, short enough that a row would have said the whole of it.
///
/// Which is the case worth taking: a result that fitted is dropped where a row
/// said it, and a call in a run has no row — so a short one folded away and not
/// held would leave the reader opening the line to find a call missing from it.
fn glanced(at: usize) -> String {
    format!("{}\nand the second line of it\n", top(at))
}

/// How many round trips the run in
/// [`a_run_that_spans_round_trips_settles_once_a_round_trip`] is spread over.
const TRIPS: usize = 2;

/// How many files each of those round trips reads.
const EACH: usize = 2;

#[test]
fn a_run_that_spans_round_trips_settles_once_a_round_trip() {
    // A turn that only looks around can go on for minutes, and while it does,
    // the counted line stands over the box rather than in the transcript. If
    // the run is only closed when the turn is, a reader watching one of those
    // turns is watching an empty screen with a number on the bottom of it —
    // nothing to scroll back through, nothing to point at, nothing to open.
    //
    // So a round trip closes it. Each is the agent having asked, been answered
    // and gone back for more, which is the smallest unit of a turn that is
    // worth a row, and the line for it joins the transcript the moment it is
    // over.
    let batches: Vec<Vec<(&str, String)>> = (0..TRIPS)
        .map(|trip| {
            (1..=EACH)
                .map(|at| {
                    let path = format!("file-{}.txt", trip * EACH + at);
                    ("read", serde_json::json!({ "path": path }).to_string())
                })
                .collect()
        })
        .collect();

    let vendor = Vendor::calling_batches(&batches, "Read them all.");
    let mut window = Watched::allowing("run-per-trip", 80, 40, &vendor, "read(*)");

    for at in 1..=(TRIPS * EACH) {
        std::fs::write(
            window.workspace().join(format!("file-{at}.txt")),
            glanced(at),
        )
        .expect("the file is written");
    }

    window.types_until("read those files\r", "Read them all");
    let picture = window.picture();

    let counted = picture
        .lines()
        .filter(|line| line.contains(&format!("Read {EACH} files")))
        .count();

    assert_eq!(
        counted,
        TRIPS,
        "one settled line per round trip, not one for the whole turn: {:#?}",
        picture.lines().collect::<Vec<_>>()
    );
}

#[test]
fn a_run_of_lookups_is_one_line_that_opens_it_all() {
    // Three reads and one row, and the row is the door. The promise the fold is
    // made under is that a reader scrolls past fewer rows and loses nothing, so
    // the two halves are asserted together: none of the three results is on the
    // screen, and one click on the line that replaced them stands every one.
    let calls: Vec<(&str, String)> = (1..=GLANCED)
        .map(|at| {
            (
                "read",
                serde_json::json!({ "path": format!("file-{at}.txt") }).to_string(),
            )
        })
        .collect();

    let vendor = Vendor::calling_batches(&[calls], "Read them all.");
    let mut window = Watched::allowing("run-folded", 80, 40, &vendor, "read(*)");

    for at in 1..=GLANCED {
        std::fs::write(
            window.workspace().join(format!("file-{at}.txt")),
            glanced(at),
        )
        .expect("a file for the call to read");
    }

    window.types_until("read those files\r", "Read them all");
    let folded = window.picture();

    assert!(
        folded.contains("Read 3 files"),
        "the run went unsaid: {folded}"
    );
    assert!(
        !folded.contains("Read(file-1.txt)"),
        "a call in the run kept a row of its own: {folded}"
    );
    for at in 1..=GLANCED {
        assert!(
            !folded.contains(&top(at)),
            "a result in the run kept a row of its own: {folded}"
        );
    }

    // The line itself, which is what the slot it is written in offers.
    let at = folded
        .lines()
        .position(|line| line.contains("Read 3 files"))
        .expect("the line the run came to")
        - 1;
    window.clicks(at, 4);
    let opened = window.picture();

    for at in 1..=GLANCED {
        assert!(
            opened.contains(&top(at)),
            "the line opened onto {at} results short of the run: {opened}"
        );
    }
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
    // Waited on rather than typed and left: a command is drawn in two frames
    // with the work between them, and quiet alone cannot tell that gap from the
    // end of the last frame. Without a mark this case reads the screen where
    // the composer has echoed `/effort` and the ladder has not been drawn yet,
    // and blames the renderer for a picture the keyboard was simply ahead of.
    window.types_until("/model claude-test-1\r", "anthropic/claude-test-1");
    window.types_until("/effort\r", "Effort · claude-test-1");

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
    // folded at whatever the window is now, and components whose source is still
    // held — the opening, submitted prompts and file-change blocks — are laid out
    // again. A card arranged by something that is gone is clipped instead.
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
fn the_session_picker_stands_over_the_whole_window() {
    // The picker in the binary rather than in a component test: the words a
    // reader is actually handed, the two panes, and the row of keys under
    // them, on a real screen at a real size — which is where a row that falls
    // off the bottom of the window shows up and a component test cannot.
    let vendor = Vendor::answering("The first thing this session said.");
    let mut window = Watched::asking_on_resume("resume-picture", 80, 24, &vendor);

    window.types_until("say something\r", "The first thing");
    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume\r", "a session, or a branch");

    // Not a snapshot: the heading carries the directory the sessions were
    // recorded in, which is a fresh one per run.
    let picture = window.picture();
    let rows: Vec<&str> = picture.lines().map(str::trim_end).collect();
    for said in [
        "Resume a session · 1 of 1 ·",
        "Enter to resume · Esc to cancel",
        "↑↓ to walk · ctrl+r to rename · type to search · esc to cancel",
    ] {
        assert!(rows.iter().any(|row| row.contains(said)), "{picture}");
    }

    // The keys row is the last thing on the window rather than the first thing
    // off the bottom of it: a panel laid out into a height it does not get
    // loses exactly this row, and loses it silently.
    let keys = rows
        .iter()
        .rposition(|row| row.contains("↑↓ to walk"))
        .expect("the keys row");
    let framed = rows
        .iter()
        .rposition(|row| row.contains('╯'))
        .expect("the foot of the panes");
    assert!(keys > framed, "the keys stand above the panes: {picture}");
}

#[test]
fn picking_a_session_up_asks_before_carrying_it_whole() {
    // The panel in the binary rather than in a component test: it stands where
    // the box was, and what it says has to be readable against a real screen.
    let vendor = Vendor::answering("The first thing this session said.");
    let mut window = Watched::asking_on_resume("resume-asks", 80, 24, &vendor);

    window.types_until("say something\r", "The first thing");
    window.types_until("/clear\r", "ask mode on");

    // The picker stands over the window with the cleared session marked, and
    // Enter takes the mark.
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\r", "This session is large");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn choosing_notes_makes_room_and_says_what_it_took() {
    // Three turns, because a recap stands in place of what is behind the two
    // this keeps whole: a shorter session has no middle to replace, and the
    // choice would spend nothing.
    // Each answer names the turn it belongs to, because a turn is waited out by
    // watching for what only that turn can put on the screen. The permission
    // row is on the screen before, during and after one, so a wait on it is no
    // wait at all: it returns on the first lull, which on a loaded machine is
    // the gap between the keys being echoed and the answer starting to arrive,
    // and the next keys are then typed into a session still answering.
    let vendor = Vendor::recapping_after(
        &[
            "Notes on the first thing.",
            "Notes on the second thing.",
            "Notes on the third thing.",
        ],
        "Notes on everything that came before.",
        None,
    );
    let mut window = Watched::compacting("resume-notes", 80, 24, &vendor);

    window.types_until("the first thing\r", "Notes on the first thing.");
    window.types_until("the second thing\r", "Notes on the second thing.");
    window.types_until("the third thing\r", "Notes on the third thing.");
    window.types_until("/clear\r", "ask mode on");

    // The picker stands over the window with the cleared session marked, and
    // Enter takes the mark.
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\r", "This session is large");

    // Enter takes the first answer, which is the one that spends a request.
    window.types_until("\r", "compacted");

    // The block is the session's own record and hangs off nothing. Ruled above
    // and below, and a rule with a result mark shoved in front of it reads as a
    // result whose first column went missing — which is exactly what nothing
    // asked for it. The choice was made on a picker, so there is not even a
    // line above for a reply to hang from.
    let picture = window.picture();
    assert!(
        !picture.contains(HANGS),
        "the record of room having been made hangs off a call it never had: {picture}"
    );

    insta::assert_snapshot!(picture);
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

    // The other half of what the mark means, held here so removing it wholesale
    // cannot be mistaken for fixing where it did not belong. A line the person
    // typed is above this one, and the sentence under it is the answer to it.
    let picture = window.picture();
    assert!(
        picture.contains(&format!("{HANGS} ! stopped")),
        "the reply to a typed command stands loose of the line that asked: {picture}"
    );

    insta::assert_snapshot!(picture);
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

#[test]
fn renaming_a_long_title_types_where_the_reader_can_see_it() {
    // A title is the first thing that was asked, which is a sentence, and the
    // pane it is renamed in is half a narrow window — so every real rename is
    // this one. Cut to the pane, the field answered every keystroke with the
    // same picture and parked the cursor in the pane beside it: ctrl+r looked
    // like a key that does nothing.
    let vendor = Vendor::answering("The first thing this session said.");
    let mut window = Watched::asking_on_resume("resume-rename", 80, 24, &vendor);

    window.types_until(
        "please tell me everything about the quick brown fox\r",
        "The first thing",
    );
    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume\r", "a session, or a branch");

    // Ctrl+R, then something typed onto the end of the title it opened over.
    window.types_until("\u{12}ZZZ", "enter to save · esc to cancel");

    let picture = window.picture();
    assert!(
        picture.contains("ZZZ"),
        "what was typed never reached the screen: {picture}"
    );
}

/// An answer naming work by the number everybody working on it uses.
const ABOUT_A_NUMBER: &str = "The fix landed in #487, after someone/else#12 \
    was reverted.\n";

#[test]
fn a_number_the_answer_wrote_is_written_as_somewhere_the_reader_can_go() {
    // In colour, because that is the run that reads the model's markdown at
    // all — and a hyperlink is written by the same painter the colour is.
    //
    // The picture is half of this. A hyperlink takes no column and shows in no
    // screenshot, so what the reader sees is the four characters the model
    // wrote, unchanged; whether they point anywhere is a question only the
    // command strings can answer, and both are asserted here.
    let vendor = Vendor::answering(ABOUT_A_NUMBER);
    let mut window = Watched::in_colour("answered-numbers", 80, 32, &vendor);

    window.types_until("say something about a pull request\r", "reverted");

    let picture = window.picture();
    assert!(
        picture.contains("The fix landed in #487, after someone/else#12 was reverted."),
        "the words are the words the model wrote: {picture}"
    );

    let commands = window.commands();
    for address in [
        "8;;https://github.com/augments-labs/crucible-code/issues/487",
        "8;;https://github.com/someone/else/issues/12",
    ] {
        assert!(
            commands.iter().any(|said| said == address),
            "{address} was never written: {commands:?}"
        );
    }
}
