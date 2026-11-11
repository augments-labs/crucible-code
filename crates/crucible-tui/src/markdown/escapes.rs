//! The two escapes the reader resolves in prose: a backslash before a line
//! break, and the named and numeric references that stand for one character.
//!
//! A backslash that ends a line breaks it, and is not drawn, when the line
//! under it is more of the same paragraph. It is held with the break after it
//! and the start of that line, until the start says what the line is, and is
//! then given back as written or dropped. A blank line is the paragraph's end,
//! and so is a line the reader opens a block with -- a heading, an item with a
//! bullet or a number, a quote, a fence, a rule, a table, display math, a
//! `<details>` or a `<summary>` -- or makes the title of the line above, or its
//! table's delimiter row, which it does whether or not a backslash stood in
//! front of it, so the backslash is drawn there as it was written. A line that
//! only starts like one is more of the paragraph: a tab before a marker, a bar
//! that opens no table, a `-` under an item, a number without its stop and
//! blank. What the first characters say is read here, and what takes more of
//! the line is read by the reader's own table and title readers, fed the
//! characters of the line as the reader's are, so that they decide as those
//! do. The break and the line go through the scan afterwards as if no
//! backslash had stood in front of them, so everything that watches a line
//! sees the same stream. The hold is the spaces and a few characters of the
//! line, or as much of it as a header, a table or a tag takes to be read, which
//! those readers bound.
//! Where the backslash is not in prose -- code, a span, a heading -- it never
//! reaches this module and is a character like any other, and a reader made
//! with [`Markdown::unescaped`] reads neither it nor a reference: that is the
//! reader a block shown as written is laid out with, and what such a block
//! says is what was written.
//!
//! A reference is held from its `&` until it is whole or is not one, and is
//! then said as text under the slot the words around it wear, or into the run
//! of a `<sub>` or a `<sup>` it stands in. What it names is never read again:
//! `&lt;details&gt;` opens nothing, `&#42;` leans on nothing. Three bounds keep
//! a hold short: a name is at most [`LONGEST`] letters, a number at most
//! [`DIGITS`] digits, and either needs its `;`, so a hold is never longer than
//! a dozen characters and an answer that cuts one anywhere draws what the
//! whole one draws. What is held and turns out to be no reference goes back
//! into the scan after its `&`, so a number sign that follows it is read as it
//! was before references were: an issue's number is still one.
//!
//! Hostile input: no reference is ever a way to put a character on the screen
//! that the screen would not draw as itself. A control character, a format
//! character, a surrogate, a value past the last scalar, zero, and the
//! character an image is placed with are all shown as written, by the same
//! test a row is cleaned with.

use std::borrow::Cow;

use super::table::{Sight, Table};
use super::{Inside, MOST, Markdown, PLACEHOLDER, Quoted};
use crate::color::Slot;
use crate::row;

/// The most digits a numeric reference may hold, after the `#` or the `x`.
const DIGITS: usize = 7;

/// The letters in the longest name of [`NAMED`].
pub(super) const LONGEST: usize = 6;

/// The names that are read, and what each stands for. Case matters, as it does
/// in HTML: `&Copy;` is not `&copy;`. A name that is not here is shown as it
/// was written.
pub(super) const NAMED: [(&str, char); 44] = [
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("copy", '\u{a9}'),
    ("reg", '\u{ae}'),
    ("trade", '\u{2122}'),
    ("hellip", '\u{2026}'),
    ("mdash", '\u{2014}'),
    ("ndash", '\u{2013}'),
    ("lsquo", '\u{2018}'),
    ("rsquo", '\u{2019}'),
    ("ldquo", '\u{201c}'),
    ("rdquo", '\u{201d}'),
    ("laquo", '\u{ab}'),
    ("raquo", '\u{bb}'),
    ("bull", '\u{2022}'),
    ("middot", '\u{b7}'),
    ("deg", '\u{b0}'),
    ("plusmn", '\u{b1}'),
    ("times", '\u{d7}'),
    ("divide", '\u{f7}'),
    ("micro", '\u{b5}'),
    ("para", '\u{b6}'),
    ("sect", '\u{a7}'),
    ("euro", '\u{20ac}'),
    ("pound", '\u{a3}'),
    ("yen", '\u{a5}'),
    ("cent", '\u{a2}'),
    ("larr", '\u{2190}'),
    ("rarr", '\u{2192}'),
    ("uarr", '\u{2191}'),
    ("darr", '\u{2193}'),
    ("harr", '\u{2194}'),
    ("rArr", '\u{21d2}'),
    ("lArr", '\u{21d0}'),
    ("hArr", '\u{21d4}'),
    ("ne", '\u{2260}'),
    ("le", '\u{2264}'),
    ("ge", '\u{2265}'),
    ("infin", '\u{221e}'),
    ("check", '\u{2713}'),
];

/// What the characters written so far, `&` first, say about a reference.
enum Shape {
    /// It may still become one.
    Partly,
    /// It is none, or names a character that is not shown as itself.
    Not,
    /// It is a whole reference, and this is the character it names.
    Whole(char),
}

/// What `written`, an `&` and the characters after it, is.
fn shape(written: &str) -> Shape {
    let Some(body) = written.strip_prefix('&') else {
        return Shape::Not;
    };
    let (body, closed) = body
        .strip_suffix(';')
        .map_or((body, false), |body| (body, true));

    if let Some(number) = body.strip_prefix('#') {
        let (digits, radix) = number
            .strip_prefix(['x', 'X'])
            .map_or((number, 10), |digits| (digits, 16));
        if digits.len() > DIGITS || !digits.chars().all(|digit| digit.is_digit(radix)) {
            return Shape::Not;
        }
        if !closed {
            return Shape::Partly;
        }
        return u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
            .filter(|character| shown_as_itself(*character))
            .map_or(Shape::Not, Shape::Whole);
    }

    if body.len() > LONGEST || !body.chars().all(|letter| letter.is_ascii_alphanumeric()) {
        return Shape::Not;
    }
    if !closed {
        return Shape::Partly;
    }
    NAMED
        .iter()
        .find(|(name, _)| *name == body)
        .map_or(Shape::Not, |(_, character)| Shape::Whole(*character))
}

/// Whether a row draws `character` as itself, which is the one thing a
/// reference may be read as.
fn shown_as_itself(character: char) -> bool {
    character != PLACEHOLDER && row::drawn(character) == Some(character)
}

/// `text` with every whole reference in it replaced by the character it names.
///
/// For the words of a link, which are held as written until the link is whole
/// and are handed on without being read again.
pub(super) fn decoded(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        let reference = tail
            .char_indices()
            .take(DIGITS + 4)
            .find(|(_, character)| *character == ';')
            .and_then(|(end, _)| tail.get(..=end));
        if let Some((written, Shape::Whole(character))) =
            reference.map(|written| (written, shape(written)))
        {
            out.push(character);
            rest = tail.get(written.len()..).unwrap_or_default();
        } else {
            out.push('&');
            rest = tail.get(1..).unwrap_or_default();
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// What the start of the line under a held break says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// It may still be either.
    Partly,
    /// The line opens a block, or is blank: the backslash is the paragraph's
    /// last character.
    Opens,
    /// The line is more of the paragraph: the backslash breaks it.
    Continues,
}

/// The longest start of a line that is held to tell what it opens, which is a
/// numbered item's, `123456789. `, or `\begin{smallmatrix*}` for display math.
/// A line that starts a tag is held as far as a tag is read, and one the
/// reader is still watching for a table or a title for as long as it watches.
const START: usize = 24;

/// A backslash that stands in front of a line break, held with the spaces that
/// follow the break and as much of the line after them as it takes to tell
/// what the line is.
///
/// Most of what a line opens is told by its first characters, here. What takes
/// the reader more than those is told by the reader's own readers, run on over
/// what is held: the watch on the line above, which an underline or a
/// delimiter row under it claims, and the block of bars that a line starting
/// with one opens. They are fed the characters the reader's own are, so that
/// they decide as those do.
#[derive(Debug, Clone)]
pub(super) struct Slashed {
    /// The blanks after the break, which are spaces: a tab is the line's.
    spaces: String,
    /// The first characters of the line, past [`START`] only while they are one
    /// character repeated, the rest of which is counted in `more`, or while a
    /// reader is still reading them.
    text: String,
    /// How many more of the character `text` is made of followed it.
    more: usize,
    /// The watch on the line above, where a line under it could make a table
    /// or a title of that one, read on from the break.
    watch: Option<Table>,
    /// The block of bars that the line opened with one is, read on.
    bars: Option<Table>,
    /// What the watch or the bars settled, which nothing after it changes.
    settled: Option<Verdict>,
}

impl Slashed {
    /// A backslash and its break, after a line that `watch` was watching, if
    /// it was.
    fn after(watch: Option<&Table>) -> Self {
        let watch = watch.filter(|watch| watch.mirrors()).and_then(|watch| {
            let mut watch = watch.clone();
            // The break the line ends with: kept while it is still a header.
            (watch.sees('\n') == Sight::Keep).then_some(watch)
        });
        Self {
            spaces: String::new(),
            text: String::new(),
            more: 0,
            watch,
            bars: None,
            settled: None,
        }
    }

    /// Whether a reader of the line has yet to say what it is.
    fn reading(&self) -> bool {
        self.watch.is_some() || self.bars.is_some()
    }

    /// Takes `character`, which is not one of the spaces that lead the line,
    /// into the readers that watch it, and into the start of the line.
    fn take(&mut self, character: char) {
        self.watches(character);
        let first = self.text.chars().next();
        if self.text.len() >= START
            && first == Some(character)
            && self.text.chars().all(|held| held == character)
        {
            self.more = self.more.saturating_add(1);
        } else {
            self.text.push(character);
        }
    }

    /// Feeds `character`, which the line has, to the reader's own readers.
    fn watches(&mut self, character: char) {
        if self.settled.is_some() {
            return;
        }
        if let Some(mut watch) = self.watch.take() {
            match watch.sees(character) {
                // A delimiter row, or an underline: the line above is a header
                // or a title, and the backslash is in it.
                Sight::Under(_) => self.settled = Some(Verdict::Opens),
                Sight::Keep if character == '\n' => self.settled = Some(Verdict::Opens),
                Sight::Keep => self.watch = Some(watch),
                Sight::Release | Sight::Over | Sight::Spill => {}
            }
        }
        if self.text.is_empty() && self.more == 0 && character == '|' {
            let mut bars = Table::opening();
            bars.takes(character);
            self.bars = Some(bars);
        } else if let Some(mut bars) = self.bars.take() {
            // The block goes on with a bar at the start of its second line,
            // which is its delimiter row, or it is no table.
            if (bars.fresh() && character != '|') || !bars.takes(character) || !bars.possible() {
                self.settled = Some(Verdict::Continues);
            } else if bars.delimited() {
                self.settled = Some(Verdict::Opens);
            } else {
                self.bars = Some(bars);
            }
        }
    }

    /// The break, the blanks and the start of the line as they were written.
    fn replay(&self) -> String {
        let first = self.text.chars().next();
        let run = first.map_or(0, |first| {
            self.text.chars().take_while(|c| *c == first).count()
        });
        let marks = self.text.get(..run).unwrap_or_default();
        let rest = self.text.get(run..).unwrap_or_default();
        let repeated: String = first.into_iter().cycle().take(self.more).collect();
        format!("\n{}{marks}{repeated}{rest}", self.spaces)
    }

    /// What the line says it is, given whether the answer has ended, whether
    /// the line above it was a quote's and whether this reader opens display
    /// math.
    fn says(&self, ended: bool, quoted: bool, display: bool) -> Verdict {
        if let Some(settled) = self.settled {
            return settled;
        }
        if ended {
            // The end of the answer is the break that ends the line. It makes
            // the line above a title where it is an underline, which the
            // reader says by itself; a delimiter row it ends is not one it
            // opens a table at, and is more of the paragraph.
            let claimed = self
                .watch
                .clone()
                .is_some_and(|mut watch| matches!(watch.sees('\n'), Sight::Under(_)));
            if claimed {
                return Verdict::Opens;
            }
        }
        match self.opening(ended, quoted, display) {
            Verdict::Continues if self.reading() && !ended => Verdict::Partly,
            verdict => verdict,
        }
    }

    /// What the first characters of the line say it opens, which is the whole
    /// of it for a line that opens with a mark and a blank.
    fn opening(&self, ended: bool, quoted: bool, display: bool) -> Verdict {
        let text = if ended {
            format!("{}\n", self.text)
        } else {
            self.text.clone()
        };
        let Some(first) = text.chars().next() else {
            return if ended {
                Verdict::Opens
            } else {
                Verdict::Partly
            };
        };
        let limit = match first {
            _ if self.reading() => usize::MAX,
            '<' => super::TAG_CAP,
            _ => START,
        };
        if self.text.len() > limit {
            return Verdict::Continues;
        }
        let run = text.chars().take_while(|c| *c == first).count();
        let after = text.chars().nth(run);
        let length = run + self.more;
        let by = |opens: bool| {
            if opens {
                Verdict::Opens
            } else {
                Verdict::Continues
            }
        };
        match (first, after) {
            // A blank line.
            ('\n', _) => Verdict::Opens,
            // The hashes of a heading.
            ('#', Some(after)) => by(after == ' '),
            // A bullet, or a rule.
            ('-' | '+' | '*' | '_', Some(after)) => {
                by((first != '_' && length == 1 && after == ' ')
                    || (first != '+' && length >= 3 && after == '\n'))
            }
            // A fence.
            ('`' | '~', _) if length >= 3 => Verdict::Opens,
            // A quote opens one, and the line of a quote it is in goes on with
            // it.
            ('>', Some(after)) => by(length == 1 && after == ' ' && !quoted),
            // The number of an item, which the reader draws as one when a
            // blank follows it.
            ('0'..='9', _) => numbered(&text),
            ('$' | '\\', _) if display => match super::opener(&format!("{}{text}", self.spaces)) {
                super::Opener::Partly => Verdict::Partly,
                super::Opener::Whole(_) => Verdict::Opens,
                super::Opener::Not => Verdict::Continues,
            },
            ('<', _) => match super::shape(&self.text) {
                super::Shape::Whole(tag) => by(matches!(
                    tag.name,
                    super::Name::Details | super::Name::Summary
                )),
                super::Shape::Partly => Verdict::Partly,
                super::Shape::Not => Verdict::Continues,
            },
            // Nothing but a run of one character so far.
            ('#' | '-' | '+' | '*' | '_' | '>' | '`' | '~', None) => Verdict::Partly,
            _ => Verdict::Continues,
        }
    }
}

/// What `text`, which starts with a digit, says about a line: its first word
/// is the number of an item when it is digits and a stop, and a blank, a tab or
/// the line's end follows it.
fn numbered(text: &str) -> Verdict {
    let word = text.split(char::is_whitespace).next().unwrap_or_default();
    if word.len() < text.len() {
        return if super::kind::ordinal(word) {
            Verdict::Opens
        } else {
            Verdict::Continues
        };
    }
    let digits = word.chars().take_while(char::is_ascii_digit).count();
    match word.get(digits..) {
        Some("" | "." | ")") if digits <= super::NUMBERED => Verdict::Partly,
        _ => Verdict::Continues,
    }
}

impl Markdown {
    /// Reads `character` into a backslash standing in front of a line break,
    /// and into the break, the spaces after it and the start of the line they
    /// lead to. Answers whether it was taken.
    ///
    /// The start of the line is what settles it, and is read with the break: a
    /// blank line is the paragraph's end and a line that opens a block is not
    /// part of it, so the backslash is drawn as written; anything else is more
    /// of the paragraph, so the line breaks and the backslash is not drawn.
    pub(super) fn slashes(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let (quoted, display) = (self.line.quoted != Quoted::Not, self.holds());
        let Some(held) = self.slashed.as_mut() else {
            // A heading's break is its end, and a key holds what is in it as
            // written; so does a block shown that way, and display math past
            // what is held of it.
            if self.escaped
                && character == '\n'
                && self.atx.is_none()
                && !self.line.key
                && !self.unescaped
                && self.passing.is_none()
            {
                self.escaped = false;
                self.slashed = Some(Slashed::after(self.table.as_ref().filter(|_| !self.told)));
                return true;
            }
            return false;
        };
        if held.text.is_empty() && held.more == 0 && character == ' ' && held.spaces.len() < MOST {
            held.spaces.push(character);
            held.watches(character);
            return true;
        }
        held.take(character);
        let verdict = held.says(false, quoted, display);
        match verdict {
            Verdict::Partly => {}
            Verdict::Opens => self.unslash(true, room, say),
            Verdict::Continues => self.unslash(false, room, say),
        }
        true
    }

    /// Lets go of a held backslash and its break at the end of the answer,
    /// which is the break the line under it ends with.
    pub(super) fn unslash_at_end(
        &mut self,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let Some(held) = self.slashed.as_ref() else {
            return;
        };
        let kept = held.says(true, self.line.quoted != Quoted::Not, self.holds()) == Verdict::Opens;
        self.unslash(kept, room, say);
    }

    /// Lets go of a held backslash and its break, drawing the backslash where
    /// `kept` says it stood in front of nothing a paragraph goes on with, and
    /// reads the break and the line under it as they would have been read had
    /// it not been there. The whole of it goes through one scan, so that the
    /// line meets what the break left open and not what stood before the break.
    fn unslash(&mut self, kept: bool, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let Some(held) = self.slashed.take() else {
            return;
        };
        if kept {
            self.say("\\", say);
            self.started = true;
            self.previous = '\\';
        }
        self.scan(&held.replay(), room, say);
    }

    /// Whether `character` opens a reference.
    ///
    /// Prose only, and not inside a span: code is where an entity is written
    /// down as one. Nor where a block is shown as written.
    pub(super) fn opens_entity(&self, character: char) -> bool {
        character == '&'
            && self.inside == Inside::Prose
            && !self.line.code
            && !self.unescaped
            && self.passing.is_none()
    }

    /// Reads `character` into the reference being held. Answers whether it
    /// was taken.
    ///
    /// `false` says it was no reference: what was held has been handed on as
    /// written, and `character` is the caller's again, which may open the next.
    pub(super) fn entitied(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(mut written) = self.entity.take() else {
            return false;
        };
        written.push(character);
        match shape(&written) {
            Shape::Partly => {
                self.entity = Some(written);
            }
            Shape::Whole(named) => {
                self.untag(named.encode_utf8(&mut [0; 4]), say);
            }
            Shape::Not => {
                written.pop();
                self.unentity(&written, room, say);
                // What was held may have left a number open that this is the
                // next digit of, or the end of.
                return self.reference.is_some() && self.referenced(character, say);
            }
        }
        true
    }

    /// Hands on a held reference that is none: the `&` as written, and what
    /// followed it read again, as it was before it was held.
    pub(super) fn unentity(
        &mut self,
        written: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.untag("&", say);
        self.again(written.get(1..).unwrap_or_default(), room, say);
    }
}
