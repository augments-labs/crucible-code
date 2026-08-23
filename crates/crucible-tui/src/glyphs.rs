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
//!
//! The transcript is drawn outside this crate and out of the same set, which is
//! why the marks that appear in a line of text are public where the frame parts
//! are not. A caller composing its own row is the reason the setting exists at
//! all — a font missing a corner is missing the one a tool call hangs its result
//! off too — and the alternative is a second answer to a question the
//! configuration asks in one word.

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
    ///
    /// Public because the prompt is not the only frame: the queue panel borders
    /// itself the same way, and a second spelling of a corner would read as a
    /// second kind of box.
    #[must_use]
    pub fn top(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╭", "╮"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// The corners a frame closes with: left, then right.
    ///
    /// Public for the reason [`Glyphs::top`] is.
    #[must_use]
    pub fn bottom(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╰", "╯"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// One column of an edge that runs across, and of a rule drawn inside one.
    /// Public because a picker draws a rule the width of the terminal across
    /// the top of itself, and that rule is the whole picture's rather than any
    /// one component's.
    #[must_use]
    pub fn horizontal(self) -> &'static str {
        match self {
            Self::Unicode => "─",
            Self::Ascii => "-",
        }
    }

    /// One row of an edge that runs down.
    ///
    /// Public for the reason [`Glyphs::top`] is.
    #[must_use]
    pub fn vertical(self) -> &'static str {
        match self {
            Self::Unicode => "│",
            Self::Ascii => "|",
        }
    }

    /// Where a rule that runs across meets an edge that runs down.
    ///
    /// One column in both sets, as [`Glyphs::horizontal`] and
    /// [`Glyphs::vertical`] are: a table's rule is laid out against the same
    /// column widths as the rows above and below it, so a crossing that cost
    /// two would put every column after it a place out.
    #[must_use]
    pub fn crossing(self) -> &'static str {
        match self {
            Self::Unicode => "┼",
            Self::Ascii => "+",
        }
    }

    /// The two ends of an edge that runs down inside a frame: where it begins
    /// at the edge across the top, then where it ends at the edge across the
    /// bottom.
    ///
    /// A pair rather than two methods, for the reason [`Glyphs::top`] is one:
    /// what a frame divided down the middle needs is both ends of the one
    /// edge, and a set that spelled them apart would let a divider be drawn
    /// closed at the top and open at the bottom.
    ///
    /// Neither is a corner. [`Glyphs::top`] and [`Glyphs::bottom`] turn a
    /// frame at its outside, and these two open a second edge inside one —
    /// which is the whole difference between a frame with two panes in it and
    /// two frames drawn touching.
    ///
    /// One column in both sets, for the reason [`Glyphs::crossing`] is: the
    /// panes either side are laid out against fixed column widths, so a joint
    /// costing two would put every column after it a place out, in that set
    /// alone.
    #[must_use]
    pub fn dividing(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("┬", "┴"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// Where a rule that runs across meets the edge down the left of a frame,
    /// then the edge down its right.
    ///
    /// [`Glyphs::crossing`] is this rule meeting an edge in the middle, and
    /// these are the same rule reaching the outside. All three are one mark in
    /// the ascii set, which has one joint for every purpose and no way to say
    /// which side of a frame it is on.
    ///
    /// A pair, and one column apiece, for the reasons [`Glyphs::dividing`]
    /// gives.
    #[must_use]
    pub fn joining(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("├", "┤"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// The mark a line is typed after, and that its record keeps afterwards.
    ///
    /// One column either way. The prompt reserves exactly that much room for
    /// it, and a set whose mark were two columns wide would push the line into
    /// the edge beside it.
    ///
    /// Public because the prompt is not the only box a line is typed into: the
    /// sign-in asks for a pasted authorization behind one of these, and a
    /// second answer to "what is typed after" would make the two boxes look
    /// like two kinds of thing.
    #[must_use]
    pub fn caret(self) -> &'static str {
        match self {
            Self::Unicode => "›",
            Self::Ascii => ">",
        }
    }

    /// The small mark that parts one thing on a row from the next, and that
    /// opens an item in a list.
    #[must_use]
    pub fn dot(self) -> &'static str {
        match self {
            Self::Unicode => "·",
            Self::Ascii => "-",
        }
    }

    /// What stands in for one character of a line that is not shown.
    ///
    /// One column in both sets, which is what lets a box count the cursor's
    /// column off the number of characters it is standing in for — the one
    /// thing a box drawing these is allowed to know about the line.
    #[must_use]
    pub fn hidden(self) -> &'static str {
        match self {
            Self::Unicode => "•",
            Self::Ascii => "*",
        }
    }

    /// The keys that step along a track, drawn as marks: left, then right.
    ///
    /// Named under a track rather than drawn on one, which is why they are a
    /// pair rather than two marks that happen to be nearby: what somebody is
    /// looking for is the two keys either side of the one their hand is on, and
    /// a set that spelled one of them out and drew the other would break that.
    #[must_use]
    pub fn stepping(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("←", "→"),
            Self::Ascii => ("<", ">"),
        }
    }

    /// The pair that closes around one word of several, holding the mark.
    ///
    /// A pair rather than one mark repeated, and not [`Glyphs::stepping`]:
    /// those are two keys named under a track, and these are two sides of one
    /// word. A row of words with a mark only in front of one says which word
    /// starts there; a word with a side on each of it says which word it is.
    #[must_use]
    pub fn bracketing(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("\u{2039}", "\u{203a}"),
            Self::Ascii => ("<", ">"),
        }
    }

    /// The keys that walk down a list, drawn as marks: up, then down.
    ///
    /// A pair of its own rather than [`Glyphs::stepping`] read sideways. A list
    /// is walked down where a track is stepped along, and a set that spelled
    /// both with the one pair would name a direction the picture does not have —
    /// which is how somebody learns not to trust the picture.
    #[must_use]
    pub fn walking(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("↑", "↓"),
            Self::Ascii => ("^", "v"),
        }
    }

    /// The long mark that stands between a thing and what is said about it.
    ///
    /// Two columns under `ascii` where [`Glyphs::dot`] is one, and that is the
    /// difference between them rather than an accident of the set: this one
    /// divides a name from a sentence, and a lone hyphen there reads as part of
    /// whichever word it lands against.
    #[must_use]
    pub fn dash(self) -> &'static str {
        match self {
            Self::Unicode => "—",
            Self::Ascii => "--",
        }
    }

    /// The mark that opens the line for a tool call.
    ///
    /// Filled, where [`Glyphs::dot`] is not. The two sit within a row of each
    /// other saying different things — this one opens a call and is the column
    /// its result hangs off, and that one is punctuation — so they are drawn
    /// apart in both sets rather than only in the one with the glyphs to spare.
    #[must_use]
    pub fn called(self) -> &'static str {
        match self {
            Self::Unicode => "●",
            Self::Ascii => "*",
        }
    }

    /// The corner a tool call's result hangs under.
    ///
    /// Square where a frame's corner is round, so a result row and the bottom
    /// of the box never read as the same shape. The ascii set has one corner
    /// for every purpose and this is it, which costs that distinction — but a
    /// terminal drawing hollow squares has already lost more than that.
    #[must_use]
    pub fn hangs(self) -> &'static str {
        match self {
            Self::Unicode => "└",
            Self::Ascii => "+",
        }
    }

    /// The mark that says what came back was a failure.
    #[must_use]
    pub fn failed(self) -> &'static str {
        match self {
            Self::Unicode => "✗",
            Self::Ascii => "x",
        }
    }

    /// The mark on the one task a plan is under way on.
    ///
    /// Filled where the next one is hollow, which is the whole of what the two
    /// have to say apart from each other. The three below are read down a
    /// column of adjacent rows rather than met one at a time, so what matters
    /// about them is that no two are alike — and the test at the bottom is
    /// where that is checked, in both sets.
    #[must_use]
    pub fn doing(self) -> &'static str {
        self.filled()
    }

    /// A mark with its middle in, whatever it is standing for.
    ///
    /// The shape rather than a meaning, for the callers that want a shape: a
    /// bar is filled and hollow marks in a row, and asking for the one that
    /// means *a task somebody is on* would be borrowing another picture's word
    /// for a rectangle.
    #[must_use]
    pub fn filled(self) -> &'static str {
        match self {
            Self::Unicode => "■",
            Self::Ascii => "*",
        }
    }

    /// And the same mark with its middle out.
    #[must_use]
    pub fn hollow(self) -> &'static str {
        match self {
            Self::Unicode => "□",
            Self::Ascii => "-",
        }
    }

    /// A semantic place on the transcript map.
    ///
    /// Hollow so the filled mark showing the current place remains distinct,
    /// and unlike the horizontal rail in both sets. One column because it
    /// replaces one cell of that rail rather than widening it.
    pub(crate) fn landmark(self) -> &'static str {
        match self {
            Self::Unicode => "○",
            Self::Ascii => "o",
        }
    }

    /// The mark on a task nobody has started.
    #[must_use]
    pub fn open(self) -> &'static str {
        self.hollow()
    }

    /// The mark on a task that is finished.
    ///
    /// The same letter as [`Glyphs::failed`] in the ascii set, and deliberately
    /// so: `x` is what a box is ticked with where there is no tick to be had,
    /// and the two never appear on the same picture — one is a plan and the
    /// other is what a tool answered.
    #[must_use]
    pub fn done(self) -> &'static str {
        match self {
            Self::Unicode => "✓",
            Self::Ascii => "x",
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

    /// The face the mark on a running turn wears on `beat`.
    ///
    /// Four of them, turned through in order and back to the first. Every face
    /// is one column wide in both sets, and none of them is a character a
    /// terminal is entitled to draw two columns wide — which is what makes this
    /// a mark changing rather than a row changing width four times a second.
    pub(crate) fn turning(self, beat: u64) -> &'static str {
        match (self, beat % 4) {
            (Self::Unicode, 0) => "\u{2733}",
            (Self::Unicode, 1) => "\u{273b}",
            (Self::Unicode, 2) => "\u{273a}",
            (Self::Unicode, _) => "\u{2731}",
            (Self::Ascii, 0) => "|",
            (Self::Ascii, 1) => "/",
            (Self::Ascii, 2) => "-",
            (Self::Ascii, _) => "\\",
        }
    }

    /// The mark that says a number is what came back rather than what went out.
    ///
    /// It stands in for the word, which is the only reason the count fits on a
    /// row that already has three other things to say.
    pub(crate) fn down(self) -> &'static str {
        match self {
            Self::Unicode => "\u{2193}",
            Self::Ascii => "v",
        }
    }

    /// What stands where something did not fit.
    ///
    /// Three columns in the ascii set against one in the other, and the only
    /// mark here whose width is not the same in both. A caller clipping a line
    /// to a width owes the room this takes rather than the one column the
    /// unicode set happens to need.
    #[must_use]
    pub fn ellipsis(self) -> &'static str {
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

    use crate::columns;

    #[test]
    fn the_marks_a_box_counts_columns_off_are_one_column_in_both_sets() {
        // A box that draws a line it may not show counts the cursor's column
        // off the characters it stood in for, and the box it is typed into
        // reserves exactly one column for its mark. Either of these two
        // columns wide in one of the sets would park the cursor short of the
        // line's end, in that set only — which is the one thing about a line
        // nobody can read that is still visible.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            assert_eq!(columns(glyphs.caret()), 1, "{glyphs:?}");
            assert_eq!(columns(glyphs.hidden()), 1, "{glyphs:?}");
        }
    }

    #[test]
    fn the_joints_of_a_frame_are_four_marks_and_none_is_the_edge_it_interrupts() {
        // A frame with a pane divider down it meets a rule across it in four
        // places, and each is a different shape: the divider's top and bottom,
        // and the rule's left and right. Two of them drawn alike is a divider
        // that reads as open at one end; one of them drawn as the plain edge
        // it interrupts is no joint at all, which is the same picture as
        // having drawn nothing.
        //
        // One column apiece because the panes are laid out against fixed
        // column widths -- a joint costing two would put every column after it
        // a place out, in that set only.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let (top, bottom) = glyphs.dividing();
            let (left, right) = glyphs.joining();
            let joints = [top, bottom, left, right];

            for joint in joints {
                assert_eq!(columns(joint), 1, "{glyphs:?}: {joint:?}");
                assert_ne!(joint, glyphs.horizontal(), "{glyphs:?}: {joint:?}");
                assert_ne!(joint, glyphs.vertical(), "{glyphs:?}: {joint:?}");
            }
        }

        // Apart from each other only where the set has the glyphs to spare.
        // The ascii set answers "+" to every joint, as it already does for
        // `crossing` and both corners, and that is what it has.
        let (top, bottom) = Glyphs::Unicode.dividing();
        let (left, right) = Glyphs::Unicode.joining();
        let joints = [top, bottom, left, right, Glyphs::Unicode.crossing()];
        for (at, joint) in joints.iter().enumerate() {
            assert!(
                !joints.iter().skip(at + 1).any(|other| other == joint),
                "{joint:?} twice"
            );
        }
    }

    #[test]
    fn every_joint_of_a_frame_is_the_one_mark_the_ascii_set_has_for_one() {
        // A set that answered "+" to a crossing and something else to a tee
        // would draw a frame whose joints do not look like each other's, on
        // the terminal that has the least to work with.
        let (top, bottom) = Glyphs::Ascii.dividing();
        let (left, right) = Glyphs::Ascii.joining();
        for joint in [top, bottom, left, right] {
            assert_eq!(joint, Glyphs::Ascii.crossing(), "{joint:?}");
        }
    }

    #[test]
    fn the_pair_that_closes_around_a_word_is_one_column_a_side_in_both_sets() {
        // The words on a track are laid out against fixed columns and the mark
        // walks between them, so a side costing two columns in one set would
        // shove every word after the marked one a place along -- and only on
        // the terminal that has that set. Two sides that were the same mark
        // would say a word starts and ends with the same thing, which is a
        // picture with no left and no right in it.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let (opens, closes) = glyphs.bracketing();

            assert_eq!(columns(opens), 1, "{glyphs:?}: {opens:?}");
            assert_eq!(columns(closes), 1, "{glyphs:?}: {closes:?}");
            assert_ne!(opens, closes, "{glyphs:?}");
        }
    }

    #[test]
    fn the_two_pairs_of_arrows_are_four_different_marks_in_both_sets() {
        // Each pair is named under the thing it moves, so a mark shared between
        // them would put the same key under two pictures that move differently.
        // The one column apiece is what lets a footer name either pair without
        // the row it sits on being measured twice.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let (left, right) = glyphs.stepping();
            let (up, down) = glyphs.walking();
            let marks = [left, right, up, down];

            for mark in marks {
                assert_eq!(columns(mark), 1, "{glyphs:?}: {mark:?}");
            }
            for (at, mark) in marks.iter().enumerate() {
                assert!(
                    !marks.iter().skip(at + 1).any(|other| other == mark),
                    "{glyphs:?}: {mark:?} twice"
                );
            }
        }
    }

    #[test]
    fn the_three_states_of_a_task_are_three_different_marks_in_both_sets() {
        // They are read down a column, one row apart, and the mark is the only
        // thing on the row that says which state a task is in -- colour says it
        // again and a terminal with none says it not at all. Two states drawn
        // alike is a panel that cannot be read on that terminal, in that set
        // only, which is the failure nobody sees on the one they are using.
        //
        // One column apiece for the reason the caret is: the words after the
        // marks start in the column the gutter reserved, and a mark two columns
        // wide in one set would indent that set's rows by one.
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let marks = [glyphs.doing(), glyphs.open(), glyphs.done()];

            for mark in marks {
                assert_eq!(columns(mark), 1, "{glyphs:?}: {mark:?}");
            }
            for (at, mark) in marks.iter().enumerate() {
                assert!(
                    !marks.iter().skip(at + 1).any(|other| other == mark),
                    "{glyphs:?}: {mark:?} twice"
                );
            }
        }
    }

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
