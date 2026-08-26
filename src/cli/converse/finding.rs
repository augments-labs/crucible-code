//! What the keys do to a session picker, and what a query keeps on it.
//!
//! The picker is the one full-shell component: a search line, the sessions the
//! query left, and a window over the tail of whichever one is marked. What it
//! looks like is `crucible_tui::Picker`'s; which sessions a query answers and
//! what each key moves is decided here, where the domain is — the crate that
//! draws is never told what a session is.
//!
//! The keys follow the shelf's grammar where the shapes agree: arrows walk the
//! list, typing lands in the search line without any focus being moved, a
//! click takes what is lit and only what is lit. They part from it in two
//! places, both deliberate. The wheel is answered rather than left to the
//! transcript, because this component *is* a window over more than it holds —
//! over the list on one side and the marked session's tail on the other, and
//! the pane under the pointer is the one that moves. And Escape is layered
//! rather than immediate: it closes the rename if one is open, then clears the
//! query if one is typed, and only then hands the screen back — each press
//! undoes the innermost thing the reader built, instead of all of them.
//!
//! Renaming stays in the reader's hands until Enter. The handler here stages
//! the accepted title on the state rather than writing it anywhere: what a
//! title is written into is the session index, which is the caller's to reach.

use crucible_tui::{Editor, Hit, Key, Pressed, Typed};

use super::picking;
use super::region::{Moved, step};

/// What the picker keeps between frames.
///
/// The narrowing is written here by the frame that laid it out, the way the
/// shelf's is: which rows the query left, and how far back the preview window
/// may still go, are facts about a picture at a width, and the frame that drew
/// it is the party that knows them. The keys read them back.
pub(super) struct Standing {
    /// The search line.
    pub(super) query: Editor,
    /// The title being typed over the marked row, while a rename is open.
    pub(super) renaming: Option<Editor>,
    /// Whether the last rename Enter was refused for being empty.
    pub(super) refused: bool,
    /// A title Enter accepted, staged for the caller to write down.
    pub(super) saving: Option<String>,
    /// Which sessions the query left, as indices into the caller's list, in
    /// the order the list pane shows them. Written by the layout.
    pub(super) found: Vec<usize>,
    /// The mark, into `found`.
    pub(super) marked: usize,
    /// How many rows back from the tail the preview window is standing.
    pub(super) behind: usize,
    /// The most `behind` can be at this height. Written by the layout.
    pub(super) over: usize,
    /// Where the pointer is resting, in the picker's own rows and the window's
    /// columns, or `None` where it rests on nothing the picker drew.
    pub(super) pointer: Option<(usize, usize)>,
    /// What the last frame found under that pointer, written by the frame that
    /// drew it. A click and the wheel read this rather than working the place
    /// out a second time.
    pub(super) lit: Option<Hit>,
}

/// Whether a session whose title and branch these are answers `query`.
///
/// Case-folded substring against both names, because both are on the row:
/// somebody typing `fix-the-caret` is reaching for a branch the same way
/// somebody typing the first words of a prompt is reaching for a title, and
/// nothing on screen says which of the two they are looking at. An empty
/// query answers everything.
pub(super) fn matches(title: &str, branch: Option<&str>, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    // Folded on both sides. Folding the query alone answers the row's spelling
    // and not the reader's; folding the row alone answers the reader's and not
    // the row's, and there is no third spelling either of them agreed to.
    let query = query.to_lowercase();

    [Some(title), branch]
        .into_iter()
        .flatten()
        .any(|name| name.to_lowercase().contains(&query))
}

/// What one key does to the picker.
///
/// `titled` is the marked session's current title, or `None` where the
/// narrowing left nothing to mark — it is what a rename opens over, and the
/// caller reads it off its own list because this module never holds one.
pub(super) fn sifting(arrived: Pressed, standing: &mut Standing, titled: Option<&str>) -> Moved {
    // While a rename is open every key is the rename's, except the ways out.
    // The query and the marks hold still underneath it. Taken out and put
    // back rather than borrowed, because half the arms close it.
    if let Some(mut renaming) = standing.renaming.take() {
        return match arrived {
            // Ctrl+C and Ctrl+D close the whole thing: they are the reader
            // leaving, not stepping back.
            Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,

            // The innermost thing the reader built, undone; the query under
            // it is theirs and stays.
            Pressed::Escape => {
                standing.refused = false;
                Moved::Redraw
            }

            Pressed::Key(Key::Enter) => {
                let title = renaming.text().trim().to_owned();
                if title.is_empty() {
                    // A session with no title falls back to its first prompt,
                    // so an empty rename could not stick; saying so in place
                    // beats silently keeping the old name.
                    standing.refused = true;
                    standing.renaming = Some(renaming);
                } else {
                    // Staged rather than written: the index the title goes
                    // into is the caller's to reach.
                    standing.saving = Some(title);
                    standing.refused = false;
                }
                Moved::Redraw
            }

            Pressed::Pasted(pasted) => {
                let answered = renaming.paste(&pasted);
                standing.renaming = Some(renaming);
                moved(answered)
            }
            Pressed::Key(key) => {
                let answered = renaming.press(key);
                standing.renaming = Some(renaming);
                moved(answered)
            }

            other => {
                standing.renaming = Some(renaming);
                match other {
                    Pressed::Resized => Moved::Redraw,
                    _ => Moved::Still,
                }
            }
        };
    }

    match arrived {
        Pressed::Key(Key::Interrupt | Key::Eof) => Moved::Left,

        // Layered: with no rename open, Escape undoes the query next, and
        // only with nothing left to undo does it hand the screen back. The
        // clearing walks the mark back to the top, because the row number it
        // stood on was the narrowing's and the narrowing is gone.
        Pressed::Escape => {
            if standing.query.is_empty() {
                Moved::Left
            } else {
                standing.query.clear();
                standing.marked = 0;
                standing.behind = 0;
                Moved::Redraw
            }
        }

        // Nothing to take. Refused here rather than answered by the caller,
        // because a picker that closed on a query matching nothing would be
        // the list disagreeing with the search line the reader is looking at.
        Pressed::Key(Key::Enter) if standing.found.is_empty() => Moved::Still,
        Pressed::Key(Key::Enter) => Moved::Took,

        // A rename opens over the title the row already has: most renames are
        // edits, and an empty line would make the reader retype the part they
        // wanted to keep. No marked row, nothing to open over.
        Pressed::Rename => titled.map_or(Moved::Still, |title| {
            let mut renaming = Editor::new();
            renaming.put(title);
            standing.renaming = Some(renaming);
            Moved::Redraw
        }),

        Pressed::Resized => Moved::Redraw,

        Pressed::Up => stepped(standing, |at| at.checked_sub(1)),
        Pressed::Down => stepped(standing, |at| Some(at + 1)),

        // The pane under the pointer is the one the wheel moves: the list
        // walks its mark, the preview walks its window over the tail, and a
        // wheel over neither goes back to the loop, whose transcript it
        // scrolls. What is under the pointer is read off the last frame,
        // because which pane a place falls on is a fact about the picture.
        Pressed::Scrolled { back } => match standing.lit {
            Some(Hit::Session(_)) => {
                if back {
                    stepped(standing, |at| at.checked_sub(1))
                } else {
                    stepped(standing, |at| Some(at + 1))
                }
            }
            Some(Hit::Preview) => {
                let next = if back {
                    Some(standing.behind + 1).filter(|next| *next <= standing.over)
                } else {
                    standing.behind.checked_sub(1)
                };
                step(&mut standing.behind, next)
            }
            Some(Hit::Nothing | Hit::Search) | None => Moved::Still,
        },

        // Where the pointer is, rather than what is under it: which row of
        // which pane a place falls on is a fact about the picture, and the
        // picture is laid out a layer up.
        Pressed::Hovered { row, column } => {
            let next = (row != usize::MAX).then_some((row, column));
            if standing.pointer == next {
                return Moved::Still;
            }
            standing.pointer = next;
            Moved::Redraw
        }

        // A click takes what is lit, and never what is under the place the
        // click reports. A click arriving where nothing is lit lights it and
        // stops there, and the next one takes it.
        Pressed::Clicked { row, column } => {
            if standing.pointer != Some((row, column)) {
                standing.pointer = Some((row, column));
                return Moved::Redraw;
            }
            clicked(standing)
        }

        Pressed::Pasted(pasted) => {
            let answered = standing.query.paste(&pasted);
            queried(answered, true, standing)
        }
        Pressed::Key(key) => {
            let rewrites = picking::rewrites(key);
            let answered = standing.query.press(key);
            queried(answered, rewrites, standing)
        }

        _ => Moved::Still,
    }
}

/// Steps the mark and, where it moved, reopens the preview at its tail: the
/// window the reader had scrolled belonged to the session they were reading.
fn stepped(standing: &mut Standing, next: impl Fn(usize) -> Option<usize>) -> Moved {
    let next = next(standing.marked).filter(|next| *next < standing.found.len());
    let moved = step(&mut standing.marked, next);
    if moved == Moved::Redraw {
        standing.behind = 0;
    }
    moved
}

/// What a click on a lit place does: marks a session, takes the marked one,
/// and chooses nothing anywhere else — the search line already has the keys,
/// and the preview is a window, not a thing to take.
fn clicked(standing: &mut Standing) -> Moved {
    match standing.lit {
        Some(Hit::Session(at)) => {
            if at == standing.marked {
                Moved::Took
            } else {
                standing.marked = at;
                standing.behind = 0;
                Moved::Redraw
            }
        }
        Some(Hit::Nothing | Hit::Search | Hit::Preview) | None => Moved::Still,
    }
}

/// What the search line's answer does to the marks: a key that changed the
/// query narrowed to a different list, so the mark walks back to the top and
/// the preview reopens at its tail; a cursor move leaves both.
fn queried(answered: Typed, rewrites: bool, standing: &mut Standing) -> Moved {
    if answered == Typed::Ignored {
        return Moved::Still;
    }

    if rewrites {
        standing.marked = 0;
        standing.behind = 0;
    }

    Moved::Redraw
}

/// What an editor's answer means to the frame: an ignored key moved nothing.
fn moved(answered: Typed) -> Moved {
    if answered == Typed::Ignored {
        Moved::Still
    } else {
        Moved::Redraw
    }
}

#[cfg(test)]
mod tests;
