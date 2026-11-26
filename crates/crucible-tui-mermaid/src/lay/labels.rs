//! A flowchart's or a state diagram's drawing read back in colour, so each
//! line's label is held to the line it labels.
//!
//! Invariants:
//!
//! - The engine places a line's label beside a stretch of that line without
//!   looking at the lines around it, so a label can land against another
//!   line, nearer it than its own, or be left out where it finds no room. So
//!   the one layout is drawn with each line, and each line's label, in a
//!   colour of its own, and a label left out, or nearer another line than
//!   its own, refuses the diagram whole. A label as near its own line as any
//!   other is its own: where lines run side by side, one beside them is
//!   beside both.
//! - The drawing is the drawing in colour with its colours taken out, which
//!   is the drawing the engine makes without colour. Any colour this did not
//!   give, and any escape other than setting a colour or clearing it,
//!   refuses the diagram.

use std::fmt::Write as _;

use crucible_tui::columns;

use super::Unlaid;

/// `source` with each of the engine's `lines`, and its label, styled a
/// colour of its own: line `n`, counted from zero, drawn in `2n + 1` and
/// labelled in `2n + 2`.
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
    painted.push('\n');
    painted
}

/// `drawn`, the engine's drawing of [`painted`] in colour, with its colours
/// taken out, if each line `labelled` marks has its label drawn nearer its
/// own line than any other.
pub(super) fn held(drawn: &str, labelled: &[bool]) -> Result<String, Unlaid> {
    let (plain, runs) = read(drawn).ok_or(Unlaid::Unread)?;
    if runs
        .iter()
        .any(|run| run.colour == 0 || run.colour > 2 * labelled.len())
    {
        return Err(Unlaid::Unread);
    }
    let lines = labelled
        .iter()
        .enumerate()
        .filter(|(_, labelled)| **labelled);
    for (line, _) in lines {
        let label: Vec<&Run> = runs
            .iter()
            .filter(|run| run.colour == 2 * line + 2)
            .collect();
        let nearest = |own: bool| {
            runs.iter()
                .filter(|run| run.colour % 2 == 1 && (run.colour == 2 * line + 1) == own)
                .flat_map(|stroke| label.iter().map(|label| label.apart(stroke)))
                .min()
        };
        match (nearest(true), nearest(false)) {
            (Some(own), other) if other.is_none_or(|other| own <= other) => {}
            _ => return Err(Unlaid::Misplaced),
        }
    }
    Ok(plain)
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

/// `drawn` with its colours taken out, and the runs of colour it had; `None`
/// when it holds an escape other than setting a colour or clearing it.
fn read(drawn: &str) -> Option<(String, Vec<Run>)> {
    let mut plain = String::with_capacity(drawn.len());
    let mut runs: Vec<Run> = Vec::new();
    let (mut row, mut column, mut colour) = (0, 0, None);
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
            (row, column) = (row + 1, 0);
            continue;
        }
        let width = columns(character.encode_utf8(&mut [0; 4]));
        if let Some(colour) = colour.filter(|_| width > 0) {
            match runs.last_mut() {
                Some(run) if run.row == row && run.to == column && run.colour == colour => {
                    run.to += width;
                }
                _ => runs.push(Run {
                    row,
                    from: column,
                    to: column + width,
                    colour,
                }),
            }
        }
        column += width;
    }
    Some((plain, runs))
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
