//! Full-screen rendering: crucible owns the window and every row in it.
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
//! the most on cannot be expressed here.
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
use crate::glyphs::Glyphs;
use crate::head::Head;
use crate::markdown::Markdown;
use crate::record::Record;
use crate::row::Row;
use crate::select::Taken;
use crate::terminal::keys::{Pressed, pressed, waiting};
use crate::terminal::{Size, Terminal, TerminalError};
use crate::transcript_map::{self, TranscriptMap};

use std::ops::Range;
use std::time::{Duration, Instant};

mod frame;
mod painted;

use painted::Painted;

/// How far one notch of the wheel moves the transcript, until told otherwise.
///
/// A renderer nobody configured still has to answer the wheel, and the answer
/// that is wrong here is nought: it looks like a terminal that has stopped
/// reporting rather than like a setting waiting to be made. Three rows is a
/// short paragraph, which is enough to see the picture move.
const NOTCH: i32 = 3;

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

/// The rows standing at the foot of the window, painted.
///
/// Painted when they are set rather than once per frame: what they say changes
/// when the session changes, and a turn is a great many frames.
///
/// Two slots, in the order they are drawn down the screen, and either may be
/// full without the other: a turn stands in the first while it runs, a list a
/// line opened stands there between turns, and the box holds the second
/// whenever there is one to type into.
#[derive(Debug, Default)]
struct Standing {
    /// What a running turn is showing, and anything else standing over the box.
    turn: Vec<String>,
    /// Where the cursor belongs in it, where anything is typing into it.
    turned: Option<Caret>,
    /// The box.
    prompt: Vec<String>,
    /// Where the cursor belongs in the box.
    prompted: Option<Caret>,
}

impl Standing {
    /// Forget both, for a window whose size has changed underneath them.
    fn clear(&mut self) {
        self.turn.clear();
        self.turned = None;
        self.prompt.clear();
        self.prompted = None;
    }
}

/// What the row at the top of the window says, in its own words.
///
/// Held rather than the row it becomes, because the row is laid out against a
/// window that changes size and what it says does not. Owned rather than
/// borrowed for the same reason: it outlives every call that sets it, and what
/// it names belongs to the session rather than to a frame.
#[derive(Debug)]
struct Heading {
    /// The directory the session is bound to.
    root: String,
}

/// Draws the session onto a screen this process owns.
#[derive(Debug)]
pub struct Renderer<T: Terminal> {
    terminal: T,
    /// Everything that has been said, and where in it the reader is looking.
    record: Record,
    /// The rows at the foot of the window that are not the transcript.
    standing: Standing,
    /// What the row at the top says, and that row laid out for this window.
    ///
    /// Both, because the second is what a frame puts down and the first is what
    /// a resize lays out again. This is the one component this crate lays out
    /// itself: everything else standing is handed over as rows and dropped when
    /// the window changes under it, and a row that is meant to be there the
    /// whole session cannot be a row the caller has to remember to put back.
    /// The opening is laid out again too and is not one of these — it is a line
    /// of the record rather than a band, and [`Self::opens`] says how.
    heading: Option<Heading>,
    crowned: Option<Row>,
    /// Whether the pointer is over the transcript-map door in the fixed foot.
    ///
    /// One bit rather than its column, because every motion on the same side of
    /// either edge should cost no layout and no frame.
    map_pointed: bool,
    /// The row of the prompt that offers an action while the box is standing.
    ///
    /// Relative to the prompt band. The renderer owns the absolute placement,
    /// so this is enough for it to decide whether a motion crossed the offer
    /// without teaching it what the row means.
    prompt_target: Option<usize>,
    /// Whether a pointer transition is waiting for the prompt and fixed foot
    /// to be replaced together.
    pointed_changed: bool,
    /// The size the record is folded for and the bands are shared out over.
    ///
    /// Held rather than asked for per frame: a read costs a syscall, and
    /// [`Renderer::resized`] is what keeps it true.
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
    /// Absolute travel over the retained transcript in the fixed bottom row.
    map: TranscriptMap,
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
    /// Screen rows and columns, and therefore only ever true of the frame it
    /// was made against. Dropped by anything that moves the picture out from
    /// under it — a scroll, a resize — because a selection that stayed put
    /// while its words moved is a highlight over the wrong text.
    taken: Option<Taken>,
    /// Which window row the pointer was last reported on.
    ///
    /// A row rather than what is on it, and kept across everything that moves
    /// the picture: what the pointer is over is worked out again for every
    /// frame, from this and from where the record has got to. A pointer resting
    /// still while an answer scrolls under it is over whatever is under it now,
    /// which is what a reader watching the screen sees.
    pointing: Option<usize>,
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
        let size = terminal.size().unwrap_or(Size::FALLBACK);

        Self {
            record: Record::new(size.columns),
            terminal,
            standing: Standing::default(),
            heading: None,
            crowned: None,
            map_pointed: false,
            prompt_target: None,
            pointed_changed: false,
            size,
            painted: Painted::new(),
            free: String::new(),
            escapes: Escapes::default(),
            markdown: Markdown::default(),
            palette: Palette::plain(),
            glyphs: Glyphs::default(),
            map: TranscriptMap::default(),
            notch: NOTCH,
            taken: None,
            pointing: None,
        }
    }

    /// Waits for one press, while still restoring an idle transcript map.
    ///
    /// `None` where the map or selection consumed the press. A map deadline
    /// wakes this wait, redraws the identity row, and waits again; it never
    /// becomes a key the caller could mistake for input.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be read or written.
    pub fn pressed(&mut self) -> Result<Option<Pressed>, TerminalError> {
        loop {
            if let Some(patience) = self.rests_in()
                && !waiting(patience)?
            {
                self.repose()?;
                continue;
            }
            return self.took(pressed()?);
        }
    }

    /// Whether a press is ready within `patience`, shortening that wait to an
    /// open map's idle deadline when necessary.
    ///
    /// The caller already has something else to watch — a running turn or a
    /// login attempt — so a map that returned to rest answers `false` and lets
    /// that caller make its ordinary pass before polling again.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be read or written.
    pub fn waiting(&mut self, patience: Duration) -> Result<bool, TerminalError> {
        self.repose()?;
        let patience = self.rests_in().map_or(patience, |map| map.min(patience));
        let ready = waiting(patience)?;
        if !ready {
            self.repose()?;
        }
        Ok(ready)
    }

    /// What a press means once the selection has had it.
    ///
    /// The one seam every input loop wraps its reads in, so that a drag works
    /// the same wherever the reader started it: the loop hands over what
    /// arrived and gets back what is left for it to answer. `None` where the
    /// press belonged to the selection and nothing else is owed — the pointer
    /// moving under a held button, and the button coming up again.
    ///
    /// A click is handed back rather than swallowed. It anchors a drag that may
    /// never happen, and until it does it still means whatever it meant to the
    /// loop underneath — a caret placed in the box, a cut result opened.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn took(&mut self, arrived: Pressed) -> Result<Option<Pressed>, TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(Some(arrived));
        }

        let bands = self.bands();
        let on_map = |row| bands.foot.end.checked_sub(1) == Some(row);
        if let Pressed::Hovered { row, column } = arrived {
            let lit = self.pointed();
            let prompt_pointed = self.prompt_pointed();
            let over = !self.map.open()
                && on_map(row)
                && transcript_map::door(self.size.columns)
                    .is_some_and(|door| door.contains(&column));
            let crossed = over != self.map_pointed;
            self.pointing = Some(row);
            self.map_pointed = over;
            let changed =
                lit != self.pointed() || prompt_pointed != self.prompt_pointed() || crossed;
            if changed && self.prompt_target.is_some() {
                // The caller has the pointable row in both of its palette
                // states. It replaces that row, the rest of the prompt, and
                // the map in one candidate rather than letting this write an
                // intermediate frame.
                self.pointed_changed = true;
            } else if changed {
                // One frame for both answers: main's cut-result offer is a fact
                // about the row, while the map chip is a fact about crossing
                // either horizontal edge inside its row.
                self.draw()?;
            }
            return Ok(None);
        }
        if self.map.open() {
            match arrived {
                Pressed::Clicked { row, column }
                    if on_map(row)
                        && transcript_map::track(self.size.columns)
                            .is_some_and(|track| track.contains(&column)) =>
                {
                    self.map.press(column);
                    return Ok(None);
                }
                Pressed::Dragged { column, .. } if self.map.drag() => {
                    self.seek_map(column, false)?;
                    return Ok(None);
                }
                Pressed::Released { column, .. } => {
                    if let Some((column, dragged)) = self.map.release(column, Instant::now()) {
                        self.seek_map(column, !dragged)?;
                        return Ok(None);
                    }
                }
                Pressed::Scrolled { back } => {
                    self.notched(back)?;
                    return Ok(None);
                }
                _ => {}
            }
        } else if let Pressed::Clicked { row, column } = arrived
            && on_map(row)
            && transcript_map::door(self.size.columns).is_some_and(|door| door.contains(&column))
        {
            self.open_map()?;
            return Ok(None);
        }

        match arrived {
            // A new press drops whatever the last one selected, which is how a
            // reader puts a selection away: click, anywhere.
            Pressed::Clicked { row, column } => {
                let had = self.taken.is_some_and(|taken| !taken.empty());
                self.taken = Some(Taken::opened(row, column));
                if had {
                    self.draw()?;
                }
                Ok(Some(arrived))
            }
            Pressed::Dragged { row, column } => {
                if let Some(taken) = &mut self.taken {
                    taken.reaches(row, column);
                    self.draw()?;
                }
                Ok(None)
            }
            // Copied where the drag covered something, and quietly dropped
            // where it did not: a button coming up after a plain click is not
            // an empty clipboard, it is nothing at all.
            Pressed::Released { .. } => {
                if self.taken.is_some_and(|taken| !taken.empty()) {
                    let said = self.painted.read();
                    self.copied(&said)?;
                }
                Ok(None)
            }
            _ => Ok(Some(arrived)),
        }
    }

    /// Whether a pointer transition is waiting for a pointable prompt redraw.
    ///
    /// Taken once. Motion within the same effective target sets no new
    /// transition, so all-motion reporting does not turn into one frame per
    /// cell. A transition already pending remains true until this takes it.
    #[must_use]
    pub fn pointed_changed(&mut self) -> bool {
        std::mem::take(&mut self.pointed_changed)
    }

    /// The rows showing the result the transcript cut short that the pointer is
    /// resting on. Empty where it is resting on nothing of the kind.
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

        let Some(row) = self.pointing else {
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

    /// Whether the pointer is over the prompt row the caller marked pointable.
    fn prompt_pointed(&self) -> bool {
        let (Some(row), Some(target)) = (self.pointing, self.prompt_target) else {
            return false;
        };

        matches!(self.aimed(row), Some(Aimed::Boxed(at)) if at == target)
    }

    /// Drops whatever is selected, because the picture under it is about to
    /// move.
    fn unselects(&mut self) {
        self.taken = None;
        self.painted.selects(None);
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
    /// Said once, at startup, because which set a font has is settled before
    /// the first frame and no command changes it. It reaches the transcript
    /// through the markdown reader, which is the one thing here that puts a
    /// character of its own in place of one the model wrote.
    pub fn draws(&mut self, glyphs: Glyphs) {
        self.glyphs = glyphs;
        self.markdown = Markdown::new(glyphs);
        self.crown();
    }

    /// Marks the next record line as the start of a prompt.
    ///
    /// Called after the blank parting it from the previous block and before the
    /// prompt rows themselves, so a map landmark lands on the words it names.
    pub fn landmark(&mut self) {
        self.record.landmark();
    }

    /// How long an input wait may sleep before the map restores the identity
    /// row. `None` where no map is standing or a drag is still held.
    #[must_use]
    fn rests_in(&self) -> Option<Duration> {
        self.map.remaining(Instant::now())
    }

    /// Restores the bottom-row control if the map has been idle long enough.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the restored row could not be drawn.
    fn repose(&mut self) -> Result<bool, TerminalError> {
        if !self.map.repose(Instant::now()) {
            return Ok(false);
        }
        self.draw()?;
        Ok(true)
    }

    /// Restores the bottom-row control now.
    ///
    /// Used before a secret box takes the keyboard, where no pointer action is
    /// offered to the renderer and an open map would otherwise have no clock.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the restored row could not be drawn.
    pub fn identifies(&mut self) -> Result<bool, TerminalError> {
        let changed = self.map.close() || self.map_pointed;
        if !changed {
            return Ok(false);
        }
        self.map_pointed = false;
        self.draw()?;
        Ok(true)
    }

    /// Tells this renderer how far one notch of the wheel moves the transcript.
    ///
    /// Said once, at startup. The wheel arrives as a count of notches and
    /// nothing more — how far one is worth is the reader's, and this is where
    /// their answer lands.
    pub fn rolls(&mut self, rows: i32) {
        self.notch = rows;
    }

    /// Puts the row at the top of the window and keeps it there.
    ///
    /// The one thing on screen that is neither the transcript nor something
    /// standing over the box: it says which directory the session is bound to
    /// and is held against the top while everything under it moves.
    ///
    /// Said at each prompt because that is where a resize is first known. What
    /// it says cannot change during a session, so the renderer owns the laid-out
    /// row and restores it at the new width itself.
    ///
    /// Nothing at all happens where output is redirected, for the reason
    /// [`Renderer::live`] draws nothing there.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn heads(&mut self, head: Head<'_>) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        self.heading = Some(Heading {
            root: head.root.to_owned(),
        });
        self.crown();
        self.draw()
    }

    /// Lays the head row out for the window as it is now.
    ///
    /// Called wherever what it is drawn against moves: the size, and the set of
    /// characters it may draw with.
    fn crown(&mut self) {
        let glyphs = self.glyphs;
        let columns = self.size.columns;

        self.crowned = self.heading.as_ref().map(|heading| {
            Head {
                root: &heading.root,
            }
            .row(columns, glyphs)
        });
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
        // Nowhere to put a slot is nowhere to put a marker either. A redirected
        // run, `NO_COLOR`, `--color never`: the answer arrives as the model
        // wrote it, which is markdown, and a file of markdown is worth more
        // than a file it has been taken out of.
        if !self.palette.writes_color() {
            self.take(Slot::Plain, delta)?;
            return self.draw();
        }

        let columns = self.size.columns;
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
        markdown.read(delta, columns, &mut |slot, text| {
            let said = taking.take(slot, text);
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
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn commit(&mut self, line: &str) -> Result<(), TerminalError> {
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
    /// width and returns rows that fit it. A window that narrows clips them
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

    /// Writes the opening into the transcript, keeping what draws it.
    ///
    /// Everything else handed to [`Self::present`] arrives as rows and stays as
    /// rows: what laid them out is a component that answered once and went, so
    /// a narrower window clips them. The opening is the exception, and the only
    /// one. It is drawn from facts read once at launch and held for the whole
    /// session, so what laid it is still here to lay it again — and it is what
    /// a reader is looking at when they take the corner of a fresh window and
    /// pull.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn opens(&mut self, lay: Box<dyn Fn(usize) -> Vec<Row>>) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            // Nothing will resize a file, so holding what could draw it again
            // would be holding it for an event that cannot arrive.
            return self.present(&lay(self.columns()));
        }

        self.record.opens(lay);
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
    /// about what is standing above one. A caller that must replace prompt,
    /// standing foot, and transcript map together uses [`Renderer::replace`].
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

        paint(rows, &palette, self.size.columns, &mut self.standing.prompt);
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
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        paint(
            prompt.rows,
            &palette,
            self.size.columns,
            &mut self.standing.prompt,
        );
        paint(over, &palette, self.size.columns, &mut self.standing.turn);
        self.standing.prompted = Some(prompt.caret);
        self.standing.turned = None;
        self.prompt_target = prompt.pointed.map(|(at, _)| at);

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
    /// This band gives up its rows before the head and the foot and after the
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
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        paint(rows, &palette, self.size.columns, &mut self.standing.turn);
        self.standing.turned = caret;
        self.draw()
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
        let columns = self.size.columns;
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
        markdown.finish(columns, &mut |slot, text| {
            let said = taking.take(slot, text);
            if wrote.is_ok() {
                wrote = said;
            }
        });
        wrote?;

        // The markers belong to the message that is ending. A fence the model
        // opened and never closed would otherwise read the tool result under it
        // as code, and the whole of the next answer after that.
        self.markdown = Markdown::new(self.glyphs);

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
    /// What was standing is dropped rather than redrawn wrongly: it was laid
    /// out against a window that has gone, and the caller lays out the next
    /// one.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn resized(&mut self) -> Result<(), TerminalError> {
        let size = self.terminal.size().unwrap_or(Size::FALLBACK);
        if size == self.size {
            return Ok(());
        }

        self.size = size;
        self.record.resized(size.columns);
        self.standing.clear();
        self.prompt_target = None;
        self.pointed_changed = false;
        self.unselects();
        self.map.close();
        self.map_pointed = false;
        self.crown();

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
    /// The record's numbering carries on past the lines it drops, so a number
    /// some other part of the program is holding names the line it named and
    /// names nothing once that line has gone.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn empties(&mut self) -> Result<(), TerminalError> {
        self.record.empties();
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
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn apart(&mut self) -> Result<(), TerminalError> {
        if self.record.parted() || self.record.writing() {
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
        let moved = self.scrolled(if back { -self.notch } else { self.notch })?;
        self.map.touch(Instant::now());
        Ok(moved)
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
        if !self.terminal.is_terminal() {
            return Ok(false);
        }

        let rows = self.bands().transcript.len();
        if !self.record.scroll(by, rows) {
            return Ok(false);
        }

        self.unselects();
        self.refresh_map();
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
        self.refresh_map();
        self.draw()
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

    /// How tall it was when this last looked.
    ///
    /// Read by a component that can grow, which asks how much room there is
    /// before it does. How much of that room it may actually have is
    /// [`crate::bands`]'s answer and not the component's.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.size.rows
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

    /// What is under window row `at`.
    ///
    /// The whole of what a click means. On a screen this process owns, the
    /// answer needs nothing from the terminal: the bands say which region the
    /// row is in and the record says which line is on it, so there is no round
    /// trip to ask where the cursor happens to be.
    ///
    /// `None` for a row in a band nothing is drawn in, and for one below the
    /// last line the transcript is showing.
    #[must_use]
    pub fn aimed(&self, at: usize) -> Option<Aimed> {
        let bands = self.bands();

        if bands.transcript.contains(&at) {
            let into = at - bands.transcript.start;
            return self
                .record
                .at(into, bands.transcript.len())
                .map(Aimed::Line);
        }

        if bands.turn.contains(&at) {
            return Some(Aimed::Stood(at - bands.turn.start));
        }

        if bands.prompt.contains(&at) {
            return Some(Aimed::Boxed(at - bands.prompt.start));
        }

        None
    }

    /// Opens the map over the retained range the transcript band can reach.
    fn open_map(&mut self) -> Result<bool, TerminalError> {
        let rows = self.bands().transcript.len();
        let Some(span) = self.record.map_span(rows) else {
            return Ok(false);
        };
        let row = transcript_map::row(&self.record, span, self.size.columns, self.glyphs);
        self.unselects();
        self.map_pointed = false;
        self.map.show(span, row, Instant::now());
        self.draw()?;
        Ok(true)
    }

    /// Moves the map to `column`, snapping to a prompt landmark for a click and
    /// travelling exactly for a drag.
    fn seek_map(&mut self, column: usize, landmark: bool) -> Result<bool, TerminalError> {
        let Some(span) = self.map.span() else {
            return Ok(false);
        };
        let Some(track) = transcript_map::track(self.size.columns) else {
            return Ok(false);
        };
        let at = column.clamp(track.start, track.end.saturating_sub(1)) - track.start;
        let rows = self.bands().transcript.len();
        let moved = if landmark {
            self.record.map_seek_landmark(span, at, track.len(), rows)
        } else {
            self.record.map_seek(span, at, track.len(), rows)
        };
        self.unselects();
        self.refresh_map();
        self.draw()?;
        Ok(moved)
    }

    /// Lays the open map out again after its mark or the window moved.
    fn refresh_map(&mut self) {
        let Some(span) = self.map.span() else {
            return;
        };
        let row = transcript_map::row(&self.record, span, self.size.columns, self.glyphs);
        self.map.replace(row);
    }

    /// How the window is shared out, given what is standing in it.
    fn bands(&self) -> Bands {
        Bands::share(
            self.size.rows,
            Wants {
                head: self.crowned.as_ref().map_or(0, |_| Head::ROWS),
                turn: self.standing.turn.len(),
                prompt: self.standing.prompt.len(),
                // The bottom control belongs to a session, the same as the
                // fixed head. Before a session has named itself there is no
                // transcript-map door to offer.
                foot: self.crowned.as_ref().map_or(0, |_| 1),
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
        .take(slot, text)
    }

    /// One frame.
    fn draw(&mut self) -> Result<(), TerminalError> {
        // A redirected run has already been given every byte, at the moment
        // each arrived. What is left is to make sure it has them.
        if !self.terminal.is_terminal() {
            return self.terminal.flush();
        }

        let bands = self.bands();
        // Worked out once for the whole frame, and it names rows rather than
        // setting a mode: the result under the pointer is painted from the lit
        // palette and everything else on screen from the plain one.
        let lit = self.pointed();
        let palette = self.palette;
        let pointed = self.palette.pointing(true);
        self.painted.selects(self.taken);
        self.painted.open(self.size.rows, self.size.columns);

        if let Some(crowned) = &self.crowned {
            for at in bands.head.start..bands.head.end {
                self.painted.paint(at, crowned, &palette);
            }
        }

        let showing = self.record.view(bands.transcript.len());
        for at in bands.transcript.start..bands.transcript.end {
            match showing.get(at - bands.transcript.start) {
                Some(row) if lit.contains(&at) => self.painted.paint(at, row, &pointed),
                Some(row) => self.painted.paint(at, row, &palette),
                // Below what there is to show. A session that has just started
                // reads from the top of the window down, as a terminal's own
                // scrollback would.
                None => self.painted.blank(at),
            }
        }

        // Taken out and put back, because a painted row is borrowed from the
        // same `self` the frame is written into.
        let turn = std::mem::take(&mut self.standing.turn);
        for (at, row) in (bands.turn.start..bands.turn.end).zip(&turn) {
            self.painted.put(at, row);
        }
        self.standing.turn = turn;

        let prompt = std::mem::take(&mut self.standing.prompt);
        for (at, row) in (bands.prompt.start..bands.prompt.end).zip(&prompt) {
            self.painted.put(at, row);
        }
        self.standing.prompt = prompt;

        let map = self.map.row().cloned().unwrap_or_else(|| {
            transcript_map::resting(self.size.columns, self.glyphs, self.map_pointed)
        });
        if let Some(at) = (!bands.foot.is_empty()).then(|| bands.foot.end - 1) {
            self.painted.paint(at, &map, &self.palette);
        }

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
    /// The escape bytes go here and nowhere else. Colour in a tool result is
    /// bytes an untrusted string put there: a row of the record is spans this
    /// program painted from a palette, and a byte of a redirected run's output
    /// is one this program meant to write.
    fn take(&mut self, slot: Slot, text: &str) -> Result<(), TerminalError> {
        let escapes = &mut *self.escapes;
        self.free.clear();
        self.free
            .extend(text.chars().filter(|character| !escapes.holds(*character)));

        self.record.write(slot, self.free);

        match &mut self.out {
            Some(out) => out.write(self.free),
            None => Ok(()),
        }
    }
}

/// Paints `rows` into buffers the caller keeps between frames.
///
/// Kept rather than built per frame because what stands at the foot is redrawn
/// on every keystroke, and a `String` per row per key is an allocation the
/// render path does not have to make.
///
/// Clipped here rather than at the frame, because this is where the width is
/// known and the row still is: a row wider than the window would otherwise be
/// wrapped by the terminal onto a row belonging to another band.
fn paint(rows: &[Row], palette: &Palette, columns: usize, into: &mut Vec<String>) {
    into.resize_with(rows.len(), String::new);

    for (row, painted) in rows.iter().zip(into.iter_mut()) {
        painted.clear();
        row.clipped(columns).paint_into(palette, painted);
    }
}

#[cfg(test)]
mod tests;
