//! What the layers together say the terminal shows.
//!
//! Both answers here are strings in the document and values in the program, so
//! the reading of each sits beside the type it produces. Nothing in this module
//! decides anything about a terminal — that is `Style`'s job, one crate up,
//! from these answers and what the terminal itself reports.

use super::Settings;

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

    /// Which theme fenced code is drawn in.
    ///
    /// Free text rather than a closed set, because the answers are somebody
    /// else's theme names and a reader may drop a `.tmTheme` beside them. A
    /// name nothing knows is reported where it is read, not here.
    #[must_use]
    pub fn syntax_theme(&self) -> Option<&str> {
        self.output("syntaxTheme")
    }

    /// Whether crucible takes the mouse from the terminal.
    #[must_use]
    pub fn mouse(&self) -> Option<Mouse> {
        Mouse::read(self.output("mouse")?)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Follow the terminal and `NO_COLOR`.
    Auto,
    /// Colour even when the output is not a terminal.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Follow the terminal's own background.
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

/// How much of a tool call and its result one line shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDetail {
    /// Truncated to fit a line.
    Compact,
    /// Whatever the terminal is wide enough for.
    Full,
}

impl ToolDetail {
    /// Reads one of [`shape::TOOL_DETAIL`](crate::shape::TOOL_DETAIL).
    fn read(found: &str) -> Option<Self> {
        match found {
            "compact" => Some(Self::Compact),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// Who the mouse belongs to for the length of a session.
///
/// The two ends of one trade rather than a preference. A terminal forwarding
/// buttons to crucible is not using them itself, and the wheel is a button —
/// so a session where clicking means something to crucible is a session where
/// the wheel no longer scrolls the terminal's scrollback, which is where
/// crucible's transcript lives. An inline renderer cannot offer both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouse {
    /// The terminal keeps it: the wheel scrolls, dragging selects, the middle
    /// button pastes.
    Off,
    /// crucible takes button reports for the whole session, so a click places
    /// the cursor in the prompt and opens a result that was cut short.
    Click,
}

impl Mouse {
    /// Whether this answer means crucible asks for button reports.
    #[must_use]
    pub fn places(self) -> bool {
        matches!(self, Self::Click)
    }

    /// Reads one of [`shape::MOUSE`](crate::shape::MOUSE).
    fn read(found: &str) -> Option<Self> {
        match found {
            "off" => Some(Self::Off),
            "click" => Some(Self::Click),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyphs {
    /// Box drawing, bullets and an ellipsis.
    Unicode,
    /// Characters every font has had since before there were fonts to choose.
    Ascii,
}

impl Glyphs {
    /// Reads one of [`shape::GLYPHS`](crate::shape::GLYPHS).
    fn read(found: &str) -> Option<Self> {
        match found {
            "unicode" => Some(Self::Unicode),
            "ascii" => Some(Self::Ascii),
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
        assert_eq!(settings.mouse(), None);
    }

    #[test]
    fn the_mouse_is_the_terminals_unless_a_layer_asks_for_it() {
        // The wheel scrolls the terminal's own scrollback, which is where this
        // program's transcript lives. Taking it by default would trade the
        // whole transcript for a cursor move.
        let user = Document::sample(r#"{"output": {"mouse": "click"}}"#, Origin::User);

        assert_eq!(Settings::resolve(vec![user]).mouse(), Some(Mouse::Click));
        assert!(Mouse::Click.places());
        assert!(!Mouse::Off.places());
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
        for name in shape::MOUSE {
            assert!(Mouse::read(name).is_some(), "mouse: {name}");
        }
        for name in shape::THEME {
            assert!(ThemeChoice::read(name).is_some(), "theme: {name}");
        }
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
}
