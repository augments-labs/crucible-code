//! `/clear`: forgetting what has been said.

use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;

use super::Terms;

/// Runs it.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    Ok(renderer.present(&forgotten(runner.forget(), columns), terms.style.palette())?)
}

/// What `/clear` says, having forgotten `held` messages.
///
/// The second row is the one worth drawing every time. What is above the box is
/// the terminal's scrollback and stays exactly where it is — this program never
/// took that screen and is not about to hand it back empty — so the difference
/// between a session that forgot and one that did not is invisible without a
/// line saying which happened.
fn forgotten(held: usize, columns: usize) -> Vec<Row> {
    if held == 0 {
        return vec![Row::new().then(Slot::Quiet, clip("nothing had been said", columns))];
    }

    let said = match held {
        1 => "forgotten: 1 message".to_owned(),
        _ => format!("forgotten: {held} messages"),
    };

    vec![
        Row::new().then(Slot::Plain, clip(&said, columns)),
        Row::new().then(
            Slot::Quiet,
            clip("what is on screen stays where it is", columns),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::forgotten;

    /// What a list of rows says, row by row.
    fn art(rows: &[crucible_tui::Row]) -> Vec<String> {
        rows.iter().map(crucible_tui::Row::text).collect()
    }

    #[test]
    fn what_was_forgotten_is_counted_and_the_screen_is_said_to_be_untouched() {
        assert_eq!(
            art(&forgotten(12, 60)),
            [
                "forgotten: 12 messages",
                "what is on screen stays where it is",
            ]
        );

        // One of them is one message. A count is read as a count, and
        // `1 messages` reads as a program that did not expect the number it
        // printed.
        assert_eq!(
            art(&forgotten(1, 60)).first().map(String::as_str),
            Some("forgotten: 1 message")
        );
    }

    #[test]
    fn forgetting_a_session_with_nothing_in_it_says_that_instead() {
        // Rather than `forgotten: 0 messages`, which counts something that
        // never happened, and rather than the row about the screen, which
        // answers a question nobody asked.
        assert_eq!(art(&forgotten(0, 60)), ["nothing had been said"]);
    }

    #[test]
    fn nothing_it_says_is_wider_than_the_window_it_was_said_in() {
        // A row over the width would wrap, and a wrapped row leaves the cursor
        // a row below where the next frame expects it.
        for columns in 1..=60 {
            for row in forgotten(12, columns) {
                assert!(row.columns() <= columns, "at {columns}: {row:?}");
            }
        }
    }
}
