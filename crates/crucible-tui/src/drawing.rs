//! The contract a drawer keeps: what turns a closed block of an answer, or a
//! tool's change, into rows a terminal can show.
//!
//! A drawer is a plugin. It names the fences it takes, may say what a block is
//! from its first line, may turn an inline span into styled runs, lays a closed
//! block out into [`Row`]s for a [`Room`], and may draw a display block as an
//! [`Svg`] for the image tier. [`Drawers`] is the registry the composition
//! fills; nothing in this crate names a drawer, and an empty registry means
//! every block is shown as it was written.
//!
//! What every drawer keeps, which no type here can enforce and every drawer's
//! own tests and the fit sweep do:
//!
//! - It runs on the drawing thread when a block closes. It never writes to a
//!   terminal and never starts a thread or a process.
//! - It counts the input's structure against the caps it states before any
//!   engine runs, and answers [`Undrawn::PastCap`] for a block past them,
//!   rather than spending the frame finding out.
//! - What it lays out fits the room it was handed: no row is wider than
//!   [`Room::columns`]. A block is laid out at its natural size and checked
//!   again for fit when the window changes, so the same source and room always
//!   give the same rows.
//! - Its input is hostile: a block is model output, and a path or a name in it
//!   is drawn safe.
//! - A row of its that offers a key says so in [`EXPAND`], and
//!   [`Draws::unoffered`] takes every such offer out of rows it laid out and
//!   leaves what else they say. The reader asks it to where no key is read,
//!   so a row reads there as one whose offer was withdrawn on a screen does.

use std::sync::Arc;

use crate::chrome::Undrawn;
use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::markdown::Said;
use crate::row::Row;

/// The words that name the key an offer to show more is made with.
///
/// One action has one name: a drawer that folds what it was given offers the
/// rest in these words, and so does a row that cut what came back, so the
/// reader learns the key once. Where no key is read the drawer takes them
/// out again, through [`Draws::unoffered`].
pub const EXPAND: &str = "ctrl+o to expand";

/// What a drawer is handed to lay a block out in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    /// The columns the block may take. No row a drawer returns is wider.
    pub columns: usize,
    /// The glyph set in force; a change of set lays every block out again.
    pub glyphs: Glyphs,
}

/// One laid-out run of an inline span: its slot and its words.
///
/// Distinct from the reader's [`Said::Run`]: a laid-out run carries no link,
/// and becomes one with none through [`Run::said`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The ink it is worn in.
    pub slot: Slot,
    /// What it says.
    pub text: String,
}

impl Run {
    /// The reader's run this is: the same slot and words, and no link.
    #[must_use]
    pub fn said(&self) -> Said<'_> {
        Said::Run {
            slot: self.slot,
            text: &self.text,
            link: None,
        }
    }
}

/// A closed block, laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laid {
    /// The rows, each no wider than the room they were laid out for.
    pub rows: Vec<Row>,
    /// The block's natural size, columns by rows: what it would take with no
    /// limit on the columns. A renderer reads from it whether the block was
    /// squeezed, and whether a wider window would change it.
    pub natural: (usize, usize),
    /// Another layout the drawer offers where the natural one does not fit
    /// the room, such as a table drawn as a list. It fits the room too.
    pub narrow: Option<Vec<Row>>,
}

/// A display block drawn as a picture, for the image tier.
///
/// SVG text, laid out at natural size, held to [`Svg::MOST`] bytes. Its inks
/// are written as slot classes — the names [`Slot::class`] gives, their one
/// source — never as colours, so whatever rasterises it applies the palette in
/// force. It uses only the elements `crucible-raster` admits; a picture naming
/// any other is refused there, not drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Svg(Box<str>);

impl Svg {
    /// The most bytes one picture is kept at: the most `crucible-raster` takes
    /// as input, so a picture kept here is never one it refuses for its size.
    pub const MOST: usize = 1 << 20;

    /// Keeps `text` as a picture, or refuses it past [`Svg::MOST`] bytes.
    ///
    /// # Errors
    ///
    /// [`Undrawn::PastCap`] when `text` is longer than [`Svg::MOST`] bytes.
    pub fn new(text: String) -> Result<Self, Undrawn> {
        if text.len() > Self::MOST {
            return Err(Undrawn::PastCap);
        }
        Ok(Self(text.into_boxed_str()))
    }

    /// The picture's text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it says nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A window's width to the nearest four cells below it: what a picture drawn
/// for the image tier is kept against, so a resize by a column or two reuses
/// the picture rather than drawing it again. Its one owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bucket(usize);

impl Bucket {
    /// The cells one bucket spans.
    pub const CELLS: usize = 4;

    /// The bucket `columns` falls in.
    #[must_use]
    pub const fn of(columns: usize) -> Self {
        Self(columns / Self::CELLS)
    }

    /// Its narrow edge, in columns: the widest a picture drawn for it may be.
    #[must_use]
    pub const fn columns(self) -> usize {
        self.0.saturating_mul(Self::CELLS)
    }
}

/// How a kind of block is drawn: the renderer's own type for the settings
/// that choose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// As it was written, in its frame.
    Written,
    /// Laid out as rows of text, by its drawer's [`Draws::block`].
    Text,
    /// As a picture, by its drawer's [`Draws::picture`], where the terminal
    /// can show one.
    Image,
}

/// What draws one kind of block.
///
/// `Send + Sync` because the composition builds the registry once and the
/// drawing thread asks it.
pub trait Draws: Send + Sync {
    /// The fence names this drawer takes, so the reader can hold a block
    /// under one until it closes.
    fn names(&self) -> &'static [&'static str];

    /// What the block is, read from its first line, for a status row to name
    /// ("a mermaid flowchart"). The default says nothing, and the row names
    /// the fence.
    fn label(&self, fence: &str, first_line: &str) -> Option<&'static str> {
        let _ = (fence, first_line);
        None
    }

    /// An inline span turned into styled runs. The default draws none.
    fn inline(&self, span: &str, room: &Room) -> Option<Vec<Run>> {
        let _ = (span, room);
        None
    }

    /// A closed block's rows at natural size for `room`.
    ///
    /// # Errors
    ///
    /// Why the block is shown as written instead: past this drawer's caps, or
    /// not something it draws.
    fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn>;

    /// Takes out of `rows`, which this drawer laid out, each offer of a key
    /// they make and leaves the rest of what they say: the rows are going
    /// where no key is read. The default changes nothing, for a drawer whose
    /// rows offer none.
    fn unoffered(&self, rows: &mut [Row]) {
        let _ = rows;
    }

    /// A display block as a picture. The default draws none.
    ///
    /// # Errors
    ///
    /// [`Undrawn::NotHere`] by default; a drawer that draws pictures answers
    /// why it drew none for this block.
    fn picture(&self, source: &str, room: &Room) -> Result<Svg, Undrawn> {
        let _ = (source, room);
        Err(Undrawn::NotHere)
    }
}

/// The drawers the composition registered, asked by fence name.
///
/// Empty means every block is shown as it was written. It holds trait
/// objects, so it says nothing about equality.
#[derive(Clone, Default)]
pub struct Drawers(Vec<Arc<dyn Draws>>);

impl Drawers {
    /// The registry with `drawer` added after the ones already in it.
    #[must_use]
    pub fn with(mut self, drawer: Arc<dyn Draws>) -> Self {
        self.0.push(drawer);
        self
    }

    /// The drawer for a fence named `name`, in any case: the first registered
    /// that takes it.
    #[must_use]
    pub fn for_name(&self, name: &str) -> Option<&dyn Draws> {
        self.0
            .iter()
            .find(|drawer| {
                drawer
                    .names()
                    .iter()
                    .any(|taken| taken.eq_ignore_ascii_case(name))
            })
            .map(AsRef::as_ref)
    }

    /// An inline span as the runs the first registered drawer that draws it
    /// lays out, asked in order; `None` where none does, and the span is shown
    /// as written.
    #[must_use]
    pub fn inline(&self, span: &str, room: &Room) -> Option<Vec<Run>> {
        self.0.iter().find_map(|drawer| drawer.inline(span, room))
    }

    /// Whether nothing is registered, so every block is shown as written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Drawers {
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        into.debug_tuple("Drawers")
            .field(
                &self
                    .0
                    .iter()
                    .map(|drawer| drawer.names())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Said;

    /// A drawer that takes two names and draws each line of a block as a row.
    struct Lines;

    impl Draws for Lines {
        fn names(&self) -> &'static [&'static str] {
            &["lines", "rows"]
        }

        fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
            let rows: Vec<Row> = source
                .lines()
                .map(|line| Row::plain(crate::width::clip(line, room.columns)))
                .collect();
            let natural = (
                source.lines().map(crate::width::columns).max().unwrap_or(0),
                rows.len(),
            );
            Ok(Laid {
                rows,
                natural,
                narrow: None,
            })
        }
    }

    /// Another, taking one of the same names.
    struct Second;

    impl Draws for Second {
        fn names(&self) -> &'static [&'static str] {
            &["rows"]
        }

        fn block(&self, _: &str, _: &Room) -> Result<Laid, Undrawn> {
            Err(Undrawn::PastCap)
        }
    }

    fn room(columns: usize) -> Room {
        Room {
            columns,
            glyphs: Glyphs::Unicode,
        }
    }

    #[test]
    fn an_empty_registry_has_no_drawer_so_every_block_is_shown_as_written() {
        let drawers = Drawers::default();
        assert!(drawers.is_empty());
        assert!(drawers.for_name("diff").is_none());
        assert!(drawers.for_name("").is_none());
    }

    #[test]
    fn a_drawer_is_found_by_any_of_its_names_in_any_case() {
        let drawers = Drawers::default().with(Arc::new(Lines));
        assert!(!drawers.is_empty());
        for name in ["lines", "rows", "LINES", "Rows"] {
            let drawer = drawers.for_name(name);
            assert!(drawer.is_some(), "{name} found nothing");
        }
        assert!(drawers.for_name("line").is_none());
        assert!(drawers.for_name("lines ").is_none());
    }

    #[test]
    fn the_first_drawer_registered_for_a_name_is_the_one_asked() {
        let drawers = Drawers::default()
            .with(Arc::new(Lines))
            .with(Arc::new(Second));
        let drawn = drawers
            .for_name("rows")
            .map(|drawer| drawer.block("one\ntwo", &room(80)));
        assert!(matches!(drawn, Some(Ok(laid)) if laid.rows.len() == 2));
        assert_eq!(
            format!("{drawers:?}"),
            "Drawers([[\"lines\", \"rows\"], [\"rows\"]])"
        );
    }

    #[test]
    fn a_drawer_that_says_nothing_of_labels_spans_or_pictures_draws_none_of_them() {
        let lines = Lines;
        assert_eq!(lines.label("lines", "anything"), None);
        assert!(lines.inline("x", &room(80)).is_none());
        assert_eq!(
            lines.picture("x", &room(80)).map(|svg| svg.len()),
            Err(Undrawn::NotHere)
        );
        assert_eq!(Undrawn::NotHere.words(), "not drawn here");
    }

    #[test]
    fn a_picture_past_its_cap_is_refused_before_it_is_kept() {
        let small = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let kept = Svg::new(small.to_owned());
        assert_eq!(kept.as_ref().map(Svg::as_str), Ok(small));
        assert_eq!(
            Svg::new("x".repeat(Svg::MOST + 1)).map(|svg| svg.len()),
            Err(Undrawn::PastCap)
        );
        assert_eq!(
            Svg::new("x".repeat(Svg::MOST)).map(|svg| svg.len()),
            Ok(Svg::MOST)
        );
    }

    #[test]
    fn a_bucket_is_four_cells_wide_and_names_its_narrow_edge() {
        assert_eq!(Bucket::CELLS, 4);
        assert_eq!(Bucket::of(0), Bucket::of(3));
        assert_ne!(Bucket::of(3), Bucket::of(4));
        assert_eq!(Bucket::of(4), Bucket::of(7));
        assert_eq!(Bucket::of(7).columns(), 4);
        assert_eq!(Bucket::of(80).columns(), 80);
        assert_eq!(Bucket::of(usize::MAX).columns() % Bucket::CELLS, 0);
    }

    #[test]
    fn a_laid_run_is_said_as_a_run_with_no_link() {
        let run = Run {
            slot: Slot::Added,
            text: "x".to_owned(),
        };
        assert!(matches!(
            run.said(),
            Said::Run {
                slot: Slot::Added,
                text: "x",
                link: None
            }
        ));
    }

    #[test]
    fn every_mode_is_one_of_three() {
        let modes = [Mode::Written, Mode::Text, Mode::Image];
        for mode in modes {
            let place = match mode {
                Mode::Written => 0,
                Mode::Text => 1,
                Mode::Image => 2,
            };
            assert_eq!(modes.get(place), Some(&mode));
        }
    }
}
