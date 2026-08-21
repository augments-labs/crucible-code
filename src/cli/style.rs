//! What the `output` block and the terminal together decided.
//!
//! Resolved once, at startup, into plain answers the drawing can read without
//! asking anything: whether to write colour and how much of it, which
//! characters to draw with, and how much of a line to show. None of those may
//! be asked per event — two are syscalls and the third is a file.

use crucible_config::{Color, Glyphs as Wanted, ThemeChoice, ToolDetail};
use crucible_tui::{Glyphs, Ground, Palette, Theme};

/// What the `output` block said, before the terminal and the environment have
/// their say.
///
/// One value rather than five parameters: they arrive together, out of one
/// block, and a call site passing five `None`s in a row says nothing about
/// which is which.
///
/// Not `Copy`, unlike everything else here: one of the answers is a theme name,
/// which is somebody else's string. It is built once at startup and moved into
/// [`Style::resolve`], so there is nothing for `Copy` to buy.
#[derive(Debug, Default, Clone)]
pub(crate) struct Output {
    /// Whether to write colour.
    pub(crate) color: Option<Color>,
    /// Which characters to draw with.
    pub(crate) glyphs: Option<Wanted>,
    /// How much of a tool call one line shows.
    pub(crate) detail: Option<ToolDetail>,
    /// Which table of colours to draw with.
    pub(crate) theme: Option<ThemeChoice>,
    /// Which theme fenced code is drawn in.
    pub(crate) syntax: Option<String>,
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

/// Whether a run writes colour at all.
///
/// Named rather than inlined because two callers need the same answer and one
/// of them needs it *before* a style exists: what decides whether the terminal
/// is asked about its background is whether the answer would ever be drawn.
///
/// Both overrides mean it: `always` is how a run whose output is being captured
/// on purpose — a recording, a pty in CI — asks for the colour it would have
/// had, and it would be no override at all if the terminal check still had a
/// veto.
pub(crate) fn writes_colour(
    wanted: Option<Color>,
    terminal: bool,
    from: &dyn Fn(&str) -> Option<String>,
) -> bool {
    match wanted.unwrap_or(Color::Auto) {
        Color::Always => true,
        Color::Never => false,
        Color::Auto => terminal && from(NO_COLOR).is_none_or(|set| set.is_empty()),
    }
}

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
    /// Which way the terminal said its ground goes, where it has said.
    ground: Option<Ground>,
}

impl Style {
    /// Settles every question, from the files, the terminal and the environment.
    ///
    /// `terminal` is whether output is going to a terminal rather than a pipe;
    /// `exact` is what the terminal answered when asked what its background is,
    /// and `seeded` is which way a variable it set at launch says that ground
    /// goes. Two parameters rather than one because they are not the same
    /// answer: which way is enough to pick a table, and only the channels are
    /// enough to blend a band off. Either may be absent, and both absent is a
    /// state everything downstream is correct in rather than a failure. `from` reads the environment, as a
    /// parameter because writing to the real one is `unsafe` in edition 2024
    /// and this workspace forbids it.
    pub(crate) fn resolve(
        output: Output,
        terminal: bool,
        exact: Option<(u8, u8, u8)>,
        seeded: Option<Ground>,
        from: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        // Taken apart first: one of the answers is a theme name, which is a
        // string this moves out rather than copies.
        let Output {
            color: wanted,
            glyphs,
            detail,
            theme: chosen,
            syntax,
        } = output;

        let color = match wanted.unwrap_or(Color::Auto) {
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
        // What was measured outranks what was merely set at launch: a variable
        // says what the terminal was configured with and the answer says what it
        // is now, and the two disagree the moment somebody changes their theme.
        let ground = exact
            .map(|colour| {
                if crucible_tui::is_light(colour) {
                    Ground::Light
                } else {
                    Ground::Dark
                }
            })
            .or(seeded);

        let theme = Self::theme(chosen, ground);

        // The six colours a fenced block is drawn in. Settled here with
        // everything else, because a palette is settled once — and it is
        // only the *themes* that are read now, about half a millisecond of
        // the startup budget. The syntax definitions, which are the larger
        // half, stay unread until a fence actually arrives.
        let code = color
            .then(|| {
                let named = syntax
                    .as_deref()
                    .unwrap_or(crucible_tui::syntax::THEME_UNLESS_SAID);

                crucible_tui::syntax::colours(named).or_else(|| {
                    crucible_tui::syntax::colours(crucible_tui::syntax::THEME_UNLESS_SAID)
                })
            })
            .flatten();

        Self {
            color,
            // Whether, then how much: the answer above is the veto, and the
            // ladder below it only decides how far up a terminal that is having
            // colour at all can go.
            palette: match code {
                Some(six) => Palette::resolve(color, theme, exact, from).reading(six),
                None => Palette::resolve(color, theme, exact, from),
            },
            ground,
            glyphs: match glyphs.unwrap_or(Wanted::Unicode) {
                Wanted::Unicode => Glyphs::Unicode,
                Wanted::Ascii => Glyphs::Ascii,
            },
            detail: detail.unwrap_or(ToolDetail::Compact),
        }
    }

    /// Which table a configured answer and a reported ground come to between
    /// them.
    ///
    /// `auto` with nothing reported settles on dark. Not because dark is more
    /// likely — it is the fallback that degrades best: the band is simply not
    /// painted, and every hue in that table still clears a light ground at the
    /// old three-to-one bar even though it is tuned for a dark one.
    pub(crate) fn theme(chosen: Option<ThemeChoice>, ground: Option<Ground>) -> Theme {
        match chosen {
            None | Some(ThemeChoice::Auto) => match ground {
                Some(Ground::Light) => Theme::Light,
                Some(Ground::Dark) | None => Theme::Dark,
            },
            Some(ThemeChoice::Dark) => Theme::Dark,
            Some(ThemeChoice::Light) => Theme::Light,
            Some(ThemeChoice::ColourblindDark) => Theme::ColourblindDark,
            Some(ThemeChoice::ColourblindLight) => Theme::ColourblindLight,
            Some(ThemeChoice::Ansi) => Theme::Ansi,
        }
    }

    /// Which way the terminal said its ground goes, where anything has said.
    ///
    /// Read by `/theme`, which has to answer `auto` a second time: the reader
    /// may pick it after startup, and what it means is still whatever the
    /// terminal reported then.
    pub(crate) fn ground(self) -> Option<Ground> {
        self.ground
    }

    /// The same style, reading code in a different syntax theme.
    pub(crate) fn reading(self, colours: [(u8, u8, u8); 6]) -> Self {
        Self {
            palette: self.palette.reading(colours),
            ..self
        }
    }

    /// The same style, drawn with a different table.
    ///
    /// The palette is settled again rather than patched, because a palette is
    /// more than its table: the band was blended off the reader's own ground
    /// and the ladder was settled from the terminal, and both have to be
    /// carried across rather than recomputed from a guess.
    pub(crate) fn wearing(self, theme: Theme) -> Self {
        Self {
            palette: self.palette.wearing(theme),
            ..self
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
        Self::resolve(Output::default(), true, None, None, &|_| None)
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
        Self::resolve(Output::default(), true, None, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    /// And what it uses when the band down the side of a prompt is.
    ///
    /// The mark in front of a prompt and the ground behind it are blended off
    /// the reader's own background rather than read off a theme, so a palette
    /// never told one paints them with nothing at all. That is the right answer
    /// on a terminal that announced no background, and no instrument at all for
    /// a test about the band.
    pub(crate) fn grounded(background: (u8, u8, u8)) -> Self {
        Self::resolve(Output::default(), true, Some(background), None, &|name| {
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
        Style::resolve(Output::default(), terminal, None, None, &environment(&[]))
    }

    /// A style that configured `theme` and nothing else, over a reported ground.
    fn themed(chosen: Option<ThemeChoice>, ground: Option<Ground>) -> Style {
        Style::resolve(
            Output {
                theme: chosen,
                ..Output::default()
            },
            true,
            None,
            ground,
            &environment(&[]),
        )
    }

    #[test]
    fn a_named_theme_wins_over_whatever_the_terminal_reported() {
        // The reader said which table. Following the ground anyway would be
        // this program overruling an answer it asked for.
        let light = Some(Ground::Light);

        assert_eq!(
            themed(Some(ThemeChoice::Dark), light).palette().theme(),
            Theme::Dark
        );
        assert_eq!(
            themed(Some(ThemeChoice::ColourblindLight), Some(Ground::Dark))
                .palette()
                .theme(),
            Theme::ColourblindLight
        );
    }

    #[test]
    fn auto_follows_the_ground_however_little_of_it_was_reported() {
        // Which way it goes is all this question needs, so the variable that
        // says only that is a whole answer here even though it is not one for
        // the band.
        // Both the way it can arrive: measured channels, and a variable that
        // says only which way the ground goes. Which way is a whole answer to
        // this question even though it is not one for the band.
        let auto = |exact, seeded| {
            Style::resolve(
                Output {
                    theme: Some(ThemeChoice::Auto),
                    ..Output::default()
                },
                true,
                exact,
                seeded,
                &environment(&[]),
            )
            .palette()
            .theme()
        };

        assert_eq!(auto(None, Some(Ground::Light)), Theme::Light);
        assert_eq!(auto(Some((255, 255, 255)), None), Theme::Light);
        assert_eq!(auto(None, Some(Ground::Dark)), Theme::Dark);
        assert_eq!(auto(Some((0, 0, 0)), None), Theme::Dark);
    }

    #[test]
    fn a_run_that_named_no_theme_follows_the_ground_as_auto_would() {
        // No layer said, so there is nothing to override -- and the answer to
        // "decide from the terminal" is the same answer either way.
        assert_eq!(
            themed(None, Some(Ground::Light)).palette().theme(),
            Theme::Light
        );
        assert_eq!(themed(None, None).palette().theme(), Theme::Dark);
    }

    #[test]
    fn a_band_is_blended_only_where_the_terminal_answered_with_channels() {
        // The distinction the two parameters exist for. A ground known only by
        // which way it goes cannot be blended off -- but it still picks the
        // table, and the table is what the band falls back to, so every one of
        // these paints. What the exact answer buys is a band off the reader's
        // own colour rather than off the one their table was drawn for, which
        // is a different sequence and is the assertion at the foot of this.
        let painted = |exact, seeded| {
            Style::resolve(
                Output {
                    theme: Some(ThemeChoice::Dark),
                    ..Output::default()
                },
                true,
                exact,
                seeded,
                &environment(&[("COLORTERM", "truecolor")]),
            )
            .palette()
            .open(crucible_tui::Slot::Prompt)
            .as_str()
            .to_owned()
        };

        assert!(!painted(None, None).is_empty());
        assert!(!painted(None, Some(Ground::Dark)).is_empty());
        assert!(!painted(None, Some(Ground::Light)).is_empty());
        assert!(!painted(Some((13, 13, 16)), None).is_empty());
        assert!(!painted(Some((255, 255, 255)), None).is_empty());

        assert_ne!(painted(Some((13, 13, 16)), None), painted(None, None));
        assert_ne!(painted(Some((255, 255, 255)), None), painted(None, None));
    }

    #[test]
    fn what_the_terminal_answered_outranks_what_a_variable_said_at_launch() {
        // They disagree the moment somebody changes their terminal theme
        // without restarting their shell: the variable is what it was
        // configured with, and the answer is what it is now.
        let style = |exact, seeded| {
            Style::resolve(
                Output {
                    theme: Some(ThemeChoice::Auto),
                    ..Output::default()
                },
                true,
                exact,
                seeded,
                &environment(&[]),
            )
            .palette()
            .theme()
        };

        assert_eq!(
            style(Some((255, 255, 255)), Some(Ground::Dark)),
            Theme::Light
        );
        assert_eq!(style(Some((0, 0, 0)), Some(Ground::Light)), Theme::Dark);
        // And the variable is what is left when nothing answered.
        assert_eq!(style(None, Some(Ground::Light)), Theme::Light);
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
            Some((13, 13, 16)),
            None,
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
            !Style::resolve(Output::default(), true, None, None, &shouting)
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
                None,
                &environment(&[]),
            );

            assert_eq!(style.args(20), 20, "{detail:?}");
            assert_eq!(style.output(20), 20, "{detail:?}");
        }
    }
}
