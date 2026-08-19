//! Fenced code, read for what each run of it is.
//!
//! **Six answers, and no more.** A syntax theme distinguishes a great many
//! things and this keeps six of them — comment, keyword, string, number, name,
//! operator — because six is about as many as a reader tells apart on a
//! terminal with sixteen colours, and because a [`Slot`] is a decision about
//! what something *means*. Everything else in a block is the reader's own
//! foreground, which is what most of a block is anyway.
//!
//! **Nothing is loaded until a fence arrives.** The syntax definitions and the
//! themes are about a megabyte and a millisecond between them, and the first
//! frame is budgeted at twenty. A session that never shows code never pays.
//!
//! **A language nothing knows is not read at all.** [`Syntax::of`] answers
//! `None`, the caller leaves the block quiet, and that is the same block this
//! program drew before any of this existed — a disappointment rather than a
//! defect.
//!
//! **A reading is a partition of the line.** Every byte handed in comes back
//! exactly once, in order. A byte dropped is code that quietly changed meaning
//! on screen; a byte doubled is a row wider than it measured itself to be.

use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme as Highlighting, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::color::Slot;

/// The syntax definitions, read once and kept.
///
/// A `OnceLock` rather than a field, because the parser is shared by every
/// block in every session and holds no state of its own — what carries state
/// from line to line is [`Syntax`], which is per block.
static PARSERS: OnceLock<SyntaxSet> = OnceLock::new();

/// The themes that ship, read once and kept, for the same reason.
static THEMES: OnceLock<ThemeSet> = OnceLock::new();

/// What a session draws code in when nothing has said.
///
/// One of the dark ones, because that is what the theme `auto` settles on where
/// a terminal will not say what its background is.
pub const THEME_UNLESS_SAID: &str = "Monokai Extended";

/// The syntax definitions, loading them if this is the first fence.
fn parsers() -> &'static SyntaxSet {
    PARSERS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// The themes, loading them if this is the first fence.
///
/// The ones people name — Monokai, GitHub, Dracula, Nord, gruvbox and the rest
/// — rather than the seven base16-and-Solarized that syntect alone ships. A
/// picker whose list nobody recognises is a picker nobody uses.
fn themes() -> &'static ThemeSet {
    THEMES.get_or_init(|| two_face::theme::extra().into())
}

/// What the six slots are worth in `theme`, in the order [`Palette::reading`]
/// takes them.
///
/// A scope carries no colour of its own — a theme says what a scope is worth —
/// so each of the six is asked for by the scope a theme is most likely to have
/// an opinion about, and falls back to the theme's own foreground where it has
/// none.
///
/// [`Palette::reading`]: crate::Palette::reading
#[must_use]
pub fn colours(theme: &str) -> Option<[(u8, u8, u8); 6]> {
    let theme = themes().themes.get(theme)?;

    Some([
        of(theme, Slot::Comment),
        of(theme, Slot::Keyword),
        of(theme, Slot::Str),
        of(theme, Slot::Number),
        of(theme, Slot::Name),
        of(theme, Slot::Operator),
    ])
}

/// Every syntax theme there is to choose from, built in and otherwise.
#[must_use]
pub fn every_theme() -> Vec<String> {
    let mut named: Vec<String> = themes().themes.keys().cloned().collect();
    named.sort();
    named
}

/// What one slot is worth in one theme.
fn of(theme: &Highlighting, slot: Slot) -> (u8, u8, u8) {
    let scope = match slot {
        Slot::Comment => "comment",
        Slot::Keyword => "keyword",
        Slot::Str => "string",
        Slot::Number => "constant.numeric",
        Slot::Name => "entity.name.function",
        _ => "keyword.operator",
    };

    let wanted: Option<syntect::parsing::Scope> = scope.parse().ok();
    let found = wanted
        .and_then(|wanted| {
            theme
                .scopes
                .iter()
                .find(|item| item.scope.does_match(&[wanted]).is_some())
        })
        .and_then(|item| item.style.foreground);

    let colour = found.or(theme.settings.foreground);

    colour.map_or((255, 255, 255), |found| (found.r, found.g, found.b))
}

/// Reads one block of code, one line at a time.
///
/// Held for the length of a fence rather than made per line, because a string
/// or a block comment opened on one line and closed on another is one run, and
/// a reader that started fresh each line would draw the middle of it as code.
pub struct Syntax {
    reading: HighlightLines<'static>,
}

impl std::fmt::Debug for Syntax {
    /// By hand: what `HighlightLines` holds is a parser state machine, and its
    /// derived `Debug` is pages of it.
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        into.debug_struct("Syntax").finish_non_exhaustive()
    }
}

impl Syntax {
    /// One ready to read `language`, or `None` where nothing here knows it.
    ///
    /// The name is whatever the fence said, which is a word somebody typed: it
    /// is matched against the extensions and the names the definitions carry,
    /// case-insensitively, so `rs`, `Rust` and `rust` all arrive.
    #[must_use]
    pub fn of(language: &str) -> Option<Self> {
        let language = language.trim();
        if language.is_empty() {
            return None;
        }

        let parsers = parsers();
        let found = named(parsers, language)?;

        // A plain-text definition matches a great many things and highlights
        // none of them, so answering with it would turn "nothing knows this"
        // into "this is read and is all one colour" — which costs the load and
        // buys nothing.
        if found.name.eq_ignore_ascii_case("plain text") {
            return None;
        }

        Some(Self {
            reading: HighlightLines::new(found, plain()),
        })
    }

    /// The runs of `line`, in order, each with the slot it turned out to be.
    ///
    /// A line that cannot be read is handed back whole and plain rather than
    /// dropped: the block is the model's words either way.
    pub fn read(&mut self, line: &str, say: &mut dyn FnMut(Slot, &str)) {
        let Ok(runs) = self.reading.highlight_line(line, parsers()) else {
            say(Slot::Plain, line);
            return;
        };

        for (style, text) in runs {
            say(slot(style.foreground), text);
        }
    }
}

/// What a fence calls a language, where the definitions call it something else
/// or do not carry it at all.
///
/// Two kinds of entry, and the difference matters. Most are spellings — `ts` is
/// TypeScript and nothing here disagrees. The rest are **admissions**: there is
/// no TypeScript definition in the set that ships, so it is read as JavaScript,
/// which it is a superset of. What that costs is the types: `interface`, `enum`
/// and an annotation after a colon are drawn as ordinary words. That is worth
/// having over a block drawn flat, and it is worth saying out loud rather than
/// leaving somebody to notice.
const CALLED: [(&str, &str); 10] = [
    // Read as JavaScript, which they extend.
    ("typescript", "js"),
    ("ts", "js"),
    ("tsx", "js"),
    ("mts", "js"),
    ("cts", "js"),
    ("jsx", "js"),
    // Spellings the definitions do not list.
    ("shell", "sh"),
    ("console", "sh"),
    ("golang", "go"),
    ("rust", "rs"),
];

/// The definition `language` names, by extension or by name.
fn named<'a>(parsers: &'a SyntaxSet, language: &str) -> Option<&'a SyntaxReference> {
    let language = CALLED
        .iter()
        .find(|(spelled, _)| spelled.eq_ignore_ascii_case(language))
        .map_or(language, |(_, called)| called);

    parsers
        .find_syntax_by_extension(language)
        .or_else(|| parsers.find_syntax_by_token(language))
        .or_else(|| {
            parsers
                .syntaxes()
                .iter()
                .find(|syntax| syntax.name.eq_ignore_ascii_case(language))
        })
}

/// The theme the parser is driven with.
///
/// Not the reader's. What is wanted out of `syntect` here is *which scope* each
/// run is, and a scope is a fact about the language rather than about anybody's
/// colours — so the parser is given one fixed theme whose foregrounds this file
/// reads back as the six slots, and the reader's own theme decides what those
/// six are worth much later, in the palette.
fn plain() -> &'static Highlighting {
    static PLAIN: OnceLock<Highlighting> = OnceLock::new();

    PLAIN.get_or_init(|| {
        let scopes = MARKERS
            .iter()
            .filter_map(|(scope, marker)| {
                Some(syntect::highlighting::ThemeItem {
                    scope: (*scope).parse().ok()?,
                    style: syntect::highlighting::StyleModifier {
                        foreground: Some(syntect::highlighting::Color {
                            r: *marker,
                            g: 0,
                            b: 0,
                            a: 255,
                        }),
                        background: None,
                        font_style: None,
                    },
                })
            })
            .collect();

        Highlighting {
            scopes,
            ..Highlighting::default()
        }
    })
}

/// The scopes each slot answers to, and the marker the parser is told to paint
/// them with.
///
/// The marker is a number rather than a colour: it travels out of `syntect` in
/// the red channel and is read straight back as a slot. Scopes are listed
/// longest-first, because that is the order a theme's own matching walks them
/// and the more specific rule has to win.
const MARKERS: [(&str, u8); 8] = [
    ("comment", 1),
    ("string", 3),
    ("constant.numeric", 4),
    ("constant.character", 3),
    ("keyword", 2),
    ("storage", 2),
    ("entity.name", 5),
    ("keyword.operator", 6),
];

/// The slot a marker came back as.
fn slot(marker: syntect::highlighting::Color) -> Slot {
    match Some(marker.r) {
        Some(1) => Slot::Comment,
        Some(2) => Slot::Keyword,
        Some(3) => Slot::Str,
        Some(4) => Slot::Number,
        Some(5) => Slot::Name,
        Some(6) => Slot::Operator,
        _ => Slot::Plain,
    }
}

#[cfg(test)]
mod tests;
