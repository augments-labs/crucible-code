//! The width rule held over every drawer in this crate, at every width and
//! room the components of `crucible-tui` are held to.
//!
//! A drawer named here is swept; one that is not, the repository gate finds:
//! every `impl Draws for` in this crate's sources must name its type in this
//! file. The fixture is hostile on purpose: a formula as wide and as tall as
//! one gets inside the caps, with wide characters, a matrix and nested
//! fractions, so most rooms refuse it and the widest draw it.

use crucible_tui::drawing::Draws;
use crucible_tui::testing::{fits, rooms};

use crate::Formulas;

/// A display formula as hostile as its kind gets inside the caps.
const HOSTILE: &str = r"\sum_{i=1}^{n} \frac{\frac{a_i^2 + \text{漢字漢字漢字}}{\sqrt[3]{b_i - c_i}}}{\prod_{j} x_j} = \begin{pmatrix} \alpha & \beta & \gamma \\ \frac{1}{2} & \sqrt{\pi} & \infty \\ a_{11} & a_{12} & a_{13} \end{pmatrix} \cdot \begin{cases} x & \text{if } x \geq 0 \\ -x & \text{otherwise, a long reason that runs past any width} \end{cases}";

#[test]
fn a_display_formula_fits_every_room() {
    fits("a display formula", &Formulas, HOSTILE);
}

#[test]
fn an_inline_formula_is_one_line_in_every_room() {
    for room in rooms() {
        let runs = Formulas.inline(HOSTILE, &room).unwrap_or_default();
        assert!(
            runs.iter().all(|run| !run.text.contains('\n')),
            "an inline formula broke a line in {room:?}"
        );
    }
}
