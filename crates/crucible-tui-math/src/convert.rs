//! TeX math read into text: compact text for a line, or rows of text for a
//! block.
//!
//! Ported from jcode's math renderer (`jcode-render-core/src/math.rs` at
//! commit b2f9f9d8), whose notice follows. The port keeps its parser and its
//! layout, and differs where a drawing has to be the formula or nothing: a
//! part it does not know is a refusal rather than text kept as written, the
//! input, its depth and the layout are capped, and the structural characters
//! of a layout are ASCII under the ASCII glyph set.
//!
//! ```text
//! MIT License
//!
//! Copyright (c) 2025 Jeremy Huang
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.
//! ```

use crucible_tui::{Glyphs, columns};

/// The most bytes of TeX one span or block is read from.
pub const MOST_BYTES: usize = 4 * 1024;

/// The deepest groups, arguments and environments nest before the formula is
/// refused rather than read further.
pub const MOST_DEPTH: usize = 64;

/// The most rows a block's layout reaches before it is refused.
pub const MOST_ROWS: usize = 48;

/// The most columns any layout reaches before it is refused, whatever the
/// room: a bound on the work, not the width it is drawn at.
pub const MOST_COLUMNS: usize = 1024;

/// Why a formula is not drawn as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Its source is past [`MOST_BYTES`].
    TooLong,
    /// Its layout or its nesting is past [`MOST_ROWS`], [`MOST_COLUMNS`] or
    /// [`MOST_DEPTH`].
    TooLarge,
    /// A part of it does not convert: a command or environment this does not
    /// know, a group or environment left open, a character that means
    /// something only to TeX, or one a terminal would act on.
    Unclean,
    /// It holds nothing to draw.
    Empty,
}

type Read<T> = Result<T, Refused>;

/// `source` as compact text for an ordinary line.
pub(crate) fn inline(source: &str, glyphs: Glyphs) -> Read<String> {
    let expr = parsed(source, glyphs)?;
    let compact = expr.compact(glyphs);
    if compact.trim().is_empty() {
        return Err(Refused::Empty);
    }
    if columns(&compact) > MOST_COLUMNS {
        return Err(Refused::TooLarge);
    }
    Ok(compact.trim().to_owned())
}

/// `source` laid out as rows of text, with no row ending in a space and no
/// blank row above or below.
pub(crate) fn display(source: &str, glyphs: Glyphs) -> Read<Vec<String>> {
    let expr = parsed(source, glyphs)?;
    let layout = expr.layout(glyphs)?;
    let mut lines: Vec<String> = layout
        .lines
        .iter()
        .map(|line| line.trim_end().to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let blank = lines.iter().take_while(|line| line.is_empty()).count();
    lines.drain(..blank);
    if lines.is_empty() {
        return Err(Refused::Empty);
    }
    // What every row is indented by is no part of the formula.
    let indent = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.chars()
                .take_while(|character| *character == ' ')
                .count()
        })
        .min()
        .unwrap_or(0);
    Ok(lines
        .into_iter()
        .map(|line| line.chars().skip(indent).collect())
        .collect())
}

fn parsed(source: &str, glyphs: Glyphs) -> Read<Expr> {
    if source.len() > MOST_BYTES {
        return Err(Refused::TooLong);
    }
    if source.chars().any(obeyed) {
        return Err(Refused::Unclean);
    }
    let source = source.trim();
    if source.is_empty() {
        return Err(Refused::Empty);
    }
    Parser::new(source, glyphs).sequence(None, 0)
}

/// Whether a terminal would act on `character`, or draw it so the characters
/// around it read as something they are not. A line's end and a tab are
/// spaces to TeX.
fn obeyed(character: char) -> bool {
    (character.is_control() && !matches!(character, '\n' | '\t' | '\r'))
        || matches!(
            character,
            '\u{61c}'
                | '\u{200b}'
                | '\u{200e}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2069}'
                | '\u{feff}'
        )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Sequence(Vec<Expr>),
    Text(String),
    Fraction(Box<Expr>, Box<Expr>),
    Root(Option<Box<Expr>>, Box<Expr>),
    Scripts {
        base: Box<Expr>,
        superscript: Option<Box<Expr>>,
        subscript: Option<Box<Expr>>,
    },
    Matrix {
        rows: Vec<Vec<Expr>>,
        delimiters: Delimiters,
        cells: Cells,
    },
}

/// What stands either side of a matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delimiters {
    None,
    Parentheses,
    Brackets,
    Braces,
    LeftBrace,
    Bars,
    DoubleBars,
}

/// How the cells of a matrix stand in their columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cells {
    /// Each centred, as a matrix's are.
    Centred,
    /// Each against the left, as the cases of `cases` are.
    Left,
    /// Pairs meeting at the mark they align on, as `align`'s are.
    Aligned,
}

impl Expr {
    fn compact(&self, glyphs: Glyphs) -> String {
        match self {
            Self::Sequence(items) => items.iter().map(|item| item.compact(glyphs)).collect(),
            Self::Text(text) => text.clone(),
            Self::Fraction(numerator, denominator) => format!(
                "{}/{}",
                compact_group(numerator, glyphs),
                compact_group(denominator, glyphs)
            ),
            Self::Root(index, radicand) => match index {
                Some(index) => format!(
                    "{}√{}",
                    script_or_group(&index.compact(glyphs), true),
                    compact_group(radicand, glyphs)
                ),
                None => format!("√{}", compact_group(radicand, glyphs)),
            },
            Self::Scripts {
                base,
                superscript,
                subscript,
            } => {
                let mut out = base.compact(glyphs);
                if let Some(subscript) = subscript {
                    out.push_str(&script_or_group(&subscript.compact(glyphs), false));
                }
                if let Some(superscript) = superscript {
                    out.push_str(&script_or_group(&superscript.compact(glyphs), true));
                }
                out
            }
            Self::Matrix {
                rows, delimiters, ..
            } => {
                let body = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell| cell.compact(glyphs))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                let (left, right) = delimiters.compact_pair(glyphs);
                format!("{left}{body}{right}")
            }
        }
    }

    fn layout(&self, glyphs: Glyphs) -> Read<Layout> {
        let laid = match self {
            Self::Sequence(items) => Layout::horizontal(
                items
                    .iter()
                    .map(|item| item.layout(glyphs))
                    .collect::<Read<Vec<_>>>()?,
            ),
            Self::Text(text) => Layout::text(text.clone()),
            Self::Fraction(numerator, denominator) => Layout::fraction(
                &numerator.layout(glyphs)?,
                &denominator.layout(glyphs)?,
                glyphs,
            ),
            Self::Root(index, radicand) => {
                // A root has no bar over what it takes in text, so what it
                // takes is grouped the way the compact text groups it: else
                // `√b² − 4ac` reads as a root of `b²` alone.
                let radical = if grouped(radicand, glyphs) {
                    Layout::horizontal(vec![
                        Layout::text("√("),
                        radicand.layout(glyphs)?,
                        Layout::text(")"),
                    ])
                } else {
                    Layout::horizontal(vec![Layout::text("√"), radicand.layout(glyphs)?])
                };
                match index {
                    Some(index) => match map_script(&index.compact(glyphs), true) {
                        Some(script) => Layout::horizontal(vec![Layout::text(script), radical]),
                        None => Layout::horizontal(vec![
                            Layout::scripts(Layout::text(""), Some(index.layout(glyphs)?), None),
                            radical,
                        ]),
                    },
                    None => radical,
                }
            }
            Self::Scripts {
                base,
                superscript,
                subscript,
            } => {
                let up = superscript
                    .as_deref()
                    .map(|script| map_script(&script.compact(glyphs), true));
                let down = subscript
                    .as_deref()
                    .map(|script| map_script(&script.compact(glyphs), false));
                if up.as_ref().is_none_or(Option::is_some)
                    && down.as_ref().is_none_or(Option::is_some)
                {
                    let mut scripts = String::new();
                    if let Some(Some(down)) = down {
                        scripts.push_str(&down);
                    }
                    if let Some(Some(up)) = up {
                        scripts.push_str(&up);
                    }
                    Layout::horizontal(vec![base.layout(glyphs)?, Layout::text(scripts)])
                } else {
                    Layout::scripts(
                        base.layout(glyphs)?,
                        superscript
                            .as_deref()
                            .map(|script| script.layout(glyphs))
                            .transpose()?,
                        subscript
                            .as_deref()
                            .map(|script| script.layout(glyphs))
                            .transpose()?,
                    )
                }
            }
            Self::Matrix {
                rows,
                delimiters,
                cells,
            } => Layout::matrix(rows, *delimiters, *cells, glyphs),
        };
        laid.bounded()
    }
}

/// `expr` as compact text, in parentheses unless it is one thing already.
///
/// Its text is made once: made again to ask whether to group it, every level
/// of a nested fraction would make the levels under it twice, and a span the
/// cap holds would take seconds.
fn compact_group(expr: &Expr, glyphs: Glyphs) -> String {
    let rendered = expr.compact(glyphs);
    if stands_apart(expr, &rendered) {
        format!("({rendered})")
    } else {
        rendered
    }
}

/// Whether `expr` is more than one thing, and is put in parentheses where it
/// stands as one.
fn grouped(expr: &Expr, glyphs: Glyphs) -> bool {
    stands_apart(expr, &expr.compact(glyphs))
}

/// Whether `expr`, whose compact text is `rendered`, is more than one thing.
fn stands_apart(expr: &Expr, rendered: &str) -> bool {
    !matches!(
        expr,
        Expr::Text(_) | Expr::Scripts { .. } | Expr::Root(_, _)
    ) && rendered.chars().count() != 1
}

/// `text` raised or lowered: in the script characters where every character
/// has one, else after a caret or an underscore, in parentheses unless it is
/// one character.
fn script_or_group(text: &str, superscript: bool) -> String {
    map_script(text, superscript).unwrap_or_else(|| {
        let mark = if superscript { '^' } else { '_' };
        if text.chars().count() == 1 {
            format!("{mark}{text}")
        } else {
            format!("{mark}({text})")
        }
    })
}

fn map_script(text: &str, superscript: bool) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    text.chars()
        .map(|character| {
            if superscript {
                raised(character)
            } else {
                lowered(character)
            }
        })
        .collect()
}

fn raised(character: char) -> Option<char> {
    Some(match character {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' | '−' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

fn lowered(character: char) -> Option<char> {
    Some(match character {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' | '−' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        _ => return None,
    })
}

struct Parser<'a> {
    source: &'a str,
    at: usize,
    glyphs: Glyphs,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, glyphs: Glyphs) -> Self {
        Self {
            source,
            at: 0,
            glyphs,
        }
    }

    /// Atoms with their scripts up to `closer`, which must come, or to the end
    /// where there is none.
    fn sequence(&mut self, closer: Option<char>, depth: usize) -> Read<Expr> {
        if depth > MOST_DEPTH {
            return Err(Refused::TooLarge);
        }
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None if closer.is_some() => return Err(Refused::Unclean),
                None => break,
                Some(character) if Some(character) == closer => {
                    self.bump();
                    break;
                }
                Some(_) => {}
            }
            let base = self.atom(depth)?;
            // Math takes no space from the source: one already standing, as
            // after a `\text{if }`, is the space there is.
            if base == Expr::Text(" ".to_owned())
                && !matches!(self.peek(), Some('^' | '_'))
                && items
                    .last()
                    .is_some_and(|last| matches!(last, Expr::Text(text) if text.ends_with(' ')))
            {
                continue;
            }
            let mut superscript = None;
            let mut subscript = None;
            loop {
                let slot = match self.peek() {
                    Some('^') => &mut superscript,
                    Some('_') => &mut subscript,
                    _ => break,
                };
                // TeX refuses a second script of the same kind.
                if slot.is_some() {
                    return Err(Refused::Unclean);
                }
                self.bump();
                *slot = Some(Box::new(self.argument(depth.saturating_add(1))?));
            }
            items.push(if superscript.is_some() || subscript.is_some() {
                Expr::Scripts {
                    base: Box::new(base),
                    superscript,
                    subscript,
                }
            } else {
                base
            });
        }
        Ok(collapse(items))
    }

    fn atom(&mut self, depth: usize) -> Read<Expr> {
        match self.peek() {
            Some('{') => {
                self.bump();
                self.sequence(Some('}'), depth.saturating_add(1))
            }
            Some('\\') => self.command(depth),
            Some(character) if character.is_whitespace() || character == '~' => {
                while self
                    .peek()
                    .is_some_and(|next| next.is_whitespace() || next == '~')
                {
                    self.bump();
                }
                Ok(Expr::Text(" ".to_owned()))
            }
            // Each means something to TeX alone where it stands here: a cell's
            // end, a comment, a parameter, the end of math, a script with no
            // base, a group closed that was never opened.
            Some('&' | '%' | '#' | '$' | '^' | '_' | '}') | None => Err(Refused::Unclean),
            Some('-') => {
                self.bump();
                Ok(Expr::Text(
                    match self.glyphs {
                        Glyphs::Unicode => "−",
                        Glyphs::Ascii => "-",
                    }
                    .to_owned(),
                ))
            }
            Some(character) => {
                self.bump();
                Ok(Expr::Text(character.to_string()))
            }
        }
    }

    fn argument(&mut self, depth: usize) -> Read<Expr> {
        self.skip_spaces();
        self.atom(depth)
    }

    fn command(&mut self, depth: usize) -> Read<Expr> {
        self.bump();
        let start = self.at;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_alphabetic())
        {
            self.bump();
        }
        let name = if self.at == start {
            self.bump().ok_or(Refused::Unclean)?.to_string()
        } else {
            self.slice(start, self.at).to_owned()
        };
        let word = name
            .chars()
            .all(|character| character.is_ascii_alphabetic());
        let separated = word && self.peek().is_some_and(char::is_whitespace);
        if word {
            self.skip_spaces();
        }
        let deeper = depth.saturating_add(1);
        if deeper > MOST_DEPTH {
            return Err(Refused::TooLarge);
        }

        Ok(match name.as_str() {
            "frac" | "dfrac" | "tfrac" => Expr::Fraction(
                Box::new(self.argument(deeper)?),
                Box::new(self.argument(deeper)?),
            ),
            "sqrt" => {
                let index = if self.peek() == Some('[') {
                    self.bump();
                    let raw = self.bracketed()?;
                    Some(Box::new(
                        Parser::new(raw, self.glyphs).sequence(None, deeper)?,
                    ))
                } else {
                    None
                };
                Expr::Root(index, Box::new(self.argument(deeper)?))
            }
            "text" | "textrm" | "textsf" | "texttt" | "textit" | "textbf" | "textnormal"
            | "mbox" | "operatorname" => Expr::Text(self.text()?),
            "begin" => self.environment(deeper)?,
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl"
            | "Bigr" | "biggl" | "biggr" | "Biggl" | "Biggr" | "middle" => {
                Expr::Text(self.delimiter()?)
            }
            "overline" => self.marked('\u{0305}', deeper)?,
            "underline" => self.marked('\u{0332}', deeper)?,
            "hat" | "widehat" => self.marked('\u{0302}', deeper)?,
            "bar" => self.marked('\u{0304}', deeper)?,
            "vec" => self.marked('\u{20d7}', deeper)?,
            "dot" => self.marked('\u{0307}', deeper)?,
            "ddot" => self.marked('\u{0308}', deeper)?,
            "tilde" | "widetilde" => self.marked('\u{0303}', deeper)?,
            "mathbf" | "mathrm" | "mathit" | "mathsf" | "mathtt" | "mathcal" | "mathbb"
            | "mathfrak" | "boldsymbol" | "bm" => self.argument(deeper)?,
            "displaystyle" | "textstyle" | "scriptstyle" | "limits" | "nolimits" | "!" => {
                Expr::Text(String::new())
            }
            "," | ";" | ":" | ">" | " " | "quad" => Expr::Text(" ".to_owned()),
            "qquad" => Expr::Text("  ".to_owned()),
            _ => {
                let mut symbol = self.named(&name)?.to_owned();
                // The space after a word ends its name in TeX, and a reader
                // expects `\alpha \leq \pi` to keep its spacing; not before a
                // script, so `\alpha ^2` still reads as α².
                if separated && !matches!(self.peek(), Some('^' | '_')) {
                    symbol.push(' ');
                }
                Expr::Text(symbol)
            }
        })
    }

    /// The words of a `\text{…}`, as they were written but for the escapes
    /// TeX's text keeps, with every run of spaces one space.
    fn text(&mut self) -> Read<String> {
        self.skip_spaces();
        if self.bump() != Some('{') {
            return Err(Refused::Unclean);
        }
        let mut depth = 1_usize;
        let mut out = String::new();
        loop {
            match self.bump().ok_or(Refused::Unclean)? {
                '{' => depth = depth.saturating_add(1),
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                '\\' => match self.bump() {
                    Some(escaped @ ('{' | '}' | '$' | '%' | '&' | '_' | '#')) => out.push(escaped),
                    Some(' ') => out.push(' '),
                    _ => return Err(Refused::Unclean),
                },
                '$' | '^' | '%' | '#' | '&' => return Err(Refused::Unclean),
                space if space.is_whitespace() || space == '~' => {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn environment(&mut self, depth: usize) -> Read<Expr> {
        let name = self.name()?;
        // Known before its end is looked for, so the walk is for a name of a
        // few letters. `None` is an environment read as one formula.
        let laid = match name.as_str() {
            "equation" | "equation*" | "displaymath" => None,
            "matrix" | "smallmatrix" | "array" => Some((Delimiters::None, Cells::Centred)),
            "align" | "align*" | "aligned" | "split" | "eqnarray" | "eqnarray*" | "alignat"
            | "alignat*" => Some((Delimiters::None, Cells::Aligned)),
            "gather" | "gather*" | "gathered" | "multline" | "multline*" => {
                Some((Delimiters::None, Cells::Centred))
            }
            "cases" | "cases*" => Some((Delimiters::LeftBrace, Cells::Left)),
            "pmatrix" => Some((Delimiters::Parentheses, Cells::Centred)),
            "bmatrix" => Some((Delimiters::Brackets, Cells::Centred)),
            "Bmatrix" => Some((Delimiters::Braces, Cells::Centred)),
            "vmatrix" => Some((Delimiters::Bars, Cells::Centred)),
            "Vmatrix" => Some((Delimiters::DoubleBars, Cells::Centred)),
            _ => return Err(Refused::Unclean),
        };
        let (end, after) = environment_end(
            self.source,
            self.at,
            &name,
            MOST_DEPTH.saturating_sub(depth),
        )?;
        let body = self.slice(self.at, end);
        self.at = after;
        let Some((delimiters, cells)) = laid else {
            return Parser::new(body, self.glyphs).sequence(None, depth);
        };

        let body = if matches!(name.as_str(), "array" | "alignat" | "alignat*") {
            leading_group_dropped(body).ok_or(Refused::Unclean)?
        } else {
            body
        };
        let split = split_matrix(body);
        if split.len() > MOST_ROWS {
            return Err(Refused::TooLarge);
        }
        let rows = split
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| Parser::new(cell.trim(), self.glyphs).sequence(None, depth))
                    .collect::<Read<Vec<_>>>()
            })
            .collect::<Read<Vec<_>>>()?;
        Ok(Expr::Matrix {
            rows,
            delimiters,
            cells,
        })
    }

    /// `body` with `mark` after each of its characters.
    fn marked(&mut self, mark: char, depth: usize) -> Read<Expr> {
        let body = self.argument(depth)?.compact(self.glyphs);
        Ok(Expr::Text(
            body.chars()
                .flat_map(|character| [character, mark])
                .collect(),
        ))
    }

    /// What follows `\left`, `\right` and the sizes: one of the characters or
    /// commands TeX takes as a delimiter.
    fn delimiter(&mut self) -> Read<String> {
        self.skip_spaces();
        Ok(match self.bump().ok_or(Refused::Unclean)? {
            '.' => String::new(),
            character @ ('(' | ')' | '[' | ']' | '|' | '/') => character.to_string(),
            '<' => self.named("langle")?.to_owned(),
            '>' => self.named("rangle")?.to_owned(),
            '\\' => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|character| character.is_ascii_alphabetic())
                {
                    self.bump();
                }
                if self.at == start {
                    match self.bump() {
                        Some(character @ ('{' | '}')) => character.to_string(),
                        Some('|') => self.named("|")?.to_owned(),
                        _ => return Err(Refused::Unclean),
                    }
                } else {
                    let name = self.slice(start, self.at);
                    match name {
                        "langle" | "rangle" | "lceil" | "rceil" | "lfloor" | "rfloor" | "vert"
                        | "Vert" | "lvert" | "rvert" | "lVert" | "rVert" | "lbrace" | "rbrace"
                        | "uparrow" | "downarrow" | "backslash" => self.named(name)?.to_owned(),
                        _ => return Err(Refused::Unclean),
                    }
                }
            }
            _ => return Err(Refused::Unclean),
        })
    }

    /// The character a command names, in this formula's glyph set.
    ///
    /// A double bar and an angle bracket are bracket pieces, so under ASCII
    /// they are drawn as two bars and as the less-than and greater-than
    /// signs. A ceiling or a floor has no ASCII shape that does not read as a
    /// bracket, so under ASCII it refuses the formula, which is then shown as
    /// written. Every other character is the formula's content and is drawn
    /// as itself in either set.
    fn named(&self, command: &str) -> Read<&'static str> {
        match (command, self.glyphs) {
            ("Vert" | "lVert" | "rVert" | "|", Glyphs::Ascii) => Ok("||"),
            ("langle", Glyphs::Ascii) => Ok("<"),
            ("rangle", Glyphs::Ascii) => Ok(">"),
            ("lceil" | "rceil" | "lfloor" | "rfloor", Glyphs::Ascii) => Err(Refused::Unclean),
            _ => symbol(command).ok_or(Refused::Unclean),
        }
    }

    /// An environment's name, in its braces.
    fn name(&mut self) -> Read<String> {
        self.skip_spaces();
        if self.bump() != Some('{') {
            return Err(Refused::Unclean);
        }
        let start = self.at;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '*')
        {
            self.bump();
        }
        let name = self.slice(start, self.at).to_owned();
        if name.is_empty() || self.bump() != Some('}') {
            return Err(Refused::Unclean);
        }
        Ok(name)
    }

    /// What stands between a `[` already read and the `]` that closes it.
    fn bracketed(&mut self) -> Read<&'a str> {
        let start = self.at;
        let mut brackets = 1_usize;
        let mut braces = 0_usize;
        while let Some(character) = self.bump() {
            match character {
                '\\' => {
                    self.bump();
                }
                '{' => braces = braces.saturating_add(1),
                '}' => braces = braces.saturating_sub(1),
                '[' if braces == 0 => brackets = brackets.saturating_add(1),
                ']' if braces == 0 => {
                    brackets = brackets.saturating_sub(1);
                    if brackets == 0 {
                        return Ok(self.slice(start, self.at.saturating_sub(1)));
                    }
                }
                _ => {}
            }
        }
        Err(Refused::Unclean)
    }

    fn skip_spaces(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn slice(&self, from: usize, to: usize) -> &'a str {
        self.source.get(from..to).unwrap_or_default()
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.at..)?.chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.at = self.at.saturating_add(character.len_utf8());
        Some(character)
    }
}

fn collapse(mut items: Vec<Expr>) -> Expr {
    match (items.pop(), items.is_empty()) {
        (Some(only), true) => only,
        (Some(last), false) => {
            items.push(last);
            Expr::Sequence(items)
        }
        (None, _) => Expr::Sequence(items),
    }
}

/// Where the body of the environment `name` that opens before `from` ends,
/// and where the text after its `\end{name}` begins, in one walk over what
/// follows: nested environments of the same name are counted, and one nested
/// more than `room` levels inside it is refused where it opens.
///
/// Each nested environment is read one level deeper than the one it is in, so
/// one past `room` is past [`MOST_DEPTH`] however its body reads, and refusing
/// it here spares every level above it its own walk. Every environment that
/// is read walks its own body once, and no more than [`MOST_DEPTH`] of them
/// are read inside one another, so a formula costs a bounded number of walks
/// over its bytes.
fn environment_end(source: &str, from: usize, name: &str, room: usize) -> Read<(usize, usize)> {
    let begin = format!("\\begin{{{name}}}");
    let end = format!("\\end{{{name}}}");
    let rest = source.get(from..).ok_or(Refused::Unclean)?;
    let mut at = 0_usize;
    let mut depth = 1_usize;
    while let Some(found) = rest.get(at..).and_then(|tail| tail.find('\\')) {
        let marker = at.saturating_add(found);
        let tail = rest.get(marker..).unwrap_or_default();
        if tail.starts_with(&begin) {
            depth = depth.saturating_add(1);
            if depth > room.saturating_add(1) {
                return Err(Refused::TooLarge);
            }
            at = marker.saturating_add(begin.len());
        } else if tail.starts_with(&end) {
            depth = depth.saturating_sub(1);
            at = marker.saturating_add(end.len());
            if depth == 0 {
                return Ok((from.saturating_add(marker), from.saturating_add(at)));
            }
        } else {
            // A backslash is one byte, so the next character starts after it.
            at = marker.saturating_add(1);
        }
    }
    Err(Refused::Unclean)
}

/// `source` without the braced group it opens with, as an `array`'s column
/// specification is.
fn leading_group_dropped(source: &str) -> Option<&str> {
    let trimmed = source.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let mut depth = 0_usize;
    for (at, character) in trimmed.char_indices() {
        match character {
            '{' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return trimmed.get(at.saturating_add(1)..);
                }
            }
            _ => {}
        }
    }
    None
}

/// An environment's body cut into rows at `\\` and cells at `&`, but for those
/// inside a group or a nested environment.
fn split_matrix(source: &str) -> Vec<Vec<&str>> {
    let bytes = source.as_bytes();
    let cut = |from: usize, to: usize| source.get(from..to).unwrap_or_default();
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let (mut start, mut at, mut braces, mut nested) = (0_usize, 0_usize, 0_usize, 0_usize);
    while let Some(&byte) = bytes.get(at) {
        let rest = source.get(at..).unwrap_or_default();
        match byte {
            b'\\' if rest.starts_with("\\begin{") || rest.starts_with("\\end{") => {
                if rest.starts_with("\\begin{") {
                    nested = nested.saturating_add(1);
                } else {
                    nested = nested.saturating_sub(1);
                }
                at = at.saturating_add(rest.find('}').unwrap_or(0));
            }
            b'\\'
                if bytes.get(at.saturating_add(1)) == Some(&b'\\')
                    && braces == 0
                    && nested == 0 =>
            {
                row.push(cut(start, at));
                rows.push(std::mem::take(&mut row));
                at = at.saturating_add(2);
                start = at;
                // A row's end may say how much room to leave under it.
                if let Some(close) = source
                    .get(at..)
                    .filter(|rest| rest.starts_with('['))
                    .and_then(|rest| rest.find(']'))
                {
                    at = at.saturating_add(close).saturating_add(1);
                    start = at;
                }
                continue;
            }
            b'\\' => at = at.saturating_add(1),
            b'{' => braces = braces.saturating_add(1),
            b'}' => braces = braces.saturating_sub(1),
            b'&' if braces == 0 && nested == 0 => {
                row.push(cut(start, at));
                start = at.saturating_add(1);
            }
            _ => {}
        }
        at = at.saturating_add(1);
    }
    row.push(source.get(start..).unwrap_or_default());
    rows.push(row);
    if rows.len() > 1
        && rows
            .last()
            .is_some_and(|row| row.iter().all(|cell| cell.trim().is_empty()))
    {
        rows.pop();
    }
    rows
}

/// The character a command names, for the commands that name one.
fn symbol(command: &str) -> Option<&'static str> {
    Some(match command {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "varphi" => "ϕ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "sum" => "∑",
        "prod" => "∏",
        "coprod" => "∐",
        "int" => "∫",
        "iint" => "∬",
        "iiint" => "∭",
        "oint" => "∮",
        "partial" => "∂",
        "nabla" => "∇",
        "infty" => "∞",
        "ell" => "ℓ",
        "hbar" => "ℏ",
        "times" => "×",
        "div" => "÷",
        "cdot" => "·",
        "circ" => "∘",
        "pm" => "±",
        "mp" => "∓",
        "ast" => "∗",
        "star" => "⋆",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "sim" => "∼",
        "simeq" => "≃",
        "equiv" => "≡",
        "propto" => "∝",
        "ll" => "≪",
        "gg" => "≫",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "supset" => "⊃",
        "subseteq" => "⊆",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "forall" => "∀",
        "exists" => "∃",
        "nexists" => "∄",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "vdash" => "⊢",
        "models" => "⊨",
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "Leftrightarrow" | "iff" => "⇔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "angle" => "∠",
        "degree" => "°",
        "prime" => "′",
        "perp" | "bot" => "⊥",
        "top" => "⊤",
        "parallel" => "∥",
        "mid" => "∣",
        "vert" | "lvert" | "rvert" => "|",
        "Vert" | "lVert" | "rVert" | "|" => "‖",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "backslash" => "\\",
        "%" => "%",
        "$" => "$",
        "#" => "#",
        "&" => "&",
        "_" => "_",
        "{" | "lbrace" => "{",
        "}" | "rbrace" => "}",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "sec" => "sec",
        "csc" => "csc",
        "cot" => "cot",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "log" => "log",
        "ln" => "ln",
        "exp" => "exp",
        "lim" => "lim",
        "sup" => "sup",
        "inf" => "inf",
        "min" => "min",
        "max" => "max",
        "det" => "det",
        "gcd" => "gcd",
        "mod" | "bmod" => "mod",
        _ => return None,
    })
}

/// Rows of text with one of them the baseline, the row the next thing to the
/// right lines up with.
///
/// Every row is exactly [`Self::width`] columns, padded on the right, so a
/// layout placed beside or centred over another moves as one piece, and its
/// width is counted once when it is made rather than again at every level it
/// is nested under: a continued fraction at the row cap would otherwise count
/// every row it holds once for each level above it, several times over.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    lines: Vec<String>,
    baseline: usize,
    width: usize,
}

impl Layout {
    fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            width: columns(&text),
            lines: vec![text],
            baseline: 0,
        }
    }

    /// `lines` padded to the widest of them, with `baseline` the row the next
    /// thing lines up with.
    fn padded(lines: Vec<String>, baseline: usize) -> Self {
        let counted: Vec<usize> = lines.iter().map(|line| columns(line)).collect();
        let width = counted.iter().copied().max().unwrap_or(0);
        let lines = lines
            .into_iter()
            .zip(counted)
            .map(|(mut line, counted)| {
                line.push_str(&" ".repeat(width.saturating_sub(counted)));
                line
            })
            .collect();
        Self {
            lines,
            baseline,
            width,
        }
    }

    const fn width(&self) -> usize {
        self.width
    }

    /// This layout, or its refusal where it is past the caps.
    fn bounded(self) -> Read<Self> {
        if self.lines.len() > MOST_ROWS || self.width() > MOST_COLUMNS {
            Err(Refused::TooLarge)
        } else {
            Ok(self)
        }
    }

    fn horizontal(items: Vec<Self>) -> Self {
        if items.is_empty() {
            return Self::text("");
        }
        let above = items.iter().map(|item| item.baseline).max().unwrap_or(0);
        let below = items
            .iter()
            .map(|item| {
                item.lines
                    .len()
                    .saturating_sub(item.baseline.saturating_add(1))
            })
            .max()
            .unwrap_or(0);
        let mut lines = vec![String::new(); above.saturating_add(below).saturating_add(1)];
        let mut width = 0_usize;
        for item in items {
            let offset = above.saturating_sub(item.baseline);
            for (row, line) in lines.iter_mut().enumerate() {
                match row.checked_sub(offset).and_then(|at| item.lines.get(at)) {
                    Some(content) => line.push_str(content),
                    None => line.push_str(&" ".repeat(item.width)),
                }
            }
            width = width.saturating_add(item.width);
        }
        Self {
            lines,
            baseline: above,
            width,
        }
    }

    fn fraction(numerator: &Self, denominator: &Self, glyphs: Glyphs) -> Self {
        let width = numerator
            .width()
            .max(denominator.width())
            .max(1)
            .saturating_add(2);
        let mut lines: Vec<String> = numerator
            .lines
            .iter()
            .map(|line| centre(line, numerator.width, width))
            .collect();
        let rule = match glyphs {
            Glyphs::Unicode => "─",
            Glyphs::Ascii => "-",
        };
        lines.push(rule.repeat(width));
        let baseline = lines.len().saturating_sub(1);
        lines.extend(
            denominator
                .lines
                .iter()
                .map(|line| centre(line, denominator.width, width)),
        );
        Self {
            lines,
            baseline,
            width,
        }
    }

    fn scripts(base: Self, superscript: Option<Self>, subscript: Option<Self>) -> Self {
        if superscript.is_none() && subscript.is_none() {
            return base;
        }
        let script_width = superscript
            .as_ref()
            .map_or(0, Self::width)
            .max(subscript.as_ref().map_or(0, Self::width));
        let raised = superscript.as_ref().map_or(0, |script| script.lines.len());
        let base_width = base.width;
        let width = base_width.saturating_add(script_width);
        let beside = |line: &str, line_width: usize| {
            format!(
                "{}{line}{}",
                " ".repeat(base_width),
                " ".repeat(script_width.saturating_sub(line_width))
            )
        };
        let mut lines = Vec::new();
        if let Some(superscript) = superscript {
            for line in superscript.lines {
                lines.push(beside(&line, superscript.width));
            }
        }
        let baseline = raised.saturating_add(base.baseline);
        for line in base.lines {
            lines.push(format!("{line}{}", " ".repeat(script_width)));
        }
        if let Some(subscript) = subscript {
            for line in subscript.lines {
                lines.push(beside(&line, subscript.width));
            }
        }
        Self {
            lines,
            baseline,
            width,
        }
    }

    fn matrix(rows: &[Vec<Expr>], delimiters: Delimiters, cells: Cells, glyphs: Glyphs) -> Self {
        let height = rows.len();
        if height == 0 {
            return Self::text("");
        }
        let count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let rendered: Vec<Vec<String>> = rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.compact(glyphs)).collect())
            .collect();
        let widths: Vec<usize> = (0..count)
            .map(|column| {
                rendered
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|text| columns(text))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let gap = match cells {
            Cells::Aligned => " ",
            Cells::Centred | Cells::Left => "  ",
        };
        let lines = rendered
            .iter()
            .enumerate()
            .map(|(at, row)| {
                let body = widths
                    .iter()
                    .enumerate()
                    .map(|(column, width)| {
                        let text = row.get(column).map_or("", String::as_str);
                        match cells {
                            Cells::Centred => centre(text, columns(text), *width),
                            // The left of each pair stands against the mark,
                            // the right of it after.
                            Cells::Aligned if column % 2 == 0 => pad_left(text, *width),
                            Cells::Left | Cells::Aligned => pad_right(text, *width),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(gap);
                let left = delimiters.piece(true, at, height, glyphs);
                let right = delimiters.piece(false, at, height, glyphs);
                match (left.is_empty(), right.is_empty()) {
                    (true, true) => body,
                    (false, true) => format!("{left} {body}"),
                    (true, false) => format!("{body} {right}"),
                    (false, false) => format!("{left} {body} {right}"),
                }
            })
            .collect();
        Self::padded(lines, height / 2)
    }
}

/// Where a row stands in a delimiter more than one row tall.
#[derive(Clone, Copy)]
enum Place {
    Top,
    Middle,
    Between,
    Bottom,
}

impl Delimiters {
    fn compact_pair(self, glyphs: Glyphs) -> (&'static str, &'static str) {
        match (self, glyphs) {
            (Self::None, _) => ("", ""),
            (Self::Parentheses, _) => ("(", ")"),
            (Self::Brackets, _) => ("[", "]"),
            (Self::Braces, _) => ("{", "}"),
            (Self::LeftBrace, _) => ("{", ""),
            (Self::Bars, _) => ("|", "|"),
            (Self::DoubleBars, Glyphs::Unicode) => ("‖", "‖"),
            (Self::DoubleBars, Glyphs::Ascii) => ("||", "||"),
        }
    }

    /// The piece of the delimiter on the `left` or the right of row `at` of
    /// `height`: the bracket pieces in the Unicode set, their ASCII shapes in
    /// the other.
    fn piece(self, left: bool, at: usize, height: usize, glyphs: Glyphs) -> &'static str {
        if height <= 1 {
            let (opening, closing) = self.compact_pair(glyphs);
            return if left { opening } else { closing };
        }
        let place = if at == 0 {
            Place::Top
        } else if at.saturating_add(1) == height {
            Place::Bottom
        } else if at == height / 2 {
            Place::Middle
        } else {
            Place::Between
        };
        let unicode = glyphs == Glyphs::Unicode;
        match (self, left, place) {
            (Self::None, _, _) | (Self::LeftBrace, false, _) => "",
            (Self::Parentheses, true, Place::Top) => {
                if unicode {
                    "⎛"
                } else {
                    "/"
                }
            }
            (Self::Parentheses, true, Place::Bottom) => {
                if unicode {
                    "⎝"
                } else {
                    "\\"
                }
            }
            (Self::Parentheses, true, _) => {
                if unicode {
                    "⎜"
                } else {
                    "|"
                }
            }
            (Self::Parentheses, false, Place::Top) => {
                if unicode {
                    "⎞"
                } else {
                    "\\"
                }
            }
            (Self::Parentheses, false, Place::Bottom) => {
                if unicode {
                    "⎠"
                } else {
                    "/"
                }
            }
            (Self::Parentheses, false, _) => {
                if unicode {
                    "⎟"
                } else {
                    "|"
                }
            }
            (Self::Brackets, true, Place::Top) => {
                if unicode {
                    "⎡"
                } else {
                    "["
                }
            }
            (Self::Brackets, true, Place::Bottom) => {
                if unicode {
                    "⎣"
                } else {
                    "["
                }
            }
            (Self::Brackets, true, _) => {
                if unicode {
                    "⎢"
                } else {
                    "["
                }
            }
            (Self::Brackets, false, Place::Top) => {
                if unicode {
                    "⎤"
                } else {
                    "]"
                }
            }
            (Self::Brackets, false, Place::Bottom) => {
                if unicode {
                    "⎦"
                } else {
                    "]"
                }
            }
            (Self::Brackets, false, _) => {
                if unicode {
                    "⎥"
                } else {
                    "]"
                }
            }
            (Self::Braces | Self::LeftBrace, true, Place::Top) => {
                if unicode {
                    "⎧"
                } else {
                    "/"
                }
            }
            (Self::Braces | Self::LeftBrace, true, Place::Middle) => {
                if unicode {
                    "⎨"
                } else {
                    "<"
                }
            }
            (Self::Braces | Self::LeftBrace, true, Place::Bottom) => {
                if unicode {
                    "⎩"
                } else {
                    "\\"
                }
            }
            // A brace's straight piece is the same on either side.
            (Self::Braces, _, Place::Between) | (Self::LeftBrace, true, Place::Between) => {
                if unicode {
                    "⎪"
                } else {
                    "|"
                }
            }
            (Self::Braces, false, Place::Top) => {
                if unicode {
                    "⎫"
                } else {
                    "\\"
                }
            }
            (Self::Braces, false, Place::Middle) => {
                if unicode {
                    "⎬"
                } else {
                    ">"
                }
            }
            (Self::Braces, false, Place::Bottom) => {
                if unicode {
                    "⎭"
                } else {
                    "/"
                }
            }
            (Self::Bars, _, _) => {
                if unicode {
                    "│"
                } else {
                    "|"
                }
            }
            (Self::DoubleBars, _, _) => {
                if unicode {
                    "‖"
                } else {
                    "||"
                }
            }
        }
    }
}

/// `text`, `counted` columns wide, centred in `width`.
fn centre(text: &str, counted: usize, width: usize) -> String {
    let padding = width.saturating_sub(counted);
    let left = padding / 2;
    format!(
        "{}{text}{}",
        " ".repeat(left),
        " ".repeat(padding.saturating_sub(left))
    )
}

fn pad_right(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(columns(text))))
}

fn pad_left(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(columns(text))))
}
