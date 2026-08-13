//! What the prompt knows about a permission mode beyond the mode itself.
//!
//! The mode is core's, and so is every word it is spelled with: `allowEdits` is
//! what you type and `allow edits on` is what you read, and both live beside the
//! enum so that a mode added later cannot be given one and not the other.
//!
//! What lives here is what only a terminal needs. The colour a mode is drawn in
//! is a fact about this renderer rather than about permission, and the way back
//! from a typed word to the mode it names is a fact about `/mode` — no other
//! caller ever has a word and wants a mode. Both would be dead weight compiled
//! into every crate if they sat one level down.

use crucible_core::Mode;
use crucible_tui::{Glyphs, Slot};

/// Every mode, in the order the key steps through them.
///
/// A list beside a ring, which is the same fact twice. The walk below refuses
/// to let them drift: a mode added to the ring and not to this list would be
/// one `/mode` never offered and never accepted, and nothing else would notice.
const RING: [Mode; 3] = [Mode::Ask, Mode::AllowEdits, Mode::FullAccess];

/// The modes there are, written out, for the row saying what `/mode` takes.
///
/// A literal rather than a join, because it is on screen for as long as the
/// menu is filtering and a row built per keystroke is work done to produce the
/// bytes that were already there. The test below joins the ring and asserts
/// what the join comes to.
pub(super) const fn ring(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "ask · allowEdits · fullAccess",
        Glyphs::Ascii => "ask, allowEdits, fullAccess",
    }
}

/// The mode that word names, or `None` where it names none.
///
/// Compared against the spelling configuration uses, which is the spelling on
/// screen: what `/mode` lists is what `/mode` takes back.
pub(super) fn named(said: &str) -> Option<Mode> {
    RING.into_iter().find(|mode| mode.to_string() == said)
}

/// The colour a mode's border and its sentence are drawn in.
///
/// A ramp: the quiet state is quiet, and the border only starts pulling the eye
/// as the mode gets more permissive. Not red at the top of it — red is what a
/// denial and a failed tool call are already spelled in, and full access is a
/// choice somebody made rather than something that went wrong.
pub(super) const fn tone(mode: Mode) -> Slot {
    match mode {
        Mode::Ask => Slot::Quiet,
        Mode::AllowEdits => Slot::AllowEdits,
        Mode::FullAccess => Slot::FullAccess,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_holds_every_mode_of_the_ring_once_and_in_its_order() {
        let mut stepped = Mode::default();

        for listed in RING {
            assert_eq!(listed, stepped, "the list and the ring part ways here");
            stepped = stepped.next();
        }

        // Back where the walk started, so the list is not a mode short of the
        // ring it claims to be.
        assert_eq!(stepped, Mode::default());
    }

    #[test]
    fn every_mode_is_reached_by_the_word_the_row_under_the_box_shows() {
        for mode in RING {
            assert_eq!(named(&mode.to_string()), Some(mode));
        }
    }

    #[test]
    fn a_word_that_names_no_mode_names_none() {
        // Spelled the way it is typed rather than the way it reads, and matched
        // whole: a mode is named exactly or it is not named.
        for said in ["", "Ask", "ask mode on", "allowedits", "full"] {
            assert_eq!(named(said), None, "{said:?}");
        }
    }

    #[test]
    fn the_row_saying_what_mode_takes_lists_the_ring_it_takes_from() {
        for (glyphs, between) in [(Glyphs::Unicode, " · "), (Glyphs::Ascii, ", ")] {
            let joined = RING
                .iter()
                .map(Mode::to_string)
                .collect::<Vec<_>>()
                .join(between);

            assert_eq!(ring(glyphs), joined, "{glyphs:?}");
        }
    }
}
