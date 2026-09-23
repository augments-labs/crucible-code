//! What a server was given in confidence, kept out of what it says back.
//!
//! A server's `envFrom` values reach its process as environment variables, and
//! a server that repeats one — a failed login quoting its request, a debug
//! dump — would otherwise hand it to the model, the terminal and the session.
//! The pipe is the wrong place to look for it. There the value is JSON: a
//! quote arrives as `\"`, a newline as `\n`, a slash perhaps as `\/` and any
//! character as `\uXXXX`, so its own bytes need not appear at all; and a short
//! value masked in the frame would rewrite the frame around it, `"id":1` read
//! as `"id":*` for a value of `1`.
//!
//! So a value is hidden here, after decoding, in every string this crate keeps
//! of what a server said that anything but that server may read: a tool
//! result, including the kind of a content block crucible names in it rather
//! than shows; a server's words about a failure; what a sentence says of an
//! identifier crucible could not have issued, its spelling or, where hiding
//! would leave two of its member names the same, its kind alone; a catalogue's
//! names, descriptions and schemas, as the model is shown them; the name a
//! server gave itself; a version crucible refused.
//!
//! Read as sent and never hidden, because it never reaches the model, the
//! terminal or the session: the version member, a member name the protocol
//! defines and a content block's kind where it is `text`, each checked against
//! crucible's own constant and dropped; a notification, which is dropped
//! unread; and what is kept but only ever compared, or sent back to the server
//! that said it — a protocol version crucible accepted, which is one of its own
//! constants; a tool's name as the server spelled it, which names the tool to
//! that server when it is called and is compared with the names it offers after
//! a restart, beside the hidden name everything else is shown; a page cursor,
//! which asks that server for the next page; and the name of a question the
//! server asked, which goes back to it in crucible's refusal. Read as sent too
//! is a number — an identifier crucible issued, an error code, a number in a
//! schema — which is what keeps every frame readable whatever the value is; so
//! a value a server says as a number is shown where such a number is kept: an
//! error's code in what a refusal says, a number in a tool's schema, the
//! identifier of an answer to a call crucible was not waiting on. The one
//! exception is an identifier crucible could not have issued: it is kept only
//! as text for a sentence saying what arrived, so its spelling is hidden like
//! any other kept string, digits included.
//!
//! Every byte an occurrence covers reads as `*`, one per byte, overlapping
//! occurrences included. A value is looked for in each kept string whole: a
//! tool result's text blocks, each of which may hold any number of lines, are
//! kept as one string with a line break between blocks, so a value with a line
//! break in it that a server split there across two blocks is still found.
//! What a server cut short, split any other way or transformed before saying
//! it is not a whole occurrence and is not hidden, and a value that is not
//! UTF-8 cannot occur whole in JSON text at all.
//!
//! Hiding can make two member names of one object read the same. A schema
//! where that happens is refused rather than shown with one of them gone, and
//! an identifier crucible could not have issued, where it happens, is named by
//! its kind rather than quoted with one of them gone.

use std::borrow::Cow;
use std::fmt;

use crucible_sandbox::SandboxEnvironment;
use serde_json::Value;

/// The values a server was given in confidence, as what it says is read.
#[derive(Clone, Default)]
pub struct Withheld {
    /// Each non-empty value, in the order it was given.
    values: Box<[Sought]>,
}

/// Two member names of one object that read the same once hidden.
///
/// An object holds one member per name, so keeping both is not possible and
/// keeping either would be a member the server wrote silently gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("two member names of one object read the same once what the server was given is hidden")]
pub struct Indistinct;

/// One value, ready to be looked for.
#[derive(Clone)]
struct Sought {
    /// The value.
    value: Box<str>,
    /// For each length of the value's start matched so far, the longest
    /// shorter start that is also an end of it: where a search falls back to
    /// when the next byte does not continue the match, so no byte is read
    /// twice and no occurrence, overlapping or not, is passed over.
    fallback: Box<[usize]>,
}

impl Sought {
    /// `value`, with its fallback worked out once.
    fn new(value: Box<str>) -> Self {
        let bytes = value.as_bytes();
        let mut fallback = vec![0; bytes.len().saturating_add(1)];
        let mut matched = 0;
        for (at, byte) in bytes.iter().enumerate().skip(1) {
            while matched > 0 && bytes.get(matched) != Some(byte) {
                matched = fallback.get(matched).copied().unwrap_or(0);
            }
            if bytes.get(matched) == Some(byte) {
                matched = matched.saturating_add(1);
            }
            if let Some(slot) = fallback.get_mut(at.saturating_add(1)) {
                *slot = matched;
            }
        }
        Self {
            value,
            fallback: fallback.into(),
        }
    }

    /// Marks every byte of `text` that some occurrence of the value covers,
    /// overlapping occurrences included, in one pass.
    ///
    /// Each byte is read once and marked at most once, whatever the value's
    /// length, so the work is the text's length and not its length times the
    /// value's.
    fn mark(&self, text: &str, marks: &mut Option<Vec<bool>>) {
        let bytes = self.value.as_bytes();
        let whole = bytes.len();
        let mut matched = 0;
        let mut marked_to = 0;
        for (at, byte) in text.bytes().enumerate() {
            while matched > 0 && (matched == whole || bytes.get(matched) != Some(&byte)) {
                matched = self.fallback.get(matched).copied().unwrap_or(0);
            }
            if bytes.get(matched) == Some(&byte) {
                matched = matched.saturating_add(1);
            }
            if matched == whole {
                let end = at.saturating_add(1);
                let from = end.saturating_sub(whole).max(marked_to);
                let marks = marks.get_or_insert_with(|| vec![false; text.len()]);
                if let Some(span) = marks.get_mut(from..end) {
                    span.fill(true);
                }
                marked_to = end;
            }
        }
    }
}

impl Withheld {
    /// Nothing: a server given no credential says nothing that needs hiding.
    #[must_use]
    pub fn nothing() -> Self {
        Self::default()
    }

    /// These values, less any that are empty and so could hide nothing.
    #[must_use]
    pub fn new<I>(values: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Box<str>>,
    {
        Self {
            values: values
                .into_iter()
                .map(Into::<Box<str>>::into)
                .filter(|value| !value.is_empty())
                .map(Sought::new)
                .collect(),
        }
    }

    /// Every credential `environment` gives the process.
    ///
    /// A value that is not UTF-8 is left out: JSON text is UTF-8, so no string
    /// a server sends can hold it whole.
    #[must_use]
    pub fn given(environment: &SandboxEnvironment) -> Self {
        Self::new(
            environment
                .credential_values()
                .filter_map(std::ffi::OsStr::to_str),
        )
    }

    /// Whether there is nothing to hide.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// `text` with every byte some whole occurrence of some value covers read
    /// as `*`, and borrowed where there was none.
    ///
    /// Overlapping occurrences are each found: in `ababa`, a value of `aba`
    /// occurs twice and all five bytes are hidden. Every value is looked for
    /// in the text as the server said it, so hiding one never makes or breaks
    /// an occurrence of another. A value found in UTF-8 text starts and ends
    /// on a character boundary, so what is hidden is whole characters and the
    /// result is still text.
    #[must_use]
    pub fn hide<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut marks: Option<Vec<bool>> = None;
        for sought in &self.values {
            sought.mark(text, &mut marks);
        }
        let Some(marks) = marks else {
            return Cow::Borrowed(text);
        };
        let mut shown = String::with_capacity(text.len());
        for (at, one) in text.char_indices() {
            if marks.get(at).copied().unwrap_or(false) {
                shown.extend(std::iter::repeat_n('*', one.len_utf8()));
            } else {
                shown.push(one);
            }
        }
        Cow::Owned(shown)
    }

    /// `value` with every string in it, and every member name, hidden as
    /// [`Self::hide`] hides text. Numbers, booleans and nulls are left as they
    /// are.
    ///
    /// Recursion is bounded by the nesting the JSON parser accepts, which is
    /// the only way a value reaches this.
    ///
    /// # Errors
    ///
    /// [`Indistinct`] where two member names of one object read the same once
    /// hidden. What is left in `value` then is missing members, and is not
    /// anything the server said.
    pub fn hide_in(&self, value: &mut Value) -> Result<(), Indistinct> {
        if self.is_empty() {
            return Ok(());
        }
        match value {
            Value::String(text) => {
                if let Cow::Owned(shown) = self.hide(text) {
                    *text = shown;
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.hide_in(item)?;
                }
            }
            Value::Object(members) => {
                for (name, mut member) in std::mem::take(members) {
                    self.hide_in(&mut member)?;
                    if members
                        .insert(self.hide(&name).into_owned(), member)
                        .is_some()
                    {
                        return Err(Indistinct);
                    }
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
        Ok(())
    }
}

impl fmt::Debug for Withheld {
    /// How many values, and never one of them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Withheld")
            .field("values", &self.values.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Withheld;

    #[test]
    fn every_whole_occurrence_is_hidden_byte_for_byte_and_nothing_else() {
        let withheld = Withheld::new(["", "sé\"cret", "ab"]);
        assert_eq!(
            withheld.hide("x sé\"cret y abab aba é"),
            "x ******** y **** **a é",
            "a withheld value in decoded text was not hidden"
        );
        assert_eq!(withheld.hide("nothing here"), "nothing here");
        assert!(
            Withheld::new([""]).is_empty(),
            "an empty value hides nothing"
        );
    }

    #[test]
    fn an_occurrence_overlapping_another_is_hidden_whole_too() {
        for (value, said, hidden) in [
            ("aba", "ababa", "*****"),
            ("abab", "xababab", "x******"),
            ("tokentoken", "tokentokentoken", "***************"),
        ] {
            assert_eq!(
                Withheld::new([value]).hide(said),
                hidden,
                "an occurrence of {value:?} overlapping an earlier one was left partly shown"
            );
        }
    }

    /// Every string over `a` and `b` of each length up to `longest`.
    fn every(longest: usize) -> Vec<String> {
        let mut all = vec![String::new()];
        let mut last = vec![String::new()];
        for _ in 0..longest {
            last = last
                .iter()
                .flat_map(|held| [format!("{held}a"), format!("{held}b")])
                .collect();
            all.extend(last.iter().cloned());
        }
        all
    }

    #[test]
    fn hiding_covers_exactly_what_comparing_at_every_position_finds() {
        let texts = every(8);
        let values = every(3);
        let sets = values
            .iter()
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.clone()])
            .chain([
                vec!["aa".to_owned(), "ab".to_owned()],
                vec!["aba".to_owned(), "b".to_owned()],
            ]);
        for set in sets {
            let withheld = Withheld::new(set.clone());
            for text in &texts {
                let mut expected = text.clone().into_bytes();
                for value in &set {
                    for at in 0..=text.len().saturating_sub(value.len()) {
                        if text
                            .get(at..)
                            .is_some_and(|rest| rest.starts_with(value.as_str()))
                            && let Some(span) = expected.get_mut(at..at + value.len())
                        {
                            span.fill(b'*');
                        }
                    }
                }
                let expected = String::from_utf8(expected).expect("ascii stays text");
                assert_eq!(
                    withheld.hide(text),
                    expected,
                    "hiding {set:?} in {text:?} differs from comparing at every position"
                );
            }
        }
    }

    #[test]
    fn strings_and_member_names_are_hidden_and_numbers_are_not() {
        let withheld = Withheld::new(["1"]);
        let mut value = json!({ "k1": ["v1", 1, 11.5, true, null], "n": 1 });
        withheld
            .hide_in(&mut value)
            .expect("no two member names hide alike");
        assert_eq!(
            value,
            json!({ "k*": ["v*", 1, 11.5, true, null], "n": 1 }),
            "a withheld value in a decoded string was not hidden, or a number was touched"
        );
    }

    #[test]
    fn debug_never_shows_a_value() {
        let shown = format!("{:?}", Withheld::new(["sk-canary"]));
        assert!(!shown.contains("sk-canary"), "{shown}");
    }
}
