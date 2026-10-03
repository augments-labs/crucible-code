//! Which kind of thing a run of an answer is, where the kind decides its slot.
//!
//! A path, a version, a commit hash, a figure and a list's number are each
//! drawn in whatever ink the reader's design gives that kind, so they are
//! told apart here, once, as the runs leave the scan. Telling them apart only
//! picks a slot: every character handed in is handed on, in order, exactly
//! once, which is what keeps the text with colour off the same in every
//! design.
//!
//! What is read is the runs the scan already settled, not the answer's
//! markers. Inline code is held until its span ends, since a span that names
//! a file is only known to once the whole of it has arrived; a word of prose
//! with a digit in it is held until the word ends, since a delta can stop
//! halfway through `0.45.0`. Both holds are bounded and both are given up on:
//! past the bound the run is handed on as the slot it arrived in.

use crate::color::Slot;

/// The longest word of prose held while it could still be a version or a hash.
///
/// A hash is forty characters at the most; this is room for that, a `v`, and
/// the punctuation around it.
const WORD: usize = 64;

/// The longest span of inline code held while it could still be a path.
const SPAN: usize = 512;

/// What may stand in front of a word without being part of what it is.
const OPENS: [char; 4] = ['(', '[', '"', '\''];

/// What may follow a word without being part of what it is.
const CLOSES: [char; 10] = ['.', ',', ';', ':', '!', '?', ')', ']', '"', '\''];

/// What a path cannot hold and a pattern or a command can.
const METACHARACTERS: &str = "*?[]{}|^$\\()<>\"'`";

/// The extensions a name with no slash in it is read as a file by.
///
/// A slash says path on its own. Without one, `Cargo.toml` is a file and
/// `self.line` is a field, and the only difference is the word after the dot,
/// so these are the words a coding agent's answer names a file with.
const EXTENSIONS: [&str; 52] = [
    "rs", "toml", "md", "json", "yaml", "yml", "lock", "txt", "html", "css", "scss", "js", "mjs",
    "cjs", "ts", "tsx", "jsx", "py", "sh", "bash", "zsh", "fish", "go", "c", "h", "cc", "cpp",
    "hpp", "java", "kt", "swift", "rb", "php", "sql", "xml", "svg", "png", "jpg", "jpeg", "gif",
    "log", "cfg", "ini", "conf", "env", "csv", "proto", "ps1", "vue", "lua", "nix", "wasm",
];

/// Whether `text` names a file or a directory.
///
/// One word with no metacharacter in it, that is not an option and not an
/// address, and that either has a slash in it or ends in an extension a file
/// is named with. Only an answer's inline code is told this way: a call row's
/// argument arrives with the kind its tool said.
fn path(text: &str) -> bool {
    if text.is_empty()
        || text.starts_with('-')
        || text.contains("://")
        || text
            .chars()
            .any(|c| c.is_whitespace() || METACHARACTERS.contains(c))
        || !text.chars().any(char::is_alphanumeric)
    {
        return false;
    }

    if text.contains('/') {
        return true;
    }

    // `draw.rs:20` and `draw.rs:20:4` are a file and a place in it.
    let named = text
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == ':')
        .trim_end_matches(':');
    let named = if text
        .get(named.len()..)
        .is_some_and(|place| place.starts_with(':'))
    {
        named
    } else {
        text
    };

    named.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

/// The end of a word of prose that could still be a version, a hash or a
/// figure, held until the word ends.
#[derive(Debug, Default)]
struct Word {
    /// What has arrived of it.
    text: String,
    /// Whether it stood first on its row, which is what makes `1.` a list's
    /// number rather than the end of a sentence.
    lead: bool,
}

/// The runs a scan settled, on their way out, and what is held of them.
#[derive(Debug, Default)]
pub(super) struct Kinds {
    /// The word of prose being held.
    word: Word,
    /// The word under way was handed on already, so the rest of it is too:
    /// half a word is never coloured as though it were the whole of one.
    spilt: bool,
    /// A span of inline code, held until it ends.
    span: String,
    /// The span under way outgrew its hold and is going out as code.
    long: bool,
    /// Something other than blank has gone out on this row.
    begun: bool,
}

impl Kinds {
    /// Hands one settled run on, under the slot its kind takes.
    pub(super) fn tell(
        &mut self,
        slot: Slot,
        text: &str,
        address: Option<&str>,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        if text.is_empty() {
            return;
        }

        match (slot, address) {
            (Slot::Plain, None) => {
                self.close_span(say);
                self.prose(text, say);
            }
            (Slot::Code, None) => {
                self.close_word(say);
                self.code(text, say);
            }
            _ => {
                self.flush(say);
                self.out(slot, text, address, say);
            }
        }
    }

    /// Hands on whatever is held, as the kind it turned out to be.
    pub(super) fn flush(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.close_word(say);
        self.close_span(say);
    }

    /// One run out, and what it does to whether the row has begun.
    fn out(
        &mut self,
        slot: Slot,
        text: &str,
        address: Option<&str>,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        if text.is_empty() {
            return;
        }

        let after = text
            .rfind('\n')
            .and_then(|at| text.get(at + 1..))
            .unwrap_or(text);
        let blank = after.chars().all(char::is_whitespace);
        self.begun = if after.len() < text.len() {
            !blank
        } else {
            self.begun || !blank
        };

        say(slot, text, address);
    }

    /// A piece of inline code, into the span being held.
    fn code(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        if self.long {
            self.out(Slot::Code, text, None, say);
            return;
        }

        if self.span.len().saturating_add(text.len()) > SPAN {
            // Longer than any path anybody wrote, so it is code, and so is the
            // rest of the span.
            let span = std::mem::take(&mut self.span);
            self.out(Slot::Code, &span, None, say);
            self.out(Slot::Code, text, None, say);
            self.long = true;
            return;
        }

        self.span.push_str(text);
    }

    /// The span being held, out as a path or as the code it arrived as.
    fn close_span(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.long = false;
        if self.span.is_empty() {
            return;
        }

        let span = std::mem::take(&mut self.span);
        let slot = if path(&span) { Slot::Path } else { Slot::Code };
        self.out(slot, &span, None, say);
    }

    /// The word being held, out as whatever kind it turned out to be.
    fn close_word(&mut self, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        self.spilt = false;
        if self.word.text.is_empty() {
            return;
        }

        let word = std::mem::take(&mut self.word);
        self.word_out(&word.text, word.lead, say);
    }

    /// A run of prose, with each word that is a kind told apart from the rest.
    fn prose(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let mut rest = text;

        // The word the last run ended in carries on into this one.
        if !self.word.text.is_empty() || self.spilt {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let (piece, after) = rest.split_at(end);

            if self.spilt {
                self.out(Slot::Plain, piece, None, say);
                self.spilt = after.is_empty();
            } else if self.word.text.len().saturating_add(piece.len()) > WORD {
                let word = std::mem::take(&mut self.word.text);
                self.out(Slot::Plain, &word, None, say);
                self.out(Slot::Plain, piece, None, say);
                self.spilt = after.is_empty();
            } else {
                self.word.text.push_str(piece);
                if after.is_empty() {
                    return;
                }
                self.close_word(say);
            }
            rest = after;
        }

        if rest.is_empty() {
            return;
        }

        // Nothing a kind is spelled with: every one of them has a digit in it.
        if !rest.bytes().any(|byte| byte.is_ascii_digit()) {
            self.out(Slot::Plain, rest, None, say);
            self.spilt = !rest.ends_with(char::is_whitespace);
            return;
        }

        self.words(rest, say);
    }

    /// Tells apart the words of a run with a digit somewhere in it.
    fn words(&mut self, text: &str, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        // Where the prose not yet handed on starts.
        let mut plain = 0;
        let mut at = 0;
        let mut lead = !self.begun;

        while let Some(skip) = text
            .get(at..)
            .and_then(|rest| rest.find(|c: char| !c.is_whitespace()))
        {
            let start = at + skip;
            if text
                .get(at..start)
                .is_some_and(|blank| blank.contains('\n'))
            {
                lead = true;
            }
            let rest = text.get(start..).unwrap_or_default();
            let end = start + rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = text.get(start..end).unwrap_or_default();

            // The run stops partway through this word, and the next one may
            // finish it as a version or a hash.
            if end == text.len() {
                let held = word.bytes().any(|byte| byte.is_ascii_digit()) && word.len() <= WORD;
                self.out(
                    Slot::Plain,
                    text.get(plain..start).unwrap_or_default(),
                    None,
                    say,
                );
                if held {
                    self.word.text.push_str(word);
                    self.word.lead = lead;
                } else {
                    self.out(Slot::Plain, word, None, say);
                    self.spilt = true;
                }
                return;
            }

            if let Some((before, core, slot)) = kind(word, lead) {
                self.out(
                    Slot::Plain,
                    text.get(plain..start + before).unwrap_or_default(),
                    None,
                    say,
                );
                self.out(slot, core, None, say);
                plain = start + before + core.len();
            }

            lead = false;
            at = end;
        }

        self.out(
            Slot::Plain,
            text.get(plain..).unwrap_or_default(),
            None,
            say,
        );
    }

    /// One whole word, out as its kind with the punctuation around it plain.
    fn word_out(&mut self, word: &str, lead: bool, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        match kind(word, lead) {
            Some((before, core, slot)) => {
                self.out(
                    Slot::Plain,
                    word.get(..before).unwrap_or_default(),
                    None,
                    say,
                );
                self.out(slot, core, None, say);
                self.out(
                    Slot::Plain,
                    word.get(before + core.len()..).unwrap_or_default(),
                    None,
                    say,
                );
            }
            None => self.out(Slot::Plain, word, None, say),
        }
    }
}

/// The kind one word of prose is, where it is one: how many bytes of
/// punctuation stand in front of it, the word itself, and its slot.
fn kind(word: &str, lead: bool) -> Option<(usize, &str, Slot)> {
    if lead && ordinal(word) {
        return Some((0, word, Slot::Ordinal));
    }

    let opened = word.trim_start_matches(OPENS);
    let core = opened.trim_end_matches(CLOSES);
    let before = word.len() - opened.len();

    if version(core) || hash(core) {
        Some((before, core, Slot::Revision))
    } else if figure(core) {
        Some((before, core, Slot::Figure))
    } else {
        None
    }
}

/// `1.` or `1)` opening a row: the number of an item in a list.
fn ordinal(word: &str) -> bool {
    word.strip_suffix(['.', ')']).is_some_and(|digits| {
        (1..=9).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Three or more numbers joined by dots, with a `v` in front or not.
fn version(word: &str) -> bool {
    let numbers = word.strip_prefix('v').unwrap_or(word);
    let parts = numbers.split('.');

    parts.clone().count() >= 3
        && parts
            .into_iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Seven to forty hex digits in one case, with both a digit and a letter
/// among them.
///
/// Either case, since tools print both, but never the two mixed, which no
/// tool prints; a word of the letters a to f alone is a word, and digits
/// alone are a figure.
fn hash(word: &str) -> bool {
    let lower = word
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let upper = word
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));

    (7..=40).contains(&word.len())
        && (lower || upper)
        && word.bytes().any(|b| b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_alphabetic())
}

/// A number, whole or with a fraction after one dot.
fn figure(word: &str) -> bool {
    let (whole, fraction) = word.split_once('.').unwrap_or((word, "0"));
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());

    digits(whole) && digits(fraction)
}
