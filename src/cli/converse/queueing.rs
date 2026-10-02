//! The prompts waiting behind a turn, and the view that stands them open to be
//! gone over.
//!
//! Ctrl+Q stands the whole queue where the panel above the box named the first
//! few of it. Up and down walk it, `e` takes the marked line back into the box
//! to be edited or sent sooner (`x` did that before the footer named a key, and
//! still does), `d` or Delete drops it without taking it back, and `esc` — or
//! the key that opened it — closes it again. A line too long to go into the box
//! beside what is already typed there stays queued, with the mark on it.
//!
//! While it stands, the queue is held, and that is the point of it. The turn
//! above goes on writing and takes none of these lines: a line the reader is
//! still going over is not one the agent should be reading, and one taken
//! mid-edit is in the transcript, where it cannot be taken back. Closing the
//! view releases the whole batch at once — the lines that were edited and the
//! ones that were not — and the turn works them in at its next pass boundary.
//!
//! The row saying a turn is running stays directly over the view's rule, with
//! its clock counting. The list is laid in the rows left under it, so a list
//! long enough to fill the window gives up one row to it. The working row is
//! dropped only when the rows left would show none of the list.
//!
//! Nothing above it stops for that. The turn writes into the tail as it always
//! does; a held queue answers the exchange loop the way an empty one does, which
//! is what it meets at almost every pass anyway. What the reader sees is their
//! own lines sitting still while the answer above them goes on arriving.
//!
//! Where it stands depends on whether a turn is running, and on nothing else —
//! the shape [`super::expanding`] sets. Between turns it takes the region the
//! box was in and reads keys of its own, because nothing else is reading; while
//! a turn runs it stands under the tail, in the rows the box has, and every
//! frame draws it again beneath whatever arrived above it. The keys are the same
//! either way, and a view opened under a turn is still open when the turn ends —
//! which is what stops the queue being committed out from under a reader who was
//! halfway through it.

use std::collections::VecDeque;

use crucible_app::Conversation;
use crucible_runtime::Steer;
use crucible_tui::{Caret, Editor, Key, Pressed, Renderer, Row, Terminal, Typed};

use crate::cli::Fatal;
use crate::cli::draw;
use crate::cli::style::Style;

use super::region::{self, Moved};
use super::typing::Asked;
use super::{
    Held, QUEUED_BYTES, QUEUED_LINES, Terms, Turning, Work, answerable, attaching, ran, unanswered,
};

/// What stands around the lines of the view: the rule, the title, the blank
/// under each of them, and the blank above the footer. The footer's own rows
/// are counted beside it, since the width decides how many there are.
const CHROME: usize = 5;

/// What leads a line in the view: the mark, or the space that stands in for it,
/// and the space after.
const MARKED: usize = 2;

/// Prompts finished while a turn is still running.
///
/// Kept in order, and every one of them kept — the other two answers were both
/// wrong for the same reason: keeping one line and dropping the rest loses
/// something the user typed, watched the box take, and never sees again, and
/// joining them into one prompt puts a message in the transcript nobody wrote.
///
/// Lines and bytes are both bounded: one-byte prompts cannot choose an
/// unbounded number of allocations, and full-sized prompts cannot choose an
/// unbounded retained buffer. Refusal leaves the editor untouched, so a prompt
/// is never silently dropped after the box appeared to accept it.
#[derive(Debug, Default)]
pub(super) struct Prompts {
    pub(super) lines: VecDeque<String>,
    pub(super) bytes: usize,
}

/// Whether a finished line moved from the editor into [`Prompts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Retained {
    /// The line is waiting for its turn.
    Accepted,
    /// A line or byte ceiling left it in the editor.
    Refused,
}

impl Prompts {
    /// Takes the editor whole where both ceilings have room.
    pub(super) fn accept(&mut self, editor: &mut Editor) -> Retained {
        let bytes = editor.text().len();
        if self.lines.len() >= QUEUED_LINES || bytes > QUEUED_BYTES.saturating_sub(self.bytes) {
            return Retained::Refused;
        }

        self.bytes += bytes;
        self.lines.push_back(editor.take());
        Retained::Accepted
    }

    /// The prompt the next turn will be taken from, where one is waiting.
    ///
    /// Read while the turn ahead of it is still running, for the row that says
    /// what is coming after it. A line that went into the box and vanished is
    /// the thing this exists to stop: the queue is the only place it is, and
    /// until it is named there is nothing on screen to say it was kept.
    #[cfg(test)]
    pub(super) fn waiting(&self) -> Option<&str> {
        self.lines.front().map(String::as_str)
    }

    /// Every prompt waiting, oldest first.
    ///
    /// The panel above the box is drawn from these: the second and third are as
    /// much queued as the first, and a list that named only the front one said
    /// the rest were not there.
    pub(super) fn waiting_all(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// How many prompts are waiting.
    pub(super) fn waiting_count(&self) -> usize {
        self.lines.len()
    }

    /// Drops the prompt `at` places back, releasing its byte reservation.
    ///
    /// What the queue's full view removes one with: a line typed and not yet
    /// sent is the reader's to take back until the turn takes it. `None` where
    /// there is no such place.
    pub(super) fn drop(&mut self, at: usize) -> Option<String> {
        let prompt = self.lines.remove(at)?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }

    /// Drops the oldest waiting prompt that says `line`, and answers whether
    /// there was one.
    ///
    /// What a turn taking a line to steer by leaves behind. From the moment it
    /// is taken the line is in the transcript, so a panel that goes on naming
    /// it says the reader is owed a turn they have already had — and the count
    /// beside it says so twice. Matched on what the line says rather than on
    /// where it sat, because the reader may have taken an earlier one back
    /// between the turn reading the queue and saying what it read.
    pub(super) fn steered(&mut self, line: &str) -> bool {
        let Some(at) = self.lines.iter().position(|waiting| waiting == line) else {
            return false;
        };

        self.drop(at).is_some()
    }

    /// Takes the oldest waiting prompt and releases its byte reservation.
    pub(super) fn pop(&mut self) -> Option<String> {
        let prompt = self.lines.pop_front()?;
        self.bytes = self.bytes.saturating_sub(prompt.len());
        Some(prompt)
    }
}

/// Takes the whole queue for one turn: the oldest line is the prompt, and every
/// line behind it is offered to that same turn.
///
/// A burst typed behind a turn is one thing the reader wanted said. Taken a line
/// per turn, the first was answered before the model had read the second, so the
/// agent worked to a question the reader had already added to — and three turns
/// went by answering what was asked once. Handed over together, the runner
/// records the batch at the first boundary of the turn this starts, which is
/// before it asks anything: the model reads all of it and then answers all of
/// it.
///
/// The lines behind the prompt go through the steer rather than into it,
/// because joining them would put a message in the transcript nobody wrote.
/// Each reaches it as the line it was typed as; what they share is the turn.
///
/// `None` where nothing is waiting, which is the ordinary case.
pub(super) fn batched(queued: &mut Prompts, steer: &Steer) -> Option<String> {
    let said = queued.pop()?;

    while let Some(behind) = queued.pop() {
        steer.say(behind);
    }

    Some(said)
}

/// Runs the lines queued during the last turn as the next turn, all of them at
/// once: the oldest is its prompt and the rest are offered to it.
///
/// They are committed here rather than where they were typed: at that moment
/// the answer above them was still arriving, and a line written into the middle
/// of one is a line in the wrong place.
///
/// Not after work that stopped on a used-up plan: the lines stay queued for
/// the reader, who can take them back or send a prompt, because the plan
/// they would be sent to is spent until its reset.
///
/// `None` beside the conversation where nothing was waiting or the queue is
/// held, and otherwise whether the session is leaving, as [`ran`] says it.
pub(super) fn taken<T: Terminal>(
    conversation: Conversation,
    renderer: &mut Renderer<T>,
    terms: &Terms,
    held: &mut Held<'_>,
    style: Style,
) -> Result<(Conversation, Option<bool>), Fatal> {
    if held.used_up {
        return Ok((conversation, None));
    }

    // Each line gets what it would have got typed at the box with nobody to
    // ask: its row, the warning, and no turn. Not one turn for the batch,
    // because no turn is what is owed, and not one warning, because each line
    // is somebody's prompt that was not sent.
    if held.queued.waiting_count() > 0 && !answerable(&conversation) {
        while let Some(said) = held.queued.pop() {
            draw::queued(renderer, &said, style)?;
            unanswered(&conversation, renderer, terms)?;
        }
        return Ok((conversation, Some(false)));
    }

    let Some(said) = batched(&mut held.queued, &terms.steer) else {
        return Ok((conversation, None));
    };
    draw::queued(renderer, &said, style)?;

    let imported = attaching::imported(held);
    let attached = attaching::beside(
        renderer,
        attaching::Asking::of(conversation.runner(), imported.as_deref()),
        &terms.workspace,
        attaching::Sent {
            prompt: &said,
            images: &held.images,
        },
        style,
    )?;
    let work = Work::Turn(said, attached);
    let (back, leaving) = ran(conversation, renderer, terms, work, held)?;
    Ok((back, Some(leaving)))
}

/// What the view acts on, held together so that one call carries all of it.
///
/// Three references rather than three arguments at each call, for the reason
/// [`super::Held`] is one value: the list, the box a line taken back returns
/// to, and the queue the turn reads are one subject, and a key press is
/// answered against all three or against none of them. Enter while a turn runs
/// is answered against the same three, moving a line the other way.
pub(super) struct Reading<'a> {
    /// The list being read, which is also the panel above the box.
    pub(super) queue: &'a mut Prompts,
    /// The box a line taken back returns to.
    pub(super) editor: &'a mut Editor,
    /// The offer the running turn reads, held while this stands.
    pub(super) steer: &'a Steer,
}

/// Whether the queue is standing open, and which line the keys act on.
///
/// Held by the session rather than by either of the loops that draw it, for the
/// reason the other view here is: one opened while a turn ran is still open when
/// that turn ends, and the reader who opened it is still reading.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum Standing {
    /// Nothing is standing, and the turn may take the queue.
    #[default]
    Closed,
    /// The list is standing, with the mark this far down it.
    Open(usize),
}

impl Standing {
    /// Whether the list is standing.
    pub(super) fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }

    /// Opens the list, and holds the queue while it stands.
    ///
    /// Nothing opens on an empty queue: the key is offered by the panel that
    /// names what is waiting, so a session with nothing waiting has made no
    /// offer, and a frame put up in answer to a press nobody meant is one that
    /// took the box away for no reason.
    pub(super) fn open(&mut self, queue: &Prompts, steer: &Steer) {
        if queue.waiting_count() == 0 {
            return;
        }

        steer.hold();
        *self = Self::Open(0);
    }

    /// Answers what the box between turns reported, where it is this view's:
    /// Ctrl+Q over the lines a used-up plan held stands the list, which the
    /// loop then reads keys for until it is closed.
    pub(super) fn asked(&mut self, asked: &Asked, queue: &Prompts, steer: &Steer) -> bool {
        match asked {
            Asked::Queue => self.open(queue, steer),

            // Not this one's. A line, a turn nobody typed, the end of a
            // session and the other view are answered elsewhere.
            Asked::Said(_)
            | Asked::Woke(_)
            | Asked::Ended
            | Asked::Untyped
            | Asked::Expand
            | Asked::Clicked(_) => return false,
        }

        true
    }

    /// Gives one key to the list, and answers whether a frame is owed.
    ///
    /// That it closed is read off [`Standing::is_open`] afterwards rather than
    /// reported here: the caller draws something either way, and which of the
    /// two it draws is a question about the state and not about the key.
    pub(super) fn against(&mut self, arrived: &Pressed, reading: Reading<'_>) -> bool {
        let Self::Open(at) = self else {
            return false;
        };

        let mut open = Open { at: *at, reading };

        match moving(arrived, &mut open) {
            Moved::Redraw => {
                *self = Self::Open(open.at);
                true
            }
            Moved::Still => false,

            // Nothing here is committed, so the two ways out are one way out.
            Moved::Took | Moved::Left => {
                open.reading.steer.release();
                *self = Self::Closed;
                true
            }
        }
    }
}

/// The list and the mark on it, which is what a key is answered against.
///
/// One value so that the two closures [`region::stand`] drives can each borrow
/// the whole of it.
struct Open<'a> {
    /// Which line a key acts on.
    at: usize,
    /// What it acts on it with.
    reading: Reading<'a>,
}

/// Stands the list where the box was, and reads keys until it is closed.
///
/// Between turns, which is the half of this that has the keyboard to itself.
/// Reached where a turn ended under an open view: the queue is still the
/// reader's until they close it, and committing it out from under them is the
/// one thing this whole view exists to stop. Returns as soon as it is called if
/// nothing is open.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn stand<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    reading: Reading<'_>,
    standing: &mut Standing,
) -> Result<(), Fatal> {
    let Standing::Open(at) = *standing else {
        return Ok(());
    };

    let mut open = Open { at, reading };
    let picture =
        |open: &mut Open<'_>, columns: usize, rows: usize| (laid(open, columns, rows, style), None);

    region::stand(
        renderer,
        |_| style,
        &mut open,
        picture,
        |arrived, open| moving(&arrived, open),
    )?;

    // Every way out is the same way out, a window with no room among them: the
    // region goes back, the box comes up under it, and the lines the reader left
    // in the queue are the turn's again.
    open.reading.steer.release();
    *standing = Standing::Closed;
    Ok(())
}

/// Stands the list under the tail, in the rows the box has while a turn runs.
///
/// Answers whether it stood, which is the caller's question rather than this
/// one's: the box and the list take the same rows, so exactly one of them is
/// drawn per frame and the caller draws the other. The row that says the turn
/// is running stands directly over the list's rule, as it stands over the box.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
// Six is one over clippy's limit, and each is a distinct thing the frame is
// drawn from: the terminal, the dress, the queue, whether it stands, the offer
// the turn reads, and the turn whose row stands over it. The caller holds all
// six as separate values, and a type made to bundle two of them would be built
// at the one call site only to be taken apart here.
#[allow(clippy::too_many_arguments)]
pub(super) fn under<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    queue: &Prompts,
    standing: &mut Standing,
    steer: &Steer,
    turning: &Turning,
) -> Result<bool, Fatal> {
    let Standing::Open(at) = *standing else {
        return Ok(false);
    };

    // One row is left to the transcript whatever stands here, and it is the row
    // the turn goes on writing into: a list that asked for the whole window
    // would leave what is being said now nowhere at all to appear.
    let room = renderer.rows().saturating_sub(1);
    let columns = renderer.columns();

    // The row that says the turn is running stays directly over the view's rule,
    // as it stands over the box. It is the first thing to give way: a window
    // that can hold the list only without it holds the list, since the list is
    // what the reader opened.
    let mut laid = rows(queue, at, columns, room.saturating_sub(1), style);
    if laid.is_empty() {
        laid = rows(queue, at, columns, room, style);
    } else {
        laid.insert(0, turning.working(columns, style));
    }

    // Nothing left to stand: the reader took the last line back, or the window
    // has no room for the list at all. Either way the box comes back in this
    // same frame, and the queue is the turn's again.
    let Some(row) = laid.len().checked_sub(1) else {
        steer.release();
        *standing = Standing::Closed;
        return Ok(false);
    };

    renderer.under(&laid, Some(Caret { row, column: 0 }), style.palette())?;
    Ok(true)
}

/// One key against the list, and what it owes the picture.
///
/// The one handler for both halves of the view, so a key that does one thing
/// under a turn cannot come to do another between them.
fn moving(arrived: &Pressed, open: &mut Open<'_>) -> Moved {
    let at = open.at;

    match arrived {
        Pressed::Up => region::step(&mut open.at, at.checked_sub(1)),
        Pressed::Down => region::step(
            &mut open.at,
            (at + 1 < open.reading.queue.waiting_count()).then(|| at + 1),
        ),

        // The two keys that change the queue. `e` takes the marked line back into
        // the box, where it can be edited or sent ahead of the rest, and `x` is
        // the key it was before the footer named it; `d` and Delete drop it and
        // put nothing in the box. Either way it leaves both places it sits in,
        // because the panel and the turn's own offer hold the same line — one
        // dropped from the panel alone is a prompt the reader deleted that the
        // turn works in anyway. With one line each is also the way out, since the
        // list it was read from is then empty. A line the box refuses leaves
        // neither: it stays where it was, rather than being in no place at all.
        Pressed::Key(Key::Char('e' | 'x')) => removed(open, at, true),
        Pressed::Key(Key::Char('d') | Key::Delete) => removed(open, at, false),

        Pressed::Escape | Pressed::Queue => Moved::Left,
        _ => Moved::Still,
    }
}

/// Takes the line at `at` out of the queue and the turn's offer, into the box
/// where `back` is set, and answers what that owes the picture.
///
/// The box is asked first. A line it refuses — too long to go in beside what
/// is already typed there — stays queued with the mark on it, and nothing
/// changes: taken out before the box said no, it would be in neither place.
fn removed(open: &mut Open<'_>, at: usize, back: bool) -> Moved {
    if back {
        let Some(line) = open.reading.queue.waiting_all().nth(at) else {
            return Moved::Still;
        };
        if open.reading.editor.paste(line) == Typed::Refused {
            return Moved::Still;
        }
    }

    let Some(line) = open.reading.queue.drop(at) else {
        return Moved::Still;
    };

    open.reading.steer.forget(&line);
    open.at = at.min(open.reading.queue.waiting_count().saturating_sub(1));

    if open.reading.queue.waiting_count() == 0 {
        Moved::Left
    } else {
        Moved::Redraw
    }
}

/// The rows of the list as [`region::stand`] asks for them.
fn laid(open: &mut Open<'_>, columns: usize, rows: usize, style: Style) -> Vec<Row> {
    self::rows(open.reading.queue, open.at, columns, rows, style)
}

/// The queue laid out as a panel, with the marked line standing out.
///
/// The shape every other panel has: a rule, a title, the lines, and a footer
/// naming every key that works. The marked line leads with the mark a line is
/// typed after and is drawn in the accent, so a key's target is never a guess;
/// the rest stand two columns in under it. A line is read whole here where the
/// box cut it to a row, so it wraps and hangs under its own first word. The
/// marked line is always among those drawn: a window short of the whole queue
/// scrolls to it. No rows at all where there is nothing left to name, which
/// both callers read as the view closing.
fn rows(queue: &Prompts, at: usize, columns: usize, rows: usize, style: Style) -> Vec<Row> {
    use crucible_tui::{Slot, fold};

    let waiting = queue.waiting_count();
    let glyphs = style.glyphs();
    let (up, down) = glyphs.walking();
    let dot = glyphs.dot();
    let keys = format!("{up}{down} to walk {dot} e edit {dot} d delete {dot} esc to close");
    let footer = fold(&keys, columns);

    // The rule, the title, the three blanks and the footer take their rows
    // before a line is given one.
    let room = rows.saturating_sub(CHROME + footer.len());
    if waiting == 0 || room == 0 {
        return Vec::new();
    }

    let mut laid = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
        Row::new().then(
            Slot::Strong,
            draw::clipped(format!("{waiting} queued"), columns, glyphs),
        ),
        Row::new(),
    ];

    // Only as much of a line as the room could show is folded: a prompt may be
    // a megabyte, and the view is drawn again on every frame.
    let across = columns.saturating_sub(MARKED);
    let parts = |place: usize| -> Vec<String> {
        queue
            .waiting_all()
            .nth(place)
            .map(|said| {
                let shown = draw::clipped(said, across.saturating_mul(room), glyphs);
                fold(&shown, across)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };

    // The marked line is always among the lines drawn, whole where the room
    // holds it and from its first row where it does not: `d` takes away the
    // words the mark stands on, and words the reader never saw are not a thing
    // to take away. The room is filled backwards from it with the lines before
    // it that fit whole, then forwards with what comes after. Nothing is kept
    // between frames to say where the list was scrolled to; the mark alone says.
    let at = at.min(waiting - 1);
    let mut marked = parts(at);
    marked.truncate(room);
    let mut used = marked.len();

    let mut before = Vec::new();
    for place in (0..at).rev() {
        let earlier = parts(place);
        if used + earlier.len() > room {
            break;
        }
        used += earlier.len();
        before.push((place, earlier));
    }
    before.reverse();

    let mut blocks = before;
    blocks.push((at, marked));
    for place in at + 1..waiting {
        if used >= room {
            break;
        }
        let mut later = parts(place);
        later.truncate(room - used);
        used += later.len();
        blocks.push((place, later));
    }

    for (place, lines) in blocks {
        let tone = if place == at {
            Slot::Accent
        } else {
            Slot::Plain
        };

        for (nth, part) in lines.into_iter().enumerate() {
            let lead = if nth == 0 && place == at {
                glyphs.caret()
            } else {
                " "
            };
            laid.push(
                Row::new()
                    .then(tone, lead)
                    .then(Slot::Plain, " ")
                    .then(tone, part),
            );
        }
    }

    laid.push(Row::new());
    laid.extend(
        footer
            .into_iter()
            .map(|row| Row::new().then(Slot::Quiet, row)),
    );
    laid
}

#[cfg(test)]
mod tests;
