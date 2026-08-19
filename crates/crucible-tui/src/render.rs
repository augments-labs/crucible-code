//! Inline rendering: draw the live tail, commit everything above it.
//!
//! There is no alternate screen. Output goes into the terminal's own scrollback,
//! so the scroll buffer, the find bar and the scrollbar are the terminal's --
//! already written, already fast, already familiar. What this process keeps is
//! only the tail it is still changing.
//!
//! The redraw is the whole design in three sequences: return to the start of the
//! tail, erase from the cursor *down*, and write the tail again. Erasing
//! downward cannot reach scrollback, so committed output is unreachable by
//! construction rather than by care. The sequences themselves live in
//! [`frame`]; this module decides when a frame happens.

use crate::color::Palette;
use crate::markdown::Markdown;
use crate::row::Row;
use crate::terminal::{Size, Terminal, TerminalError};

mod frame;
mod plain;
mod region;
mod screen;
mod tail;

use frame::Frame;
pub use region::Caret;
use tail::Tail;

/// How many rows of the transcript are still this process's to change.
///
/// One: streamed text only ever grows at the end, so the row being written to is
/// the only one a later delta can alter. Every row above it was final the moment
/// the wrap broke it, and holding it live would mean writing it again on every
/// frame for the rest of the answer — a frame costing the height of the window
/// rather than the size of the delta, on the one path the burst budget bounds.
///
/// It is also what keeps the live region on screen. A frame is taken back by
/// moving the cursor up over it, so the tail and whatever stands under it have
/// to fit together: one taller than the window is one whose top has already
/// scrolled out of reach. A tail of a single row leaves the rest of the window
/// to the component standing under it, so there is nothing to negotiate.
///
/// What it costs is re-wrapping: [`Renderer::resized`] can only re-wrap the row
/// still being written to. Everything above it was committed at the width it was
/// written at, and reflowing scrollback is the terminal's job — which it was for
/// most of the answer regardless, since a taller tail would still have committed
/// everything that scrolled out of it.
const LIVE: usize = 1;

/// Draws the transcript into the terminal's scrollback.
#[derive(Debug)]
pub struct Renderer<T: Terminal> {
    terminal: T,
    tail: Tail,
    /// Wraps committed lines by the same rules as streamed output without
    /// duplicating the wrap. Its bound is one, so every row it is given
    /// overflows immediately instead of staying live.
    finished: Tail,
    /// Rows currently on screen. What the next frame must move back over, and
    /// one of the two pieces of screen state this process tracks.
    drawn: usize,
    /// How many of those rows sit *below* the cursor.
    ///
    /// Zero for everything streamed, where the cursor is left at the end of the
    /// last row written. A prompt parks it on the row being typed instead, so
    /// the way back to the top of the region is that many rows shorter than the
    /// region is tall.
    parked: usize,
    /// The size the tail is currently wrapped for.
    ///
    /// Rows as much as columns, though the tail's own height is [`LIVE`] and
    /// does not move: a rewind reaching further than the window is tall reaches
    /// above the top, which is what [`Renderer::region`] clamps against, and the
    /// components standing under the tail lay themselves out against it.
    size: Size,
    /// Reused across frames: a frame is one string, so it is one write.
    frame: Frame,
    /// Reused across frames: rows leaving the tail on their way to scrollback.
    overflow: Vec<String>,
    /// Reused across frames: the rows of a live region, painted by [`region`].
    painted: Vec<String>,
    /// The rows standing under the tail, painted by [`region`] as well.
    ///
    /// Painted when they are set rather than once per frame. What they say
    /// changes when the session changes, and a turn is a great many frames.
    footing: Vec<String>,
    /// Where the cursor belongs inside [`Self::footing`], where somebody is
    /// typing into it. `None` parks the cursor at the end of the tail, which is
    /// where the next delta lands.
    footed: Option<Caret>,
    /// Reads the markers out of the model's markdown and says what each run is.
    ///
    /// Held here rather than made fresh per delta, for the reason the tail holds
    /// its escape state: a delta is a piece of the wire, so a fence arrives
    /// split across two of them as often as not.
    markdown: Markdown,
    /// The palette this run resolved, for the slots the scan above hands back.
    ///
    /// Plain until [`Renderer::wears`] says otherwise, which is what leaves a
    /// renderer nobody told showing the answer exactly as it arrived.
    palette: Palette,
    /// How many rows the record has grown by this session.
    ///
    /// Never read for its own sake — what it is for is the difference between
    /// two readings of it, which is how far a row written at the first has
    /// travelled up the screen by the second. That is the whole of what an
    /// inline renderer can know about a row it has let go of: this process
    /// draws into the terminal's own buffer and never learns where the top of
    /// the screen is, so a row's place is counted from the live region
    /// downwards rather than from an origin.
    ///
    /// One `usize` and no more: what is counted is rows, not rows kept.
    record: usize,
    /// Whether the record already ends in a blank row.
    ///
    /// What [`Renderer::apart`] reads, and the reason the rhythm is one
    /// decision rather than one per caller: the thing that knows whether a
    /// blank is owed is the thing that knows what was last written. True to
    /// start with, because the top of a session is already a boundary and a
    /// transcript that opens on an empty row has spent one for nothing.
    parted: bool,
}

impl<T: Terminal> Renderer<T> {
    /// A renderer drawing on `terminal`.
    ///
    /// The tail holds [`LIVE`] rows. Everything else a frame draws has already
    /// overflowed into scrollback and is the terminal's to keep.
    ///
    /// Nothing here can fail, and the signature says so. A terminal that will
    /// not report a size is still a terminal worth drawing on, so the size is
    /// guessed rather than refused — and a width that turns out wrong is
    /// corrected by [`Renderer::resized`] at the next prompt, once the terminal
    /// will say what it is. Somewhere that never answers keeps the guess for the
    /// whole session, which is the right trade for a pipe.
    #[must_use]
    pub fn new(terminal: T) -> Self {
        let size = terminal.size().unwrap_or(Size::FALLBACK);

        Self {
            tail: Tail::new(size.columns, LIVE),
            finished: Tail::new(size.columns, 1),
            terminal,
            drawn: 0,
            parked: 0,
            size,
            frame: Frame::new(),
            overflow: Vec::new(),
            painted: Vec::new(),
            footing: Vec::new(),
            footed: None,
            markdown: Markdown::default(),
            palette: Palette::plain(),
            record: 0,
            parted: true,
        }
    }

    /// Tells this renderer which palette the run resolved.
    ///
    /// Said once, at startup, because it is settled once: the configuration,
    /// the environment and `is_terminal` have already agreed between them. It
    /// is what decides whether the model's markdown is read at all — a run with
    /// no colour in it keeps every marker the model wrote, since dropping one
    /// there would take the emphasis away and put nothing in its place.
    pub fn wears(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Appends streamed output and puts a frame on screen.
    ///
    /// The markers in the model's markdown are read here rather than drawn: a
    /// heading, a run of code or a phrase under emphasis is recognised, its
    /// marker is dropped, and the run it covered is written wearing a slot. The
    /// tone belongs to the row rather than to the text, so it costs no column
    /// and the answer wraps where the same answer would have wrapped plain.
    ///
    /// One frame per delta however many slots it turned out to hold. A slot
    /// changes between two pieces of one delta, and a frame per change would be
    /// several frames for one piece of the wire.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn stream(&mut self, delta: &str) -> Result<(), TerminalError> {
        if !delta.trim().is_empty() {
            self.parted = false;
        }

        // Nowhere to put a slot is nowhere to put a marker either. A redirected
        // run, `NO_COLOR`, `--color never`: the answer arrives as the model
        // wrote it, which is markdown, and a file of markdown is worth more
        // than a file it has been taken out of.
        if !self.palette.writes_color() {
            self.tail.push(delta, &mut self.overflow);
            return self.draw();
        }

        let Self {
            markdown,
            tail,
            overflow,
            palette,
            ..
        } = self;
        let palette = *palette;

        markdown.read(delta, &mut |slot, text| {
            tail.wear(slot, &palette);
            tail.push(text, overflow);
        });

        self.draw()
    }

    /// Writes a line that is finished and will never be redrawn.
    ///
    /// Used for a completed message, a tool result, or anything else that
    /// belongs to the record rather than to the live region.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn commit(&mut self, line: &str) -> Result<(), TerminalError> {
        self.parted = line.trim().is_empty();
        self.finished.push(line, &mut self.overflow);
        // The newline is what makes the last row complete, and only complete
        // rows leave a tail.
        self.finished.push("\n", &mut self.overflow);
        self.draw()
    }

    /// Writes rows crucible composed itself, in colour, above everything after
    /// them.
    ///
    /// The counterpart to [`Renderer::commit`], and separate from it for one
    /// reason: a committed line is text that came from somewhere else, so it
    /// goes through the same wrap and the same escape-dropping as streamed
    /// output — a colour code in a tool result is bytes an untrusted string put
    /// there. A [`Row`] is not text that arrived; it is spans this program
    /// built, whose colour the palette decides here, at the last moment.
    ///
    /// Not wrapped either, and it does not need to be: a component is given the
    /// width and returns rows that fit it. Nothing is ever redrawn over these,
    /// so no frame is counting the columns they cost.
    ///
    /// Whatever stands under the tail is put back afterwards. Ending the live
    /// region is what makes room to write above it and takes that row off the
    /// screen with everything else, and a presented row is the one frame with
    /// nothing after it to draw it again — so this one draws it, and a result
    /// arriving mid-turn lands above the box rather than in place of it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn present(&mut self, rows: &[Row], palette: Palette) -> Result<(), TerminalError> {
        self.settle()?;

        if let Some(last) = rows.last() {
            self.parted = last.text().trim().is_empty();
        }

        let terminal = self.terminal.is_terminal();
        let mut painted = String::new();

        self.record = self.record.saturating_add(rows.len());
        self.frame.plain();
        for row in rows {
            painted.clear();
            row.paint_into(&palette, &mut painted);
            self.frame.settled(&painted, terminal);
        }

        self.terminal.write(self.frame.sealed())?;

        // Between turns there is nothing standing, and a frame drawn to put
        // nothing back is a write and a flush on every row of a transcript that
        // only gets longer.
        if self.footing.is_empty() {
            return self.terminal.flush();
        }

        self.draw()
    }

    /// Draws rows crucible composed itself and leaves them live, with the
    /// cursor where `caret` says it goes.
    ///
    /// The counterpart to [`Renderer::present`] for a component that is still
    /// being changed: the same rows and the same palette, but redrawn where
    /// they stand instead of being written once and forgotten. What a keystroke
    /// costs is therefore one frame — the region is erased and put back — and
    /// the caller redraws only when something moved.
    ///
    /// The cursor is left on the row the caret named rather than at the end of
    /// what was written, so the terminal's own cursor is the one the reader
    /// sees, in whatever shape and blink they chose. Nothing is drawn to stand
    /// in for it.
    ///
    /// Nothing at all happens where output is redirected. A live region is a
    /// thing only a terminal has, and a run whose output is a file has no
    /// keystrokes arriving to redraw for either — the same condition [`Raw`]
    /// refuses to enter under.
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

        // Anything the last turn left live belongs above the prompt rather than
        // underneath it, and a rewind that reached over it would take it off
        // the screen without it ever having been written down.
        if !self.tail.is_empty() {
            self.settle()?;
        }

        region::paint(rows, &palette, &mut self.painted);

        let region = self.region();
        self.parked = region::draw(&mut self.frame, region, &self.painted, caret);
        self.drawn = self.painted.len();

        self.terminal.write(self.frame.sealed())?;
        self.terminal.flush()
    }

    /// Keeps `rows` under the tail until something takes them back.
    ///
    /// The counterpart to [`Renderer::live`] for rows nobody is typing into.
    /// Streamed output goes on arriving above them and every frame draws them
    /// again underneath it, so they stay on screen through a turn instead of
    /// being the first thing it scrolls away. An empty slice takes them back.
    ///
    /// They never settle. What stands here is a fact about the session rather
    /// than something that was said, so the record reads afterwards as though
    /// it had never been there — and the alternative, a row committed once a
    /// frame, is a session's worth of scrollback repeating itself.
    ///
    /// Nothing happens where output is redirected, for the reason
    /// [`Renderer::live`] draws nothing there: a file has no bottom row to hold
    /// anything at.
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

        region::paint(rows, &palette, &mut self.footing);
        self.footed = caret;
        self.draw()
    }

    /// Ends the live region, leaving what it held in scrollback.
    ///
    /// Called between turns. After this the cursor sits on a fresh row and the
    /// next frame starts a new tail. Whatever was standing under the tail is
    /// erased with it and is not written down, but it is not forgotten: the
    /// next frame draws it again, so a question asked in the middle of a turn
    /// costs it nothing.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn settle(&mut self) -> Result<(), TerminalError> {
        // The markers belong to the message that is ending. A fence the model
        // opened and never closed would otherwise read the tool result under it
        // as code, and the whole of the next answer after that.
        self.markdown = Markdown::default();

        if !self.terminal.is_terminal() {
            return self.settle_plain();
        }

        if self.tail.is_empty() {
            // Nothing live is worth keeping, but the region may still be on
            // screen from a frame that only committed rows. The tail is emptied
            // whichever way this ends, so both paths leave the same thing
            // behind: empty rows kept here would be drawn again by the next
            // frame and settle into the record as blank lines nobody wrote.
            self.tail.clear();

            if self.drawn > 0 {
                self.erase()?;
                self.terminal.flush()?;
            }
            return Ok(());
        }

        let region = self.region();
        self.record = self
            .record
            .saturating_add(self.overflow.len() + self.tail.content().len());
        screen::settle(&mut self.frame, region, &mut self.overflow, &mut self.tail);

        self.terminal.write(self.frame.sealed())?;
        self.terminal.flush()?;

        self.drawn = 0;
        self.parked = 0;
        Ok(())
    }

    /// Re-wraps for a terminal the user resized.
    ///
    /// The rows already on screen were wrapped for the old width, so on a
    /// terminal what is live is dropped rather than redrawn wrongly: it is at
    /// most one screen, and the model is still streaming into it. Height counts
    /// as much as width, because the height is what bounds the tail.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn resized(&mut self) -> Result<(), TerminalError> {
        let size = self.terminal.size().unwrap_or(Size::FALLBACK);
        if size == self.size {
            return Ok(());
        }

        // Before the erase rather than after it. An erase is counted in rows of
        // the screen it is written to, and by now that is the new one: the
        // region below has to be measured against the window the cursor is
        // sitting in, not the one it was drawn on.
        self.size = size;

        // The size comes from the terminal, not from the stream, so a run whose
        // output is redirected still sees the window change. There is no live
        // region in a pipe to wipe, and an erase sequence written into one ends
        // up in whatever kept the output -- but neither can a pipe take a row
        // back, so what is live there is written out instead of dropped. The
        // rewrap below applies either way.
        if self.terminal.is_terminal() {
            // Only ever what this renderer drew, which is what `drawn` counts.
            // With nothing counted there is nothing of its own on screen to
            // drop, and the row the cursor is on is one a prompt was written
            // verbatim onto -- a row this promised no frame would move back
            // over. A question waiting to be answered is exactly that state,
            // and erasing it takes the offer off the screen while somebody is
            // still reading it.
            if self.drawn > 0 {
                self.erase()?;
                self.terminal.flush()?;
            }
        } else {
            self.settle_plain()?;
        }

        self.tail = Tail::new(size.columns, LIVE);
        self.finished = Tail::new(size.columns, 1);
        // Dropped for the reason the live rows are: it was laid out for a width
        // the window no longer has, and a row too wide for the screen is one
        // the terminal wraps itself. The caller lays out the next one.
        self.footing.clear();
        self.footed = None;
        self.drawn = 0;
        self.parked = 0;
        Ok(())
    }

    /// Ends the live region and writes `text` exactly as given.
    ///
    /// Nothing is added: a prompt mark the user types after wants no line
    /// ending, and a caller that wants the row to end puts one in `text`. The
    /// bytes are not counted either, which is safe because the settle above has
    /// just left nothing live -- no frame will ever move back over this row.
    /// That is also what makes colour the caller's to decide and to put into
    /// `text`: escape bytes in a row that is never redrawn cannot cost a column
    /// this renderer is counting.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn prompt(&mut self, text: &str) -> Result<(), TerminalError> {
        if !text.trim().is_empty() {
            self.parted = false;
        }

        self.settle()?;
        self.terminal.write(text)?;
        self.terminal.flush()
    }

    /// Leaves one blank row between what has been written and what comes next.
    ///
    /// The transcript is a column of blocks — what was asked, what was
    /// answered, each call and the line under it — and what separates one from
    /// the next is a row of nothing. Every block asks for that row on its way
    /// in rather than leaving one behind, because a block cannot know it was
    /// the last: a session that parted on the way out would end on a blank row
    /// under the final answer, and the shell's own prompt would come back one
    /// row lower than it left.
    ///
    /// Blank rows do not accumulate, and none is owed at the very top, where
    /// the boundary is the start of the session. Nor is one owed while the tail
    /// still holds something: what comes next is then the rest of that line
    /// rather than a block after it, and a caller that asks on every piece of a
    /// streamed answer — which is the only way it can ask on the first — must
    /// get a row before the answer and none inside it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn apart(&mut self) -> Result<(), TerminalError> {
        if self.parted || !self.tail.is_empty() {
            return Ok(());
        }

        self.commit("")
    }

    /// Whether output is going to a terminal rather than a pipe or a file.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.terminal.is_terminal()
    }

    /// The terminal underneath, to read rather than to write on.
    ///
    /// Shared and not exclusive: a write made through here would land in the
    /// middle of the live region, and the count of what is on screen would be
    /// wrong with nothing in this crate able to notice. [`Renderer::prompt`] is
    /// how a caller puts bytes down; this is how it asks what came back.
    pub fn terminal(&self) -> &T {
        &self.terminal
    }

    /// How wide the terminal was when this last looked.
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
    /// Held for the same reason the width is, and read for one thing: a live
    /// region taller than the screen is one whose top has scrolled out of
    /// reach, so the next rewind would move back over rows the terminal has
    /// already taken. A component that can grow asks how much room there is
    /// before it does.
    #[must_use]
    pub fn rows(&self) -> usize {
        self.size.rows
    }

    /// How many rows have gone into the record.
    ///
    /// Read straight after writing something, to learn where it went: a caller
    /// that means to point at a row later keeps this number and asks
    /// [`Renderer::recorded`] about it, and the difference between the two
    /// readings is how far the row has travelled.
    ///
    /// It counts rows written rather than rows kept, so it goes on rising for
    /// the length of a session and says nothing about what is still on screen.
    /// That is [`Renderer::recorded`]'s half of the question.
    #[must_use]
    pub fn record(&self) -> usize {
        self.record
    }

    /// Which row of the live region is on screen row `at`, with the cursor on
    /// screen row `cursor`.
    ///
    /// The half of [`Renderer::top`] that looks downwards. `None` above the
    /// region and `None` below the last row of it, which are the two ways a
    /// pointer lands on something this renderer is not currently drawing.
    #[must_use]
    pub fn within(&self, at: usize, cursor: usize) -> Option<usize> {
        at.checked_sub(self.top(cursor)?)
            .filter(|row| *row < self.drawn)
    }

    /// Which row of the record is on screen row `at`, with the cursor on
    /// screen row `cursor`.
    ///
    /// The half that looks upwards. Directly above the region is the last row
    /// of the record, so a row that many back from the end of it is the answer.
    ///
    /// `None` where that would be a guess: a row inside the live region or
    /// below it, which the record does not hold, and a row above everything
    /// this renderer has written, which belongs to whatever ran before it.
    #[must_use]
    pub fn recorded(&self, at: usize, cursor: usize) -> Option<usize> {
        // Zero is the region's own first row rather than the record's last,
        // which is why this is filtered rather than saturated.
        let above = self
            .top(cursor)?
            .checked_sub(at)
            .filter(|above| *above > 0)?;

        self.record.checked_sub(above)
    }

    /// Which screen row the live region starts on, with the cursor on screen
    /// row `cursor`.
    ///
    /// An inline renderer never learns where the top of the screen is: what it
    /// draws starts wherever the shell left off, and the terminal scrolls it
    /// without saying so. What it does know is the shape of its own live
    /// region — how tall it is and how far up the cursor was parked in it — so
    /// a caller that has asked the terminal where its cursor is has given this
    /// the one fact it was missing, and both directions follow from here.
    fn top(&self, cursor: usize) -> Option<usize> {
        // Where the cursor sits inside the region, which is what its own two
        // counts come to: the rows drawn, less the ones below it, less the one
        // it is on. Nothing drawn leaves it on the row after the record, which
        // is where a settle steps it to, and the arithmetic holds there
        // unchanged.
        cursor.checked_sub(self.drawn.saturating_sub(self.parked + 1))
    }

    /// One frame, assembled by [`screen`].
    fn draw(&mut self) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return self.draw_plain();
        }

        let region = self.region();
        self.record = self.record.saturating_add(self.overflow.len());
        self.parked = screen::draw(
            &mut self.frame,
            region,
            &mut self.overflow,
            &self.tail,
            (&self.footing, self.footed),
        );

        self.drawn = self.tail.len() + self.footing.len();
        self.terminal.write(self.frame.sealed())?;
        self.terminal.flush()
    }

    /// A frame for a pipe, assembled by [`plain`].
    fn draw_plain(&mut self) -> Result<(), TerminalError> {
        self.record = self.record.saturating_add(self.overflow.len());

        if !plain::draw(&mut self.frame, &mut self.overflow) {
            return Ok(());
        }

        self.terminal.write(self.frame.sealed())?;
        self.terminal.flush()
    }

    /// Ending a turn into a pipe, assembled by [`plain`].
    fn settle_plain(&mut self) -> Result<(), TerminalError> {
        self.record = self
            .record
            .saturating_add(self.overflow.len() + self.tail.content().len());

        if !plain::settle(&mut self.frame, &mut self.overflow, &mut self.tail) {
            return Ok(());
        }

        self.terminal.write(self.frame.sealed())?;
        self.terminal.flush()
    }

    /// Wipes the live region without drawing anything back.
    fn erase(&mut self) -> Result<(), TerminalError> {
        self.frame.rewind(self.region());
        self.drawn = 0;
        self.parked = 0;
        self.terminal.write(self.frame.sealed())
    }

    /// How many rows a rewind has to move back over to reach the top of the
    /// live region.
    ///
    /// The rows on screen, less the ones below wherever the cursor was parked,
    /// and never more than the window is tall. A rewind moves the cursor
    /// through rows of the screen it is written to; above the top of that
    /// screen there is no live region left to reach, only scrollback that has
    /// been committed and is still being read.
    ///
    /// The two counts come apart when the window gets shorter: the rows were
    /// drawn on the taller one, the terminal has already scrolled the top of
    /// them away, and what is left to take back is one screen of them at most.
    /// Clamping here rather than at the one caller that resizes, because this
    /// is the whole of the answer to how far back a frame may reach, and a
    /// second answer beside it is one that would go on being right by accident.
    fn region(&self) -> usize {
        self.drawn.saturating_sub(self.parked).min(self.size.rows)
    }
}

#[cfg(test)]
mod tests;
