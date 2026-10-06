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
//! The [`native`] module tests native mode, where crucible draws at the foot
//! of the terminal's own buffer and lets finished rows scroll. Every case asks
//! whether the alternate screen was never entered. A case that asserts a line
//! went out once reads both the scrollback and the window.
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

mod fast;
mod native;
mod providers;
mod reaching;
mod screen;
mod vendor;
mod warning;
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
/// moves. The elapsed seconds on that same live status row are zeroed one
/// digit for one digit too: scheduling may advance the clock, but a change in
/// the duration's width must still be visible to the layout assertion.
fn on_the_first_beat(picture: &str) -> String {
    let mut steadied = picture.to_owned();
    for face in ["\u{273b}", "\u{273a}", "\u{2731}"] {
        steadied = steadied.replace(face, "\u{2733}");
    }
    steadied
        .split('\n')
        .map(|line| {
            if let Some((elapsed, rest)) = line
                .strip_prefix("|✳ writing (")
                .and_then(|tail| tail.split_once("s ·"))
                && !elapsed.is_empty()
                && elapsed.bytes().all(|byte| byte.is_ascii_digit())
            {
                format!("|✳ writing ({}s ·{rest}", "0".repeat(elapsed.len()))
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn steadying_a_live_status_changes_only_its_clock_and_spinner() {
    let initial = "|✳ writing (0s · ↓ 4 · esc to interrupt) |\n|allow edits on|";
    for seconds in ['1', '9'] {
        let later = initial
            .replace("0s", &format!("{seconds}s"))
            .replace('✳', "✻");
        assert_eq!(on_the_first_beat(&later), initial);
    }
    let two_digits = initial.replace("0s", "12s");
    let steadied = on_the_first_beat(&two_digits);
    assert_eq!(steadied, initial.replace("0s", "00s"));
    assert_eq!(steadied.chars().count(), two_digits.chars().count());
    let different_usage = initial.replace("↓ 4", "↓ 5");
    assert_eq!(on_the_first_beat(&different_usage), different_usage);
    let transcript = "|the answer says writing (1s) |\n|ask mode on|";
    assert_eq!(on_the_first_beat(transcript), transcript);
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

/// The same turn, held open long enough for a case that queues prompts behind
/// it and then works on them.
fn a_turn_still_running_long() -> Vendor {
    Vendor::calling_then_holding_long(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        HELD_ANSWER,
    )
}

/// Moves the mark of the panel standing to the entry named `name` and takes
/// it: a row is reached by what it says, never by how far down it stands,
/// so a row added above it moves no case.
fn takes(window: &mut Watched, name: &str) {
    let marked = format!("› {name}");
    for _ in 0..24 {
        let picture = window.picture();
        if picture
            .lines()
            .any(|row| row.trim_matches('|').trim_end() == marked)
        {
            window.types("\r");
            return;
        }
        window.types("\x1b[B");
    }
    panic!("no entry reads {name}: {}", window.picture());
}

/// Walks `/login` to the key box of the provider named `name`, saying yes on
/// the way where its route's vendor uses what is sent.
fn keyed(window: &mut Watched, name: &str) {
    window.types_until("/login\r", "Provide your own API key");
    takes(window, "Provide your own API key");
    window.types_until("", "set DEEPSEEK_API_KEY");
    takes(window, name);
    answers(window);
    window.types_until("", "paste or type your API key");
}

/// Says yes to the question a route whose vendor uses what is sent puts, where
/// it stands.
fn answers(window: &mut Watched) {
    if window.picture().contains("Use it anyway") {
        window.types("\r");
    }
}

/// The row directly under the one that reads `› {command}`, without the
/// picture's edges.
fn under(picture: &str, command: &str) -> String {
    let rows: Vec<&str> = picture.lines().collect();
    let at = rows
        .iter()
        .position(|row| row.starts_with(&format!("|› {command}")))
        .unwrap_or_else(|| panic!("no row reads › {command}: {picture}"));
    let Some(row) = rows.get(at + 1) else {
        panic!("nothing under › {command}: {picture}")
    };
    // Less the scroll rail's cell, which stands at the end of a transcript row
    // that is not the row's words.
    row.trim_matches('|')
        .trim_end_matches(['\u{2502}', '\u{2503}', '\u{2022}'])
        .trim_end()
        .to_owned()
}

#[test]
fn a_settled_prompt_writes_nothing_and_stays_off_cpu() {
    // The ordinary between-turns state, not a turn whose working mark is meant
    // to move. `open` has already seen the final ready row and a full quiet
    // window, so every byte and every nanosecond of CPU after that belongs to idle.
    let mut window = Watched::open("idle-prompt", 80, 24);

    window.stays_idle();
}

#[test]
fn a_silent_running_turn_animates_and_echoes_a_key_promptly() {
    // The answer arrives whole, then the vendor sends only invisible keep-alives
    // while holding the request open. That makes every visible change after the
    // last word the shipped binary's own active-turn animation.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("animated-echo", 80, 24, &vendor, "bash(*)");
    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // More than one quarter-second face must reach the PTY, and a plain key must
    // still reach the box inside one generous 400 ms beat window.
    window.turns_then_echoes("z");
}

#[test]
fn every_streamed_frame_keeps_the_answer_prefix_and_never_blanks_it() {
    let words = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
    let vendor = Vendor::answering(&words.join(" "));
    let mut window = Watched::answering("stream-frames", 80, 24, &vendor);

    window.streams_without_flicker("say the planted words", &words);
}

#[test]
fn a_light_terminal_reply_selects_the_light_palette_on_the_real_pty() {
    let window = Watched::on_light_terminal("light-terminal", 80, 24);

    window.uses_light_theme();
}

#[test]
fn every_onboarding_state_can_be_left_for_the_same_clean_prompt() {
    let mut first = Watched::open("login-leave-first", 80, 24);
    first.types_until("/login\r", "Provide your own API key");
    first.types_until("\x1b", "cancelled, nothing signed in");

    // Below the first panel Escape goes back one screen at a time, so each
    // state is left by as many presses as it stands deep.
    let mut provider = Watched::open("login-leave-provider", 80, 24);
    provider.types_until("/login\r", "Provide your own API key");
    takes(&mut provider, "Provide your own API key");
    provider.types_until("", "set DEEPSEEK_API_KEY");
    provider.types_until("\x1b", "Choose how usage is paid for.");
    provider.types_until("\x1b", "cancelled, nothing signed in");

    let mut secret = Watched::open("login-leave-secret", 80, 24);
    keyed(&mut secret, "Anthropic");
    secret.types_until("\x1b", "set DEEPSEEK_API_KEY");
    secret.types_until("\x1b", "Choose how usage is paid for.");
    secret.types_until("\x1b", "cancelled, nothing signed in");

    for picture in [first.picture(), provider.picture(), secret.picture()] {
        assert_eq!(under(&picture, "/login"), "⎿ cancelled, nothing signed in");
        assert!(picture.contains("ask mode on"), "{picture}");
        assert!(!picture.contains("Provide your own API key"), "{picture}");
        assert!(!picture.contains("Choose the provider"), "{picture}");
        assert!(!picture.contains("paste or type your API key"), "{picture}");
    }
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
fn esc_closes_the_command_list_and_keeps_the_line() {
    // The list is the thing in front of the reader, so Esc backs out of it the
    // way it backs out of every other thing standing. What it stood over comes
    // back, and the `/` stays in the box for the next key to go on from.
    let mut window = Watched::open("commands-closed", 80, 24);

    window.types("/");
    window.types_until("\x1b", "Warning: No models available");

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

    window.types_until("what is 2+2\r", "Two plus two is four.");

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

/// Where the count of what is still running lands, as the row of the window
/// and the first cell of it the click below is aimed at: the count is the door,
/// and the mode before it on the same row is not. The picture carries its size
/// and cursor on a header line, so a line of it is one further down than the
/// row it shows, and it frames each row in a bar.
fn count_cell(picture: &str) -> (usize, usize) {
    let (line, row) = picture
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("1 command"))
        .expect("the count row under the box");
    let row = row.strip_prefix('|').unwrap_or(row);
    let before = row.find("1 command").expect("the count on its row");
    (
        line - 1,
        crucible_tui::columns(row.get(..before).unwrap_or_default()),
    )
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
    let (at, column) = count_cell(&window.picture());

    // Caught by its heading rather than waited out to a still screen: the
    // spinner of a turn that is still running keeps the screen beating.
    window.clicks_catching(at, column, "Still running");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_b_opens_the_running_list_during_a_turn() {
    // The key beside the click above. The model backgrounded its own command,
    // so the turn is waiting on no call it could leave running, and Ctrl+B has
    // nothing to background: it opens the list the count opens, by the same
    // call, so the same list stands over the same rows.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("ctrl-b-mid-turn", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);

    // Caught by its heading rather than waited out, for the reason the click
    // gives: the spinner of a turn still running keeps the screen beating.
    window.types_and_catches("\x02", "Still running");

    insta::assert_snapshot!(window.picture());
}

/// The row of the window `said` is written on, and where its drawn cells end.
///
/// The picture carries its size on a header line and frames each row in bars,
/// so a line of it is one further down than the row it shows.
fn row_ending(picture: &str, said: &str) -> (usize, usize) {
    let (line, row) = picture
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(said))
        .unwrap_or_else(|| panic!("no row says {said}:\n{picture}"));
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    (line - 1, crucible_tui::columns(row.trim_end()))
}

#[test]
fn a_click_beside_a_running_command_opens_nothing() {
    // A row of the list is the command's only as far as it drew: the blank
    // after the facts on it is the window's. The row is marked already, so a
    // click that counted there would stand what the command has printed.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("click-beside-running", 60, 24, &vendor, "bash(*)");
    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("\x02", "Still running");

    let before = window.picture();
    let (at, end) = row_ending(&before, "› 1. Bash(sleep 30)");
    for column in [end, 59] {
        let (x, y) = (column + 1, at + 1);
        window.reports(&format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m"));
        let after = window.picture();
        assert_eq!(
            after, before,
            "a click on blank cell {column} of the command's row moved the screen"
        );
    }

    // The same row's own text still opens it, so the clicks above were heard.
    window.clicks(at, 2);
    let opened = window.picture();
    assert!(!opened.contains("enter shows it"), "{opened}");
}

#[test]
fn escape_cancels_a_real_pty_turn_and_returns_to_the_prompt() {
    let vendor = Vendor::answering(&"still arriving ".repeat(96));
    let mut window = Watched::answering("escape-cancels-turn", 80, 24, &vendor);

    window.types_and_catches("start the long answer\r", "still arriving");
    window.types_until("\x1b", "! stopped");

    let picture = window.picture();
    assert!(picture.contains("ask mode on"), "{picture}");
    assert!(!picture.contains("interrupting"), "{picture}");
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
    window.types_and_catches("/", "/help");

    // Enter on the bare slash: the list is still open and the box still holds
    // the slash — nothing was submitted.
    window.types_and_catches("\r", "/help");
    let still = window.picture();
    assert!(!still.contains("names no command"), "no refusal:\n{still}");
}

#[test]
fn a_bare_slash_mid_turn_is_neither_refused_nor_queued() {
    // Enter on the slash that opened the list is a reader still choosing. The
    // next key typed lands after the slash in the box, which is what says the
    // Enter was taken and the line kept: a refused slash would have stood a
    // panel, and a queued one would have left the box empty.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("bare-slash-kept", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/", "/help");
    window.types_and_catches("\r", "/help");
    window.types_and_catches("h", "› /h ");

    let still = window.picture();
    assert!(!still.contains("no such command"), "no refusal:\n{still}");
    assert!(!still.contains("esc to close"), "no panel:\n{still}");
    assert!(!still.contains("queued"), "nothing queued:\n{still}");
}

/// A word typed alone that names no command, said back with what it was near.
///
/// Two rows under the line and nothing after them: the whole list under a slip
/// buries the names that answer it. Nothing is sent, and the arrow that brings
/// back the last line brings back this one to be corrected.
fn refused_as_a_slip(window: &mut Watched) -> String {
    window.types_until("/modle\r", "nearest: /model, /mode");
    let refused = window.picture();
    assert!(refused.contains("no such command: /modle"), "{refused}");
    assert!(!refused.contains("what these are"), "no list:\n{refused}");

    window.types_until("\x1b[A", "│ › /modle");
    refused
}

#[test]
fn a_mistyped_command_is_refused_with_the_names_it_was_nearest_to() {
    let mut window = Watched::open("mistyped", 80, 24);

    insta::assert_snapshot!(refused_as_a_slip(&mut window));
}

#[test]
fn a_mistyped_command_is_refused_the_same_way_in_a_narrow_window() {
    let mut window = Watched::open("mistyped-narrow", 40, 24);

    insta::assert_snapshot!(refused_as_a_slip(&mut window));
}

#[test]
fn a_mistyped_command_mid_turn_is_refused_on_the_panel_and_not_queued() {
    // The same two rows, on the panel a command that cannot run now stands.
    // What heads it is the word typed, not a command it was taken for.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("mistyped-mid-turn", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/modle\r", "nearest: /model, /mode");

    let refused = window.picture();
    assert!(refused.contains("esc to close"), "{refused}");
    assert!(!refused.contains("/exit"), "{refused}");
    assert!(!refused.contains("queued"), "{refused}");
    insta::assert_snapshot!(refused);
}

#[test]
fn a_mistyped_command_with_words_after_it_mid_turn_is_queued_as_a_prompt() {
    // With words after it the line is somebody's sentence, and waits for the
    // turn like any other line typed while one runs.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("mistyped-queued", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/modle gpt-6-sol\r", "1 queued");

    let queued = window.picture();
    assert!(!queued.contains("no such command"), "{queued}");
    assert!(!queued.contains("esc to close"), "{queued}");
}

/// What the queue cases type behind a running turn, in the order typed.
const WAITING: [&str; 5] = [
    "and add a test for the windows path",
    "then run the whole gate",
    "check the ascii glyphs too",
    "update the changelog entry",
    "and say what changed",
];

/// Starts the held turn in `window` and queues the first `count` of
/// [`WAITING`] behind it, each one caught on screen before the next is typed.
fn waiting_behind_a_turn(window: &mut Watched, count: usize) {
    window.types_and_catches("start it\r", HELD_LAST_WORD);
    for (before, prompt) in WAITING.iter().take(count).enumerate() {
        window.types_and_catches(&format!("{prompt}\r"), &format!("{} queued", before + 1));
    }
}

/// A box filled so close to its ceiling that no waiting prompt fits beside it.
///
/// Bracketed, so it lands as one paste. Twenty bytes short of the ceiling,
/// because the shortest prompt in [`WAITING`] is longer than that.
fn a_box_with_no_room() -> String {
    const CEILING: usize = 1024 * 1024;
    format!("\x1b[200~{}\x1b[201~", "a".repeat(CEILING - 20))
}

/// The rows from the running turn's working row to the last one drawn, as a
/// reader sees each: the frame off and the blank cells after the text gone.
///
/// The working row is steadied the way [`on_the_first_beat`] steadies it, in
/// either set of marks. The two rows over it are held to what stands there
/// under a turn: the transcript's last line, then one blank row.
fn under_the_turn(picture: &str) -> Vec<String> {
    let rows: Vec<&str> = picture
        .lines()
        .skip(1)
        .map(|row| {
            row.strip_prefix('|')
                .and_then(|row| row.strip_suffix('|'))
                .unwrap_or(row)
        })
        .collect();
    let working = rows
        .iter()
        .position(|row| row.contains("esc to interrupt"))
        .unwrap_or_else(|| panic!("no working row in\n{picture}"));

    let parted = rows.get(working.wrapping_sub(1)).is_some_and(|row| {
        // The last column is the rail's in fullscreen.
        let width = row.chars().count().saturating_sub(1);
        row.chars().take(width).all(|cell| cell == ' ')
    });
    assert!(parted, "no blank row over the working row in\n{picture}");
    assert!(
        rows.get(working.wrapping_sub(2))
            .is_some_and(|row| row.contains(HELD_ANSWER)),
        "the transcript does not end over the working row in\n{picture}"
    );

    let mut under: Vec<String> = rows
        .iter()
        .skip(working)
        .map(|row| steadied(row.trim_end()))
        .collect();
    // Native mode draws under the transcript, so until the transcript fills
    // the window the rows past the footer are ones nothing has drawn yet.
    while under.last().is_some_and(String::is_empty) {
        under.pop();
    }
    under
}

/// `row` with a working row's mark on its first face and its clock at zero.
fn steadied(row: &str) -> String {
    let Some((mark, rest)) = row.split_once(" writing (") else {
        return row.to_owned();
    };
    let mark = match mark {
        "\u{2733}" | "\u{273b}" | "\u{273a}" | "\u{2731}" => "\u{2733}",
        "|" | "/" | "-" | "\\" => "|",
        other => other,
    };
    match rest.split_once('s') {
        Some((elapsed, tail))
            if !elapsed.is_empty() && elapsed.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            format!("{mark} writing ({}s{tail}", "0".repeat(elapsed.len()))
        }
        _ => format!("{mark} writing ({rest}"),
    }
}

/// `picture` with its working row steadied, for a snapshot.
fn steadied_picture(picture: &str) -> String {
    picture
        .lines()
        .map(|row| match row.strip_prefix('|') {
            Some(inner) if inner.contains("esc to interrupt") => {
                let (cells, edge) = inner.split_at(inner.len() - 1);
                let text = cells.trim_end();
                let pad = cells.chars().count() - text.chars().count();
                format!("|{}{}{edge}", steadied(text), " ".repeat(pad))
            }
            _ => row.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Asserts the rows under the turn in `window` are `drawn`, row for row.
fn draws(window: &Watched, drawn: &[&str]) {
    let picture = window.picture();
    let under = under_the_turn(&picture);
    assert_eq!(under, drawn, "\n{picture}");
}

/// Asserts the panel's own rows under the turn in `window`, from the working
/// row to the blank row under the footer, are those of `drawn`.
///
/// For a box holding a paste: the design draws the box empty, and what the
/// box holds is not what the case is about.
fn draws_the_panel(window: &Watched, drawn: &[&str]) {
    let picture = window.picture();
    let under = under_the_turn(&picture);
    let panel = drawn.len().saturating_sub(5);
    let drawn: Vec<String> = drawn
        .iter()
        .take(panel)
        .map(|row| (*row).to_owned())
        .collect();
    assert_eq!(under.get(..panel), Some(&drawn[..]), "\n{picture}");
}

/// Five prompts waiting, 80 columns.
const WAITING_80: &[&str] = &[
    "✳ writing (0s · ↓ 4 · esc to interrupt)",
    "────────────────────────────────────────────────────────────────────────────────",
    "",
    "5 queued · ctrl+enter to send all now",
    "",
    "› and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "↑↓ to walk · ctrl+e to edit · ctrl+x to delete · ctrl+s to send now",
    "",
    "                                                                 99% window left",
    "╭──────────────────────────────────────────────────────────────────────────────╮",
    "│ ›                                                                            │",
    "╰──────────────────────────────────────────────────────────────────────────────╯",
    "ask mode on (shift+tab to cycle) · 1 command           anthropic · claude-test-1",
];

/// The same in ASCII.
const WAITING_80_ASCII: &[&str] = &[
    "| writing (0s - v 4 - esc to interrupt)",
    "--------------------------------------------------------------------------------",
    "",
    "5 queued - ctrl+enter to send all now",
    "",
    "> and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "^v to walk - ctrl+e to edit - ctrl+x to delete - ctrl+s to send now",
    "",
    "                                                                 99% window left",
    "+------------------------------------------------------------------------------+",
    "| >                                                                            |",
    "+------------------------------------------------------------------------------+",
    "ask mode on (shift+tab to cycle) - 1 command           anthropic - claude-test-1",
];

/// Five prompts waiting, 40 columns: the footer folds.
const WAITING_40: &[&str] = &[
    "✳ writing (0s · ↓ 4 · esc to interrupt)",
    "────────────────────────────────────────",
    "",
    "5 queued · ctrl+enter to send all now",
    "",
    "› and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "↑↓ to walk · ctrl+e to edit · ctrl+x to",
    "delete · ctrl+s to send now",
    "",
    "                         99% window left",
    "╭──────────────────────────────────────╮",
    "│ ›                                    │",
    "╰──────────────────────────────────────╯",
    "ask mode on    anthropic · claude-test-1",
];

/// The same in ASCII.
const WAITING_40_ASCII: &[&str] = &[
    "| writing (0s - v 4 - esc to interrupt)",
    "----------------------------------------",
    "",
    "5 queued - ctrl+enter to send all now",
    "",
    "> and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "^v to walk - ctrl+e to edit - ctrl+x to",
    "delete - ctrl+s to send now",
    "",
    "                         99% window left",
    "+--------------------------------------+",
    "| >                                    |",
    "+--------------------------------------+",
    "ask mode on    anthropic - claude-test-1",
];

/// Walked to the fourth of five: the window has followed the highlight.
const WALKED_TO_THE_FOURTH: &[&str] = &[
    "✳ writing (0s · ↓ 4 · esc to interrupt)",
    "────────────────────────────────────────────────────────────────────────────────",
    "",
    "5 queued · ctrl+enter to send all now",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "› update the changelog entry",
    "",
    "↑↓ to walk · ctrl+e to edit · ctrl+x to delete · ctrl+s to send now",
    "",
    "                                                                 99% window left",
    "╭──────────────────────────────────────────────────────────────────────────────╮",
    "│ ›                                                                            │",
    "╰──────────────────────────────────────────────────────────────────────────────╯",
    "ask mode on (shift+tab to cycle) · 1 command           anthropic · claude-test-1",
];

/// A take-back the box had no room for, 80 columns: the notice beside the title.
const REFUSED_80: &[&str] = &[
    "✳ writing (0s · ↓ 4 · esc to interrupt)",
    "────────────────────────────────────────────────────────────────────────────────",
    "",
    "5 queued · ctrl+enter to send all now · no room in the box · line stays queued",
    "",
    "› and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "↑↓ to walk · ctrl+e to edit · ctrl+x to delete · ctrl+s to send now",
    "",
    "                                                                 99% window left",
    "╭──────────────────────────────────────────────────────────────────────────────╮",
    "│ ›                                                                            │",
    "╰──────────────────────────────────────────────────────────────────────────────╯",
    "ask mode on (shift+tab to cycle) · 1 command           anthropic · claude-test-1",
];

/// The same at 40 columns, where the notice takes the row under the title.
const REFUSED_40: &[&str] = &[
    "✳ writing (0s · ↓ 4 · esc to interrupt)",
    "────────────────────────────────────────",
    "",
    "5 queued · ctrl+enter to send all now",
    "no room in the box · line stays queued",
    "› and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "↑↓ to walk · ctrl+e to edit · ctrl+x to",
    "delete · ctrl+s to send now",
    "",
    "                         99% window left",
    "╭──────────────────────────────────────╮",
    "│ ›                                    │",
    "╰──────────────────────────────────────╯",
    "ask mode on    anthropic · claude-test-1",
];

/// The same at 80 columns in ASCII.
const REFUSED_80_ASCII: &[&str] = &[
    "| writing (0s - v 4 - esc to interrupt)",
    "--------------------------------------------------------------------------------",
    "",
    "5 queued - ctrl+enter to send all now - no room in the box - line stays queued",
    "",
    "> and add a test for the windows path",
    "",
    "  then run the whole gate",
    "",
    "  check the ascii glyphs too",
    "",
    "^v to walk - ctrl+e to edit - ctrl+x to delete - ctrl+s to send now",
    "",
    "                                                                 99% window left",
    "+------------------------------------------------------------------------------+",
    "| >                                                                            |",
    "+------------------------------------------------------------------------------+",
    "ask mode on (shift+tab to cycle) - 1 command           anthropic - claude-test-1",
];

#[test]
fn five_waiting_prompts_stand_in_one_panel_over_the_box() {
    // One panel says what is queued and which goes next, and its keys are
    // Ctrl keys, so the box under it keeps every letter typed. Three prompts
    // are shown and nothing counts the other two: the title and the footer
    // already say there are more and how to reach them.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-panel", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);

    draws(&window, WAITING_80);
    insta::assert_snapshot!(steadied_picture(&window.picture()));
}

#[test]
fn five_waiting_prompts_stand_in_one_panel_in_a_narrow_window() {
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-panel-narrow", 40, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);

    draws(&window, WAITING_40);
    insta::assert_snapshot!(steadied_picture(&window.picture()));
}

#[test]
fn five_waiting_prompts_stand_in_one_panel_in_ascii() {
    for (columns, drawn) in [(80, WAITING_80_ASCII), (40, WAITING_40_ASCII)] {
        let vendor = a_turn_still_running_long();
        let mut window = Watched::allowing_drawn(
            &format!("queue-panel-ascii-{columns}"),
            (columns, 24),
            &vendor,
            "bash(*)",
            ("ascii", "fullscreen"),
        );
        waiting_behind_a_turn(&mut window, 5);

        draws(&window, drawn);
        insta::assert_snapshot!(
            format!("queue_panel_in_ascii_at_{columns}"),
            steadied_picture(&window.picture())
        );
    }
}

#[test]
fn the_arrows_walk_the_highlight_and_the_window_follows_it() {
    // With prompts waiting the arrows walk them, in the place the history
    // walk had: an empty box has no lines of its own to move through. The
    // three rows shown follow the highlight, so it is never walked off screen.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-walked", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);

    for walked_to in WAITING.iter().skip(1).take(3) {
        window.types_and_catches("\x1b[B", &format!("› {walked_to}"));
    }
    draws(&window, WALKED_TO_THE_FOURTH);
    insta::assert_snapshot!(steadied_picture(&window.picture()));

    // And back up, without the history putting an earlier prompt in the box.
    window.types_and_catches("\x1b[A", "› check the ascii glyphs too");
    let picture = window.picture();
    let under = under_the_turn(&picture);
    assert!(
        under
            .iter()
            .any(|row| row.trim_end_matches(['│', ' ']) == "│ ›"),
        "the box took a line from the history:\n{picture}"
    );
}

#[test]
fn ctrl_x_deletes_the_highlighted_prompt_and_the_highlight_stays_in_place() {
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-deleted", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);

    window.types_and_catches("\x18", "4 queued");
    let picture = window.picture();
    let under = under_the_turn(&picture);
    assert_eq!(
        under.get(5..10),
        Some(
            &[
                "› then run the whole gate",
                "",
                "  check the ascii glyphs too",
                "",
                "  update the changelog entry",
            ]
            .map(String::from)[..]
        ),
        "\n{picture}"
    );
    assert!(!picture.contains("windows path"), "{picture}");
}

#[test]
fn deleting_the_last_waiting_prompt_takes_the_panel_away_and_leaves_the_box_empty() {
    // Taking it back would put its words in the box; deleting must not.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-delete-last", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 1);

    // What is typed after it lands in a box holding nothing else: a prompt
    // taken back would be in it already, and the line would read as the two
    // joined.
    window.types_and_catches("\x18hi", "│ › hi");
    let picture = window.picture();
    assert!(!picture.contains("queued"), "{picture}");
    assert!(!picture.contains("windows path"), "{picture}");
}

#[test]
fn ctrl_e_takes_the_highlighted_prompt_back_into_the_box() {
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-taken-back", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 2);

    window.types_and_catches("\x05", "│ › and add a test for the windows path");
    let picture = window.picture();
    assert!(picture.contains("1 queued"), "{picture}");
    assert!(picture.contains("› then run the whole gate"), "{picture}");
}

#[test]
fn ctrl_q_mid_turn_opens_nothing_and_the_next_letter_lands_in_the_box() {
    // Ctrl+Q opened a view that held the queue. Nothing is bound to it now:
    // the panel stands as it stood and what is typed next is the box's.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-ctrl-q", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 2);

    window.types_and_catches("\x11z", "│ › z");
    let picture = window.picture();
    assert!(picture.contains("2 queued"), "{picture}");
    assert!(
        picture.contains("› and add a test for the windows path"),
        "{picture}"
    );
}

#[test]
fn esc_stops_the_turn_and_leaves_what_waits_behind_it() {
    // Esc is about the turn, not the queue: the turn stops as it always has,
    // and the prompts typed behind it are not dropped with it. They are what
    // runs next, each as the line it was typed as.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-esc", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 2);

    window.types_and_catches("\x1b", "! stopped");
    window.catches(
        "the turn stopped",
        &format!("\u{203a} {}", WAITING.get(1).expect("two waiting")),
    );
    let picture = window.picture();
    let stopped = picture.find("! stopped");
    for prompt in WAITING.iter().take(2) {
        let sent = picture.find(&format!("\u{203a} {prompt}"));
        assert!(
            stopped.is_some() && sent > stopped,
            "{prompt:?} was not what ran after the stop:\n{picture}"
        );
    }
}

/// Ctrl+Enter as a terminal asked to spell a modified key distinctly sends it.
const CTRL_ENTER: &str = "\x1b[13;5u";

/// What a case types into the box under a turn, short enough to sit beside
/// what is queued and named nowhere else.
const IN_THE_BOX: &str = "and the box last";

#[test]
fn ctrl_enter_stops_the_turn_and_sends_everything_waiting_then_the_box() {
    // One key for what took Esc and the queue typed again: the turn stops as
    // Esc stops it, and the next turn is every prompt waiting, in the order
    // they were typed, with the line still in the box last. Nothing is left
    // waiting, so the panel goes.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-all", 80, 40, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 3);
    window.types_and_catches(IN_THE_BOX, &format!("\u{203a} {IN_THE_BOX}"));

    // The box's line in the transcript, at the left edge rather than in the
    // box, is the last of what was sent: the answer's words are already on
    // screen from the turn that was stopped, so they say nothing here.
    window.types_and_catches(CTRL_ENTER, "! stopped");
    window.catches("the queue sent", &format!("|\u{203a} {IN_THE_BOX}"));
    let picture = window.picture();
    let stopped = picture.find("! stopped");
    let mut after = stopped;
    for prompt in WAITING.iter().take(3).chain([&IN_THE_BOX]) {
        let sent = picture.find(&format!("|\u{203a} {prompt}"));
        assert!(
            after.is_some() && sent > after,
            "{prompt:?} was not sent after the stop, in its order:\n{picture}"
        );
        after = sent;
    }
    assert!(!picture.contains("queued"), "the panel stayed:\n{picture}");
}

#[test]
fn ctrl_s_stops_the_turn_and_sends_the_highlighted_prompt_alone() {
    // The one prompt that matters most, sooner: the turn stops as Esc stops
    // it and the highlighted prompt is the next turn, alone. The others wait
    // behind it in their order, counted again with the highlight on the one
    // that followed, and what is typed in the box stays there.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-now", 80, 40, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 3);
    window.types_and_catches("\x1b[B", &format!("\u{203a} {}", WAITING[1]));
    window.types_and_catches(IN_THE_BOX, &format!("\u{203a} {IN_THE_BOX}"));

    window.types_and_catches("\x13", "! stopped");
    window.catches(
        "the highlighted prompt sent",
        &format!("|\u{203a} {}", WAITING[1]),
    );
    window.catches("the rest counted again", "2 queued");
    let picture = window.picture();
    let stopped = picture.find("! stopped");
    let sent = picture.find(&format!("\u{203a} {}", WAITING[1]));
    assert!(
        stopped.is_some() && sent > stopped,
        "the highlighted prompt was not what ran after the stop:\n{picture}"
    );
    assert!(
        picture.contains(&format!("  {}", WAITING[0]))
            && picture.contains(&format!("\u{203a} {}", WAITING[2])),
        "the other two are not waiting, the highlight on the one that followed:\n{picture}"
    );
    assert!(
        !picture.contains(&format!("|\u{203a} {}", WAITING[0])),
        "a prompt that was not highlighted was sent:\n{picture}"
    );
    assert!(
        picture.contains(&format!("\u{2502} \u{203a} {IN_THE_BOX}")),
        "the box lost what was typed in it:\n{picture}"
    );
}

/// Longer than crucible takes to draw the stop a key asks for: a key broken on
/// purpose to stop the turn had the stop drawn in under a second.
const LONGER_THAN_A_STOP: std::time::Duration = std::time::Duration::from_secs(2);

/// Ends a case whose turn a key should have left running: Esc stops it, and
/// the one stop on screen is that one.
fn still_running(window: &mut Watched) {
    window.types_and_catches("\x1b", "! stopped");
    let picture = window.picture();
    assert_eq!(
        picture.matches("! stopped").count(),
        1,
        "the turn had stopped before Esc:\n{picture}"
    );
}

#[test]
fn ctrl_enter_with_nothing_to_send_leaves_the_turn_running() {
    // With the box empty and nothing waiting there is nothing to send, and a
    // stop with nothing after it is what Esc is for.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-all-nothing", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 0);

    window.types_and_never_draws(CTRL_ENTER, "! stopped", LONGER_THAN_A_STOP);
    still_running(&mut window);
}

#[test]
fn ctrl_enter_on_a_line_with_no_room_says_so_and_leaves_the_turn_running() {
    // The line in the box will not fit beside what waits, so sending all of it
    // cannot be done: the row says why, and the turn is not stopped for a next
    // turn that would leave the line behind.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-all-refused", 80, 40, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);
    window.types_and_catches(&a_box_with_no_room(), "[Pasted text");

    window.types_and_catches(
        CTRL_ENTER,
        "typed-ahead prompts are limited to 64 lines and 1 MiB",
    );
    window.never_draws("the refusal was drawn", "! stopped", LONGER_THAN_A_STOP);
    assert!(
        window.picture().contains("5 queued"),
        "{}",
        window.picture()
    );
    still_running(&mut window);
}

#[test]
fn ctrl_s_with_nothing_queued_leaves_the_turn_running() {
    // No line is highlighted, so there is none to send sooner: the key does
    // nothing, and the turn goes on.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-now-nothing", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 0);

    window.types_and_never_draws("\x13", "! stopped", LONGER_THAN_A_STOP);
    still_running(&mut window);
}

#[test]
fn ctrl_enter_on_a_command_runs_it_and_leaves_the_turn_running() {
    // A command is no prompt to send: Ctrl+Enter runs it as Return does under
    // a turn, and stops nothing to do it.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-all-command", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 0);
    window.types_and_catches("/theme", "\u{203a} /theme");

    window.types_and_never_draws(CTRL_ENTER, "! stopped", LONGER_THAN_A_STOP);
    let picture = window.picture();
    assert!(
        picture.contains("Theme"),
        "the command did not run:\n{picture}"
    );
    assert!(!picture.contains("queued"), "{picture}");
}

#[test]
fn ctrl_enter_on_a_bare_slash_keeps_it_and_the_list() {
    // The slash that opened the list is a reader still choosing, as it is for
    // Return: the next key typed lands after it, under the list it opened.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-send-all-slash", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 0);
    window.types_and_catches("/", "/help");

    window.types_and_never_draws(CTRL_ENTER, "! stopped", LONGER_THAN_A_STOP);
    window.types_and_catches("h", "\u{203a} /h ");
    let picture = window.picture();
    assert!(picture.contains("/help"), "the list closed:\n{picture}");
    assert!(!picture.contains("queued"), "{picture}");
}

#[test]
fn the_queue_ctrl_s_holds_back_still_answers_its_keys_and_keeps_what_is_typed_for_after() {
    // Behind the line Ctrl+S sent, the rest wait for that turn to end, and
    // they are still the reader's: Ctrl+X deletes one, Ctrl+E takes one back
    // into the box, and Return queues it again. None of it reaches the turn
    // running alone; once it ends, what is left is the next turn, in order.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-held-keys", 80, 40, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 4);

    window.types_and_catches("\x13", "! stopped");
    window.catches(
        "the oldest sent alone",
        &format!("|\u{203a} {}", WAITING[0]),
    );
    window.catches("the rest counted again", "3 queued");

    window.types_and_catches("\x18", "2 queued");
    window.types_and_catches("\x05", &format!("\u{2502} \u{203a} {}", WAITING[2]));
    assert!(
        window.picture().contains("1 queued"),
        "{}",
        window.picture()
    );
    window.types_and_catches("\r", "2 queued");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while !window.recorded().contains(WAITING[3]) {
        assert!(
            std::time::Instant::now() < deadline,
            "what was left waiting was never sent\n{}",
            window.picture()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    window.catches("the rest sent", &format!("|\u{203a} {}", WAITING[2]));

    let picture = window.picture();
    let after_the_last = picture.find(&format!("|\u{203a} {}", WAITING[3]));
    let typed_again = picture.find(&format!("|\u{203a} {}", WAITING[2]));
    assert!(
        after_the_last.is_some() && typed_again > after_the_last,
        "the line typed again was sent before what waited ahead of it:\n{picture}"
    );
    assert_eq!(
        picture
            .matches(&format!("|\u{203a} {}", WAITING[2]))
            .count(),
        1,
        "the line typed again was sent twice:\n{picture}"
    );
    assert!(
        !picture.contains(&format!("|\u{203a} {}", WAITING[1])),
        "the deleted line was sent:\n{picture}"
    );
    assert!(!picture.contains("queued"), "the panel stayed:\n{picture}");
}

/// Queues three prompts behind a held turn in `window`, drawn in `glyphs`,
/// walks the highlight to the second, types [`IN_THE_BOX`] and presses `key`.
/// Returns once the turn the key sent has answered under the last line it
/// sent, `last`: the screen that turn stands on, not the stop on the way to it.
fn sent_now(window: &mut Watched, glyphs: &str, key: &str, last: &str) {
    let mark = if glyphs == "ascii" { '>' } else { '\u{203a}' };
    waiting_behind_a_turn(window, 3);
    window.types_and_catches("\x1b[B", &format!("{mark} {}", WAITING[1]));
    window.types_and_catches(IN_THE_BOX, &format!("{mark} {IN_THE_BOX}"));
    window.types_and_catches(key, "! stopped");

    let sent = format!("|{mark} {last}");
    window.catches_where(
        "the key sent it",
        &format!("answer under {sent:?}"),
        |picture| {
            picture
                .rfind(&sent)
                .and_then(|at| picture.get(at..))
                .is_some_and(|after| after.contains(HELD_ANSWER))
        },
    );
}

#[test]
fn the_queue_sent_now_is_drawn_as_the_stop_and_the_next_turn_draw_it() {
    // Nothing new is drawn for either key. The stopped turn's row reads as
    // Esc leaves it, the prompt sent is written into the transcript as any
    // prompt is, and the panel goes or stands counted again.
    for (columns, glyphs) in [
        (80, "unicode"),
        (40, "unicode"),
        (80, "ascii"),
        (40, "ascii"),
    ] {
        for (key, named, last) in [
            (CTRL_ENTER, "all", IN_THE_BOX),
            ("\x13", "highlighted", WAITING[1]),
        ] {
            let vendor = a_turn_still_running_long();
            let mut window = Watched::allowing_drawn(
                &format!("queue-sent-{named}-{glyphs}-{columns}"),
                (columns, 40),
                &vendor,
                "bash(*)",
                (glyphs, "fullscreen"),
            );
            sent_now(&mut window, glyphs, key, last);

            insta::assert_snapshot!(
                format!("queue_sent_{named}_now_in_{glyphs}_at_{columns}"),
                steadied_picture(&window.picture())
            );
        }
    }
}

#[test]
fn ctrl_enter_between_turns_sends_the_box_as_return_does() {
    // No turn to stop and nothing queued: sending everything now is sending
    // the line, so a terminal that spells the key apart loses nothing by it.
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::answering("send-all-idle", 80, 24, &vendor);

    window.types_and_catches(&format!("say hello{CTRL_ENTER}"), "Hello.");
    let picture = window.picture();
    assert!(
        picture
            .lines()
            .any(|row| row.starts_with("|\u{203a} say hello")),
        "the box was not sent:\n{picture}"
    );
}

#[test]
fn a_take_back_the_box_has_no_room_for_says_so_beside_the_title() {
    // A key that seemed to do nothing left the reader asking whether the
    // prompt was lost. The notice says why and that it was not, in the room
    // beside the title, and the next key takes it away and does what it does.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-refused", 80, 40, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);
    window.types_and_catches(&a_box_with_no_room(), "[Pasted text");

    window.types_and_catches("\x05", "no room in the box · line stays queued");
    draws_the_panel(&window, REFUSED_80);
    insta::assert_snapshot!(steadied_picture(&window.picture()));

    window.types_and_catches("\x1b[B", "› then run the whole gate");
    let picture = window.picture();
    assert!(!picture.contains("no room in the box"), "{picture}");
    assert!(picture.contains("5 queued"), "{picture}");
}

#[test]
fn a_refused_take_back_in_a_narrow_window_says_so_under_the_title() {
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-refused-narrow", 40, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 5);
    window.types_and_catches(&a_box_with_no_room(), "[Pasted text");

    window.types_and_catches("\x05", "no room in the box · line stays queued");
    draws_the_panel(&window, REFUSED_40);
}

#[test]
fn a_prompt_waiting_while_the_reader_walks_the_panel_is_taken_by_the_turn() {
    // The panel is only looked at: walking its highlight holds nothing, so
    // the turn still takes what waits behind it while a reader is on it.
    let vendor = a_turn_still_running_long();
    let mut window = Watched::allowing("queue-not-held", 80, 24, &vendor, "bash(*)");
    waiting_behind_a_turn(&mut window, 2);

    window.types_and_catches("\x1b[B", "2 queued");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while !window.recorded().contains(WAITING[1]) {
        assert!(
            std::time::Instant::now() < deadline,
            "the turn never took what was waiting\n{}",
            window.picture()
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    window.types_and_catches("hi", "│ › hi");
    let picture = window.picture();
    assert!(!picture.contains("queued"), "{picture}");
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

    // `/` typed into the box opens the list above it. At twenty-four rows the
    // running turn's footing leaves too little room for every command, so the
    // list shows what fits and counts the rest on its last row.
    window.types_and_catches("/", "more");

    insta::assert_snapshot!(on_the_first_beat(&window.picture()));
}

#[test]
fn esc_mid_turn_closes_the_command_list_and_the_turn_runs_on() {
    // The list stands over a running turn, and it is the thing in front of the
    // reader rather than the turn: Esc closes it, as it closes the Ctrl+O view,
    // and the turn goes on behind it. The answer the list stood
    // over is what comes back.
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("list-closed-mid-turn", 80, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/", "more");
    window.types_and_catches("\x1b", HELD_ANSWER);

    let picture = window.picture();
    assert!(!picture.contains("interrupting"), "{picture}");
    assert!(picture.contains("esc to interrupt"), "{picture}");
    insta::assert_snapshot!(on_the_first_beat(&picture));
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

    // Name the intended model so additions to the catalogue do not change
    // which switch this case observes. What stands next is the consequence,
    // said and asked about before anything is held.
    window.types_and_catches("/model\r", "Model");
    window.types_and_catches("claude-opus-5\r", "cached for the current model");

    insta::assert_snapshot!(window.picture());
}

/// The last word of the second answer [`two_long_turns`] is given.
const AGAIN_END: &str = "again.";

/// Two long answers, one after the other, each under the prompt that asked.
///
/// Two prompts, so the scroll rail has more than one mark to draw and a
/// transcript several windows tall to stand in. Each answer ends on a word only
/// it has, so the second is waited for rather than mistaken for the first.
fn two_long_turns(case: &str, columns: u16, rows: u16) -> Watched {
    let again = format!("{}{AGAIN_END}", taller_than_the_window());
    let vendor = Vendor::answering_each(&[&a_long_answer(), &again]);
    let mut window = Watched::answering(case, columns, rows, &vendor);
    window.types_until("say something long\r", ANSWER_END);
    window.types_until("say it again\r", AGAIN_END);
    window
}

/// The zero-based rows of `picture` whose last cell is `cell`.
fn rail_rows(picture: &str, cell: char) -> Vec<usize> {
    picture
        .lines()
        .skip(1)
        .enumerate()
        .filter(|(_, line)| line.trim_end_matches('|').ends_with(cell))
        .map(|(row, _)| row)
        .collect()
}

#[test]
fn the_scroll_rail_drags_a_long_answer_back_to_its_first_retained_row() {
    // A real SGR mouse press on the thumb at the foot of the rail, dragged to
    // the rail's first row. The transcript jumps from the answer's foot to the
    // opening while the box stays on the same rows underneath it.
    let vendor = Vendor::answering(&a_long_answer());
    let mut window = Watched::answering("scroll-rail", 80, 16, &vendor);
    window.types_until("say something long\r", ANSWER_END);

    let thumb = rail_rows(&window.picture(), '\u{2503}');
    let foot = *thumb.last().expect("a thumb on the rail");
    window.drags((foot, 79), (0, 79));

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_scroll_rail_left_off_gives_the_transcript_its_last_column() {
    let vendor = Vendor::answering(&a_long_answer());
    let document = format!(
        "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
         \"output\": {{\"scrollRail\": false}},\n  \
         \"providers\": {{\n    \"anthropic\": {{\n      \
         \"model\": \"claude-test-1\",\n      \"baseUrl\": \"{}\"\n    }}\n  }}\n}}\n",
        vendor.address()
    );
    let mut window = Watched::configured("scroll-rail-off", 80, 16, &document, true);
    window.types_until("say something long\r", ANSWER_END);

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_click_on_a_scroll_rail_mark_lands_on_the_prompt_it_marks() {
    let mut window = two_long_turns("scroll-rail-mark", 80, 16);

    // The second prompt's mark is the current prompt's, grown, above the
    // thumb. Landed on, it is still grown, on the thumb.
    let marks = rail_rows(&window.picture(), '\u{25cf}');
    let mark = *marks.last().expect("a mark on the rail");
    window.clicks_catching(mark, 79, "\u{203a} say it again");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn the_scroll_rail_at_rest_grows_the_current_prompt_s_mark() {
    // At the foot of the second answer its prompt is above the band, and its
    // mark is the grown one; the first prompt's is a plain mark.
    for columns in [80, 40] {
        let window = two_long_turns(&format!("scroll-rail-rest-{columns}"), columns, 16);

        insta::assert_snapshot!(
            format!("scroll_rail_at_rest_at_{columns}"),
            window.picture()
        );
    }
}

#[test]
fn a_pointer_on_the_scroll_rail_grows_the_mark_under_it() {
    // A real SGR motion report with no button held, onto the first prompt's
    // mark: it grows beside the current prompt's.
    for columns in [80, 40] {
        let mut window = two_long_turns(&format!("scroll-rail-hover-{columns}"), columns, 16);
        let rail = usize::from(columns) - 1;
        let marks = rail_rows(&window.picture(), '\u{2022}');
        let mark = *marks.first().expect("a mark on the rail");
        window.hovers(mark, rail);

        insta::assert_snapshot!(
            format!("scroll_rail_hovered_at_{columns}"),
            window.picture()
        );
    }
}

#[test]
fn the_scroll_rail_grows_the_current_prompt_s_mark_on_the_thumb_in_a_narrow_window() {
    // Landed on by a click on its mark, the second prompt is in the band and
    // its grown mark stands on the thumb rather than under it. This is the
    // 40-column case; the 80-column one is
    // `a_click_on_a_scroll_rail_mark_lands_on_the_prompt_it_marks`.
    let mut window = two_long_turns("scroll-rail-current-40", 40, 16);
    let marks = rail_rows(&window.picture(), '\u{25cf}');
    let mark = *marks.last().expect("the current prompt's mark");
    window.clicks_catching(mark, 39, "\u{203a} say it again");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn a_click_on_the_scroll_rail_in_a_narrow_window_scrolls_back_to_it() {
    let mut window = two_long_turns("scroll-rail-narrow", 40, 16);

    // A row of bare track in the transcript's band, the middle one of them, so
    // the click seeks rather than landing on a prompt or taking the thumb.
    let track: Vec<usize> = rail_rows(&window.picture(), '\u{2502}')
        .into_iter()
        .filter(|row| *row < 10)
        .collect();
    let row = *track.get(track.len() / 2).expect("track on the rail");
    window.clicks(row, 39);

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
/// cannot show. Both live and resumed previews must report the same omission.
const REWRITTEN: usize = 40;

/// The header that call leaves on the row answering it.
const CHANGED: &str = "Added 40 lines, removed 40 lines";

/// The omitted-line count on both the live and restored bounded preview.
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
    // The private display history retains the bounded preview and its omitted
    // line count without adding either to the model's conversation context.
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
    // The picker's preview carries the answer and the change counts as well, so
    // neither says the session has been picked up. The note counting what the
    // block left out stays with the block, and only the resumed transcript
    // draws it.
    window.types_until("\r", UNSHOWN);
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
        again.contains("the line that is now here, number 1"),
        "the resumed screen lost the stored diff preview: {again}"
    );
    assert!(
        again.contains(UNSHOWN),
        "the resumed preview lost its omitted-line count: {again}"
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
    // Replay restores the original output and bounded change preview from the
    // private log, followed by the notice at the point pruning happened. The
    // smaller model transcript remains independent of the visible history.
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
    // The picker heading precedes its loaded rows, and its preview also
    // contains `fn main`. Wait for a selectable row, then a transcript-only mark.
    window.types_until("/resume\r", "Resume a session · 1 of 1");
    window.types_until("\r", CHANGED);
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

    // Stored display details and compaction notices survive alongside the
    // original results, even though the model context has pruned those results.
    assert!(
        again.contains("the line that is now here, number 1"),
        "the resumed screen lost the stored diff preview: {again}"
    );
    assert!(
        again.contains(UNSHOWN),
        "the resumed preview lost its omitted-line count: {again}"
    );
    assert!(
        again.contains("old tool output was cleared"),
        "the resumed screen omitted the historical compaction marker: {again}"
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

    // The reply is the mark, because quiet is not. Between the echo of the
    // typed line and the answer to it there is nothing on the stream, and on a
    // loaded machine that gap outlasts the settling wait: the screen captured
    // then still holds the command list over an unsent line, which is a
    // picture of the moment before this case begins.
    window.types_until("/logout\r", "still uses ANTHROPIC_API_KEY");

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
    window.types_until("not-a-key-and-nothing-reads-it\r", "login successful");
    window.types("/model claude-test-1\r");
    window.types_until("what is 2+2\r", "Two plus two is four.");

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

    keyed(&mut window, "Anthropic");
    window.types_until("not-a-key-and-nothing-reads-it\r", "login successful");
    window.types("/model claude-test-1\r");
    window.types_until("what is 2+2\r", "Two plus two is four.");

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

    keyed(&mut window, "Anthropic");
    window.types_until("not-a-key-and-nothing-reads-it\r", "login successful");

    let held = std::fs::read_to_string(window.home().join("config.json"))
        .expect("the configuration file this case was given");

    assert!(held.contains("\"provider\": \"anthropic\""), "{held}");
    // What was already in it, byte for byte: the file is spliced rather than
    // rewritten, and a `/login` that dropped the address this case reaches its
    // vendor at would have taken the rest of the suite with it.
    assert!(held.contains("\"baseUrl\""), "{held}");
}

#[test]
fn the_login_panel_offers_the_plans_the_reader_may_hold_and_a_key_of_their_own() {
    // The first panel is a choice between two ways to pay: an account whose
    // plan includes the usage, or a key billed by what is sent. It is the
    // same two rows whatever the store holds, at either width; what is held
    // is said on the lists they lead to.
    for columns in [80, 40] {
        let mut panels = Vec::new();
        for (held, store) in [
            ("empty", None),
            ("key", Some(KEY_HELD)),
            ("sign-in", Some(SIGN_IN_HELD)),
        ] {
            let mut window = Watched::open(&format!("login-ways-{held}-{columns}"), columns, 24);
            if let Some(store) = store {
                std::fs::write(window.home().join("auth.json"), store).expect("a store");
            }
            window.types_until("/login\r", "Provide your own API key");
            let picture = window.picture();
            let panel: Vec<&str> = picture
                .lines()
                .skip_while(|row| !row.contains("Log in"))
                .collect();
            panels.push(panel.join("\n"));
            if store.is_none() {
                if columns == 80 {
                    insta::assert_snapshot!(picture);
                } else {
                    insta::assert_snapshot!("the_login_panel_at_40", picture);
                }
            }
        }
        let first = panels.first().expect("the panel over an empty store");
        assert!(panels.iter().all(|panel| panel == first), "{panels:?}");
        assert!(first.contains("Choose how usage is paid for."), "{first}");
    }
}

/// A store holding an Anthropic key and nothing else, fabricated.
const KEY_HELD: &str =
    r#"{"version":2,"keys":{"anthropic":"fabricated-anthropic-key"},"subscriptions":{}}"#;

/// A store holding an OpenAI sign-in and nothing else, fabricated.
pub(crate) const SIGN_IN_HELD: &str = r#"{"version":2,"keys":{},"subscriptions":{"openai":{"access_token":"fabricated-openai-access","refresh_token":"fabricated-openai-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;

/// A store holding an OpenAI sign-in, an Anthropic key and a kimi.com key.
const HELD_THREE: &str = r#"{"version":2,"keys":{"anthropic":"fabricated-anthropic-key","moonshot":"fabricated-kimi-com-key"},"subscriptions":{"openai":{"access_token":"fabricated-openai-access","refresh_token":"fabricated-openai-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;

#[test]
fn a_row_holding_its_providers_credential_says_signed_in_at_forty_columns() {
    // The mark opens the row's description, so the narrowest window cuts the
    // plan words and keeps it. Nothing of the credential itself is drawn.
    let mut window = Watched::open("login-signed-in-40", 40, 30);
    std::fs::write(window.home().join("auth.json"), HELD_THREE).expect("a store");

    window.types_until("/login\r", "Provide your own API key");
    takes(&mut window, "Your account with subscription");
    window.types_until("", "Kimi Code · kimi.com");
    let plans = window.picture();
    assert!(
        plans.contains("signed in · may train on what is sent"),
        "{plans}"
    );
    assert!(plans.contains("esc to go back"), "{plans}");
    insta::assert_snapshot!("login_accounts_signed_in_40", plans);

    window.types_until("\x1b", "Choose how usage is paid for.");
    takes(&mut window, "Provide your own API key");
    window.types_until("", "set DEEPSEEK_API_KEY");
    // Anthropic's row, signed in with a stored key, is at the top of the
    // list; the kimi.com key row is below what forty columns by thirty rows
    // shows of it, so the mark is walked down to that one.
    let top = window.picture();
    assert_eq!(
        top.matches("signed in with a stored key").count(),
        1,
        "{top}"
    );
    for _ in 0..16 {
        if window
            .picture()
            .contains("signed in · may use what is sent")
        {
            break;
        }
        window.types("\x1b[B");
    }
    let keys = window.picture();
    // The kimi.com key row, whose caution stands where the words about the
    // key would.
    assert!(keys.contains("signed in · may use what is sent"), "{keys}");
    insta::assert_snapshot!("login_keys_signed_in_40", keys);

    for picture in [&plans, &top, &keys] {
        assert!(!picture.contains("fabricated"), "{picture}");
    }
}

#[test]
fn escape_in_a_list_goes_back_to_the_first_panel_with_its_mark_where_it_was() {
    let mut window = Watched::open("login-back", 80, 24);

    window.types_until("/login\r", "Provide your own API key");
    takes(&mut window, "Provide your own API key");
    window.types_until("", "set DEEPSEEK_API_KEY");
    assert!(
        window.picture().contains("esc to go back"),
        "{}",
        window.picture()
    );
    window.types_until("\x1b", "Choose how usage is paid for.");

    let picture = window.picture();
    assert!(picture.contains("› Provide your own API key"), "{picture}");
    assert!(picture.contains("esc to cancel"), "{picture}");
    assert!(!picture.contains("cancelled"), "{picture}");
}

#[test]
fn escape_in_a_key_box_goes_back_to_its_list_with_the_mark_on_the_row_it_came_from() {
    let mut window = Watched::open("login-back-to-google", 80, 24);

    keyed(&mut window, "Google");
    window.types_until("\x1b", "set DEEPSEEK_API_KEY");

    let picture = window.picture();
    assert!(
        picture
            .lines()
            .any(|row| row.trim_matches('|').trim_end() == "› Google"),
        "{picture}"
    );
    assert!(picture.contains("esc to go back"), "{picture}");
}

#[test]
fn a_key_for_a_provider_holding_a_credential_says_what_it_replaces_and_replaces_it_once_stored() {
    // Key over sign-in, and one Kimi site's key over the other's: the box
    // says which credential goes, and the store keeps it until the new key
    // is written.
    for (row, replaced, held_name, map) in [
        (
            "OpenAI",
            "the sign-in held for OpenAI",
            "openai",
            "subscriptions",
        ),
        (
            "MoonshotAI · kimi.ai",
            "the API key held for MoonshotAI · kimi.com",
            "moonshot",
            "keys",
        ),
    ] {
        let mut window = Watched::open("login-replaces", 80, 30);
        let store = window.home().join("auth.json");
        std::fs::write(&store, HELD_THREE).expect("a store");

        keyed(&mut window, row);
        let picture = window.picture();
        let sentence = picture.replace("|\n|", " ").replace("  ", " ");
        assert!(
            sentence.contains(&format!("It replaces {replaced}")),
            "{picture}"
        );
        assert_eq!(
            std::fs::read_to_string(&store).expect("the store"),
            HELD_THREE,
            "nothing replaced before the key is stored"
        );

        window.types_until("not-a-key-and-nothing-reads-it\r", "login successful");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&store).expect("the store"))
                .expect("a store in its format");
        let held = written.get(map).and_then(|map| map.get(held_name));
        assert!(held.is_none(), "{written}");
        let anthropic = written.get("keys").and_then(|keys| keys.get("anthropic"));
        assert!(
            anthropic.is_some_and(serde_json::Value::is_string),
            "{written}"
        );
    }
}

#[test]
fn words_after_login_narrow_the_rows_and_stand_them_under_their_kinds() {
    for columns in [80, 40] {
        let mut window = Watched::open(&format!("login-narrowed-{columns}"), columns, 30);

        window.types_until("/login kimi\r", "Choose the account or key");
        let picture = window.picture();
        for heading in ["Subscription", "API key"] {
            assert!(
                picture
                    .lines()
                    .any(|row| row.trim_matches('|').trim_end() == heading),
                "{picture}"
            );
        }
        assert!(picture.contains("esc to cancel"), "{picture}");
        insta::assert_snapshot!(format!("login_narrowed_{columns}"), picture);

        // Opened by words, so Escape cancels.
        window.types_until("\x1b", "cancelled, nothing signed in");
    }
}

#[test]
fn words_that_leave_one_key_row_open_its_box_and_words_that_leave_none_say_so() {
    let mut window = Watched::open("login-one-row", 80, 24);

    window.types_until("/login openai key\r", "OpenAI API key");
    assert!(
        window.picture().contains("esc to cancel"),
        "{}",
        window.picture()
    );
    window.types_until("\x1b", "cancelled, nothing signed in");

    // A key row of a provider with two opens a box that names its site, once
    // its route has its yes.
    window.types_until("/login moonshot key kimi.com\r", "Use it anyway");
    window.types_until("\r", "API key");
    assert!(
        window.picture().contains("MoonshotAI · kimi.com API key"),
        "{}",
        window.picture()
    );
    window.types_until("\x1b", "cancelled, nothing signed in");

    window.types_until("/login nope\r", "no sign-in matches");
    let picture = window.picture();
    assert!(
        picture.contains(r#"! no sign-in matches "nope"; /login lists them"#),
        "{picture}"
    );
}

#[test]
fn the_key_box_stands_empty_under_the_provider_it_is_for() {
    // The third screen of the walk, before anything is typed: a breadcrumb
    // saying whose key and which way here, one sentence saying where the key
    // goes, a labelled frame with nothing in it yet, and a footer that says
    // paste or type. Nothing of the prompt is on it — no window reading, no
    // model, no map — because a key box is not a turn.
    let mut window = Watched::open("login-key-empty", 80, 24);

    keyed(&mut window, "Anthropic");

    let picture = window.picture();
    assert!(!picture.contains("window"), "{picture}");
    assert!(!picture.contains('%'), "{picture}");
    assert!(!picture.contains("transcript"), "{picture}");
    assert!(!picture.contains("claude"), "{picture}");
    insta::assert_snapshot!(picture);
}

#[test]
fn model_picker_rows_show_wire_ids_instead_of_display_names() {
    let mut window = Watched::open("model-wire-ids", 100, 24);
    for (query, id) in [
        ("Astra", "gpt-6-astra"),
        ("Fable 5.1", "claude-fable-5-1"),
        ("Gemini 3.8", "gemini-3.8-flash"),
        ("Gemini 3.7", "gemini-3.7-flash"),
        ("Gemini 3.6", "gemini-3.6-flash"),
        ("Gemini 3.1", "gemini-3.1-pro-preview"),
        ("K2.7 Coding Highspeed", "kimi-for-coding-highspeed"),
        ("K2.7 Coding", "kimi-for-coding"),
        ("k3-256k", "k3-256k"),
        ("k3", "k3"),
    ] {
        window.types_until("/model\r", "nothing asked yet");
        window.types(query);
        let picture = window.picture();
        assert!(picture.contains(&format!("› {id}")), "{picture}");
        window.types_until("\x1b", "ask mode on");
    }
}

#[test]
fn google_login_and_model_selection_keep_keys_private_and_offer_three_efforts() {
    for columns in [40, 80] {
        let mut window = Watched::open(&format!("google-login-{columns}"), columns, 24);
        // The Google key row's vendor uses what is sent on unpaid quota, so it
        // is asked about before its box stands.
        window.types_until("/login google\r", "Use it anyway");
        window.types_until("\r", "Google API key");
        let login = window.picture();
        assert!(login.contains("Google"));
        assert!(!login.contains("ChatGPT"));
        assert!(!login.contains("device code"));
        assert!(login.contains("esc to cancel"));
        insta::assert_snapshot!(format!("google_api_key_login_{columns}"), login);

        let key = "synthetic-google-key-never-send";
        window.types_until(key, "enter to save");
        assert!(!window.picture().contains(key));
        window.types_until("\r", "login successful");
        window.types("/model\r");
        window.types("gemini");
        let models = window.picture();
        // The model this case takes below; at forty columns the providers
        // folded into the header are cut before Google's name ends.
        assert!(models.contains("gemini-3.8-flash"), "{models}");
        assert!(models.contains("low"));
        assert!(models.contains("medium"));
        assert!(models.contains("high"));
        assert!(!models.contains("xhigh"));
        assert!(!models.contains("max"));
        insta::assert_snapshot!(format!("google_model_effort_{columns}"), models);
        window.types("\r");
        assert!(window.picture().contains("gemini-3.8-flash"));
    }
}

#[test]
fn a_pasted_key_draws_one_mark_per_character_and_offers_to_save() {
    // A key arrives by paste. The box shows how many characters it holds and
    // not one of them, and the footer turns from how to give a key into how to
    // keep it.
    let key = format!("sk-ant-{}", "k".repeat(55));
    let mut window = Watched::open("login-key-pasted", 80, 24);

    keyed(&mut window, "Anthropic");
    window.types_until(&format!("\x1b[200~{key}\x1b[201~"), "enter to save");

    let picture = window.picture();
    assert!(!picture.contains("sk-ant"), "{picture}");
    assert!(picture.contains(&"•".repeat(62)), "{picture}");
    assert!(!picture.contains("window"), "{picture}");
    assert!(!picture.contains('%'), "{picture}");
    insta::assert_snapshot!(picture);
}

#[test]
fn leaving_the_key_box_says_nothing_was_signed_in() {
    // Escape in the box goes back to the list it was chosen from, and from
    // there to the first panel; the Escape that leaves is answered on the
    // same row as leaving the first panel: hung under the command, one
    // sentence, nothing about what was or was not typed.
    let mut window = Watched::open("login-key-left", 80, 24);

    keyed(&mut window, "Anthropic");
    window.types_until("\x1b", "set DEEPSEEK_API_KEY");
    window.types_until("\x1b", "Choose how usage is paid for.");
    window.types_until("\x1b", "cancelled, nothing signed in");

    let picture = window.picture();
    assert_eq!(under(&picture, "/login"), "⎿ cancelled, nothing signed in");
    insta::assert_snapshot!(picture);
}

#[test]
fn a_key_that_cannot_be_written_down_is_said_without_a_path() {
    // A home the store cannot be written into: the row under the command says
    // what stopped and the way back in, and names no file, no directory and
    // nothing the operating system said. The one `/` on it is the command.
    let key = format!("sk-ant-{}", "k".repeat(55));
    let mut window = Watched::open("login-store-failed", 80, 24);
    std::fs::create_dir_all(window.home().join("auth.json"))
        .expect("a directory where the store's file goes");

    keyed(&mut window, "Anthropic");
    window.types_until(&format!("\x1b[200~{key}\x1b[201~\r"), "could not be saved");

    let picture = window.picture();
    let said = under(&picture, "/login");
    assert!(
        said.starts_with("⎿ ! the key could not be saved — "),
        "{said}"
    );
    assert!(!picture.contains("sk-ant"), "{picture}");
    assert!(!picture.contains("auth.json"), "{picture}");
    assert!(!picture.contains("directory"), "{picture}");
    assert!(!said.contains('~'), "{said}");
    assert_eq!(said.replace("/login", "").matches('/').count(), 0, "{said}");
    insta::assert_snapshot!(picture);
}

#[test]
fn openai_account_login_offers_browser_and_device_code_methods() {
    // Provider first, method second. Browser sign-in is the ordinary local
    // path; device code stays visible for a remote terminal or another device.
    // This stops before either network flow starts and snapshots Crucible's
    // own inline panel rather than a provider's interface.
    let mut window = Watched::open("openai-login-methods", 80, 24);

    window.types_until("/login\r", "Provide your own API key");
    takes(&mut window, "Your account with subscription");
    window.types_until("", "Kimi Code · kimi.com");
    takes(&mut window, "OpenAI");
    answers(&mut window);
    window.types_until("", "Choose where to finish account authorization.");

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
    window.types_until("not-a-key-and-nothing-reads-it\r", "login successful");
    // Waited on rather than typed and left: a command is drawn in two frames
    // with the work between them, and quiet alone cannot tell that gap from the
    // end of the last frame. Without a mark this case reads the screen where
    // the composer has echoed `/effort` and the ladder has not been drawn yet,
    // and blames the renderer for a picture the keyboard was simply ahead of.
    window.types_until("/model claude-test-1\r", "anthropic · claude-test-1");
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
fn a_resize_after_the_glyphs_change_lays_the_opening_out_in_the_new_set() {
    // The opening is laid out again on a resize, and what it is laid out in is
    // the glyph set in force then: a reader who turned to ascii because their
    // font lacks the box characters would otherwise get the card back in the
    // very characters they turned away from. Tall enough that the whole card,
    // stacked at the narrower width, is still on screen to be read.
    let mut window = Watched::open("relaid-glyphs", 80, 40);
    window.types_until("/settings\r", "esc to close");
    window.types_until("/glyphs", "Glyphs");
    window.types_until("\r\r", "ascii");
    window.types_until("\x1b", "> Theme");
    window.types_until("\x1b", "Glyphs set to ascii");

    window.resize(52, 40);

    let picture = window.picture();
    let top = picture
        .lines()
        .find(|row| row.contains(" crucible v"))
        .unwrap_or_else(|| panic!("the card's top edge:\n{picture}"));
    assert!(top.starts_with("|+- crucible v"), "{picture}");
    for unicode in ['╭', '╮', '╰', '╯', '│', '─'] {
        assert!(!picture.contains(unicode), "{unicode}:\n{picture}");
    }
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
        // The long form of the keys row is wider than eighty columns, so this
        // is the middle one, each toggle named by what it does next; no
        // branch is checked out here, so Ctrl+B is not offered.
        "ctrl+r rename · ctrl+a all projects · ctrl+w worktrees · esc",
    ] {
        assert!(rows.iter().any(|row| row.contains(said)), "{picture}");
    }

    // The keys row is the last thing on the window rather than the first thing
    // off the bottom of it: a panel laid out into a height it does not get
    // loses exactly this row, and loses it silently.
    let keys = rows
        .iter()
        .rposition(|row| row.contains("ctrl+r rename"))
        .expect("the keys row");
    let framed = rows
        .iter()
        .rposition(|row| row.contains('╯'))
        .expect("the foot of the panes");
    assert!(keys > framed, "the keys stand above the panes: {picture}");
}

// How far `/resume` looks, on a real screen at a real size. Each case starts in
// the home `reaching::planted` leaves, and each picture is the picker after the
// keys the case is about, at eighty columns: the width the keys row has to give
// up its long form in.

#[test]
fn the_resume_picker_opens_on_this_directory() {
    let planted = reaching::planted("reach-here");
    let mut window = Watched::launched(
        "reach-here",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&planted.earlier),
        },
    );

    window.types_until("/resume\r", "a session, or a branch");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_a_shows_every_project_and_says_where_each_session_is() {
    let planted = reaching::planted("reach-all");
    let mut window = Watched::launched(
        "reach-all",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&planted.earlier),
        },
    );

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x01", "all projects");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_w_adds_this_repositorys_other_checkouts() {
    let planted = reaching::planted("reach-worktrees");
    let mut window = Watched::launched(
        "reach-worktrees",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&planted.earlier),
        },
    );

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x17", "this repository's worktrees");

    insta::assert_snapshot!(window.picture());
}

#[test]
fn ctrl_b_keeps_the_branch_checked_out_here() {
    let planted = reaching::planted("reach-branch");
    let mut window = Watched::launched(
        "reach-branch",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&planted.earlier),
        },
    );

    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x02", "1 of 1");

    // The branch is before the directory, so a long directory is what is cut.
    let picture = window.picture();
    assert!(
        picture.contains("Resume a session · 1 of 1 · main · ~"),
        "{picture}"
    );

    insta::assert_snapshot!(picture);
}

#[test]
fn enter_on_another_projects_session_says_how_to_resume_it_there() {
    let planted = reaching::planted("reach-elsewhere");
    let mut window = Watched::launched(
        "reach-elsewhere",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&planted.earlier),
        },
    );

    // Found by the directory its row shows, which nothing in what it asked
    // says.
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\x01", "all projects");
    window.types_until("website", "1 of 4");
    window.types_until("\r", "crucible --resume");

    // The picker is still standing, with the session's own tail beside it:
    // a row the keys put on the list is a row the pane can show.
    let picture = window.picture();
    assert!(picture.contains("Resume a session"), "{picture}");
    assert!(
        picture
            .lines()
            .any(|row| row.contains("│ │ › tidy the stylesheet")),
        "{picture}"
    );
    // Its foot says what Enter does to it, which is not resuming it here.
    assert!(
        picture.contains("│ Enter to see how to resume · Esc to cancel"),
        "{picture}"
    );

    // The command is wider than the window, so it breaks after its `&&` and
    // the id stands whole on a row of its own: copied, both rows run.
    let rows: Vec<&str> = picture.lines().map(str::trim_end).collect();
    let resume = format!("| crucible --resume {}", planted.website.as_str());
    let at = rows
        .iter()
        .position(|row| row.starts_with(&resume))
        .unwrap_or_else(|| panic!("the whole id on a row: {picture}"));
    assert!(
        at.checked_sub(1)
            .and_then(|before| rows.get(before))
            .is_some_and(|row| row.starts_with("| cd ~/projects/website &&")),
        "{picture}"
    );

    // The id is this run's own, so the capture writes it as a mask.
    insta::assert_snapshot!(reaching::unnamed(&picture, &planted.website));
}

#[test]
fn a_directory_with_no_session_of_its_own_still_reaches_the_others() {
    // Nothing was recorded here, but something was elsewhere: the picker
    // opens on this directory's empty list and says so, and Ctrl+A is one key
    // away rather than behind a line that ends the command.
    let earlier = reaching::away("reach-none-here", true);
    let mut window = Watched::launched(
        "reach-none-here",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&earlier),
        },
    );

    window.types_until("/resume\r", "no earlier session for this workspace");
    let picture = window.picture();
    assert!(
        picture.contains("Resume a session · 0 of 0 · "),
        "{picture}"
    );
    assert!(picture.contains("ctrl+a all projects"), "{picture}");
    assert!(picture.contains("ctrl+w worktrees"), "{picture}");

    window.types_until("\x01", "1 of 1 · all projects");
    let picture = window.picture();
    assert!(picture.contains("tidy the stylesheet"), "{picture}");
}

#[test]
fn a_session_that_never_got_past_its_header_is_still_nothing_to_resume() {
    // A log another directory left with nothing in it is no session, so the
    // one line saying there is none stands where the picker would.
    let earlier = reaching::away("reach-header-only", false);
    let mut window = Watched::launched(
        "reach-header-only",
        80,
        24,
        &watched::Launch {
            document: reaching::DOCUMENT,
            env: &[],
            args: &[],
            home: Some(&earlier),
        },
    );

    window.types_until("/resume\r", "no earlier session for this workspace");
    let picture = window.picture();
    assert!(!picture.contains("Resume a session"), "{picture}");
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
fn full_conversation_and_compaction_marker_survive_resume() {
    let vendor = Vendor::recapping_after(
        &[
            "First original answer.",
            "Second original answer.",
            "Third original answer.",
        ],
        "A compact summary of earlier work.",
        Some("Answer after compaction."),
    );
    let mut window = Watched::compacting("resume-full-history", 80, 80, &vendor);
    window.types_until("first original request\r", "First original answer.");
    window.types_until("second original request\r", "Second original answer.");
    window.types_until("third original request\r", "Third original answer.");
    window.types_until("/compact\r", "compacted");
    window.types_until("request after compaction\r", "Answer after compaction.");
    window.types_until("/clear\r", "ask mode on");
    window.types_until("/resume\r", "a session, or a branch");
    window.types_until("\r", "This session is large");
    // Keep the existing conditional model-context decision. The second choice
    // carries the current context without spending another recap request.
    window.types("\x1b[B\r");
    let picture = window.picture();
    assert!(!picture.contains("This session is large"), "{picture}");
    let first = picture
        .find("First original answer.")
        .expect("earliest answer restored");
    let second = picture
        .find("Second original answer.")
        .expect("second answer restored");
    let third = picture
        .find("Third original answer.")
        .expect("third answer restored");
    let compacted = picture
        .find(" compacted ")
        .expect("historical compaction marker");
    let after = picture
        .find("Answer after compaction.")
        .expect("later answer restored");
    assert!(
        first < second && second < third && third < compacted && compacted < after,
        "{picture}"
    );
    assert!(picture.contains("first original request"), "{picture}");
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

#[test]
fn expanded_results_use_the_configured_wheel_speed() {
    for (during, speed) in [(false, 6), (false, 12), (true, 6), (true, 12)] {
        let args = r#"{"path":"wheel.txt"}"#;
        let vendor = if during {
            Vendor::calling_then_holding("read", args, "Ready to inspect.")
        } else {
            Vendor::calling("read", args, "Ready to inspect.")
        };
        let config = serde_json::json!({
            "updates": {"check":"never"},
            "env": {"CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": speed.to_string()},
            "permissions": {"allow":["read(*)"]},
            "providers": {"anthropic": {"model":"claude-sonnet-4-6", "baseUrl": vendor.address()}}
        });
        let config = serde_json::to_string_pretty(&config).unwrap();
        let mut window = Watched::configured("wheel-speed", 80, 24, &config, true);
        let mut text = String::new();
        for at in 1..=100 {
            writeln!(text, "wheel line {at:03}").unwrap();
        }
        std::fs::write(window.workspace().join("wheel.txt"), text).unwrap();
        window.types_until("read the file\r", "Ready to inspect.");
        window.types("\x0f");
        let initial = window.picture();
        window.types(&"\x1b[B".repeat(speed));
        let arrows = window.picture();
        assert_ne!(initial, arrows);
        window.types("\x0f\x0f");
        window.types("\x1b[<65;5;10M");
        let wheeled = window.picture();
        // Compare the content, since a live working marker may animate.
        let content = |picture: &str| {
            picture
                .lines()
                .filter(|line| line.contains("wheel line"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            content(&wheeled),
            content(&arrows),
            "during={during}, speed={speed}"
        );
        window.types("\x1b[<64;5;10M");
        assert_eq!(content(&window.picture()), content(&initial));
    }
}

/// Three files of thirty numbered lines each, too long for the row a read of
/// one is cut to, read `alpha` first and `gamma` last: the results view stands
/// `gamma` at the top and `alpha` at the foot.
fn three_files(window: &Watched) {
    for name in ["alpha", "beta", "gamma"] {
        let mut text = String::new();
        for at in 1..=30 {
            writeln!(text, "{name} line {at:02}").expect("a String takes a line");
        }
        std::fs::write(window.workspace().join(format!("{name}.txt")), text)
            .expect("the file is written into the workspace");
    }
}

/// The calls that read them, one turn's worth each.
fn reading_three() -> Vec<Vec<(&'static str, String)>> {
    ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| vec![("read", format!(r#"{{"path":"{name}.txt"}}"#))])
        .collect()
}

#[test]
fn ctrl_o_pages_and_steps_through_cut_results() {
    // A page moves by the rows the view shows less one, and the arrows across
    // move from the top of one result to the top of the next. The footer says
    // which result is at the top, counting the newest as the first.
    let vendor = Vendor::calling_batches(&reading_three(), "All three are read.");
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "updates": {"check":"never"},
        "permissions": {"allow":["read(*)"]},
        "providers": {"anthropic": {"model":"claude-sonnet-4-6", "baseUrl": vendor.address()}}
    }))
    .unwrap();
    let mut window = Watched::configured("results-paged", 80, 24, &config, true);
    three_files(&window);
    window.types_until("read all three\r", "All three are read.");

    window.types_until("\x0f", "result 1 of 3");
    let opened = window.picture();
    assert!(opened.contains("gamma line 01"), "{opened}");
    assert!(
        opened.contains("esc to close · ↑↓ pgup pgdn to see more · ←→ result 1 of 3"),
        "{opened}"
    );
    insta::assert_snapshot!("ctrl_o_results_opened", opened);

    window.types("\x1b[6~");
    let paged = window.picture();
    assert!(!paged.contains("gamma line 01"), "{paged}");
    insta::assert_snapshot!("ctrl_o_results_paged_down", paged);
    window.types("\x1b[5~");
    assert_eq!(window.picture(), opened);

    window.types_until("\x1b[C", "result 2 of 3");
    let stepped = window.picture();
    assert!(stepped.contains("beta line 01"), "{stepped}");
    insta::assert_snapshot!("ctrl_o_results_stepped_older", stepped);
    window.types_until("\x1b[D", "result 1 of 3");
    assert_eq!(window.picture(), opened);

    window.types("\x1b");
    let closed = window.picture();
    assert!(!closed.contains("pgup pgdn"), "{closed}");
    assert!(closed.contains("All three are read."), "{closed}");
}

#[test]
fn ctrl_o_under_a_running_turn_stands_the_view_in_the_box_s_place() {
    // Under a turn the view takes the rows the box has, as it does between
    // turns: the box standing under it as well pushes the view's foot, and the
    // footer that says how to close it, off the bottom of the window.
    for screen in ["fullscreen", "native"] {
        let vendor =
            Vendor::calling_then_holding("read", r#"{"path":"gamma.txt"}"#, "Ready to inspect.");
        let mut window = Watched::allowing_drawn(
            &format!("results-in-the-box-s-place-{screen}"),
            (80, 24),
            &vendor,
            "read(*)",
            ("unicode", screen),
        );
        three_files(&window);
        window.types_and_catches("read gamma\r", "Ready to inspect.");
        window.types_and_catches("\x0f", "gamma line 02");

        let picture = window.picture();
        let rows: Vec<&str> = picture
            .lines()
            .filter_map(|row| row.strip_prefix('|')?.strip_suffix('|'))
            .collect();
        assert!(
            rows.last()
                .is_some_and(|row| row.starts_with("esc to close")),
            "the view's footer is not on the window's last row: {picture}"
        );
        assert!(
            !rows.iter().any(|row| row.starts_with('\u{256d}')),
            "the box still stands under the view: {picture}"
        );

        // And it was a turn it stood under: closed, it gives the box back with
        // the working row over it.
        window.types_and_catches("\x1b", "esc to interrupt");
    }
}

#[test]
fn ctrl_o_under_a_running_turn_keeps_the_working_row_over_the_view() {
    // The view stands in the box's place while a turn runs, and the box had
    // the row saying the turn is running over it: the only thing on screen
    // that says so, and the one that says how to stop it. So the view keeps
    // it, over its rule and parted from the transcript by a blank as the box
    // has it, and shows two lines fewer for them. In both screens and both
    // glyph sets, at a width that fits the footer whole and one that does not.
    for (screen, glyphs, columns, lines) in [
        ("fullscreen", "unicode", 80, 15),
        ("fullscreen", "ascii", 80, 15),
        ("fullscreen", "unicode", 40, 15),
        ("fullscreen", "ascii", 40, 15),
        ("native", "unicode", 80, 3),
        ("native", "ascii", 80, 3),
        ("native", "unicode", 40, 3),
        ("native", "ascii", 40, 3),
    ] {
        // All three asked for in one response, which reports its four tokens.
        let vendor = Vendor::calling_batches_then_holding(
            &[reading_three().concat()],
            "All three are read.",
        );
        let mut window = Watched::allowing_drawn(
            &format!("results-under-a-turn-{screen}-{glyphs}-{columns}"),
            (columns, 24),
            &vendor,
            "read(*)",
            (glyphs, screen),
        );
        three_files(&window);
        window.types_and_catches("read all three\r", "All three are read.");
        window.types_and_catches("\x0f", "to see more");

        let picture = steadied_picture(&window.picture());
        let rows: Vec<&str> = picture
            .lines()
            .filter_map(|row| row.strip_prefix('|')?.strip_suffix('|'))
            .collect();
        let rule = if glyphs == "ascii" { "-" } else { "\u{2500}" }.repeat(columns.into());
        let opens = rows
            .iter()
            .position(|row| *row == rule)
            .unwrap_or_else(|| panic!("no view is standing:\n{picture}"));
        let mark = if glyphs == "ascii" { "|" } else { "\u{2733}" };
        // Whatever the rail draws beside the blank: it is the transcript's row,
        // and the rail's cell there is as it is anywhere in the transcript.
        let rail = ['\u{2502}', '\u{2503}', '\u{25cf}', '|', '#', '*'];

        assert_eq!(
            rows.get(opens.wrapping_sub(1)).map(|row| row.trim_end()),
            Some(
                format!(
                    "{mark} writing (0s {} esc to interrupt)",
                    if glyphs == "ascii" {
                        "- v 4 -"
                    } else {
                        "\u{b7} \u{2193} 4 \u{b7}"
                    }
                )
                .as_str()
            ),
            "the working row is not directly over the view:\n{picture}"
        );
        assert!(
            rows.get(opens.wrapping_sub(2))
                .is_some_and(|row| row.trim_end_matches(rail).trim().is_empty()),
            "no blank row parts the working row from the transcript:\n{picture}"
        );
        assert!(
            rows.get(opens.wrapping_sub(3))
                .is_some_and(|row| row.starts_with("All three are read.")),
            "the transcript lost its row:\n{picture}"
        );
        if screen == "fullscreen" {
            assert_eq!(opens, 3, "the transcript has more than its row:\n{picture}");
        }
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains(" gamma line "))
                .count(),
            lines,
            "{picture}"
        );
        // The last row drawn: a native window with less in it than it is tall
        // leaves the rows under its live region empty.
        assert!(
            rows.iter()
                .rfind(|row| !row.trim().is_empty())
                .is_some_and(|row| row.starts_with("esc to close")),
            "{picture}"
        );
        insta::assert_snapshot!(
            format!("ctrl_o_under_a_turn_{screen}_{glyphs}_at_{columns}"),
            picture
        );
    }
}

#[test]
fn the_working_row_over_the_view_keeps_counting_and_goes_with_its_turn() {
    // The row the view keeps is the live one: its clock goes on counting while
    // the view stands, because it is the only thing on screen saying the turn
    // is alive. And it is the turn's, not the view's: once the turn ends under
    // a view still open, the row and the blank over it go, and the view
    // stands as it does between turns, from the window's top.
    let vendor =
        Vendor::calling_batches_then_holding(&[reading_three().concat()], "All three are read.");
    let mut window = Watched::allowing_drawn(
        "results-outlive-their-turn",
        (80, 24),
        &vendor,
        "read(*)",
        ("unicode", "fullscreen"),
    );
    three_files(&window);
    window.types_and_catches("read all three\r", "All three are read.");
    window.types_and_catches("\x0f", "to see more");
    let rule = "\u{2500}".repeat(80);
    let rows_of = |picture: &str| -> Vec<String> {
        picture
            .lines()
            .filter_map(|row| Some(row.strip_prefix('|')?.strip_suffix('|')?.to_owned()))
            .collect()
    };

    window.catches("the clock over the view", "writing (2s");
    let picture = window.picture();
    let rows = rows_of(&picture);
    let opens = rows
        .iter()
        .position(|row| *row == rule)
        .unwrap_or_else(|| panic!("the view closed while the clock ran:\n{picture}"));
    assert!(
        rows.get(opens.wrapping_sub(1))
            .is_some_and(|row| row.contains("writing (2s")),
        "the clock is not counting over the view:\n{picture}"
    );

    // Its sixteenth line is the first the view shows that it could not while
    // the turn held rows over it.
    window.catches("the turn ends under the view", "gamma line 16");
    let picture = window.picture();
    let rows = rows_of(&picture);
    assert!(!picture.contains("esc to interrupt"), "{picture}");
    assert_eq!(
        rows.iter().position(|row| *row == rule),
        Some(0),
        "the view does not stand from the window's top as between turns:\n{picture}"
    );
    assert!(
        rows.iter()
            .rfind(|row| !row.trim().is_empty())
            .is_some_and(|row| row.starts_with("esc to close")),
        "{picture}"
    );
}

/// A window whose transcript holds one read the transcript cut short, and
/// the row and the drawn cells of the line that offers it.
///
/// The picture carries its size on a header line and frames each row in
/// bars, and the last column of the transcript's rows is the scroll rail's:
/// the cells are read from what is left.
fn one_cut_read(case: &str) -> (Watched, usize, std::ops::Range<usize>) {
    let reading = vec![vec![("read", r#"{"path":"alpha.txt"}"#.to_owned())]];
    let vendor = Vendor::calling_batches(&reading, "The file is read.");
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "updates": {"check":"never"},
        "permissions": {"allow":["read(*)"]},
        "providers": {"anthropic": {"model":"claude-sonnet-4-6", "baseUrl": vendor.address()}}
    }))
    .expect("the configuration is written as JSON");
    let mut window = Watched::configured(case, 80, 24, &config, true);
    three_files(&window);
    window.types_until("read alpha\r", "The file is read.");

    let picture = window.picture();
    let (line, row) = picture
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("ctrl+o to expand"))
        .unwrap_or_else(|| panic!("no row offers the result:\n{picture}"));
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    let transcript: String = row.chars().take(79).collect();
    let said = transcript.trim_end();
    let blank = said.len() - said.trim_start().len();
    let cells = blank..crucible_tui::columns(said);
    (window, line - 1, cells)
}

#[test]
fn a_click_beside_a_cut_result_opens_nothing() {
    // The row is the result's only as far as it drew: the indent under the
    // call and the blank after the offer are the window's.
    let (mut window, at, cells) = one_cut_read("click-beside-cut");
    let before = window.picture();

    for column in [0, cells.start - 1, cells.end, 78] {
        let (x, y) = (column + 1, at + 1);
        window.reports(&format!("\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m"));
        let after = window.picture();
        assert!(
            !after.contains("esc to close"),
            "a click on blank cell {column} opened the result:\n{after}"
        );
        assert_eq!(
            after, before,
            "a click on blank cell {column} moved the screen"
        );
    }
}

#[test]
fn a_click_on_a_cut_result_opens_it() {
    let (mut window, at, cells) = one_cut_read("click-on-cut");

    window.clicks(at, cells.start);
    let opened = window.picture();
    assert!(opened.contains("esc to close"), "{opened}");
    assert!(opened.contains("alpha line 02"), "{opened}");
    insta::assert_snapshot!("a_click_on_a_cut_result_opens_it", opened);
}

#[test]
fn compact_tool_activity_and_its_group_expand_in_the_real_terminal() {
    let script = format!(
        "cat > generated.txt <<'EOF'\n{}\nLAST_ARGUMENT\nEOF",
        "long script line\n".repeat(60)
    );
    let calls = vec![
        vec![
            ("read", r#"{"path":"one.txt"}"#.to_owned()),
            ("read", r#"{"path":"two.txt"}"#.to_owned()),
        ],
        vec![("bash", serde_json::json!({"command":script}).to_string())],
        vec![("read", r#"{"path":"missing.txt"}"#.to_owned())],
    ];
    let vendor = Vendor::calling_batches(&calls, "Inspection finished.");
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "updates":{"check":"never"},
        "permissions":{"allow":["read(*)","bash(*)"]},
        "providers":{"anthropic":{"model":"claude-sonnet-4-6","baseUrl":vendor.address()}}
    }))
    .unwrap();
    let mut window = Watched::configured("compact-activity", 80, 32, &config, true);
    std::fs::write(window.workspace().join("one.txt"), "first retained result").unwrap();
    std::fs::write(window.workspace().join("two.txt"), "second retained result").unwrap();
    window.types_until("inspect and write\r", "Inspection finished.");
    let picture = window.picture();
    assert!(picture.contains("Read 2 files"), "{picture}");
    assert!(picture.contains("Read(missing.txt)"), "{picture}");
    assert!(!picture.contains("LAST_ARGUMENT"), "{picture}");
    let group = picture
        .lines()
        .position(|line| line.contains("Read 2 files"))
        .unwrap();
    assert!(!picture.lines().nth(group).unwrap().contains("ctrl+o"));
    insta::assert_snapshot!("compact_tool_activity", picture);
    window.clicks(group - 1, 4);
    let opened = window.picture();
    assert!(opened.contains("first retained result"), "{opened}");
    assert!(opened.contains("second retained result"), "{opened}");
    insta::assert_snapshot!("compact_tool_group_expanded", opened);
}

#[test]
fn a_turn_ended_from_outside_mid_answer_leaves_what_was_said_in_the_log() {
    use std::os::unix::process::ExitStatusExt as _;

    // Closing the window is a hang-up and `kill` is a termination, and either
    // arrives while the answer is on screen and nothing has said it is over:
    // the vendor holds the message open behind its last word, so the turn has
    // not ended and what was heard is recorded nowhere yet. What the reader
    // watched arrive has to be what the log holds once the process is gone.
    for (case, signal, number) in [("hung-up", "HUP", 1), ("terminated", "TERM", 15)] {
        let vendor = Vendor::holding(HELD_ANSWER);
        let mut window = Watched::answering(case, 80, 24, &vendor);

        window.types_and_catches("say it\r", HELD_LAST_WORD);
        let (ended, wrote) = window.ends_on(signal);
        let recorded = window.recorded();

        assert!(
            recorded.contains(HELD_ANSWER),
            "{signal}: the answer on screen never reached the log: {recorded:?}"
        );
        // The screen it borrowed is handed back on the way, which is every
        // guard the session held being dropped rather than abandoned: a shell
        // left in the alternate screen with the keys still raw is what dying
        // where it stood looked like from the chair.
        assert!(
            wrote.contains("\u{1b}[?1049l"),
            "{signal}: the screen was never handed back: {wrote:?}"
        );
        assert_eq!(
            ended.signal(),
            Some(number),
            "{signal}: crucible did not end the way the signal ends a process: {ended:?}"
        );
    }
}

#[test]
fn a_termination_sent_while_a_question_stands_is_not_kept_waiting_for_a_key() {
    use std::os::unix::process::ExitStatusExt as _;

    // The other half of holding a signal back while a turn runs. A permission
    // question waits on the keyboard with no clock, so a termination that was
    // only noted there would do nothing until somebody pressed a key — a `kill`
    // that needs a person. The call has been heard and written down by the time
    // it is asked about, so there is no answer in flight to protect either.
    let vendor = Vendor::calling(
        "edit",
        r#"{"path":"notes.md","find":"was","replace":"is"}"#,
        "Never reached.",
    );
    let mut window = Watched::answering("terminated-asking", 80, 24, &vendor);
    std::fs::write(window.workspace().join("notes.md"), "was").expect("a file to ask about");

    window.types_until("change it\r", "Do you want to proceed?");
    let (ended, _) = window.ends_on("TERM");

    assert_eq!(ended.signal(), Some(15), "{ended:?}");
}

#[test]
fn a_termination_sent_while_the_list_stands_over_an_answer_is_not_kept_waiting() {
    use std::os::unix::process::ExitStatusExt as _;

    // The list of running commands holds the keyboard over a turn the way the
    // question above does, and while it stands the drawing thread takes nothing
    // more from the turn. With the answer still arriving, the queue between them
    // is full a few words after the click and the turn waits on it. A
    // termination noted there has to close the list, stop the turn and end the
    // run, rather than wait for a key or for the answer to finish.
    let answer = format!("{}ANSWER-ENDS-HERE", "still arriving ".repeat(1500));
    let vendor = Vendor::calling(
        "bash",
        r#"{"command":"sleep 30","background":true}"#,
        &answer,
    );
    let mut window = Watched::allowing("terminated-listing", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", "still arriving");
    let (at, column) = count_cell(&window.picture());
    window.clicks_catching(at, column, "Still running");
    let (ended, wrote) = window.ends_on("TERM");

    assert_eq!(ended.signal(), Some(15), "{ended:?}");
    assert!(
        wrote.contains("\u{1b}[?1049l"),
        "the screen was never handed back: {wrote:?}"
    );
    // What was heard before the signal is saved, as it is for a signal anywhere
    // else. Three thousand words at the vendor's pace is fifteen seconds of
    // answer, so one that reached the log whole was waited out, not stopped.
    let recorded = window.recorded();
    assert!(
        recorded.contains("still arriving"),
        "the answer heard before the signal never reached the log: {recorded:?}"
    );
    assert!(
        !recorded.contains("ANSWER-ENDS-HERE"),
        "the turn ran to the end of its answer instead of being stopped"
    );
}

/// A configuration that draws crucible's own marks with the characters every
/// font has.
fn in_ascii() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "updates": {"check": "never"},
        "output": {"glyphs": "ascii"}
    }))
    .expect("a configuration document")
}

/// The last word of 0.41.1 in the changelog: the one release these pictures
/// are of, which no later release changes.
const RELEASE_ENDS: &str = "rule.";

#[test]
fn one_release_is_printed_in_full_with_no_rail() {
    for columns in [40, 80] {
        let mut window = Watched::open(&format!("release-notes-one-{columns}"), columns, 24);
        window.types_until("/release-notes 0.41.1\r", RELEASE_ENDS);

        insta::assert_snapshot!(format!("one_release_at_{columns}"), window.picture());
    }
}

#[test]
fn one_release_is_printed_in_full_in_ascii() {
    for columns in [40, 80] {
        let config = in_ascii();
        let mut window = Watched::configured(
            &format!("release-notes-one-ascii-{columns}"),
            columns,
            24,
            &config,
            false,
        );
        window.types_until("/release-notes v0.41.1\r", RELEASE_ENDS);

        insta::assert_snapshot!(
            format!("one_release_in_ascii_at_{columns}"),
            window.picture()
        );
    }
}

#[test]
fn a_version_that_is_no_release_is_refused_naming_the_newest() {
    // The newest is the running version, and so masked: no release moves it.
    for columns in [40, 80] {
        let mut window = Watched::open(&format!("release-notes-none-{columns}"), columns, 24);
        window.types_until("/release-notes 0.0.0\r", "no release 0.0.0");

        insta::assert_snapshot!(format!("no_release_at_{columns}"), window.picture());
    }
}

#[test]
fn a_version_that_is_no_release_is_refused_the_same_in_ascii() {
    for columns in [40, 80] {
        let config = in_ascii();
        let mut window = Watched::configured(
            &format!("release-notes-none-ascii-{columns}"),
            columns,
            24,
            &config,
            false,
        );
        window.types_until("/release-notes 0.0.0\r", "no release 0.0.0");

        insta::assert_snapshot!(
            format!("no_release_in_ascii_at_{columns}"),
            window.picture()
        );
    }
}

/// Rows enough to hold the newest release of the changelog built in, whole, at
/// `columns`, with the box and the closing row under it.
///
/// Its words hang five columns in, and a word that does not fit is carried
/// whole to the next row, so a row and the one after it together hold more
/// than the room: each line of the section takes at most its length over half
/// that room, and one more. A quarter more again, and the box and the closing
/// row, is margin.
fn holding_the_newest(columns: u16) -> u16 {
    const CHANGELOG: &str = include_str!("../../CHANGELOG.md");
    let newest = CHANGELOG
        .split("\n## [")
        .nth(2)
        .expect("a release under Unreleased");
    let half = (usize::from(columns) - 5) / 2;
    let rows: usize = newest.lines().map(|line| line.len() / half + 1).sum();
    u16::try_from(rows * 5 / 4 + 40).expect("a window a terminal can be")
}

#[test]
fn the_whole_list_ends_on_the_running_version_and_the_closing_row() {
    // Read rather than pictured: the list is the changelog built in, which every
    // release adds to. The window is sized from the newest release, so the rows
    // under its head are all on screen whatever that release says.
    for columns in [40, 80] {
        let rows = holding_the_newest(columns);
        let mut window = Watched::open(&format!("release-notes-whole-{columns}"), columns, rows);
        window.types_until("/release-notes all\r", "newest in full");
        let picture = window.picture();
        // A row of the picture without the edges it is drawn between.
        let lines: Vec<&str> = picture
            .lines()
            .map(|line| line.strip_prefix('|').unwrap_or(line))
            .collect();
        let at = format!("{columns} columns:\n{picture}");

        // The release's own row, not a line of some release's notes that
        // happens to say the same words.
        let head = lines
            .iter()
            .rposition(|line| line.starts_with('◆') && line.contains("this version"))
            .unwrap_or_else(|| panic!("no release marked as this version at {at}"));
        let closing = lines
            .iter()
            .rposition(|line| line.contains('⎿'))
            .unwrap_or_else(|| panic!("no closing row at {at}"));
        let newest = lines.get(head).copied().unwrap_or_default();

        // The running version is the one masked out of every picture.
        assert!(newest.contains("◆ ######"), "{newest:?} at {at}");
        assert!(
            head < closing,
            "the closing row is above the newest at {at}"
        );
        let body = lines.get(head + 1..closing).unwrap_or_default();
        assert!(
            body.iter().all(|line| !line.starts_with('│')),
            "a rail stands under the newest at {at}"
        );
        assert!(
            !body
                .iter()
                .any(|line| line.starts_with('◆') || line.starts_with('◇')),
            "another release follows the newest at {at}"
        );
        // Between the closing row and the prompt box, whose own sides are drawn
        // with the rail's character.
        let boxed = lines
            .iter()
            .skip(closing)
            .position(|line| line.starts_with('╭'))
            .map_or(lines.len(), |under| closing + under);
        let after = lines.get(closing + 1..boxed).unwrap_or_default();
        assert!(
            !after
                .iter()
                .any(|line| line.contains('◆') || line.contains('◇') || line.starts_with('│')),
            "the closing row is not the last of the list at {at}"
        );
    }
}

/// Every release the changelog built in holds, newest first, as the headings
/// number them.
fn releases_newest_first() -> Vec<&'static str> {
    include_str!("../../CHANGELOG.md")
        .lines()
        .filter_map(|line| line.strip_prefix("## ["))
        .filter_map(|rest| rest.split_once(']').map(|(version, _)| version))
        .filter(|version| *version != "Unreleased")
        .collect()
}

/// How many lines `version`'s section takes in the changelog.
fn release_lines(version: &str) -> usize {
    let heading = format!("## [{version}]");
    include_str!("../../CHANGELOG.md")
        .lines()
        .skip_while(|line| !line.starts_with(&heading))
        .skip(1)
        .take_while(|line| !line.starts_with("## ["))
        .count()
}

/// The rows of `picture` without the edges they are drawn between or the
/// spaces that pad them.
fn trimmed(picture: &str) -> Vec<String> {
    picture
        .lines()
        .skip(1)
        .map(|line| {
            let line = line.strip_prefix('|').unwrap_or(line);
            let line = line.strip_suffix('|').unwrap_or(line);
            line.trim_end().to_owned()
        })
        .collect()
}

/// `picture` with the numbers a release moves taken out, so that what is
/// accepted beside it is the list's shape and not the changelog it was drawn
/// from: every digit is a `#`, the count of entries is always two of them over
/// the plural, and the number in `6 newer`, `88 older` and `all 101 releases`
/// is one, with the spaces it leaves put back so every row is as wide as it
/// was drawn. The line giving the window's size is left as it is.
fn shape(picture: &str) -> String {
    let mut shaped = String::new();
    for (index, line) in picture.lines().enumerate() {
        if index == 0 {
            shaped.push_str(line);
            shaped.push('\n');
            continue;
        }
        let line = line.replacen(" entry  ", " entries", 1);
        let line = match line.find(" entries") {
            Some(at) if at >= 2 && line.is_char_boundary(at - 2) => {
                format!(
                    "{}##{}",
                    line.get(..at - 2).unwrap_or(""),
                    line.get(at..).unwrap_or("")
                )
            }
            _ => line,
        };
        let drawn = line.chars().count();
        let mut row = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if !c.is_ascii_digit() {
                row.push(c);
                continue;
            }
            let mut digits = 1;
            while chars.next_if(char::is_ascii_digit).is_some() {
                digits += 1;
            }
            let rest: String = chars.clone().collect();
            let counted = [" newer", " older", " releases"]
                .iter()
                .any(|word| rest.starts_with(word));
            row.push_str(&"#".repeat(if counted { 1 } else { digits }));
        }
        // Whatever a counted number lost goes back before the row's edge.
        let lost = drawn - row.chars().count();
        if lost > 0 {
            let edge = row.pop();
            row.push_str(&" ".repeat(lost));
            row.extend(edge);
        }
        shaped.push_str(&row);
        shaped.push('\n');
    }
    shaped
}

/// `count` presses of the down arrow, as one string.
fn downs(count: usize) -> String {
    "\x1b[B".repeat(count)
}

#[test]
fn release_notes_list_stands_the_newest_few_and_a_row_that_reveals_the_rest() {
    // Read rather than pictured, like the whole list above: its rows are the
    // changelog's newest, which every release moves.
    let every = releases_newest_first();
    for columns in [40, 80] {
        let mut window = Watched::open(&format!("release-notes-list-{columns}"), columns, 24);
        window.types_until("/release-notes\r", "enter opens it");
        let picture = window.picture();
        insta::assert_snapshot!(format!("release_notes_list_at_{columns}"), shape(&picture));
        let lines = trimmed(&picture);
        let at = format!("{columns} columns:\n{picture}");

        let title = lines
            .iter()
            .position(|line| line == "Release notes")
            .unwrap_or_else(|| panic!("no title at {at}"));
        let listed = lines.get(title + 2..title + 11).unwrap_or_default();

        // The mark is on the newest, which is the running version: masked.
        let first = listed.first().map_or("", String::as_str);
        assert!(first.starts_with("\u{203a} ######"), "{first:?} at {at}");
        assert!(first.ends_with("this version"), "{first:?} at {at}");
        for (line, version) in listed.iter().skip(1).zip(every.iter().skip(1)).take(7) {
            assert!(
                line.starts_with(&format!("  {version} ")),
                "{line:?} should be {version} at {at}"
            );
        }
        for line in listed.iter().take(8) {
            assert!(line.contains("entr"), "{line:?} has no count at {at}");
            // The date goes first: a date is the one thing here with a dash.
            assert_eq!(line.contains('-'), columns >= 80, "{line:?} at {at}");
        }
        assert_eq!(
            listed.get(8).map(String::as_str),
            Some(format!("  all {} releases \u{2193}", every.len()).as_str()),
            "{at}"
        );

        let foot: Vec<&str> = lines
            .iter()
            .skip(title + 12)
            .map(String::as_str)
            .filter(|line| !line.is_empty())
            .take(2)
            .collect();
        if columns >= 80 {
            assert_eq!(
                foot.first(),
                Some(&"\u{2191}\u{2193} to walk \u{b7} enter opens it \u{b7} esc to close"),
                "{at}"
            );
        } else {
            assert_eq!(
                foot,
                [
                    "\u{2191}\u{2193} to walk \u{b7} enter opens it \u{b7} esc to",
                    "close"
                ],
                "{at}"
            );
        }
    }
}

#[test]
fn release_notes_list_reveals_every_release_in_place_on_the_row_that_was_ninth() {
    let every = releases_newest_first();
    let mut window = Watched::open("release-notes-list-reveal", 80, 24);
    window.types_until("/release-notes\r", "enter opens it");
    window.types_until(&format!("{}\r", downs(8)), "newer");
    let picture = window.picture();
    insta::assert_snapshot!("release_notes_list_reveal_at_80", shape(&picture));
    let lines = trimmed(&picture);

    let title = lines
        .iter()
        .position(|line| line == "Release notes")
        .unwrap_or_else(|| panic!("no title at\n{picture}"));
    let listed = lines.get(title + 2..title + 11).unwrap_or_default();
    let marked: Vec<&String> = listed
        .iter()
        .filter(|line| line.starts_with('\u{203a}'))
        .collect();
    let ninth = every.get(8).copied().unwrap_or_default();

    assert_eq!(marked.len(), 1, "{picture}");
    assert!(
        marked
            .first()
            .is_some_and(|line| line.starts_with(&format!("\u{203a} {ninth} "))),
        "{picture}"
    );
    // The window shows seven releases between its counts, opened with two
    // rows of those before the ninth, so six are above it and the rest below.
    assert_eq!(
        listed.first().map(String::as_str),
        Some("  \u{2191} 6 newer"),
        "{picture}"
    );
    assert_eq!(
        listed.last().map(String::as_str),
        Some(format!("  \u{2193} {} older", every.len() - 13).as_str()),
        "{picture}"
    );
    assert!(
        !picture.contains("releases \u{2193}"),
        "the reveal row stayed: {picture}"
    );
}

#[test]
fn release_notes_list_down_then_enter_puts_the_second_version_alone_in_the_transcript() {
    let every = releases_newest_first();
    let second = every.get(1).copied().expect("two releases");
    // Tall enough for the whole release, so its heading and the command above
    // it are still on screen however long the newest-but-one notes run: no
    // changelog line wraps to more than two rows at this width.
    let rows = 2 * release_lines(second) + 40;
    let mut window = Watched::open(
        "release-notes-list-second",
        80,
        u16::try_from(rows).expect("a release fits a terminal"),
    );
    window.types_until("/release-notes\r", "enter opens it");
    window.types_until("\x1b[B\r", &format!("\u{25c6} {second}"));
    let picture = window.picture();

    assert!(
        !picture.contains("this version"),
        "the newest came too: {picture}"
    );
    assert!(
        !picture.contains('\u{25c7}'),
        "an older release came too: {picture}"
    );
    assert!(
        !picture.contains("enter opens it"),
        "the list stayed: {picture}"
    );
    assert!(picture.contains("\u{203a} /release-notes"), "{picture}");
}

#[test]
fn release_notes_list_opens_one_version_from_the_revealed_rows() {
    let every = releases_newest_first();
    let at = every
        .iter()
        .position(|version| *version == "0.41.1")
        .expect("0.41.1 is a release");
    assert!(at >= 8, "0.41.1 is past the rows the list opens with");

    for columns in [40, 80] {
        let mut window = Watched::open(&format!("release-notes-list-open-{columns}"), columns, 24);
        window.types_until("/release-notes\r", "enter opens it");
        window.types_until(&format!("{}\r", downs(8)), "newer");
        window.types_until(&format!("{}\r", downs(at - 8)), RELEASE_ENDS);

        insta::assert_snapshot!(
            format!("release_notes_one_from_the_list_at_{columns}"),
            window.picture()
        );
    }
}

#[test]
fn release_notes_list_escape_leaves_the_transcript_as_it_was() {
    let mut window = Watched::open("release-notes-list-escape", 80, 24);
    window.types_until("/release-notes\r", "enter opens it");
    window.types(&downs(2));
    window.types("\x1b");
    let picture = window.picture();

    assert!(picture.contains("\u{203a} /release-notes"), "{picture}");
    for gone in [
        "Release notes",
        "enter opens it",
        "\u{25c6}",
        "\u{25c7}",
        "this version",
    ] {
        assert!(!picture.contains(gone), "{gone:?} is on screen: {picture}");
    }
    // The command's own row and the blank row under it are all that is
    // written, so the rows down to the box are blank.
    let under = under_echo(&picture, "/release-notes");
    assert!(under.iter().all(String::is_empty), "{picture}");
}

#[test]
fn release_notes_list_a_resize_that_leaves_no_room_closes_it_and_prints_nothing() {
    let mut window = Watched::open("release-notes-list-cramped", 80, 24);
    window.types_until("/release-notes\r", "enter opens it");
    window.resize(80, 6);
    window.resize(80, 24);
    let picture = window.picture();

    // Walked and abandoned, as escape does: neither the list nor the
    // `all` output, which is what a window that never had room is given.
    for gone in ["enter opens it", "newest in full", "\u{25c6}", "\u{25c7}"] {
        assert!(!picture.contains(gone), "{gone:?} is on screen: {picture}");
    }
    assert!(picture.contains("\u{203a} /release-notes"), "{picture}");
}

/// The rows between the last line that typed `command` and the top of the box
/// under it, trimmed, and less the scroll rail's cell at the end of each, which
/// stands where the transcript is longer than the window and is not the row's.
/// Takes off at most one rail cell at the end of each row.
fn under_echo(picture: &str, command: &str) -> Vec<String> {
    let lines = trimmed(picture);
    let echo = lines
        .iter()
        .rposition(|line| line.starts_with(&format!("\u{203a} {command}")))
        .unwrap_or_else(|| panic!("no {command} at\n{picture}"));
    let under = lines.get(echo + 1..).unwrap_or_default();
    let top = under
        .iter()
        .position(|line| line.starts_with('\u{256d}'))
        .unwrap_or_else(|| panic!("no box under {command} at\n{picture}"));
    under
        .get(..top)
        .unwrap_or_default()
        .iter()
        .map(|row| {
            row.strip_suffix(['\u{2502}', '\u{2503}', '\u{2022}', '\u{25cf}'])
                .unwrap_or(row)
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn under_echo_strips_only_one_trailing_rail_cell() {
    let picture = "80×24\n\
        |› /x │\n\
        |content ends •│\n\
        |marked here ●┃\n\
        |╭─ box top|";
    let result = under_echo(picture, "/x");
    assert_eq!(result, vec!["content ends •", "marked here ●"]);
}

/// A window of 80 by 24 on `screen`, `fullscreen` or `native`, answered by
/// `vendor`.
fn on_screen(screen: &str, case: &str, vendor: &Vendor) -> Watched {
    if screen == "native" {
        Watched::native(case, 80, 24, vendor)
    } else {
        Watched::answering(case, 80, 24, vendor)
    }
}

/// What `/context` closed by escape leaves under its line on `screen`: the
/// spacing every panel closed that way leaves.
fn context_closed(screen: &str, vendor: &Vendor) -> Vec<String> {
    let mut window = on_screen(screen, &format!("context-closed-{screen}"), vendor);
    window.types_until("/context\r", "esc to close");
    window.types_until("\x1b", "ask mode on");
    under_echo(&window.picture(), "/context")
}

/// What `/release-notes` closed by `close` leaves under its line, beside what
/// `/context` closed by escape leaves, on each of `screens`; each screen's
/// picture follows, for the message of a case that finds them apart. In
/// native mode a run of blank rows counts as one.
fn closed_as_context_is(
    case: &str,
    screens: &[&str],
    close: impl Fn(&mut Watched),
) -> (Vec<Vec<String>>, Vec<Vec<String>>, String) {
    let mut seen = Vec::new();
    let mut wanted = Vec::new();
    let mut pictures = String::new();
    for &screen in screens {
        let vendor = Vendor::answering("Hello.");
        let mut context = context_closed(screen, &vendor);

        let mut window = on_screen(screen, &format!("{case}-{screen}"), &vendor);
        window.types_until("/release-notes\r", "enter opens it");
        close(&mut window);
        if screen == "native" {
            window.assert_never_alternate();
        }
        let picture = window.picture();
        assert!(!picture.contains("enter opens it"), "{screen}\n{picture}");
        let mut notes = under_echo(&picture, "/release-notes");
        if screen == "native" {
            // The native region keeps the height its tallest frame gave it, so
            // the blank rows under a closed panel's line count how tall that
            // panel stood. What holds is that they are blank down to the box,
            // so one stands for the run.
            for rows in [&mut context, &mut notes] {
                rows.dedup_by(|row, above| row.is_empty() && above.is_empty());
            }
        }
        wanted.push(context);
        seen.push(notes);
        let _ = writeln!(pictures, "{screen}\n{picture}");
    }
    (seen, wanted, pictures)
}

#[test]
fn release_notes_list_escape_leaves_the_rows_any_other_panel_does() {
    let (seen, wanted, pictures) = closed_as_context_is(
        "release-notes-escape",
        &["fullscreen", "native"],
        |window| {
            window.types_until("\x1b", "ask mode on");
        },
    );
    assert_eq!(seen, wanted, "{pictures}");
}

#[test]
fn release_notes_list_closed_by_a_resize_leaves_the_rows_any_other_panel_does() {
    // Drawn, then a window with no room for it: the list ends as escape ends
    // it, and leaves what escape leaves. On the full screen only: in native
    // mode the rows a window gives up as it shrinks are the terminal's, and
    // what it hands back as it grows again is not this case's to count.
    let (seen, wanted, pictures) =
        closed_as_context_is("release-notes-cramped", &["fullscreen"], |window| {
            window.resize(80, 6);
            window.resize(80, 24);
        });
    assert_eq!(seen, wanted, "{pictures}");
}

#[test]
fn release_notes_mid_turn_are_refused_on_the_panel() {
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("release-notes-mid-turn", 60, 24, &vendor, "bash(*)");

    window.types_and_catches("start it\r", HELD_LAST_WORD);
    window.types_and_catches("/release-notes\r", "stands over, the answer");

    let refused = window.picture();
    assert!(refused.contains("esc to close"), "{refused}");
    assert!(!refused.contains("newest in full"), "{refused}");
    insta::assert_snapshot!(refused);
}

#[test]
fn a_send_on_a_warned_route_stands_the_question_and_sends_nothing_before_the_yes() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-send",
        (80, 30),
        warning::GOOGLE,
        &proxy,
        (&[], None),
    );

    // Enter twice in one write: the first sends, and the second is in hand
    // before the panel is drawn, so it does not answer it.
    window.types_until("hello\r\r", "Use it anyway");
    let asked = window.picture();
    assert!(asked.contains("Use it anyway"), "{asked}");
    assert!(
        asked.contains("On unpaid quota, Google uses what you send"),
        "{asked}"
    );
    assert!(asked.contains("Gemini API terms, 30 Sep 2026"), "{asked}");
    assert!(
        asked.contains("enter to choose · esc to go back"),
        "{asked}"
    );
    assert!(!asked.contains(warning::KEY), "{asked}");
    insta::assert_snapshot!("question_at_send_80", asked);
    assert_eq!(proxy.asked(), Vec::<String>::new(), "sent before the yes");

    // Going back keeps the message, and the question stands again at the
    // next send.
    window.types_until("\x1b", "nothing was sent; your message is back");
    let back = window.picture();
    assert!(back.contains("hello"), "{back}");
    assert!(!back.contains("Use it anyway"), "{back}");
    window.types_until("\r", "Use it anyway");
    assert_eq!(proxy.asked(), Vec::<String>::new(), "sent before the yes");

    // The yes sends it, to its route's host alone (the transport asks again
    // after the refusal, as it does after any connection that failed), and is
    // written into the user's own file.
    window.types("\r");
    let reached = proxy.reached(1);
    assert!(!reached.is_empty(), "{}", window.picture());
    assert!(
        reached.iter().all(|host| host == warning::GEMINI_HOST),
        "{reached:?}"
    );
    assert!(
        warning::said(&window).contains("key:google"),
        "{}",
        warning::said(&window)
    );
}

#[test]
fn the_question_stands_at_forty_and_eighty_columns_in_either_glyph_set() {
    for (columns, ascii) in [(40, false), (40, true), (80, true)] {
        let proxy = warning::Proxy::new();
        let glyphs = if ascii {
            warning::GOOGLE.replace(
                "  \"updates\"",
                "  \"output\": {\"glyphs\": \"ascii\"},\n  \"updates\"",
            )
        } else {
            warning::GOOGLE.to_owned()
        };
        let case = format!("warning-{columns}-{ascii}");
        let mut window = warning::through(&case, (columns, 30), &glyphs, &proxy, (&[], None));
        window.types_until("hello\r", "Use it anyway");
        let picture = window.picture();
        assert!(picture.contains("Gemini API terms, 30 Sep"), "{picture}");
        let set = if ascii { "ascii" } else { "unicode" };
        insta::assert_snapshot!(format!("warning_at_send_{columns}_{set}"), picture);
        assert_eq!(proxy.asked(), Vec::<String>::new());
    }
}

#[test]
fn a_yes_holds_for_the_next_run_on_the_same_home() {
    let first = warning::Proxy::new();
    let mut window = warning::through(
        "warning-once",
        (80, 30),
        warning::GOOGLE,
        &first,
        (&[], None),
    );
    window.types_until("hello\r", "Use it anyway");
    window.types("\r");
    assert!(
        first
            .reached(1)
            .iter()
            .all(|host| host == warning::GEMINI_HOST)
    );
    let document = warning::said(&window);
    assert!(document.contains("key:google"), "{document}");

    // Another key in the variable is the same route: the yes is to the
    // vendor's terms on that route, not to one key.
    let second = warning::Proxy::new();
    let home = window.home();
    let mut again = Watched::launched(
        "warning-once-again",
        80,
        30,
        &watched::Launch {
            document: &document,
            env: &[
                ("GEMINI_API_KEY", "another-fabricated-gemini-key"),
                ("HTTPS_PROXY", &second.address),
            ],
            args: &[],
            home: Some(&home),
        },
    );
    again.types("again\r");
    assert!(
        !again.picture().contains("Use it anyway"),
        "{}",
        again.picture()
    );
    let reached = second.reached(1);
    assert!(
        !reached.is_empty() && reached.iter().all(|host| host == warning::GEMINI_HOST),
        "{reached:?}"
    );
}

/// A yes given at `/login` is written down with the key it stored, so the
/// first send after it goes without asking again.
#[test]
fn a_yes_given_at_login_is_kept_with_the_key_and_the_send_after_it_asks_nothing() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-login-stored",
        (80, 30),
        warning::GOOGLE,
        &proxy,
        (&[], None),
    );
    window.types_until("/login google\r", "Use it anyway");
    window.types_until("\r", "Google API key");
    window.types_until("fabricated-google-key-never-sent", "enter to save");
    window.types_until("\r", "login successful");
    assert!(
        warning::said(&window).contains("key:google"),
        "{}",
        warning::said(&window)
    );
    assert_eq!(proxy.asked(), Vec::<String>::new(), "a login sends nothing");

    window.types("hello\r");
    let reached = proxy.reached(1);
    assert!(
        !window.picture().contains("Use it anyway"),
        "{}",
        window.picture()
    );
    assert!(
        !reached.is_empty() && reached.iter().all(|host| host == warning::GEMINI_HOST),
        "{reached:?}"
    );
}

#[test]
fn a_route_reached_by_the_command_line_is_asked_about_before_the_first_send() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-flag",
        (80, 30),
        warning::NOTHING_CHOSEN,
        &proxy,
        (&["--model", "google/gemini-3.8-flash"], None),
    );
    window.types_until("hello\r", "Use it anyway");
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn no_room_for_the_question_keeps_the_message_and_sends_nothing() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-short",
        (80, 12),
        warning::GOOGLE,
        &proxy,
        (&[], None),
    );
    window.types_until("hello\r", "make the window taller and send again");
    let picture = window.picture();
    assert!(picture.contains("hello"), "{picture}");
    assert_eq!(proxy.asked(), Vec::<String>::new());

    let mut window = warning::through(
        "warning-short-login",
        (80, 12),
        warning::NOTHING_CHOSEN,
        &proxy,
        (&[], None),
    );
    window.types_until(
        "/login kimi code kimi.ai\r",
        "make the window taller and choose again",
    );
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn choosing_a_model_of_a_warned_route_asks_first_and_going_back_returns_to_it() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-model",
        (80, 30),
        warning::GOOGLE,
        &proxy,
        (&[], None),
    );
    window.types("/model\r");
    window.types("gemini-3.7");
    window.types_until(
        "\r",
        "Takes this choice; this route is not asked about again",
    );

    window.types_until("\x1b", "› gemini-3.7-flash");
    let shelf = window.picture();
    assert!(!shelf.contains("Use it anyway"), "{shelf}");

    window.types_until("\r", "Use it anyway");
    window.types_until("\r", "gemini-3.7-flash");
    assert!(
        warning::said(&window).contains("key:google"),
        "{}",
        warning::said(&window)
    );
    assert_eq!(
        proxy.asked(),
        Vec::<String>::new(),
        "a choice sends nothing"
    );
}

#[test]
fn a_warned_row_reached_by_words_asks_first_and_going_back_cancels() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-login-words",
        (80, 30),
        warning::NOTHING_CHOSEN,
        &proxy,
        (&[], None),
    );
    window.types_until("/login google\r", "Takes this choice");
    window.types_until("\x1b", "cancelled, nothing signed in");
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn a_sign_in_waits_for_the_yes_and_a_sign_in_that_fails_records_none() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-sign-in",
        (80, 30),
        warning::NOTHING_CHOSEN,
        &proxy,
        (&[], None),
    );
    window.types_until("/login kimi code kimi.ai\r", "Use it anyway");
    let asked = window.picture();
    assert!(asked.contains("Kimi Code · kimi.ai"), "{asked}");
    assert!(
        asked.contains("kimi.ai terms of service, 30 Sep 2026"),
        "{asked}"
    );
    assert_eq!(
        proxy.asked(),
        Vec::<String>::new(),
        "a sign-in began before the yes"
    );

    // The yes lets the sign-in's first request go, and the proxy refuses it,
    // so the sign-in fails and nothing is written down.
    window.types("\r");
    assert_eq!(
        proxy.reached(1),
        ["auth.kimi.ai:443"],
        "{}",
        window.picture()
    );
    assert!(
        !warning::said(&window).contains("kimi.ai"),
        "{}",
        warning::said(&window)
    );

    // Asked again the next time the row is chosen.
    window.types_until("\x1b", "");
    window.types_until("/login kimi code kimi.ai\r", "Use it anyway");
}

#[test]
fn a_credential_taken_out_takes_its_yes_and_the_question_stands_again() {
    let proxy = warning::Proxy::new();
    let earlier = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-warning-logout-home",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&earlier);
    std::fs::create_dir_all(&earlier).expect("a home to start from");
    let store =
        r#"{"version":2,"keys":{"google":"fabricated-stored-google-key"},"subscriptions":{}}"#;
    std::fs::write(earlier.join("auth.json"), store).expect("a store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            earlier.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("an owner-only store");
    }

    let mut window = warning::through(
        "warning-logout",
        (80, 30),
        warning::GOOGLE_SAID,
        &proxy,
        (&[], Some(&earlier)),
    );
    let _ = std::fs::remove_dir_all(&earlier);
    window.types_until(
        "/logout google\r",
        "removed the stored credential for google",
    );
    assert!(
        !warning::said(&window).contains("key:google"),
        "{}",
        warning::said(&window)
    );

    // The key from the environment still serves the route, and it is asked
    // about again.
    window.types_until("hello\r", "Use it anyway");
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

/// Taking out a stored key of a provider nobody is asking takes its yes, and
/// a key from the environment still serves the same route: choosing that
/// provider at `/model` asks about the route again before it is taken.
#[test]
fn a_provider_logged_out_while_another_answers_is_asked_about_again_at_model() {
    let proxy = warning::Proxy::new();
    let earlier = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-warning-logout-other-home",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&earlier);
    std::fs::create_dir_all(&earlier).expect("a home to start from");
    let store =
        r#"{"version":2,"keys":{"google":"fabricated-stored-google-key"},"subscriptions":{}}"#;
    std::fs::write(earlier.join("auth.json"), store).expect("a store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            earlier.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("an owner-only store");
    }
    let document = concat!(
        "{\n",
        "  \"sandbox\": {\"enabled\": false},\n",
        "  \"updates\": {\"check\": \"never\"},\n",
        "  \"provider\": \"anthropic\",\n",
        "  \"providers\": {\"anthropic\": {\"model\": \"claude-sonnet-5\"}},\n",
        "  \"contentUse\": {\"accepted\": [\"key:google\"]}\n",
        "}\n"
    );
    let mut window = Watched::launched(
        "warning-logout-other",
        80,
        30,
        &watched::Launch {
            document,
            env: &[
                ("ANTHROPIC_API_KEY", "fabricated-anthropic-key-never-sent"),
                ("GEMINI_API_KEY", warning::KEY),
                ("HTTPS_PROXY", &proxy.address),
            ],
            args: &[],
            home: Some(&earlier),
        },
    );
    let _ = std::fs::remove_dir_all(&earlier);
    window.types_until(
        "/logout google\r",
        "removed the stored credential for google",
    );
    assert!(
        !warning::said(&window).contains("key:google"),
        "{}",
        warning::said(&window)
    );

    window.types("/model\r");
    window.types("gemini-3.7");
    window.types_until("\r", "Takes this choice");
    assert_eq!(
        proxy.asked(),
        Vec::<String>::new(),
        "a choice sends nothing"
    );
}

/// Two routes with no yes hold one origin: a sign-in's, and the one a
/// `baseUrl` there answers for. Each is asked about before the send, and the
/// send goes once both have their yes.
#[test]
fn a_send_two_routes_hold_asks_about_each_before_it_goes() {
    let proxy = warning::Proxy::new();
    let mut window = two_routes_at_one_origin("warning-two", &proxy);

    window.types_until("hello\r", "Use it anyway");
    let first = window.picture();
    window.types_until("\r", the_other(&first));
    assert!(window.picture().contains("Use it anyway"), "{first}");
    assert_eq!(
        proxy.reached(1),
        Vec::<String>::new(),
        "sent before the second yes"
    );

    window.types("\r");
    let reached = proxy.reached(1);
    assert!(
        !reached.is_empty() && reached.iter().all(|host| host == "api.kimi.com:443"),
        "{reached:?} {}",
        window.picture()
    );
    let said = warning::said(&window);
    assert!(
        said.contains("key:moonshot") && said.contains("subscription:moonshot"),
        "{said}"
    );
}

/// The same two routes, reached by a choice at `/model`: each is asked about
/// before the choice is taken, and choosing sends nothing.
#[test]
fn a_choice_two_routes_hold_asks_about_each_before_it_is_taken() {
    let proxy = warning::Proxy::new();
    let mut window = two_routes_at_one_origin("warning-two-model", &proxy);

    window.types("/model\r");
    window.types("claude-sonnet-5");
    window.types_until("\r", "Takes this choice");
    let first = window.picture();
    window.types_until("\r", the_other(&first));
    assert!(window.picture().contains("Takes this choice"), "{first}");

    window.types("\r");
    let said = within(|| {
        let said = warning::said(&window);
        (said.contains("key:moonshot") && said.contains("subscription:moonshot")).then_some(said)
    })
    .unwrap_or_else(|| warning::said(&window));
    assert!(
        said.contains("key:moonshot") && said.contains("subscription:moonshot"),
        "{said}\n{}",
        window.picture()
    );
    assert!(
        !window.picture().contains("Takes this choice"),
        "{}",
        window.picture()
    );
    assert_eq!(
        proxy.asked(),
        Vec::<String>::new(),
        "a choice sends nothing"
    );
}

/// The name of whichever of the two routes at one origin `first` does not
/// stand, which only the second question draws.
fn the_other(first: &str) -> &'static str {
    if first.contains("Kimi Code · kimi.com") {
        "MoonshotAI · kimi.com"
    } else {
        "Kimi Code · kimi.com"
    }
}

/// What `seen` answers within ten seconds of asking, polled, or `None`: for
/// what is read afresh each time, such as a file, and never a picture, which
/// changes only when keys are typed.
fn within<T>(mut seen: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(seen) = seen() {
            return Some(seen);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// crucible over a stored Kimi Code kimi.com sign-in and Anthropic answering
/// through a `baseUrl` at the same origin, neither route said yes to.
fn two_routes_at_one_origin(case: &str, proxy: &warning::Proxy) -> Watched {
    let earlier = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-{case}-home",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&earlier);
    std::fs::create_dir_all(&earlier).expect("a home to start from");
    let store = r#"{"version":2,"keys":{},"subscriptions":{"moonshot":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;
    std::fs::write(earlier.join("auth.json"), store).expect("a store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            earlier.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("an owner-only store");
    }
    let document = concat!(
        "{\n",
        "  \"sandbox\": {\"enabled\": false},\n",
        "  \"updates\": {\"check\": \"never\"},\n",
        "  \"provider\": \"anthropic\",\n",
        "  \"providers\": {\"anthropic\": {\"model\": \"claude-sonnet-5\", ",
        "\"baseUrl\": \"https://api.kimi.com/coding/v1\"}}\n",
        "}\n"
    );
    let window = Watched::launched(
        case,
        80,
        30,
        &watched::Launch {
            document,
            env: &[
                ("ANTHROPIC_API_KEY", "fabricated-anthropic-key-never-sent"),
                ("HTTPS_PROXY", &proxy.address),
            ],
            args: &[],
            home: Some(&earlier),
        },
    );
    let _ = std::fs::remove_dir_all(&earlier);
    window
}

#[test]
fn a_base_url_crucible_recognises_is_asked_about_and_any_other_is_sent_to() {
    for (at, base, shown) in [
        (
            0,
            "https://api.moonshot.ai/v1",
            Some("Kimi open platform · api.moonshot.ai"),
        ),
        (
            1,
            "https://API.Kimi.ai:443/coding/v1",
            Some("MoonshotAI · kimi.ai"),
        ),
        (2, "https://gateway.example/v1", None),
        // A plan key row's address answers for that row, whichever
        // provider's `baseUrl` holds it.
        (
            3,
            "https://coding.dashscope.aliyuncs.com/v1",
            Some("Qwen Coding Plan · aliyun.com"),
        ),
    ] {
        let proxy = warning::Proxy::new();
        let document = warning::based(base);
        let case = format!("warning-base-{at}");
        let mut window = warning::through(&case, (80, 30), &document, &proxy, (&[], None));
        if let Some(shown) = shown {
            window.types_until("hello\r", "Use it anyway");
            assert!(window.picture().contains(shown), "{}", window.picture());
            assert_eq!(proxy.asked(), Vec::<String>::new(), "{base}");
        } else {
            window.types("hello\r");
            assert!(
                !window.picture().contains("Use it anyway"),
                "{}",
                window.picture()
            );
            let reached = proxy.reached(1);
            assert!(
                !reached.is_empty() && reached.iter().all(|host| host == "gateway.example:443"),
                "{reached:?}"
            );
        }
    }
}

#[test]
fn a_start_that_takes_a_second_credential_out_takes_its_yes_and_asks_again() {
    // What a roll back to 0.43.3 can leave: a kimi.ai sign-in said yes to, and
    // a kimi.com key 0.43.3 stored beside it. The start keeps the key and takes
    // the sign-in out, and its yes with it, before the store is written.
    let proxy = warning::Proxy::new();
    let earlier = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-warning-settle-home",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&earlier);
    std::fs::create_dir_all(&earlier).expect("a home to start from");
    let store = r#"{"version":2,"keys":{"moonshot":"fabricated-kimi-com-key"},"subscriptions":{"moonshot@kimi.ai":{"access_token":"fabricated-access","refresh_token":"fabricated-refresh","details":{},"expires_at":4102444800,"refreshed_at":1790000000}}}"#;
    std::fs::write(earlier.join("auth.json"), store).expect("a store");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            earlier.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("an owner-only store");
    }
    let document = warning::based("https://api.kimi.com/coding/v1").replace(
        "\n}\n",
        ",\n  \"contentUse\": {\"accepted\": [\"subscription:moonshot@kimi.ai\"]}\n}\n",
    );

    let mut window = warning::through(
        "warning-settle",
        (80, 30),
        &document,
        &proxy,
        (&[], Some(&earlier)),
    );
    let _ = std::fs::remove_dir_all(&earlier);
    assert!(
        !warning::said(&window).contains("subscription:moonshot@kimi.ai"),
        "{}",
        warning::said(&window)
    );
    window.types_until("hello\r", "Use it anyway");
    assert!(
        window.picture().contains("MoonshotAI · kimi.com"),
        "{}",
        window.picture()
    );
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn a_session_picked_up_on_a_warned_route_is_asked_about_before_it_sends() {
    // The first run said yes and sent, so there is a session on the route; the
    // second picks it up from a file with no yes in it, and asks again before
    // anything goes.
    let first = warning::Proxy::new();
    let mut window = warning::through(
        "warning-resume",
        (80, 30),
        warning::GOOGLE,
        &first,
        (&[], None),
    );
    window.types_until("hello\r", "Use it anyway");
    window.types("\r");
    assert!(!first.reached(1).is_empty());
    // Kept aside: the case's directory goes with the window, and the session
    // belongs to the workspace path under it, so the second run takes the
    // same case name and a copy of the home.
    let kept = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-warning-resume-kept",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&kept);
    copy_tree(&window.home(), &kept);
    drop(window);

    let second = warning::Proxy::new();
    let mut again = warning::through(
        "warning-resume",
        (80, 30),
        warning::GOOGLE,
        &second,
        (&["--continue"], Some(&kept)),
    );
    let _ = std::fs::remove_dir_all(&kept);
    again.types_until("again\r", "Use it anyway");
    assert_eq!(second.asked(), Vec::<String>::new());
}

#[test]
fn the_openai_plan_row_asks_with_its_condition_and_going_back_returns_to_its_list() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-openai",
        (80, 30),
        warning::NOTHING_CHOSEN,
        &proxy,
        (&[], None),
    );
    window.types_until("/login\r", "Provide your own API key");
    takes(&mut window, "Your account with subscription");
    window.types_until("", "Kimi Code · kimi.com");
    takes(&mut window, "OpenAI");
    window.types_until("", "Use it anyway");
    let asked = window.picture();
    assert!(
        asked.contains("On Free, Plus and Pro, OpenAI may use what you send"),
        "{asked}"
    );
    assert!(
        asked.contains("Takes this choice; this route is not asked about again"),
        "{asked}"
    );
    insta::assert_snapshot!("question_at_openai_plan_80", asked);

    // Go back: the list again, with the mark on the row it came from.
    window.types_until("\x1b", "Kimi Code · kimi.com");
    let list = window.picture();
    assert!(
        list.lines()
            .any(|row| row.trim_matches('|').trim_end() == "› OpenAI"),
        "{list}"
    );
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn with_input_and_output_redirected_a_warned_route_ends_the_run_and_sends_nothing() {
    use std::io::Write as _;

    let proxy = warning::Proxy::new();
    let scratch = std::env::temp_dir().join(format!(
        "crucible-whole-screen-{}-warning-redirected",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    let home = scratch.join("home");
    let work = scratch.join("work");
    std::fs::create_dir_all(&home).expect("a home");
    std::fs::create_dir_all(&work).expect("a workspace");
    std::fs::write(home.join("config.json"), warning::GOOGLE).expect("a configuration file");

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_crucible"))
        .current_dir(&work)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &scratch)
        .env("CRUCIBLE_CODE_HOME", &home)
        .env("GEMINI_API_KEY", warning::KEY)
        .env("HTTPS_PROXY", proxy.address())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("crucible starts");
    child
        .stdin
        .take()
        .expect("its input")
        .write_all(b"hello\n")
        .expect("a line written");
    let ended = child.wait_with_output().expect("crucible ends");
    let _ = std::fs::remove_dir_all(&scratch);

    let said = String::from_utf8_lossy(&ended.stderr);
    assert_eq!(ended.status.code(), Some(1), "{said}");
    assert!(
        said.contains("On unpaid quota, Google uses what you send"),
        "{said}"
    );
    assert!(said.contains("answer it once in a terminal"), "{said}");
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

/// Copies the tree at `from` into `to`, file by file.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("a directory to copy into");
    for entry in std::fs::read_dir(from).expect("a tree to copy").flatten() {
        let into = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &into);
        } else {
            std::fs::copy(entry.path(), &into).expect("a file copied");
        }
    }
}

#[test]
fn room_asked_for_on_a_warned_route_is_asked_about_and_going_back_says_nothing_was_sent() {
    let proxy = warning::Proxy::new();
    let mut window = warning::through(
        "warning-compact",
        (80, 30),
        warning::GOOGLE,
        &proxy,
        (&[], None),
    );
    window.types_until("/compact\r", "Use it anyway");
    window.types_until("\x1b", "nothing was sent");
    let picture = window.picture();
    assert!(!picture.contains("your message is back"), "{picture}");
    assert_eq!(proxy.asked(), Vec::<String>::new());
}

#[test]
fn the_fast_panel_stands_over_a_model_with_a_fast_form() {
    // Under a key and under a sign-in, whose prices differ, in both glyph sets.
    let credentials = [
        ("key", "google", "gemini-3.8-flash"),
        ("sign_in", "openai", "gpt-5.6-sol"),
    ];
    for (credential, provider, model) in credentials {
        for (glyphs, ascii) in [("unicode", false), ("ascii", true)] {
            for columns in [40, 80] {
                let case = format!("fast-{credential}-{glyphs}-{columns}");
                let document = fast::document(provider, model, ascii);
                let home = (credential == "sign_in").then(|| fast::signed_in_home(&case));
                let mut window = Watched::launched(
                    &case,
                    columns,
                    24,
                    &watched::Launch {
                        document: &document,
                        env: &[("GEMINI_API_KEY", fast::KEY)],
                        args: &[],
                        home: home.as_deref(),
                    },
                );
                if let Some(home) = &home {
                    let _ = std::fs::remove_dir_all(home);
                }
                window.types_until("/fast\r", "enter to choose");

                let picture = window.picture();
                assert!(picture.contains("Standard"), "{picture}");
                assert!(picture.contains("Fast"), "{picture}");
                assert!(!picture.contains("fabricated"), "{picture}");
                insta::assert_snapshot!(
                    format!("fast_panel_{credential}_{glyphs}_{columns}"),
                    picture
                );
            }
        }
    }
}

#[test]
fn a_window_too_short_for_the_fast_panel_is_given_the_lines_to_type() {
    // Eight rows hold the two lines and not the panel.
    let document = fast::document("google", "gemini-3.8-flash", false);
    let mut window = Watched::launched(
        "fast-short",
        80,
        24,
        &watched::Launch {
            document: &document,
            env: &[("GEMINI_API_KEY", fast::KEY)],
            args: &[],
            home: None,
        },
    );
    window.resize(80, 8);
    window.types_until("/fast\r", "/fast off");

    let picture = window.picture();
    assert!(picture.contains("/fast on"), "{picture}");
    assert!(picture.contains("75-100% more than Standard"), "{picture}");
    assert!(!picture.contains("enter to choose"), "{picture}");
    insta::assert_snapshot!("fast_lines_where_no_panel_fits", picture);
}

// The models pane under a credential that serves fewer than its provider
// offers, and the note a warned model carries.

/// `/model` over a home holding a `ChatGPT` sign-in, `columns` wide, narrowed
/// to `OpenAI` in the providers pane and taken back in the models pane.
fn signed_in_models(columns: u16) -> String {
    let case = format!("model-signed-in-{columns}");
    let document = fast::document("openai", "gpt-6-sol", false);
    let home = fast::signed_in_home(&case);
    let mut window = Watched::launched(
        &case,
        columns,
        24,
        &watched::Launch {
            document: &document,
            env: &[],
            args: &[],
            home: Some(&home),
        },
    );
    let _ = std::fs::remove_dir_all(&home);

    window.types_until("/model\r", "Search");
    // Tab alone changes only how the mark is drawn, so it goes with the
    // first step down; the closing row stands once `OpenAI` is marked.
    window.types("\t\x1b[B");
    for _ in 0..24 {
        if window.picture().contains("1 more with an API key") {
            break;
        }
        window.types("\x1b[B");
    }
    // Back to the models, which are what the heading names.
    window.types_until("\t", "openai · ChatGPT sign-in");
    window.picture()
}

#[test]
fn a_sign_in_heads_the_models_it_serves_and_says_what_a_key_would_add() {
    for columns in [40, 80] {
        let picture = signed_in_models(columns);

        // At forty the heading takes the frame's top row from the providers,
        // and the pane keeps its models and its closing row, which never
        // takes the mark.
        assert!(picture.contains("openai · ChatGPT sign-in"), "{picture}");
        assert!(picture.contains("1 more with an API key"), "{picture}");
        assert!(picture.contains("gpt-6.1-sol"), "{picture}");
        assert!(!picture.contains("gpt-5.5 "), "{picture}");
        assert!(!picture.contains("› 1 more"), "{picture}");
        insta::assert_snapshot!(format!("model_signed_in_{columns}"), picture);
    }
}

#[test]
fn a_contributor_model_says_trains_where_its_standard_twin_says_nothing() {
    let vendor = Vendor::answering("Two plus two is four.");
    let mut window = Watched::keyless("model-trains", 80, 24, &vendor);

    window.types_until("/model\r", "Search");
    window.types_until("muse", "trains");
    let picture = window.picture();

    // The names are cut at the column, so the rows are told apart by the part
    // that is drawn: the two contributor models say `trains`, and the two
    // standard ones say nothing.
    let marked: Vec<&str> = picture
        .lines()
        .filter(|row| row.contains("trains"))
        .collect();
    assert_eq!(marked.len(), 2, "{picture}");
    assert!(
        marked.iter().all(|row| row.contains("contribu")),
        "{picture}"
    );
    insta::assert_snapshot!("model_trains_80", picture);
}

// `/context`: how the window of the next request is spent.

/// The percentage the prompt line says is left of the window.
fn window_left(picture: &str) -> Option<String> {
    let (before, _) = picture.split_once("% window left")?;
    let figure = before.rsplit(|cell: char| !cell.is_ascii_digit()).next()?;
    Some(format!("{figure}%"))
}

/// The share `/context` prints on its free row.
fn free_share(picture: &str) -> Option<String> {
    let row = picture.lines().find(|line| line.contains(" free "))?;
    row.trim_end_matches('|')
        .split_whitespace()
        .last()
        .map(str::to_owned)
}

#[test]
fn context_stands_over_a_fresh_session_and_closes_on_escape() {
    for columns in [80, 40] {
        let vendor = Vendor::answering("Hello.");
        let mut window =
            Watched::answering(&format!("context-fresh-{columns}"), columns, 24, &vendor);
        let before = window.picture();
        window.types_until("/context\r", "esc to close");

        let picture = window.picture();
        assert!(picture.contains("Context"), "{picture}");
        assert!(picture.contains("system prompt"), "{picture}");
        assert_eq!(free_share(&picture), window_left(&before), "{picture}");
        insta::assert_snapshot!(format!("context_fresh_{columns}"), picture);

        window.types_until("\x1b", "ask mode on");
        let closed = window.picture();
        assert!(!closed.contains("esc to close"), "{closed}");
    }
}

#[test]
fn context_stands_over_a_long_session_with_what_the_prompt_line_says_is_left() {
    let mut window = two_long_turns("context-long", 80, 24);
    let before = window.picture();
    window.types_until("/context\r", "esc to close");

    let picture = window.picture();
    assert!(window_left(&before).is_some(), "{before}");
    assert_eq!(free_share(&picture), window_left(&before), "{picture}");
    insta::assert_snapshot!("context_long_80", picture);
}

#[test]
fn context_stands_over_a_running_turn_with_the_figures_it_last_carried() {
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("context-mid-turn", 80, 24, &vendor, "bash(*)");
    window.types_and_catches("start it\r", HELD_LAST_WORD);
    let before = window.picture();

    window.types_and_catches("/context\r", "esc to close");
    let picture = window.picture();
    assert!(window_left(&before).is_some(), "{before}");
    assert_eq!(free_share(&picture), window_left(&before), "{picture}");
    insta::assert_snapshot!("context_mid_turn_80", on_the_first_beat(&picture));
}

#[test]
fn context_taller_than_the_window_stands_and_scrolls_with_the_arrows() {
    // A 40x12 window is shorter than the panel's 15 rows: the panel stands in
    // the window's rows with its body cut short, and the arrows move the body.
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::answering("context-short-40", 40, 12, &vendor);
    window.types_until("/context\r", "Context · ");

    let stood = window.picture();
    assert!(stood.contains(PANEL_SCROLLS), "{stood}");
    assert!(stood.contains("↓ 4 more"), "{stood}");
    assert_eq!(stood_rows(&stood), 12, "{stood}");
    insta::assert_snapshot!("context_short_40", stood);

    window.types_until("\x1b[B", "↓ 3 more");
    let scrolled = window.picture();
    assert!(!scrolled.contains("Context · "), "{scrolled}");
    assert_eq!(stood_rows(&scrolled), 12, "{scrolled}");

    window.types_until("\x1b[A", "Context · ");
    window.types_until("\x1b", "ask mode on");
    let closed = window.picture();
    assert!(!closed.contains("esc to close"), "{closed}");
}

#[test]
fn context_taller_than_half_the_window_stands_in_half_and_scrolls_in_native_mode() {
    // Native mode gives a panel half the window. The panel's 15 rows at 40
    // columns are cut to the 12 of a 24-row window rather than taking 15, and
    // to the 7 of a 14-row window rather than being printed.
    for (rows, below) in [(24, 4), (14, 9)] {
        let vendor = Vendor::answering("Hello.");
        let mut window = Watched::native(&format!("context-native-40x{rows}"), 40, rows, &vendor);
        window.types_until("/context\r", "Context · ");

        let stood = window.picture();
        assert!(stood.contains(PANEL_SCROLLS), "{stood}");
        assert!(stood.contains(&format!("↓ {below} more")), "{stood}");
        assert_eq!(stood_rows(&stood), usize::from(rows / 2), "{stood}");

        window.types_until("\x1b[B", &format!("↓ {} more", below - 1));
        let scrolled = window.picture();
        assert!(!scrolled.contains("Context · "), "{scrolled}");
        assert_eq!(stood_rows(&scrolled), usize::from(rows / 2), "{scrolled}");

        window.types_until("\x1b", "ask mode on");
        window.assert_never_alternate();
        let closed = window.picture();
        assert!(!closed.contains("esc to close"), "{closed}");
    }
}

// `/usage`: what the session has used, and the plan windows its vendor reported.

/// `picture` with the two figures `/usage` reads off the wall clock written as
/// `Ns`, the rest of each row as it was.
///
/// How long a case's requests took, and how long since its session started,
/// are this machine's seconds today; what the case is about is the rows
/// around them.
fn timeless(picture: &str) -> String {
    picture
        .split('\n')
        .map(|line| {
            let Some(inner) = line.strip_prefix('|').and_then(|it| it.strip_suffix('|')) else {
                return line.to_owned();
            };
            let Some(label) = ["API time", "Wall time"]
                .into_iter()
                .find(|label| inner.trim_start().starts_with(label))
            else {
                return line.to_owned();
            };
            let Some(at) = inner.find(label) else {
                return line.to_owned();
            };
            let after = inner.get(at + label.len()..).unwrap_or_default();
            let gap = after.len() - after.trim_start().len();
            let kept = inner.get(..at + label.len() + gap).unwrap_or_default();
            let width = inner.chars().count();
            format!("|{:<width$}|", format!("{kept}Ns"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn usage_after_a_turn_on_a_key_says_no_limits_were_reported_and_closes_on_escape() {
    for columns in [80, 40] {
        let vendor = Vendor::answering("Hello.");
        let mut window = Watched::answering(&format!("usage-key-{columns}"), columns, 30, &vendor);
        window.types_until("say hello\r", "Hello.");
        window.types_until("/usage\r", "esc to close");

        let picture = window.picture();
        // A key is named for what it is, not for where it was read from.
        assert!(picture.contains("Usage · anthropic · API key"), "{picture}");
        assert!(!picture.contains("ANTHROPIC_API_KEY"), "{picture}");
        assert!(picture.contains("Plan limits"), "{picture}");
        assert!(picture.contains("limits not reported"), "{picture}");
        assert!(!picture.contains("$0.00"), "{picture}");
        insta::assert_snapshot!(format!("usage_key_{columns}"), timeless(&picture));

        window.types_until("\x1b", "ask mode on");
        let closed = window.picture();
        assert!(!closed.contains("esc to close"), "{closed}");
    }
}

#[test]
fn usage_stands_over_a_running_turn_with_the_figures_it_last_posted() {
    let vendor = a_turn_still_running();
    let mut window = Watched::allowing("usage-mid-turn", 80, 30, &vendor, "bash(*)");
    window.types_and_catches("start it\r", HELD_LAST_WORD);

    window.types_and_catches("/usage\r", "esc to close");
    let picture = window.picture();
    assert!(picture.contains("Plan limits"), "{picture}");
    assert!(!picture.contains("can't"), "{picture}");
    // Counted while the turn runs, from what it posts: the session had asked
    // nothing before it.
    assert!(picture.contains("0 in · 4 out"), "{picture}");
    insta::assert_snapshot!("usage_mid_turn_80", timeless(&on_the_first_beat(&picture)));
}

/// The footer `/usage` and `/context` stand with where they are taller than
/// their room.
const PANEL_SCROLLS: &str = "esc to close · ↑↓ to see more";

/// The rows a panel such as `/usage` stands in on `picture`: from its rule to
/// its footer.
fn stood_rows(picture: &str) -> usize {
    let rows: Vec<&str> = picture.lines().filter(|row| row.starts_with('|')).collect();
    let rule = rows
        .iter()
        .position(|row| row.trim_matches('|').chars().all(|cell| cell == '─'))
        .unwrap_or_else(|| panic!("no rule over the panel:\n{picture}"));
    let footer = rows
        .iter()
        .position(|row| row.contains("esc to close"))
        .unwrap_or_else(|| panic!("no footer under the panel:\n{picture}"));
    footer + 1 - rule
}

#[test]
fn usage_taller_than_the_window_stands_and_scrolls_with_the_arrows() {
    // A 40x16 window is shorter than the panel's 20 rows at 40 columns, as a
    // 40x24 one is with two plan windows reported: the panel stands in the
    // window's rows with its body cut short, and the arrows move the body.
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::answering("usage-short-40", 40, 16, &vendor);
    window.types_until("say hello\r", "Hello.");
    window.types_until("/usage\r", "Usage · anthropic");

    let stood = window.picture();
    assert!(stood.contains(PANEL_SCROLLS), "{stood}");
    assert!(stood.contains("↓ 5 more"), "{stood}");
    assert_eq!(stood_rows(&stood), 16, "{stood}");
    insta::assert_snapshot!("usage_short_40", timeless(&stood));

    window.types_until("\x1b[B", "↓ 4 more");
    let scrolled = window.picture();
    assert!(!scrolled.contains("Usage · anthropic"), "{scrolled}");
    assert_eq!(stood_rows(&scrolled), 16, "{scrolled}");
    insta::assert_snapshot!("usage_short_40_scrolled", timeless(&scrolled));

    window.types_until("\x1b[A", "Usage · anthropic");
    window.types_until("\x1b", "ask mode on");
    let closed = window.picture();
    assert!(!closed.contains("esc to close"), "{closed}");
}

#[test]
fn usage_taller_than_half_the_window_stands_in_half_and_scrolls_in_native_mode() {
    // Native mode gives a panel half the window, 12 rows of 24, and the
    // panel's 20 rows at 40 columns are cut to them rather than taking 20.
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::native("usage-native-40", 40, 24, &vendor);
    window.types_until("say hello\r", "Hello.");
    window.types_until("/usage\r", "Usage · anthropic");

    let stood = window.picture();
    assert!(stood.contains(PANEL_SCROLLS), "{stood}");
    assert_eq!(stood_rows(&stood), 12, "{stood}");

    window.types_until("\x1b[B", "↓ 8 more");
    let scrolled = window.picture();
    assert!(!scrolled.contains("Usage · anthropic"), "{scrolled}");
    assert_eq!(stood_rows(&scrolled), 12, "{scrolled}");

    window.types_until("\x1b", "ask mode on");
    window.assert_never_alternate();
    let closed = window.picture();
    assert!(!closed.contains("esc to close"), "{closed}");
}

// `/settings`: what is in force, the settings a menu can change, and the
// session's usage, as three tabs of one panel.

/// `picture` with the session on the Status tab written as `#`s.
///
/// An id is minted from the clock and chance as the session opens, so the
/// Status tab shows a different one every run; what the case is about is the
/// row it stands in. The tab shows the id's first eight, which is the run of
/// eight hex digits after the label.
fn sessionless(picture: &str) -> String {
    const LABEL: &str = "Session";
    const SHOWN: usize = 8;
    picture
        .lines()
        .map(|row| {
            let Some((before, after)) = row.split_once(LABEL) else {
                return row.to_owned();
            };
            let gap = after.len() - after.trim_start().len();
            let (spaces, rest) = after.split_at(gap);
            match rest.get(..SHOWN) {
                Some(id) if gap > 0 && id.chars().all(|letter| letter.is_ascii_hexdigit()) => {
                    format!(
                        "{before}{LABEL}{spaces}{}{}",
                        "#".repeat(SHOWN),
                        rest.get(SHOWN..).unwrap_or_default()
                    )
                }
                _ => row.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn settings_opens_on_config_and_shows_each_tab_and_a_search() {
    for columns in [80, 40] {
        let vendor = Vendor::answering("Hello.");
        let case = format!("settings-tabs-{columns}");
        let mut window = Watched::answering(&case, columns, 30, &vendor);

        window.types_until("/settings\r", "esc to close");
        let config = window.picture();
        assert!(config.contains("Config"), "{config}");
        assert!(config.contains("Scroll rail"), "{config}");
        insta::assert_snapshot!(format!("settings_config_{columns}"), config);

        window.types_until("/cache", "cache ");
        let search = window.picture();
        assert!(search.contains("Cache retention"), "{search}");
        assert!(!search.contains("Scroll rail"), "{search}");
        insta::assert_snapshot!(format!("settings_search_{columns}"), search);

        // Escape clears the search; the left arrow is the tab before Config.
        window.types_until("\x1b", "Scroll rail");
        window.types_until("\x1b[D", "Permission mode");
        let status = window.picture();
        assert!(status.contains("Sandbox"), "{status}");
        insta::assert_snapshot!(format!("settings_status_{columns}"), sessionless(&status));

        window.types_until("\x1b[D", "Plan limits");
        let usage = window.picture();
        insta::assert_snapshot!(format!("settings_usage_{columns}"), timeless(&usage));

        window.types_until("\x1b", "ask mode on");
        let closed = window.picture();
        assert!(!closed.contains("esc to close"), "{closed}");
    }
}

#[test]
fn a_settings_toggle_folds_the_transcript_again_and_is_left_in_it() {
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::answering("settings-toggle", 80, 30, &vendor);
    window.types_until("say hello\r", "Hello.");

    window.types_until("/settings\r", "esc to close");
    // Down to the rail, six rows below the theme, and turn it off.
    window.types_until("\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B", "› Scroll rail");
    window.types_until("\r", "off");
    window.types_until("\x1b", "Scroll rail set to off");
    let picture = window.picture();
    insta::assert_snapshot!("settings_toggle_80", picture);
}

/// Turns the Send with row to `altEnter` through `/settings` and closes the
/// panel, between turns or over a running one.
fn sends_with_alt_enter(window: &mut Watched) {
    window.types_until("/settings\r", "esc to close");
    // Enter ends the search on the one row left; the second changes it.
    window.types_until("/send", "Send with");
    window.types_until("\r\r", "altEnter");
    window.types_until("\x1b", "› Theme");
    window.types_until("\x1b", "Send with set to altEnter");
}

/// Whether the prompt box holds `first` on one row and `second` on the next.
fn boxed_on_two_rows(picture: &str, first: &str, second: &str) -> bool {
    let rows: Vec<&str> = picture.lines().collect();
    rows.windows(2).any(|pair| {
        pair.first()
            .is_some_and(|row| row.contains(&format!("│ › {first} ")))
            && pair
                .get(1)
                .is_some_and(|row| row.contains(&format!("│   {second} ")))
    })
}

#[test]
fn a_send_key_changed_in_settings_is_the_one_return_obeys_next() {
    let vendor = Vendor::answering("Hello.");
    let mut window = Watched::answering("settings-send", 80, 30, &vendor);
    sends_with_alt_enter(&mut window);

    // Return now opens a line, so nothing is sent and no turn starts.
    window.types_until("say hello\rthere", "there");
    let held = window.picture();
    assert!(boxed_on_two_rows(&held, "say hello", "there"), "{held}");
    assert!(
        !held.lines().any(|row| row.starts_with("|› say hello")),
        "{held}"
    );
    assert!(!held.contains("Hello."), "{held}");
    insta::assert_snapshot!("settings_send_with_alt_enter_80", held);

    // And Alt+Return is the press that sends the two lines.
    window.types_until("\x1b\r", "Hello.");
    let sent = window.picture();
    assert!(sent.contains("› say hello"), "{sent}");
    assert!(sent.contains("  there"), "{sent}");
}

#[test]
fn a_send_key_changed_in_settings_over_a_running_turn_is_obeyed_once_it_closes() {
    let vendor = Vendor::holding("Still going.");
    let mut window = Watched::answering("settings-send-live", 80, 30, &vendor);
    window.types_and_catches("say it\r", "going.");
    sends_with_alt_enter(&mut window);

    window.types_until("one\rtwo", "two");
    let typed = window.picture();
    assert!(boxed_on_two_rows(&typed, "one", "two"), "{typed}");
    assert!(
        !typed.lines().any(|row| row.starts_with("|› one")),
        "{typed}"
    );
}
