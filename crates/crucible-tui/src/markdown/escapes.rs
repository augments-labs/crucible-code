//! The two escapes the reader resolves in prose: a backslash before a line
//! break, and the named and numeric references that stand for one character.
//!
//! A backslash that ends a line breaks it, and is not drawn, when more of the
//! same paragraph follows. It is held with the break after it until the line
//! under it has said whether it is blank, and then given back as written or
//! dropped, and the break goes through the scan as if no backslash had stood
//! in front of it, so everything that watches a line sees the same stream.
//! Where the backslash is not in prose -- code, a span, a heading, a block
//! drawn as written -- it never reaches this module and is a character like
//! any other.
//!
//! A reference is held from its `&` until it is whole or is not one, and is
//! then said as text under the slot the words around it wear. What it names
//! is never read again: `&lt;details&gt;` opens nothing, `&#42;` leans on
//! nothing. Three bounds keep a hold short: a name is at most [`LONGEST`]
//! letters, a number at most [`DIGITS`] digits, and either needs its `;`, so
//! a hold is never longer than a dozen characters and an answer that cuts one
//! anywhere draws what the whole one draws.
//!
//! Hostile input: no reference is ever a way to put a character on the screen
//! that the screen would not draw as itself. A control character, a format
//! character, a surrogate, a value past the last scalar, zero, and the
//! character an image is placed with are all shown as written, by the same
//! test a row is cleaned with.

use std::borrow::Cow;

use super::{Inside, MOST, Markdown, PLACEHOLDER};
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

impl Markdown {
    /// Reads `character` into a backslash standing in front of a line break,
    /// and into the break and the spaces after it. Answers whether it was
    /// taken.
    ///
    /// A character that is neither is what settles it, and is read with the
    /// break: a blank line is the paragraph's end, so the backslash is drawn as
    /// written; anything else is more of the paragraph, so the line breaks and
    /// the backslash is not drawn.
    pub(super) fn slashes(
        &mut self,
        character: char,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) -> bool {
        let Some(spaces) = self.slashed.as_mut() else {
            // A heading's break is its end, and a key holds what is in it as
            // written.
            if self.escaped && character == '\n' && self.atx.is_none() && !self.line.key {
                self.escaped = false;
                self.slashed = Some(String::new());
                return true;
            }
            return false;
        };
        if matches!(character, ' ' | '\t') && spaces.len() < MOST {
            spaces.push(character);
            return true;
        }
        self.unslash(
            character == '\n',
            character.encode_utf8(&mut [0; 4]),
            room,
            say,
        );
        true
    }

    /// Lets go of a held backslash and its break, drawing the backslash where
    /// `kept` says it stood in front of nothing, and reads the break, the
    /// spaces after it and `then` as they would have been read had it not been
    /// there. The whole of it goes through one scan, so that `then` meets what
    /// the break left open and not what stood before the break.
    pub(super) fn unslash(
        &mut self,
        kept: bool,
        then: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let Some(spaces) = self.slashed.take() else {
            return;
        };
        if kept {
            self.say("\\", say);
            self.started = true;
            self.previous = '\\';
        }
        self.scan(&format!("\n{spaces}{then}"), room, say);
    }

    /// Whether `character` opens a reference.
    ///
    /// Prose only, and not inside a span: code is where an entity is written
    /// down as one.
    pub(super) fn opens_entity(&self, character: char) -> bool {
        character == '&' && self.inside == Inside::Prose && !self.line.code
    }

    /// Reads `character` into the reference being held. Answers whether it
    /// was taken.
    ///
    /// `false` says it was no reference: what was held has gone out as
    /// written, and `character` is the caller's again, which may open the next.
    pub(super) fn entitied(
        &mut self,
        character: char,
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
                self.started = true;
                self.previous = named;
                self.say(named.encode_utf8(&mut [0; 4]), say);
            }
            Shape::Not => {
                written.pop();
                self.unentity(&written, say);
                return false;
            }
        }
        true
    }

    /// Hands on a held reference as it was written.
    pub(super) fn unentity(
        &mut self,
        written: &str,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.started = true;
        self.previous = written.chars().next_back().unwrap_or('&');
        self.say(written, say);
    }
}
