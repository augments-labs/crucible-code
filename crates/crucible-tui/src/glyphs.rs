//! The characters a component draws itself with.
//!
//! Two sets, and which one is in force is answered once rather than asked per
//! row. Box drawing and the half blocks have been in CP437 and in every font
//! shipped this century, so a terminal that shows a row of hollow squares has a
//! font problem rather than an encoding one — and a font is not something this
//! process can ask about. What it can do is take an answer, which is why this
//! is a setting and never a guess.
//!
//! One set for every component, and that is the reason this sits beside them
//! rather than inside one: the welcome and the prompt draw the same corner, and
//! a terminal that shows a hollow square for it shows one in both places.

/// The name, as letters.
///
/// Beside the art rather than beside the component that prints it, because the
/// art spells this and a test below reads it back to prove that it still does.
pub(crate) const WORDMARK: &str = "CRUCIBLE";

/// The wordmark, drawn from half blocks.
///
/// crucible's own, drawn for this program. Every row is the same width, which
/// the welcome's own tests are what keep true — a row a column short leans the
/// whole mark, and a row a column long pushes it through the frame beside it.
///
/// Three columns to a letter and a blank column between them, which is what the
/// test below splits on to read back what this spells. `B` closes on the right
/// where `E` is open, and that one column is the whole difference between the
/// two: it is the column that was missing.
const ART: [&str; 3] = [
    "▄▄▄ ▄▄▄ █  █ ▄▄▄ █ ▄▄▄ █   ▄▄▄",
    "█   █▄▄ █  █ █   █ █▄█ █   █▄▄",
    "▀▄▄ █ █ ▀▄▄▀ ▀▄▄ █ █▄█ █▄▄ █▄▄",
];

/// Which characters a component draws its frame and its marks with.
///
/// Closed, because a third set would be a third answer to a question the
/// configuration asks in one word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Glyphs {
    /// Box drawing and the half blocks.
    #[default]
    Unicode,
    /// What is left when the font has neither.
    Ascii,
}

impl Glyphs {
    /// The corners a frame opens with: left, then right.
    pub(crate) fn top(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╭", "╮"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// The corners a frame closes with: left, then right.
    pub(crate) fn bottom(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╰", "╯"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// One column of an edge that runs across, and of a rule drawn inside one.
    pub(crate) fn horizontal(self) -> &'static str {
        match self {
            Self::Unicode => "─",
            Self::Ascii => "-",
        }
    }

    /// One row of an edge that runs down.
    pub(crate) fn vertical(self) -> &'static str {
        match self {
            Self::Unicode => "│",
            Self::Ascii => "|",
        }
    }

    /// The mark a line is typed after, and that its record keeps afterwards.
    ///
    /// One column either way. The prompt reserves exactly that much room for
    /// it, and a set whose mark were two columns wide would push the line into
    /// the edge beside it.
    pub(crate) fn caret(self) -> &'static str {
        match self {
            Self::Unicode => "›",
            Self::Ascii => ">",
        }
    }

    /// The small mark that parts one thing on a row from the next, and that
    /// opens an item in a list.
    pub(crate) fn dot(self) -> &'static str {
        match self {
            Self::Unicode => "·",
            Self::Ascii => "-",
        }
    }

    /// The mark that stands on a track, pointing at the rung in force.
    ///
    /// One column either way, like the caret: it is drawn into a track of a
    /// measured width, and a mark two columns wide would push the track's last
    /// column past the one the row was laid out for.
    pub(crate) fn mark(self) -> &'static str {
        match self {
            Self::Unicode => "\u{25b2}",
            Self::Ascii => "^",
        }
    }

    /// What stands where something did not fit.
    pub(crate) fn ellipsis(self) -> &'static str {
        match self {
            Self::Unicode => "…",
            Self::Ascii => "...",
        }
    }

    /// The wordmark drawn from half blocks, where the font has them.
    ///
    /// `None` is not a failure to draw the name — it is the name drawn as
    /// letters instead, which is what every form narrower than two columns uses
    /// as well.
    pub(crate) fn art(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Unicode => Some(&ART),
            Self::Ascii => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wordmark split into the letters it is made of, one string each.
    ///
    /// The columns blank in every row are what part one letter from the next,
    /// which is the same thing the eye reads them by.
    fn letters() -> Vec<String> {
        let rows: Vec<Vec<char>> = ART.iter().map(|row| row.chars().collect()).collect();
        let wide = rows.iter().map(Vec::len).max().unwrap_or_default();

        let columns: Vec<String> = (0..wide)
            .map(|at| {
                rows.iter()
                    .map(|row| row.get(at).copied().unwrap_or(' '))
                    .collect()
            })
            .collect();

        columns
            .split(|column| column.chars().all(char::is_whitespace))
            .filter(|letter| !letter.is_empty())
            .map(|letter| letter.join("/"))
            .collect()
    }

    #[test]
    fn no_two_letters_of_the_wordmark_are_drawn_the_same() {
        // The defect this catches: `B` was drawn as a second `E`, so the first
        // thing on screen spelled the program's name wrong. Letters that are
        // the same letter are drawn alike; letters that are not, are not — and
        // the wordmark is the one place where spelling is a picture, so nothing
        // about widths or colours can see this go wrong.
        let name: Vec<char> = WORDMARK.chars().collect();
        let drawn = letters();

        assert_eq!(drawn.len(), name.len(), "{drawn:?}");

        for (letter, art) in name.iter().zip(&drawn) {
            for (other, theirs) in name.iter().zip(&drawn) {
                assert_eq!(
                    art == theirs,
                    letter == other,
                    "{letter} is {art}, {other} is {theirs}"
                );
            }
        }
    }
}
