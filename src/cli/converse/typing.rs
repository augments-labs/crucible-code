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
    Caret, Editor, Glyphs, Key, Listed, Menu, Pressed, Prompt, Renderer, Reporting, Row, Slot,
    Terminal, Typed, caret, characters, pressed, waiting,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::command;
use super::mode::tone;
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
const LEAVING: &str = "press ctrl-c again to leave";

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
const WORKING: &str = "(enter queues it · esc to interrupt)";

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
///
/// The guard is taken for this call and dropped on the way out of it, including
/// on the `?` below, so no path out of here leaves a terminal that shows
/// nothing as the user types.
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

    // Held for exactly as long as a prompt is being read, and dropped before
    // the turn is spawned. Held any longer the terminal would be forwarding
    // buttons through a turn that reads none of them — and the wheel is a
    // button, so the cost would be the scrollback this program's transcript
    // lives in, paid for nothing.
    let _pointer = style.clicks().then(Reporting::on).transpose()?.flatten();

    let glyphs = style.glyphs();

    let mut says = saying(runner);

    // When Ctrl-C was last pressed against an empty line, if it is still the
    // last key pressed. Taken by every other key, so the pair has to be two
    // presses in a row rather than two with a session between them.
    let mut leaving: Option<Instant> = None;

    // Whatever was typed while the last turn ran is already here, so the list a
    // slash opens has to be worked out from it rather than assumed empty.
    let mut open = Opened::filtered(editor.text(), glyphs);
    let mut shown = draw(renderer, editor, style, &says, &open)?;

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
                shown = draw(renderer, editor, style, &says, &open)?;
            }

            // Nothing is standing, so there is nothing to back out of — except
            // the offer above, which is on screen and has just been taken back.
            Pressed::Escape | Pressed::Ignored => {
                if offered.is_some() {
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
            }

            // The arrows walk the line one place at a time; this is the same
            // move made in one go. A click that landed anywhere but on the line
            // moves nothing, which is the same as a key that moved nothing.
            Pressed::Clicked { row, column } => {
                let moved = placed(renderer, editor, &says, shown, (row, column));

                if moved || offered.is_some() {
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
            }

            // Through whatever the line has open above the box. With nothing
            // open, or at the end it is already on, the key costs no frame.
            Pressed::Up => {
                if open.up() || offered.is_some() {
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
            }
            Pressed::Down => {
                if open.down() || offered.is_some() {
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
            }

            // Stepping the mode on. Every step takes effect on the press: the
            // row under the box says which mode that landed in, and the same
            // key is what steps out of it again.
            Pressed::Cycle => {
                runner.cycle();

                says = saying(runner);
                shown = draw(renderer, editor, style, &says, &open)?;
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
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
            }

            Pressed::Key(key) => match editor.press(key) {
                // A key that moved nothing costs no frame, unless the offer
                // above is on screen and now stale. An arrow held down against
                // the end of a line is what the first half of that saves.
                Typed::Ignored => {
                    if offered.is_some() {
                        shown = draw(renderer, editor, style, &says, &open)?;
                    }
                }
                Typed::Changed => {
                    open = Opened::filtered(editor.text(), glyphs);
                    shown = draw(renderer, editor, style, &says, &open)?;
                }
                Typed::Refused => {
                    says.asking = Some(LIMITED);
                    shown = draw(renderer, editor, style, &says, &open)?;
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
                    shown = draw(renderer, editor, style, &says, &open)?;
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
    keys: &'static str,
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
pub(super) fn working<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &Editor,
    says: &Says,
    style: Style,
) -> (Vec<Row>, Caret) {
    let columns = renderer.columns();
    let prompt = writing(editor, says, Prompt::room(renderer.rows()));

    (prompt.rows(columns, style.glyphs()), prompt.caret(columns))
}

/// What the row under a running turn says.
///
/// Read before the runner leaves for the worker, because nothing on this side
/// can ask it anything while it is away — and nothing needs to: `/mode`,
/// `/model` and `/effort` are all lines typed between turns, so none of the
/// three facts here can change while the turn they describe is the one running.
pub(super) fn under(runner: &Runner) -> Says {
    Says {
        keys: WORKING,
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
/// finishes a line, Esc asks the turn to stop, Ctrl-C is the line's own — in
/// raw mode the terminal sends it rather than raising a signal, so it reaches
/// the editor here exactly as it does at the prompt — and the rest edit the
/// line. Stepping the mode is not among them: the runner that holds it is on
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

        match meant(arrived) {
            // The rows on screen were laid out for a width the window no longer
            // has. The renderer takes them back; the redraw below puts the box
            // down again at the new one.
            Meant::Resized => {
                renderer.resized()?;
                moved = true;
            }

            Meant::Interrupt => {
                cancel.request();
                moved = true;
            }

            Meant::Queue => {
                if !editor.is_empty() {
                    match queued.accept(editor) {
                        Retained::Accepted => notice = None,
                        Retained::Refused => notice = Some(QUEUED_LIMITED),
                    }
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
                        return Ok(Meanwhile::Leaving);
                    }

                    *leaving = Some(Instant::now());
                    notice = Some(LEAVING);
                    moved = true;
                }

                Typed::Ignored | Typed::Submitted | Typed::Ended => {}
            },

            Meant::Ignored => {}
        }
    }

    if moved {
        if let Some(notice) = notice {
            let mut says = says.clone();
            says.asking = Some(notice);
            stand(renderer, editor, &says, style)?;
        } else {
            stand(renderer, editor, says, style)?;
        }
    }

    Ok(Meanwhile::Nothing)
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
    /// An arrow through a list there is none of, a click, a mode step. None of
    /// them has anything to act on while a turn is running.
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

        Pressed::Clicked { .. }
        | Pressed::Cycle
        | Pressed::Up
        | Pressed::Down
        | Pressed::Ignored => Meant::Ignored,
    }
}

/// What can change while one turn is running.
pub(super) struct During<'a> {
    pub(super) editor: &'a mut Editor,
    pub(super) queued: &'a mut Prompts,
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
    says: &Says,
    style: Style,
) -> Result<(), Fatal> {
    let (rows, caret) = working(renderer, editor, says, style);

    renderer.under(&rows, Some(caret), style.palette())?;
    Ok(())
}

/// The row for the mode the box is showing.
fn saying(runner: &Runner) -> Says {
    let mode = runner.mode();

    Says {
        mode: Cow::Borrowed(mode.sentence()),
        keys: CYCLE,
        model: runner.model().to_owned(),
        provider: runner.serving(),
        effort: runner.effort().map(Effort::as_str),
        tone: tone(mode),
        asking: None,
    }
}

/// Puts the box on screen with the cursor where the line was typed to.
///
/// The box is the last rows of the region and the list, when there is one, is
/// the first. Drawn in that order for the reason the list opens upwards at all:
/// the box and the row under it are what the eye is resting on, and rows added
/// above them leave both exactly where they were.
fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    says: &Says,
    open: &Opened,
) -> Result<Shown, Fatal> {
    let columns = renderer.columns();
    let prompt = writing(editor, says, Prompt::room(renderer.rows()));

    let mut boxed = prompt.rows(columns, style.glyphs());
    let mut caret = prompt.caret(columns);

    // What is left for a list once the box and the blank row that keeps it off
    // the box have taken theirs.
    let room = renderer.rows().saturating_sub(boxed.len() + 1);

    let mut rows = open.rows(columns, room, style.glyphs());
    let above = rows.len();
    caret.row += above;
    rows.append(&mut boxed);

    renderer.live(&rows, caret, style.palette())?;
    Ok(Shown { caret, above })
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
        hint: says.keys,
        model: says.model.as_str(),
        provider: says.provider,
        effort: says.effort,
        asking: says.asking,
        room,
    }
}

/// Where the last frame put things, for reading a click against.
#[derive(Debug, Clone, Copy)]
struct Shown {
    /// Where the cursor was left, counted from the top of the live region.
    caret: Caret,
    /// How many rows of that region sit above the box.
    above: usize,
}

/// Moves the cursor to where the pointer was, and says whether it moved.
///
/// crucible draws inline, so it does not know which row of the screen it is on.
/// What it does know is that the terminal's cursor is parked on the caret, and
/// the caret's row inside the region was worked out when the region was drawn —
/// so asking the terminal where its cursor is turns one into the other. The
/// question costs a round trip and is asked once per click, never per frame.
///
/// Nothing happens where any step of that does not answer: a terminal that will
/// not report its cursor, a click above the box or below it, a click on the
/// border. Leaving the cursor where it is is the right answer to a click that
/// did not land on the line, and a click is not worth failing a session over.
fn placed<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &mut Editor,
    says: &Says,
    shown: Shown,
    at: (usize, usize),
) -> bool {
    let (row, column) = at;

    let Ok((where_row, _)) = caret() else {
        return false;
    };

    let Some(top) = where_row.checked_sub(shown.caret.row) else {
        return false;
    };
    let Some(at) = row.checked_sub(top + shown.above) else {
        return false;
    };

    let prompt = writing(editor, says, Prompt::room(renderer.rows()));
    let Some(into) = prompt.clicked(renderer.columns(), at, column) else {
        return false;
    };

    editor.place(into) == Typed::Changed
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
