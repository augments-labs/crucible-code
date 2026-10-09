//! Rendering, full screen by default: crucible owns the window and every row
//! in it. Native mode, the other [`ScreenMode`], is the exception to most of
//! what follows and says so in its own module.
//!
//! This process takes the alternate screen, so the terminal's scroll buffer is
//! not where the session lives. What replaces it is [`Record`] — a bounded
//! window of the lines that have been said, folded to the width the terminal
//! has now, with a viewport over it that the reader moves. Nothing here is
//! proportional to how long the session has run: the record drops its oldest
//! lines at a ceiling it owns, and a frame folds only the lines the transcript
//! band happens to cover.
//!
//! The window is shared out by [`crate::bands`] and every row of it belongs to
//! exactly one band, which is what makes the whole design absolute: a frame
//! names the row it writes and never counts how far to move or how much to
//! erase. There is no rewind, no live region, and no arithmetic about how tall
//! the last frame turned out to be — the class of defect this crate has spent
//! the most on cannot be expressed here. Native mode has all three, because
//! the reader's own buffer is a screen nobody owns; they are confined to
//! [`native`], whose frame is the one place a row is counted rather than
//! named.
//!
//! What a frame costs is bounded by the window and not by the delta: the rows
//! whose painted bytes are the same as last time are not written at all, which
//! [`painted`] decides, and a frame that changed nothing costs no write and no
//! flush.
//!
//! A run whose output is redirected has no screen to own, and no frame either:
//! text goes straight through at the moment it arrives, stripped of the escape
//! bytes an untrusted string put in it. It keeps the same record all the same,
//! so what separates one block from the next is decided once rather than twice.
//! Nothing on that path can write an escape sequence, because nothing on it
//! goes near one.

use crate::bands::{Bands, Wants};
use crate::clipboard;
use crate::color::{Palette, Slot};
use crate::escape::Escapes;
use crate::files::Files;
use crate::forge::Forge;
use crate::glyphs::Glyphs;
use crate::markdown::Markdown;
use crate::record::Record;
use crate::row::Row;
use crate::scroll_rail::{self, ScrollRail};
use crate::select::{self, Place, Taken, View};
use crate::terminal::keys::{Pressed, pressed, waiting};
use crate::terminal::system::ResizeFlag;
use crate::terminal::{Size, Terminal, TerminalError};
use crate::width;

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod frame;
mod native;
mod painted;
mod recall;

use native::Native;
use painted::Painted;
pub use recall::Recall;
use recall::Watch;

/// How far one notch of the wheel moves the transcript, until told otherwise.
///
/// A renderer nobody configured still has to answer the wheel, and the answer
/// that is wrong here is nought: it looks like a terminal that has stopped
/// reporting rather than like a setting waiting to be made. Three rows is a
/// short paragraph, which is enough to see the picture move.
const NOTCH: i32 = 3;

/// How often a drag resting at an edge of the transcript carries it another
/// row.
///
/// Slow enough that a reader who overshot the top row by a hair can bring the
/// pointer back before more than a line or two has gone, quick enough that a
/// reader who wants the paragraph above gets there without letting go.
const CREEP: Duration = Duration::from_millis(60);

/// The most of a native window a panel standing over or in place of the box
/// may take: half, rounded down.
///
/// Half, so that what the panel stands over can still be read while it is
/// open — and, in native mode, so that it is still there when the panel
/// closes. The same share the prompt is held to (`crate::bands`), kept apart
/// from it because they are two rules: that one is about a prompt being
/// written, this one is about the rows a panel would otherwise push into the
/// terminal's scrollback for good.
const PANEL_SHARE: usize = 2;

/// Where the cursor rests inside a band.
///
/// Counted from the top left of the rows the caller handed over rather than
/// from the window's origin, because which band those rows end up in is this
/// module's answer and not the caller's: a component knows only which of its
/// own rows the cursor belongs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caret {
    /// Which row of the rows, from the top.
    pub row: usize,
    /// How many columns from the left of it.
    pub column: usize,
}

/// Prompt rows replaced together, including the one row that can be pointed.
#[derive(Debug, Clone, Copy)]
pub struct PromptRows<'a> {
    /// The resting rows of the prompt component.
    rows: &'a [Row],
    /// Where the terminal cursor belongs inside them.
    caret: Caret,
    /// A relative row and that row in its pointed palette state.
    pointed: Option<(usize, &'a Row)>,
}

impl<'a> PromptRows<'a> {
    /// A prompt replacement, with an optional pointed form of one of its rows.
    ///
    /// An out-of-range target or one whose text differs from the resting row
    /// is not a target. This keeps the renderer from associating an action with
    /// a different visible row while letting a prompt with no surviving
    /// command control use the same constructor. Returns `None` when the caret
    /// or pointed target does not belong to these rows.
    #[must_use]
    pub fn new(rows: &'a [Row], caret: Caret, pointed: Option<(usize, &'a Row)>) -> Option<Self> {
        let caret_row = rows.get(caret.row)?;
        if caret.column > caret_row.columns() {
            return None;
        }
        if pointed.is_some_and(|(at, row)| {
            rows.get(at)
                .is_none_or(|resting| resting.text() != row.text())
        }) {
            return None;
        }

        Some(Self {
            rows,
            caret,
            pointed,
        })
    }
}

/// What is under a row of the window.
///
/// The answer to a click, and the only thing that turns a row the terminal
/// reported into something the session can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aimed {
    /// A line of the transcript, counted from the first of the session.
    Line(usize),
    /// A row of what is standing over the box, counted from the top of it.
    ///
    /// A running turn's rows, or a list a line has opened. Told apart from the
    /// box because the two are laid out by different components and a click is
    /// answered by whichever drew the row — the same row number means a
    /// different thing in each, and nothing further down could tell.
    Stood(usize),
    /// A row of the box, counted from the top of it.
    Boxed(usize),
}

/// The rows standing at the foot of the window, as they were handed in and
/// painted.
///
/// Painted when they are set rather than once per frame: what they say changes
/// when the session changes, and a turn is a great many frames. Painted again
/// when the window changes size, from the rows they were handed in as, so
/// that what stands is never written wider than the window it is written
/// into and never goes missing while the caller has not yet laid out the
/// next one. A row laid out for a wider window is cut at the new edge until
/// then.
///
/// Two slots, in the order they are drawn down the screen, and either may be
/// full without the other: a turn stands in the first while it runs, a list a
/// line opened stands there between turns, and the box holds the second
/// whenever there is one to type into.
///
/// The rows a running turn leads the first slot with read as the transcript's
/// last rows, so the scroll rail stands beside them too, and its cell there
/// changes from one frame to the next as the transcript's does.
///
/// Of every row of the first slot, the cells it drew are kept as well, which
/// is all a press needs of it: the painted copy cannot say where its text ends.
#[derive(Debug, Default)]
struct Standing {
    /// What a running turn is showing, and anything else standing over the
    /// box, painted.
    turn: Vec<String>,
    /// The cells each row of `turn` drew, as [`Renderer::cells`] answers for it.
    drew: Vec<Range<usize>>,
    /// The first rows of `turn` as they were handed in, where a running turn
    /// put them there: the turn's own rows, laid out at the transcript's
    /// width. Empty between turns and under anything else standing there.
    running: Vec<Row>,
    /// The rest of `turn` as it was handed in: whatever stands under the
    /// turn's own rows and over the box.
    over: Vec<Row>,
    /// The palette `running` and `over` were handed in with.
    ran: Option<Palette>,
    /// Where the cursor belongs in it, where anything is typing into it.
    turned: Option<Caret>,
    /// The box, painted.
    prompt: Vec<String>,
    /// The box as it was handed in.
    boxed: Vec<Row>,
    /// The palette `boxed` was handed in with.
    boxed_in: Option<Palette>,
    /// Where the cursor belongs in the box.
    prompted: Option<Caret>,
}

impl Standing {
    /// Forget both, for a transcript that has been emptied under them.
    fn clear(&mut self) {
        self.turn.clear();
        self.drew.clear();
        self.running.clear();
        self.over.clear();
        self.ran = None;
        self.turned = None;
        self.prompt.clear();
        self.boxed.clear();
        self.boxed_in = None;
        self.prompted = None;
    }

    /// Holds `turn` and `over` as the first slot, to be painted by
    /// [`Standing::paint_turn`].
    fn stands(&mut self, turn: &[Row], over: &[Row], palette: Palette) {
        self.running.clear();
        self.running.extend_from_slice(turn);
        self.over.clear();
        self.over.extend_from_slice(over);
        self.ran = Some(palette);
    }

    /// Holds `rows` as the box and paints them `columns` wide.
    fn boxes(&mut self, rows: &[Row], palette: Palette, columns: usize) {
        self.boxed.clear();
        self.boxed.extend_from_slice(rows);
        self.boxed_in = Some(palette);
        self.paint_box(columns);
    }

    /// Paints both slots again from what they were handed in as, for a window
    /// `columns` wide whose transcript is `folds` wide.
    fn repaint(&mut self, folds: usize, columns: usize) {
        self.paint_turn(folds, columns);
        self.paint_box(columns);
    }

    /// Paints the first slot, the turn's own rows `folds` wide and the rest
    /// `columns` wide, and measures the cells each of its rows draws.
    fn paint_turn(&mut self, folds: usize, columns: usize) {
        let rows = || {
            self.running
                .iter()
                .map(move |row| (row, folds))
                .chain(self.over.iter().map(move |row| (row, columns)))
        };
        match self.ran {
            Some(palette) => paint(rows(), &palette, &mut self.turn),
            None => self.turn.clear(),
        }
        self.drew.clear();
        self.drew.extend(rows().map(|(row, room)| drawn(row, room)));
    }

    /// Paints the box.
    fn paint_box(&mut self, columns: usize) {
        match self.boxed_in {
            Some(palette) => paint(
                self.boxed.iter().map(|row| (row, columns)),
                &palette,
                &mut self.prompt,
            ),
            None => self.prompt.clear(),
        }
    }
}

/// Whether an answer is on its way in.
///
/// Two states rather than a flag because they are what the transcript is doing,
/// and a reader of [`Renderer::apart`] should not have to work out which way
/// round the flag ran.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Arriving {
    /// Nothing is being said. Whatever goes down next is a block of its own.
    #[default]
    Nothing,
    /// An answer is under way, in whatever pieces the wire cut it into. No row
    /// parts a block from itself.
    Answer,
}

/// Where a renderer draws.
///
/// Settled once, when the renderer is made, because the two are different
/// promises about the terminal rather than two looks of one: one takes a screen
/// of its own and keeps the session there, the other writes into the buffer the
/// reader's shell was using and leaves what is finished to the terminal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScreenMode {
    /// A screen of crucible's own: its own scrollback, rail and selection.
    #[default]
    Fullscreen,
    /// The terminal's own buffer: what is finished is written once into its
    /// scrollback, and only a live region at the bottom is drawn again.
    Native,
}

/// Draws the session: onto a screen this process owns, or at the foot of the
/// terminal's own buffer.
#[derive(Debug)]
pub struct Renderer<T: Terminal> {
    terminal: T,
    /// Where finished rows go and what a frame may redraw, in native mode.
    ///
    /// `None` is the full screen. Held as the state native mode needs rather
    /// than as a flag beside it, so the mode and what it keeps cannot disagree.
    native: Option<Native>,
    /// Everything that has been said, and where in it the reader is looking.
    record: Record,
    /// The rows at the foot of the window that are not the transcript.
    standing: Standing,
    /// The row of the prompt that offers an action while the box is standing,
    /// and the cells of it the offer takes.
    ///
    /// Relative to the prompt band. The renderer owns the absolute placement,
    /// so this is enough for it to decide whether a motion crossed the offer
    /// without teaching it what the row means. The cells are the ones the
    /// caller drew pointed ([`door`]): the rest of the row says facts beside
    /// the offer, and a pointer there is over none.
    prompt_target: Option<(usize, Range<usize>)>,
    /// Whether a pointer transition is waiting for the prompt to be replaced.
    pointed_changed: bool,
    /// The size the record is folded for and the bands are shared out over.
    ///
    /// Held rather than asked for per frame: a read costs a syscall, and
    /// [`Renderer::resized`] is what keeps it true, called for the press that
    /// reports a resize or by the first frame after a [`ResizeFlag`] says one
    /// happened.
    size: Size,
    /// What each row of the window is currently showing, and the frame that
    /// changes it.
    painted: Painted,
    /// Reused across deltas: one run of text with its escape bytes taken out.
    free: String,
    /// How far into an escape sequence streamed text is.
    ///
    /// Held across deltas because a sequence arrives split across two of them
    /// as often as not. What it is for: colour in a tool result is bytes an
    /// untrusted string put there, and a row of the record is spans this
    /// program painted from a palette rather than bytes it forwarded.
    escapes: Escapes,
    /// Reads the markers out of the model's markdown and says what each run is.
    ///
    /// Held here rather than made fresh per delta, for the reason the escapes
    /// above are: a fence arrives split across two deltas as often as not.
    markdown: Markdown,
    /// The repository the transcript's bare numbers are counted against.
    ///
    /// Held here rather than in the reader alone because the reader is thrown
    /// away and made again between messages, and which checkout this is does
    /// not change while the session runs.
    forge: Option<Forge>,
    /// The checkout the transcript's links to paths are read against, held
    /// here for the reason the forge is.
    files: Option<Files>,
    /// Whether an answer is still arriving.
    ///
    /// Set by the first piece of one and put down by [`Renderer::settle`],
    /// which is the call that ends an answer however it ended. What it is for
    /// is [`Renderer::apart`]: a caller asking for the row between two blocks
    /// asks on every piece, because the first is the only one worth asking on
    /// and the caller cannot tell which that was. Nothing the record or the
    /// markdown reader holds can tell it either — a piece cut exactly at a line
    /// break leaves both of them settled while the block it was in the middle
    /// of is still open — so the one thing that knows is whether an answer is
    /// under way at all.
    arriving: Arriving,
    /// The palette this run resolved.
    ///
    /// Read at the moment a row is drawn rather than at the moment it was said,
    /// which is what lets a theme chosen mid-session repaint what is already on
    /// screen. Plain until [`Renderer::wears`] says otherwise, which is what
    /// leaves a renderer nobody told showing the answer as it arrived.
    palette: Palette,
    /// Which characters the markdown reader draws a bullet and a quote bar
    /// with.
    ///
    /// Settled the same way the palette is: what a reader's font has is a fact
    /// about the terminal rather than about the answer. Unicode until
    /// [`Renderer::draws`] says otherwise.
    glyphs: Glyphs,
    /// Whether the transcript gives up its rightmost column to the scroll
    /// rail, where the window can spare one.
    ///
    /// Off until [`Renderer::rails`] says otherwise, so a renderer nobody
    /// configured — a preview, a test — folds at the window's own width.
    rails: bool,
    /// Where on the thumb a press on the rail took hold of it, in rail rows
    /// from the thumb's first, from the press to the release.
    ///
    /// What a drag keeps under the pointer: the thumb follows the pointer by
    /// the row it was taken by, so a drag that starts on its middle does not
    /// jump to put its top there.
    grip: Option<usize>,
    /// How many rows of the transcript one notch of the wheel moves.
    ///
    /// Held here for the reason the palette and the glyphs are: it is settled
    /// where configuration is read, and asked about on a path that may not open
    /// a file. What number it should be is not this crate's to decide — a wheel
    /// is a piece of hardware whose notch means whatever its owner has told
    /// their system it means. Three rows until [`Renderer::rolls`] says
    /// otherwise, which is a notch that moves something for a reader nobody
    /// configured.
    notch: i32,
    /// What the reader has dragged over, if anything.
    ///
    /// Its ends in the transcript are record rows, so a scroll carries the
    /// highlight with the words. Dropped by anything that changes what a
    /// record row is — a resize refolds every line — and by anything that
    /// replaces the record under it.
    taken: Option<Taken>,
    /// Where the pointer is while a button is held down, from the press to the
    /// release.
    ///
    /// What lets a wheel turned mid-drag extend the drag to the words that have
    /// just arrived under the pointer, and what a drag resting at an edge of
    /// the transcript scrolls towards.
    held: Option<(usize, usize)>,
    /// When a drag resting at an edge of the transcript next carries it a row.
    creeps: Option<Instant>,
    /// Which window row and column the pointer was last reported on.
    ///
    /// A place rather than what is on it, and kept across everything that moves
    /// the picture: what the pointer is over is worked out again for every
    /// frame, from this and from where the record has got to. A pointer resting
    /// still while an answer scrolls under it is over whatever is under it now,
    /// which is what a reader watching the screen sees.
    pointing: Option<(usize, usize)>,
    /// What may call off a wait on the keyboard, where anything may.
    recall: Option<Arc<dyn Recall>>,
    /// What says the window changed size, where anything does.
    resizes: Option<ResizeFlag>,
}

impl<T: Terminal> Renderer<T> {
    /// A renderer drawing on `terminal`.
    ///
    /// Nothing here can fail, and the signature says so. A terminal that will
    /// not report a size is still a terminal worth drawing on, so the size is
    /// guessed rather than refused — and a guess that turns out wrong is
    /// corrected by [`Renderer::resized`] at the next prompt. Somewhere that
    /// never answers keeps the guess for the whole session, which is the right
    /// trade for a pipe.
    #[must_use]
    pub fn new(terminal: T) -> Self {
        Self::drawing(terminal, ScreenMode::Fullscreen)
    }

    /// A renderer drawing on `terminal` in `mode`.
    ///
    /// [`Renderer::new`] is the full screen. What decides which a session gets
    /// is configuration, read once at launch: changing it under a session
    /// would leave half of it in a scrollback the other mode does not keep.
    #[must_use]
    pub fn drawing(terminal: T, mode: ScreenMode) -> Self {
        // The one place a size is guessed: with none known yet there is
        // nothing to keep, and every later query that fails keeps the size
        // already known rather than guessing again.
        let size = terminal.size().unwrap_or(Size::FALLBACK);

        Self {
            record: Record::new(size.columns),
            terminal,
            native: match mode {
                ScreenMode::Fullscreen => None,
                ScreenMode::Native => Some(Native::default()),
            },
            standing: Standing::default(),
            prompt_target: None,
            pointed_changed: false,
            size,
            painted: Painted::new(),
            free: String::new(),
            escapes: Escapes::default(),
            markdown: Markdown::default(),
            forge: None,
            files: None,
            arriving: Arriving::Nothing,
            palette: Palette::plain(),
            glyphs: Glyphs::default(),
            rails: false,
            grip: None,
            notch: NOTCH,
            taken: None,
            held: None,
            creeps: None,
            pointing: None,
            recall: None,
            resizes: None,
        }
    }

    /// Lets `recall` call off every wait on the keyboard from here on, as it
    /// says in [`Recall`].
    pub fn recalled_by(&mut self, recall: Arc<dyn Recall>) {
        self.recall = Some(recall);
    }

    /// Asks the window its size only after `resizes` says it changed.
    pub fn watches_size(&mut self, resizes: ResizeFlag) {
        self.resizes = Some(resizes);
    }

    /// Waits for one press, carrying a drag resting at an edge of the
    /// transcript along while it waits.
    ///
    /// `None` where the scroll rail or the selection consumed the press. A
    /// drag's next step wakes this wait, moves the transcript, and waits again;
    /// it never becomes a key the caller could mistake for input. A wait a
    /// [`Recall`] watches wakes on a beat as well, to ask it, and asks once
    /// more after the key is read.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be read or written, and
    /// [`TerminalError::Recalled`] if the wait was called off.
    pub fn pressed(&mut self) -> Result<Option<Pressed>, TerminalError> {
        self.pressed_from(waiting, pressed)
    }

    /// [`Renderer::pressed`], polling with `poll` and reading with `read`.
    fn pressed_from(
        &mut self,
        mut poll: impl FnMut(Duration) -> Result<bool, TerminalError>,
        read: impl FnOnce() -> Result<Pressed, TerminalError>,
    ) -> Result<Option<Pressed>, TerminalError> {
        self.seal()?;
        let watch = Watch::began(self.recall.as_ref());
        loop {
            watch.held()?;
            if let Some(patience) = watch.patience(self.rests_in())
                && !poll(patience)?
            {
                self.repose()?;
                continue;
            }
            watch.held()?;
            let arrived = read()?;
            watch.over()?;
            return self.took(arrived);
        }
    }

    /// Whether a press is ready within `patience`, shortening that wait to a
    /// resting drag's next step when necessary.
    ///
    /// The caller already has something else to watch — a running turn or a
    /// login attempt — so a step taken answers `false` and lets that caller
    /// make its ordinary pass before polling again. A wait a [`Recall`]
    /// watches is taken a beat at a time, asking it between beats and once
    /// more after the last.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be read or written, and
    /// [`TerminalError::Recalled`] if the wait was called off.
    pub fn waiting(&mut self, patience: Duration) -> Result<bool, TerminalError> {
        self.waiting_from(patience, waiting)
    }

    /// [`Renderer::waiting`], polling with `poll`.
    fn waiting_from(
        &mut self,
        patience: Duration,
        mut poll: impl FnMut(Duration) -> Result<bool, TerminalError>,
    ) -> Result<bool, TerminalError> {
        self.seal()?;
        self.repose()?;
        let watch = Watch::began(self.recall.as_ref());
        let mut left = self.rests_in().map_or(patience, |due| due.min(patience));
        loop {
            watch.held()?;
            let beat = watch.patience(Some(left)).unwrap_or(left);
            if poll(beat)? {
                watch.over()?;
                return Ok(true);
            }
            left = left.saturating_sub(beat);
            if left.is_zero() {
                watch.over()?;
                self.repose()?;
                return Ok(false);
            }
        }
    }

    /// What a press means once the scroll rail and the selection have had it.
    ///
    /// The one seam every input loop wraps its reads in, so that a drag works
    /// the same wherever the reader started it: the loop hands over what
    /// arrived and gets back what is left for it to answer. `None` where the
    /// press belonged to the selection and nothing else is owed — the pointer
    /// moving under a held button, and the button coming up again.
    ///
    /// A click is handed back rather than swallowed. It anchors a drag that may
    /// never happen, and until it does it still means whatever it meant to the
    /// loop underneath — a caret placed in the box, a cut result opened. A
    /// press on the rail is the exception: it moves the transcript, where there
    /// is anywhere to move it, and nothing else, so it goes no further, and
    /// neither does the drag it starts.
    ///
    /// A resize is taken here too, by [`Renderer::resized`], and handed back
    /// for the loop to lay out again what it stands, so no loop has to ask
    /// for it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn took(&mut self, arrived: Pressed) -> Result<Option<Pressed>, TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(Some(arrived));
        }

        // The window's size is the renderer's to keep, so a press saying it
        // changed is taken here, whichever loop read it: the record is folded
        // again and what stands is painted at the new width. The press goes
        // on so the loop can lay out again what it stands.
        if arrived == Pressed::Resized {
            self.resized()?;
            return Ok(Some(arrived));
        }

        // The terminal's own buffer has its own selection and its own wheel,
        // and this process asked for no presses of the pointer. One that
        // arrives anyway is answered by nothing: there is no rail to steer, no
        // selection to make and no band to scroll.
        if self.native.is_some()
            && matches!(
                arrived,
                Pressed::Hovered { .. }
                    | Pressed::Clicked { .. }
                    | Pressed::Dragged { .. }
                    | Pressed::Released { .. }
                    | Pressed::Scrolled { .. }
            )
        {
            return Ok(None);
        }

        let bands = self.bands();
        if let Pressed::Hovered { row, column } = arrived {
            if self.points(row, column, &bands) {
                self.draw()?;
            }
            return Ok(None);
        }
        if self.steered(&arrived, &bands)? {
            return Ok(None);
        }

        match arrived {
            // A new press drops whatever the last one selected, which is how a
            // reader puts a selection away: click, anywhere.
            Pressed::Clicked { row, column } => {
                let had = self.taken.is_some_and(|taken| !taken.empty());
                let view = self.view(&bands);
                self.taken = Some(Taken::opened(row, column, &view));
                self.held = Some((row, column));
                self.creeps = None;
                if had {
                    self.draw()?;
                }
                Ok(Some(arrived))
            }
            Pressed::Dragged { row, column } => {
                self.held = Some((row, column));
                if self.taken.is_some() {
                    // The band scrolls a row towards a pointer at its edge
                    // before the drag is placed, so the words that arrive
                    // under the pointer are the words it reaches.
                    if self.creeps.is_none() {
                        self.creeps = Some(Instant::now());
                    }
                    self.crept()?;
                    self.reached();
                    self.draw()?;
                }
                Ok(None)
            }
            // Copied where the drag covered something, and quietly dropped
            // where it did not: a button coming up after a plain click is not
            // an empty clipboard, it is nothing at all.
            Pressed::Released { .. } => {
                self.held = None;
                self.creeps = None;
                if self.taken.is_some_and(|taken| !taken.empty()) {
                    let said = self.selected();
                    self.copied(&said)?;
                }
                Ok(None)
            }
            // A wheel turned with the button down is part of the drag: the
            // band moves and the drag reaches the words now under the pointer.
            // Answered here so the loop underneath does not scroll it twice.
            Pressed::Scrolled { back } if self.held.is_some() && self.taken.is_some() => {
                self.notched(back)?;
                self.reached();
                self.draw()?;
                Ok(None)
            }
            _ => Ok(Some(arrived)),
        }
    }

    /// Answers a press that belongs to the scroll rail, and says whether it
    /// did.
    ///
    /// A press on the thumb takes hold of it where it was pressed and moves
    /// nothing, a mark the thumb covers included. A press on a mark off the
    /// thumb lands on the prompt the mark stands for, which is then the rail's
    /// current prompt for as long as it starts in the band and no prompt is
    /// sent after it, and anywhere else on the rail puts the thumb's middle
    /// there, as near as the rail's ends allow; either way the thumb is then
    /// held where the pointer is. A drag moves the held thumb, and the
    /// transcript with it, and the release lets go. A rail with no thumb — a
    /// record that fits — has nowhere to go, so a press on it moves nothing; it
    /// is still the rail's, so it names no line beside it, just as a pointer
    /// resting there lights none.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the frame could not be written.
    fn steered(&mut self, arrived: &Pressed, bands: &Bands) -> Result<bool, TerminalError> {
        // The band's height is what the record seeks against; the rail's,
        // which is the band's and the running turn's under it, is what a row
        // of the rail is counted in.
        let rows = bands.transcript.len();
        let railed = self.railed_rows(bands);
        let height = railed.len();
        match *arrived {
            Pressed::Clicked { row, column } => {
                // A press is a new hold, whatever the last one was: a thumb
                // whose release never arrived is let go here, or the drag
                // this press starts would move it.
                self.grip = None;
                if Some(column) != self.rail_column() || !railed.contains(&row) {
                    return Ok(false);
                }
                let Some((rail, thumb)) = self
                    .rail(bands)
                    .and_then(|rail| rail.thumb().map(|thumb| (rail, thumb)))
                else {
                    self.held = None;
                    self.creeps = None;
                    self.unselects();
                    self.draw()?;
                    return Ok(true);
                };
                let at = row - railed.start;
                if !thumb.contains(&at) {
                    let top = if let Some(prompt) = rail.prompt_at(at, self.record.prompts()) {
                        self.record.lands(prompt);
                        prompt
                    } else {
                        rail.top_for(at.saturating_sub(thumb.len() / 2))
                    };
                    self.record.seek(top, rows);
                }
                let thumb = self
                    .rail(bands)
                    .and_then(|rail| rail.thumb())
                    .unwrap_or(thumb);
                self.grip = Some(
                    at.saturating_sub(thumb.start)
                        .min(thumb.len().saturating_sub(1)),
                );
                self.held = None;
                self.creeps = None;
                self.unselects();
                self.draw()?;
                Ok(true)
            }
            Pressed::Dragged { row, column } => {
                let Some(grip) = self.grip else {
                    return Ok(false);
                };
                // A drag reports where the pointer is as motion does, so the
                // mark grown under it is the one under it now, not the one
                // the press was on.
                let mut owed = self.points(row, column, bands);
                if let Some(rail) = self.rail(bands)
                    && let Some(thumb) = rail.thumb()
                {
                    let at = row
                        .saturating_sub(railed.start)
                        .min(height.saturating_sub(1));
                    let start = at
                        .saturating_sub(grip)
                        .min(height.saturating_sub(thumb.len()));
                    owed |= self.record.seek(rail.top_for(start), rows);
                }
                if owed {
                    self.draw()?;
                }
                Ok(true)
            }
            Pressed::Released { .. } => Ok(self.grip.take().is_some()),
            _ => Ok(false),
        }
    }

    /// Moves the pointer to `row` and `column`, and says whether this renderer
    /// owes a frame for it: when what the pointer lights, the prompt row it is
    /// on or the rail row it is on changed.
    ///
    /// Motion within the same targets owes nothing, so all-motion reporting
    /// does not turn into one frame per cell.
    fn points(&mut self, row: usize, column: usize, bands: &Bands) -> bool {
        let lit = self.pointed();
        let prompt_pointed = self.prompt_pointed();
        let railed = self.rail_pointed(bands);
        self.pointing = Some((row, column));
        let changed = lit != self.pointed()
            || prompt_pointed != self.prompt_pointed()
            || railed != self.rail_pointed(bands);
        if changed && self.prompt_target.is_some() {
            // The caller has the pointable row in both of its palette
            // states. It replaces that row and the rest of the prompt in
            // one candidate rather than letting this write an
            // intermediate frame. A change of rail row rides the same flag:
            // that replacement redraws the whole frame, rail and all.
            self.pointed_changed = true;
            return false;
        }
        changed
    }

    /// The width the transcript is folded at: the window's, less the rail's
    /// column where there is one.
    fn folds(&self) -> usize {
        if self.rails && self.terminal.is_terminal() && scroll_rail::spared(self.size.columns) {
            self.size.columns - 1
        } else {
            self.size.columns
        }
    }

    /// The window column the rail stands in, where it stands in one.
    fn rail_column(&self) -> Option<usize> {
        let folds = self.folds();
        (folds < self.size.columns).then_some(folds)
    }

    /// How many of the turn band's rows read as the transcript's tail: a
    /// running turn's own rows, as many of them as the band has room for.
    fn turn_tail(&self, bands: &Bands) -> usize {
        self.standing.running.len().min(bands.turn.len())
    }

    /// The window rows the rail stands beside: the transcript band, and the
    /// running turn's rows directly under it.
    fn railed_rows(&self, bands: &Bands) -> Range<usize> {
        bands.transcript.start..bands.transcript.end + self.turn_tail(bands)
    }

    /// The rail as the transcript band, and the running turn under it, stand
    /// now, where one is drawn.
    fn rail(&self, bands: &Bands) -> Option<ScrollRail> {
        self.rail_column()?;
        let place = self
            .record
            .place(bands.transcript.len())
            .under_turn(self.turn_tail(bands));
        Some(ScrollRail::new(
            place,
            self.record.prompts(),
            self.record.landed(),
        ))
    }

    /// The rail row the pointer is on, where it is on a rail with a thumb.
    ///
    /// `None` off the rail's column, outside the rows it stands beside, and on
    /// a rail over a record that fits, which is blank and answers no pointer.
    fn rail_pointed(&self, bands: &Bands) -> Option<usize> {
        let (row, column) = self.pointing?;
        if Some(column) != self.rail_column() || !self.railed_rows(bands).contains(&row) {
            return None;
        }
        self.rail(bands)?.thumb()?;
        Some(row - bands.transcript.start)
    }

    /// The transcript band as it stands, for placing a drag's ends.
    fn view(&self, bands: &Bands) -> View {
        View {
            band: bands.transcript.clone(),
            top: self.record.top_row(bands.transcript.len()),
        }
    }

    /// Extends the drag to wherever the held pointer is on the window now.
    fn reached(&mut self) {
        let Some((row, column)) = self.held else {
            return;
        };
        let bands = self.bands();
        let view = self.view(&bands);
        if let Some(taken) = &mut self.taken {
            taken.reaches(row, column, &view);
        }
    }

    /// Scrolls the transcript a row towards a held pointer resting at its
    /// edge, if its next step is due.
    ///
    /// A pointer on the band's first row asks for the row above it and a
    /// pointer on its last row, or anywhere below the band, for the row under
    /// it. The step is rescheduled while the band still has that way to go,
    /// so a drag left resting there keeps going, and forgotten the moment it
    /// does not — a pointer resting in the box after the band reached its foot
    /// is the drag a reader wanted, not a clock to keep.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    fn crept(&mut self) -> Result<bool, TerminalError> {
        let (Some((row, _)), Some(due)) = (self.held, self.creeps) else {
            self.creeps = None;
            return Ok(false);
        };
        let now = Instant::now();
        if now < due {
            return Ok(false);
        }
        let bands = self.bands();
        let by = if row <= bands.transcript.start {
            -1
        } else if row + 1 >= bands.transcript.end {
            1
        } else {
            self.creeps = None;
            return Ok(false);
        };
        if !self.record.scroll(by, bands.transcript.len()) {
            self.creeps = None;
            return Ok(false);
        }
        self.creeps = now.checked_add(CREEP);
        self.reached();
        self.draw()?;
        Ok(true)
    }

    /// The text of what is selected, in the order the reader reads it.
    ///
    /// Rows the window is showing are read off the screen, so the box and the
    /// turn band come along and a folded row comes as folded. Record rows a
    /// drag scrolled off the window are folded again from the record, because
    /// a reader who dragged past them took them. Each row is trimmed of the
    /// blank it was padded out with, because that padding is screen and not
    /// text.
    fn selected(&self) -> String {
        let Some(taken) = self.taken else {
            return String::new();
        };
        let bands = self.bands();
        let view = self.view(&bands);
        let mut painted = String::new();
        let lines: Vec<String> = taken
            .places(self.size.columns, &view, self.record.last_row())
            .into_iter()
            .filter_map(|(place, covered)| match (view.row(place), place) {
                (Some(row), _) => self.painted.said(row, &covered),
                (None, Place::Said(at)) => {
                    let row = self.record.row_at(at)?;
                    painted.clear();
                    row.paint_into(&self.palette, &mut painted);
                    Some(select::said(&painted, &covered, &row.structural()))
                }
                (None, Place::Stood(_)) => None,
            })
            .map(|line| line.trim_end().to_owned())
            .collect();
        lines.join("\n")
    }

    /// Where the pointer was last reported, as a window row and column.
    ///
    /// `None` until one is reported at all, which is every session on a
    /// terminal that answers nothing about the mouse. The place is the
    /// window's; what is on it is [`Renderer::aimed`]'s answer, and what that
    /// row means is the answer of whoever drew it.
    #[must_use]
    pub fn pointer(&self) -> Option<(usize, usize)> {
        self.pointing
    }

    /// Whether a pointer transition is waiting for a redraw.
    ///
    /// Taken once. Motion within the same effective target sets no new
    /// transition, so all-motion reporting does not turn into one frame per
    /// cell. A transition already pending remains true until this takes it.
    #[must_use]
    pub fn pointed_changed(&mut self) -> bool {
        std::mem::take(&mut self.pointed_changed)
    }

    /// The rows showing the result the transcript cut short that the pointer is
    /// resting on. Empty where it is resting on nothing of the kind, which
    /// includes a cell of the row the result did not draw ([`Self::cells`]).
    ///
    /// Every row of that one result and no row of any other, because what a
    /// pointer asks is what *this* opens: the light and the click have to name
    /// the same result, or the reader is told one thing and given another.
    ///
    /// Read per frame rather than remembered, which is what keeps it true while
    /// the picture moves under a pointer that has not: an answer arriving
    /// scrolls a cut result out from under it, and the next frame says so
    /// without anything having to notice.
    fn pointed(&self) -> Range<usize> {
        let bands = self.bands();
        let nothing = bands.transcript.start..bands.transcript.start;

        let Some((row, column)) = self.pointing else {
            return nothing;
        };

        if !bands.transcript.contains(&row) {
            return nothing;
        }

        let rows = bands.transcript.len();
        let Some(line) = self.record.at(row - bands.transcript.start, rows) else {
            return nothing;
        };

        if !self.record.wears(line, Slot::Cut) {
            return nothing;
        }

        // A row is a result only as far as it drew: the indent before it, the
        // blank after it and the rail beside it are the window's, and a pointer
        // resting there is over no result.
        if !self.cells(row).contains(&column) {
            return nothing;
        }

        // One result is however many lines of it are in a row, because a result
        // is written down in one go and nothing else is written down in the
        // middle of it. The lines around it are the call it answers and the
        // answer that follows, and neither is a result the transcript cut --
        // so the run ends where the result does.
        // Stopped at the head of the band on the way up, because a run that
        // started above it starts at the top row as far as this frame is
        // concerned. Downwards needs no such stop: a line past the foot is
        // covered by no row, which is the answer already.
        let top = self.record.at(0, rows).unwrap_or(line);
        let mut first = line;
        while first > top && self.record.wears(first - 1, Slot::Cut) {
            first -= 1;
        }
        let mut last = line;
        while self.record.wears(last + 1, Slot::Cut) {
            last += 1;
        }

        let head = self.record.covering(first, rows);
        let foot = self.record.covering(last, rows);
        (bands.transcript.start + head.start)..(bands.transcript.start + foot.end)
    }

    /// Whether the pointer is over the offer on the prompt row the caller
    /// marked pointable: on that row, and on a cell of it the offer took.
    fn prompt_pointed(&self) -> bool {
        let (Some((row, column)), Some((target, door))) = (self.pointing, &self.prompt_target)
        else {
            return false;
        };

        door.contains(&column) && self.aimed(row) == Some(Aimed::Boxed(*target))
    }

    /// Drops whatever is selected, because the picture under it is about to
    /// move.
    fn unselects(&mut self) {
        self.taken = None;
        self.painted.selects(None, View::default());
    }

    /// Tells this renderer which palette the run resolved.
    ///
    /// Said at startup and again whenever the theme changes, because it is what
    /// every row on screen is painted from: the record holds what was said, and
    /// the palette decides what it looks like at the moment it is drawn.
    ///
    /// It is also what decides whether the model's markdown is read at all — a
    /// run with no colour in it keeps every marker the model wrote, since
    /// dropping one there would take the emphasis away and put nothing in its
    /// place.
    pub fn wears(&mut self, palette: Palette) {
        self.palette = palette;
        self.painted.forget();
    }

    /// Tells this renderer which characters it may draw with.
    ///
    /// Said at startup, and again whenever the reader changes the setting. It
    /// reaches the transcript through the markdown reader, which is the one
    /// thing here that puts a character of its own in place of one the model
    /// wrote. The reader is told rather than replaced, since a change can
    /// arrive between two deltas of one answer and the scan has to go on where
    /// it was; rows already written keep the characters they were drawn with,
    /// except the opening, which the next resize lays out again in this set.
    pub fn draws(&mut self, glyphs: Glyphs) {
        self.glyphs = glyphs;
        self.markdown.draws(glyphs);
        self.painted.forget();
    }

    /// Tells this renderer which repository the answer's bare numbers count
    /// against.
    ///
    /// Said once, at startup, for the reason the glyph set is: which checkout
    /// the session opened in is settled before the first frame. `None` where
    /// there is no forge behind it, which leaves a number the plain text it
    /// has always been.
    pub fn counts(&mut self, forge: Option<Forge>) {
        self.forge = forge;
        self.markdown = self.reader();
    }

    /// Tells this renderer which checkout a link to a path in the answer is
    /// read against, and how the terminal reads its line.
    ///
    /// Said once, at startup, for the reason [`Renderer::counts`] is. `None`
    /// hands a path on as it was written.
    pub fn reads_paths(&mut self, files: Option<Files>) {
        self.files = files;
        self.markdown = self.reader();
    }

    /// A reader for the next message, drawing, counting and opening the way
    /// this renderer was told to.
    fn reader(&self) -> Markdown {
        Markdown::new(self.glyphs)
            .counting(self.forge.clone())
            .opening(self.files.clone())
    }

    /// Marks the next record line as the start of a prompt.
    ///
    /// Called after the blank parting it from the previous block and before the
    /// prompt rows themselves, so the rail's mark for it stands for the words it
    /// names.
    pub fn landmark(&mut self) {
        self.record.landmark();
    }

    /// How long an input wait may sleep before a drag resting at an edge of
    /// the transcript carries it another row, or a native window that changed
    /// size has settled. `None` where neither is pending.
    #[must_use]
    fn rests_in(&self) -> Option<Duration> {
        let now = Instant::now();
        let creeps = self.creeps.map(|due| due.saturating_duration_since(now));
        match (creeps, self.settles_in(now)) {
            (Some(creeps), Some(settles)) => Some(creeps.min(settles)),
            (creeps, settles) => creeps.or(settles),
        }
    }

    /// Does whatever fell due while an input wait slept: carries the
    /// transcript a row towards a drag resting at its edge, and gives a native
    /// window that has settled at a new size what was kept.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the frame could not be drawn.
    fn repose(&mut self) -> Result<bool, TerminalError> {
        let crept = self.crept()?;
        self.settled(Instant::now())?;
        Ok(crept)
    }

    /// Tells this renderer how far one notch of the wheel moves the transcript.
    ///
    /// Said at startup, and again whenever the reader changes the setting. The
    /// wheel arrives as a count of notches and nothing more — how far one is
    /// worth is the reader's, and this is where their answer lands.
    pub fn rolls(&mut self, rows: i32) {
        self.notch = rows;
    }

    /// How many rows one wheel notch moves in any scrollable view.
    #[must_use]
    pub fn scroll_rows(&self) -> usize {
        usize::try_from(self.notch).unwrap_or(1).max(1)
    }

    /// Tells this renderer whether the transcript has a scroll rail.
    ///
    /// Said at startup from the setting, and again whenever the reader changes
    /// it, when the record is folded again to the width it leaves. With it on,
    /// the rightmost column of the transcript band is the rail wherever the
    /// window can spare one, and the transcript folds a column narrower to
    /// leave it; with it off, nothing is drawn there and the transcript has
    /// the whole width. Nothing changes where output is redirected: a file has
    /// no right edge.
    pub fn rails(&mut self, on: bool) {
        // Native mode has no band to put one beside: the terminal's own
        // scrollbar is the rail.
        self.rails = on && self.native.is_none();
        self.grip = None;
        self.record.resized(self.folds(), self.glyphs);
        self.unselects();
        self.painted.forget();
    }

    /// Appends streamed output and puts a frame on screen.
    ///
    /// The markers in the model's markdown are read here rather than drawn: a
    /// heading, a run of code or a phrase under emphasis is recognised, its
    /// marker is dropped, and the run it covered is written wearing a slot. The
    /// tone belongs to the row rather than to the text, so it costs no column
    /// and the answer folds where the same answer would have folded plain.
    ///
    /// One frame per delta however many slots it turned out to hold. A slot
    /// changes between two pieces of one delta, and a frame per change would be
    /// several frames for one piece of the wire.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn stream(&mut self, delta: &str) -> Result<(), TerminalError> {
        // Before either branch, because both of them are an answer arriving and
        // the row between blocks is owed to neither.
        self.arriving = Arriving::Answer;

        // Nowhere to put a slot is nowhere to put a marker either. A redirected
        // run, `NO_COLOR`, `output.color` set to `never`: the answer arrives as
        // the model wrote it, which is markdown, and a file of markdown is
        // worth more than a file it has been taken out of.
        if !self.palette.writes_color() {
            self.take(Slot::Plain, delta)?;
            return self.draw();
        }

        let columns = self.folds();
        let redirected = !self.terminal.is_terminal();
        let Self {
            markdown,
            record,
            escapes,
            free,
            terminal,
            ..
        } = self;

        let mut taking = Taking {
            record,
            escapes,
            free,
            out: redirected.then_some(terminal),
        };

        // The first failure is kept and the rest of the delta is still read:
        // the markdown state has to walk every byte of it either way, or the
        // next delta is read against a state that skipped part of one.
        let mut wrote = Ok(());
        markdown.read(delta, columns, &mut |slot, text, link| {
            let said = taking.take(slot, text, link);
            if wrote.is_ok() {
                wrote = said;
            }
        });
        wrote?;

        self.draw()
    }

    /// Writes a line that is finished and will never be re-read.
    ///
    /// Used for a completed message, a tool result, or anything else that came
    /// from somewhere other than this program. It is folded and stripped of
    /// escape bytes by the same rules streamed output is, because it arrived
    /// the same way.
    ///
    /// An empty line straight under a divider ([`Renderer::divides`]) is not
    /// taken: the divider is already the row that parts what follows, and a
    /// blank one under it would part the new session from its own start.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn commit(&mut self, line: &str) -> Result<(), TerminalError> {
        if line.is_empty() && self.record.divided() {
            return Ok(());
        }
        self.take(Slot::Plain, line)?;
        // The newline is what ends the line; without it the next thing written
        // would continue this one.
        self.take(Slot::Plain, "\n")?;
        self.draw()
    }

    /// Writes rows crucible composed itself, above everything after them.
    ///
    /// The counterpart to [`Renderer::commit`], and separate from it for one
    /// reason: a committed line is text that came from somewhere else, so it
    /// goes through the same fold and the same escape-dropping as streamed
    /// output. A [`Row`] is not text that arrived; it is spans this program
    /// built, whose colour the palette decides at the moment it is drawn — so
    /// a theme chosen later repaints it along with everything else.
    ///
    /// Not folded either, and it does not need to be: a component is given the
    /// width and returns rows that fit it. That width is
    /// [`Self::transcript_columns`], not the window's: with the rail on, a row
    /// laid at the window's width loses its last column to it. A window that
    /// narrows clips them
    /// rather than folding them, because rows a component laid out against each
    /// other are not prose and re-flowing one of them would break the column
    /// the others are aligned in.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn present(&mut self, rows: &[Row]) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            // The line still open has already gone out without its ending, so
            // the ending is owed before anything is written under it.
            if self.record.writing() {
                self.terminal.write("\n")?;
            }

            // Plain, and structurally so: there is no palette to paint from
            // where nothing resolved one, and an escape written into a file is
            // bytes in the middle of it.
            for row in rows {
                self.terminal.write(&row.text())?;
                self.terminal.write("\n")?;
            }
        }

        // A row of a component is a line of its own, so whatever was still open
        // ends here rather than having the first of them appended to it.
        self.record.end();
        self.record.lay(rows.iter().cloned());
        self.draw()
    }

    /// Writes a responsive block into the transcript, keeping what draws it.
    ///
    /// For committed components whose width-independent source remains in hand:
    /// prompts, file changes, and a command's answer drawn from figures it
    /// still holds. The closure is retained by the bounded record and called
    /// only when the terminal width changes; ordinary frames read the rows
    /// built for the current width. `retained` is the source bytes it closes
    /// over, charged against that record's ceiling. Each width it is handed is
    /// [`Self::transcript_columns`], as for [`Self::present`].
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    #[allow(clippy::needless_pass_by_value)]
    pub fn responsive(
        &mut self,
        retained: usize,
        lay: Box<dyn Fn(usize) -> Vec<Row>>,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return self.present(&lay(self.transcript_columns()));
        }

        self.record.responsive(retained, lay);
        self.draw()
    }

    /// Writes the opening into the transcript, keeping what draws it.
    ///
    /// Everything else handed to [`Self::present`] arrives as rows and stays as
    /// rows: what laid them out is a component that answered once and went, so
    /// a narrower window clips them. The opening also retains its source and is
    /// drawn again from facts read once at launch and held for the whole
    /// session, so what laid it is still here to lay it again — and it is what
    /// a reader is looking at when they take the corner of a fresh window and
    /// pull. Each width it is handed is [`Self::transcript_columns`], as for
    /// [`Self::present`], and each glyph set the one [`Self::draws`] last
    /// named, so a card laid out again after the reader changed it is drawn
    /// in the set they chose.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn opens(
        &mut self,
        lay: Box<dyn Fn(usize, Glyphs) -> Vec<Row>>,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            // Nothing will resize a file, so holding what could draw it again
            // would be holding it for an event that cannot arrive.
            return self.present(&lay(self.transcript_columns(), self.glyphs));
        }

        self.record.opens(self.glyphs, lay);
        self.draw()
    }

    /// Draws the box and leaves it standing, with the cursor where `caret`
    /// says it goes.
    ///
    /// The counterpart to [`Renderer::present`] for a component that is still
    /// being changed: the same rows and the same palette, but redrawn where
    /// they stand instead of joining the transcript. What a keystroke costs is
    /// therefore the rows that changed, and the caller redraws only when
    /// something moved.
    ///
    /// The box alone. Anything a line has opened over it — a list or a plan —
    /// is [`Renderer::under`]'s, because the band this fills is held to a share
    /// of the window and that share is a rule about a long prompt rather than
    /// about what is standing above one. A caller that must replace the prompt
    /// and what stands over it together uses [`Renderer::replace`].
    ///
    /// An empty slice takes the box off, and the caret goes unread — which is
    /// what a component standing where the box was does on its way in, so that
    /// the share the box was held to is not still being held against the thing
    /// that replaced it.
    ///
    /// The cursor is left on the row the caret named rather than at the end of
    /// what was written, so the terminal's own cursor is the one the reader
    /// sees, in whatever shape and blink they chose. Nothing is drawn to stand
    /// in for it.
    ///
    /// Nothing at all happens where output is redirected. A band is a thing
    /// only a screen has, and a run whose output is a file has no keystrokes
    /// arriving to redraw for either — the same condition [`Raw`] refuses to
    /// enter under.
    ///
    /// [`Raw`]: crate::Raw
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn live(
        &mut self,
        rows: &[Row],
        caret: Caret,
        palette: Palette,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        self.standing.boxes(rows, palette, self.size.columns);
        self.standing.prompted = Some(caret);
        self.prompt_target = None;
        self.pointed_changed = false;
        self.draw()
    }

    /// Replaces everything standing at the foot in one frame.
    ///
    /// `prompt.pointed` names one prompt row and the same row in its pointed
    /// palette state. The renderer chooses it from the pointer's absolute
    /// window row; the caller remains the owner of the component and its words.
    ///
    /// # Errors
    ///
    /// `TerminalError::Io` if the terminal could not be written to.
    pub fn replace(
        &mut self,
        prompt: PromptRows<'_>,
        over: &[Row],
        palette: Palette,
    ) -> Result<(), TerminalError> {
        self.replace_running(prompt, &[], over, palette)
    }

    /// [`Renderer::replace`] while a turn runs: `turn` is what the turn itself
    /// is showing, and `over` anything else standing under it and over the box.
    ///
    /// The turn's rows read as the transcript's last rows while it runs, and
    /// the scroll rail treats them so: it stands beside them as it does beside
    /// the transcript band, counts them as the record's tail and as rows on
    /// screen, and so keeps one length while they grow and shrink. They are
    /// laid out at [`Renderer::transcript_columns`] to leave it its column; a
    /// row wider than that is clipped to it. What stands in `over` — a list a
    /// line opened — is not the transcript's and stands beside no rail.
    ///
    /// # Errors
    ///
    /// `TerminalError::Io` if the terminal could not be written to.
    pub fn replace_running(
        &mut self,
        prompt: PromptRows<'_>,
        turn: &[Row],
        over: &[Row],
        palette: Palette,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        let columns = self.size.columns;
        self.standing.boxes(prompt.rows, palette, columns);
        // The turn's own rows at the width the rail leaves them, as the frame
        // draws them; what stands under them at the window's.
        self.standing.stands(turn, over, palette);
        self.standing.paint_turn(self.folds(), columns);
        self.standing.prompted = Some(prompt.caret);
        self.standing.turned = None;
        self.prompt_target = prompt.pointed.map(|(at, row)| (at, door(row, columns)));

        if self.prompt_pointed()
            && let Some((at, row)) = prompt.pointed
            && let Some(shown) = self.standing.prompt.get_mut(at)
        {
            shown.clear();
            row.clipped(self.size.columns).paint_into(&palette, shown);
        }

        self.pointed_changed = false;
        self.draw()
    }

    /// Keeps `rows` directly over the box until something takes them back.
    ///
    /// The counterpart to [`Renderer::live`] for everything at the foot of the
    /// window that is not the box: what a turn is showing while it runs, and
    /// between turns whatever a line has opened. Streamed output goes on
    /// arriving in the transcript above them and every frame draws them again
    /// underneath it, so they stay on screen through a turn instead of being
    /// the first thing it scrolls away. An empty slice takes them back.
    ///
    /// This band gives up its rows before the prompt and after the
    /// transcript, so a list opened over a session takes the room it asks for
    /// and the transcript is what gives way — which is the right way round, a
    /// list being the thing the reader is looking at while it is open.
    ///
    /// They never join the transcript. What stands here is a fact about the
    /// session rather than something that was said, so the record reads
    /// afterwards as though it had never been there.
    ///
    /// Nothing happens where output is redirected, for the reason
    /// [`Renderer::live`] draws nothing there.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn under(
        &mut self,
        rows: &[Row],
        caret: Option<Caret>,
        palette: Palette,
    ) -> Result<(), TerminalError> {
        self.standing_under(&[], rows, caret, palette)
    }

    /// Stands `rows` under the transcript, with `turn` over them: rows that
    /// are still the transcript's, laid out beside the scroll rail at the width
    /// it leaves and counted as the record's tail, as
    /// [`Renderer::replace_running`] stands its own `turn`. The caret's row
    /// counts from the first of `turn`.
    fn standing_under(
        &mut self,
        turn: &[Row],
        rows: &[Row],
        caret: Option<Caret>,
        palette: Palette,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        self.standing.stands(turn, rows, palette);
        self.standing.paint_turn(self.folds(), self.size.columns);
        self.standing.turned = caret;
        self.draw()
    }

    /// [`Renderer::under`] in the box's place, with `turn` over it as
    /// [`Renderer::replace_running`] stands it: takes the box off and stands
    /// `rows` where it was, in one frame.
    ///
    /// For a component that takes the rows the box has while a turn runs, and
    /// so is drawn again on every beat of it. Taking the box off with
    /// [`Renderer::live`] first would draw every one of those twice, the first
    /// time with neither the box nor the component's new rows on screen.
    ///
    /// `turn` is the rows over `rows` that are still the transcript's, so the
    /// scroll rail stands beside them and counts them, as it does
    /// [`Renderer::replace_running`]'s `turn`: they are laid out at the width
    /// the rail leaves, and the caret's row counts from the first of them.
    /// Anything else a turn keeps over `rows` goes in `rows`. Empty between
    /// turns.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn instead(
        &mut self,
        turn: &[Row],
        rows: &[Row],
        caret: Option<Caret>,
        palette: Palette,
    ) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        // As an empty slice given to `live` takes it off, without its frame.
        self.standing.boxes(&[], palette, self.size.columns);
        self.standing.prompted = Some(Caret::default());
        self.prompt_target = None;
        self.pointed_changed = false;
        self.standing_under(turn, rows, caret, palette)
    }

    /// Ends the line the transcript is still writing to.
    ///
    /// Called between turns. After this the next delta starts a line of its
    /// own, and anything a reader held back for a shape that never arrived has
    /// been put down.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn settle(&mut self) -> Result<(), TerminalError> {
        // Text held back for a shape that never arrived is text, and this is
        // the last moment it can be written: the reader below is about to be
        // dropped, and with it anything it was still holding.
        let columns = self.folds();
        let redirected = !self.terminal.is_terminal();
        let Self {
            markdown,
            record,
            escapes,
            free,
            terminal,
            ..
        } = self;

        let mut taking = Taking {
            record,
            escapes,
            free,
            out: redirected.then_some(terminal),
        };

        let mut wrote = Ok(());
        markdown.finish(columns, &mut |slot, text, link| {
            let said = taking.take(slot, text, link);
            if wrote.is_ok() {
                wrote = said;
            }
        });
        wrote?;

        // The markers belong to the message that is ending. A fence the model
        // opened and never closed would otherwise read the tool result under it
        // as code, and the whole of the next answer after that.
        self.markdown = self.reader();

        // And the answer they belonged to is over, whatever ended it: the next
        // block down is a block of its own and is owed the row that says so.
        self.arriving = Arriving::Nothing;

        if redirected && self.record.writing() {
            self.terminal.write("\n")?;
        }

        self.record.end();
        self.draw()
    }

    /// Writes `rows` to whatever screen is there now, one to a line.
    ///
    /// Nothing is addressed, nothing is diffed and nothing is recorded: this is
    /// for after the screen a session ran on has been handed back, when the
    /// thing being written to is the reader's own scrollback and the rows this
    /// renderer is holding describe a screen that no longer exists.
    ///
    /// Which is also why it may only be called once, at the end. A renderer
    /// whose picture is still on a screen would be writing under its own frame.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn parting(&mut self, rows: &[Row]) -> Result<(), TerminalError> {
        self.leave()?;
        for row in rows {
            self.terminal.write(&row.paint(&self.palette))?;

            // Both halves, because raw mode may or may not have been left by
            // the time this runs and a bare newline under it moves down a row
            // without going back to the first column.
            self.terminal.write("\r\n")?;
        }

        self.terminal.flush()
    }

    /// Asks the terminal to put `text` on the reader's clipboard.
    ///
    /// Answers whether it asked. `false` where there is nothing to copy, where
    /// there is more of it than a terminal will take, and where output is
    /// redirected — a clipboard is a thing only a terminal has, and the request
    /// written into a file would be bytes in the middle of it.
    ///
    /// A terminal that does not implement the sequence drops it, and there is
    /// no reply to wait for either way. So a `true` here says the request was
    /// written, not that it landed; what says it landed is the reader's next
    /// paste, and nothing this process can ask changes that.
    ///
    /// Outside the frame, deliberately. This is not a row and never becomes
    /// one: it goes down between frames and changes nothing about what the
    /// window is showing.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn copied(&mut self, text: &str) -> Result<bool, TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(false);
        }

        let Some(asking) = clipboard::copying(text) else {
            return Ok(false);
        };

        self.terminal.write(&asking)?;
        self.terminal.flush()?;
        Ok(true)
    }

    /// Re-folds for a terminal the user resized.
    ///
    /// The record is folded again at the new width, which is what keeps the
    /// reader on the line they were reading rather than on a row number that
    /// meant something else, and the opening is drawn again rather than folded.
    /// What was standing stays, painted again at the new width from the rows
    /// it was handed in as: it was laid out against a window that has gone, so
    /// a row wider than the new one is cut at its edge until the caller lays
    /// out the next one.
    ///
    /// A size the terminal could not report is no news of a size, and the one
    /// already known is kept.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn resized(&mut self) -> Result<(), TerminalError> {
        let Ok(size) = self.terminal.size() else {
            return Ok(());
        };
        if size == self.size {
            return Ok(());
        }

        self.native_resize(size);
        self.size = size;
        self.record.resized(self.folds(), self.glyphs);
        self.standing.repaint(self.folds(), size.columns);
        self.prompt_target = None;
        self.pointed_changed = false;
        self.unselects();
        self.grip = None;

        // Every row of the window is now showing something drawn for a size it
        // no longer has, so the next frame may not skip any of them.
        self.painted.forget();

        self.draw()
    }

    /// Empties the transcript, leaving the band ready for what replaces it.
    ///
    /// What a session picked up asks for. A resumed session is not the next
    /// thing that happened in the one on screen — it is a different
    /// conversation, and putting it under what was there would leave a reader
    /// scrolling back through two of them, joined at a point nothing marks.
    ///
    /// In native mode what was there stays in the terminal's scrollback until
    /// a resize clears it, and is not given back then, so the next block is
    /// parted from the last row that went out there rather than from nothing,
    /// and [`Renderer::divides`] is what marks the point.
    ///
    /// The record's numbering carries on past the lines it drops, so a number
    /// some other part of the program is holding names the line it named and
    /// names nothing once that line has gone.
    ///
    /// A reply hung by [`Renderer::hangs`] is let go of unmarked, as the full
    /// screen marks nothing of a reply that emptied the transcript.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn empties(&mut self) -> Result<(), TerminalError> {
        self.record.unhangs();
        self.native_empties()?;
        match self.native {
            Some(_) => self.record.empties_under(),
            None => self.record.empties(),
        }
        self.standing.clear();
        self.prompt_target = None;
        self.pointed_changed = false;
        self.unselects();

        // Every row of the band is showing a line that is no longer in the
        // record, so the next frame may not skip any of them.
        self.painted.forget();

        self.draw()
    }

    /// Writes something the reader is expected to type after.
    ///
    /// The one thing here with two answers. On a screen this process owns, it
    /// is a line of the transcript like any other and the cursor is the one the
    /// box parks — nothing is echoed onto it, because there is no box and no
    /// raw mode on the path that asks. Redirected, it is written through
    /// immediately and unterminated: whatever is reading the output has to see
    /// the question before it can answer, and the record only lets go of a line
    /// once that line can no longer change.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn prompt(&mut self, slot: Slot, text: &str) -> Result<(), TerminalError> {
        self.take(slot, text)?;
        self.draw()
    }

    /// Leaves one blank row between what has been written and what comes next.
    ///
    /// The transcript is a column of blocks — what was asked, what was
    /// answered, each call and the line under it — and what separates one from
    /// the next is a row of nothing. Every block asks for that row on its way
    /// in rather than leaving one behind, because a block cannot know it was
    /// the last: a session that parted on the way out would end on a blank row
    /// under the final answer.
    ///
    /// Blank rows do not accumulate, and none is owed once an answer has begun
    /// arriving: what comes next is then the rest of that answer rather than a
    /// block after it, and a caller that asks on every piece of a streamed
    /// answer — which is the only way it can ask on the first — must get a row
    /// before the answer and none inside it. That is what the record's own
    /// open line answers.
    ///
    /// Except while an answer is arriving, because then the record is not the
    /// whole of the question. A piece cut mid-row leaves the reader holding
    /// characters the record has not been told about; a piece cut exactly at a
    /// line break leaves nothing held at all, and the record ending in a
    /// finished line looks the same whether the block above it closed or is
    /// still being written. Neither can say which, so the answer being under
    /// way is what is asked instead — and that is what keeps the same answer
    /// from drawing differently for having been cut into different pieces on
    /// the way here.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn apart(&mut self) -> Result<(), TerminalError> {
        if self.record.parted() || self.record.writing() || self.arriving == Arriving::Answer {
            return Ok(());
        }

        self.commit("")
    }

    /// Moves the transcript's viewport one notch of the wheel, and says whether
    /// it moved.
    ///
    /// `back` is towards the top of the session. Its own call rather than
    /// arithmetic at each of the loops that read the wheel, because how far a
    /// notch goes is one answer for the session and every one of them would
    /// otherwise have to be handed it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn notched(&mut self, back: bool) -> Result<bool, TerminalError> {
        self.scrolled(if back { -self.notch } else { self.notch })
    }

    /// Moves the transcript's viewport by `by` display rows, and says whether
    /// it moved.
    ///
    /// Negative is towards the top of the session. Nothing moves where output
    /// is redirected: a file has no viewport, and a reader of one scrolls it
    /// with whatever they opened it in.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn scrolled(&mut self, by: i32) -> Result<bool, TerminalError> {
        if !self.terminal.is_terminal() || self.native.is_some() {
            return Ok(false);
        }

        let rows = self.bands().transcript.len();
        if !self.record.scroll(by, rows) {
            return Ok(false);
        }

        self.draw()?;
        Ok(true)
    }

    /// Puts the transcript's viewport back at the foot of the record.
    ///
    /// What a reader who scrolled up is taken to be done doing the moment they
    /// send something: they were reading back, and what they sent is about to
    /// be answered at the bottom. Everything else leaves the viewport where
    /// they put it — text arriving while somebody reads back through the
    /// transcript is exactly what must not move it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn follows(&mut self) -> Result<(), TerminalError> {
        self.record.follow();
        self.draw()
    }

    /// Where this renderer draws.
    #[must_use]
    pub fn screen(&self) -> ScreenMode {
        match self.native {
            Some(_) => ScreenMode::Native,
            None => ScreenMode::Fullscreen,
        }
    }

    /// Whether output is going to a terminal rather than a pipe or a file.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.terminal.is_terminal()
    }

    /// The terminal underneath, to read rather than to write on.
    ///
    /// Shared and not exclusive: a write made through here would land in the
    /// middle of a frame, and what the window is showing would be wrong with
    /// nothing in this crate able to notice.
    pub fn terminal(&self) -> &T {
        &self.terminal
    }

    /// How wide the window was when this last looked.
    ///
    /// Held rather than asked for, because a caller that decides how much of a
    /// line to show does so once per event and asking the operating system each
    /// time would put a syscall on the render path. [`Renderer::resized`] is
    /// what keeps it true.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.size.columns
    }

    /// How wide a row of the transcript may be.
    ///
    /// The window's width, less the scroll rail's column where it stands in
    /// one. What a caller lays rows out at before handing them to
    /// [`Renderer::present`]; what stands at the foot — the box and the turn —
    /// is laid out at [`Renderer::columns`], because the rail stops where the
    /// transcript does.
    #[must_use]
    pub fn transcript_columns(&self) -> usize {
        self.folds()
    }

    /// How tall it was when this last looked.
    ///
    /// Read by a component that can grow, which asks how much room there is
    /// before it does. How much of that room it may actually have is
    /// `crate::bands`'s answer and not the component's.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.size.rows
    }

    /// How many rows what stands over or in place of the box gets.
    ///
    /// The whole window on the full screen, since nothing else holds a row of
    /// it for the length of a session. Half the window, rounded down, in native
    /// mode: a row the transcript gives up there goes into the terminal's
    /// scrollback, and closing what took it does not bring it back, so a panel
    /// is held to a share and scrolls inside it instead. The one place the
    /// share is worked out, so every component that asks is held to the same
    /// one — a panel the share cannot hold at all is the caller's to stand
    /// taller, at the least it can be drawn in.
    #[must_use]
    pub fn room(&self) -> usize {
        if self.native.is_some() {
            self.size.rows / PANEL_SHARE
        } else {
            self.size.rows
        }
    }

    /// How many lines the transcript has taken this session.
    ///
    /// Read straight after writing something, to learn where it went: a caller
    /// that means to point at a line later keeps this number, and
    /// [`Renderer::aimed`] hands back the same numbering.
    ///
    /// It counts lines written rather than lines kept, so it goes on rising for
    /// the length of a session and a number kept from an hour ago still names
    /// the line it named then — whether or not that line is still held.
    #[must_use]
    pub fn lines(&self) -> usize {
        self.record.lines()
    }

    /// Edits the rows record line `at` was written as, now and at every width
    /// the record is laid out at again.
    ///
    /// The one way a row already written says less than it did: an offer to
    /// expand whose result has gone is taken off the row that made it, and the
    /// row stays where it was. `at` numbers lines the way [`Renderer::lines`]
    /// does; a line no longer held, and one not written yet, is left alone.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be redrawn.
    pub fn amend(&mut self, at: usize, edit: fn(&mut [Row])) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            // Redirected output was written through as it arrived; there is
            // nothing on a screen to take anything back from.
            return Ok(());
        }

        self.record.amend(at, edit);
        self.draw()
    }

    /// Hangs what is written from here on under `glyphs.hangs()`, for
    /// [`Renderer::subordinate`] to finish once it has all been written.
    ///
    /// Told before a reply starts rather than after it ends, because in native
    /// mode a row is written once, and a reply that leaves the live region
    /// before its command ends has to leave it carrying its mark. Each line
    /// goes out marked, and [`Renderer::subordinate`], given the line this
    /// started at, marks whatever is still held. The full screen marks nothing
    /// before that. Emptying the transcript forgets it, and where output is
    /// redirected nothing is marked at all.
    pub fn hangs(&mut self, glyphs: Glyphs) {
        if self.terminal.is_terminal() {
            self.record.hangs(glyphs.hangs());
        }
    }

    /// Hangs every transcript row written since `from` under one result mark.
    ///
    /// The first row receives `glyphs.hangs()` and the rest align beneath its
    /// text. The mark is structural, so custom selection neither lights nor
    /// copies it. A command that emptied the record is handled by the record's
    /// stable line numbering: only output written after that reset is changed.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be redrawn.
    pub fn subordinate(&mut self, from: usize, glyphs: Glyphs) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            // Redirected output was written through as each command produced it,
            // so a post-hoc record rewrite cannot insert bytes before it. Keep
            // that path byte-for-byte compatible rather than printing a late
            // prefix after the answer.
            return Ok(());
        }

        self.record.subordinate(from, glyphs.hangs());
        self.draw()
    }

    /// The last `rows` display rows of what has been drawn, as rows.
    ///
    /// What the transcript band would be showing, folded at the width this
    /// renderer was opened at, for a caller that puts them somewhere this
    /// renderer does not own. Fewer than asked for where less has been drawn.
    ///
    /// It is how a session is shown without being picked up: replayed onto a
    /// renderer of its own and taken back out here, so a preview of a session
    /// is drawn by the code that draws the session, down to the mark in front
    /// of a prompt and the row a cut result was cut on.
    #[must_use]
    pub fn tail(&self, rows: usize) -> Vec<Row> {
        self.record.view(rows)
    }

    /// What is under window row `at`.
    ///
    /// What a click means, row by row. On a screen this process owns, the
    /// answer needs nothing from the terminal: the bands say which region the
    /// row is in and the record says which line is on it, so there is no round
    /// trip to ask where the cursor happens to be.
    ///
    /// `None` for a row in a band nothing is drawn in, and for one below the
    /// last line the transcript is showing.
    ///
    /// A line of a result that was written down over several is answered as
    /// the first of them: the offer to open the result was made on that one,
    /// and a reader pointing at the second row of a sentence is pointing at the
    /// sentence.
    ///
    /// The row only: which of its cells the row drew is [`Self::cells`], and a
    /// press on any other cell of it lands on nothing.
    #[must_use]
    pub fn aimed(&self, at: usize) -> Option<Aimed> {
        // A window row names nothing here: the region moves with the
        // reader's own scrollback, and no press of the pointer arrives.
        if self.native.is_some() {
            return None;
        }

        let bands = self.bands();

        if bands.transcript.contains(&at) {
            let into = at - bands.transcript.start;
            return self
                .record
                .at(into, bands.transcript.len())
                .map(|line| Aimed::Line(self.record.heads(line)));
        }

        if bands.turn.contains(&at) {
            return Some(Aimed::Stood(at - bands.turn.start));
        }

        if bands.prompt.contains(&at) {
            return Some(Aimed::Boxed(at - bands.prompt.start));
        }

        None
    }

    /// The cells of window row `at` that a click or a resting pointer counts
    /// on: from the first cell the row drew to its last, in terminal cells.
    ///
    /// [`Self::aimed`] says what a row is; this says how much of it is that.
    /// The indent before a row's first mark and the blank after its last
    /// character are the window's, not the row's, so a press there lands on
    /// nothing. Read from the row the band shows, folded and clipped as the
    /// frame draws it, so what answers is what is on screen.
    ///
    /// A row standing over the box answers the same way, at the width it was
    /// drawn at, so a list's row is the list's only as far as its text goes.
    ///
    /// The prompt row the caller marked pointable answers with the cells of
    /// its offer alone: what it says beside the offer is a fact, not a door.
    ///
    /// Empty in native mode, where no press arrives, and for any row but a
    /// transcript row, a row standing over the box, or that one.
    #[must_use]
    pub fn cells(&self, at: usize) -> Range<usize> {
        if self.native.is_some() {
            return 0..0;
        }

        if let Some((target, door)) = &self.prompt_target
            && self.aimed(at) == Some(Aimed::Boxed(*target))
        {
            return door.clone();
        }

        let bands = self.bands();
        if bands.turn.contains(&at) {
            return self
                .standing
                .drew
                .get(at - bands.turn.start)
                .cloned()
                .unwrap_or(0..0);
        }
        if !bands.transcript.contains(&at) {
            return 0..0;
        }

        let top = self.record.top_row(bands.transcript.len());
        self.record
            .row_at(top + (at - bands.transcript.start))
            .map_or(0..0, |row| drawn(&row, self.folds()))
    }

    /// How the window is shared out, given what is standing in it.
    fn bands(&self) -> Bands {
        Bands::share(
            self.size.rows,
            Wants {
                turn: self.standing.turn.len(),
                prompt: self.standing.prompt.len(),
            },
        )
    }

    /// Add `text` to the record, and to a redirected run's output.
    fn take(&mut self, slot: Slot, text: &str) -> Result<(), TerminalError> {
        let redirected = !self.terminal.is_terminal();
        let Self {
            record,
            escapes,
            free,
            terminal,
            ..
        } = self;

        Taking {
            record,
            escapes,
            free,
            out: redirected.then_some(terminal),
        }
        .take(slot, text, None)
    }

    /// One frame.
    fn draw(&mut self) -> Result<(), TerminalError> {
        // A redirected run has already been given every byte, at the moment
        // each arrived. What is left is to make sure it has them.
        if !self.terminal.is_terminal() {
            return self.terminal.flush();
        }

        if self.native.is_some() {
            return self.draw_native();
        }

        // A resize the operating system reported but no wait on the keyboard
        // has read, as while an answer streams: `resized` lays everything out
        // again for the window as it is now and draws it, through here again
        // with the flag lowered, so the frame it draws is this one. A query
        // that fails says nothing, and the frame is drawn for the size known.
        if self.resizes.as_ref().is_some_and(ResizeFlag::taken)
            && self.terminal.size().is_ok_and(|size| size != self.size)
        {
            return self.resized();
        }

        let bands = self.bands();
        // Worked out once for the whole frame, and it names rows rather than
        // setting a mode: the result under the pointer is painted from the lit
        // palette and everything else on screen from the plain one.
        let lit = self.pointed();
        let palette = self.palette;
        let pointed = self.palette.pointing(true);
        let view = self.view(&bands);
        self.painted.selects(self.taken, view);
        self.painted.open(self.size.rows, self.size.columns);

        // The rail's cells, one a band row, set after the row beside them has
        // been made the transcript's width — clipped where a component's row
        // is wider, padded where it is shorter — so every cell lands in the
        // same column. Empty where there is no rail.
        let folds = self.folds();
        let railed = self.rail_pointed(&bands);
        let rail = self
            .rail(&bands)
            .map(|rail| rail.rows(self.size.columns, self.glyphs, railed))
            .unwrap_or_default();
        let mut rail = rail.into_iter();
        let mut showing = self.record.view(bands.transcript.len()).into_iter();
        for at in bands.transcript.start..bands.transcript.end {
            let row = match (showing.next(), rail.next()) {
                (row, Some(cell)) => {
                    let mut row = row.unwrap_or_default();
                    if row.columns() > folds {
                        row = row.clipped(folds);
                    }
                    row.pad(folds);
                    Some(row.join(cell))
                }
                (row, None) => row,
            };
            match row {
                Some(row) if lit.contains(&at) => self.painted.paint(at, &row, &pointed),
                Some(row) => self.painted.paint(at, &row, &palette),
                // Below what there is to show. A session that has just started
                // reads from the top of the window down, as a terminal's own
                // scrollback would.
                None => self.painted.blank(at),
            }
        }

        // Taken out and put back, because a painted row is borrowed from the
        // same `self` the frame is written into. A running turn's own rows
        // lead the band and read as the transcript's tail, so the rail goes on
        // beside them: each is painted afresh with the rail's cell for its
        // row, made the transcript's width first as a transcript row is.
        let turn = std::mem::take(&mut self.standing.turn);
        let running = std::mem::take(&mut self.standing.running);
        let ran = self.standing.ran.unwrap_or(palette);
        for (index, (at, painted)) in (bands.turn.start..bands.turn.end).zip(&turn).enumerate() {
            match (running.get(index), rail.next()) {
                (Some(row), Some(cell)) => {
                    let mut row = row.clipped(folds);
                    row.pad(folds);
                    self.painted.paint(at, &row.join(cell), &ran);
                }
                _ => self.painted.put(at, painted),
            }
        }
        self.standing.turn = turn;
        self.standing.running = running;

        let prompt = std::mem::take(&mut self.standing.prompt);
        for (at, row) in (bands.prompt.start..bands.prompt.end).zip(&prompt) {
            self.painted.put(at, row);
        }
        self.standing.prompt = prompt;

        let (row, column) = self.parked(&bands);
        self.painted.park(row, column);

        // A frame that changed nothing costs nothing. The bracket that holds
        // the screen and the sequences that hide the cursor are bytes too, and
        // a turn is a great many frames in which only the clock moved.
        if !self.painted.moved() {
            return Ok(());
        }

        self.terminal.write(self.painted.sealed())?;
        self.terminal.flush()
    }

    /// Where the cursor goes, in window rows and columns.
    ///
    /// The box first, because between turns it is the only thing anybody is
    /// typing into. Then whatever a turn is holding, which is where a question
    /// asked mid-turn puts it. With neither, the top of the prompt band — the
    /// row the box would be on, which is where the next thing to be typed will
    /// appear.
    fn parked(&self, bands: &Bands) -> (usize, usize) {
        let foot = self.size.rows.saturating_sub(1);

        let at = self
            .standing
            .prompted
            .filter(|_| !self.standing.prompt.is_empty())
            .map(|caret| (bands.prompt.start + caret.row, caret.column))
            .or_else(|| {
                self.standing
                    .turned
                    .filter(|_| !self.standing.turn.is_empty())
                    .map(|caret| (bands.turn.start + caret.row, caret.column))
            })
            .unwrap_or((bands.prompt.start, 0));

        (
            at.0.min(foot),
            at.1.min(self.size.columns.saturating_sub(1)),
        )
    }
}

/// Where arriving text goes.
///
/// Three pieces of one renderer, borrowed together because the markdown reader
/// holds the fourth while it walks a delta: the record the text lands in, the
/// escape state it is read against, and — where output is redirected — the
/// reader it goes straight out to.
struct Taking<'a, T: Terminal> {
    record: &'a mut Record,
    escapes: &'a mut Escapes,
    /// Reused: one run of text with its escape bytes taken out.
    free: &'a mut String,
    /// `None` on a screen, where a frame decides when a row is written.
    out: Option<&'a mut T>,
}

impl<T: Terminal> Taking<'_, T> {
    /// Add one run of text wearing `slot`.
    ///
    /// The escape bytes go here and nowhere else, and the format characters
    /// with them. Colour in a tool result is bytes an untrusted string put
    /// there: a row of the record is spans this program painted from a
    /// palette, and a byte of a redirected run's output is one this program
    /// meant to write.
    fn take(&mut self, slot: Slot, text: &str, link: Option<&str>) -> Result<(), TerminalError> {
        let escapes = &mut *self.escapes;
        self.free.clear();
        // A format character goes with the escapes: it would reorder or hide
        // the text around it in a terminal and in a file read in one later.
        self.free.extend(
            text.chars()
                .filter(|character| !escapes.holds(*character) && !width::unshown(*character)),
        );

        self.record.write(slot, self.free, link);

        // The address never reaches a redirected run. What that run is written
        // is what the row says, and an address a terminal would have swallowed
        // is bytes in the middle of a file somebody is going to read.
        match &mut self.out {
            Some(out) => out.write(self.free),
            None => Ok(()),
        }
    }
}

/// The cells `row` draws when it is given `room` of them: from its first
/// character that is not a blank to the end of its last, a wide character
/// counting as two.
fn drawn(row: &Row, room: usize) -> Range<usize> {
    let text = row.text();
    let said = width::clip(&text, room).trim_end_matches(' ');
    let indent = said.len() - said.trim_start_matches(' ').len();
    indent..width::columns(said)
}

/// The cells of `row` its [`Slot::Pointed`] runs take when it is given `room`
/// of them: the offer on a pointable row, from its first such cell to its last.
/// Empty for a row that offers nothing.
fn door(row: &Row, room: usize) -> Range<usize> {
    let mut column = 0;
    let mut door: Option<Range<usize>> = None;
    for (slot, text) in row.clipped(room).spans() {
        let end = column + width::columns(text);
        if slot == Slot::Pointed {
            door = Some(door.map_or(column..end, |door| door.start..end));
        }
        column = end;
    }
    door.unwrap_or(0..0)
}

/// Paints `rows` into buffers the caller keeps between frames.
///
/// Kept rather than built per frame because what stands at the foot is redrawn
/// on every keystroke, and a `String` per row per key is an allocation the
/// render path does not have to make.
///
/// Each row comes with the columns it may take, and is clipped to them here
/// rather than at the frame, because this is where the width is known and the
/// row still is: a row wider than the window would otherwise be wrapped by the
/// terminal onto a row belonging to another band.
fn paint<'a>(
    rows: impl IntoIterator<Item = (&'a Row, usize)>,
    palette: &Palette,
    into: &mut Vec<String>,
) {
    let mut count = 0;
    for (row, columns) in rows {
        if count == into.len() {
            into.push(String::new());
        }
        if let Some(painted) = into.get_mut(count) {
            painted.clear();
            row.clipped(columns).paint_into(palette, painted);
        }
        count += 1;
    }
    into.truncate(count);
}

#[cfg(test)]
mod tests;
