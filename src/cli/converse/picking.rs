//! Something standing where the prompt box was, until one of its entries is
//! taken or it is left.
//!
//! The box and the row under it are what the bands at the foot of the window
//! hold while a line is being typed; this puts a panel or a ladder there instead
//! and reads keys against it. That is the whole of what "it replaces the prompt"
//! means — nothing is drawn over anything, those bands are simply given
//! different rows, and the next frame after this returns is the box again.
//!
//! Nothing about the *contents* is decided here. What is being chosen, what
//! each entry says and what the footer names are the caller's, which is what
//! lets `/login`, `/logout` and a first run that opens with the same list share
//! one loop rather than one string.
//!
//! Three sets of keys rather than one, because the three shapes are read
//! differently: a list is walked down, a ladder is walked along, and a shelf is
//! both at once with a line being typed above them. A component whose arrows
//! disagree with its picture is one nobody trusts twice.
//! What happens around the keys — the resize, the answer, and the box coming
//! back — is [`region`]'s, and is one loop.
//!
//! The one promise this module makes about the keys is that the mark stops at
//! each end instead of wrapping. A ring puts the first entry one key past the
//! last, so the key that went too far is the key that goes further.

use crucible_tui::{
    Caret, Editor, Head, Key, Ladder, Pane, Panel, Pressed, Renderer, Row, Terminal, Typed,
};

use crate::cli::Fatal;
use crate::cli::style::Style;

use super::region::{self, Ended, Moved, step};

/// What came back off a panel.
///
/// Three answers rather than two, because what a caller owes differs in each.
/// A panel that was left has already been answered — the person who pressed
/// escape asked for the screen they had before it, and the list underneath
/// would be this program insisting on the question a second time. What is owed
/// there is one line saying the question was dropped, in the words of whatever
/// asked it. A panel there was no room for was never drawn and read no key, so
/// the answer still has to come from somewhere, and the list is it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    /// The entry at this index was taken. It is an index into the slice that
    /// was handed in.
    Took(usize),
    /// The panel was left with nothing taken. What is owed is the caller's
    /// line saying so.
    Left,
    /// There was no room to stand one. [`Panel::within`] gives up rows rather
    /// than overflowing, and its last rung is nothing at all.
    Cramped,
}

/// [`Picked`], with the index already looked up in what the panel was built
/// from — which is the shape every caller wants, none of them having a use for
/// a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Taken<T> {
    /// This entry was taken off the panel.
    Took(T),
    /// The panel was left. What is owed is the caller's line saying so.
    Left,
    /// Nothing was drawn and nothing read. The caller still owes an answer.
    Cramped,
}

impl Picked {
    /// Looks a taken index up in `all`, the slice the panel was built from.
    ///
    /// The index cannot miss — it came back out of a list this same slice was
    /// mapped into. Written as a lookup that can rather than as an assertion
    /// nobody would read again, and a miss falls to the answer a caller has for
    /// a panel it could not stand, which is the one that draws something.
    pub(super) fn of<T: Copy>(self, all: &[T]) -> Taken<T> {
        match self {
            Self::Took(at) => all.get(at).copied().map_or(Taken::Cramped, Taken::Took),
            Self::Left => Taken::Left,
            Self::Cramped => Taken::Cramped,
        }
    }

    /// How the loop ended, with the mark it finished on read back into it.
    ///
    /// The mark is where the index comes from: [`Ended`] says a thing was
    /// taken and this says which, because the loop that read the key has no
    /// use for what the mark stood on.
    fn ended(ended: Ended, at: usize) -> Self {
        match ended {
            Ended::Took => Self::Took(at),
            Ended::Left => Self::Left,
            Ended::Cramped => Self::Cramped,
        }
    }
}

/// Stands `panel` where the prompt box was and reads keys until one is chosen.
///
/// `panel.chosen` is where the mark starts, and [`Picked`] says which of the
/// three ways it ended.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn pick<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    panel: Panel<'_>,
) -> Result<Picked, Fatal> {
    pick_while(renderer, style, panel, &mut |_| Ok(()))
}

/// [`pick`], with the turn's drain run once a pass so the transcript keeps
/// moving while the panel stands over a turn. Between turns there is nothing
/// to drain, so `pick` passes a no-op.
pub(super) fn pick_while<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    mut panel: Panel<'_>,
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Picked, Fatal> {
    let count = panel.shown.len();
    if count == 0 {
        return Ok(Picked::Cramped);
    }

    let mut at = panel.chosen.min(count - 1);

    let ended = region::stand_while(
        renderer,
        |_| style,
        &mut at,
        |marked, columns, rows| {
            panel.chosen = *marked;
            (panel.within(columns, rows, style.glyphs()), None)
        },
        |arrived, at| moving(arrived, at, count),
        while_waiting,
    )?;

    Ok(Picked::ended(ended, at))
}

/// Stands `ladder` where the prompt box was and reads keys until a rung is
/// taken.
///
/// `ladder.chosen` is where the mark starts, and the index that comes back is
/// into the rungs it was handed. Otherwise [`pick`], for a component read along
/// rather than down.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn adjust<T: Terminal>(
    renderer: &mut Renderer<T>,
    style: Style,
    mut ladder: Ladder<'_>,
) -> Result<Picked, Fatal> {
    let count = ladder.rungs.len();
    if count == 0 {
        return Ok(Picked::Cramped);
    }

    let mut at = ladder.chosen.min(count - 1);

    let ended = region::stand(
        renderer,
        |_| style,
        &mut at,
        |marked, columns, _| {
            ladder.chosen = *marked;
            (ladder.rows(columns, style.glyphs()), None)
        },
        |arrived, at| sliding(arrived, at, count),
    )?;

    Ok(Picked::ended(ended, at))
}

/// What `arrived` does to a panel of `count` entries with `at` marked.
///
/// Every key that is not one of the five is a key that moved nothing. A panel
/// is not a line, so a letter is not a character being typed, and a frame for
/// each of them would be a frame per keystroke for somebody typing at a list.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn moving(arrived: Pressed, at: &mut usize, count: usize) -> Moved {
    match arrived {
        Pressed::Up => step(at, at.checked_sub(1)),
        Pressed::Down => step(at, Some(*at + 1).filter(|next| *next < count)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// What `arrived` does to a ladder of `count` rungs with `at` marked.
///
/// The arrows that walk it are the ones drawn under it, and they are the across
/// pair: a ladder is one row of rungs, so up and down point at nothing. They are
/// left alone rather than aliased onto the across pair, because a key that moves
/// the mark in a direction the picture does not have is how somebody learns not
/// to trust the picture.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn sliding(arrived: Pressed, at: &mut usize, count: usize) -> Moved {
    match arrived {
        Pressed::Key(Key::Left) => step(at, at.checked_sub(1)),
        Pressed::Key(Key::Right) => step(at, Some(*at + 1).filter(|next| *next < count)),
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        _ => Moved::Still,
    }
}

/// The rows the window keeps for itself while something stands in it.
///
/// The head at the top and the row that maps the transcript at the foot. A
/// component is handed the whole height of the window and lays itself out
/// against it, and these two are already spoken for — rows asked for past them
/// are laid out and then dropped off the bottom, which costs the keys row
/// first. Under-asking costs nothing: the transcript is above and fills what is
/// left.
const CHROME: usize = Head::ROWS + 1;

/// What came off the shelf: a model, and the rung marked under it.
///
/// `Option<usize>` rather than a rung index into nothing, because a model whose
/// provider serves no rung is taken with no rung and that is not a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shelved<T> {
    /// This came off the shelf, with the rung marked under it where there was
    /// one to mark.
    Took(T, Option<usize>),
    /// The shelf was left with nothing taken. What is owed is the caller's line
    /// saying so.
    Left,
    /// There was no room to stand one. The caller still owes an answer.
    Cramped,
}

/// What a shelf keeps between frames.
///
/// The line being typed, a mark per pane, and what the last frame narrowed —
/// which is written here by the frame that laid it out rather than by the key
/// that caused it, because narrowing is the caller's and this module is never
/// told what is on the shelf. The two counts are the same fact for the panes
/// nothing is taken out of: the keys need to know where each pane ends, and the
/// rows themselves are the caller's.
pub(super) struct Standing<M> {
    /// The search line.
    pub(super) query: Editor,
    /// Which pane the arrows walk.
    pub(super) pane: Pane,
    /// The mark in the pane of providers, counting the row that means all of
    /// them.
    pub(super) provider: usize,
    /// The mark in the pane of models, into `models`.
    pub(super) model: usize,
    /// The mark on the strip of rungs.
    pub(super) rung: usize,
    /// What the last frame narrowed the shelf to. The one thing here that comes
    /// back out.
    pub(super) models: Vec<M>,
    /// How many rows the pane of providers has.
    pub(super) providers: usize,
    /// How many rungs the strip has. None at all is a model that takes none.
    pub(super) rungs: usize,
}

/// Stands a shelf over the whole window and reads keys until one is taken.
///
/// `laid` is handed the room under the window's own chrome rather than the
/// window's height, because a shelf fills what it is given and the two rows at
/// the ends are not its to fill.
///
/// A shelf whose marked row is not there — nothing narrowed to, at the moment
/// Enter arrived — comes back [`Shelved::Cramped`], which is the answer a
/// caller already has for a shelf it could not stand: the listing. The keys
/// below refuse Enter on an empty shelf, so it is a miss that cannot happen
/// rather than a case with behaviour of its own.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on or read from.
pub(super) fn shelve<T: Terminal, M: Copy>(
    renderer: &mut Renderer<T>,
    style: Style,
    standing: &mut Standing<M>,
    mut laid: impl FnMut(&mut Standing<M>, usize, usize) -> (Vec<Row>, Option<Caret>),
    while_waiting: &mut dyn FnMut(&mut Renderer<T>) -> Result<(), Fatal>,
) -> Result<Shelved<M>, Fatal> {
    let ended = region::stand_while(
        renderer,
        |_| style,
        standing,
        |standing, columns, rows| laid(standing, columns, rows.saturating_sub(CHROME)),
        searching,
        while_waiting,
    )?;

    Ok(match ended {
        Ended::Took => standing
            .models
            .get(standing.model)
            .copied()
            .map_or(Shelved::Cramped, |took| {
                Shelved::Took(took, (standing.rungs > 0).then_some(standing.rung))
            }),
        Ended::Left => Shelved::Left,
        Ended::Cramped => Shelved::Cramped,
    })
}

/// What `arrived` does to a shelf.
///
/// Three shapes under one hand, so the keys are parted by what they are for
/// rather than by which pane has the mark. The down pair walks whichever pane
/// the mark is in, tab is what moves the mark between them, and the across pair
/// walks the strip of rungs — which is the one binding both readings of could
/// have had: the strip is a picture with a left and a right, and the search
/// line is not the thing being walked. Everything else the line has a use for
/// goes to the line, so a long query is still editable with Home, End and the
/// word keys.
///
/// The wheel is nothing here on purpose. A shelf is not a window over more than
/// it holds, so the wheel scrolls the transcript underneath it, which is what
/// every other standing component already does.
// An event token is handed over, not lent: the handler takes the one thing
// the reader produced, and a reference would say the caller kept a say in it.
#[allow(clippy::needless_pass_by_value)]
fn searching<M>(arrived: Pressed, standing: &mut Standing<M>) -> Moved {
    match arrived {
        // Nothing to take. Refused here rather than answered by the caller,
        // because a shelf that closed on a query matching nothing would be the
        // panel disagreeing with the search line the reader is looking at.
        Pressed::Key(Key::Enter) if standing.models.is_empty() => Moved::Still,
        Pressed::Key(Key::Enter) => Moved::Took,
        Pressed::Escape | Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,
        Pressed::Resized => Moved::Redraw,
        Pressed::Tab => {
            standing.pane = match standing.pane {
                Pane::Providers => Pane::Models,
                Pane::Models => Pane::Providers,
            };
            Moved::Redraw
        }
        Pressed::Up => stepped(standing, |at| at.checked_sub(1)),
        Pressed::Down => stepped(standing, |at| Some(at + 1)),
        Pressed::Key(Key::Left) => {
            let next = standing.rung.checked_sub(1);
            step(&mut standing.rung, next)
        }
        Pressed::Key(Key::Right) => {
            let next = Some(standing.rung + 1).filter(|next| *next < standing.rungs);
            step(&mut standing.rung, next)
        }
        Pressed::Pasted(pasted) => {
            let answered = standing.query.paste(&pasted);
            typed(answered, true, standing)
        }
        Pressed::Key(key) => {
            let rewrites = rewrites(key);
            let answered = standing.query.press(key);
            typed(answered, rewrites, standing)
        }
        _ => Moved::Still,
    }
}

/// Walks the mark of whichever pane it is in, and says what moved.
///
/// A step through the providers takes the other two marks with it: the shelf
/// beside them is about to be narrowed to something else, and a mark left where
/// it was would be standing on whatever slid under it.
fn stepped<M>(standing: &mut Standing<M>, next: impl Fn(usize) -> Option<usize>) -> Moved {
    let (at, count) = match standing.pane {
        Pane::Providers => (&mut standing.provider, standing.providers),
        Pane::Models => (&mut standing.model, standing.models.len()),
    };
    let moved = step(at, next(*at).filter(|next| *next < count));

    if moved == Moved::Redraw && standing.pane == Pane::Providers {
        standing.model = 0;
        standing.rung = 0;
    }

    moved
}

/// What a key the search line took does to the marks beside it.
///
/// `rewrites` is whether the key changed what has been typed rather than only
/// where the cursor is in it. A changed query is a different shelf and sends
/// every mark back to the top of it; somebody pressing Home to fix the front of
/// a word has not asked for the row under the mark to move.
///
/// The mark on the providers goes back with the other two, which is the whole
/// of what stops a query from being answered by a pane that disagrees with it.
/// Typing a vendor's name while the mark stands on a different vendor asks two
/// questions whose answer is nothing at all -- and the reader can see the line
/// they just typed, so it is the line that has to win.
fn typed<M>(answered: Typed, rewrites: bool, standing: &mut Standing<M>) -> Moved {
    if answered == Typed::Ignored {
        return Moved::Still;
    }

    if rewrites {
        standing.provider = 0;
        standing.model = 0;
        standing.rung = 0;
    }

    Moved::Redraw
}

/// Whether `key` changes what has been typed, rather than only where the cursor
/// is in it.
///
/// Written out rather than matched loosely so that a key added to the set has
/// to be answered here: the two halves differ by whether the shelf underneath
/// is about to be a different shelf, and a new key guessed into the wrong half
/// moves a mark nobody touched.
const fn rewrites(key: Key) -> bool {
    match key {
        Key::Char(_)
        | Key::Backspace
        | Key::Delete
        | Key::RubWord
        | Key::RubToStart
        | Key::RubToEnd => true,
        // The last three never reach the line — they are answered above — and
        // the rest move the cursor along a query that stays as it was.
        Key::Left
        | Key::Right
        | Key::Up
        | Key::Down
        | Key::WordLeft
        | Key::WordRight
        | Key::Home
        | Key::End
        | Key::Newline
        | Key::Enter
        | Key::Interrupt
        | Key::Eof => false,
    }
}

#[cfg(test)]
mod tests;
