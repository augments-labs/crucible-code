//! A diagram's layout drawn as a picture: the rows the engine laid out, each
//! character in a cell of the picture where the text drawing puts it.
//!
//! What it holds to:
//!
//! - **The layout, once.** A picture is drawn from the rows the text drawing
//!   is laid out as, at their natural size: a cell is [`CELL_WIDTH`] units
//!   wide and [`CELL_HEIGHT`] high, and the outermost `<svg>` is as many cells
//!   wide as the widest row and as high as the rows, with a view box of the
//!   same size. Nothing is laid out again.
//! - **Strokes for the characters lines are drawn with, letters for the
//!   rest.** A box-drawing character becomes the arms it draws, merged along
//!   a row or a column into one line; a block becomes a filled rectangle; an
//!   arrowhead, a diamond and a circle become their shape. Every other
//!   character is written as text, a run of it at the cell it starts in and
//!   stretched to the cells it takes, so a label stands where the text
//!   drawing puts it whatever the font. Under the ASCII glyph set the rows
//!   hold none of those characters, so the whole picture is text.
//! - **Inks are slot classes.** Every shape and run of text is inked by a
//!   [`Slot::class`] name, carried as `class` with `currentColor` as its
//!   paint, so whatever rasterises the picture applies the palette in force.
//!   No colour is written anywhere.
//! - **Nothing outside itself.** The picture is `<svg>`, `<path>` and
//!   `<text>` alone, with no reference, style sheet or address, and text is
//!   escaped for XML, so a label holding markup is written as its characters.
//! - **Only letters the font has.** A picture is written in one font, which
//!   has no glyph for most of the world's scripts, so a row whose text holds
//!   a letter outside [`PICTURE_LETTERS`] refuses the picture and the diagram
//!   keeps its text drawing, which the terminal's own fonts draw.
//! - **Bounded.** The picture is refused past [`Svg::MOST`] bytes, checked as
//!   each row is written and once more whole.

use std::fmt::Write as _;

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::Svg;
use crucible_tui::{Slot, columns};

use crate::PICTURE_LETTERS;

/// The width of one cell of the layout, in the picture's units.
const CELL_WIDTH: usize = 8;

/// The height of one cell of the layout, in the picture's units.
const CELL_HEIGHT: usize = 16;

/// Where in its cell a row's text stands, from the cell's top.
const BASELINE: usize = 12;

/// The size the picture's text is set in: as wide as a cell, near enough,
/// in the one font the picture is drawn with.
const FONT_SIZE: usize = 13;

/// How a line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stroke {
    /// `─`, `│` and the light corners and junctions.
    Light,
    /// `━`, `┃` and the heavy corners and junctions.
    Heavy,
    /// `═`, `║` and the double corners and junctions: two lines.
    Double,
    /// `┄`, `┆`, `╍` and `╏`.
    Dashed,
}

impl Stroke {
    /// Every way a line is drawn, in the order their paths are written.
    const ALL: [Self; 4] = [Self::Light, Self::Heavy, Self::Double, Self::Dashed];

    /// How far each of a double line's two lines stands from the middle of
    /// its cell; a single line stands in the middle.
    const fn offsets(self) -> &'static [isize] {
        match self {
            Self::Double => &[-2, 2],
            Self::Light | Self::Heavy | Self::Dashed => &[0],
        }
    }

    /// The attributes a path of this stroke is drawn with.
    const fn drawn(self) -> &'static str {
        match self {
            Self::Light | Self::Double => "stroke-width=\"1\"",
            Self::Heavy => "stroke-width=\"2\"",
            Self::Dashed => "stroke-width=\"1\" stroke-dasharray=\"2 2\"",
        }
    }
}

/// Which way a pointed mark points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pointing {
    Up,
    Right,
    Down,
    Left,
}

/// What a character of the layout is drawn as, when it is not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Arms from the middle of the cell to its edges: up, right, down, left.
    Arms([Option<Stroke>; 4]),
    /// `╱`: from the cell's bottom left to its top right.
    Rising,
    /// `╲`: from the cell's top left to its bottom right.
    Falling,
    /// A block filling the cell from its left, in eighths.
    Block(usize),
    /// `░`: the cell shaded.
    Shade,
    /// An arrowhead, filled or not.
    Point(Pointing, bool),
    /// `◆` and `◇`.
    Diamond(bool),
    /// `●`, `○` and `◯`, with their radius.
    Circle(bool, usize),
    /// `⟩`: an open arrowhead.
    Chevron,
}

/// What `character` is drawn as, or `None` where it is written.
fn shape(character: char) -> Option<Shape> {
    use Stroke::{Dashed, Double, Heavy, Light};
    let arms = |up, right, down, left| Some(Shape::Arms([up, right, down, left]));
    let (l, h, d) = (Some(Light), Some(Heavy), Some(Double));
    match character {
        '─' => arms(None, l, None, l),
        '│' => arms(l, None, l, None),
        '┌' | '╭' => arms(None, l, l, None),
        '┐' | '╮' => arms(None, None, l, l),
        '└' | '╰' => arms(l, l, None, None),
        '┘' | '╯' => arms(l, None, None, l),
        '├' => arms(l, l, l, None),
        '┤' => arms(l, None, l, l),
        '┬' => arms(None, l, l, l),
        '┴' => arms(l, l, None, l),
        '┼' => arms(l, l, l, l),
        '╴' => arms(None, None, None, l),
        '╵' => arms(l, None, None, None),
        '╶' => arms(None, l, None, None),
        '╷' => arms(None, None, l, None),
        '━' => arms(None, h, None, h),
        '┃' => arms(h, None, h, None),
        '┏' => arms(None, h, h, None),
        '┓' => arms(None, None, h, h),
        '┗' => arms(h, h, None, None),
        '┛' => arms(h, None, None, h),
        '┣' => arms(h, h, h, None),
        '┫' => arms(h, None, h, h),
        '┳' => arms(None, h, h, h),
        '┻' => arms(h, h, None, h),
        '╋' => arms(h, h, h, h),
        '═' => arms(None, d, None, d),
        '║' => arms(d, None, d, None),
        '╔' => arms(None, d, d, None),
        '╗' => arms(None, None, d, d),
        '╚' => arms(d, d, None, None),
        '╝' => arms(d, None, None, d),
        '╠' => arms(d, d, d, None),
        '╣' => arms(d, None, d, d),
        '╦' => arms(None, d, d, d),
        '╩' => arms(d, d, None, d),
        '╬' => arms(d, d, d, d),
        '╓' => arms(None, l, d, None),
        '╖' => arms(None, None, d, l),
        '╙' => arms(d, l, None, None),
        '╜' => arms(d, None, None, l),
        '┄' | '╍' => arms(None, Some(Dashed), None, Some(Dashed)),
        '┆' | '╏' => arms(Some(Dashed), None, Some(Dashed), None),
        '╱' => Some(Shape::Rising),
        '╲' => Some(Shape::Falling),
        '█' => Some(Shape::Block(8)),
        '▉' => Some(Shape::Block(7)),
        '▊' => Some(Shape::Block(6)),
        '▋' => Some(Shape::Block(5)),
        '▌' => Some(Shape::Block(4)),
        '▍' => Some(Shape::Block(3)),
        '▎' => Some(Shape::Block(2)),
        '▏' => Some(Shape::Block(1)),
        '░' => Some(Shape::Shade),
        '▴' | '▲' => Some(Shape::Point(Pointing::Up, true)),
        '▸' | '▶' | '►' => Some(Shape::Point(Pointing::Right, true)),
        '▾' | '▼' => Some(Shape::Point(Pointing::Down, true)),
        '◂' | '◄' => Some(Shape::Point(Pointing::Left, true)),
        '△' => Some(Shape::Point(Pointing::Up, false)),
        '▹' => Some(Shape::Point(Pointing::Right, false)),
        '◆' => Some(Shape::Diamond(true)),
        '◇' => Some(Shape::Diamond(false)),
        '●' => Some(Shape::Circle(true, 3)),
        '○' => Some(Shape::Circle(false, 3)),
        '◯' => Some(Shape::Circle(false, 4)),
        '⟩' => Some(Shape::Chevron),
        _ => None,
    }
}

/// One straight piece of a line: along a row at `at` from `from` to `to`,
/// or down a column at `at` from `from` to `to`, in the picture's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Piece {
    at: usize,
    from: usize,
    to: usize,
}

/// The pieces of one way of drawing a line, gathered across the rows.
#[derive(Debug, Default)]
struct Lines {
    across: Vec<Piece>,
    down: Vec<Piece>,
}

/// What a picture is drawn with before it is written out.
#[derive(Debug, Default)]
struct Drawn {
    /// The pieces of each [`Stroke`], in [`Stroke::ALL`]'s order.
    lines: [Lines; 4],
    /// `╱` and `╲`, drawn with the light lines.
    slants: String,
    /// What is filled with the plain ink.
    filled: String,
    /// What is outlined with the plain ink.
    outlined: String,
    /// What is filled with the quiet ink.
    shaded: String,
    /// The runs of text, each a `<text>`.
    text: String,
}

/// `lines`, laid out by the engine `natural` columns wide at the most, as a
/// picture at their natural size.
///
/// # Errors
///
/// [`Undrawn::NotHere`] when a row writes a letter outside
/// [`PICTURE_LETTERS`], and [`Undrawn::PastCap`] when the picture comes to
/// more than [`Svg::MOST`] bytes.
pub(crate) fn picture(lines: &[String], natural: usize) -> Result<Svg, Undrawn> {
    let mut drawn = Drawn::default();
    for (row, line) in lines.iter().enumerate() {
        drawn.row(row, line)?;
        if drawn.text.len() > Svg::MOST {
            return Err(Undrawn::PastCap);
        }
    }
    let width = natural.saturating_mul(CELL_WIDTH);
    let height = lines.len().saturating_mul(CELL_HEIGHT);
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\" font-size=\"{FONT_SIZE}\">"
    );
    let plain = Slot::Plain.class();
    for (stroke, lines) in Stroke::ALL.into_iter().zip(&drawn.lines) {
        let mut data = lines.data(stroke);
        if stroke == Stroke::Light {
            data.push_str(&drawn.slants);
        }
        if !data.is_empty() {
            let _ = write!(
                svg,
                "<path class=\"{plain}\" fill=\"none\" stroke=\"currentColor\" {} d=\"{data}\"/>",
                stroke.drawn()
            );
        }
    }
    if !drawn.outlined.is_empty() {
        let _ = write!(
            svg,
            "<path class=\"{plain}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1\" d=\"{}\"/>",
            drawn.outlined
        );
    }
    if !drawn.filled.is_empty() {
        let _ = write!(
            svg,
            "<path class=\"{plain}\" fill=\"currentColor\" d=\"{}\"/>",
            drawn.filled
        );
    }
    if !drawn.shaded.is_empty() {
        let _ = write!(
            svg,
            "<path class=\"{}\" fill=\"currentColor\" d=\"{}\"/>",
            Slot::Quiet.class(),
            drawn.shaded
        );
    }
    svg.push_str(&drawn.text);
    svg.push_str("</svg>");
    Svg::new(svg)
}

impl Drawn {
    /// Draws `line`, the row at `row`.
    ///
    /// # Errors
    ///
    /// [`Undrawn::NotHere`] when `line` writes a letter outside
    /// [`PICTURE_LETTERS`].
    fn row(&mut self, row: usize, line: &str) -> Result<(), Undrawn> {
        let top = row.saturating_mul(CELL_HEIGHT);
        let mut column: usize = 0;
        let mut run = Run {
            top,
            ..Run::default()
        };
        for character in line.chars() {
            let width = columns(character.encode_utf8(&mut [0; 4]));
            match shape(character) {
                Some(shape) => {
                    run.end(&mut self.text);
                    self.shape(shape, column.saturating_mul(CELL_WIDTH), top);
                }
                None if !written(character) => return Err(Undrawn::NotHere),
                None => run.take(&mut self.text, column, width, character),
            }
            column = column.saturating_add(width);
        }
        run.end(&mut self.text);
        Ok(())
    }

    /// Draws `shape` in the cell whose top left is at `x` and `y`.
    fn shape(&mut self, shape: Shape, x: usize, y: usize) {
        let (middle_x, middle_y) = (x + CELL_WIDTH / 2, y + CELL_HEIGHT / 2);
        let (right, bottom) = (x + CELL_WIDTH, y + CELL_HEIGHT);
        match shape {
            Shape::Arms([up, across_right, down, across_left]) => {
                let reaches = [
                    (up, false, y, middle_y),
                    (down, false, middle_y, bottom),
                    (across_left, true, x, middle_x),
                    (across_right, true, middle_x, right),
                ];
                for (stroke, across, from, to) in reaches {
                    let Some(stroke) = stroke else { continue };
                    let Some(lines) = Stroke::ALL
                        .iter()
                        .position(|each| *each == stroke)
                        .and_then(|at| self.lines.get_mut(at))
                    else {
                        continue;
                    };
                    if across {
                        lines.across.push(Piece {
                            at: middle_y,
                            from,
                            to,
                        });
                    } else {
                        lines.down.push(Piece {
                            at: middle_x,
                            from,
                            to,
                        });
                    }
                }
            }
            Shape::Rising => {
                let _ = write!(self.slants, "M{x} {bottom}L{right} {y}");
            }
            Shape::Falling => {
                let _ = write!(self.slants, "M{x} {y}L{right} {bottom}");
            }
            Shape::Block(eighths) => {
                let _ = write!(self.filled, "M{x} {y}h{eighths}v{CELL_HEIGHT}h-{eighths}z");
            }
            Shape::Shade => {
                let _ = write!(
                    self.shaded,
                    "M{x} {y}h{CELL_WIDTH}v{CELL_HEIGHT}h-{CELL_WIDTH}z"
                );
            }
            Shape::Point(pointing, filled) => {
                let corners = match pointing {
                    Pointing::Up => [(x + 1, y + 12), (middle_x, y + 4), (x + 7, y + 12)],
                    Pointing::Right => [(x + 1, y + 3), (x + 7, middle_y), (x + 1, y + 13)],
                    Pointing::Down => [(x + 1, y + 4), (middle_x, y + 12), (x + 7, y + 4)],
                    Pointing::Left => [(x + 7, y + 3), (x + 1, middle_y), (x + 7, y + 13)],
                };
                let [(x1, y1), (x2, y2), (x3, y3)] = corners;
                let _ = write!(self.mark(filled), "M{x1} {y1}L{x2} {y2}L{x3} {y3}z");
            }
            Shape::Diamond(filled) => {
                let _ = write!(
                    self.mark(filled),
                    "M{middle_x} {}L{} {middle_y}L{middle_x} {}L{} {middle_y}z",
                    y + 3,
                    x + 7,
                    y + 13,
                    x + 1
                );
            }
            Shape::Circle(filled, radius) => {
                let _ = write!(
                    self.mark(filled),
                    "M{} {middle_y}a{radius} {radius} 0 1 0 {} 0a{radius} {radius} 0 1 0 -{} 0z",
                    middle_x - radius,
                    radius * 2,
                    radius * 2
                );
            }
            Shape::Chevron => {
                let _ = write!(
                    self.outlined,
                    "M{} {}L{} {middle_y}L{} {}",
                    x + 2,
                    y + 3,
                    x + 6,
                    x + 2,
                    y + 13
                );
            }
        }
    }

    /// Where a mark is drawn: filled, or outlined.
    fn mark(&mut self, filled: bool) -> &mut String {
        if filled {
            &mut self.filled
        } else {
            &mut self.outlined
        }
    }
}

impl Lines {
    /// The path data that draws these pieces as `stroke`: the pieces of a
    /// row or a column that meet, merged into one line.
    fn data(&self, stroke: Stroke) -> String {
        let mut data = String::new();
        for (pieces, across) in [(&self.across, true), (&self.down, false)] {
            for piece in merged(pieces) {
                for offset in stroke.offsets() {
                    let at = piece.at.saturating_add_signed(*offset);
                    let Piece { from, to, .. } = piece;
                    let _ = if across {
                        write!(data, "M{from} {at}H{to}")
                    } else {
                        write!(data, "M{at} {from}V{to}")
                    };
                }
            }
        }
        data
    }
}

/// `pieces`, with those along the same line that meet or overlap made one.
fn merged(pieces: &[Piece]) -> Vec<Piece> {
    let mut sorted = pieces.to_vec();
    sorted.sort_unstable();
    let mut merged: Vec<Piece> = Vec::with_capacity(sorted.len());
    for piece in sorted {
        match merged.last_mut() {
            Some(last) if last.at == piece.at && piece.from <= last.to => {
                last.to = last.to.max(piece.to);
            }
            _ => merged.push(piece),
        }
    }
    merged
}

/// A run of text being gathered along a row: letters with single spaces
/// between them. Two spaces, or a shape, end it.
#[derive(Debug, Default)]
struct Run {
    /// The top of the row it stands in, in the picture's units.
    top: usize,
    /// The column the run starts at.
    start: usize,
    /// The column after its last letter.
    end: usize,
    /// Its letters, escaped for XML.
    letters: String,
    /// Spaces seen since its last letter.
    spaces: usize,
}

impl Run {
    /// Takes `character`, `width` columns wide at `column`, into the run, or
    /// ends the run where it is the second space in a row.
    fn take(&mut self, text: &mut String, column: usize, width: usize, character: char) {
        if character == ' ' {
            if !self.letters.is_empty() {
                self.spaces += 1;
                if self.spaces > 1 {
                    self.end(text);
                }
            }
            return;
        }
        if self.letters.is_empty() {
            self.start = column;
        } else {
            for _ in 0..self.spaces {
                self.letters.push(' ');
            }
        }
        self.spaces = 0;
        escape(&mut self.letters, character);
        self.end = column.saturating_add(width);
    }

    /// Writes the run gathered so far, if any, as a `<text>` in its row, and
    /// starts afresh in the same row.
    fn end(&mut self, text: &mut String) {
        if !self.letters.is_empty() {
            let _ = write!(
                text,
                "<text class=\"{}\" fill=\"currentColor\" x=\"{}\" y=\"{}\" textLength=\"{}\">{}</text>",
                Slot::Plain.class(),
                self.start.saturating_mul(CELL_WIDTH),
                self.top.saturating_add(BASELINE),
                self.end
                    .saturating_sub(self.start)
                    .saturating_mul(CELL_WIDTH),
                self.letters
            );
        }
        *self = Self {
            top: self.top,
            ..Self::default()
        };
    }
}

/// Whether a picture writes `character`: whether it is one of
/// [`PICTURE_LETTERS`].
fn written(character: char) -> bool {
    PICTURE_LETTERS
        .iter()
        .any(|letters| letters.contains(&character))
}

/// `character` written into `text` as XML reads it back.
fn escape(text: &mut String, character: char) {
    match character {
        '&' => text.push_str("&amp;"),
        '<' => text.push_str("&lt;"),
        '>' => text.push_str("&gt;"),
        '"' => text.push_str("&quot;"),
        '\'' => text.push_str("&apos;"),
        _ => text.push(character),
    }
}

#[cfg(test)]
mod tests;
