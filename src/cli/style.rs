//! What the `output` block and the terminal together decided.
//!
//! Resolved once, at startup, into plain answers the drawing can read without
//! asking anything: whether to write colour and how much of it, which
//! characters to draw with, and how much of a line to show. None of those may
//! be asked per event — two are syscalls and the third is a file.

use crucible_config::{Color, Glyphs as Wanted, Mouse, ToolDetail};
use crucible_tui::{Glyphs, Palette};

/// What the `output` block said, before the terminal and the environment have
/// their say.
///
/// One value rather than four parameters: they arrive together, out of one
/// block, and a call site passing four `None`s in a row says nothing about
/// which is which.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Output {
    /// Whether to write colour.
    pub(crate) color: Option<Color>,
    /// Which characters to draw with.
    pub(crate) glyphs: Option<Wanted>,
    /// How much of a tool call one line shows.
    pub(crate) detail: Option<ToolDetail>,
    /// Who the mouse belongs to while a prompt is up.
    pub(crate) mouse: Option<Mouse>,
}

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
    /// What a slot is worth on this terminal, once colour is on at all.
    palette: Palette,
    /// Which characters crucible's own interface is drawn with.
    glyphs: Glyphs,
    /// How much of a tool call and its result one line shows.
    detail: ToolDetail,
    /// Whether a click in the box places the cursor. Off leaves the mouse to
    /// the terminal, which is where the transcript above the box lives.
    clicks: bool,
}

impl Style {
    /// Settles every question, from the files, the terminal and the environment.
    ///
    /// `terminal` is whether output is going to a terminal rather than a pipe;
    /// `from` reads the environment, as a parameter because writing to the real
    /// one is `unsafe` in edition 2024 and this workspace forbids it.
    pub(crate) fn resolve(
        output: Output,
        terminal: bool,
        from: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let color = match output.color.unwrap_or(Color::Auto) {
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
            // Whether, then how much: the answer above is the veto, and the
            // ladder below it only decides how far up a terminal that is having
            // colour at all can go.
            palette: Palette::resolve(color, from),
            glyphs: match output.glyphs.unwrap_or(Wanted::Unicode) {
                Wanted::Unicode => Glyphs::Unicode,
                Wanted::Ascii => Glyphs::Ascii,
            },
            detail: output.detail.unwrap_or(ToolDetail::Compact),

            // Off unless a layer asked. The wheel is a button, so a terminal
            // forwarding buttons to crucible is one whose wheel no longer
            // scrolls the scrollback this program's transcript lives in.
            clicks: output.mouse.is_some_and(Mouse::places),
        }
    }

    /// Whether the prompt mark is dimmed.
    pub(crate) fn color(self) -> bool {
        self.color
    }

    /// What a component's slots are worth here.
    pub(crate) fn palette(self) -> Palette {
        self.palette
    }

    /// Which characters a component draws with.
    /// Whether a click in the prompt box places the cursor, at the price of the
    /// terminal's own use of the mouse.
    pub(crate) fn clicks(self) -> bool {
        self.clicks
    }

    pub(crate) fn glyphs(self) -> Glyphs {
        self.glyphs
    }

    /// How much of a tool's arguments to show, in a terminal this wide.
    ///
    /// For the line that says a call is about to run, and not for the question
    /// that asks whether it may. That one is wrapped to the window instead: a
    /// report is read at a glance and can afford a ceiling, and a decision
    /// cannot — a command padded past the cut would be consented to by its
    /// leading columns and do whatever the rest of it does.
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
        Self::resolve(Output::default(), true, &|_| None)
    }

    /// And what it uses when the glyph set *is* the thing being tested: the
    /// same answers to everything else, so a test that loops over both sets
    /// changes one thing between its two runs.
    pub(crate) fn drawn(glyphs: Glyphs) -> Self {
        Self {
            glyphs,
            ..Self::plain()
        }
    }

    /// And what it uses when the colour is. [`Style::plain`] settles on a
    /// terminal that announced nothing, which is a palette writing no bytes at
    /// all — the right instrument for a test reading what a row says, and no
    /// instrument at all for one reading what a row is painted as.
    pub(crate) fn coloured() -> Self {
        Self::resolve(Output::default(), true, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
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
        Style::resolve(Output::default(), terminal, &environment(&[]))
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
        let style = Style::resolve(Output::default(), true, &environment(&[(NO_COLOR, "1")]));

        assert!(!style.color());
    }

    #[test]
    fn no_color_set_to_nothing_is_not_set() {
        // The convention is that the variable's presence is the signal, but an
        // empty value is what a shell leaves behind when somebody unsets it the
        // wrong way, and reading that as "no colour" surprises them.
        let style = Style::resolve(Output::default(), true, &environment(&[(NO_COLOR, "")]));

        assert!(style.color());
    }

    #[test]
    fn always_and_never_override_both_the_terminal_and_the_variable() {
        let shouting = environment(&[(NO_COLOR, "1")]);

        // `always` on a pipe, with NO_COLOR set: the file said colour, and both
        // of the things it overrides are saying no.
        assert!(
            Style::resolve(
                Output {
                    color: Some(Color::Always),
                    ..Output::default()
                },
                false,
                &shouting
            )
            .color()
        );

        // `never` on a terminal with nothing else objecting.
        assert!(
            !Style::resolve(
                Output {
                    color: Some(Color::Never),
                    ..Output::default()
                },
                true,
                &environment(&[])
            )
            .color()
        );
    }

    #[test]
    fn a_run_that_named_no_font_is_drawn_with_the_characters_the_design_uses() {
        // Unicode is the answer rather than a platform guess: a font missing a
        // box-drawing glyph is invisible to this process, so ascii is something
        // a reader who sees hollow squares asks for, and never something that is
        // inferred from what operating system they are on.
        assert_eq!(plain(true).glyphs(), Glyphs::Unicode);
        assert_eq!(plain(false).glyphs(), Glyphs::Unicode);

        let asked = Style::resolve(
            Output {
                glyphs: Some(Wanted::Ascii),
                ..Output::default()
            },
            true,
            &environment(&[]),
        );
        assert_eq!(asked.glyphs(), Glyphs::Ascii);
    }

    #[test]
    fn the_palette_is_off_wherever_the_answer_about_colour_was_no() {
        // One veto, not two. Whether colour is written at all is settled above,
        // and the ladder only decides how far a terminal already having it goes
        // -- so a run with colour off cannot have a palette that writes any.
        assert!(!plain(false).palette().writes_color());

        let shouting = environment(&[(NO_COLOR, "1"), ("COLORTERM", "truecolor")]);
        assert!(
            !Style::resolve(Output::default(), true, &shouting)
                .palette()
                .writes_color()
        );

        // And a run that overrode its way back on gets the depth its terminal
        // announced, even though nothing here is a terminal.
        let captured = environment(&[("COLORTERM", "truecolor")]);
        let style = Style::resolve(
            Output {
                color: Some(Color::Always),
                ..Output::default()
            },
            false,
            &captured,
        );
        assert!(style.palette().writes_color());
    }

    #[test]
    fn compact_is_the_answer_when_no_layer_chose_one() {
        let style = plain(true);

        assert_eq!(style.args(200), ARGS);
        assert_eq!(style.output(200), OUTPUT);
    }

    #[test]
    fn full_shows_as_much_as_the_terminal_is_wide() {
        let style = Style::resolve(
            Output {
                detail: Some(ToolDetail::Full),
                ..Output::default()
            },
            true,
            &environment(&[]),
        );

        assert_eq!(style.args(200), 200);
        assert_eq!(style.output(200), 200);
    }

    #[test]
    fn no_line_is_ever_allowed_to_be_wider_than_the_window() {
        // A committed line that wraps is two rows, and the live tail counts the
        // rows it drew so it can move back over them. Compact is a ceiling on a
        // wide terminal and the window is the ceiling on a narrow one.
        for detail in [ToolDetail::Compact, ToolDetail::Full] {
            let style = Style::resolve(
                Output {
                    detail: Some(detail),
                    ..Output::default()
                },
                true,
                &environment(&[]),
            );

            assert_eq!(style.args(20), 20, "{detail:?}");
            assert_eq!(style.output(20), 20, "{detail:?}");
        }
    }
}
