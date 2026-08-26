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
use std::io;
use std::time::{Duration, Instant};

use crucible_core::{Cancel, Effort, Mode};
use crucible_runner::Runner;
use crucible_tools::{Background, Ended};
use crucible_tui::{
    Aimed, Caret, CommandCount, Draft, Editor, Glyphs, Key, Listed, Menu, Pressed, Prompt,
    PromptRows, Remaining, Renderer, Row, Slot, Terminal, TerminalError, Typed, characters,
    pressed,
};

use crate::cli::Fatal;
use crate::cli::kept::Kept;
use crate::cli::standing;
use crate::cli::style::Style;

use super::command;
use super::expanding::{self, Standing};
use super::leaving::Leaving;
use super::mode::tone;
use super::planning::Planning;
use super::queueing;
use super::turning::Turning;
use super::{Prompts, Retained, Terms};

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

/// What the row under the box says once the line has gone to the clipboard.
const COPIED: &str = "line copied";

/// And what it says when the terminal would not take it. A line long enough to
/// reach that is one nobody typed by hand, so the number is not worth naming:
/// what a reader can act on is that the clipboard does not have it.
const UNCOPIED: &str = "the line is too long for the terminal to copy";

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
pub(super) enum Asked {
    /// A line was typed and finished.
    Said(Said),
    /// The session is over: Ctrl-D on an empty line, or Ctrl-C twice against
    /// one.
    Ended,
    /// Ctrl+O: whatever the transcript cut down to a row is to be shown whole,
    /// and the box asked for again once it has been closed.
    ///
    /// The box is not written down on the way out. It stands in a band and so
    /// does the view, so the one replaces the other and the line being typed is
    /// still there underneath when this comes back.
    ///
    /// Reported rather than acted on here, because the same key pressed while a
    /// turn runs opens the same view a row further down the screen. Both go
    /// through the one door at the top of the loop above.
    Expand,
    /// A click on this row of the record, which stands the one result that row
    /// offered to expand — or nothing, where it offered none. The box comes back
    /// with the line still in it either way.
    ///
    /// Which of the two it is, is not decided here. What was cut belongs to the
    /// loop above and so does the view, and asking this module to hold either of
    /// them to answer a click would put the transcript's half of the session
    /// inside the box's.
    Clicked(usize),
    /// There is nothing here to type into. The caller reads a line instead.
    Untyped,
    /// A command left running has ended, and this is the turn it asks for.
    ///
    /// The reader was told in a line the moment it happened, and being told is
    /// where it ends for them. For the model it is not: one that started a
    /// build and yielded is waiting on the machine rather than on the person at
    /// the keyboard, and a fact it can only be handed at the top of a turn is a
    /// fact it never gets where nobody types one.
    ///
    /// Nothing comes out of the box for it. Nobody typed this, so the half-
    /// written line somebody left there is still there afterwards.
    Woke(String),
}

/// A finished line and the local-command provenance established while typing.
///
/// The fields stay private to this module so a caller cannot pair arbitrary
/// expanded source with a `true` local verdict. The parent can only consume
/// the pair that [`said`] constructed from the editor and its visible menu.
pub(super) struct Said {
    said: String,
    local: bool,
}

impl Said {
    fn new(said: String, local: bool) -> Self {
        Self { said, local }
    }

    pub(super) fn into_parts(self) -> (String, bool) {
        (self.said, self.local)
    }
}

/// Moves the cursor within a many-rowed line, where there is a row to reach.
///
/// Whether the editor moved is the answer a caller keys a frame on, and whether
/// the key was the line's at all: a one-line line has no row above or below, so
/// the arrows stay with whatever is open above the box instead.
fn vertical(editor: &mut Editor, key: Key) -> bool {
    editor.moves(key) && editor.press(key) == Typed::Changed
}

/// Pastes a bracketed block into the line, and says what the row owes for it.
///
/// One place for the two loops that read a prompt, because a paste means the
/// same thing in both: its newlines are characters rather than submissions, and
/// a block over the line's bound is refused whole. `true` is a frame owed.
fn pasted(editor: &mut Editor, text: &str, says: &mut Says) -> bool {
    match editor.paste(text) {
        Typed::Changed => true,
        Typed::Refused => {
            says.asking = Some(Cow::Borrowed(LIMITED));
            true
        }
        _ => false,
    }
}

/// Puts the line on the reader's clipboard, and says what the row owes for it.
///
/// One place for the two loops that read a prompt, because the key means the
/// same thing in both. The line is what goes, rather than what a drag over the
/// box would have taken: that selection is the picture — a border down each
/// side, and ground padded out to the last column between them — and there is
/// no asking a terminal to select something else.
///
/// Nothing to say about a line with nothing on it: a reader who pressed the key
/// over an empty box has already been answered by the box.
fn copy<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
) -> Result<Option<&'static str>, Fatal> {
    if editor.text().is_empty() {
        return Ok(None);
    }

    let took = renderer.copied(editor.text())?;
    Ok(Some(if took { COPIED } else { UNCOPIED }))
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

/// What one prompt is read against.
///
/// Everything here belongs to the session rather than to the prompt, and every
/// one of them can be changed by a key pressed at it: Shift+Tab steps the mode
/// the runner holds, Ctrl+T opens the plan, and the rest go into the line.
pub(crate) struct Between<'a> {
    /// Holds the mode, which is the one thing a key at the prompt changes about
    /// the session rather than about the screen.
    pub(crate) runner: &'a mut Runner,
    /// The line being written, which still holds whatever was typed while the
    /// last turn ran.
    pub(crate) editor: &'a mut Editor,
    /// The plan above the box, and whether the reader has opened it.
    pub(crate) planning: &'a mut Planning,
    /// The images pasted so far, which Ctrl+V adds to. The line gets
    /// `[image N]` and this gets the path the marker stands for, so the
    /// session owns the list the way it owns the line.
    pub(crate) images: &'a mut Vec<Box<str>>,
    /// What is still running behind the box: the count on the row under it, and
    /// what this loop wakes on a clock for while there is anything left to end.
    pub(crate) left: &'a Background,
    /// Whether there is a keyboard to read. A session with a terminal at only
    /// one end reads whole lines instead, and the caller is what does that.
    pub(crate) keys: bool,
}

/// How often the prompt looks up from the keyboard while something is running.
///
/// The beat the row above a running turn already moves on, because it is the
/// coarsest clock this program has and there is nothing here that needs a finer
/// one: a command ends once, and a quarter of a second later is soon enough to
/// hear about it.
const BEAT: Duration = Duration::from_millis(250);

/// Stands the list of what is running, and says the box is owed a frame.
///
/// One place, because the key and the count under the box are one door: the key
/// names it and the row is it, and two call sites doing the same two things is how
/// they come to do slightly different ones.
///
/// Stood here rather than handed back to the loop above, unlike the view of what
/// the transcript cut: that key means the same thing while a turn runs, and this
/// one means something else there entirely.
fn stood<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    listing: &mut Leaving,
    left: &Background,
) -> Result<bool, Fatal> {
    listing.stand(renderer, style, left)?;
    Ok(true)
}

/// Waits for a key, reporting any command that ended while nobody was typing.
///
/// `None` where one did: the count under the box has moved and a line has been
/// written above it, so the box is owed a frame rather than handed a key.
///
/// While nothing is running this waits on the keyboard exactly as it always did.
/// The clock is only consulted while there is something that could end without a
/// keystroke — a wake-up four times a second, and only then, because a row that
/// silently went on saying `1 command` after the server behind it fell over is the
/// stale fact the row exists to prevent.
fn arriving<T: Terminal>(
    renderer: &mut Renderer<T>,
    left: &Background,
    says: &mut Says,
    style: Style,
) -> Result<Option<Pressed>, Fatal> {
    loop {
        // Offered to the selection first, the same as every other read in this
        // session: a drag comes back as nothing, having already drawn itself.
        if left.count() == 0 && says.running == 0 {
            loop {
                let arrived = renderer.pressed()?;
                if arrived.is_some() || renderer.pointed_changed() {
                    return Ok(arrived);
                }
            }
        }

        if renderer.waiting(BEAT)? {
            let arrived = renderer.took(pressed()?)?;
            if arrived.is_some() || renderer.pointed_changed() {
                return Ok(arrived);
            }
            continue;
        }

        let ended = left.reap();
        let count = left.count();
        if ended.is_empty() && count == says.running {
            continue;
        }

        for one in &ended {
            crate::cli::draw::gone(renderer, one, style)?;
        }

        says.running = count;
        return Ok(None);
    }
}

/// The turn `ended` asks for, where it asks for one.
///
/// The endings are taken from the one place a typed turn's note is taken from,
/// so a command that ended is either the turn or under one and never both.
/// `None` is what almost every call gets, and it is the answer that costs
/// nothing.
fn woken(ended: &[Ended]) -> Option<Asked> {
    standing::said(ended).map(Asked::Woke)
}

/// Reads one prompt, drawing it as it arrives.
pub(crate) fn ask<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    between: Between<'_>,
) -> Result<Asked, Fatal> {
    let Between {
        runner,
        editor,
        planning,
        images,
        left,
        keys,
    } = between;

    if !keys {
        return Ok(Asked::Untyped);
    }

    let glyphs = style.glyphs();

    // The plan may have moved since the last frame that drew it: the turn above
    // wrote to it, or a command emptied it. Asked once here rather than per key,
    // because nothing between turns can write to it — the tool that does runs
    // inside one.
    planning.moved();

    let mut says = saying(runner);
    says.running = left.count();

    // A local, because where the mark was in it is not worth keeping: a list of
    // four things opened twice reads better from the top than from wherever it was
    // left, and the scroll position that *is* worth keeping belongs to the view of
    // one command's output, which is closed with the list.
    let mut listing = Leaving::default();

    // When Ctrl-C was last pressed against an empty line, if it is still the
    // last key pressed. Taken by every other key, so the pair has to be two
    // presses in a row rather than two with a session between them.
    let mut leaving: Option<Instant> = None;

    // Whatever was typed while the last turn ran is already here, so the list a
    // slash opens has to be worked out from it rather than assumed empty.
    // Before the box is drawn, because what this answers is whether a box is
    // what is owed at all: a command that ended during the last turn — or after
    // it, while this was between two calls — leaves the model owing a turn, and
    // drawing the box first would put a cursor under it and wait for a person
    // who is waiting for the agent.
    if let Some(woke) = woken(&left.reported()) {
        return Ok(woke);
    }

    let mut open = Opened::filtered(editor.projection().text(), glyphs);
    draw(renderer, editor, style, around(planning, &open, &says))?;

    let mut following = None;
    loop {
        // Nothing arriving means a command ended instead: the count under the box
        // has moved and a line has been written above it, so what is owed is a
        // frame rather than a key to act on.
        let arrived = if let Some(arrived) = following.take() {
            let Some(arrived) = renderer.took(arrived)? else {
                continue;
            };
            arrived
        } else {
            let Some(arrived) = arriving(renderer, left, &mut says, style)? else {
                // The other end of the same fact: nothing arrived because a
                // command ended, and the line above it is now on screen. The
                // model is owed the turn from here rather than from the top of
                // the call, since this is where the waiting was happening.
                if let Some(woke) = woken(&left.reported()) {
                    return Ok(woke);
                }

                draw(renderer, editor, style, around(planning, &open, &says))?;
                continue;
            };

            arrived
        };

        // Whatever arrived, the offer to leave was made to the key after the
        // one that made it, and this is that key.
        let offered = leaving.take();
        says.asking = None;

        // Whether this key left the box looking like anything other than what
        // is already on screen. Answered by every arm and drawn on once, below:
        // a key that moved nothing costs no frame, and the arms that end the
        // call leave through their own `return` without drawing at all.
        let moved = match arrived {
            Pressed::Background => stood(renderer, style, &mut listing, left)?,
            // Redrawn rather than re-wrapped: the box was laid out for a width
            // the window no longer has, and the rows it left on screen are the
            // renderer's to take back before the new ones go down.
            Pressed::Resized => {
                renderer.resized()?;
                true
            }

            // Handed back to the caller rather than answered here. What was
            // cut is the transcript's, this module is the box's, and the two
            // meet where the loop that owns both of them is.
            Pressed::Expand => return Ok(Asked::Expand),

            // The line, out to wherever the reader is going to paste it.
            Pressed::Copy => {
                says.asking = copy(renderer, editor)?.map(Cow::Borrowed);
                says.asking.is_some() || offered.is_some()
            }

            // Image bytes come from the desktop clipboard, into the same durable
            // session store an external path is imported through. The editor
            // holds only `[image N]` and the session holds the path the marker
            // stands for, so submission takes the ordinary attachment path and
            // all of its capability checks.
            Pressed::PasteImage => match super::attaching::clipboard(runner) {
                Ok(path) => {
                    // The path joins the list only once its marker is in the
                    // line, or a refused paste would leave a number nobody sees.
                    let marker = format!("[image {}]", images.len() + 1);
                    match editor.paste(&marker) {
                        Typed::Changed => {
                            images.push(path.into_boxed_str());
                            open = Opened::filtered(editor.projection().text(), glyphs);
                            true
                        }
                        Typed::Refused => {
                            says.asking = Some(Cow::Borrowed(LIMITED));
                            true
                        }
                        _ => offered.is_some(),
                    }
                }
                Err(problem) => {
                    says.asking = Some(Cow::Owned(problem));
                    true
                }
            },

            // And the key that says the same word about the other thing that
            // was cut down to fit. Answered here rather than handed back,
            // because the plan stands in the region this call is drawing: it is
            // the box's own footing rather than something committed above it.
            Pressed::Plan => planning.expand(),

            // Nothing is standing, so there is nothing to back out of and
            // nothing to explain — except the offer above, which is on screen
            // and has just been taken back. Ctrl+Q among them: between turns
            // nothing is queued, so the queue view has nothing to show — the key
            // is the panel's while a turn runs, and the panel is the turn's.
            // The pointer moving under a held button and the button coming up
            // again among them: both belong to the selection, which was
            // offered every press before this one saw it, so neither reaches
            // here. The key that crosses regions among them for the first
            // reason of all: nothing is standing, so there are no regions.
            // Named all the same — a variant nothing decides about is one that
            // will arrive undecided the day something changes.
            Pressed::Escape
            | Pressed::Explain
            | Pressed::Queue
            | Pressed::Tab
            | Pressed::Dragged { .. }
            | Pressed::Hovered { .. }
            | Pressed::Released { .. }
            | Pressed::Ignored => offered.is_some(),

            // Two things a click can land on and one round trip to tell them
            // apart. On the line it is the move the arrows make one place at a
            // time, made in one go; above the box it is a row of the record,
            // which is the loop above's to answer because what was cut is
            // there. Anywhere else — the border, a blank row, the shell's own
            // output — it moves nothing, the same as a key that moved nothing.
            Pressed::Clicked { row, column } => {
                match landed(renderer, editor, &says, Pointed { row, column }) {
                    Landed::Record(at) => return Ok(Asked::Clicked(at)),
                    Landed::Line => true,

                    Landed::Counted => stood(renderer, style, &mut listing, left)?,
                    Landed::Nothing => offered.is_some(),
                }
            }

            // The arrows walk whatever is open above the box — unless the line
            // being typed is many rows, in which case they move within it. The
            // editor answers whether there is a row to reach, and a one-line
            // line has none, so the list keeps the key it has always had.
            Pressed::Up => vertical(editor, Key::Up) || open.up() || offered.is_some(),
            Pressed::Down => vertical(editor, Key::Down) || open.down() || offered.is_some(),

            // And the wheel walks the transcript, which is the thing the arrows
            // never reach. It draws its own frame, so what is left to say here
            // is whether the offer above went with it — the box is repainted
            // from what the renderer already had, and a second frame for it
            // would cost nothing and change nothing.
            Pressed::Scrolled { back } => {
                renderer.notched(back)?;
                offered.is_some()
            }

            // Stepping the mode on. Every step takes effect on the press: the
            // row under the box says which mode that landed in, and the same
            // key is what steps out of it again.
            Pressed::Cycle => {
                runner.cycle();

                says = saying(runner);
                true
            }

            Pressed::Key(Key::Char(first)) => {
                let inserted = insert(editor, first)?;
                following = inserted.following;

                if inserted.changed {
                    open = Opened::filtered(editor.projection().text(), glyphs);
                }
                if inserted.refused {
                    says.asking = Some(Cow::Borrowed(LIMITED));
                }
                inserted.redraw || offered.is_some()
            }

            // A bracketed paste, inserted whole. Its newlines are characters in
            // the line rather than submissions, which is the difference between
            // this and a run of typed characters — and the reason it is not one.
            Pressed::Pasted(text) => {
                let moved = pasted(editor, &text, &mut says);
                if moved {
                    open = Opened::filtered(editor.projection().text(), glyphs);
                }
                moved || offered.is_some()
            }

            Pressed::Key(key) => match editor.press(key) {
                // A key that moved nothing costs no frame, unless the offer
                // above is on screen and now stale. An arrow held down against
                // the end of a line is what the first half of that saves.
                Typed::Ignored => offered.is_some(),
                Typed::Changed => {
                    open = Opened::filtered(editor.projection().text(), glyphs);
                    true
                }
                Typed::Refused => {
                    says.asking = Some(Cow::Borrowed(LIMITED));
                    true
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
                    says.asking = Some(Cow::Borrowed(LEAVING));
                    true
                }

                Typed::Ended => {
                    renderer.settle()?;
                    return Ok(Asked::Ended);
                }
            },
        };

        if moved {
            draw(renderer, editor, style, around(planning, &open, &says))?;
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
    pub(super) mode: Cow<'static, str>,
    /// The keys that act on it, said quietly after.
    pub(super) keys: Cow<'static, str>,
    /// Which model the next turn goes to, at the other end of a framed row.
    /// On a bare row the remaining-window reading stands after it.
    pub(super) model: String,
    /// The vendor it is asked of, drawn before it.
    pub(super) provider: &'static str,
    /// How hard it is being asked to think. `None` where no rung is in force.
    pub(super) effort: Option<&'static str>,
    /// What the border and the sentence are both drawn in.
    pub(super) tone: Slot,
    /// A row under that, for something waiting on the very next key. `None` in
    /// the ordinary state, where the mode is the last row there is. Owned only
    /// where the sentence carries a fact of the moment — what a paste actually
    /// failed on — and borrowed everywhere the words are fixed.
    pub(super) asking: Option<Cow<'static, str>>,
    /// How many commands are still running. Read per frame rather than per turn,
    /// because a command ending is neither a keystroke nor a turn.
    pub(super) running: usize,
    /// How much usable room remains before compaction at the latest reading.
    /// `None` makes the prompt say that the reading is unknown.
    pub(super) left: Option<u8>,
    /// The mode in force, kept as a value so shift+tab can step off it while
    /// the runner that owns it is away.
    ///
    /// Mid-turn the sentence above is the only thing of it drawn, and a step
    /// reads the mode rather than the sentence — which is why the value is
    /// kept beside its words instead of the words being parsed back.
    pub(crate) running_mode: Mode,
}

impl Says {
    /// The same rows with one more thing said under them.
    ///
    /// A copy, because what it is made from belongs to the session and what is
    /// folded into it belongs to the frame: a notice is put up by a key or a
    /// bound and taken back by the next one. Made only where there is one to
    /// fold in, so the ordinary frame copies nothing.
    fn noticing(&self, notice: &'static str) -> Self {
        Self {
            asking: Some(Cow::Borrowed(notice)),
            ..self.clone()
        }
    }

    /// The same rows, saying the mode shift+tab just stepped to.
    ///
    /// In the words the running mode would be said in, with its tone — the one
    /// difference is that the step lands on the next turn rather than this
    /// one, which is a fact of the mode's timing, not of how the row reads.
    pub(super) fn cycling(&mut self, mode: Mode) {
        self.mode = Cow::Borrowed(mode.sentence());
        self.tone = tone(mode);
    }
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
/// Above it, the row that says the turn is running and the plan it is working
/// to. Laid out after the box rather than before it, because the box takes its
/// share of the window first and what is left is what decides how much of that
/// footing is drawn at all.
fn working<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &Editor,
    footing: Footing<'_>,
    says: &Says,
    style: Style,
) -> Footed {
    let columns = renderer.columns();
    let boxed = boxing(renderer, editor, says, footing.turning.left(), style);
    let room = renderer.rows().saturating_sub(boxed.rows.len());

    // The list a `/`-started line has open stands directly above the box, over
    // the running row and plan, as it does at the prompt: a list is what the
    // reader is looking at while it is open, so the rows above give way to it.
    let mut over = footing.turning.rows(footing.planning, columns, style, room);
    let left = room.saturating_sub(over.len());
    over.extend(footing.opened_list.rows(columns, left, style.glyphs()));

    Footed {
        over,
        boxed: boxed.rows,
        pointed: boxed.pointed,
        caret: boxed.caret,
    }
}

/// The two layout bands the renderer replaces together while a turn runs.
///
/// They are not one region with the box at the end of it. The box is held to a
/// share of the window and what stands over it is not, so the two are kept
/// apart all the way to the renderer — a turn whose plan is open would
/// otherwise push the box off the bottom of the screen.
struct Footed {
    /// The row saying the turn is running, and the plan above it.
    over: Vec<Row>,
    /// The box.
    boxed: Vec<Row>,
    /// The pointable prompt row in its pointed palette state.
    pointed: Option<(usize, Row)>,
    /// Where the cursor sits in the box, counted from its own first row.
    caret: Caret,
}

/// What stands between the transcript and the box while a turn runs.
///
/// The two of them together because they are laid out together: the row that
/// says the turn is running and the plan above it share one window, and which
/// of them gives way when there is not enough of it is a single decision made
/// in one place.
#[derive(Clone, Copy)]
pub(super) struct Footing<'a> {
    /// The row that says the turn is running, and what it is queueing.
    pub(super) turning: &'a Turning,
    /// The plan the agent is working to, above that row.
    pub(super) planning: &'a Planning,
    /// The command list a `/`-started line has open, standing over both.
    pub(super) opened_list: &'a Opened,
}

/// What the row under a running turn says.
///
/// The same mode and key as between turns. Shift+Tab is live while the turn
/// runs: its mode is held for the next turn until the runner comes back. Esc is
/// already named on the working row above the box, and Return needs no second
/// explanation here.
pub(super) fn under(runner: &Runner) -> Says {
    saying(runner)
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
/// stands that one result, Ctrl+T opens the whole of the plan above the box or
/// bounds it again, Ctrl-C is the line's own — in raw mode the terminal sends it
/// rather than raising a signal, so it reaches the editor here exactly as it
/// does at the prompt — and the rest edit the line. While that view stands it
/// has all of them: it takes the rows the box has, so the box is not on screen
/// to be typed into and Esc closes the view rather than stopping the turn behind
/// it.
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
        planning,
        kept,
        opened,
        viewing,
        opened_list,
        listing,
        says,
        background,
        style,
        cancel,
        steer,
        terms,
        leaving,
    } = during;
    let mut moved = false;
    let mut notice = None;

    let mut following = None;
    while following.is_some() || renderer.waiting(Duration::ZERO)? {
        let arrived = match following.take() {
            Some(arrived) => arrived,
            None => pressed()?,
        };
        let Some(arrived) = renderer.took(arrived)? else {
            // A selection draws for itself. A pointer transition waits for the
            // pointable prompt and fixed foot to be replaced together below.
            moved |= renderer.pointed_changed();
            continue;
        };

        // Whatever arrived, the offer to leave was made to the key after the
        // one that made it, and this is that key. Taken here rather than in the
        // arm that reads it, so that every other key takes it back — including
        // the ones this loop does nothing else with.
        let offered = leaving.take();
        moved |= offered.is_some();

        // News about the window rather than a key aimed at whatever is
        // standing, so it is acted on before the view below is offered it —
        // which is the order every other loop in this session reads a resize
        // in. A view handed it first would redraw itself against the size the
        // renderer is still holding, and go on being rewound over at that size
        // for the rest of the turn.
        if arrived == Pressed::Resized {
            rewrap(renderer, turning, queued, style)?;
        }

        // While the view stands it has the keyboard, the way whatever is
        // standing has it everywhere else in a session: Esc closes it rather
        // than stopping the turn, and Ctrl-C reaches it before it reaches the
        // line. The turn goes on writing above it either way, which is the
        // whole reason the view is worth standing there.
        if opened.is_open() {
            // Read before the press is handed over, because it is handed over.
            let wheel = match arrived {
                Pressed::Scrolled { back } => Some(back),
                _ => None,
            };

            let walked = opened.against(arrived);
            moved |= walked;

            // A wheel the view did nothing with is the transcript's, by the
            // rule the region loop between turns reads one under: at either end
            // of a view the reader is still reading back through what was said,
            // and here it is still being added to above them.
            if let (false, Some(back)) = (walked, wheel) {
                renderer.notched(back)?;
            }

            continue;
        }

        // And the queue, for the same reason and by the same rule. Only one of
        // the two is ever standing — the key that opens this one is swallowed
        // above while that one is up — so the order between them decides
        // nothing, and reading them in the order they are drawn in is what
        // keeps that visible.
        if viewing.is_open() {
            let walked = viewing.against(
                &arrived,
                queueing::Reading {
                    queue: queued,
                    editor,
                    steer,
                },
            );
            moved |= walked;

            // And the same about the wheel, for the same reason.
            if let (false, Pressed::Scrolled { back }) = (walked, arrived) {
                renderer.notched(back)?;
            }

            continue;
        }

        match meant(arrived) {
            Meant::Background => background.ask(),
            // Taken back and re-wrapped above, before the view could have been
            // handed the same press. What is left to say is that the picture no
            // longer matches, which is what the redraw below reads.
            Meant::Resized => moved = true,

            // Stepped to on this side and held for the next turn: the mode the
            // running turn is decided under was settled before it ran, so the
            // step cannot reach the runner on the worker — it goes into the
            // pending slot, and the row under the box says which mode that is,
            // marked for the turn it lands on. The step is read off the slot's
            // last value, or off the running mode the row was frozen with.
            // The one list a running turn can stand, walked. A line that is
            // not a command has no list open, and up and down both leave the
            // mark where it is, so an arrow with nothing to walk moves
            // nothing.
            Meant::Arrow { back } => {
                moved |= if back {
                    opened_list.up()
                } else {
                    opened_list.down()
                };
            }

            Meant::Cycle => {
                let next = terms.pending_mode.get().unwrap_or(says.running_mode).next();
                terms.pending_mode.set(Some(next));
                says.cycling(next);
                moved = true;
            }

            Meant::Interrupt => {
                cancel.request();
                turning.interrupting();
                moved = true;
            }

            // The queue itself, opened whole: the panel names what fits and
            // counts the rest, and this is the list the count is about. A line
            // taken back returns to the box to be edited or sent sooner.
            Meant::QueueView => {
                viewing.open(queued, steer);
                moved |= viewing.is_open();
            }

            Meant::Copy => {
                if let Some(said) = copy(renderer, editor)? {
                    notice = Some(said);
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
                    // The list a `/`-started line has open is of what the line
                    // now says, so it is filtered again on the change — a line
                    // that stopped being one closes the list with it.
                    *opened_list = Opened::filtered(editor.projection().text(), style.glyphs());
                }
            }

            Meant::Pasting(text) => match editor.paste(&text) {
                Typed::Refused => {
                    moved = true;
                    notice = Some(LIMITED);
                }
                Typed::Changed => {
                    moved = true;
                    notice = None;
                }
                _ => {}
            },

            Meant::Editing(key) => match editor.press(key) {
                Typed::Changed => {
                    moved = true;
                    notice = None;
                    *opened_list = Opened::filtered(editor.projection().text(), style.glyphs());
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

                // The line is finished, and what finished it was whichever
                // press `input.send` says finishes one. Steered first, so the
                // running turn works it in at its next pass; then queued,
                // because a turn already finishing takes nothing and the line
                // is still owed a turn of its own. Whichever happens, it leaves
                // the queue: the turn reports the lines it reached, and the
                // loop that reads that drops them. The queue takes the editor
                // empty, so the text is read off it before `queue` clears it.
                Typed::Submitted => {
                    // A slash command is not a line for the turn: it is answered
                    // on this thread, the way it is between turns. But the panel
                    // it opens is stood from the turn's own loop, where the turn
                    // it stands over can be kept rendering — so the command is
                    // returned, and that loop runs it.
                    //
                    // What Enter runs is the marked row where the list is open —
                    // a line still being typed is a reader choosing, and the mark
                    // is what they have chosen — and the typed word where it is
                    // not. A bare `/` is the key that opens the list, not a
                    // command, so it is never a submission.
                    // A bare `/` is the key that opens the list, not a command:
                    // it parses as a word that names none and is refused, which
                    // is not what a reader pressing Enter while still choosing
                    // meant. Only a line past the bare slash is ever submitted.
                    let bare = editor.text() == "/";
                    let marked = opened_list.chosen().filter(|_| !bare);
                    let owned = if bare {
                        None
                    } else {
                        marked
                            .and_then(command::owned)
                            .or_else(|| command::owned(editor.text()))
                    };
                    if let Some(owned) = owned {
                        editor.take();
                        // The list the line had open goes with the line: the
                        // box is cleared for the command, and a list left open
                        // over an empty box would stand the panel's close back
                        // into a menu of a line that is gone — and leave the
                        // arrows walking it against a box that is not one.
                        *opened_list = Opened::default();
                        return Ok(Meanwhile::Command(owned));
                    }
                    let line = editor.text().to_owned();
                    steer.say(line);
                    notice = queue(editor, queued, turning, renderer.columns(), style);
                    moved = true;
                }

                Typed::Ignored | Typed::Ended => {}
            },

            // The key the cut rows themselves name, doing what they say while
            // the turn that cut them is still running. What it opens stands
            // under the tail, which is the one part of the screen a turn
            // writing above it does not reach.
            Meant::Expand => {
                opened.open(kept);
                moved |= opened.is_open();
            }

            // The other key that says *expand*, on the other thing that was cut
            // down to fit. Answered here as well as between turns because this
            // is where a plan is usually read: the tool that writes it runs
            // inside a turn, so the list moves under the reader while it stands.
            Meant::Plan => moved |= planning.expand(),

            // Two things a click can land on while a turn runs, and the same
            // round trip tells them apart as between turns. Up in the
            // transcript it is the view over one result rather than all of
            // them, asked for by pointing at the row that offered it. In the
            // box it is the cursor, put where the pointer is: the line being
            // written mid-turn is the one most worth pointing into, since it is
            // being written while something else is on screen holding the
            // reader's attention. A click anywhere else is answered by the
            // screen the reader was already looking at.
            Meant::Clicked(at) => match landed(renderer, editor, says, at) {
                Landed::Record(one) => {
                    opened.one(kept, one);
                    moved |= opened.is_open();
                }
                Landed::Line => moved = true,
                // The count is the one door on that row, and it is the door it
                // is between turns: the key cannot open the list here — it
                // means backgrounding the command the turn is waiting on — so
                // the click is the way the list is reached while a turn runs.
                Landed::Counted => moved |= stood(renderer, style, listing, background)?,
                Landed::Nothing => {}
            },

            Meant::Scrolled { back } => {
                renderer.notched(back)?;
            }

            Meant::PasteImage | Meant::Ignored => {}
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

    // And the plan beside it, for the half of the same reason that is not the
    // clock: this is the one place in a session where a tool call can change
    // what stands above the box while nobody is pressing anything.
    moved |= planning.moved();

    // The count under the box, for the rest of it. A command left running can
    // begin or end while the turn waits on it, and nothing then reaches this
    // loop to say so: no key, and no turn event until the call is answered.
    // This is the same refresh the row gets between turns off `arriving`, on
    // the beat the working row already moves to — so the number that says
    // something is still running is never a stale one, and a click on it has
    // something to land on.
    let count = background.count();
    if count != says.running {
        says.running = count;
        moved = true;
    }

    if moved
        && !expanding::under(renderer, style, kept, opened)?
        && !queueing::under(renderer, style, queued, viewing, steer)?
    {
        // A view takes the rows the box has, so a frame draws one of the three.
        // A window with no room for either view has closed it above, and the
        // box comes back in the same frame.
        let footing = Footing {
            turning,
            planning,
            opened_list,
        };
        match notice {
            Some(notice) => stand(renderer, editor, footing, &says.noticing(notice), style)?,
            None => stand(renderer, editor, footing, says, style)?,
        }

        // After the frame, never before it: Compacted and its successor may
        // already be waiting in the bounded inbox, but this pass has now offered
        // the factual 100% state to the renderer. The next ordinary beat removes
        // only that live row; no worker sleep and no committed scrollback.
        turning.finished_frame();
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

    turning.queueing(queued.waiting_all(), columns, style);
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
    /// A slash command was finished. The turn's own loop runs it — the keyboard
    /// is this loop's, and the command's panel is stood from there, where the
    /// turn it stands over can be kept rendering.
    Command(command::Owned),
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
/// silently join the third group. Clone rather than Copy, because one variant
/// carries a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Meant {
    /// The window changed under the box.
    Resized,
    /// The turn is asked to stop.
    Interrupt,
    /// Ctrl+Q: the queue itself, stood whole so it can be read and a waiting
    /// line taken back. Distinct from the press that finishes a line, which
    /// adds to it — that one is [`Meant::Editing`], because which press finishes
    /// a line is the editor's to say.
    QueueView,
    /// A run of characters beginning here, taken into the line in one edit.
    Typing(char),
    /// A bracketed paste, taken into the line whole: its newlines are characters
    /// in it rather than submissions, which is the difference between this and
    /// [`Meant::Typing`].
    Pasting(Box<str>),
    /// The line's own key — a cursor move, a delete, Ctrl-C against it, Ctrl-D.
    Editing(Key),
    /// Ctrl+O: the whole of what the transcript cut down to a row, stood under
    /// the turn that is still writing it.
    Expand,
    /// Ctrl+B: the command the turn is waiting on is to be left running, and the
    /// turn is to go on without it.
    ///
    /// Asked of the registry rather than done in the loop that reads the key. The
    /// command is being waited on by the worker thread, so what this side can do
    /// is put the request where that thread looks — the same shape, and the same
    /// latency, as asking a turn to stop.
    Background,
    /// Ctrl+T: the whole of the plan above the box, or the bounded list again.
    Plan,
    /// Ctrl+Y: the line on the clipboard. The line is the reader's whether or
    /// not a turn is running above it, so the key is the same on both sides.
    Copy,
    /// Ctrl+V: an image on the clipboard. A running turn owns the runner whose
    /// session store it must enter, so this is held apart and ignored here.
    PasteImage,
    /// A click, which lands on a result the transcript cut short, on the line
    /// being typed, or on nothing.
    ///
    /// Both numbers, because both are answered here: up in the transcript the
    /// row is the whole of it and a row that offered to expand offers it along
    /// its width, and in the box the column is which character the cursor goes
    /// before.
    Clicked(Pointed),
    /// The wheel, moving the transcript. `back` is towards the top of the
    /// session.
    Scrolled {
        /// Whether the notch was towards the top of the session.
        back: bool,
    },
    /// An arrow through the command list a `/`-started line has open, which is
    /// the one list a running turn can stand. `back` is the up-arrow, towards
    /// the top of the list.
    Arrow {
        /// Whether the mark moves towards the top of the list.
        back: bool,
    },
    /// Shift+Tab: the mode the next turn runs under, stepped to and held. The
    /// runner holding the mode is on the worker for this turn's length, so the
    /// step is taken here and applied when the runner is back — not ignored,
    /// as it was, which was a key that did nothing.
    Cycle,
    /// An arrow through a list there is none of. It has nothing to act on while
    /// a turn is running.
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

        Pressed::Key(Key::Char(first)) => Meant::Typing(first),
        // A paste mid-turn is typed into the box like any other: the turn above
        // it is none of the line's business, and its newlines are characters.
        Pressed::Pasted(text) => Meant::Pasting(text),

        // Ctrl-C among them, which is the point: it reaches the editor here the
        // same way it does between turns, so it throws away the line rather
        // than meaning something a running turn taught it to mean. Both
        // spellings of Return for the same reason: which one finishes a line is
        // `input.send`, and the editor is what reads it — decided here instead,
        // the bare key queued a line a reader had asked to break and the
        // modified one came back finished with nowhere to go.
        Pressed::Key(key) => Meant::Editing(key),

        Pressed::Expand => Meant::Expand,
        Pressed::Plan => Meant::Plan,
        Pressed::Background => Meant::Background,
        Pressed::Copy => Meant::Copy,
        Pressed::PasteImage => Meant::PasteImage,

        // The panel of what is waiting behind the turn, opened whole. Its own
        // meaning rather than the queue's: Return adds to the queue, and this
        // is the list that reads it and takes a line back.
        Pressed::Queue => Meant::QueueView,

        Pressed::Clicked { row, column } => Meant::Clicked(Pointed { row, column }),

        // The wheel, which means the same mid-turn as it does between turns:
        // the reader is looking back through what has already been said. That
        // it is being added to above them is the reason they would reach for it.
        Pressed::Scrolled { back } => Meant::Scrolled { back },

        // Ctrl+E among them: what it opens is an explanation of something
        // waiting to be decided about, and a running turn has decided already.
        // The arrows for a plainer reason — they walk a view that is not
        // standing, and a key that means nothing until Ctrl+O has been pressed
        // means nothing before it.
        // The two halves of a drag among them, for the reason the box gives:
        // the selection was offered every press first, so neither arrives.
        // Shift+Tab steps the mode the next turn runs under, held until the
        // runner is back: the running turn's mode was decided before it ran,
        // and a step that moved nothing now would be a key that did nothing.
        Pressed::Cycle => Meant::Cycle,
        // The arrows walk the command list a `/`-started line has open, as
        // they do at the prompt. A line that is not one has no list, and the
        // arm it reaches leaves the box alone, so an arrow with nothing to walk
        // is still the nothing it was.
        Pressed::Up => Meant::Arrow { back: true },
        Pressed::Down => Meant::Arrow { back: false },
        // The key that crosses regions among them: it is read by whatever is
        // standing, and nothing is standing while this arm is the one reading.
        Pressed::Explain
        | Pressed::Tab
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Meant::Ignored,
    }
}

/// What can change while one turn is running.
pub(super) struct During<'a> {
    pub(super) editor: &'a mut Editor,
    pub(super) queued: &'a mut Prompts,
    /// The row above the box, which is the one thing on screen that changes
    /// without anybody pressing anything.
    pub(super) turning: &'a mut Turning,
    /// The plan above that row. The other thing on screen a turn moves without
    /// a key being pressed — the tool that writes it runs on the worker thread.
    pub(super) planning: &'a mut Planning,
    /// What this turn's results have had no room to say, which is what Ctrl+O
    /// stands. Read only: the turn's own thread is what adds to it.
    pub(super) kept: &'a Kept,
    /// Whether that view is standing, and where over it.
    ///
    /// Owned by the session rather than by this call for the same reason
    /// `leaving` is, and one more: the view goes on standing after the turn it
    /// was opened under has ended.
    pub(super) opened: &'a mut Standing,
    /// Whether the queue is standing open to be gone over, which is the other
    /// thing that takes the box's rows while the turn goes on writing above.
    pub(super) viewing: &'a mut queueing::Standing,
    /// The command list a `/`-started line has open above the box.
    ///
    /// Owned by the session rather than read fresh each look: a turn is many
    /// looks at the keyboard, and the list has to survive from the character
    /// that opened it to the arrow that walks it.
    pub(super) opened_list: &'a mut Opened,
    /// The list of what is still running, which a click on the count under the
    /// box stands — the same door the key is at the prompt, kept across the
    /// looks at the channel a turn is one of.
    pub(super) listing: &'a mut Leaving,
    /// Mutable for the one fact on it that moves while the turn does: a
    /// command left running can begin or end between two of this loop's looks
    /// at the keyboard, with no turn event to carry the news, and the count
    /// under the box is the row that exists to report it. Read between turns,
    /// where the same refresh happens off the beat instead.
    pub(super) says: &'a mut Says,
    /// Where a command the turn is waiting on is asked to be left running.
    ///
    /// The request only: the command is being waited on by the worker thread, so
    /// what this side can do is put the ask where that thread looks — the same
    /// shape as asking a turn to stop.
    ///
    /// Named for what it holds rather than for what a press does with it, because
    /// the field below already carries the session's own use of that word: that
    /// one is the offer to leave, made on one Ctrl-C and taken on the next.
    pub(super) background: &'a Background,
    pub(super) style: Style,
    pub(super) cancel: &'a Cancel,
    /// Where a line typed now is pushed so the running turn works it in, between
    /// one pass of asking and running tools and the next.
    ///
    /// The queue above still keeps the line, and its own turn answers it after:
    /// steering is the agent adjusting course at once, not a reason the question
    /// stops being one. The two are what a mid-turn Enter means, and this is the
    /// half the turn in front of it reads.
    pub(super) steer: &'a crucible_core::Steer,
    /// Where a mode stepped to mid-turn is held until the runner is back.
    ///
    /// The runner holding the mode is on the worker for the turn's length, so
    /// a shift+tab pressed at it steps a pending slot here instead, and the
    /// row under the box says which mode that is — marked for the next turn,
    /// so the press is not dead and the running turn's mode is not lied about.
    pub(super) terms: &'a Terms,
    /// When Ctrl-C was last pressed against an empty line, if it is still the
    /// last key pressed.
    ///
    /// Owned by the loop above rather than by [`during`], which is called once
    /// per look at the channel the turn reports on: an offer made on one call
    /// is answered on a later one, and a clock that started again each time
    /// would be an offer nothing could ever take.
    pub(super) leaving: &'a mut Option<Instant>,
}

/// Re-wraps the live rows for the window's new size and re-measures the queue.
///
/// Both halves of what a resize costs the box: the renderer takes back rows
/// wrapped for a width the window no longer has, and the row above the box is
/// measured again for the one it does.
fn rewrap<T: Terminal>(
    renderer: &mut Renderer<T>,
    turning: &mut Turning,
    queued: &Prompts,
    style: Style,
) -> Result<(), Fatal> {
    renderer.resized()?;
    turning.queueing(queued.waiting_all(), renderer.columns(), style);
    Ok(())
}

/// Puts the box under the turn, with the cursor in it.
///
/// The renderer receives the box and footing together so their replacement is
/// one frame.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    footing: Footing<'_>,
    says: &Says,
    style: Style,
) -> Result<(), Fatal> {
    let footed = working(renderer, editor, footing, says, style);

    let pointed = footed.pointed.as_ref().map(|(at, row)| (*at, row));
    let prompt = replacement(&footed.boxed, footed.caret, pointed)?;
    renderer.replace(prompt, &footed.over, style.palette())?;
    Ok(())
}

/// The row for the mode the box is showing.
pub(super) fn saying(runner: &Runner) -> Says {
    let mode = runner.mode();

    Says {
        mode: Cow::Borrowed(mode.sentence()),
        keys: Cow::Borrowed(CYCLE),
        model: runner.model().to_owned(),
        provider: runner.serving(),
        effort: runner.effort().map(Effort::as_str),
        tone: tone(mode),
        asking: None,
        left: runner.left(),
        running_mode: mode,
        // Filled in by the frame rather than by the session, because it changes
        // while nothing else on this row does.
        running: 0,
    }
}

/// What is drawn around the box between turns.
///
/// Three borrows in one value because a call taking all three beside the
/// renderer, the editor and the style is a call nobody can read — which is what
/// the argument ceiling is there to stop.
#[derive(Clone, Copy)]
struct Around<'a> {
    /// The plan the agent is working to, above everything else.
    planning: &'a Planning,
    /// The commands a leading slash is offering, between the plan and the box.
    open: &'a Opened,
    /// What the rows under the box say.
    says: &'a Says,
}

/// The three of them at one call.
///
/// A function rather than a literal at each place that draws: the names are
/// worth writing once, and the call that draws the box is worth keeping on one
/// line.
fn around<'a>(planning: &'a Planning, open: &'a Opened, says: &'a Says) -> Around<'a> {
    Around {
        planning,
        open,
        says,
    }
}

/// Puts the box on screen with the cursor where the line was typed to, and
/// whatever the line has opened directly over it.
///
/// The box goes into the band that is held to a share of the window, because
/// that share is a rule about how much of the screen a long prompt may take
/// from what it is answering. Everything above it — the list and the plan —
/// goes into the band above, which has no share: a list is what the reader is
/// looking at while it is open, so the transcript is what gives way to it
/// rather than the box being pushed off the screen. Both bands reach the
/// renderer together so the prompt, status and map control come from one frame.
///
/// The list takes its share of the window before the plan is asked for any. It
/// is the shorter of the two and it was opened by the character last typed,
/// which is a stronger claim on the rows than a panel that was already there.
/// Neither is counted against the opening: a list opened over it is what the
/// reader is looking at, and shrinking it to keep a card that has already been
/// read is the wrong way round.
fn draw<T: Terminal>(
    renderer: &mut Renderer<T>,
    editor: &Editor,
    style: Style,
    around: Around<'_>,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let boxed = boxing(renderer, editor, around.says, around.says.left, style);

    // What is left for a list once the box and the blank row that keeps it off
    // the box have taken theirs.
    let room = renderer.rows().saturating_sub(boxed.rows.len() + 1);

    let mut listed = around.open.rows(columns, room, style.glyphs());

    // The row that keeps the box off whatever was last committed. A list opens
    // with its own, so this is only owed where there is no list -- and the box
    // is owed one either way, because a border drawn against the last line of
    // an answer reads as part of it.
    if listed.is_empty() {
        listed.push(Row::new());
    }

    let mut over = Vec::new();
    over.append(&mut around.planning.rows(
        columns,
        room.saturating_sub(listed.len()),
        style.glyphs(),
    ));
    over.append(&mut listed);

    let pointed = boxed.pointed.as_ref().map(|(at, row)| (*at, row));
    let prompt = replacement(&boxed.rows, boxed.caret, pointed)?;
    renderer.replace(prompt, &over, style.palette())?;
    Ok(())
}

/// Validates the prompt rows before they become one renderer replacement.
fn replacement<'a>(
    rows: &'a [Row],
    caret: Caret,
    pointed: Option<(usize, &'a Row)>,
) -> Result<PromptRows<'a>, Fatal> {
    PromptRows::new(rows, caret, pointed).ok_or_else(|| {
        Fatal::from(TerminalError::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "the prompt replacement disagrees with its rows",
        )))
    })
}

/// The box rows in their resting state and the sole pointable row contrasted.
struct Boxed {
    rows: Vec<Row>,
    pointed: Option<(usize, Row)>,
    caret: Caret,
}

/// Lays out the prompt rows once and composes only its pointable row a second time.
fn boxing<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &Editor,
    says: &Says,
    left: Option<u8>,
    style: Style,
) -> Boxed {
    let room = Prompt::room(renderer.rows());
    let columns = renderer.columns();
    let glyphs = style.glyphs();
    let prompt = writing(editor, says, left, false, room);
    let (rows, pointed) = prompt.rows_with_pointed(columns, glyphs);
    let caret = prompt.caret(columns);

    Boxed {
        rows,
        pointed,
        caret,
    }
}

/// The box as it is being typed into.
///
/// One place the fields are named, because [`draw`] and the click that follows
/// it have to lay the same component out: a click read against a box drawn any
/// other way lands somewhere nobody pointed at.
fn writing<'a>(
    editor: &'a Editor,
    says: &'a Says,
    left: Option<u8>,
    running_pointed: bool,
    room: usize,
) -> Prompt<'a> {
    Prompt {
        draft: Draft::projected(editor.projection()),
        left: Remaining::new(left),
        mode: says.mode.as_ref(),
        tone: says.tone,
        hint: &says.keys,
        model: says.model.as_str(),
        provider: says.provider,
        effort: says.effort,
        asking: says.asking.as_deref(),
        commands: CommandCount::new(says.running, running_pointed),
        room,
    }
}

/// Where a click landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pointed {
    /// The screen row the pointer was on.
    row: usize,
    /// How many columns from the left of it.
    column: usize,
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
    /// The row under the box naming what is still running, which is the one thing
    /// on it that is an offer rather than a fact.
    Counted,
    /// The border, a blank row, the shell's own output from before crucible
    /// started — or a terminal that would not say where its cursor is.
    Nothing,
}

/// Reads where a click landed, moving the cursor where it landed on the line.
///
/// What is under a window row is the renderer's answer: it shares the window
/// out and it holds the transcript, so a row of one band and a line of the
/// session come out of the same reading. What is left here is where the box
/// sits inside what is standing, which the frame that drew it said. Leaving the
/// cursor where it is is the right answer to a click that did not land on the
/// line.
fn landed<T: Terminal>(
    renderer: &Renderer<T>,
    editor: &mut Editor,
    says: &Says,
    at: Pointed,
) -> Landed {
    let row = match renderer.aimed(at.row) {
        Some(Aimed::Line(line)) => return Landed::Record(line),
        Some(Aimed::Boxed(row)) => row,
        // A list or a plan standing over the box. Answered where it is drawn,
        // by the loop that opened it, and not here.
        Some(Aimed::Stood(_)) | None => return Landed::Nothing,
    };

    let prompt = writing(
        editor,
        says,
        says.left,
        false,
        Prompt::room(renderer.rows()),
    );

    // Asked of the box, because which row the count came out on is the box's own
    // arithmetic at this width. The column is not asked about: the row is the
    // affordance, and nothing else on it is one, so a click anywhere along it
    // means the one thing it could mean.
    if prompt.counting(renderer.columns(), row) {
        return Landed::Counted;
    }

    // A line and a column within it, which is what the box's arithmetic knows:
    // a newline makes a bare column into the whole text ambiguous, so the editor
    // is placed by the pair rather than by one number.
    let Some((line, into)) = prompt.clicked(renderer.columns(), row, at.column) else {
        return Landed::Nothing;
    };

    if editor.place_at(line, into) == Typed::Changed {
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
pub(super) struct Opened {
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
    pub(super) fn filtered(said: &str, glyphs: Glyphs) -> Self {
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
    pub(super) fn up(&mut self) -> bool {
        let moved = self.at > 0;
        self.at = self.at.saturating_sub(1);
        moved
    }

    /// Moves it on a row.
    pub(super) fn down(&mut self) -> bool {
        let last = self.shown.len().saturating_sub(1);
        let moved = self.at < last;
        self.at = last.min(self.at + 1);
        moved
    }

    /// What return runs, or `None` where there is no list and the line is what
    /// was typed.
    pub(super) fn chosen(&self) -> Option<&'static str> {
        self.shown.get(self.at).map(|one| one.name)
    }

    /// The rows to open above the box, and the blank row that keeps them off
    /// it.
    ///
    /// A list with no room for it is not opened, and not cut down to what there
    /// is room for either: a list cut off at the top reads as the whole list,
    /// which is worse than drawing nothing at all. Nothing is what a reader can
    /// tell is nothing.
    pub(super) fn rows(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
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
    let local = local(editor, chosen.is_some());
    let typed = editor.take();
    let said = chosen.map_or(typed, str::to_owned);

    // Back to the foot before a word of it is drawn: what was just sent is
    // about to be answered at the bottom, and somebody who had scrolled up to
    // read is done doing that the moment they send something.
    renderer.follows()?;
    renderer.settle()?;

    // What was asked is a block like any other, and what parts one block from
    // the next is a row of nothing. Asked on the way in rather than left behind
    // on the way out, because this cannot know it was the last: a session that
    // parted afterwards would end on a blank row under the final answer, and the
    // shell's own prompt would come back one row lower than it left.
    renderer.apart()?;
    renderer.landmark();
    let responsive = said.clone();
    renderer.responsive(
        responsive.len(),
        Box::new(move |columns| {
            Prompt::committed(
                &responsive,
                columns,
                style.glyphs(),
                style.palette().bands(),
            )
        }),
    )?;

    Ok(Asked::Said(Said::new(said, local)))
}

/// Whether visible non-element text selected the local command path.
fn local(editor: &Editor, chosen: bool) -> bool {
    if chosen {
        return true;
    }

    let source = editor.text().trim();
    if editor.projection().text() != editor.text() {
        return false;
    }
    if command::wanted(source).is_none() {
        return false;
    }

    let word = source
        .split_once(char::is_whitespace)
        .map_or(source, |(word, _)| word);
    editor.projection().text().trim_start().starts_with(word)
}

#[cfg(test)]
mod tests;
