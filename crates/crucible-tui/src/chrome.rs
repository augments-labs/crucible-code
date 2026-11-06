//! The frame a drawn block stands in: an edge across the top that names it,
//! a gutter in front of every row, and an edge across the bottom.
//!
//! One owner for the look of it, so a fence streamed as code and a block drawn
//! whole later wear the same frame and a change to the look is made once. The
//! top edge carries the label -- what the block is -- and, where the block was
//! not drawn as what it is, the reason, so a reader is never left to wonder why
//! a diagram is showing as its source.
//!
//! Everything here is quiet and none of it knows what is inside the frame:
//! how a block's own rows are laid out, and how they wrap, is the block's.
//!
//! The label is text a model chose, so it is drawn as the reader hands it on:
//! whoever makes a [`Label`] has already cleaned what it was made from.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width;

/// The most columns of a block's own name that a label spends before it is cut
/// short. Wide enough for any language or diagram name anybody uses, and narrow
/// enough that the reason a block was not drawn still fits beside it on a
/// window of ordinary width.
const NAME_COLUMNS: usize = 32;

/// What a frame's top edge names the block as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// What the block is called, already cut to [`NAME_COLUMNS`].
    name: String,
    /// The set the name was cut for, which is what its ellipsis is drawn with.
    glyphs: Glyphs,
}

impl Label {
    /// The label for a block called `name`, which is cut short with an
    /// ellipsis where it is longer than a label may be.
    ///
    /// `name` is taken as it is: a model's text has to be cleaned of what a
    /// terminal would act on before it gets here, by whoever read it.
    #[must_use]
    pub fn new(name: &str, glyphs: Glyphs) -> Self {
        let name = if width::columns(name) > NAME_COLUMNS {
            let room = NAME_COLUMNS.saturating_sub(width::columns(glyphs.ellipsis()));
            format!("{}{}", width::clip(name, room), glyphs.ellipsis())
        } else {
            name.to_owned()
        };
        Self { name, glyphs }
    }
}

/// Why a block was not drawn as what it is, and is shown as it was written.
///
/// Closed, so that whoever draws a block for a reason of its own has to add the
/// reason here, and everything that tells a reader about one decides what to do
/// with the new case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undrawn {
    /// It ran past the most a block of its kind is held for.
    PastCap,
}

impl Undrawn {
    /// The reason, as a reader is told it.
    #[must_use]
    pub fn words(self) -> &'static str {
        match self {
            Self::PastCap => "longer than this can hold",
        }
    }
}

/// The frame around one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// What the block is.
    pub label: Label,
    /// Why it is shown as it was written, where it is.
    pub refusal: Option<Undrawn>,
}

impl Frame {
    /// The words of the top edge: the corner, the rule, and after them the
    /// label and the reason, parted by the glyph set's dot.
    ///
    /// A block with no name and no reason is the edge alone, with nothing
    /// after the rule to trail off into.
    #[must_use]
    pub fn top(&self) -> String {
        let glyphs = self.label.glyphs;
        let mut parts = Vec::new();
        if !self.label.name.is_empty() {
            parts.push(self.label.name.as_str());
        }
        if let Some(refusal) = self.refusal {
            parts.push(refusal.words());
            parts.push("shown as written");
        }

        let mut top = format!("{}{}", glyphs.square().0, glyphs.horizontal());
        if !parts.is_empty() {
            top.push(' ');
            top.push_str(&parts.join(&format!(" {} ", glyphs.dot())));
        }
        top
    }

    /// `inner` in its frame: the top edge, each row behind the gutter, and the
    /// bottom edge, none of them wider than `columns`.
    ///
    /// The gutter is structural, so a drag over the block takes what is inside
    /// it and not the frame's bars. `inner` is laid out already, and for the
    /// room the two columns of the gutter leave.
    #[must_use]
    pub fn rows(&self, columns: usize, inner: &[Row]) -> Vec<Row> {
        let glyphs = self.label.glyphs;
        let edge = |text: &str| Row::new().then(Slot::Quiet, text).clipped(columns);

        let mut rows = Vec::with_capacity(inner.len() + 2);
        rows.push(edge(&self.top()));
        rows.extend(inner.iter().map(|row| {
            Row::new()
                .then_structural(Slot::Quiet, gutter(glyphs))
                .join(row.clone())
                .clipped(columns)
        }));
        rows.push(edge(bottom(glyphs)));
        rows
    }
}

/// What stands in front of every row inside a frame: the bar and the space
/// that parts it from the row, in one run so a row can be known by it.
#[must_use]
pub(crate) const fn gutter(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "│ ",
        Glyphs::Ascii => "| ",
    }
}

/// The columns [`gutter`] takes, which is what a frame leaves a row.
pub(crate) const GUTTER: usize = 2;

/// The bottom edge of a frame.
#[must_use]
pub(crate) const fn bottom(glyphs: Glyphs) -> &'static str {
    match glyphs {
        Glyphs::Unicode => "└─",
        Glyphs::Ascii => "+-",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::markdown::corpus_pictures::{laid, picture};

    const BOTH: [Glyphs; 2] = [Glyphs::Unicode, Glyphs::Ascii];

    fn frame(name: &str, refusal: Option<Undrawn>, glyphs: Glyphs) -> Frame {
        Frame {
            label: Label::new(name, glyphs),
            refusal,
        }
    }

    #[test]
    fn the_run_a_row_is_known_by_is_the_bar_the_set_draws_and_a_space() {
        for glyphs in BOTH {
            assert_eq!(gutter(glyphs), format!("{} ", glyphs.vertical()));
            assert_eq!(width::columns(gutter(glyphs)), GUTTER);
        }
    }

    #[test]
    fn the_bottom_edge_is_the_corner_and_the_rule_of_the_set() {
        for glyphs in BOTH {
            assert_eq!(
                bottom(glyphs),
                format!("{}{}", glyphs.square().1, glyphs.horizontal())
            );
        }
    }

    #[test]
    fn the_top_edge_names_the_block() {
        assert_eq!(frame("rust", None, Glyphs::Unicode).top(), "┌─ rust");
        assert_eq!(frame("rust", None, Glyphs::Ascii).top(), "+- rust");
    }

    #[test]
    fn the_top_edge_of_a_block_with_no_name_is_the_edge_alone() {
        assert_eq!(frame("", None, Glyphs::Unicode).top(), "┌─");
        assert_eq!(frame("", None, Glyphs::Ascii).top(), "+-");
    }

    #[test]
    fn a_refusal_is_in_the_top_edge_with_the_dot_of_the_set() {
        let refused = Some(Undrawn::PastCap);

        assert_eq!(
            frame("mermaid", refused, Glyphs::Unicode).top(),
            "┌─ mermaid · longer than this can hold · shown as written"
        );
        assert_eq!(
            frame("mermaid", refused, Glyphs::Ascii).top(),
            "+- mermaid - longer than this can hold - shown as written"
        );
    }

    #[test]
    fn a_name_longer_than_a_label_may_be_ends_in_the_ellipsis_of_its_set() {
        let long = "x".repeat(100);

        for glyphs in BOTH {
            let top = frame(&long, None, glyphs).top();

            assert!(top.ends_with(glyphs.ellipsis()), "{top:?}");
            assert_eq!(
                width::columns(&top),
                width::columns(&format!("{}{} ", glyphs.square().0, glyphs.horizontal()))
                    + NAME_COLUMNS,
                "{top:?}"
            );
        }
    }

    #[test]
    fn a_name_that_fits_is_kept_whole() {
        let name = "x".repeat(NAME_COLUMNS);

        assert_eq!(
            frame(&name, None, Glyphs::Unicode).top(),
            format!("┌─ {name}")
        );
    }

    #[test]
    fn a_name_in_wide_characters_is_cut_by_its_columns() {
        let name = "\u{691c}".repeat(40);

        for glyphs in BOTH {
            let top = frame(&name, None, glyphs).top();
            let label = top.split_once(' ').map_or("", |(_, label)| label);

            assert!(width::columns(label) <= NAME_COLUMNS, "{top:?}");
            assert!(label.ends_with(glyphs.ellipsis()), "{top:?}");
        }
    }

    #[test]
    fn the_rows_of_a_frame_are_its_edges_and_each_row_behind_the_gutter() {
        let inner = [
            Row::new().then(Slot::Plain, "one"),
            Row::new(),
            Row::new().then(Slot::Keyword, "two"),
        ];

        let rows = frame("rust", None, Glyphs::Unicode).rows(40, &inner);

        let text: Vec<String> = rows.iter().map(Row::text).collect();
        assert_eq!(text, ["┌─ rust", "│ one", "│ ", "│ two", "└─"]);
    }

    #[test]
    fn the_gutter_of_a_framed_row_is_quiet_and_structural_and_the_row_is_not() {
        let inner = [Row::new().then(Slot::Keyword, "two")];

        let rows = frame("", None, Glyphs::Ascii).rows(40, &inner);

        let framed = rows.get(1).expect("a framed row");
        assert_eq!(framed.structural(), vec![0..GUTTER]);
        let slots: Vec<Slot> = framed.kinds().collect();
        assert_eq!(slots, [Slot::Quiet, Slot::Keyword]);
    }

    #[test]
    fn a_fence_streamed_into_the_record_is_the_rows_a_frame_makes_of_its_lines() {
        // Two sources for one look: the reader says a fence's frame as it
        // streams, and `Frame::rows` is what a block drawn whole is framed
        // with. Where nothing wraps they have to be the same rows.
        for glyphs in BOTH {
            let lines = ["one", "", "    two"];
            let inner: Vec<Row> = lines
                .iter()
                .map(|line| Row::new().then(Slot::Quiet, *line))
                .collect();
            let frame = Frame {
                label: Label::new("", glyphs),
                refusal: None,
            };

            let streamed = laid(&format!("```\n{}\n```\n", lines.join("\n")), 40, glyphs);

            assert_eq!(
                streamed.get(..inner.len() + 2),
                Some(frame.rows(40, &inner).as_slice()),
                "{glyphs:?}: {streamed:?}"
            );
        }
    }

    #[test]
    fn no_row_of_a_frame_is_wider_than_the_window_it_stands_in() {
        let inner = [Row::new().then(Slot::Plain, "x".repeat(300))];

        for glyphs in BOTH {
            for columns in 1..=80 {
                let rows =
                    frame(&"y".repeat(60), Some(Undrawn::PastCap), glyphs).rows(columns, &inner);

                for row in rows {
                    assert!(row.columns() <= columns, "{columns}: {:?}", row.text());
                }
            }
        }
    }

    // The frame as the transcript lays it out, at the width a laptop terminal
    // splits in two. The pictures show slots, so one picture per set stands for
    // every theme.

    const RUST: &str = "A fence that names its language:\n\n```rust\nfn main() {\n    let value = compute(one, two);\n}\n```\n";
    const PLAIN: &str =
        "A fence that names nothing:\n\n```\nplain block\n\nafter a blank line\n```\n";
    const WRAPPED: &str = "A line longer than the window:\n\n```rust\nlet total = first_value + second_value + third_value + fourth_value + fifth_value;\n```\n";

    #[test]
    fn a_rust_fence_at_64_columns() {
        insta::assert_snapshot!(picture(RUST, 64, Glyphs::Unicode));
    }

    #[test]
    fn a_rust_fence_at_64_columns_in_ascii() {
        insta::assert_snapshot!(picture(RUST, 64, Glyphs::Ascii));
    }

    #[test]
    fn a_fence_naming_nothing_at_64_columns() {
        insta::assert_snapshot!(picture(PLAIN, 64, Glyphs::Unicode));
    }

    #[test]
    fn a_fence_naming_nothing_at_64_columns_in_ascii() {
        insta::assert_snapshot!(picture(PLAIN, 64, Glyphs::Ascii));
    }

    #[test]
    fn a_wrapped_line_of_code_at_64_columns() {
        insta::assert_snapshot!(picture(WRAPPED, 64, Glyphs::Unicode));
    }

    #[test]
    fn a_wrapped_line_of_code_at_64_columns_in_ascii() {
        insta::assert_snapshot!(picture(WRAPPED, 64, Glyphs::Ascii));
    }
}
