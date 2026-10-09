//! Native mode: drawing in the terminal's own buffer.
//!
//! The full screen names every row it writes, because it owns them all. Here
//! nothing is owned but a live region at the foot of whatever the reader's
//! shell left on screen, so a frame is relative: back to the top of the region
//! the last frame drew, erase to the end of the screen, write anything the
//! record has sent out as rows the terminal scrolls into its own scrollback,
//! then the region again — the transcript still being written, what a turn is
//! showing, and the box. No row is ever addressed by number, because a number
//! here lands on whatever the shell had put there.
//!
//! A line goes out once while the window keeps its size. It leaves the live
//! region when it is sealed — at the moment the session next waits for a key,
//! before anyone could read it half-finished — or sooner, when the region could
//! no longer show it. The terminal shows it from then on, and the record keeps
//! it under the same ceiling as the full screen's, for a resize to give back.
//! An edit to a line that has gone out ([`Renderer::amend`]) changes only the
//! copy kept, which the next resize shows; [`Renderer::subordinate`] leaves it
//! alone, as it was marked when it went out.
//!
//! Which is why a reply that can wait for a key, or outgrow the region, is
//! hung under its mark before it is written ([`Renderer::hangs`]): either
//! sends rows of it out before the command ends. Each line of it goes out
//! carrying the mark or the indent the full screen gives it, and
//! [`Renderer::subordinate`] marks the lines still held when it ends. A
//! one-line reply written after the last key wait is marked by
//! [`Renderer::subordinate`] alone, before the next wait seals it. A reply
//! that empties the transcript is let go of unmarked, as the full screen marks
//! none of it.
//!
//! Emptying the transcript takes nothing back from the screen: the session a
//! resume or a clear leaves stays in the scrollback, under the card the launch
//! drew, until a resize gives the window what is kept, which is only what came
//! after. What replaces it goes under one divider row ([`Renderer::divides`])
//! rather than under a second card, which would read as a second launch.
//!
//! What stands over or in place of the box is held to half the window here
//! ([`Renderer::room`]), because a transcript row it takes is let go of into
//! the terminal's scrollback, and taking the panel down again brings none of
//! them back. A panel that cannot be drawn in half the window stands at the
//! least it can be drawn in, and the rows that costs stay in the scrollback.
//!
//! The region keeps its height from one frame to the next. What a turn shows,
//! or a panel, grows it, and the rows it grows over scroll into the terminal's
//! scrollback; when the turn ends or the panel closes, a frame drawing fewer
//! rows than the last one stood in pads the difference with blank rows at the
//! region's top, so that the box stays at the foot instead of climbing to
//! where the region ends and leaving the rows it stood in blank under it. The
//! height kept is never more than the window's, since a window made shorter
//! cannot have kept it all. A frame that closes the region — a resume, a
//! clear, or leaving — writes no region and keeps no height, so the next one,
//! where there is one, starts again from only what it has to show.
//!
//! A resize is answered twice. At once, the region is drawn again and nothing
//! else. Once the window has kept its new size for [`SETTLE`], the screen and
//! the terminal's scrollback are cleared and everything kept is written again
//! at the new width, opening with one quiet row where the ceiling has dropped
//! the start of the session, and the region under it. What the terminal had
//! kept was cut at the old width, and no two terminals put it at a new one the
//! same way: some fold it again, some leave it as it was cut, and some count
//! a symbol's width differently. Writing it again is the one answer that reads
//! the same on all of them. A terminal that ignores the request to clear its
//! scrollback keeps the old rows above the new ones.
//!
//! A frame asks the window's size before it is drawn, so that one drawn while
//! an answer is arriving goes out at the width the window already has rather
//! than the one the press reporting the change will name. Given a
//! [`ResizeFlag`], only the first frame after the flag is raised asks, and the
//! one after it where that query failed; without one, every frame does. How
//! far back the region's top now is cannot be asked of the terminal, so it is
//! worked out from how wide each row of the region was against the new width,
//! counted as a terminal that rewraps would count it. On one that does not,
//! narrowing counts high, and the erase that opens the next frame takes
//! finished rows just above the region off the visible screen until the window
//! settles and they are written again. Counting low instead would leave a
//! stale copy of the region in the scrollback on every terminal that does
//! rewrap, which is most of them.
//!
//! The rewind also trusts that a row crucible counted as fitting the window
//! takes one row of it. A terminal that draws a symbol wider than crucible
//! counts it, as some draw one that has a picture form, would carry the end
//! of such a row onto the next, and every rewind after it would fall a row
//! short and leave a copy of the region behind. So autowrap is off from the
//! first frame until the region is closed for good: a row the terminal draws
//! too wide loses its last cells instead. Closing the region is what turns
//! it on again, and that happens from `Drop` too, as the rest of closing it
//! does, and not at all where output is redirected, since nothing is drawn
//! there to begin with.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use super::Renderer;
use super::frame::{BEGIN_SYNC, END_SYNC, HIDE, SHOW};
use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::terminal::system::ResizeFlag;
use crate::terminal::{Size, Terminal, TerminalError};
use crate::width;

/// Erases from the cursor to the end of the screen.
const ERASE_BELOW: &str = "\x1b[J";

/// Turns autowrap off, from the first frame on.
const WRAP_OFF: &str = "\x1b[?7l";

/// Turns it back on, once the region is closed for good.
const WRAP_ON: &str = "\x1b[?7h";

/// Clears the screen and the terminal's scrollback, leaving the cursor at the
/// top: what a resize clears before what was kept is written again.
///
/// Colour goes back to the default first, so the cleared rows are not filled
/// in whatever the last row written was drawn in.
const CLEARS: &str = "\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H";

/// How long the window has to keep one size before what was kept is written
/// again at it.
///
/// A window dragged to a new size reports every size it passes through, and
/// writing the whole session again for each would be most of what the
/// terminal did while the drag lasted.
pub(super) const SETTLE: Duration = Duration::from_millis(75);

/// The quiet row a replay opens with when the ceiling has dropped the start
/// of the session.
const EARLIER: &str = "Earlier output is in the session log.";

/// What a native frame keeps between one frame and the next.
#[derive(Debug, Default)]
pub(super) struct Native {
    /// One past the last line the session has sealed: lines before it go out
    /// to the scrollback at the next frame.
    sealed: usize,
    /// The display width of each row of the region last drawn, top first; a
    /// blank row padding its top is a width of nothing.
    widths: Vec<usize>,
    /// The row of the region the cursor was left on, counted from its top.
    parked: usize,
    /// The column the cursor was left on.
    column: usize,
    /// How far back the region's top is, where something other than the last
    /// frame decided it — a resize.
    rewind: Option<usize>,
    /// What the region said last time, park included, so a frame that would
    /// say the same is not written.
    shown: String,
    /// Reused for each frame's bytes, except a replay's, which are let go of
    /// once the write has been tried, whether or not the terminal took them.
    frame: String,
    /// Whether a frame has been written, so there is a region to close.
    drawn: bool,
    /// Whether the region has been closed for good.
    left: bool,
    /// When the window will have kept its size long enough to be given what
    /// was kept, after it changed.
    settles: Option<Instant>,
    /// Whether the next frame clears the screen and its scrollback and writes
    /// what was kept before the region.
    replays: bool,
    /// Whether the next frame asks the window its size although the flag was
    /// not raised again.
    asks: Asks,
}

/// When a frame given a [`ResizeFlag`] asks the window its size.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Asks {
    /// Only when the flag has been raised since a frame last took it.
    #[default]
    WhenTold,
    /// At the next frame as well: the frame that took the flag could not read
    /// the size, so the resize it told of is still owed.
    Again,
}

impl Native {
    /// Works out how far back the region's top is once the window is `size`.
    fn rewound(&mut self, size: Size) {
        let columns = size.columns.max(1);
        let above: usize = self
            .widths
            .iter()
            .take(self.parked)
            .map(|width| width.div_ceil(columns).max(1))
            .sum();
        let back = above + self.column / columns;
        self.rewind = Some(back.min(size.rows.saturating_sub(1)));
    }
}

/// What a frame writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writes {
    /// What has been sealed, then the live region.
    Live,
    /// Everything the record holds, the line being written included, and no
    /// region after it: the frame that closes one.
    Everything,
}

impl<T: Terminal> Renderer<T> {
    /// Lets every line finished so far go to the scrollback, and draws.
    ///
    /// Called wherever the session waits for a key: nothing still to be
    /// edited is finished by then, and nothing the reader is about to read is
    /// left only in the region.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub(super) fn seal(&mut self) -> Result<(), TerminalError> {
        let finished = self.record.finished();
        let Some(native) = &mut self.native else {
            return Ok(());
        };
        if native.sealed >= finished {
            return Ok(());
        }
        native.sealed = finished;
        self.draw()
    }

    /// One native frame.
    pub(super) fn draw_native(&mut self) -> Result<(), TerminalError> {
        self.framed(Writes::Live)
    }

    /// Notes that the window changed size, before it is laid out again, and
    /// puts off giving it what was kept until it has settled.
    pub(super) fn native_resize(&mut self, size: Size) {
        if let Some(native) = &mut self.native {
            native.rewound(size);
            if native.drawn && !native.left {
                native.settles = Some(Instant::now() + SETTLE);
            }
        }
    }

    /// How long until a window that changed size has settled, where one has.
    pub(super) fn settles_in(&self, now: Instant) -> Option<Duration> {
        let due = self.native.as_ref()?.settles?;
        Some(due.saturating_duration_since(now))
    }

    /// Gives a window that has settled at a new size everything kept, written
    /// again at that size, where it is due by `now`.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub(super) fn settled(&mut self, now: Instant) -> Result<(), TerminalError> {
        let Some(native) = &mut self.native else {
            return Ok(());
        };
        if native.settles.is_none_or(|due| now < due) {
            return Ok(());
        }
        native.settles = None;
        native.replays = true;
        self.draw()
    }

    /// Writes out everything the record holds, leaving no region behind.
    ///
    /// What emptying the transcript means here: the scrollback is the
    /// reader's terminal's, and nothing this process writes can take back
    /// what is in it, so what was said stays said above what replaces it.
    pub(super) fn native_empties(&mut self) -> Result<(), TerminalError> {
        if self.native.is_none() || !self.terminal.is_terminal() {
            return Ok(());
        }
        self.record.end();
        self.framed(Writes::Everything)
    }

    /// Marks where the transcript just emptied gives way to what replaces it.
    ///
    /// What a session resumed or cleared is given here in place of the opening
    /// card the full screen draws again: the session above stays in the
    /// scrollback, and a second card under it would read as a second launch.
    /// One blank row parts the divider from what is above it, and the divider
    /// parts what follows, which asks for no blank row of its own: neither
    /// [`Renderer::apart`] nor an empty [`Renderer::commit`] puts one under it.
    ///
    /// The divider is marked as parting before the frame that draws it, not
    /// after: a window with no transcript row lets every line go in the frame
    /// that first draws it, and a mark hung after that frame would find the
    /// divider gone and mark nothing.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn divides(&mut self, label: &str) -> Result<(), TerminalError> {
        self.apart()?;
        let divider = divider(label, self.transcript_columns(), self.glyphs);
        if !self.terminal.is_terminal() {
            // Nothing is framed where output is redirected, so nothing goes
            // out before it is marked, and the plain copy a redirected run is
            // owed is [`Renderer::present`]'s to write.
            self.present(&[divider])?;
            self.record.parts();
            return Ok(());
        }
        self.record.end();
        self.record.lay([divider]);
        self.record.parts();
        self.draw()
    }

    /// Closes the region for good: everything held is written out, nothing
    /// that stood is left, the cursor is shown at the start of a row of its
    /// own, where the shell will write next, and autowrap is on again.
    ///
    /// Nothing where output is redirected, in the full screen, where nothing
    /// was drawn, and the second time.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub(super) fn leave(&mut self) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return Ok(());
        }
        match &self.native {
            Some(native) if native.drawn && !native.left => {}
            _ => return Ok(()),
        }
        self.record.end();
        let written = self.framed(Writes::Everything);
        if let Some(native) = &mut self.native {
            native.left = true;
        }
        // Put back even when the frame before it failed: the shell's lines
        // would otherwise stop at the edge of the window.
        let restored = self
            .terminal
            .write(WRAP_ON)
            .and_then(|()| self.terminal.flush());
        written.and(restored)
    }

    /// Closes the region for good, before the renderer itself goes.
    ///
    /// For a session ending while it still holds the terminal's modes: what
    /// those write on their way out, and anything held back to be said then,
    /// lands on the clean row this leaves rather than inside a region that
    /// dropping the renderer would rewind into. Nothing in the full screen,
    /// and nothing is drawn after it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn closes(&mut self) -> Result<(), TerminalError> {
        self.leave()
    }

    /// Writes one frame, taking the state it keeps out of `self` while it
    /// does.
    ///
    /// The window's size is asked for first, where it may have changed. The
    /// press that reports a resize is read between frames, and an answer
    /// still arriving draws frames until it is: a frame drawn at the old
    /// width is wrapped by the terminal, the next rewinds over the rows it
    /// counted rather than the rows the terminal made of them, and what it
    /// did not reach stays above the region as a second copy. So a size the
    /// press has not yet reported is taken here, and the frame is drawn for
    /// the window as it is now; the press, when it comes, finds nothing left
    /// to do.
    ///
    /// Given a [`ResizeFlag`], a frame asks only when the flag says the
    /// window changed since a frame last asked, so a streaming answer costs
    /// no query per frame. Without one, as where the operating system sends
    /// no word of a resize, every frame asks.
    ///
    /// A query that fails says nothing about the window. It is not a resize,
    /// and the frame is drawn for the size already known, as it would have
    /// been before the query was asked here. Where the flag sent it to ask,
    /// the resize it told of is still owed, so the next frame asks once more
    /// though the flag is down. Each signal is owed that one query: when one
    /// asked only for what was owed fails as well, the next signal or press is
    /// waited for rather than asking at every frame while the window will not
    /// say.
    fn framed(&mut self, writes: Writes) -> Result<(), TerminalError> {
        let (asks, owed, told) = match &mut self.native {
            None => (false, false, false),
            Some(native) => {
                let owed = std::mem::take(&mut native.asks) == Asks::Again;
                let told = self.resizes.as_ref().is_some_and(ResizeFlag::taken);
                (self.resizes.is_none() || told || owed, owed, told)
            }
        };
        let size = asks.then(|| self.terminal.size().ok()).flatten();
        if asks
            && size.is_none()
            && self.resizes.is_some()
            && let Some(native) = &mut self.native
        {
            native.asks = if owed && !told {
                Asks::WhenTold
            } else {
                Asks::Again
            };
        }
        if size.is_some_and(|size| size != self.size) {
            // `resized` asks the size again and takes what it reads, as it
            // does for the press: it lays the region out and draws it,
            // through this function again, when that differs from the size
            // known, and does nothing when it does not. A live frame it drew
            // is whole, so nothing follows it; one it did not draw is drawn
            // below for the size known. A frame that closes the region goes
            // out either way.
            let known = self.size;
            self.resized()?;
            if writes == Writes::Live && self.size != known {
                return Ok(());
            }
        }
        let Some(mut native) = self.native.take() else {
            return Ok(());
        };
        let written = self.frame_into(&mut native, writes);
        self.native = Some(native);
        written
    }

    fn frame_into(&mut self, native: &mut Native, writes: Writes) -> Result<(), TerminalError> {
        if native.left {
            return Ok(());
        }

        let columns = self.size.columns.max(1);
        let bands = self.bands();
        let room = bands.transcript.len();
        let first = self.record.first();
        let finished = self.record.finished();

        // What goes out: what is sealed, and then whatever the region could
        // not show — a line that has scrolled off the top of the region would
        // otherwise be a line nobody ever sees.
        let through = match writes {
            Writes::Everything => self.record.lines(),
            Writes::Live => {
                let mut through = native.sealed.min(finished).max(first);
                while through < finished && self.record.rows_from(through) > room {
                    through += 1;
                }
                through
            }
        };

        let mut out = std::mem::take(&mut native.frame);
        out.clear();
        out.push_str(BEGIN_SYNC);
        out.push_str(HIDE);
        if !native.drawn {
            out.push_str(WRAP_OFF);
        }
        let replays = std::mem::take(&mut native.replays) && writes == Writes::Live;
        let rewound = native.rewind.take();
        if replays {
            // The screen and the scrollback are cleared, so nothing of the last
            // region is left to rewind over or to keep the height of.
            //
            // Every row kept is painted into this one string before any of it
            // is written, so for as long as this frame lasts the session is
            // held twice: in the record, and here as the bytes that write it,
            // both bounded by the record's ceiling.
            out.push_str(CLEARS);
            if self.record.spilled() {
                Row::new()
                    .then(Slot::Quiet, EARLIER)
                    .clipped(columns)
                    .paint_into(&self.palette, &mut out);
                out.push_str("\r\n");
            }
            self.record.kept(self.glyphs, |row| {
                row.clipped(columns).paint_into(&self.palette, &mut out);
                out.push_str("\r\n");
            });
            native.widths.clear();
            native.parked = 0;
            native.column = 0;
        }
        let up = if replays {
            0
        } else {
            rewound.unwrap_or(native.parked)
        };
        out.push('\r');
        if up > 0 {
            let _ = write!(out, "\x1b[{up}A");
        }
        out.push_str(ERASE_BELOW);

        self.record.hangs_through(through);
        let mut emitted = 0;
        for line in first..through {
            for row in self.record.folded(line) {
                row.clipped(columns).paint_into(&self.palette, &mut out);
                out.push_str("\r\n");
                emitted += 1;
            }
        }
        self.record.lets_go(through);

        // The rows the last region stood in, counted as the rewind counts
        // them, and never more than the window: a window made shorter since
        // cannot have kept them all.
        let stood = native
            .widths
            .iter()
            .map(|width| width.div_ceil(columns).max(1))
            .sum::<usize>()
            .min(self.size.rows);
        let region = out.len();
        native.widths.clear();
        let mut parked = 0;
        let mut column = 0;
        if writes == Writes::Live {
            let showing = self.record.view(room);
            let shown = showing.len();
            let turn = self.standing.turn.iter().take(bands.turn.len());
            let turned = turn.len();
            let prompt = self.standing.prompt.iter().take(bands.prompt.len());
            let prompted = prompt.len();

            // What this frame writes falls short of the rows the last one
            // stood in: the rest is blank rows at the top of the region, so
            // that what stands at the foot stays there. Rows the region grew
            // over went into the scrollback, and giving the height back would
            // not bring them back.
            let pad = stood.saturating_sub(emitted + shown + turned + prompted);
            for _ in 0..pad {
                if !native.widths.is_empty() {
                    out.push_str("\r\n");
                }
                native.widths.push(0);
            }
            for row in showing {
                if !native.widths.is_empty() {
                    out.push_str("\r\n");
                }
                let row = if row.columns() > columns {
                    row.clipped(columns)
                } else {
                    row
                };
                native.widths.push(row.columns());
                row.paint_into(&self.palette, &mut out);
            }
            for painted in turn.chain(prompt) {
                if !native.widths.is_empty() {
                    out.push_str("\r\n");
                }
                native.widths.push(width::columns(painted));
                out.push_str(painted);
            }

            let rows = native.widths.len();
            if rows > 0 {
                let (row, at) = self
                    .standing
                    .prompted
                    .filter(|_| prompted > 0)
                    .map(|caret| (pad + shown + turned + caret.row, caret.column))
                    .or_else(|| {
                        self.standing
                            .turned
                            .filter(|_| turned > 0)
                            .map(|caret| (pad + shown + caret.row, caret.column))
                    })
                    .unwrap_or((pad + shown + turned, 0));
                parked = row.min(rows - 1);
                column = at.min(columns - 1);
                let back = rows - 1 - parked;
                if back > 0 {
                    let _ = write!(out, "\x1b[{back}A");
                }
                let _ = write!(out, "\x1b[{}G", column + 1);
            }
        }
        let live = out.len();

        // A frame that would leave the screen as it is costs nothing: a turn
        // is a great many frames in which only the clock moved.
        let unchanged = writes == Writes::Live
            && emitted == 0
            && rewound.is_none()
            && !replays
            && out.get(region..live) == Some(native.shown.as_str());
        if unchanged {
            native.frame = out;
            return Ok(());
        }

        out.push_str(SHOW);
        out.push_str(END_SYNC);
        let written = self
            .terminal
            .write(&out)
            .and_then(|()| self.terminal.flush());

        // A replay the terminal refused is still owed: nothing says how much
        // of it arrived, so the next frame clears and gives it all again.
        native.replays = replays && written.is_err();
        native.shown.clear();
        native
            .shown
            .push_str(out.get(region..live).unwrap_or_default());
        native.parked = parked;
        native.column = column;
        native.drawn = true;
        // A replay's string holds every row kept, painted, and it goes here
        // rather than being kept for reuse: kept, it would hold the session
        // twice for as long as the session lasts instead of for one write.
        native.frame = if replays { String::new() } else { out };
        written
    }
}

/// One row in the quiet colour: two rule cells, the label between spaces, and
/// rule cells to the last of `columns`.
///
/// The rule cell is the one the compaction record is ruled in. A label that
/// would leave fewer than two rule cells after it is clipped, with the glyph
/// set's ellipsis, and a width with no room for any of it is ruled across.
fn divider(label: &str, columns: usize, glyphs: Glyphs) -> Row {
    let rule = glyphs.horizontal();
    // Two rule cells and a space either side of the label.
    let room = columns.saturating_sub(6);
    let label = if width::columns(label) <= room {
        label.to_owned()
    } else {
        let ellipsis = glyphs.ellipsis();
        let kept = width::clip(label, room.saturating_sub(width::columns(ellipsis)));
        format!("{kept}{ellipsis}")
    };
    let wide = width::columns(&label);
    if room == 0 || wide > room {
        return Row::new().then(Slot::Quiet, rule.repeat(columns));
    }
    let after = rule.repeat(columns - 4 - wide);
    Row::new().then(Slot::Quiet, format!("{rule}{rule} {label} {after}"))
}

/// The region closed on the way out, unwinding included, so a session that
/// ends any way at all leaves the reader's shell on a clean row with its
/// cursor showing.
impl<T: Terminal> Drop for Renderer<T> {
    fn drop(&mut self) {
        // Nowhere to report it: the terminal that refused is the one a report
        // would be written to.
        let _ = self.leave();
    }
}

#[cfg(test)]
mod tests;
