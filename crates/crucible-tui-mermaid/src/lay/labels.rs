//! A flowchart's or a state diagram's drawing read back in colour, so each
//! line's label is held to the line it labels.
//!
//! Invariants:
//!
//! - The engine places a line's label beside a stretch of that line without
//!   looking at the lines around it, so a label can land against another
//!   line, nearer it than its own, across it, or be left out where it finds
//!   no room. So the one layout is drawn with each line, each line's label,
//!   and the boxes in colours of their own, and a label left out, nearer
//!   another line than its own, or set across another line's stroke on its
//!   row refuses the diagram whole. A label as near its own line as any other
//!   is its own: where lines run side by side, one beside them is beside
//!   both. A label set over another line's stroke running down is held where
//!   it is as near its own: refusing it as well refuses about one labelled
//!   diagram in four the engine draws.
//! - A diagram with a labelled line is held only where the colours account
//!   for every cell. A cell holding a glyph with no colour, a line's colour
//!   on a cell holding no glyph of a line, refuses it, since the engine
//!   leaves some joins it stamps unpainted and draws text over painted cells,
//!   and the hold sees lines only by their colour.
//! - The drawing is the drawing in colour with its colours taken out, which
//!   is the drawing the engine makes without colour. Any colour this did not
//!   give, and any escape other than setting a colour or clearing it,
//!   refuses the diagram.

use std::fmt::Write as _;

use crucible_tui::columns;

use super::Unlaid;

/// `source` with each of the engine's `lines`, and its label, styled a
/// colour of its own, and every box and its text another two: line `n`,
/// counted from zero, drawn in `2n + 1` and labelled in `2n + 2`, and the
/// boxes drawn in the colour after the last label's and their text in the
/// one after that.
pub(super) fn painted(source: &str, lines: usize) -> String {
    let mut painted = source.trim_end().to_owned();
    for line in 0..lines {
        let _ = write!(
            painted,
            "\nlinkStyle {line} stroke:#{:06x},color:#{:06x}",
            2 * line + 1,
            2 * line + 2
        );
    }
    let _ = write!(
        painted,
        "\nclassDef DEFAULT stroke:#{:06x},color:#{:06x}",
        2 * lines + 1,
        2 * lines + 2
    );
    painted.push('\n');
    painted
}

/// `drawn`, the engine's drawing of [`painted`] in colour, with its colours
/// taken out, if each line `labelled` marks has its label drawn nearer its
/// own line than any other, and across no other.
pub(super) fn held(drawn: &str, labelled: &[bool]) -> Result<String, Unlaid> {
    let (plain, grid) = read(drawn).ok_or(Unlaid::Unread)?;
    let lines = labelled.len();
    let painted = |colour: usize| Painted::of(colour, lines);
    let cells = || grid.iter().flatten();
    if cells().any(|cell| cell.colour.is_some_and(|colour| painted(colour).is_none())) {
        return Err(Unlaid::Unread);
    }
    if !labelled.contains(&true) {
        return Ok(plain);
    }
    let accounted = |cell: &Cell| match cell.colour.and_then(painted) {
        None => cell.glyph == ' ',
        Some(Painted::Stroke(_)) => stroke(cell.glyph),
        Some(Painted::Label(_) | Painted::Box) => true,
    };
    if !cells().all(accounted) {
        return Err(Unlaid::Misplaced);
    }
    let runs = runs(&grid);
    let labels = labelled
        .iter()
        .enumerate()
        .filter(|(_, labelled)| **labelled);
    for (line, _) in labels {
        let label: Vec<&Run> = runs
            .iter()
            .filter(|run| painted(run.colour) == Some(Painted::Label(line)))
            .collect();
        let nearest = |own: bool| {
            runs.iter()
                .filter(|run| match painted(run.colour) {
                    Some(Painted::Stroke(stroke)) => (stroke == line) == own,
                    _ => false,
                })
                .flat_map(|stroke| label.iter().map(|label| label.apart(stroke)))
                .min()
        };
        match (nearest(true), nearest(false)) {
            (Some(own), other) if other.is_none_or(|other| own <= other) => {}
            _ => return Err(Unlaid::Misplaced),
        }
        if label.iter().any(|label| across(&grid, label, line, lines)) {
            return Err(Unlaid::Misplaced);
        }
    }
    Ok(plain)
}

/// What a colour [`painted`] gives paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Painted {
    /// The stroke of the line counted from zero.
    Stroke(usize),
    /// The label of the line counted from zero.
    Label(usize),
    /// A box, or its text.
    Box,
}

impl Painted {
    /// What `colour` paints in a drawing of `lines` lines; `None` for a
    /// colour [`painted`] does not give.
    fn of(colour: usize, lines: usize) -> Option<Self> {
        match colour {
            0 => None,
            colour if colour > 2 * lines + 2 => None,
            colour if colour > 2 * lines => Some(Self::Box),
            colour if colour % 2 == 1 => Some(Self::Stroke(colour / 2)),
            colour => Some(Self::Label(colour / 2 - 1)),
        }
    }
}

/// Whether `glyph` is one the engine draws a line with: a stroke, a turn, a
/// join, an arrowhead, or an end.
fn stroke(glyph: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&glyph) || "▸▾◂▴○×".contains(glyph)
}

/// Whether `glyph`, drawn as a line, runs on into the cell beside it on its
/// row, to its right when `rightward`, else to its left.
fn reaches(glyph: char, rightward: bool) -> bool {
    let toward = if rightward {
        "─━┄┅┈┉╌╍┌└├┬┴┼╭╰┏┗┣┳┻╋◂"
    } else {
        "─━┄┅┈┉╌╍┐┘┤┬┴┼╮╯┓┛┫┳┻╋▸"
    };
    toward.contains(glyph)
}

/// Whether `label`, a run of line `line`'s label in `grid`, a drawing of
/// `lines` lines, is set across another line on its row: whether the cells
/// on its two sides, past any label, are one other line's stroke running on
/// into it from each side.
fn across(grid: &[Vec<Cell>], label: &Run, line: usize, lines: usize) -> bool {
    let Some(row) = grid.get(label.row) else {
        return false;
    };
    // The line whose stroke runs into the label from the first cell past any
    // label, walking from `from` the way `rightward` says.
    let into = |from: usize, rightward: bool| {
        let mut at = from;
        loop {
            at = if rightward {
                at.checked_add(1)?
            } else {
                at.checked_sub(1)?
            };
            let beside = row.get(at)?;
            match beside
                .colour
                .and_then(|colour| Painted::of(colour, lines))?
            {
                Painted::Label(_) => {}
                Painted::Stroke(stroke) if stroke != line && reaches(beside.glyph, !rightward) => {
                    return Some(stroke);
                }
                Painted::Stroke(_) | Painted::Box => return None,
            }
        }
    };
    let last = label.to.saturating_sub(1);
    into(label.from, false).is_some_and(|stroke| into(last, true) == Some(stroke))
}

/// A cell of the drawing: the glyph it holds, a blank for the second column
/// of a wide one, and the colour it carries.
#[derive(Debug)]
struct Cell {
    glyph: char,
    colour: Option<usize>,
}

/// A stretch of one row of the drawing painted one colour.
#[derive(Debug)]
struct Run {
    row: usize,
    /// Its first column.
    from: usize,
    /// The column after its last.
    to: usize,
    colour: usize,
}

impl Run {
    /// The fewest steps, across, down or aslant, from a cell of this run to a
    /// cell of `other`.
    fn apart(&self, other: &Self) -> usize {
        let across = if self.to <= other.from {
            other.from + 1 - self.to
        } else if other.to <= self.from {
            self.from + 1 - other.to
        } else {
            0
        };
        self.row.abs_diff(other.row).max(across)
    }
}

/// The runs of colour `grid` has.
fn runs(grid: &[Vec<Cell>]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (row, cells) in grid.iter().enumerate() {
        for (column, cell) in cells.iter().enumerate() {
            let Some(colour) = cell.colour else {
                continue;
            };
            match runs.last_mut() {
                Some(run) if run.row == row && run.to == column && run.colour == colour => {
                    run.to += 1;
                }
                _ => runs.push(Run {
                    row,
                    from: column,
                    to: column + 1,
                    colour,
                }),
            }
        }
    }
    runs
}

/// `drawn` with its colours taken out, and its cells row by row; `None` when
/// it holds an escape other than setting a colour or clearing it.
fn read(drawn: &str) -> Option<(String, Vec<Vec<Cell>>)> {
    let mut plain = String::with_capacity(drawn.len());
    let mut grid: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut colour = None;
    let mut rest = drawn;
    while let Some(character) = rest.chars().next() {
        if character == '\x1b' {
            let (sequence, after) = rest.split_once('m')?;
            colour = match set(sequence)? {
                Escape::Set(colour) => Some(colour),
                Escape::Cleared => None,
            };
            rest = after;
            continue;
        }
        rest = rest.get(character.len_utf8()..)?;
        plain.push(character);
        if character == '\n' {
            grid.push(Vec::new());
            continue;
        }
        let width = columns(character.encode_utf8(&mut [0; 4]));
        let row = grid.last_mut()?;
        for column in 0..width {
            let glyph = if column == 0 { character } else { ' ' };
            row.push(Cell { glyph, colour });
        }
    }
    Some((plain, grid))
}

/// What an escape setting a colour or clearing it does.
enum Escape {
    /// It sets the colour with this red, green and blue, read as one number.
    Set(usize),
    /// It clears the colour.
    Cleared,
}

/// What `sequence`, an escape up to its closing `m`, does; `None` when it
/// neither sets a colour nor clears it.
fn set(sequence: &str) -> Option<Escape> {
    let body = sequence.strip_prefix("\x1b[")?;
    if body == "0" {
        return Some(Escape::Cleared);
    }
    let mut parts = body
        .strip_prefix("38;2;")?
        .split(';')
        .map(|part| part.parse::<u8>().ok());
    let (Some(Some(red)), Some(Some(green)), Some(Some(blue)), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    Some(Escape::Set(
        usize::from(red) << 16 | usize::from(green) << 8 | usize::from(blue),
    ))
}
