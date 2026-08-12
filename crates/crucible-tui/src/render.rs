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
    /// The size the tail is currently wrapped and bounded for.
    ///
    /// Rows as much as columns: the bound is the height, so a window that only
    /// got shorter would otherwise leave the tail holding more rows than there
    /// is screen to show them, and every rewind would reach above the top.
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
}

impl<T: Terminal> Renderer<T> {
    /// A renderer drawing on `terminal`.
    ///
    /// The tail is bounded by the height of the visible area: holding more than
    /// one screen would keep rows nobody can see.
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
            tail: Tail::new(size.columns, size.rows),
            finished: Tail::new(size.columns, 1),
            terminal,
            drawn: 0,
            parked: 0,
            size,
            frame: Frame::new(),
            overflow: Vec::new(),
            painted: Vec::new(),
            footing: Vec::new(),
            markdown: Markdown::default(),
            palette: Palette::plain(),
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
            tail.wear(slot, palette);
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
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn present(&mut self, rows: &[Row], palette: Palette) -> Result<(), TerminalError> {
        self.settle()?;

        let terminal = self.terminal.is_terminal();
        let mut painted = String::new();

        self.frame.plain();
        for row in rows {
            painted.clear();
            row.paint_into(palette, &mut painted);
            self.frame.settled(&painted, terminal);
        }

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
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

        region::paint(rows, palette, &mut self.painted);

        let region = self.region();
        self.parked = region::draw(&mut self.frame, region, &self.painted, caret);
        self.drawn = self.painted.len();

        self.terminal.write(self.frame.as_str())?;
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
    pub fn under(&mut self, rows: &[Row], palette: Palette) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }

        region::paint(rows, palette, &mut self.footing);
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
        screen::settle(&mut self.frame, region, &mut self.overflow, &mut self.tail);

        self.terminal.write(self.frame.as_str())?;
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

        // The size comes from the terminal, not from the stream, so a run whose
        // output is redirected still sees the window change. There is no live
        // region in a pipe to wipe, and an erase sequence written into one ends
        // up in whatever kept the output -- but neither can a pipe take a row
        // back, so what is live there is written out instead of dropped. The
        // rewrap below applies either way.
        if self.terminal.is_terminal() {
            self.erase()?;
            self.terminal.flush()?;
        } else {
            self.settle_plain()?;
        }

        self.size = size;
        self.tail = Tail::new(size.columns, size.rows);
        self.finished = Tail::new(size.columns, 1);
        // Dropped for the reason the live rows are: it was laid out for a width
        // the window no longer has, and a row too wide for the screen is one
        // the terminal wraps itself. The caller lays out the next one.
        self.footing.clear();
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
        self.settle()?;
        self.terminal.write(text)?;
        self.terminal.flush()
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

    /// One frame, assembled by [`screen`].
    fn draw(&mut self) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return self.draw_plain();
        }

        let region = self.region();
        self.parked = screen::draw(
            &mut self.frame,
            region,
            &mut self.overflow,
            &self.tail,
            &self.footing,
        );

        self.drawn = self.tail.len() + self.footing.len();
        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// A frame for a pipe, assembled by [`plain`].
    fn draw_plain(&mut self) -> Result<(), TerminalError> {
        if !plain::draw(&mut self.frame, &mut self.overflow) {
            return Ok(());
        }

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// Ending a turn into a pipe, assembled by [`plain`].
    fn settle_plain(&mut self) -> Result<(), TerminalError> {
        if !plain::settle(&mut self.frame, &mut self.overflow, &mut self.tail) {
            return Ok(());
        }

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// Wipes the live region without drawing anything back.
    fn erase(&mut self) -> Result<(), TerminalError> {
        self.frame.rewind(self.region());
        self.drawn = 0;
        self.parked = 0;
        self.terminal.write(self.frame.as_str())
    }

    /// How many rows a rewind has to move back over to reach the top of the
    /// live region.
    ///
    /// The rows on screen, less the ones below wherever the cursor was parked.
    fn region(&self) -> usize {
        self.drawn.saturating_sub(self.parked)
    }
}

#[cfg(test)]
mod tests;
