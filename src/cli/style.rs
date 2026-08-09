//! What the `output` block and the terminal together decided.
//!
//! Resolved once, at startup, into two plain answers the drawing can read
//! without asking anything: whether to write colour, and how much of a line to
//! show. Neither question may be asked per event — one is a syscall and the
//! other is a file.

use crucible_config::{Color, ToolDetail};

/// How much of a tool's arguments a compact line shows.
const ARGS: usize = 56;

/// How much of a tool's output a compact line shows.
const OUTPUT: usize = 96;

/// The variable every command-line tool is expected to honour.
///
/// <https://no-color.org>: set to anything non-empty, colour is off. It is a
/// convention rather than a standard, but it is the one users already have in
/// their shell profile, and a program that ignores it is the reason they had to
/// set it in the first place.
const NO_COLOR: &str = "NO_COLOR";

/// What the terminal is written with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    /// Whether an escape sequence is written at all.
    color: bool,
    /// How much of a tool call and its result one line shows.
    detail: ToolDetail,
}

impl Style {
    /// Settles both questions, from the files, the terminal and the environment.
    ///
    /// `terminal` is whether output is going to a terminal rather than a pipe;
    /// `from` reads the environment, as a parameter because writing to the real
    /// one is `unsafe` in edition 2024 and this workspace forbids it.
    pub(crate) fn resolve(
        color: Option<Color>,
        detail: Option<ToolDetail>,
        terminal: bool,
        from: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let color = match color.unwrap_or(Color::Auto) {
            // Both overrides mean it: `always` is how a run whose output is
            // being captured on purpose — a recording, a pty in CI — asks for
            // the colour it would have had, and it would be no override at all
            // if the terminal check still had a veto.
            Color::Always => true,
            Color::Never => false,
            Color::Auto => terminal && from(NO_COLOR).is_none_or(|set| set.is_empty()),
        };

        Self {
            color,
            detail: detail.unwrap_or(ToolDetail::Compact),
        }
    }

    /// Whether the prompt mark is dimmed.
    pub(crate) fn color(self) -> bool {
        self.color
    }

    /// How much of a tool's arguments to show, in a terminal this wide.
    pub(crate) fn args(self, columns: usize) -> usize {
        self.width(ARGS, columns)
    }

    /// How much of a tool's output, or of an error, to show.
    pub(crate) fn output(self, columns: usize) -> usize {
        self.width(OUTPUT, columns)
    }

    /// One width, according to the detail asked for.
    ///
    /// `full` is the terminal's width rather than no limit at all, so one event
    /// stays about one row either way. The renderer wraps a longer line
    /// correctly — it measures what it commits — so this is about how much of
    /// the screen a tool call is allowed to take, not about drawing it right.
    fn width(self, compact: usize, columns: usize) -> usize {
        match self.detail {
            ToolDetail::Compact => compact.min(columns),
            ToolDetail::Full => columns,
        }
    }
}

#[cfg(test)]
impl Style {
    /// What a test uses when the drawing is not the thing being tested: a
    /// terminal, nothing configured, nothing in the environment.
    pub(crate) fn plain() -> Self {
        Self::resolve(None, None, true, &|_| None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment holding exactly these variables.
    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let held: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();

        move |wanted| {
            held.iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
        }
    }

    /// The style a run with nothing configured and nothing in the environment
    /// gets, on a terminal or not.
    fn plain(terminal: bool) -> Style {
        Style::resolve(None, None, terminal, &environment(&[]))
    }

    #[test]
    fn a_run_that_configured_nothing_writes_colour_only_to_a_terminal() {
        assert!(plain(true).color());

        // Redirected. The escape bytes would end up in whatever kept the
        // output rather than colouring anything.
        assert!(!plain(false).color());
    }

    #[test]
    fn no_color_turns_it_off_on_a_terminal_that_would_otherwise_have_it() {
        let style = Style::resolve(None, None, true, &environment(&[(NO_COLOR, "1")]));

        assert!(!style.color());
    }

    #[test]
    fn no_color_set_to_nothing_is_not_set() {
        // The convention is that the variable's presence is the signal, but an
        // empty value is what a shell leaves behind when somebody unsets it the
        // wrong way, and reading that as "no colour" surprises them.
        let style = Style::resolve(None, None, true, &environment(&[(NO_COLOR, "")]));

        assert!(style.color());
    }

    #[test]
    fn always_and_never_override_both_the_terminal_and_the_variable() {
        let shouting = environment(&[(NO_COLOR, "1")]);

        // `always` on a pipe, with NO_COLOR set: the file said colour, and both
        // of the things it overrides are saying no.
        assert!(Style::resolve(Some(Color::Always), None, false, &shouting).color());

        // `never` on a terminal with nothing else objecting.
        assert!(!Style::resolve(Some(Color::Never), None, true, &environment(&[])).color());
    }

    #[test]
    fn compact_is_the_answer_when_no_layer_chose_one() {
        let style = plain(true);

        assert_eq!(style.args(200), ARGS);
        assert_eq!(style.output(200), OUTPUT);
    }

    #[test]
    fn full_shows_as_much_as_the_terminal_is_wide() {
        let style = Style::resolve(None, Some(ToolDetail::Full), true, &environment(&[]));

        assert_eq!(style.args(200), 200);
        assert_eq!(style.output(200), 200);
    }

    #[test]
    fn no_line_is_ever_allowed_to_be_wider_than_the_window() {
        // A committed line that wraps is two rows, and the live tail counts the
        // rows it drew so it can move back over them. Compact is a ceiling on a
        // wide terminal and the window is the ceiling on a narrow one.
        for detail in [ToolDetail::Compact, ToolDetail::Full] {
            let style = Style::resolve(None, Some(detail), true, &environment(&[]));

            assert_eq!(style.args(20), 20, "{detail:?}");
            assert_eq!(style.output(20), 20, "{detail:?}");
        }
    }
}
