//! A display formula whose picture is made off the drawing thread.
//!
//! A diagram's drawer draws its picture as the block closes, but a formula's
//! is made by an engine the drawing thread must not wait on. So a live record
//! that would show a formula as a picture, and has none kept of it, tells the
//! formula out as it closes and shows its text meanwhile; whoever makes the
//! picture hands it back by the line it was told out at, and it is shown as
//! any picture is, under the same caps. A picture handed back for a block no
//! longer held, or no longer shown as that formula's text, changes nothing.

use super::{By, Drawing, Line, Record};
use crate::chrome::Undrawn;
use crate::drawing::{Room, Svg};
use crate::markdown::{self, Kind};
use crate::row::Row;
use crate::terminal::graphics::Formula;

use super::Drawn;

impl Record {
    /// The formula `drawn` holds, where it is a display formula a picture
    /// would be shown in place of and none can be until one is made: a
    /// live session wants it as a picture here and has none kept of it,
    /// and its drawer converts it at `room`, laid out whole or too wide to
    /// be, so what it would replace is the formula drawn.
    pub(super) fn waits_for_picture(&self, drawn: &Drawn, room: Room) -> Option<Box<str>> {
        let waits = drawn.block.kind == Kind::DisplayMath
            && self.picturing.drawing == Drawing::Live
            && self.picturing.pictures.is_some()
            && drawn.picture.is_none()
            && drawn
                .refused
                .as_ref()
                .is_some_and(|refused| refused.1 == Undrawn::NotHere);
        if !waits {
            return None;
        }
        let (drawer, source) = markdown::taken(&drawn.block, &self.drawers)?;
        match drawer.block(source, &room) {
            Ok(_) | Err(Undrawn::TooWide) => Some(source.into()),
            Err(_) => None,
        }
    }

    /// The display formulas told out since last asked, oldest first: each
    /// is shown as its text until [`Self::formula_made`] is handed its
    /// picture.
    pub(crate) fn formulas(&mut self) -> Vec<Formula> {
        std::mem::take(&mut self.picturing.told)
    }

    /// Shows the block `formula` names as `svg`, where it is still held and
    /// is shown as no picture, as any picture is shown and under the same
    /// caps; answers whether that changed what it is laid out as.
    ///
    /// A block gone from the transcript is left gone, and the picture with
    /// it. One already written to a terminal's own scrollback stays there as
    /// its text, and the picture is kept with it for the next rewrite of
    /// what is kept.
    pub(crate) fn formula_made(&mut self, formula: &Formula, svg: Svg) -> bool {
        let Some(at) = formula.line.checked_sub(self.gone) else {
            return false;
        };
        let names = |line: &Line| match line {
            Line::Drawn(drawn) => {
                drawn.block.kind == Kind::DisplayMath
                    && drawn.picture.is_none()
                    && !drawn.fallen
                    && markdown::taken(&drawn.block, &self.drawers)
                        .is_some_and(|(_, source)| *source == *formula.source)
            }
            Line::Flowed(_) | Line::Set(_) | Line::Responsive { .. } => false,
        };
        if !self.lines.get(at).is_some_and(names) {
            return false;
        }
        // Out of the lines while its picture is settled, since keeping one
        // may draw an older picture as text.
        let Some(Line::Drawn(mut drawn)) = self
            .lines
            .get_mut(at)
            .map(|line| std::mem::replace(line, Line::Set(Row::new())))
        else {
            return false;
        };
        drawn.refused = None;
        self.picturing.arrived = Some(svg);
        let glyphs = drawn.laid.1;
        let columns = self.columns;
        let changed = self.settle(&mut drawn, columns, glyphs);
        self.picturing.arrived = None;
        if changed {
            let by = By {
                links: &self.links,
                drawers: &self.drawers,
                modes: self.modes,
                offers: self.offers,
                window: self.picturing.window,
            };
            let was = drawn.weight;
            self.picturing.saw(drawn.lay(columns, glyphs, by));
            self.weight = self.weight.saturating_sub(was).saturating_add(drawn.weight);
        }
        if let Some(line) = self.lines.get_mut(at) {
            *line = Line::Drawn(drawn);
        }
        if changed {
            self.remeasure_all();
            self.spill();
        }
        changed
    }
}
