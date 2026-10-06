//! The prompts waiting behind a turn, and the panel that names them over the
//! box.
//!
//! The panel stands whenever anything is waiting — under a running turn, and
//! between turns while a used-up plan holds the lines it stopped in front of —
//! and it is the same panel in both places. There is nothing to open. Up and
//! down walk the highlight once the line in the box has no row to reach and no
//! list is standing, Ctrl+E takes the highlighted line back into the box to be
//! edited or sent sooner, and Ctrl+X deletes it. The highlight stays at the
//! same place after either, so the next line comes up under it, and the panel
//! goes when the last line does.
//!
//! A line too long to go into the box beside what is already typed there stays
//! queued, and the panel says so beside its title, or under it where the title
//! row has no room. The next key clears that and does what it always does.
//!
//! Nothing here holds the queue. The turn above goes on taking it at its next
//! pass boundary, whichever line is highlighted: a line the reader wants back
//! is one key away, and a queue that stopped the turn reading it would be a
//! second way to stop the turn that Esc already is. A line taken back or
//! deleted leaves the turn's offer as well as the panel, because the two hold
//! the same lines — one dropped from the panel alone is a prompt the reader
//! deleted that the turn works in anyway.

use std::collections::VecDeque;

use crucible_app::Conversation;
use crucible_runtime::Steer;
use crucible_tui::{Editor, Renderer, Row, Slot, Terminal, Typed, fold};

use crate::cli::Fatal;
use crate::cli::draw;
use crate::cli::style::Style;

use super::{
    Held, QUEUED_BYTES, QUEUED_LINES, Terms, Work, answerable, attaching, ran, unanswered,
};

/// The most prompts the panel names at once.
///
/// Three is enough to see the next few turns coming and few enough that a full
/// queue cannot push the box off the screen. The rest are in the count on the
/// title, and the highlight walks the window over them.
const SHOWN: usize = 3;

/// What leads a prompt in the panel: the mark, or the space that stands in for
/// it, and the space after.
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
///
/// Which line the keys act on is kept beside the lines, because every way a
/// line leaves — the turn taking it, the reader taking it back — has to move
/// the highlight with it, and a place kept anywhere else would be read against
/// a queue it was not measured on.
#[derive(Debug, Default)]
pub(super) struct Prompts {
    pub(super) lines: VecDeque<String>,
    pub(super) bytes: usize,
    /// The line the keys act on, counted from the oldest.
    at: usize,
    /// Whether the last Ctrl+E found no room in the box, until the next key.
    refused: bool,
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
    #[cfg(test)]
    pub(super) fn waiting(&self) -> Option<&str> {
        self.lines.front().map(String::as_str)
    }

    /// Every prompt waiting, oldest first.
    pub(super) fn waiting_all(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    /// How many prompts are waiting.
    pub(super) fn waiting_count(&self) -> usize {
        self.lines.len()
    }

    /// Which prompt the keys act on, counted from the oldest.
    pub(super) fn highlighted(&self) -> usize {
        self.at
    }

    /// Drops the prompt `at` places back, releasing its byte reservation.
    ///
    /// The highlight moves with the lines: one taken from before it moves it
    /// up a place so it stays on the same line, and one taken from under it
    /// leaves it where it was, on the line that came up into that place — or
    /// on the last, where there is no line after it. `None` where there is no
    /// such place.
    pub(super) fn drop(&mut self, at: usize) -> Option<String> {
        let prompt = self.lines.remove(at)?;
        self.bytes = self.bytes.saturating_sub(prompt.len());

        if at < self.at {
            self.at -= 1;
        }
        self.at = self.at.min(self.lines.len().saturating_sub(1));
        if self.lines.is_empty() {
            self.refused = false;
        }

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
        self.drop(0)
    }

    /// Moves the highlight a line toward the oldest where `back` is set, and
    /// toward the newest where it is not, and answers whether it moved.
    ///
    /// It stops at either end rather than going round: the arrow that finds
    /// nothing further is the one the history behind the box would have taken,
    /// and a highlight that wrapped would never let it go.
    pub(super) fn walk(&mut self, back: bool) -> bool {
        let to = if back {
            self.at.checked_sub(1)
        } else {
            Some(self.at + 1).filter(|to| *to < self.lines.len())
        };
        let Some(to) = to else {
            return false;
        };

        self.at = to;
        true
    }

    /// Deletes the highlighted prompt from the queue and from the turn's offer,
    /// and answers whether there was one.
    ///
    /// Nothing goes into the box: a line deleted is one the reader did not want
    /// sent, and one put in the box would be one Return away from being sent.
    pub(super) fn delete(&mut self, steer: &Steer) -> bool {
        let Some(line) = self.drop(self.at) else {
            return false;
        };

        steer.forget(&line);
        true
    }

    /// Takes the highlighted prompt back into the box, out of the queue and the
    /// turn's offer, and answers whether there was one to take.
    ///
    /// The box is asked first. A line it refuses — too long to go in beside
    /// what is already typed there — stays queued under the highlight, and the
    /// panel says why until the next key: taken out before the box said no, it
    /// would be in neither place.
    pub(super) fn take_back(&mut self, editor: &mut Editor, steer: &Steer) -> bool {
        let Some(line) = self.lines.get(self.at) else {
            return false;
        };

        if editor.paste(line) == Typed::Refused {
            self.refused = true;
            return true;
        }

        if let Some(line) = self.drop(self.at) {
            steer.forget(&line);
        }
        true
    }

    /// Clears what the panel said about a line the box had no room for, and
    /// answers whether it was saying it.
    pub(super) fn settle(&mut self) -> bool {
        std::mem::take(&mut self.refused)
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
/// `None` beside the conversation where nothing was waiting or a used-up plan
/// is holding it, and otherwise whether the session is leaving, as [`ran`] says it.
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

/// What a line moving between the box and the queue acts on, held together so
/// that one call carries all of it.
///
/// Three references rather than three arguments at each call, for the reason
/// [`super::Held`] is one value: the queue, the box a line comes from or goes
/// back to, and the offer the turn reads are one subject, and a key is answered
/// against all three or against none of them.
pub(super) struct Reading<'a> {
    /// The queue, which is also the panel over the box.
    pub(super) queue: &'a mut Prompts,
    /// The box a line is taken from or back into.
    pub(super) editor: &'a mut Editor,
    /// The offer the running turn reads.
    pub(super) steer: &'a Steer,
}

/// The panel naming the prompts waiting, as it stands over the box.
///
/// The shape every other panel has: a rule, a title, the lines, and a footer
/// naming every key that works. The title says how many are waiting and the
/// key that sends them all, because the count is the one fact that cannot go:
/// three are named at most, and the rest are only in the number. The
/// highlighted line leads with the mark a line is typed after and is drawn in
/// the accent, so a key's target is never a guess; the rest stand two columns
/// in under it. Each line is cut to a row: the panel is a list of what is
/// waiting, and the line itself is a Ctrl+E away.
///
/// The window of three follows the highlight, so the line the keys act on is
/// always among those named. Nothing is kept between frames to say where it
/// was scrolled to; the highlight alone says.
///
/// `room` is the rows it may have. A window short of them names fewer lines,
/// down to one, and below that draws nothing — the queue is still the queue,
/// and its turn will say each line. No rows at all where nothing is waiting,
/// or where the window is too narrow to put any of a line beside its mark.
pub(super) fn panel(queue: &Prompts, columns: usize, room: usize, style: Style) -> Vec<Row> {
    let count = queue.waiting_count();
    if count == 0 || columns <= MARKED {
        return Vec::new();
    }

    let glyphs = style.glyphs();
    let dot = glyphs.dot();

    let title = Row::new()
        .then(Slot::Strong, format!("{count} queued"))
        .then(Slot::Quiet, format!(" {dot} ctrl+enter to send all now"));

    // Beside the title where the row holds both, since it is about the line
    // under the highlight and not a line of its own; under it where the row
    // does not, in the blank that parts the title from the lines.
    let mut under = vec![Row::new()];
    let title = if queue.refused {
        let beside = format!(" {dot} no room in the box {dot} line stays queued");
        if title.columns() + crucible_tui::columns(&beside) <= columns {
            title.then(Slot::Quiet, beside)
        } else {
            under = Row::new()
                .then(
                    Slot::Quiet,
                    format!("no room in the box {dot} line stays queued"),
                )
                .fold(columns);
            title
        }
    } else {
        title
    };
    let title = title.fold(columns);

    let (up, down) = glyphs.walking();
    let keys = format!(
        "{up}{down} to walk {dot} ctrl+e to edit {dot} ctrl+x to delete {dot} ctrl+s to send now"
    );
    let footer = fold(&keys, columns);

    // The rule and the blank under it, the title and what is under it, then
    // the blank over the footer, the footer and the blank that parts it from
    // the box. Each line named past the first costs the blank before it too.
    let chrome = 2 + title.len() + under.len() + 1 + footer.len() + 1;
    let fits = room.saturating_sub(chrome).div_ceil(2);
    let shown = SHOWN.min(count).min(fits);
    if shown == 0 {
        return Vec::new();
    }

    let at = queue.highlighted();
    let start = at.saturating_sub(shown - 1).min(count - shown);
    let across = columns.saturating_sub(MARKED);

    let mut rows = vec![
        Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
        Row::new(),
    ];
    rows.extend(title);
    rows.extend(under);

    for (place, said) in queue.waiting_all().enumerate().skip(start).take(shown) {
        if place > start {
            rows.push(Row::new());
        }

        // Only the opening of a line is read: a prompt may be a megabyte, and
        // the panel is drawn again on every frame.
        let said = draw::clipped_start(said, across, glyphs);
        rows.push(if place == at {
            Row::new()
                .then(Slot::Accent, glyphs.caret())
                .then(Slot::Plain, " ")
                .then(Slot::Accent, said)
        } else {
            Row::new().then(Slot::Plain, format!("  {said}"))
        });
    }

    rows.push(Row::new());
    rows.extend(
        footer
            .into_iter()
            .map(|row| Row::new().then(Slot::Quiet, row)),
    );
    rows.push(Row::new());
    rows
}

#[cfg(test)]
mod tests;
