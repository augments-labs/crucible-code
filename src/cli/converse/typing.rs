//! Reading a prompt as it is typed, inside the box it is typed into.
//!
//! The other way to read one is the loop's bounded buffered-line reader,
//! wherever there is no terminal. What this adds is everything that needs a
//! keystroke rather than a line: the box around what is being written, the mode
//! under it, and a window that follows the cursor along a line longer than the
//! screen.
//!
//! Raw mode is not entered here. The loop above holds it for the whole session,
//! because the box takes typing while a turn runs and a keyboard handed back
//! between turns could not do that. The rest follows from holding it: a
//! permission question is answered by a key rather than by a line the terminal
//! collected, and Ctrl-C arrives as a byte for this side to act on rather than
//! as a signal the terminal sends.
//!
//! Which leaves two keys with two jobs, and each of them keeps its job in both
//! loops. Ctrl-C belongs to the line: it throws away what is typed, and against
//! an empty one it offers to leave and leaves on a second press soon after —
//! the same in [`ask`] as in [`during`], because a key that meant one thing
//! between turns and another during one would have to be relearned at exactly
//! the moment there is something to lose. Esc belongs to whatever is standing:
//! nothing, between turns, and the turn itself while one runs.
//!
//! It is one of the two places the mode changes — `/mode` is the other — which
//! is why the runner is a parameter here. The mode is a fact about the session
//! and lives in the runner; this reads it to draw the row under the box and
//! steps it when the key that steps it arrives. Both of those happen between
//! turns, on the thread that draws, so there is one copy of it and no lock over
//! it: while a turn is away with the runner, [`during`] draws the mode it was
//! handed and steps nothing.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use crucible_core::{Cancel, Effort};
use crucible_runner::Runner;
use crucible_tui::{
    Caret, Editor, Glyphs, Key, Listed, Menu, Pressed, Prompt, Renderer, Row, Slot, Terminal,
    Typed, caret, characters, pressed, waiting,
};

use crate::cli::Fatal;
use crate::cli::kept::Kept;
use crate::cli::style::Style;

use super::command;
use super::expanding::{self, Standing};
use super::mode::tone;
use super::turning::Turning;
use super::{Prompts, Retained};

/// What the row under the box says after the mode, when pressing the key again
/// is all there is to do with it.
///
/// The one place keys are printed beside the thing they act on. It earns that
/// by being the only way anybody finds out the row is a control at all.
const CYCLE: &str = "(shift+tab to cycle)";

/// What the row under the status says once Ctrl-C has been pressed against an
/// empty line.
///
/// On screen from the first press rather than after the window has lapsed. The
/// press is the moment somebody is asking to leave, so it is the moment worth
/// answering — and by the time waiting too long could be noticed, the thing to
/// say would be that the offer had already gone.
const LEAVING: &str = "press ctrl+c again to leave";

/// What the row under the box says when finished prompts already fill their
/// retained-memory bound.
const QUEUED_LIMITED: &str = "typed-ahead prompts are limited to 64 lines and 1 MiB";

/// What the row under the box says when an edit would retain too much input.
const LIMITED: &str = "prompt is limited to 1 MiB";

/// What the row under the box says while a turn is running.
///
/// Two keys, both of which do something at that moment. The one that steps the
/// mode is left off, because the runner holding the mode is away with the turn;
/// so is Ctrl-C, which does here what it does at the prompt and so needs no
/// second teaching.
///
/// Esc is named twice on screen while a turn runs — here and in the third
/// segment of the row above the box — and the repetition is the point. That
/// segment is the first thing dropped on a narrow window, and the key that
/// stops a turn is the wrong thing for a narrow window to take away.
///
/// Built rather than written down, because the mark parting the two keys is
/// the setting's — the same one the row above the box parts its segments with,
/// two rows away.
fn interrupting(glyphs: Glyphs) -> String {
    format!("(enter queues it {} esc to interrupt)", glyphs.dot())
}

/// How long the second press has to arrive in.
///
/// Long enough to be a pair of presses somebody meant and short enough that it
/// cannot span a thought. It is the whole of what separates leaving from
/// clearing a line, so it is the window rather than a debounce: a first press
/// says what a second would do, and a second that arrives after this says it
/// again instead of acting on the first.
///
/// It used to be aimed at a Ctrl-C meant for a turn that had already finished,
/// which was a real hazard while that key stopped turns. It stops none now, so
/// the hazard points the other way: somebody reaching twice for the key that
/// used to stop a turn ends the session instead. This window is the whole of
/// what keeps that a pair of presses rather than any two, which is why it is
/// short and why every other key takes the offer back.
const TOGETHER: Duration = Duration::from_secs(2);

/// What reading a prompt produced.
pub(crate) enum Asked {
    /// A line was typed and finished.
    Said(String),
    /// The session is over: Ctrl-D on an empty line, or Ctrl-C twice against
    /// one.
    Ended,
    /// Ctrl+O: whatever the transcript cut down to a row is to be shown whole,
    /// and the box asked for again once it has been closed.
    ///
    /// The box is not written down on the way out. It stands in the live region
    /// and so does the view, so the one replaces the other and the line being
    /// typed is still there underneath when this comes back.
    ///
    /// Reported rather than acted on here, because the same key pressed while a
    /// turn runs opens the same view a row further down the screen. Both go
    /// through the one door at the top of the loop above.
    Expand,
    /// A click on this row of the record, which stands the one result that row
    /// offered to expand — or nothing, where it offered none.
    ///
    /// Which of the two it is, is not decided here. What was cut belongs to the
    /// loop above and so does the view, and asking this module to hold either of
    /// them to answer a click would put the transcript's half of the session
    /// inside the box's.
    Clicked(usize),
    /// There is nothing here to type into. The caller reads a line instead.
    Untyped,
}

/// What inserting one immediately ready character run changed.
struct Inserted {
    /// Whether the line changed and therefore its filtered list is stale.
    changed: bool,
    /// Whether at least one character crossed the retained-memory boundary.
    refused: bool,
    /// Whether the live box owes exactly one redraw for the whole run.
    redraw: bool,
    /// The structural event that ended the run, processed next.
    following: Option<Pressed>,
}

/// Inserts the bounded run beginning at `first` in one edit.
fn insert(editor: &mut Editor, first: char) -> Result<Inserted, Fatal> {
    let room = Editor::MAX_BYTES.saturating_sub(editor.text().len());
    let (text, refused, following) = characters(first, room)?.into_parts();

    let typed = editor.paste(&text);
    let changed = typed == Typed::Changed;
    let refused = refused || typed == Typed::Refused;

    Ok(Inserted {
        changed,
        refused,
        redraw: changed || refused,
        following,
    })
}

/// Reads one prompt, drawing it as it arrives.
pub(crate) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    runner: &mut Runner,
    editor: &mut Editor,
    keys: bool,
) -> Result<Asked, Fatal> {
    if !keys {
        return Ok(Asked::Untyped);
    }

    let glyphs = style.glyphs();

    let mut says = saying(runner);

    // When Ctrl-C was last pressed against an empty line, if it is still the
    // last key pressed. Taken by every other key, so the pair has to be two
    // presses in a row rather than two with a session between them.
    let mut leaving: Option<Instant> = None;

    // Whatever was typed while the last turn ran is already here, so the list a
    // slash opens has to be worked out from it rather than assumed empty.
    let mut open = Opened::filtered(editor.text(), glyphs);
    let mut above = draw(renderer, editor, style, &says, &open)?;

    let mut following = None;
    loop {
        let arrived = match following.take() {
            Some(arrived) => arrived,
            None => pressed()?,
        };

        // Whatever arrived, the offer to leave was made to the key after the
        // one that made it, and this is that key.
        let offered = leaving.take();
        says.asking = None;

        match arrived {
            // Redrawn rather than re-wrapped: the box was laid out for a width
            // the window no longer has, and the rows it left on screen are the
            // renderer's to take back before the new ones go down.
            Pressed::Resized => {
                renderer.resized()?;
                above = draw(renderer, editor, style, &says, &open)?;
            }

            // Handed back to the caller rather than answered here. What was
            // cut is the transcript's, this module is the box's, and the two
            // meet where the loop that owns both of them is.
            Pressed::Expand => return Ok(Asked::Expand),

            // Nothing is standing, so there is nothing to back out of and
            // nothing to explain — except the offer above, which is on screen
            // and has just been taken back.
            Pressed::Escape | Pressed::Explain | Pressed::Ignored => {
                if offered.is_some() {
                    above = draw(renderer, editor, style, &says, &open)?;
                }
            }

            // Two things a click can land on and one round trip to tell them
            // apart. On the line it is the move the arrows make one place at a
            // time, made in one go; above the box it is a row of the record,
            // which is the loop above's to answer because what was cut is
            // there. Anywhere else — the border, a blank row, the shell's own
            // output — it moves nothing, the same as a key that moved nothing.
            Pressed::Clicked { row, column } => {
                match landed(renderer, editor, &says, above, Pointed { row, column }) {
                    Landed::Record(at) => return Ok(Asked::Clicked(at)),
                    Landed::Line => above = draw(renderer, editor, style, &says, &open)?,
                    Landed::Nothing => {
                        if offered.is_some() {
                            above = draw(renderer, editor, style, &says, &open)?;
                        }
                    }
                }
            }

            // Through whatever the line has open above the box. With nothing
            // open, or at the end it is already on, the key costs no frame.
            Pressed::Up => {
                if open.up() || offered.is_some() {
                    above = draw(renderer, editor, style, &says, &open)?;
                }
            }
            Pressed::Down => {
                if open.down() || offered.is_some() {
                    above = draw(renderer, editor, style, &says, &open)?;
                }
            }

            // Stepping the mode on. Every step takes effect on the press: the
            // row under the box says which mode that landed in, and the same
            // key is what steps out of it again.
            Pressed::Cycle => {
                runner.cycle();

                says = saying(runner);
                above = draw(renderer, editor, style, &says, &open)?;
            }

            Pressed::Key(Key::Char(first)) => {
                let inserted = insert(editor, first)?;
                following = inserted.following;

                if inserted.changed {
                    open = Opened::filtered(editor.text(), glyphs);
                }
                if inserted.refused {
                    says.asking = Some(LIMITED);
                }
                if inserted.redraw || offered.is_some() {
                    above = draw(renderer, editor, style, &says, &open)?;
                }
            }

            Pressed::Key(key) => match editor.press(key) {
                // A key that moved nothing costs no frame, unless the offer
                // above is on screen and now stale. An arrow held down against
                // the end of a line is what the first half of that saves.
                Typed::Ignored => {
                    if offered.is_some() {
                        above = draw(renderer, editor, style, &says, &open)?;
                    }
                }
                Typed::Changed => {
                    open = Opened::filtered(editor.text(), glyphs);
                    above = draw(renderer, editor, style, &says, &open)?;
                }
                Typed::Refused => {
                    says.asking = Some(LIMITED);
                    above = draw(renderer, editor, style, &says, &open)?;
                }
                Typed::Submitted => return said(renderer, editor, &open, style),

                // Ctrl-C against a line with nothing on it. The first press
                // says what a second one would do; the second does it, so long
                // as it is the very next key and soon enough to be one gesture
                // with the first.
                Typed::Interrupted => {
                    if together(offered, Instant::now()) {
                        renderer.settle()?;
                        return Ok(Asked::Ended);
                    }

                    leaving = Some(Instant::now());
                    says.asking = Some(LEAVING);
                    above = draw(renderer, editor, style, &says, &open)?;
                }

                Typed::Ended => {
                    renderer.settle()?;
                    return Ok(Asked::Ended);
                }
            },
        }
    }
}

/// Whether a press at `now` is the second half of the one `offered`.
///
/// The clock is an argument rather than read here, which is what lets the
/// window be tested at all: the case worth pinning is the press that arrived
/// too late, and waiting for it in a test would put [`TOGETHER`] into how long
/// the suite takes.
fn together(offered: Option<Instant>, now: Instant) -> bool {
    offered.is_some_and(|since| now.duration_since(since) < TOGETHER)
}

/// What the rows under the box say, and the colour the box says it in.
///
/// The mode is built when the mode changes rather than when the box is drawn.
/// The box is redrawn on every keystroke and this is the same row until a key
/// changes the mode, so formatting it per frame would be work done to produce
/// the bytes that were already there.
#[derive(Clone)]
pub(super) struct Says {
    /// The mode, in the words somebody reads rather than the ones they type.
    mode: Cow<'static, str>,
    /// The keys that act on it, said quietly after.
    keys: Cow<'static, str>,
    /// Which model the next turn goes to, at the other end of the row.
    model: String,
    /// The vendor it is asked of, drawn before it.
    provider: &'static str,
    /// How hard it is being asked to think. `None` where no rung is in force.
    effort: Option<&'static str>,
    /// What the border and the sentence are both drawn in.
    tone: Slot,
    /// A row under that, for something waiting on the very next key. `None` in
    /// the ordinary state, where the mode is the last row there is.
    asking: Option<&'static str>,
}

/// The box as it stands under a turn, and where the cursor sits in it.
///
/// The same component in the same place rather than a row standing in for it.
/// A turn is the longest a session goes without a prompt on screen, and a box
/// that vanished for it would take the one fixed thing off the screen exactly
/// when output is scrolling past — so what a session looks like would depend on
/// whether it happened to be working.
///
/// It is not a picture of a box: the keys below are read while the turn runs,
/// so what is typed goes in and the cursor is where it is going.
///
/// Above it, the row that says the turn is running. Laid out after the box
/// rather than before it, because the box takes its share of the window first
/// and what is left is what decides whether that row is drawn at all.
pub(super) fn working<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &Editor,
    turning: &Turning,
    says: &Says,
    style: Style,
) -> (Vec<Row>, Caret) {
    let columns = renderer.columns();
    let prompt = writing(editor, says, Prompt::room(renderer.rows()));

    let mut boxed = prompt.rows(columns, style.glyphs());
    let mut caret = prompt.caret(columns);
    let room = renderer.rows().saturating_sub(boxed.len());

    let mut rows = turning.rows(columns, style, room);
    caret.row += rows.len();
    rows.append(&mut boxed);

    (rows, caret)
}

/// What the row under a running turn says.
///
/// Read before the runner leaves for the worker, because nothing on this side
/// can ask it anything while it is away — and nothing needs to: `/mode`,
/// `/model` and `/effort` are all lines typed between turns, so none of the
/// three facts here can change while the turn they describe is the one running.
pub(super) fn under(runner: &Runner, glyphs: Glyphs) -> Says {
    Says {
        keys: Cow::Owned(interrupting(glyphs)),
        ..saying(runner)
    }
}

/// Reads whatever the keyboard already has, and redraws the box if it moved.
///
/// Called between looks at the channel the turn reports on, so it never waits:
/// a key that has not arrived yet is one the next time round will find. A line
/// finished while the answer was still arriving moves into the queue the loop
/// above takes the next prompts from, unless the queue's bound is already met —
/// then the line stays in the box and the row under it says why.
///
/// The keys that mean something here are the ones that still do. Return
/// finishes a line, Esc asks the turn to stop, Ctrl+O stands the whole of what
/// the results so far were cut down to, a click on a row that offered to expand
/// stands that one result, Ctrl-C is the line's own — in raw mode the terminal
/// sends it rather than raising a signal, so it reaches the editor here exactly
/// as it does at the prompt — and the rest edit the line. While that view
/// stands it has all of them: it takes the rows the box has, so the box is not
/// on screen to be typed into and Esc closes the view rather than stopping the
/// turn behind it.
///
/// Stepping the mode is not among them: the runner that holds it is on
/// the worker thread for the length of the turn, and a key that moved the row
/// on screen and nothing else would be a lie about what the next tool call
/// costs.
pub(super) fn during<T: Terminal>(
    renderer: &mut Renderer<T>,
    during: During<'_>,
) -> Result<Meanwhile, Fatal> {
    let During {
        editor,
        queued,
        turning,
        kept,
        opened,
        says,
        style,
        cancel,
        leaving,
    } = during;
    let mut moved = false;
    let mut notice = None;

    let mut following = None;
    while following.is_some() || waiting(Duration::ZERO)? {
        let arrived = match following.take() {
            Some(arrived) => arrived,
            None => pressed()?,
        };

        // Whatever arrived, the offer to leave was made to the key after the
        // one that made it, and this is that key. Taken here rather than in the
        // arm that reads it, so that every other key takes it back — including
        // the ones this loop does nothing else with.
        let offered = leaving.take();
        moved |= offered.is_some();

        // While the view stands it has the keyboard, the way whatever is
        // standing has it everywhere else in a session: Esc closes it rather
        // than stopping the turn, and Ctrl-C reaches it before it reaches the
        // line. The turn goes on writing above it either way, which is the
        // whole reason the view is worth standing there.
        if opened.is_open() {
            moved |= opened.against(arrived);
            continue;
        }

        match meant(arrived) {
            // The rows on screen were laid out for a width the window no longer
            // has. The renderer takes them back; the redraw below puts the box
            // down again at the new one.
            Meant::Resized => {
                renderer.resized()?;
                turning.queueing(queued.waiting(), renderer.columns(), style);
                moved = true;
            }

            Meant::Interrupt => {
                cancel.request();
                turning.interrupting();
                moved = true;
            }

            Meant::Queue => {
                if !editor.is_empty() {
                    notice = queue(editor, queued, turning, renderer.columns(), style);
                    moved = true;
                }
            }

            Meant::Typing(first) => {
                let inserted = insert(editor, first)?;
                following = inserted.following;
                moved |= inserted.redraw;
                if inserted.refused {
                    notice = Some(LIMITED);
                } else if inserted.changed {
                    notice = None;
                }
            }

            Meant::Editing(key) => match editor.press(key) {
                Typed::Changed => {
                    moved = true;
                    notice = None;
                }
                Typed::Refused => {
                    moved = true;
                    notice = Some(LIMITED);
                }

                // Ctrl-C against a line with nothing on it, which means here
                // what it means at the prompt. The turn is asked to stop on the
                // way out rather than left running: the loop above is still
                // waiting on the worker, and a session that ended over a turn
                // nobody stopped would be one waiting on a provider it has no
                // reader for.
                Typed::Interrupted => {
                    if together(offered, Instant::now()) {
                        cancel.request();
                        turning.interrupting();
                        return Ok(Meanwhile::Leaving);
                    }

                    *leaving = Some(Instant::now());
                    notice = Some(LEAVING);
                    moved = true;
                }

                Typed::Ignored | Typed::Submitted | Typed::Ended => {}
            },

            // The key the cut rows themselves name, doing what they say while
            // the turn that cut them is still running. What it opens stands
            // under the tail, which is the one part of the screen a turn
            // writing above it does not reach.
            Meant::Expand => {
                opened.open(kept);
                moved |= opened.is_open();
            }

            // The same view over one result rather than all of them, asked for
            // by pointing at the row that offered it. Most rows offered
            // nothing, and a click on one of those is answered by the screen
            // the reader was already looking at.
            Meant::Clicked(row) => {
                if let Some(at) = cursor().and_then(|cursor| renderer.recorded(row, cursor)) {
                    opened.one(kept, at);
                    moved |= opened.is_open();
                }
            }

            Meant::Ignored => {}
        }
    }

    // Asked last and folded in rather than checked first, so that a key and a
    // beat that landed in the same look at the keyboard are one frame. It is
    // also what redraws a row nobody touched: the clock counts and the mark
    // turns whether anything is typed or not.
    //
    // Asked while the view stands as well, though the row it moves is not on
    // screen then. It is what puts the view back after a question was answered
    // over the top of it, and the picture it redraws is the same one, since
    // what the view stands over does not change while it stands.
    moved |= turning.moved();

    if moved && !expanding::under(renderer, style, kept, opened)? {
        // The view takes the rows the box has, so a frame draws one or the
        // other. A window with no room for the view has closed it above, and
        // the box comes back in the same frame.
        if let Some(notice) = notice {
            let mut says = says.clone();
            says.asking = Some(notice);
            stand(renderer, editor, turning, &says, style)?;
        } else {
            stand(renderer, editor, turning, says, style)?;
        }
    }

    Ok(Meanwhile::Nothing)
}

/// Moves the finished line behind the running turn, and says what the row under
/// the box owes for it.
///
/// The row above the box is read again here rather than on the next thing to
/// move, because what it names is exactly the line that has just gone: a box
/// emptied by Return, with nothing anywhere saying where the line went, is what
/// this row exists to answer. `None` where it was taken, which is also what
/// clears whatever the row was saying before.
fn queue(
    editor: &mut Editor,
    queued: &mut Prompts,
    turning: &mut Turning,
    columns: usize,
    style: Style,
) -> Option<&'static str> {
    let retained = queued.accept(editor);

    turning.queueing(queued.waiting(), columns, style);
    matches!(retained, Retained::Refused).then_some(QUEUED_LIMITED)
}

/// What the keys read while a turn ran asked for.
///
/// Two answers rather than a flag, for the reason every closed set in this
/// program is one: the loop above has to say which it got, and a new third
/// thing a key could ask for should stop the build rather than fall in with
/// whichever of these it least resembles.
pub(super) enum Meanwhile {
    /// Nothing past the line in the box. The session goes on, and so does the
    /// turn.
    Nothing,
    /// Ctrl-C twice against an empty box. The turn has been asked to stop and
    /// the session ends once it has, which is why this is reported rather than
    /// acted on here: the worker still holds the runner, and the session's log
    /// is finished by a thread its `Drop` waits for.
    Leaving,
}

/// What one key pressed while a turn is running means.
///
/// Written apart from the loop above for the reason [`super::super::heard`] is:
/// that loop reads the process's own keyboard and cannot be driven from a test,
/// and this much of the reading can be.
///
/// Every key is named rather than caught by a rest arm. One arriving mid turn
/// either belongs to the turn, to the line in the box, or to nothing — and a
/// variant added to `Pressed` later has to be decided about here rather than
/// silently join the third group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meant {
    /// The window changed under the box.
    Resized,
    /// The turn is asked to stop.
    Interrupt,
    /// Return: the finished line joins the queue the next turn is read from.
    Queue,
    /// A run of characters beginning here, taken into the line in one edit.
    Typing(char),
    /// The line's own key — a cursor move, a delete, Ctrl-C against it, Ctrl-D.
    Editing(Key),
    /// Ctrl+O: the whole of what the transcript cut down to a row, stood under
    /// the turn that is still writing it.
    Expand,
    /// A click on this screen row, which stands the one result offered there.
    ///
    /// The rows worth clicking are the ones a turn writes, so this is the key
    /// that means *more* while one is running rather than less. It is also why
    /// the terminal is left reporting buttons for the length of a turn.
    Clicked(usize),
    /// An arrow through a list there is none of, a mode step. Neither has
    /// anything to act on while a turn is running.
    Ignored,
}

/// Reads one key as the turn's, as the line's, or as neither.
fn meant(arrived: Pressed) -> Meant {
    match arrived {
        Pressed::Resized => Meant::Resized,

        // Esc means back out of the thing in front of you everywhere else in a
        // session — a login, a secret, a list being picked from — and while a
        // turn is running the turn is the thing in front of you.
        Pressed::Escape => Meant::Interrupt,

        Pressed::Key(Key::Enter) => Meant::Queue,
        Pressed::Key(Key::Char(first)) => Meant::Typing(first),

        // Ctrl-C among them, which is the point: it reaches the editor here the
        // same way it does between turns, so it throws away the line rather
        // than meaning something a running turn taught it to mean.
        Pressed::Key(key) => Meant::Editing(key),

        Pressed::Expand => Meant::Expand,

        // The column is dropped rather than carried: what a click means up in
        // the transcript is which row it landed on, and a row that offered to
        // expand offers it along the whole of its width.
        Pressed::Clicked { row, .. } => Meant::Clicked(row),

        // Ctrl+E among them: what it opens is an explanation of something
        // waiting to be decided about, and a running turn has decided already.
        // The arrows for a plainer reason — they walk a view that is not
        // standing, and a key that means nothing until Ctrl+O has been pressed
        // means nothing before it.
        Pressed::Cycle | Pressed::Explain | Pressed::Up | Pressed::Down | Pressed::Ignored => {
            Meant::Ignored
        }
    }
}

/// What can change while one turn is running.
pub(super) struct During<'a> {
    pub(super) editor: &'a mut Editor,
    pub(super) queued: &'a mut Prompts,
    /// The row above the box, which is the one thing on screen that changes
    /// without anybody pressing anything.
    pub(super) turning: &'a mut Turning,
    /// What this turn's results have had no room to say, which is what Ctrl+O
    /// stands. Read only: the turn's own thread is what adds to it.
    pub(super) kept: &'a Kept,
    /// Whether that view is standing, and where over it.
    ///
    /// Owned by the session rather than by this call for the same reason
    /// `leaving` is, and one more: the view goes on standing after the turn it
    /// was opened under has ended.
    pub(super) opened: &'a mut Standing,
    pub(super) says: &'a Says,
    pub(super) style: Style,
    pub(super) cancel: &'a Cancel,
    /// When Ctrl-C was last pressed against an empty line, if it is still the
    /// last key pressed.
    ///
    /// Owned by the loop above rather than by [`during`], which is called once
    /// per look at the channel the turn reports on: an offer made on one call
    /// is answered on a later one, and a clock that started again each time
    /// would be an offer nothing could ever take.
    pub(super) leaving: &'a mut Option<Instant>,
}

/// Puts the box under the turn, with the cursor in it.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    turning: &Turning,
    says: &Says,
    style: Style,
) -> Result<(), Fatal> {
    let (rows, caret) = working(renderer, editor, turning, says, style);

    renderer.under(&rows, Some(caret), style.palette())?;
    Ok(())
}

/// The row for the mode the box is showing.
fn saying(runner: &Runner) -> Says {
    let mode = runner.mode();

    Says {
        mode: Cow::Borrowed(mode.sentence()),
        keys: Cow::Borrowed(CYCLE),
        model: runner.model().to_owned(),
        provider: runner.serving(),
        effort: runner.effort().map(Effort::as_str),
        tone: tone(mode),
        asking: None,
    }
}

/// Puts the box on screen with the cursor where the line was typed to, and
/// answers how many rows of the region ended up above the box.
///
/// The box is the last rows of the region and the list, when there is one, is
/// the first. Drawn in that order for the reason the list opens upwards at all:
/// the box and the row under it are what the eye is resting on, and rows added
/// above them leave both exactly where they were.
///
/// Which is also why that count is what comes back. A click is read against the
/// box, and the box is however far down the region this frame happened to put
/// it — so the frame that put it there is what has to say.
fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    says: &Says,
    open: &Opened,
) -> Result<usize, Fatal> {
    let columns = renderer.columns();
    let prompt = writing(editor, says, Prompt::room(renderer.rows()));

    let mut boxed = prompt.rows(columns, style.glyphs());
    let mut caret = prompt.caret(columns);

    // What is left for a list once the box and the blank row that keeps it off
    // the box have taken theirs.
    let room = renderer.rows().saturating_sub(boxed.len() + 1);

    let mut rows = open.rows(columns, room, style.glyphs());

    // The row that keeps the box off whatever was last committed. A list opens
    // with its own, so this is only owed where there is no list -- and the box
    // is owed one either way, because a border drawn against the last line of
    // an answer reads as part of it.
    if rows.is_empty() {
        rows.push(Row::new());
    }

    let above = rows.len();
    caret.row += above;
    rows.append(&mut boxed);

    renderer.live(&rows, caret, style.palette())?;
    Ok(above)
}

/// The box as it is being typed into.
///
/// One place the fields are named, because [`draw`] and the click that follows
/// it have to lay the same component out: a click read against a box drawn any
/// other way lands somewhere nobody pointed at.
fn writing<'a>(editor: &'a Editor, says: &'a Says, room: usize) -> Prompt<'a> {
    Prompt {
        said: editor.text(),
        column: editor.column(),
        mode: says.mode.as_ref(),
        tone: says.tone,
        hint: &says.keys,
        model: says.model.as_str(),
        provider: says.provider,
        effort: says.effort,
        asking: says.asking,
        room,
    }
}

/// Where a click landed.
#[derive(Debug, Clone, Copy)]
struct Pointed {
    /// The screen row the pointer was on.
    row: usize,
    /// How many columns from the left of it.
    column: usize,
}

/// The screen row the terminal says its cursor is on.
///
/// The one fact an inline renderer cannot work out for itself: it draws
/// wherever the shell left off and the terminal scrolls that without saying so,
/// so where a frame went is a question only the terminal can answer. Asked once
/// per click and never per frame, because it costs a round trip — and `None`
/// where it went unanswered, which leaves the click meaning nothing.
fn cursor() -> Option<usize> {
    caret().ok().map(|(row, _)| row)
}

/// What a click landed on.
///
/// Three answers rather than two, because a click that landed on nothing is not
/// the same as one that landed on the line: the first owes no frame, and the
/// second has already moved the cursor to where the pointer was.
enum Landed {
    /// This row of the record, which is above the box entirely. What was cut is
    /// held by the loop that owns the transcript, so the answer is there.
    Record(usize),
    /// The line being typed, which now has the cursor where the pointer was.
    Line,
    /// The border, a blank row, the shell's own output from before crucible
    /// started — or a terminal that would not say where its cursor is.
    Nothing,
}

/// Reads where a click landed, moving the cursor where it landed on the line.
///
/// Which row of its region a click landed on is the renderer's arithmetic — it
/// knows how tall the region is and how far up in it the cursor was parked, so
/// the screen row the terminal reported is the one thing it was missing. Both
/// answers come out of that one reading, which is why they are asked together:
/// a row above the region is a row of the record, and a row inside it may be a
/// place in the line.
///
/// What is left here is where the box sits inside that region, which the frame
/// that drew it said. Leaving the cursor where it is is the right answer to a
/// click that did not land on the line.
fn landed<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &mut Editor,
    says: &Says,
    above: usize,
    at: Pointed,
) -> Landed {
    let Some(cursor) = cursor() else {
        return Landed::Nothing;
    };

    if let Some(row) = renderer.recorded(at.row, cursor) {
        return Landed::Record(row);
    }

    let within = renderer.within(at.row, cursor);
    let Some(row) = within.and_then(|row| row.checked_sub(above)) else {
        return Landed::Nothing;
    };

    let prompt = writing(editor, says, Prompt::room(renderer.rows()));
    let Some(into) = prompt.clicked(renderer.columns(), row, at.column) else {
        return Landed::Nothing;
    };

    if editor.place(into) == Typed::Changed {
        Landed::Line
    } else {
        Landed::Nothing
    }
}

/// The command list a line has open above the box, and the row of it that
/// pressing return would run.
///
/// Empty in the ordinary case, where the line is a prompt: nothing is
/// allocated, and the region is the rows the box has always been.
///
/// The row is what makes this a list rather than a reminder. A list that only
/// showed what a line could become would leave every half-typed name to be
/// finished by hand and then rejected — the list being right about the command
/// and the line being wrong about it, at the same time and on the same screen.
#[derive(Debug, Default)]
struct Opened {
    /// What the filter left, in the order `/help` lists them.
    shown: Vec<Listed<'static>>,
    /// Which row of it return runs.
    at: usize,
}

impl Opened {
    /// The list `said` has open, and the row it points at before an arrow has
    /// moved anything.
    ///
    /// That row is the one whose name is the whole line where there is one,
    /// rather than the first of them. `/mode` is a prefix of `/model`, so a
    /// line naming a command outright would otherwise point at a different
    /// command that merely starts the same way.
    fn filtered(said: &str, glyphs: Glyphs) -> Self {
        let shown = command::filtering(said, glyphs);
        let at = shown
            .iter()
            .position(|one| one.name == said)
            .unwrap_or_default();

        Self { shown, at }
    }

    /// Moves the mark back a row, and says whether it moved.
    ///
    /// Stopping at the end rather than running round to the other one, the same
    /// as the arrows that move along the line. A list is short enough to read
    /// whole, so wrapping would buy a keystroke at the price of somebody
    /// looking away and back to find where the mark went.
    fn up(&mut self) -> bool {
        let moved = self.at > 0;
        self.at = self.at.saturating_sub(1);
        moved
    }

    /// Moves it on a row.
    fn down(&mut self) -> bool {
        let last = self.shown.len().saturating_sub(1);
        let moved = self.at < last;
        self.at = last.min(self.at + 1);
        moved
    }

    /// What return runs, or `None` where there is no list and the line is what
    /// was typed.
    fn chosen(&self) -> Option<&'static str> {
        self.shown.get(self.at).map(|one| one.name)
    }

    /// The rows to open above the box, and the blank row that keeps them off
    /// it.
    ///
    /// A list with no room for it is not opened. The live region is taken back
    /// by moving the cursor up over it, so one taller than the screen is one
    /// whose top has already scrolled beyond reach — the next rewind would move
    /// back over rows the terminal has taken. Drawing what fits instead would
    /// be worse than drawing nothing: a list cut off at the top reads as the
    /// whole list.
    fn rows(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        if self.shown.is_empty() || self.shown.len() > room {
            return Vec::new();
        }

        let mut rows = Menu {
            shown: &self.shown,
            chosen: Some(self.at),
        }
        .rows(columns, glyphs);

        rows.push(Row::new());
        rows
    }
}

/// Takes the finished line, leaving it in the record where the box was.
///
/// The box goes and the line stays: what was asked belongs in the transcript
/// beside the answer to it, and the box is chrome around a line that is no
/// longer being changed.
///
/// What is taken is the command the list was pointing at where there was one,
/// and the line as typed where there was not. A marked row is an offer, and the
/// key that finishes a line is the key that answers it.
fn said<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &mut Editor,
    open: &Opened,
    style: Style,
) -> Result<Asked, Fatal> {
    let chosen = open.chosen();
    let typed = editor.take();
    let said = chosen.map_or(typed, str::to_owned);

    renderer.settle()?;
    renderer.present(&[Prompt::committed(&said, style.glyphs())], style.palette())?;

    Ok(Asked::Said(said))
}

#[cfg(test)]
mod tests;
