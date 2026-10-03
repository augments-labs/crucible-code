//! What the layers together say the terminal shows.
//!
//! Both answers here are strings in the document and values in the program, so
//! the reading of each sits beside the type it produces. Nothing in this module
//! decides anything about a terminal — that is `Style`'s job, one crate up,
//! from these answers and what the terminal itself reports.

use serde_json::Value;

use super::Settings;

/// What `output.pinAfterSeconds` is where no layer sets it.
///
/// Three, because a command that is over at once is over in under a second and
/// one chained or reaching a server is often over in two: a row put up for each
/// of those and taken down again is the screen blinking on every call, and a
/// command that is still going after three is one somebody may want to watch
/// or put down.
const PIN_AFTER_SECONDS: u64 = 3;

impl Settings {
    /// Whether to write colour, when the command line does not say.
    #[must_use]
    pub fn color(&self) -> Option<Color> {
        Color::read(self.output("color")?)
    }

    /// How much of a tool call to show, when the command line does not say.
    #[must_use]
    pub fn tool_detail(&self) -> Option<ToolDetail> {
        ToolDetail::read(self.output("toolDetail")?)
    }

    /// Which characters crucible draws with.
    #[must_use]
    pub fn glyphs(&self) -> Option<Glyphs> {
        Glyphs::read(self.output("glyphs")?)
    }

    /// Which table of colours crucible draws with.
    #[must_use]
    pub fn theme(&self) -> Option<ThemeChoice> {
        ThemeChoice::read(self.output("theme")?)
    }

    /// How many of the theme's colours the transcript spends.
    #[must_use]
    pub fn transcript_colours(&self) -> Option<TranscriptColours> {
        TranscriptColours::read(self.output("transcriptColours")?)
    }

    /// Which theme fenced code is drawn in.
    ///
    /// Free text rather than a closed set, because the answers are somebody
    /// else's theme names and a reader may drop a `.tmTheme` beside them. A
    /// name nothing knows is reported where it is read, not here.
    #[must_use]
    pub fn syntax_theme(&self) -> Option<&str> {
        self.output("syntaxTheme")
    }

    /// Whether the transcript has a scroll rail on its right edge.
    ///
    /// A yes or no rather than an `Option`, unlike the answers above: nothing
    /// on the command line can say otherwise, so what the files fall back to
    /// is the answer, and it is the one the schema states. Only a `false`
    /// turns it off.
    #[must_use]
    pub fn scroll_rail(&self) -> bool {
        self.value
            .get("output")
            .and_then(|block| block.get("scrollRail"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// How long a running tool call is out before it is drawn above the row
    /// that says a turn is running.
    ///
    /// A value rather than an `Option`, as the scroll rail is. The walk has
    /// already refused a number outside its bounds, so what is read here is
    /// either one of them or nothing, and nothing is the default the schema
    /// states.
    #[must_use]
    pub fn pin_after(&self) -> std::time::Duration {
        let seconds = self
            .value
            .get("output")
            .and_then(|block| block.get("pinAfterSeconds"))
            .and_then(Value::as_u64)
            .unwrap_or(PIN_AFTER_SECONDS);
        std::time::Duration::from_secs(seconds)
    }

    /// Whether crucible draws on a screen of its own or in the terminal's own
    /// buffer.
    ///
    /// A value rather than an `Option`, as the scroll rail is: nothing on the
    /// command line says otherwise, so what the files fall back to is the
    /// answer. Read once, at the start, which is the only time a screen can be
    /// taken or left alone.
    #[must_use]
    pub fn screen(&self) -> ScreenMode {
        self.output("screen")
            .and_then(ScreenMode::read)
            .unwrap_or_default()
    }

    /// One string out of the `output` block.
    fn output(&self, key: &str) -> Option<&str> {
        self.value.get("output")?.get(key)?.as_str()
    }
}

/// Whether the terminal is written to in colour.
///
/// `Auto` is not the absence of an answer — it is the answer "decide from the
/// terminal", which a layer may state to override a nearer one that did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// Follow the terminal and `NO_COLOR`.
    #[default]
    Auto,
    /// Colour on a terminal even under `NO_COLOR`; a file or pipe still gets
    /// none.
    Always,
    /// Never, even when it is.
    Never,
}

impl Color {
    /// Reads one of [`shape::COLOR`](crate::shape::COLOR).
    ///
    /// `None` for anything else, which the shape refused before this could be
    /// reached. There is no fourth answer to fall back to and no call for a
    /// panic over a string that cannot arrive; the test below is what keeps
    /// "cannot arrive" true as the set changes.
    fn read(found: &str) -> Option<Self> {
        match found {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Which table of colours the terminal is drawn with.
///
/// `Auto` is a question about the terminal rather than a table, and it stops
/// existing one layer up: the wiring answers it from the ground the terminal
/// reported, and what reaches the renderer is always one of the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// Follow the terminal's own background.
    #[default]
    Auto,
    /// For a dark ground.
    Dark,
    /// For a light one.
    Light,
    /// Dark, with the diff off the red-green axis.
    ColourblindDark,
    /// Light, with the same swap.
    ColourblindLight,
    /// The sixteen the terminal already has, and nothing else.
    Ansi,
}

impl ThemeChoice {
    /// Reads one of [`shape::THEME`](crate::shape::THEME).
    ///
    /// `None` for anything else, which the shape refused before this could be
    /// reached — the test below is what keeps "cannot arrive" true as the set
    /// changes.
    fn read(found: &str) -> Option<Self> {
        match found {
            "auto" => Some(Self::Auto),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "colourblind-dark" => Some(Self::ColourblindDark),
            "colourblind-light" => Some(Self::ColourblindLight),
            "ansi" => Some(Self::Ansi),
            _ => None,
        }
    }
}

/// How wide the line of a tool call and its result may run.
///
/// Headings and result previews stay compact at either width. The expanded
/// view shows the full text of recent clipped results while they are retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolDetail {
    /// A readable measure, narrower than a wide terminal.
    #[default]
    Compact,
    /// The whole width of the terminal.
    Full,
}

impl ToolDetail {
    /// Reads one of the words `output.toolDetail` accepts, spelled as a
    /// document spells it, as a settings menu hands one over.
    #[must_use]
    pub fn read(found: &str) -> Option<Self> {
        match found {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// Which characters crucible draws its own interface with.
///
/// Stated rather than guessed. A box-drawing character that arrives at a
/// terminal whose font has no glyph for it is drawn as a hollow square, and
/// nothing about that reaches this process: the bytes were accepted, the
/// encoding was right, and the failure is in a font this program cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Glyphs {
    /// Box drawing, bullets and an ellipsis.
    #[default]
    Unicode,
    /// Characters every font has had since before there were fonts to choose.
    Ascii,
}

impl Glyphs {
    /// Reads one of the words `output.glyphs` accepts, spelled as a document
    /// spells it, as a settings menu hands one over.
    #[must_use]
    pub fn read(found: &str) -> Option<Self> {
        match found {
            "unicode" => Some(Self::Unicode),
            "ascii" => Some(Self::Ascii),
            _ => None,
        }
    }
}

/// Where crucible draws.
///
/// Fullscreen is a screen of crucible's own, with its own scrollback, rail and
/// selection. Native is the terminal's own buffer: what is finished goes into
/// its scrollback once, and the terminal scrolls, selects and copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenMode {
    /// A screen of crucible's own.
    #[default]
    Fullscreen,
    /// The terminal's own buffer and scrollback.
    Native,
}

impl ScreenMode {
    /// Reads one of the words `output.screen` accepts, spelled as a document
    /// spells it, as a settings menu hands one over.
    #[must_use]
    pub fn read(found: &str) -> Option<Self> {
        match found {
            "fullscreen" => Some(Self::Fullscreen),
            "native" => Some(Self::Native),
            _ => None,
        }
    }
}

/// How many of the theme's colours the transcript spends, and on what.
///
/// Which kind of thing gets which colour is the drawing's to say; this is only
/// which of the three the reader chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TranscriptColours {
    /// Code and paths in the theme's colour and links in a second, and
    /// nothing else in a hue.
    #[default]
    Calm,
    /// Calm, and a third and fourth colour on paths, versions and what a
    /// call was about.
    Balanced,
    /// Balanced, and headings, lists, quotes and figures in colour too.
    Rich,
}

impl TranscriptColours {
    /// Reads one of the words `output.transcriptColours` accepts, spelled as
    /// a document spells it, as a settings menu hands one over.
    #[must_use]
    pub fn read(found: &str) -> Option<Self> {
        match found {
            "calm" => Some(Self::Calm),
            "balanced" => Some(Self::Balanced),
            "rich" => Some(Self::Rich),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::document::{Document, Origin};
    use crate::shape;

    use super::*;

    #[test]
    fn the_nearest_layer_that_set_a_scalar_wins_it_outright() {
        let user = Document::sample(
            r#"{"output": {"color": "always", "toolDetail": "full"}}"#,
            Origin::User,
        );
        let local = Document::sample(r#"{"output": {"color": "never"}}"#, Origin::ProjectLocal);

        let settings = Settings::resolve(vec![user, local]);

        assert_eq!(settings.color(), Some(Color::Never));

        // `output` is still an object, so it merges key by key like any other.
        // Only the scalar inside it is replaced, and a layer that said nothing
        // about `toolDetail` has not thereby unset it.
        assert_eq!(settings.tool_detail(), Some(ToolDetail::Full));
    }

    #[test]
    fn a_setting_no_layer_mentioned_is_left_for_the_command_line_to_decide() {
        // None is "the files did not say", not a default. The default lives
        // where it already lives, and the wiring lays the command line over
        // this.
        let settings = Settings::resolve(Vec::new());

        assert_eq!(settings.color(), None);
        assert_eq!(settings.tool_detail(), None);
        assert_eq!(settings.glyphs(), None);
        assert_eq!(settings.transcript_colours(), None);
    }

    #[test]
    fn every_answer_the_document_accepts_reads_back_as_a_value() {
        // The shape decides what a document may say and this module decides
        // what each answer means, so the two lists have to agree. Without this,
        // renaming an answer in the shape leaves the reader matching a string
        // nobody can write any more, and the setting stops working with no
        // error anywhere — the schema would accept the file and the value would
        // be dropped on the floor.
        for name in shape::COLOR {
            assert!(Color::read(name).is_some(), "color: {name}");
        }
        for name in shape::TOOL_DETAIL {
            assert!(ToolDetail::read(name).is_some(), "toolDetail: {name}");
        }
        for name in shape::GLYPHS {
            assert!(Glyphs::read(name).is_some(), "glyphs: {name}");
        }
        for name in shape::THEME {
            assert!(ThemeChoice::read(name).is_some(), "theme: {name}");
        }
        for name in shape::SCREEN {
            assert!(ScreenMode::read(name).is_some(), "screen: {name}");
        }
        for name in shape::TRANSCRIPT_COLOURS {
            assert!(
                TranscriptColours::read(name).is_some(),
                "transcriptColours: {name}"
            );
        }
    }

    #[test]
    fn each_transcript_colouring_is_read_back_as_the_one_it_names() {
        let read: Vec<Option<TranscriptColours>> = shape::TRANSCRIPT_COLOURS
            .iter()
            .map(|name| {
                let text = format!(r#"{{"output": {{"transcriptColours": "{name}"}}}}"#);
                Settings::resolve(vec![Document::sample(&text, Origin::User)]).transcript_colours()
            })
            .collect();

        assert_eq!(
            read,
            [
                Some(TranscriptColours::Calm),
                Some(TranscriptColours::Balanced),
                Some(TranscriptColours::Rich)
            ]
        );
    }

    #[test]
    fn a_theme_is_read_back_as_the_table_it_names() {
        let user = Document::sample(r#"{"output": {"theme": "light"}}"#, Origin::User);

        assert_eq!(
            Settings::resolve(vec![user]).theme(),
            Some(ThemeChoice::Light)
        );
    }

    #[test]
    fn a_running_call_is_held_back_for_the_seconds_a_layer_says_and_no_more_than_a_minute() {
        let read = |text: &str| Document::parse(text, "settings.json", Origin::User);

        assert_eq!(
            Settings::resolve(Vec::new()).pin_after(),
            std::time::Duration::from_secs(3)
        );
        for seconds in [0, 1, 60] {
            let document = read(&format!(
                r#"{{"output": {{"pinAfterSeconds": {seconds}}}}}"#
            ))
            .unwrap_or_else(|error| panic!("{seconds} was refused: {error}"));
            assert_eq!(
                Settings::resolve(vec![document]).pin_after(),
                std::time::Duration::from_secs(seconds)
            );
        }
        for written in ["61", "-1", "1.5", r#""3""#, "true"] {
            assert!(
                read(&format!(
                    r#"{{"output": {{"pinAfterSeconds": {written}}}}}"#
                ))
                .is_err(),
                "{written} was accepted"
            );
        }
    }

    #[test]
    fn auto_is_an_answer_a_layer_can_state_rather_than_the_absence_of_one() {
        // The same shape `output.color` has: a nearer layer says `auto` to
        // undo a theme a further one named, and that is not the same as saying
        // nothing at all.
        let user = Document::sample(r#"{"output": {"theme": "colourblind-dark"}}"#, Origin::User);
        let local = Document::sample(r#"{"output": {"theme": "auto"}}"#, Origin::ProjectLocal);

        assert_eq!(
            Settings::resolve(vec![user, local]).theme(),
            Some(ThemeChoice::Auto)
        );
        assert_eq!(Settings::resolve(Vec::new()).theme(), None);
    }

    #[test]
    fn every_theme_the_shape_accepts_is_a_different_one() {
        // A reader that mapped two names onto one table would pass the check
        // above and still lose a theme.
        let read: Vec<ThemeChoice> = shape::THEME
            .iter()
            .filter_map(|name| ThemeChoice::read(name))
            .collect();

        for (at, one) in read.iter().enumerate() {
            for other in read.iter().skip(at + 1) {
                assert_ne!(one, other, "two names for one theme");
            }
        }
        assert_eq!(read.len(), shape::THEME.len());
    }

    #[test]
    fn the_scroll_rail_is_drawn_unless_a_layer_turns_it_off() {
        let off = Document::sample(r#"{"output": {"scrollRail": false}}"#, Origin::User);
        let on = Document::sample(r#"{"output": {"scrollRail": true}}"#, Origin::ProjectLocal);

        assert!(Settings::resolve(Vec::new()).scroll_rail());
        assert!(!Settings::resolve(vec![off.clone()]).scroll_rail());
        assert!(Settings::resolve(vec![off, on]).scroll_rail());
    }

    #[test]
    fn the_screen_is_fullscreen_unless_a_layer_says_native() {
        let native = Document::sample(r#"{"output": {"screen": "native"}}"#, Origin::User);
        let back = Document::sample(
            r#"{"output": {"screen": "fullscreen"}}"#,
            Origin::ProjectLocal,
        );

        assert_eq!(
            Settings::resolve(Vec::new()).screen(),
            ScreenMode::Fullscreen
        );
        assert_eq!(
            Settings::resolve(vec![native.clone()]).screen(),
            ScreenMode::Native
        );
        assert_eq!(
            Settings::resolve(vec![native, back]).screen(),
            ScreenMode::Fullscreen
        );
    }

    #[test]
    fn the_defaults_the_schema_states_for_output_are_the_ones_it_falls_back_to() {
        assert_eq!(
            Color::read(shape::usual(&["output", "color"])),
            Some(Color::default())
        );
        assert_eq!(
            ThemeChoice::read(shape::usual(&["output", "theme"])),
            Some(ThemeChoice::default())
        );
        assert_eq!(
            Glyphs::read(shape::usual(&["output", "glyphs"])),
            Some(Glyphs::default())
        );
        assert_eq!(
            ToolDetail::read(shape::usual(&["output", "toolDetail"])),
            Some(ToolDetail::default())
        );
        assert_eq!(
            TranscriptColours::read(shape::usual(&["output", "transcriptColours"])),
            Some(TranscriptColours::default())
        );
        assert_eq!(
            shape::usual(&["output", "scrollRail"]).parse::<bool>(),
            Ok(Settings::resolve(Vec::new()).scroll_rail())
        );
        assert_eq!(
            ScreenMode::read(shape::usual(&["output", "screen"])),
            Some(Settings::resolve(Vec::new()).screen())
        );
        assert_eq!(
            shape::usual(&["output", "pinAfterSeconds"]).parse::<u64>(),
            Ok(Settings::resolve(Vec::new()).pin_after().as_secs())
        );
    }
}
