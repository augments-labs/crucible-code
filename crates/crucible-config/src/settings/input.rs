//! What the layers together say the keyboard does.
//!
//! One question so far, and it is here rather than under `output` because it is
//! about what a reader presses rather than about what they are shown. Nothing
//! in this module touches a terminal: it turns a string into a value, and the
//! prompt one crate up decides what to do with the press.

use super::Settings;

impl Settings {
    /// Which press sends a prompt, when nothing else says.
    #[must_use]
    pub fn sending(&self) -> Option<Sending> {
        Sending::read(self.input("send")?)
    }

    /// One string out of the `input` block.
    fn input(&self, key: &str) -> Option<&str> {
        self.value.get("input")?.get(key)?.as_str()
    }
}

/// Which press ends a prompt, and which one opens a line under it.
///
/// Asked rather than detected because the answer is not a fact about the
/// terminal that a program can read back — it is a fact about what the terminal
/// keeps for itself, and a terminal that swallows a press reports nothing at
/// all. The two answers are the two arrangements that work on a keyboard whose
/// only reliable presses are Return and a modified Return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sending {
    /// Return sends; a modified Return opens a line.
    #[default]
    Enter,
    /// A modified Return sends; Return opens a line.
    ///
    /// For a terminal that keeps every modified Return for itself, where the
    /// arrangement above leaves no way to write a second line.
    AltEnter,
}

impl Sending {
    /// Whether a plain Return opens a line rather than sending.
    #[must_use]
    pub fn opens_line(self) -> bool {
        matches!(self, Self::AltEnter)
    }

    /// Reads one of [`shape::SEND`](crate::shape::SEND).
    ///
    /// `None` for anything else, which the shape refused before this could be
    /// reached — the test below is what keeps "cannot arrive" true as the set
    /// changes.
    fn read(found: &str) -> Option<Self> {
        match found {
            "enter" => Some(Self::Enter),
            "altEnter" => Some(Self::AltEnter),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};
    use crate::shape;

    use super::*;

    #[test]
    fn the_press_that_sends_is_read_back_as_the_arrangement_it_names() {
        let user = Document::sample(r#"{"input": {"send": "altEnter"}}"#, Origin::User);

        let settings = Settings::resolve(vec![user]);

        assert_eq!(settings.sending(), Some(Sending::AltEnter));
        assert!(settings.sending().is_some_and(Sending::opens_line));
    }

    #[test]
    fn the_nearest_layer_that_named_a_press_wins_it() {
        // Which press sends is a fact about the keyboard in front of somebody,
        // so the layer nearest them is the one that knows it.
        let user = Document::sample(r#"{"input": {"send": "altEnter"}}"#, Origin::User);
        let local = Document::sample(r#"{"input": {"send": "enter"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);

        assert_eq!(settings.sending(), Some(Sending::Enter));
        assert!(!settings.sending().is_some_and(Sending::opens_line));
    }

    #[test]
    fn a_keyboard_no_layer_mentioned_is_left_for_the_wiring_to_decide() {
        assert_eq!(Settings::resolve(Vec::new()).sending(), None);
    }

    #[test]
    fn every_answer_the_document_accepts_reads_back_as_a_value() {
        // The shape decides what a document may say and this module decides
        // what each answer means, so the two lists have to agree. Renaming an
        // answer in one and not the other is a setting that stops working with
        // no error anywhere.
        for name in shape::SEND {
            assert!(Sending::read(name).is_some(), "send: {name}");
        }
    }
}
