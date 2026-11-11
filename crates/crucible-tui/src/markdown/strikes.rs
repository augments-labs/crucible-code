//! What a run of emphasis markers turns on, the slot a phrase is drawn under
//! as a result, and the phrase between two single tildes, which cannot be
//! told from a path until the tilde that closes it arrives.
//!
//! A single tilde opens a phrase where it could begin one and closes the
//! nearest phrase still open where it could end one: whether each side of it
//! is a space, punctuation or part of a word decides, as it does for a pair
//! of two, so `2~3 seconds, then 5~10` strikes the words between the two
//! digits. What follows an opener is held rather than said, because until its
//! closer arrives nobody can say whether `~/Projects` was a retraction or a
//! directory somebody wrote. What holds is:
//!
//! - At most [`STRIKE_CAP`] bytes of source after the oldest opener still
//!   open. A closer that arrives later strikes nothing: that opener is drawn
//!   as the tilde it was, with what followed it, and nothing past the bound
//!   is kept back.
//! - No longer than the paragraph. A blank line, a line that opens a block of
//!   its own, a table, a block being held, a block handed on, a line
//!   taken back and the end of the message each let go of every opener still
//!   open, and each is drawn as written.
//! - Nothing held reaches the record before it settles. The live foot shows
//!   the last line of what is held, as written and clipped to the room, and
//!   once it settles the record holds what the whole answer draws.
//! - A struck phrase wears what a pair of two tildes gives the same words: the
//!   one ranking in [`Markdown::slot`] decides both.
//!
//! Only in prose: code, a key, a link's words and address, a bare address and
//! a block drawn as written never see a single tilde as a marker.

use std::cell::RefCell;
use std::rc::Rc;

use super::{HeldMarks, Inside, Markdown, Marked, Quoted, STRIKE_CAP};
use crate::color::Slot;
use crate::escape::Escapes;
use crate::row::Row;
use crate::width;

/// What is held after a single tilde that may open a phrase.
#[derive(Debug, Default)]
pub(super) struct Strikes {
    /// Every run said since the oldest opener still held, in order.
    runs: Vec<Kept>,
    /// The openers among them, oldest first.
    openers: Vec<Opener>,
    /// Source bytes read since the first of them opened.
    read: usize,
    /// The line of source being held, as written, for the live foot.
    line: String,
    /// Whether what is held is going out, so a run said now is not kept.
    releasing: bool,
}

/// A run said while a phrase is held, kept to be said once it settles.
#[derive(Debug)]
struct Kept {
    slot: Slot,
    text: String,
    link: Option<String>,
}

/// A single tilde that opened a phrase.
#[derive(Debug, Clone, Copy)]
struct Opener {
    /// Where among the held runs its tilde stands.
    run: usize,
    /// How many source bytes had been read when it opened.
    at: usize,
    /// Where among the held runs the phrase it opened ends, once closed.
    closed: Option<usize>,
}

impl Strikes {
    /// Keeps a run said while a phrase is held, and answers whether it did.
    pub(super) fn keep(&mut self, slot: Slot, text: &str, link: Option<&str>) -> bool {
        if self.releasing || self.openers.is_empty() {
            return false;
        }
        self.runs.push(Kept {
            slot,
            text: text.to_owned(),
            link: link.map(str::to_owned),
        });
        true
    }

    /// Whether an opener is waiting for its closer.
    fn pending(&self) -> bool {
        self.openers.iter().any(|opener| opener.closed.is_none())
    }

    /// Opens a phrase whose first character is `next`, read already.
    fn open(&mut self, next: char) {
        let at = if self.openers.is_empty() {
            self.line.clear();
            self.line.push('~');
            self.line.push(next);
            self.read = next.len_utf8();
            0
        } else {
            self.read
        };
        self.openers.push(Opener {
            run: self.runs.len(),
            at,
            closed: None,
        });
    }

    /// Settles where the opener just made stands: on the tilde said for it,
    /// or nowhere, where the tilde went somewhere other than out.
    fn opened(&mut self) {
        let said = self.runs.len().checked_sub(1);
        let stands = said.filter(|at| {
            self.runs.get(*at).is_some_and(|run| run.text == "~")
                && self.openers.last().is_some_and(|opener| opener.run == *at)
        });
        if stands.is_none() {
            self.openers.pop();
        }
    }

    /// Closes the phrase opened last of those still open.
    fn close(&mut self) {
        let end = self.runs.len();
        if let Some(opener) = self
            .openers
            .iter_mut()
            .rev()
            .find(|opener| opener.closed.is_none())
        {
            opener.closed = Some(end);
        }
    }

    /// Whether the oldest opener still open has held all it may.
    fn due(&self) -> bool {
        self.openers
            .iter()
            .find(|opener| opener.closed.is_none())
            .is_some_and(|opener| self.read.saturating_sub(opener.at) >= STRIKE_CAP)
    }

    /// Counts a character of source read while a phrase is held.
    fn feed(&mut self, character: char) {
        self.read = self.read.saturating_add(character.len_utf8());
        if character == '\n' {
            self.line.clear();
        } else if self.line.len() < STRIKE_CAP {
            self.line.push(character);
        }
    }

    /// Drops the oldest opener still open, which is a tilde as written now.
    fn fail_oldest(&mut self) {
        if let Some(at) = self
            .openers
            .iter()
            .position(|opener| opener.closed.is_none())
        {
            self.openers.remove(at);
        }
    }

    /// Drops every opener still open.
    fn fail_all(&mut self) {
        self.openers.retain(|opener| opener.closed.is_some());
    }

    /// Takes every run that has settled: those before the first opener still
    /// open, struck where a closed phrase covers them, with the tildes that
    /// opened those phrases left out.
    fn release(&mut self) -> Vec<Kept> {
        let boundary = self
            .openers
            .iter()
            .find(|opener| opener.closed.is_none())
            .map_or(self.runs.len(), |opener| opener.run)
            .min(self.runs.len());

        // How many closed phrases cover each run, by the change at each.
        let mut changes = vec![0_isize; boundary.saturating_add(1)];
        let mut tildes = vec![false; boundary];
        for opener in self.openers.iter().filter(|opener| opener.run < boundary) {
            let Some(closed) = opener.closed else {
                continue;
            };
            if let Some(tilde) = tildes.get_mut(opener.run) {
                *tilde = true;
            }
            if let Some(change) = changes.get_mut(opener.run.saturating_add(1)) {
                *change = change.saturating_add(1);
            }
            if let Some(change) = changes.get_mut(closed.min(boundary)) {
                *change = change.saturating_sub(1);
            }
        }

        let mut covered = 0_isize;
        let mut settled = Vec::with_capacity(boundary);
        for (at, mut run) in self.runs.drain(..boundary).enumerate() {
            covered = covered.saturating_add(changes.get(at).copied().unwrap_or_default());
            if tildes.get(at).copied().unwrap_or_default() {
                continue;
            }
            if covered > 0 {
                run.slot = struck(run.slot);
            }
            settled.push(run);
        }

        self.openers.retain(|opener| opener.run >= boundary);
        for opener in &mut self.openers {
            opener.run = opener.run.saturating_sub(boundary);
            opener.closed = opener.closed.map(|closed| closed.saturating_sub(boundary));
        }
        if self.openers.is_empty() {
            self.runs.clear();
            self.line.clear();
            self.read = 0;
        }
        settled
    }
}

/// The slot a run wears under a strike, given the one it wears without:
/// whatever the strike outranks in [`Markdown::slot`] gives way to it.
fn struck(slot: Slot) -> Slot {
    match slot {
        Slot::Heading | Slot::BoldEmphasis | Slot::Bold | Slot::Emphasis | Slot::Plain => {
            Slot::Struck
        }
        slot => slot,
    }
}

/// Whether a single tilde between `previous` and `next` could open a phrase
/// and whether it could close one, by the rule GitHub holds its tildes to.
fn flanks(previous: char, next: char) -> (bool, bool) {
    let space = |character: char| character == '\0' || character.is_whitespace();
    let mark = |character: char| !space(character) && !character.is_alphanumeric();
    let opens = !space(next) && (!mark(next) || space(previous) || mark(previous));
    let closes = !space(previous) && (!mark(previous) || space(next) || mark(next));
    (opens, closes)
}

impl Markdown {
    /// What is held after single tildes, made the first time a reader reads.
    ///
    /// Shared with the calls that tell what the scan says, because those are
    /// what keep it while a phrase is held.
    pub(super) fn shared_strikes(&mut self) -> Rc<RefCell<Strikes>> {
        Rc::clone(self.strikes.get_or_insert_with(Rc::default))
    }

    /// Whether a phrase opened by a single tilde is being held, so what
    /// followed it is not yet drawn.
    #[must_use]
    pub(crate) fn holds_a_phrase(&self) -> bool {
        self.strikes
            .as_ref()
            .is_some_and(|strikes| !strikes.borrow().openers.is_empty())
    }

    /// The row the live foot shows a held phrase by, drawn `columns` wide:
    /// the last line of what is held, as written, with nothing in it a
    /// terminal would take for an instruction. `None` while nothing is held.
    #[must_use]
    pub(crate) fn striking(&self, columns: usize) -> Option<Row> {
        let strikes = self.strikes.as_ref()?.borrow();
        if strikes.openers.is_empty() {
            return None;
        }
        let mut escapes = Escapes::default();
        let shown: String = strikes
            .line
            .chars()
            .filter(|character| {
                !escapes.holds(*character) && !width::unshown(*character) && !character.is_control()
            })
            .collect();
        Some(
            Row::new()
                .then_structural(Slot::Plain, shown)
                .clipped(columns),
        )
    }

    /// Reads a single tilde in prose that `next` followed.
    pub(super) fn tilde(
        &mut self,
        held: HeldMarks,
        next: char,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let previous = if held.opened { ' ' } else { self.previous };
        self.previous = '~';
        let (opens, closes) = flanks(previous, next);
        let strikes = self.shared_strikes();
        if closes && strikes.borrow().pending() {
            // The closer is a marker, and is not said.
            let settled = {
                let mut strikes = strikes.borrow_mut();
                strikes.close();
                strikes.release()
            };
            self.let_go(&settled, say);
        } else if opens {
            // What the line owes goes out first: it stands before the phrase.
            self.spend(say);
            self.pay(say);
            strikes.borrow_mut().open(next);
            self.say("~", say);
            // Where the tilde went somewhere other than out, it opened nothing,
            // and what was held for it goes out.
            let settled = {
                let mut strikes = strikes.borrow_mut();
                strikes.opened();
                strikes.release()
            };
            self.let_go(&settled, say);
        } else {
            self.say("~", say);
        }
    }

    /// Counts a character of source while a phrase is held, letting go of
    /// what is held where the phrase can go no further.
    pub(super) fn feeds(&mut self, character: char, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.gathering.is_some() || self.table.as_ref().is_some_and(|table| !table.pending()) {
            self.unstrike(say);
            return;
        }
        let Some(strikes) = self.strikes.clone() else {
            return;
        };
        // A tilde waiting on this character may be the closer, and is let
        // through: it arrived within the bound.
        let closing = self
            .held
            .is_some_and(|held| held.mark == '~' && held.count == 1);
        if !closing && strikes.borrow().due() {
            let settled = {
                let mut strikes = strikes.borrow_mut();
                strikes.fail_oldest();
                strikes.release()
            };
            self.let_go(&settled, say);
        }
        strikes.borrow_mut().feed(character);
    }

    /// Lets go of every phrase still open, each opener drawn as written.
    pub(super) fn unstrike(&self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let Some(strikes) = self.strikes.as_ref() else {
            return;
        };
        let settled = {
            let mut strikes = strikes.borrow_mut();
            strikes.fail_all();
            strikes.release()
        };
        self.let_go(&settled, say);
    }

    /// Says what has settled, in the order it was said.
    fn let_go(&self, settled: &[Kept], say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let Some(strikes) = self.strikes.as_ref() else {
            return;
        };
        if settled.is_empty() {
            return;
        }
        strikes.borrow_mut().releasing = true;
        for run in settled {
            say(run.slot, &run.text, run.link.as_deref());
        }
        strikes.borrow_mut().releasing = false;
    }

    /// Whether a run of two tildes followed by `next` turns a retraction on or
    /// off.
    ///
    /// The same rule the stars are held to, for the same reason: something has
    /// to follow on the same line for a run to open, so a pair left dangling at
    /// the end of one strikes nothing.
    pub(super) fn strikes(&self, next: char) -> bool {
        self.line.emphasis.struck || !next.is_whitespace()
    }

    /// Whether a run of markers followed by `next` turns emphasis on or off.
    pub(super) fn emphasises(&self, held: HeldMarks, next: char) -> bool {
        // Whichever of the two this run would close, since what a marker may do
        // depends on whether its own is already open.
        let open = if held.count >= 2 {
            self.line.emphasis.raised
        } else {
            self.line.emphasis.leant
        };

        if held.mark == '_' {
            return if open {
                // Closes at the end of a word rather than inside one, so
                // `_borrowed_ from` closes and `_borrowed_from` does not.
                !next.is_alphanumeric()
            } else {
                // Opens only between two words rather than beside one: a
                // word must not already be under way, and one has to start
                // here. `read_to_string` is a function far more often than it
                // is a sentence with emphasis in the middle of it -- and
                // `Ok(_)` is a pattern far more often than it is the start of
                // one, which the whitespace this used to ask about could not
                // tell apart, since the bracket that closes it is not
                // whitespace either.
                !self.previous.is_alphanumeric() && next.is_alphanumeric()
            };
        }

        // Something has to follow on the same line for a run to open. That is
        // what leaves `* item` a bullet instead of turning the rest of the
        // paragraph bold.
        open || !next.is_whitespace()
    }

    /// The slot everything read right now is written under.
    pub(super) fn slot(&self) -> Slot {
        let worn = if self.inside != Inside::Prose || self.line.noted {
            // A footnote's note is quiet, as what frames a block is.
            Slot::Quiet
        } else if matches!(self.line.quoted, Quoted::Owed | Quoted::Plain) {
            Slot::Quote
        } else if self.line.code || self.line.key {
            // Above everything a phrase can otherwise be: backticks say the
            // reader is being handed something to copy or go and find, and
            // that is worth more to them than the weight around it.
            Slot::Code
        } else if self.line.marked == Marked::Done {
            Slot::Done
        } else if self.line.heading {
            // A heading of its own, so a design can tell it from a phrase
            // the answer raised; weight alone until one does.
            Slot::Heading
        } else if self.line.emphasis.raised && self.line.emphasis.leant {
            // Raised and leant on at once, and drawn as both: neither of the
            // two it was written with is lost.
            Slot::BoldEmphasis
        } else if self.line.emphasis.raised || self.line.summary {
            // Weight in the reader's own foreground, not the theme's colour.
            // A summary is the one line of a folded block shown as a title.
            Slot::Bold
        } else if self.line.emphasis.leant {
            Slot::Emphasis
        } else {
            Slot::Plain
        };
        // Above weight, emphasis and a heading, and below the rest. A slot
        // says one thing, and of the things a phrase can be at once, the one
        // a reader most needs is that it was taken back -- bold words the
        // answer has retracted are worse than plain ones it has not.
        if self.line.emphasis.struck {
            struck(worn)
        } else {
            worn
        }
    }
}
