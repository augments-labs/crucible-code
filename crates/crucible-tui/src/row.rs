//! A row as slots rather than as bytes.
//!
//! What a component builds. A span is a run of text that is all one [`Slot`], so
//! a row can be measured before it is coloured and coloured without being
//! measured again: display width is a property of the text alone, and the
//! escape sequences arrive at the last moment, from the palette, on the way to
//! the terminal.
//!
//! That separation is also what makes a component testable with no terminal
//! attached. [`Row::text`] is what the row says and [`Row::paint`] is what a
//! terminal is sent — the art is asserted in one place and the colour in
//! another, and neither test has to read past the other's noise.
//!
//! It is the structure a renderer that is not a terminal would walk too. A row
//! of spans carrying a slot says what it means rather than how it looks, so the
//! part that would have to be rewritten to draw the same component somewhere
//! else is [`Row::paint`] and nothing above it.

use std::ops::Range;

use crate::color::{Palette, Slot};
use crate::width;

/// A run of text that is all one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    /// What it says.
    text: String,
    /// The job the colour does, if the palette writes one.
    slot: Slot,
    /// Columns to exclude from selection, counted from this span's start.
    structural: Vec<Range<usize>>,
    /// Where the words go, where they go anywhere.
    ///
    /// Beside the text rather than inside it, for the reason the slot is: an
    /// address written into the string would be measured as columns the reader
    /// cannot see, and every fold and clip in this file would break in the
    /// middle of one.
    link: Option<Box<str>>,
}

/// One row of a component: its spans, left to right.
///
/// Built once and then read. A row is laid out where the facts behind it are,
/// and drawn wherever it ends up — held in the record, or stood over the box —
/// so it owns its text rather than borrowing what it was assembled from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row(Vec<Span>);

impl Row {
    /// An empty row.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A row that is `text`, in the reader's own foreground.
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new().then(Slot::Plain, text)
    }

    /// The row with `text` appended in `slot`.
    #[must_use]
    pub fn then(mut self, slot: Slot, text: impl Into<String>) -> Self {
        self.push(slot, text);
        self
    }

    /// Appends `text` in `slot`.
    ///
    /// An empty span is dropped rather than kept. It would draw nothing and
    /// measure nothing, and painted it would be an opening sequence and a reset
    /// around no text at all.
    pub fn push(&mut self, slot: Slot, text: impl Into<String>) {
        self.push_span(slot, text, Vec::new(), None);
    }

    /// The row with `text` appended in `slot`, opening `link` when clicked.
    ///
    /// The address is carried rather than shown. What the reader sees is the
    /// words — the whole point of writing one is that a sentence keeps reading
    /// as a sentence — and the terminal is handed the pair.
    #[must_use]
    pub fn then_linked(
        mut self,
        slot: Slot,
        text: impl Into<String>,
        link: impl Into<Box<str>>,
    ) -> Self {
        self.push_linked(slot, text, link);
        self
    }

    /// Appends `text` in `slot`, opening `link` when clicked.
    pub fn push_linked(&mut self, slot: Slot, text: impl Into<String>, link: impl Into<Box<str>>) {
        self.push_span(slot, text, Vec::new(), Some(link.into()));
    }

    /// The row with structural art appended in `slot`.
    ///
    /// Structural art is painted and measured like every other span, but a drag
    /// does not highlight it or copy it. Marks supplied by a component use this;
    /// the same character in text supplied by the reader remains ordinary text.
    #[must_use]
    pub fn then_structural(mut self, slot: Slot, text: impl Into<String>) -> Self {
        self.push_structural(slot, text);
        self
    }

    /// Appends structural art in `slot`.
    pub fn push_structural(&mut self, slot: Slot, text: impl Into<String>) {
        let text = text.into();
        let columns = width::columns(&text);
        self.push_span(
            slot,
            text,
            (columns > 0).then_some(0..columns).into_iter().collect(),
            None,
        );
    }

    /// Appends one span, dropping an empty one.
    fn push_span(
        &mut self,
        slot: Slot,
        text: impl Into<String>,
        structural: Vec<Range<usize>>,
        link: Option<Box<str>>,
    ) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.0.push(Span {
            text,
            slot,
            structural,
            link,
        });
    }

    /// The row with `other`'s spans appended.
    ///
    /// How a component puts a row it built inside a frame it built separately:
    /// the two were measured apart and stay measured, because appending spans
    /// cannot change what any of them says or costs.
    #[must_use]
    pub fn join(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    /// Inserts `other`'s spans at the start of this row.
    pub(crate) fn prepend(&mut self, other: Self) {
        let mut spans = other.0;
        spans.append(&mut self.0);
        self.0 = spans;
    }

    /// Appends spaces until the row is `columns` wide.
    ///
    /// Does nothing to a row that is already at least that wide: padding is how
    /// a column reaches the divider on its right, and a row that has outgrown
    /// its column is one the caller has to shorten rather than one this can fix.
    pub fn pad(&mut self, columns: usize) {
        self.fill(Slot::Plain, columns);
    }

    /// The same, in a slot the caller names.
    ///
    /// What a row that paints its own ground pads with. A space carries no ink
    /// but it does carry a ground, so a row filled out in [`Slot::Plain`] is one
    /// whose colour stops where its text does — and a block of them has a ragged
    /// right edge with the reader's own ground showing through it.
    pub fn fill(&mut self, slot: Slot, columns: usize) {
        let short = columns.saturating_sub(self.columns());
        self.push(slot, " ".repeat(short));
    }

    /// How many display columns the row costs.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.0.iter().map(|span| width::columns(&span.text)).sum()
    }

    /// Whether the row says nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many display rows this row folds to at `columns`.
    ///
    /// Asked once per line per width and cached by whoever is keeping the rows,
    /// because scrolling asks how tall a body of them is far more often than it
    /// asks what is in any one.
    #[must_use]
    pub fn folds(&self, columns: usize) -> usize {
        if columns == 0 {
            return 1;
        }
        width::folds(&self.text(), columns).len().max(1)
    }

    /// This row folded to `columns`, each part keeping the slots it was written
    /// in.
    ///
    /// The same breaks [`width::fold`] would choose for the same words, because
    /// it is the same walk: a row of spans and a line of plain text must not
    /// disagree about where a line ends. A row narrow enough already is one
    /// row, and an empty row is one empty row — a blank line between paragraphs
    /// is a row somebody meant.
    #[must_use]
    pub fn fold(&self, columns: usize) -> Vec<Self> {
        if columns == 0 || self.columns() <= columns {
            return vec![self.clone()];
        }

        let text = self.text();
        let mut folded: Vec<Self> = width::folds(&text, columns)
            .into_iter()
            .map(|part| self.between(part.start, part.end))
            .collect();

        if folded.is_empty() {
            folded.push(Self::new());
        }
        folded
    }

    /// This row with anything past `columns` cut off.
    ///
    /// What a row gets instead of folding when something laid it out against a
    /// width: a narrower window is not an invitation to re-flow a table into
    /// prose, and a row of box-drawing characters has no spaces to break at.
    #[must_use]
    pub fn clipped(&self, columns: usize) -> Self {
        if self.columns() <= columns {
            return self.clone();
        }
        let text = self.text();
        self.between(0, width::clip(&text, columns).len())
    }

    /// The bytes of [`Row::text`] between `from` and `to`, as a row.
    ///
    /// Whichever spans the two ends fall inside are cut, and each piece keeps
    /// the slot the span it came from was written in — which is the whole
    /// reason folding happens here rather than on the painted string, where a
    /// break would land in the middle of an escape sequence.
    fn between(&self, from: usize, to: usize) -> Self {
        let mut cut = Self::new();
        let mut at = 0;

        for span in &self.0 {
            let (start, end) = (at, at + span.text.len());
            at = end;

            let (from, to) = (from.max(start), to.min(end));
            if from >= to {
                continue;
            }
            cut.push_span(
                span.slot,
                &span.text[from - start..to - start],
                structural_between(span, from - start, to - start),
                span.link.clone(),
            );
        }

        cut
    }

    /// What the row says, with no colour in it.
    ///
    /// What a redirected run is written, and what a test asserting on the art
    /// reads.
    #[must_use]
    pub fn text(&self) -> String {
        let mut said = String::with_capacity(self.bytes());
        for span in &self.0 {
            said.push_str(&span.text);
        }
        said
    }

    /// The slot each run of this row asked for, in order.
    ///
    /// What a test checking that a component *chose* the right meaning reads —
    /// [`Row::text`] answers what it says and [`Row::paint`] what a terminal is
    /// sent, and neither can answer this.
    pub fn kinds(&self) -> impl Iterator<Item = Slot> + '_ {
        self.0.iter().map(|span| span.slot)
    }

    /// Whether structural art already occupies the row's first column.
    pub(crate) fn starts_structural(&self) -> bool {
        self.0
            .first()
            .is_some_and(|span| span.structural.iter().any(|range| range.start == 0))
    }

    /// Display columns occupied by structural art in this row.
    pub(crate) fn structural(&self) -> Vec<Range<usize>> {
        let mut column = 0;
        let mut ranges = Vec::new();

        for span in &self.0 {
            let end = column + width::columns(&span.text);
            ranges.extend(
                span.structural
                    .iter()
                    .map(|range| column + range.start..column + range.end),
            );
            column = end;
        }

        ranges
    }

    /// The runs the row is made of: the slot each asked for, and what it says.
    ///
    /// The one thing neither [`Row::text`] nor [`Row::paint`] can answer. A
    /// test picturing a component has to see the slot the component *chose*,
    /// where paint shows the hue a palette settled on for one terminal — so
    /// this is how [`crate::dump`] writes a run's job into the picture beside
    /// the run. Tests only: shipped code has a palette in hand and asks for
    /// paint.
    #[cfg(test)]
    pub(crate) fn spans(&self) -> impl Iterator<Item = (Slot, &str)> {
        self.0.iter().map(|span| (span.slot, span.text.as_str()))
    }

    /// The row as a terminal is sent it.
    ///
    /// The palette is borrowed rather than copied. It used to be a handful of
    /// bytes and was passed by value; it now carries the sequences it worked
    /// out — the prompt band and the six a syntax theme decides — and copying
    /// those once per row is work on the one path that may not do any.
    ///
    /// A slot the palette has no colour for — [`Slot::Plain`] always, and every
    /// slot once colour is off — writes its text and nothing around it, so a
    /// run that is not coloured costs no bytes rather than costing an empty
    /// pair of sequences.
    #[must_use]
    pub fn paint(&self, palette: &Palette) -> String {
        let mut painted = String::new();
        self.paint_into(palette, &mut painted);
        painted
    }

    /// The same, appended to a buffer the caller keeps.
    ///
    /// What a component with a dozen rows to draw uses, so the whole of it
    /// costs the one allocation that buffer already grew to.
    pub fn paint_into(&self, palette: &Palette, painted: &mut String) {
        // Room for the sequences as well, so a row of a dozen spans does not
        // grow the buffer twice. Over-reserving by a few bytes is cheaper than
        // either.
        painted.reserve(if palette.writes_color() {
            self.bytes() + self.0.len() * PAINTED
        } else {
            self.bytes()
        });

        let links = palette.writes_links();

        for span in &self.0 {
            // Outside the colour, so a span that is both is opened as a link
            // and then coloured within it. A terminal that does not implement
            // the sequence drops it and is left with exactly the row it would
            // have had.
            let link = span.link.as_deref().filter(|_| links);
            if let Some(link) = link {
                painted.push_str(OPENS);
                painted.push_str(link);
                painted.push_str(ENDS);
            }

            let open = palette.open(span.slot);
            painted.push_str(open.as_str());
            painted.push_str(&span.text);
            if !open.is_empty() {
                painted.push_str(palette.close());
            }

            // Closed on every span that opened one, rather than left standing
            // to the end of the row: an address that outlived its words would
            // make the rest of the line clickable and wrong.
            if link.is_some() {
                painted.push_str(OPENS);
                painted.push_str(ENDS);
            }
        }
    }

    /// How many bytes the text of this row is.
    fn bytes(&self) -> usize {
        self.0.iter().map(|span| span.text.len()).sum()
    }
}

/// Structural display columns from `span` retained by one byte slice.
fn structural_between(span: &Span, from: usize, to: usize) -> Vec<Range<usize>> {
    if span.structural.is_empty() {
        return Vec::new();
    }

    let before = width::columns(&span.text[..from]);
    let kept = width::columns(&span.text[from..to]);
    span.structural
        .iter()
        .filter_map(|range| {
            let start = range.start.max(before);
            let end = range.end.min(before + kept);
            (start < end).then_some(start - before..end - before)
        })
        .collect()
}

/// Bytes to reserve per coloured span: the longest opening sequence the palette
/// writes is `\x1b[38;2;255;255;255m`, and every one of them is closed.
const PAINTED: usize = 24;

/// What opens an address, and what the address itself is followed by.
///
/// The pair a terminal reads as "the words after this go here", with the
/// address between them and an empty one closing it again. Written out rather
/// than assembled per span, because this is on the render path.
const OPENS: &str = "\x1b]8;;";

/// What closes the opening sequence and the closing one alike.
const ENDS: &str = "\x1b\\";

#[cfg(test)]
mod tests {
    use super::*;

    /// A palette that writes every hue it has, without an environment to say so.
    fn colourful() -> Palette {
        Palette::resolve(true, crate::color::Theme::Dark, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    /// The same, on a terminal that will take an address as well.
    fn addressed() -> Palette {
        colourful().addressing(true)
    }

    #[test]
    fn a_span_carrying_an_address_is_painted_as_one() {
        // The whole of what makes a word in the transcript openable: the text
        // is unchanged and an address travels beside it, so a terminal that
        // understands the pair hands the reader the second when they click the
        // first.
        let row = Row::new().then_linked(Slot::Link, "the pull request", "https://example.test/1");
        let painted = row.paint(&addressed());

        assert!(painted.contains("https://example.test/1"), "{painted:?}");
        assert!(painted.contains("the pull request"), "{painted:?}");
        assert_eq!(row.text(), "the pull request", "the row said the address");
    }

    #[test]
    fn a_terminal_that_takes_no_address_is_sent_the_words_alone() {
        // A pipe, or a terminal that says it is dumb. An address it cannot open
        // is bytes in the middle of a sentence somebody is reading.
        let row = Row::new().then_linked(Slot::Link, "the pull request", "https://example.test/1");
        let painted = row.paint(&colourful());

        assert!(!painted.contains("https://example.test/1"), "{painted:?}");
        assert!(painted.contains("the pull request"), "{painted:?}");
    }

    #[test]
    fn an_address_survives_the_row_being_folded_and_clipped() {
        // A link long enough to wrap is still one link. Each part carries the
        // address, because a terminal reads the pair per row and half a link is
        // a word that does nothing.
        let row = Row::plain("see ").then_linked(
            Slot::Link,
            "the pull request that changed the renderer",
            "https://example.test/1",
        );

        for part in row.fold(12) {
            let painted = part.paint(&addressed());
            if part.text().trim().is_empty() || part.text().starts_with("see") {
                continue;
            }
            assert!(
                painted.contains("https://example.test/1"),
                "{:?}: {painted:?}",
                part.text()
            );
        }

        let clipped = row.clipped(20).paint(&addressed());
        assert!(clipped.contains("https://example.test/1"), "{clipped:?}");
    }

    #[test]
    fn a_folded_row_says_what_the_same_words_folded_as_text_say() {
        // The two walks are one walk, and this is where that is stated. A row
        // of spans wrapping a column earlier than the paragraph beside it would
        // be a picture nobody could line up.
        let row = Row::plain("a sentence long enough to need somewhere sensible to break")
            .then(Slot::Accent, " and an accented tail on the end of it");

        for columns in 1..40 {
            let said: Vec<String> = row.fold(columns).iter().map(Row::text).collect();
            assert_eq!(
                said,
                width::fold(&row.text(), columns),
                "at {columns} columns"
            );
        }
    }

    #[test]
    fn no_row_a_fold_hands_back_is_wider_than_it_was_asked_for() {
        let row = Row::plain("short ")
            .then(Slot::Strong, "supercalifragilisticexpialidocious")
            .then(Slot::Quiet, " and more");

        for columns in 1..40 {
            for part in row.fold(columns) {
                assert!(part.columns() <= columns, "{:?} at {columns}", part.text());
            }
        }
    }

    #[test]
    fn a_span_cut_by_a_fold_keeps_the_slot_it_was_written_in() {
        // What makes folding a row's job rather than the painted string's: the
        // break falls inside the accented run, and both halves are still
        // accented afterwards.
        let row = Row::plain("plain ").then(Slot::Accent, "one two three");
        let folded = row.fold(10);

        assert!(folded.len() > 1, "the row did not fold");
        for part in &folded {
            for (slot, text) in part.spans() {
                let expected = if text.trim() == "plain" {
                    Slot::Plain
                } else {
                    Slot::Accent
                };
                assert_eq!(slot, expected, "{text:?} lost its slot");
            }
        }
    }

    #[test]
    fn a_row_with_nothing_in_it_folds_to_one_row_with_nothing_in_it() {
        // A blank line between paragraphs is a row somebody meant. Folding it
        // away would close the gap the reader is using to tell them apart.
        assert_eq!(Row::new().fold(20).len(), 1);
        assert_eq!(Row::plain("").fold(20).len(), 1);
        assert_eq!(Row::plain("   ").fold(20).len(), 1);
    }

    #[test]
    fn a_row_of_nothing_but_spaces_folds_to_one_row() {
        let mut row = Row::new();
        row.push(Slot::Plain, "            ");

        // The fold drops whitespace at a break, so a row that is nothing else
        // has no rows left to hand back. It is still a row somebody wrote, and
        // a blank one is what it looks like.
        assert_eq!(row.fold(4).len(), 1);
        assert_eq!(row.fold(4).first().map(Row::text).as_deref(), Some(""));
        assert_eq!(row.folds(4), 1);
    }

    #[test]
    fn a_row_already_narrow_enough_is_handed_back_whole() {
        // Including its trailing spaces, which a fold would trim: something
        // padded this row to a width and painted it, and the padding is part of
        // the picture rather than whitespace to be tidied.
        let row = Row::plain("kept ").then(Slot::Accent, "  ");
        assert_eq!(row.fold(40), vec![row.clone()]);
    }

    #[test]
    fn how_tall_a_row_folds_is_how_many_rows_it_folds_to() {
        let row = Row::plain("one two three four five six seven eight nine ten");
        for columns in 1..40 {
            assert_eq!(row.folds(columns), row.fold(columns).len(), "at {columns}");
        }
    }

    #[test]
    fn structural_columns_survive_folding_and_clipping() {
        // Deliberately the same slot on both spans: slicing the joined text must
        // not spread the first span's structural meaning over the result words.
        let row = Row::new()
            .then_structural(Slot::Quiet, "⎿")
            .then(Slot::Quiet, " a result long enough to fold");

        let folded = row.fold(12);
        assert_eq!(
            folded.first().map(Row::structural),
            Some(std::iter::once(0..1).collect())
        );
        assert!(folded.iter().skip(1).all(|row| row.structural().is_empty()));
        assert_eq!(
            row.clipped(1).structural(),
            std::iter::once(0..1).collect::<Vec<_>>()
        );
        assert_eq!(
            row.clipped(5).structural(),
            std::iter::once(0..1).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_literal_result_glyph_has_no_structural_columns() {
        assert!(Row::plain("⎿ literal").structural().is_empty());
    }

    #[test]
    fn a_clipped_row_stops_at_the_width_and_keeps_its_slots() {
        // What a set row gets instead of folding. The cut falls inside the
        // accented run, and what survives is still accented.
        let row = Row::plain("|")
            .then(Slot::Accent, "-----------")
            .then(Slot::Plain, "|");
        let clipped = row.clipped(5);

        assert_eq!(clipped.text(), "|----");
        assert_eq!(
            clipped.kinds().collect::<Vec<_>>(),
            [Slot::Plain, Slot::Accent]
        );
        assert_eq!(row.clipped(40), row);
    }

    #[test]
    fn a_wide_glyph_is_never_cut_in_half_by_a_fold_or_a_clip() {
        // Two columns and one character. A cut between its bytes is not a
        // narrower row, it is a broken one.
        let row = Row::plain("\u{65e5}\u{672c}\u{8a9e}");

        for columns in 1..8 {
            assert!(row.clipped(columns).text().chars().all(|c| c != '\u{fffd}'));
            for part in row.fold(columns) {
                assert!(part.columns() <= columns.max(2), "{:?}", part.text());
            }
        }
    }

    #[test]
    fn a_row_measures_what_it_says_and_not_what_it_is_painted_as() {
        // The property the whole separation rests on: colour is bytes, not
        // columns, so a row padded to a width stays that width once it is
        // coloured. A row that grew when it was painted is a row the terminal
        // wraps somewhere this process did not predict.
        let row = Row::new()
            .then(Slot::Accent, "crucible")
            .then(Slot::Plain, " ")
            .then(Slot::Quiet, "v0.0.8");

        assert_eq!(row.columns(), "crucible v0.0.8".len());
        assert_eq!(width::columns(&row.paint(&colourful())), row.columns());
        assert_eq!(width::columns(&row.paint(&Palette::plain())), row.columns());
    }

    #[test]
    fn a_row_says_the_same_thing_however_it_is_painted() {
        let row = Row::new()
            .then(Slot::Strong, "Tips")
            .then(Slot::Plain, ": ")
            .then(Slot::Accent, "/");

        assert_eq!(row.text(), "Tips: /");
        assert_eq!(row.paint(&Palette::plain()), "Tips: /");
        assert!(row.paint(&colourful()).contains("Tips"));
    }

    #[test]
    fn a_run_the_palette_has_no_colour_for_costs_no_bytes() {
        // Plain is the reader's own foreground at every rung, so a row of it
        // is the same bytes coloured or not — which is what keeps the wordmark
        // and the padding from paying for a palette they do not use.
        let row = Row::plain("   ~/code   ");

        assert_eq!(row.paint(&colourful()), row.text());
    }

    #[test]
    fn a_row_is_padded_to_a_width_it_has_not_already_passed() {
        let mut row = Row::plain("ask");
        row.pad(10);
        assert_eq!(row.text(), "ask       ");
        assert_eq!(row.columns(), 10);

        // Already past it: shortening is the caller's decision, and silently
        // taking columns off here would corrupt a border rather than fix one.
        let mut row = Row::plain("a much longer answer");
        row.pad(4);
        assert_eq!(row.text(), "a much longer answer");
    }

    #[test]
    fn a_row_filled_out_in_a_slot_carries_that_slot_to_the_width() {
        // Which is the whole of what a ground needs: the spaces at the end of a
        // diff row are the part of it the reader's own theme would otherwise
        // show through, and they are the part with nothing written on them to
        // notice it by.
        let mut row = Row::new().then(Slot::Added, "budgets:");
        row.fill(Slot::Added, 12);

        assert_eq!(row.columns(), 12);
        assert_eq!(row.spans().last(), Some((Slot::Added, "    ")));

        // And `pad` is the same thing in the reader's own foreground, which is
        // what every row that is not painting a ground wants.
        let mut row = Row::plain("ask");
        row.pad(6);

        assert_eq!(row.spans().last(), Some((Slot::Plain, "   ")));
    }

    #[test]
    fn a_wide_character_is_two_of_the_columns_a_row_is_padded_to() {
        let mut row = Row::plain("日本語");
        row.pad(10);

        assert_eq!(row.columns(), 10);
        assert_eq!(row.text(), "日本語    ");
    }

    #[test]
    fn a_span_with_nothing_in_it_is_not_a_span() {
        let row = Row::new()
            .then(Slot::Accent, "")
            .then(Slot::Plain, "there")
            .then(Slot::Quiet, "");

        assert_eq!(
            row.paint(&colourful()),
            colourful().open(Slot::Plain).as_str().to_owned() + "there"
        );
        assert!(!row.is_empty());
        assert!(Row::new().is_empty());
    }

    #[test]
    fn a_coloured_span_is_closed_where_it_ends() {
        // An attribute left open runs on into whatever is drawn next, which for
        // the last row of a component is the reader's own transcript.
        let painted = Row::plain("root: ")
            .then(Slot::Accent, "~/code")
            .paint(&colourful());

        assert!(painted.ends_with(colourful().close()), "{painted:?}");
    }
}
