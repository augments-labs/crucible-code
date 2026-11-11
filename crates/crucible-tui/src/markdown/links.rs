//! Links, addresses and forge references, as the reader draws them.
//!
//! Every link the reader settles -- written out, bare, between angle brackets
//! or tied to a definition -- reaches the terminal through [`Markdown::drew`],
//! which is the one place that decides what stands for it on the row:
//!
//! - The address of a pull request, an issue or a commit on a forge
//!   ([`Reference`]) is drawn as the reference people call it by, `#1324` in
//!   the checkout's own repository and `owner/repo#1324` anywhere else.
//! - Words that are not their address are followed by a space and the quiet
//!   address: the address without its scheme, or `mailto:`, and once that is
//!   past [`QUIET`] columns, its host and first segment and an ellipsis.
//!   Nothing standing before the ellipsis is wider than [`QUIET`].
//! - An address drawn as its own words has nothing after it.
//!
//! Invariants:
//!
//! - The quiet address is text for the eye: it points nowhere, is read as no
//!   markdown, and a drag copies the words without it. It is handed on as
//!   [`Said::Aside`], never as a run.
//! - The whole of every address drawn shorter than it was written is told as
//!   [`Said::Shortened`], so the record can list it; nothing else is.
//! - A reader showing a block exactly as written draws every link as the base
//!   reader did: its words and nothing after them.
//! - The scan's own channel carries both as a run whose address is
//!   [`UNLINKED`], which no link ever has, and [`said`] is the one place that
//!   turns them back into what they are.

use super::{Link, Markdown, Part, SCHEMES, Said, Spelled, WWW, escapes, spelled};
use crate::color::Slot;
use crate::forge::Reference;
use crate::glyphs::Glyphs;
use crate::width;

/// The most addresses drawn shorter than written that one answer tells of, and
/// that the record keeps whole.
pub(crate) const SHORTENED_CAP: usize = 64;

/// The widest a quiet address is, in columns, before its ellipsis.
pub(super) const QUIET: usize = 24;

/// What stands in the scan's channel for an address on a run that is no link:
/// an aside under [`Slot::Quiet`], the whole of a shortened address under
/// [`Slot::Link`]. A link always has an address to point at, so no link run
/// carries this one.
pub(super) const UNLINKED: &str = "";

/// What an e-mail address is written after in the address it is linked by.
const MAILTO: &str = "mailto:";

/// What the scan said, as what the reader tells.
pub(super) fn said<'a>(slot: Slot, text: &'a str, link: Option<&'a str>) -> Said<'a> {
    match (slot, link) {
        (Slot::Quiet, Some(UNLINKED)) => Said::Aside(text),
        (Slot::Link, Some(UNLINKED)) => Said::Shortened(text),
        _ => Said::Run { slot, text, link },
    }
}

/// The whole address in a run the scan said, where it is one drawn shorter
/// than written.
///
/// Told at once rather than through the phrase or the kinds the runs pass
/// through, since it is drawn nowhere.
pub(super) fn shortened<'a>(slot: Slot, text: &'a str, link: Option<&str>) -> Option<&'a str> {
    (slot == Slot::Link && link == Some(UNLINKED)).then_some(text)
}

impl Markdown {
    /// Draws a link's `words`, written to go to `target`, pointing at `link`.
    ///
    /// The one place a link is drawn from; the module's opening says what it
    /// draws.
    pub(super) fn drew(
        &mut self,
        words: &str,
        target: &str,
        link: Option<&str>,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        if self.unescaped || target.is_empty() {
            say(Slot::Link, words, link);
            return;
        }
        let own = own(words, target);

        if let Some(reference) = Reference::read(target, self.forge.as_ref()) {
            let short = reference.short(self.forge.as_ref());
            if own {
                say(Slot::Link, &short, link);
            } else {
                say(Slot::Link, words, link);
                say(Slot::Quiet, &format!(" ({short})"), Some(UNLINKED));
            }
            say(Slot::Link, target, Some(UNLINKED));
            return;
        }

        say(Slot::Link, words, link);
        if own {
            return;
        }
        let (quiet, cut) = quiet(target, self.glyphs);
        say(Slot::Quiet, &format!(" ({quiet})"), Some(UNLINKED));
        if cut {
            say(Slot::Link, target, Some(UNLINKED));
        }
    }

    /// Hands on an address the answer wrote bare.
    ///
    /// Drawn as the link it is, so what can be acted on in a terminal looks
    /// like it can. What ends a sentence is not part of it: a full stop after
    /// an address belongs to the prose, and a reader who copies the row gets
    /// an address that still resolves.
    pub(super) fn wrote_address(
        &mut self,
        address: &str,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let ends = address.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"']);

        // A scheme and nothing after it is a word somebody wrote, not somewhere
        // anybody can go, and `www.` needs a host to start with a letter or a
        // digit.
        let target = if let Some(host) = ends.strip_prefix(WWW) {
            host.starts_with(|character: char| character.is_ascii_alphanumeric())
                .then(|| format!("https://{ends}"))
        } else {
            (spelled(ends) == Spelled::Whole && !SCHEMES.contains(&ends)).then(|| ends.to_owned())
        };
        let Some(target) = target else {
            self.say(address, say);
            self.started = true;
            self.previous = address.chars().next_back().unwrap_or('/');
            return;
        };

        self.pay(say);
        self.drew(ends, &target, Some(&target), say);
        self.started = true;
        self.previous = ends.chars().next_back().unwrap_or('/');
        self.say(&address[ends.len()..], say);
    }

    /// Hands on a link that arrived whole.
    ///
    /// The words wear the link's own slot and carry the address, and what
    /// follows them is the quiet address [`Markdown::drew`] adds: the sentence
    /// the answer wrote goes on reading as a sentence, and a reader whose
    /// terminal opens no links still sees where it goes. A link with no words
    /// is its address, since that is the one thing there is to show. A link to
    /// a path is handed on as the file it names in the checkout, which is the
    /// one form of it a terminal can open; its words stay what the answer
    /// wrote. Nothing here is ever written as anything but text: an address is
    /// bytes a model chose, and the rule the reader opens with is that none of
    /// them leaves as an instruction.
    pub(super) fn wrote_link(
        &mut self,
        link: &Link,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.pay(say);

        let target = target(&link.target);
        let label = escapes::decoded(&link.label);
        let words = if label.is_empty() { target } else { &label };

        let file = self.files.as_ref().and_then(|files| files.address(target));
        let address = file.as_deref().unwrap_or(target);
        self.drew(words, target, (!address.is_empty()).then_some(address), say);
        self.previous = words.chars().next_back().unwrap_or(')');
    }

    /// Hands on a reference's `words`, carrying the `address` its definition
    /// gave, the way a link written out is.
    pub(super) fn wrote_reference_to(
        &mut self,
        words: &str,
        address: &str,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        self.pay(say);
        let words = escapes::decoded(words);
        let words = if words.is_empty() { address } else { &words };
        self.drew(words, address, Some(address), say);
        self.previous = words.chars().next_back().unwrap_or(']');
    }
}

/// Whether `words` are `target` as a person writes it: the same once each has
/// dropped its scheme or `mailto:`.
fn own(words: &str, target: &str) -> bool {
    let bare = |text: &'_ str| -> String {
        let text = SCHEMES
            .iter()
            .find_map(|scheme| text.strip_prefix(scheme))
            .or_else(|| text.strip_prefix(MAILTO))
            .unwrap_or(text);
        text.to_owned()
    };
    bare(words) == bare(target)
}

/// The quiet address of `target`, and whether it was cut short.
///
/// Without its scheme, and whole where that is [`QUIET`] columns or fewer.
/// Longer, an address with a host keeps the host and its first segment, then
/// the ellipsis where more follows; one without, a path or an e-mail address,
/// keeps what fits. Either is cut at [`QUIET`] columns where it is still too
/// wide, a wide character that would straddle the bound left out.
fn quiet(target: &str, glyphs: Glyphs) -> (String, bool) {
    let hosted = target.split_once("://").map(|(_, rest)| rest);
    let written = hosted.unwrap_or_else(|| target.strip_prefix(MAILTO).unwrap_or(target));
    if width::columns(written) <= QUIET {
        return (written.to_owned(), false);
    }

    let (mut kept, more) = match hosted {
        Some(rest) => {
            let parts = |text: &'_ str| text.find(['/', '?', '#']).unwrap_or(text.len());
            let (host, path) = rest.split_at(parts(rest));
            let segment = path
                .strip_prefix('/')
                .map_or("", |path| path.get(..parts(path)).unwrap_or(path));
            let after = path
                .get(
                    segment
                        .len()
                        .saturating_add(usize::from(path.starts_with('/')))..,
                )
                .unwrap_or_default();
            let more = !after.is_empty() && after != "/";
            let mut kept = host.to_owned();
            if !segment.is_empty() {
                kept.push('/');
                kept.push_str(segment);
            }
            if more {
                kept.push('/');
            }
            (kept, more)
        }
        None => (written.to_owned(), true),
    };

    if width::columns(&kept) <= QUIET {
        if more {
            kept.push_str(glyphs.ellipsis());
        }
        return (kept, more);
    }
    (
        format!("{}{}", width::clip(&kept, QUIET), glyphs.ellipsis()),
        true,
    )
}

/// Writes a link back as the characters it was written with.
///
/// Exactly them, in order, so a bracket that was never a link costs the reader
/// nothing at all.
pub(super) fn spill(link: &Link, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
    say(Slot::Plain, "[", None);
    if !link.label.is_empty() {
        say(Slot::Plain, &link.label, None);
    }
    if link.part != Part::Label {
        say(Slot::Plain, "]", None);
    }
    let opening = match link.part {
        Part::Label | Part::Between => return,
        Part::Target => "(",
        Part::Second => "[",
    };
    say(Slot::Plain, opening, None);
    if !link.target.is_empty() {
        say(Slot::Plain, &link.target, None);
    }
}

/// The address out of what stood between the brackets.
///
/// Markdown lets a title follow the address, and lets the address itself be
/// wrapped in angle brackets. Neither is the destination, and the destination
/// is the whole of what is worth a reader's columns.
pub(super) fn target(between: &str) -> &str {
    between
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(|character| character == '<' || character == '>')
}
