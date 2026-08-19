//! What the `output` block and the terminal together decided.
//!
//! Resolved once, at startup, into plain answers the drawing can read without
//! asking anything: whether to write colour and how much of it, which
//! characters to draw with, and how much of a line to show. None of those may
//! be asked per event — two are syscalls and the third is a file.

use crucible_config::{Color, Glyphs as Wanted, Mouse, ThemeChoice, ToolDetail};
use crucible_tui::{Glyphs, Ground, Palette, Theme};

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
    /// Which table of colours to draw with.
    pub(crate) theme: Option<ThemeChoice>,
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
    /// Which table of colours is in force. Never `auto`: that is a question
    /// about the terminal, and it is answered here.
    ///
    /// The lint is right that nothing reads it yet and wrong that nothing will:
    /// `/theme` is what marks the row already in force, and it cannot do that
    /// from the palette, which keeps its table private on purpose.
    // Read by the tests and, shortly, by `/theme`; not yet by anything the
    // binary does at runtime. `allow` rather than `expect` because the lint
    // fires in one build and not the other, and an expectation that holds in
    // only one of them is itself an error.
    #[allow(dead_code, reason = "`/theme` marks the row already in force")]
    theme: Theme,
    /// Which way the terminal said its ground goes, where it has said.
    #[allow(dead_code, reason = "`/theme` reads it to draw the specimen")]
    ground: Option<Ground>,
    /// Whether a click means anything to crucible. Off leaves the mouse to the
    /// terminal, which is where the transcript above the box lives.
    clicks: bool,
}

impl Style {
    /// Settles every question, from the files, the terminal and the environment.
    ///
    /// `terminal` is whether output is going to a terminal rather than a pipe;
    /// `ground` is which way the terminal said its background goes, where
    /// anything has said — a state everything downstream is correct in rather
    /// than a failure.
    ///
    /// Which way, and not the channels. The variable that answers this carries
    /// no colour, and nothing in this release asks the terminal for one: see
    /// the note on [`Palette::resolve`]'s ground about what that leaves unpainted. `from` reads the environment, as a
    /// parameter because writing to the real one is `unsafe` in edition 2024
    /// and this workspace forbids it.
    pub(crate) fn resolve(
        output: Output,
        terminal: bool,
        ground: Option<Ground>,
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

        // The one place `auto` is answered, because this is the one place that
        // holds both what was configured and what the terminal said. Past here
        // it does not exist.
        let theme = Self::theme(output.theme, ground);

        Self {
            color,
            // Whether, then how much: the answer above is the veto, and the
            // ladder below it only decides how far up a terminal that is having
            // colour at all can go.
            // No channels to blend a band off: the only thing that reports
            // them is a query this release does not make, so the prompt row
            // keeps its mark and its blank row and takes no ground.
            palette: Palette::resolve(color, theme, None, from),
            theme,
            ground,
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

    /// Which table a configured answer and a reported ground come to between
    /// them.
    ///
    /// `auto` with nothing reported settles on dark. Not because dark is more
    /// likely — it is the fallback that degrades best: the band is simply not
    /// painted, and every hue in that table still clears a light ground at the
    /// old three-to-one bar even though it is tuned for a dark one.
    fn theme(chosen: Option<ThemeChoice>, ground: Option<Ground>) -> Theme {
        let following = |ground| match ground {
            Some(Ground::Light) => Theme::Light,
            Some(Ground::Dark) | None => Theme::Dark,
        };

        match chosen {
            None | Some(ThemeChoice::Auto) => following(ground),
            Some(ThemeChoice::Dark) => Theme::Dark,
            Some(ThemeChoice::Light) => Theme::Light,
            Some(ThemeChoice::ColourblindDark) => Theme::ColourblindDark,
            Some(ThemeChoice::ColourblindLight) => Theme::ColourblindLight,
            Some(ThemeChoice::Ansi) => Theme::Ansi,
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

    /// Whether a click places the cursor in the box and expands a cut result up
    /// in the transcript, at the price of the terminal's own use of the mouse.
    pub(crate) fn clicks(self) -> bool {
        self.clicks
    }

    /// Which characters a component draws with.
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
        Self::resolve(Output::default(), true, None, &|_| None)
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
        Self::resolve(Output::default(), true, None, &|name| {
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
        Style::resolve(Output::default(), terminal, None, &environment(&[]))
    }

    /// A style that configured `theme` and nothing else, over a reported ground.
    fn themed(chosen: Option<ThemeChoice>, ground: Option<Ground>) -> Style {
        Style::resolve(
            Output {
                theme: chosen,
                ..Output::default()
            },
            true,
            ground,
            &environment(&[]),
        )
    }

    #[test]
    fn a_named_theme_wins_over_whatever_the_terminal_reported() {
        // The reader said which table. Following the ground anyway would be
        // this program overruling an answer it asked for.
        let light = Some(Ground::Light);

        assert_eq!(themed(Some(ThemeChoice::Dark), light).theme, Theme::Dark);
        assert_eq!(
            themed(Some(ThemeChoice::ColourblindLight), Some(Ground::Dark)).theme,
            Theme::ColourblindLight
        );
    }

    #[test]
    fn auto_follows_the_ground_however_little_of_it_was_reported() {
        // Which way it goes is all this question needs, so the variable that
        // says only that is a whole answer here even though it is not one for
        // the band.
        for reported in [Some(Ground::Light), Some(Ground::Light)] {
            assert_eq!(
                themed(Some(ThemeChoice::Auto), reported).theme,
                Theme::Light,
                "{reported:?}"
            );
        }
        for reported in [Some(Ground::Dark), Some(Ground::Dark)] {
            assert_eq!(
                themed(Some(ThemeChoice::Auto), reported).theme,
                Theme::Dark,
                "{reported:?}"
            );
        }
    }

    #[test]
    fn a_run_that_named_no_theme_follows_the_ground_as_auto_would() {
        // No layer said, so there is nothing to override -- and the answer to
        // "decide from the terminal" is the same answer either way.
        assert_eq!(themed(None, Some(Ground::Light)).theme, Theme::Light);
        assert_eq!(themed(None, None).theme, Theme::Dark);
    }

    #[test]
    fn the_prompt_row_takes_no_ground_until_something_reports_one() {
        // The band is built and checked in `crucible-tui`, and it is blended
        // off the exact channels of the reader's own background. Nothing in
        // this release reports those: the variable read at startup says which
        // way the ground goes and not what colour it is, and the query that
        // would say is not made.
        //
        // So the prompt row keeps its mark and the blank row above it and takes
        // no ground at all -- the fallback the design calls correct rather than
        // merely safe. This test is the record of that, and the day a source of
        // channels lands it is the test that says so.
        for ground in [None, Some(Ground::Dark), Some(Ground::Light)] {
            let style = Style::resolve(
                Output {
                    theme: Some(ThemeChoice::Dark),
                    ..Output::default()
                },
                true,
                ground,
                &environment(&[("COLORTERM", "truecolor")]),
            );

            assert!(
                style
                    .palette()
                    .open(crucible_tui::Slot::Prompt)
                    .as_str()
                    .is_empty(),
                "{ground:?}"
            );
            // And the rest of the theme is unaffected: a band nobody can paint
            // is not a palette nobody can use.
            assert!(
                !style
                    .palette()
                    .open(crucible_tui::Slot::Accent)
                    .as_str()
                    .is_empty()
            );
        }
    }

    #[test]
    fn a_run_with_no_colour_writes_nothing_whatever_theme_was_named() {
        // The veto is above the ladder and above the theme alike.
        let style = Style::resolve(
            Output {
                color: Some(Color::Never),
                theme: Some(ThemeChoice::Dark),
                ..Output::default()
            },
            true,
            Some(Ground::Dark),
            &environment(&[]),
        );

        assert!(
            style
                .palette()
                .open(crucible_tui::Slot::Prompt)
                .as_str()
                .is_empty()
        );
        assert!(!style.palette().writes_color());
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
        let style = Style::resolve(
            Output::default(),
            true,
            None,
            &environment(&[(NO_COLOR, "1")]),
        );

        assert!(!style.color());
    }

    #[test]
    fn no_color_set_to_nothing_is_not_set() {
        // The convention is that the variable's presence is the signal, but an
        // empty value is what a shell leaves behind when somebody unsets it the
        // wrong way, and reading that as "no colour" surprises them.
        let style = Style::resolve(
            Output::default(),
            true,
            None,
            &environment(&[(NO_COLOR, "")]),
        );

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
                None,
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
                None,
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
            None,
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
            !Style::resolve(Output::default(), true, None, &shouting)
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
            None,
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
            None,
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
                None,
                &environment(&[]),
            );

            assert_eq!(style.args(20), 20, "{detail:?}");
            assert_eq!(style.output(20), 20, "{detail:?}");
        }
    }
}
