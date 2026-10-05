//! Standing what the transcript had to cut down to a row.
//!
//! Two ways in and one picture. Ctrl+O names no result, so it stands every one
//! of them, newest first; a click names one by landing on the row that made the
//! offer, so it stands that one. Everything after that is the same — the same
//! rows, the same arrows through them, the same keys out.
//!
//! It stands rather than being written into the transcript for the reason
//! nothing else standing here is written down: the results are already in the
//! record as the rows that could not fit them, and committing the text a second
//! time would be a session saying everything twice. So the way out of it is the
//! way out of everything else here — what it stood in is given back, and the
//! screen reads afterwards as though nothing had been opened.
//!
//! Which is also what makes Ctrl+O a toggle rather than a door. It is the key
//! the rows themselves name, and pressing it against what it opened closes it
//! again; there is nothing to undo afterwards because there was never anything
//! to undo.
//!
//! Where it stands depends on whether a turn is running, and on nothing else.
//! Between turns it takes the region the prompt box was in and reads keys of
//! its own until it is closed. While a turn runs it stands under the tail
//! instead, in the rows the box has: results go on arriving above it, every
//! frame draws it again underneath them, and the turn never reaches down into
//! it. The keys are the same either way and so is the picture, which is the
//! point — a reader pressing Ctrl+O is not asked to know which of the two they
//! are in, and a view opened under a turn is still open when the turn ends.
//!
//! What it stands over is what had been cut when it opened. A turn writing
//! underneath it goes on cutting results, and letting those in would slide the
//! rows being read down the screen as each one arrived; they are there the next
//! time it is opened, which is one press away.
//!
//! A result the store let go of is still stood over, where the session has a
//! log to read it back from, and it is read back when the window reaches it
//! rather than when the view opens. What the view holds of those is what its
//! window reaches, up to one result's worth, so standing over a long session
//! costs what reading one result costs. A result the log cannot give back
//! stands as a line saying so, and the others still open.

use crucible_tui::{Caret, Expanded, Glyphs, Key, Pressed, Renderer, Row, Shown, Terminal};
use crucible_types::TOOL_RESULT_BYTES;

use crate::cli::Fatal;
use crate::cli::kept::{Back, Kept, Mark, Placed, Whole};
use crate::cli::style::Style;

use super::region::{self, Moved};
use super::typing::Asked;

/// What the view is standing over.
///
/// Which of the two it is depends on what asked for it, and on nothing after
/// that: the key names no result, so it stands them all, and a click names one
/// by landing on the row that offered it. Both are the same picture with a
/// different number of results in it, and both are closed by the same keys.
#[derive(Debug, PartialEq, Eq)]
enum Over {
    /// Everything that had been cut when it opened, newest first.
    ///
    /// A count of them rather than the results themselves, so that nothing
    /// standing here borrows what a running turn is writing into.
    Everything(usize),
    /// The one result whose offer was written on this row of the record.
    ///
    /// The row rather than the result, for the same reason. It is also what
    /// makes the view close on its own when the ceiling drops that result:
    /// there is then no row to find, and nothing to stand.
    One(usize),
}

/// Where the window over what is standing is open.
///
/// `end` is the layout's answer rather than the keyboard's — how far down the
/// window may go depends on how many rows the results came to at this width,
/// which is not known until they are laid out. So the frame that discovers it
/// writes it here, and the next key acts on a number the picture agrees with.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct View {
    /// How far down the whole of it the window is open.
    from: usize,
    /// The furthest down it may go, as of the last frame drawn.
    end: usize,
    /// The furthest down the window opens on rows, as of the last frame
    /// drawn: less than `end` where the window may go on only to reach a
    /// result not read back yet. Asked for further down than this, the frame
    /// is drawn from here, so this is where the result at its top is counted
    /// from, by the footer and by a step alike.
    laid: usize,
    /// Where the window was open when the last frame was drawn, which is how
    /// a frame knows which way the window moved since.
    was: usize,
    /// How many rows of results the window showed, as of the last frame
    /// drawn: what a page is, less the one row a page keeps in sight.
    page: usize,
    /// Where each result begins, counting the blank that parts it from the
    /// one above, as of the last frame drawn: where a step to the next or
    /// the last result puts the top of the window.
    starts: Vec<usize>,
    /// What it is a window over.
    over: Over,
    /// What the window reaches of the results the store let go of, read back
    /// from the session log: which result, and what it said, or `None` where
    /// the log could not give it back. No more than [`BEYOND`] of it, and held
    /// only while the view stands.
    back: Vec<(Mark, Option<Box<str>>)>,
    /// A result the window reached that did not fit beside what was read
    /// back, and where the window was: not read again until the window moves,
    /// or until nothing above it in the window has been read back.
    refused: Option<(usize, Mark)>,
}

impl View {
    /// A window at the top of `over`, before a frame has said how far it goes.
    fn onto(over: Over) -> Self {
        Self {
            from: 0,
            end: 0,
            laid: 0,
            was: 0,
            page: 0,
            starts: Vec::new(),
            over,
            back: Vec::new(),
            refused: None,
        }
    }
}

/// What stands in place of a result the log could not give back.
const UNREAD: &str = "! this result could not be read back from the session log";

/// What stands in place of a let-go result the window reaches but has not
/// read back: one whose batch the turn has not written yet, read on a frame
/// after it has, and one that would take what is read back above it past
/// [`BEYOND`], read once that has left the window.
const LATER: &str = "read back from the session log as the view moves on to it";

/// The most the view reads back at once beyond what the store holds, in
/// bytes: one result's worth, the most a recorded result can be. A result
/// with nothing read back above it is read whatever it comes to.
const BEYOND: usize = TOOL_RESULT_BYTES;

/// Whether the whole of what was cut is standing, and where over it.
///
/// Held by the session rather than by either of the loops that draw it, for one
/// reason: a view opened while a turn ran is still open when that turn ends,
/// and the reader who opened it is still reading. So it outlives the loop that
/// took the key, and the loop that comes next picks the same window up where
/// this one left it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Standing {
    /// Nothing is standing.
    #[default]
    Closed,
    /// The view is standing, with the window this far down it.
    Open(View),
}

impl Standing {
    /// Whether the view is standing.
    pub(super) fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }

    /// Opens the view over everything cut so far.
    ///
    /// Nothing opens where nothing was cut. The key is offered by the rows that
    /// were cut, so a session with none of them has made no offer, and a frame
    /// put up in answer to a press nobody meant is one that took the prompt
    /// away for no reason.
    /// Whether this is what the box was answered with, opening it if it is.
    ///
    /// Asked before the answer is read as a line, because the two keys that reach
    /// this are answered by the state that holds what they stand over rather than
    /// by the loop that read them.
    pub(super) fn asked(&mut self, asked: &Asked, kept: &Kept) -> bool {
        match asked {
            Asked::Expand => self.open(kept),
            Asked::Clicked(at) => self.one(kept, *at),

            // Not this one's. A line, a turn nobody typed and the end of a
            // session are the loop's.
            Asked::Said(_) | Asked::Woke(_) | Asked::Ended | Asked::Untyped | Asked::Queue => {
                return false;
            }
        }

        true
    }

    pub(super) fn open(&mut self, kept: &Kept) {
        if kept.is_empty() {
            return;
        }

        *self = Self::Open(View::onto(Over::Everything(kept.cut())));
    }

    /// Opens the view over the one result the record row `at` offered.
    ///
    /// Nothing opens where that row offered nothing, which is most rows: a
    /// pointer lands wherever it lands, and the answer to a click on a line of
    /// an answer, a blank row or the shell's own output is the screen the
    /// reader was already looking at.
    ///
    /// What the row offered and the store let go of is read back from the
    /// session log as the view is drawn, as either way in does.
    pub(super) fn one(&mut self, kept: &Kept, at: usize) {
        if !kept.offered(at) {
            return;
        }

        *self = Self::Open(View::onto(Over::One(at)));
    }

    /// Gives one key to the view, and answers whether a frame is owed.
    ///
    /// That it closed is read off [`Standing::is_open`] afterwards rather than
    /// reported here: the caller draws something either way, and which of the
    /// two it draws is a question about the state and not about the key.
    pub(super) fn against(&mut self, arrived: Pressed, wheel: usize) -> bool {
        let Self::Open(view) = self else {
            return false;
        };

        match region::wheeled(arrived, wheel, |key| moving(key, view)) {
            Moved::Redraw => true,
            Moved::Still => false,

            // Nothing here is ever taken, so the two ways out are one way out.
            Moved::Took | Moved::Left => {
                *self = Self::Closed;
                true
            }
        }
    }
}

/// Stands the view where the box was, and reads keys until it is closed.
///
/// Between turns, which is the half of this that has the keyboard to itself:
/// nothing else is reading, so the loop can block on a press. Returns as soon
/// as it is called if nothing is open.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    kept: &Kept,
    standing: &mut Standing,
) -> Result<(), Fatal> {
    let Standing::Open(view) = standing else {
        return Ok(());
    };

    let glyphs = style.glyphs();
    let picture = |view: &mut View, columns: usize, rows: usize| {
        (laying(kept, view, glyphs, columns, rows), None)
    };

    // Nothing is taken here and nothing is committed, so every way out is the
    // same way out: the region goes back and the box comes up under it. A
    // window with no room to stand it in is one of them — a component that
    // never drew and read no key, which here is a view the reader never saw.
    region::stand(renderer, |_| style, view, picture, moving)?;
    *standing = Standing::Closed;
    Ok(())
}

/// Stands the view under the tail, in the rows the box has while a turn runs.
///
/// Answers whether it stood, which is the caller's question rather than this
/// one's: the box and the view take the same rows, so exactly one of them is
/// drawn per frame and the caller draws the other. A window with no room for
/// the view closes it here rather than standing its chrome with no text under
/// it — the same answer the region gives between turns.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn under<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    kept: &Kept,
    standing: &mut Standing,
) -> Result<bool, Fatal> {
    let Standing::Open(view) = standing else {
        return Ok(false);
    };

    // One row is left to the transcript whatever stands here, and it is the row
    // the turn goes on writing into: a view that asked for the whole window
    // would be a reader looking back through what was said while what is being
    // said now has nowhere at all to appear. How much what stands here may have
    // is the renderer's to say, as it is for the view between turns.
    let room = renderer.room().saturating_sub(1);
    let rows = laying(kept, view, style.glyphs(), renderer.columns(), room);

    let Some(row) = rows.len().checked_sub(1) else {
        *standing = Standing::Closed;
        return Ok(false);
    };

    // On the view's last row rather than up in the tail, which is where the box
    // parks it too: the cursor sits at the bottom of whatever is standing, and
    // a cursor left in the transcript above reads as a place text is about to
    // appear.
    renderer.under(&rows, Some(Caret { row, column: 0 }), style.palette())?;
    Ok(true)
}

/// The rows of the view at this size, and the state the picture agrees with.
///
/// No rows where there is nothing left to stand, which both callers read as the
/// view closing. That is a result the ceiling dropped while somebody was
/// reading it — rare, and the honest answer to it is the screen coming back
/// rather than a frame of chrome with nothing under it.
fn laying(kept: &Kept, view: &mut View, glyphs: Glyphs, columns: usize, rows: usize) -> Vec<Row> {
    let entries = entries(kept, &view.over);
    if entries.is_empty() {
        return Vec::new();
    }
    let heights = reaching(kept, view, &entries, columns, rows)
        .unwrap_or_else(|| heights(&entries, &view.back, columns));

    let View {
        from,
        end,
        laid,
        was,
        page,
        starts: begun,
        back,
        ..
    } = view;
    let shown: Vec<Shown<'_>> = entries.iter().map(|entry| shown(*entry, back)).collect();
    let expanded = Expanded {
        shown: &shown,
        from: *from,
    };

    // Written before the rows are asked for, so the key pressed against this
    // picture is clamped to what this picture could reach, pages by what it
    // showed and steps to where its results begin. A result not read back yet
    // is a few rows until it is: the last of them could sit under the end of
    // the window and never be reached, so the window may go down as far as its
    // top, which is where the view reads it.
    let total: usize = heights.iter().sum();
    *laid = total.saturating_sub(Expanded::seen(rows));
    *end = (*laid).max(unread(&entries, back, &heights));
    *from = (*from).min(*end);
    *was = *from;
    *page = Expanded::seen(rows);
    *begun = starts(&heights);
    // Where the last of them ends, which no step goes to.
    begun.pop();

    expanded.within(columns, rows, glyphs)
}

/// One result the view stands over: held by the store, or let go of and read
/// back when the window reaches it.
#[derive(Debug, Clone, Copy)]
enum Entry<'a> {
    Held(&'a Whole),
    Let(&'a Placed),
}

/// What the view stands over, in the order it is read.
fn entries<'a>(kept: &'a Kept, over: &Over) -> Vec<Entry<'a>> {
    // Newest first by the order the rows were drawn in, held or let go of: a
    // short result can stay held after a longer one drawn later was let go of,
    // so neither list alone says which came first.
    let cut = |at: Option<usize>| {
        let mut cut: Vec<Entry<'a>> = kept
            .newest()
            .filter(|whole| at.is_none_or(|at| whole.at() == Some(at)))
            .map(Entry::Held)
            .chain(
                kept.older()
                    .filter(|placed| at.is_none_or(|at| placed.at() == at))
                    .map(Entry::Let),
            )
            .collect();
        cut.sort_by_key(|entry| std::cmp::Reverse(entry.drawn()));
        cut
    };

    match *over {
        // What had been cut when it opened, and nothing cut since: what
        // arrives underneath a view standing over a turn would slide the rows
        // being read down the screen.
        // The call still out first, where there is one, because it is the newest
        // thing there is and it is what a reader pressing this while a command
        // runs is asking about. Kept apart from what was cut: a call that has
        // not answered has not been counted among them.
        Over::Everything(opened) => kept
            .writing()
            .map(Entry::Held)
            .chain(cut(None).into_iter().filter(|entry| entry.drawn() < opened))
            .collect(),

        // Every result that row offered, and usually that is one. A row
        // counting a folded run is the exception and the reason this filters
        // rather than finds: several calls were shown as one line, so the line
        // opens on all of them — a reader who was given a count is owed
        // everything it counted.
        Over::One(at) => cut(Some(at)),
    }
}

impl Entry<'_> {
    /// The order its row was drawn in.
    fn drawn(&self) -> usize {
        match self {
            Self::Held(whole) => whole.drawn(),
            Self::Let(placed) => placed.drawn(),
        }
    }
}

/// One result as the view shows it, with what has been read back of the ones
/// let go of.
fn shown<'a>(entry: Entry<'a>, back: &'a [(Mark, Option<Box<str>>)]) -> Shown<'a> {
    match entry {
        Entry::Held(whole) => showing(whole),
        Entry::Let(placed) => Shown {
            called: placed.called(),
            text: match back.iter().find(|(mark, _)| placed.is(mark)) {
                Some((_, Some(text))) => text,
                Some((_, None)) => UNREAD,
                None => LATER,
            },
        },
    }
}

/// Reads back what the window reaches of the results the store let go of,
/// and lets go of what it has left, keeping the rows the reader sees where
/// they were. Hands back how many rows each result comes to once it has,
/// where any was let go of.
///
/// A step never passes a result the view has not read back: one down stops
/// at its top, and one up at its end, so that walking the view shows every
/// result whatever the step. What the window reaches then depends on where
/// each result begins at this width, which depends on what has been read
/// back; so it is worked out, the reading changed, and worked out again,
/// twice at most.
///
/// The window then moves with the result it opens on, so what the reader is
/// looking at stays on the same rows of the screen: a result read back keeps
/// its heading and grows below it, one let go of shrinks, and results above
/// the window changing length move it by as much. Stepping down onto a result
/// shows it from its top; stepping up onto one shows it from its end.
fn reaching(
    kept: &Kept,
    view: &mut View,
    entries: &[Entry<'_>],
    columns: usize,
    rows: usize,
) -> Option<Vec<usize>> {
    if !entries.iter().any(|entry| matches!(entry, Entry::Let(_))) {
        view.back.clear();
        return None;
    }
    let seen = Expanded::seen(rows);
    let down = view.from >= view.was;
    let mut heights = heights(entries, &view.back, columns);
    view.from = stopped(view, entries, &heights, down);

    for _ in 0..2 {
        let window = view.from..view.from.saturating_add(seen);
        let before = starts(&heights);
        let within = |span: &[usize]| matches!(span, [start, end] if *start < window.end && *end > window.start);
        let reached: Vec<&Placed> = entries
            .iter()
            .zip(before.windows(2))
            .filter(|(_, span)| within(span))
            .filter_map(|(entry, _)| match entry {
                Entry::Let(placed) => Some(*placed),
                Entry::Held(_) => None,
            })
            .collect();

        let had: Vec<Mark> = view.back.iter().map(|(mark, _)| mark.clone()).collect();
        let back = reading(kept, view, &reached);
        let has: Vec<Mark> = back.iter().map(|(mark, _)| mark.clone()).collect();
        view.back = back;
        if has == had {
            break;
        }
        remeasured(&mut heights, entries, &view.back, columns);

        // The result the window opens on, or, stepping up onto one just read
        // back, the one under it: that one's rows are the ones staying put.
        let top = before.windows(2).position(within);
        let grew = |at: usize| {
            matches!(entries.get(at), Some(Entry::Let(placed))
                if !had.iter().any(|mark| placed.is(mark)) && has.iter().any(|mark| placed.is(mark)))
        };
        let anchor = top.map(|at| if !down && grew(at) { at + 1 } else { at });

        if let Some(anchor) = anchor {
            let now = starts(&heights);
            if let (Some(was), Some(now)) = (before.get(anchor), now.get(anchor)) {
                view.from = view.from.saturating_add(*now).saturating_sub(*was);
            }
        }
    }
    Some(heights)
}

/// Where the window stops on its way from where it was to where it was
/// asked to go: at the top of the first result it would pass going down
/// that it has not read back, or at the end of the first going up.
fn stopped(view: &View, entries: &[Entry<'_>], heights: &[usize], down: bool) -> usize {
    let (near, far) = if down {
        (view.was, view.from)
    } else {
        (view.from, view.was)
    };
    let unread = |entry: &Entry<'_>| matches!(entry, Entry::Let(placed) if !view.back.iter().any(|(mark, _)| placed.is(mark)));
    let starts = starts(heights);
    let passed = entries
        .iter()
        .zip(starts.windows(2))
        .filter(|(entry, _)| unread(entry))
        .filter_map(|(_, span)| match span {
            [start, end] => Some((*start, *end)),
            _ => None,
        });

    let stop = if down {
        passed
            .map(|(start, _)| start)
            .find(|start| *start > near && *start < far)
    } else {
        passed
            .map(|(_, end)| end.saturating_sub(1))
            .filter(|last| *last > near && *last < far)
            .max()
    };
    stop.unwrap_or(view.from)
}

/// Where the last result the view has not read back begins, or nothing where
/// it has read back every one it stands over.
fn unread(entries: &[Entry<'_>], back: &[(Mark, Option<Box<str>>)], heights: &[usize]) -> usize {
    entries
        .iter()
        .zip(starts(heights))
        .filter(|(entry, _)| {
            matches!(entry, Entry::Let(placed) if !back.iter().any(|(mark, _)| placed.is(mark)))
        })
        .map(|(_, start)| start)
        .next_back()
        .unwrap_or(0)
}

/// How many rows each of `entries` comes to at this width, counting the blank
/// that parts it from the one above.
fn heights(entries: &[Entry<'_>], back: &[(Mark, Option<Box<str>>)], columns: usize) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .map(|(at, entry)| measured(*entry, at, back, columns))
        .collect()
}

/// Counts again the rows of the results let go of, which are the only ones
/// what was read back changes.
fn remeasured(
    heights: &mut [usize],
    entries: &[Entry<'_>],
    back: &[(Mark, Option<Box<str>>)],
    columns: usize,
) {
    for (at, (height, entry)) in heights.iter_mut().zip(entries).enumerate() {
        if matches!(entry, Entry::Let(_)) {
            *height = measured(*entry, at, back, columns);
        }
    }
}

/// How many rows one result comes to at this width, with the blank above it.
fn measured(
    entry: Entry<'_>,
    at: usize,
    back: &[(Mark, Option<Box<str>>)],
    columns: usize,
) -> usize {
    let one = shown(entry, back);
    Expanded {
        shown: std::slice::from_ref(&one),
        from: 0,
    }
    .length(columns)
    .saturating_add(usize::from(at > 0))
}

/// Where each result begins, and where the last of them ends.
fn starts(heights: &[usize]) -> Vec<usize> {
    std::iter::once(0)
        .chain(heights.iter().scan(0_usize, |next, height| {
            *next = next.saturating_add(*height);
            Some(*next)
        }))
        .collect()
}

/// What the view holds read back for the results the window `reached`:
/// what it held already of them, and the rest read from the log, in the
/// order the window reaches them and no more than [`BEYOND`] of it.
///
/// A result the log has not placed yet is left out, to be asked for again on
/// the next frame. One that did not fit is remembered with where the window
/// was, and not read again until the window moves, or until nothing above it
/// has been read back.
fn reading(kept: &Kept, view: &mut View, reached: &[&Placed]) -> Vec<(Mark, Option<Box<str>>)> {
    let mut held = std::mem::take(&mut view.back);
    let mut back = Vec::new();
    let mut bytes = 0_usize;
    for placed in reached {
        let text = match held.iter().position(|(mark, _)| placed.is(mark)) {
            Some(at) => held.swap_remove(at).1,
            None if !back.is_empty()
                && view
                    .refused
                    .as_ref()
                    .is_some_and(|(from, mark)| *from == view.from && placed.is(mark)) =>
            {
                break;
            }
            None => match kept.read_back(placed) {
                Back::Said(text) => Some(text),
                Back::Unread => None,
                Back::Unplaced => continue,
            },
        };
        let size = text.as_ref().map_or(0, |text| text.len());
        if !back.is_empty() && bytes.saturating_add(size) > BEYOND {
            view.refused = Some((view.from, placed.mark()));
            break;
        }
        bytes = bytes.saturating_add(size);
        back.push((placed.mark(), text));
    }
    back
}

/// One held result, as the view shows it.
fn showing(whole: &Whole) -> Shown<'_> {
    Shown {
        called: whole.called(),
        text: whole.text(),
    }
}

/// What one key does to the view.
///
/// Every key is named rather than caught by a rest arm, for the reason the
/// permission panel names every one of its own: a key arriving at something
/// standing is either something it does or something it has decided to ignore,
/// and a new [`Pressed`] must be decided about here rather than quietly join the
/// second group.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn moving(arrived: Pressed, view: &mut View) -> Moved {
    match arrived {
        // The wheel walks this view rather than the transcript under it, which
        // is why it arrives here beside the arrows: what is standing is a window
        // over more text than its rows hold, and that is exactly the thing a
        // reader turning a wheel is pointing at. At either end it moves nothing,
        // and the loop that reads this takes that as the transcript's turn.
        Pressed::Up | Pressed::Scrolled { back: true } => {
            let next = view.from.checked_sub(1);
            region::step(&mut view.from, next)
        }
        Pressed::Down | Pressed::Scrolled { back: false } => {
            let next = Some(view.from.saturating_add(1)).filter(|next| *next <= view.end);
            region::step(&mut view.from, next)
        }

        // A page is the rows the window shows less one, so the row that was at
        // the foot is at the top afterwards and the reader keeps their place.
        // A page down onto a result not read back yet stops at its top, as an
        // arrow does: the layout that reads it back is where that is decided.
        Pressed::PageUp => {
            let next = Some(view.from.saturating_sub(paged(view))).filter(|next| *next < view.from);
            region::step(&mut view.from, next)
        }
        Pressed::PageDown => {
            let next = Some(view.from.saturating_add(paged(view)).min(view.end))
                .filter(|next| *next > view.from);
            region::step(&mut view.from, next)
        }

        // From one result to the next older or newer, whatever the window was
        // part way through: the top of that result at the top of the window,
        // or as near it as the window may go. Newest is first, so older is
        // down the view. At the oldest or the newest there is nowhere to step,
        // and the key moves nothing.
        Pressed::Key(Key::Right) => {
            let next = topmost(view)
                .checked_add(1)
                .and_then(|at| heading(view, at))
                .map(|top| top.min(view.end))
                .filter(|next| *next > view.from);
            region::step(&mut view.from, next)
        }
        Pressed::Key(Key::Left) => {
            let next = topmost(view)
                .checked_sub(1)
                .and_then(|at| heading(view, at))
                .filter(|next| *next < view.from);
            region::step(&mut view.from, next)
        }

        // Ctrl+O closes what Ctrl+O opened, which is the whole of what the rows
        // offering it say the key does. Esc is the way out of whatever is
        // standing everywhere else in a session — including out of a view
        // standing under a turn, which it closes rather than stopping the turn
        // behind it. Ctrl-C and Ctrl-D belong to the line under this one, so a
        // press closes the view and goes no further: only the next press
        // reaches the line.
        Pressed::Expand | Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,

        Pressed::Resized => Moved::Redraw,

        // Nothing here is typed into or picked from, so a letter, a click and a
        // mode step have nothing to act on. Return is among them: the line under
        // this is not being read, and closing on it would send whatever is in
        // the box the moment somebody meant to scroll. Ctrl+T for the reason
        // that is nearly the opposite: the plan it opens is in the rows this
        // view is standing over, so the key would move a panel nobody can see.
        // The key about what is running among them: those have their own list,
        // opened from the row under the box, and reached from in here it would be
        // two things standing in one region. The key that copies the line for the
        // plainest reason of all — the line is under this view, not in it. The
        // key that crosses regions for a reason of the same shape: a view is
        // one window over one thing, and one region is all there is here.
        Pressed::Key(_)
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Tab
        | Pressed::Explain
        | Pressed::Plan
        | Pressed::Clicked { .. }
        | Pressed::Pasted(_)
        | Pressed::Queue
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Rename
        | Pressed::All
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Moved::Still,
    }
}

/// How far a page moves the window: the rows it showed less the one kept in
/// sight, and a row at least, so a window of one row still moves.
fn paged(view: &View) -> usize {
    view.page.saturating_sub(1).max(1)
}

/// The row the call's line of result `at` is on: where it begins, past the
/// blank that parts it from the one above. That is the row the newest result's
/// call stands on when the view opens, so a step puts each one where the first
/// was rather than under a blank.
///
/// A step onto a result not read back yet stops at the blank instead, as a
/// step down by any other key does, and the layout reads it back from there.
fn heading(view: &View, at: usize) -> Option<usize> {
    let start = view.starts.get(at)?;
    Some(start.saturating_add(usize::from(at > 0)))
}

/// Which result is at the top of the window: the last to begin at or above it.
///
/// The top of the window as it is drawn rather than as far as it was asked to
/// go, which past the rows there are to lay is further down: counted from
/// there, the footer would name one result and a step go from another.
fn topmost(view: &View) -> usize {
    let top = view.from.min(view.laid);
    view.starts
        .iter()
        .filter(|start| **start <= top)
        .count()
        .saturating_sub(1)
}

#[cfg(test)]
mod tests;
