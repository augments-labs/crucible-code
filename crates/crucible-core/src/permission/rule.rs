//! Standing statements about what calls may do.
//!
//! A rule is not a decision about one call — it is a statement made before any
//! call arrived, and it is read from configuration into these types at the
//! boundary. Three kinds, held apart rather than ordered together: the kind
//! decides which wins, never how specific the pattern is.
//!
//! That is the whole of it. A `deny` list read on its own is the list of things
//! that cannot happen, and nothing in another list can qualify it. The price is
//! that "deny every `git` except `git status`" cannot be said; the return is
//! that anyone can read a deny list and know what it protects.

use globset::GlobMatcher;

use crate::tool::ToolCall;

use super::{Sensitivity, Target};

mod matches;
pub(super) mod mint;
mod parse;
#[cfg(test)]
mod tests;

/// What is to happen to a call, before anyone has been asked.
///
/// Not a [`Verdict`](super::Verdict): `Ask` means "put it to the user", which
/// is the absence of a decision rather than one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Run it without asking.
    Allow,
    /// Put it to the user.
    Ask,
    /// Refuse it.
    Deny,
}

/// Why a rule could not be read.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// The text is not shaped like a rule at all.
    #[error("{text} is not a rule; a rule names a tool and what it may act on, like read(src/**)")]
    Shape {
        /// The text as it was written.
        text: Box<str>,
    },

    /// The tool part is empty, so the rule is about nothing.
    #[error("{text} names no tool, so nothing would ever match it")]
    NoTool {
        /// The text as it was written.
        text: Box<str>,
    },

    /// The pattern is not a glob.
    #[error("{text} does not hold a usable pattern: {problem}")]
    Pattern {
        /// The text as it was written.
        text: Box<str>,
        /// What the glob compiler said about it.
        problem: Box<str>,
    },
}

/// Every rule, held by kind.
#[derive(Debug, Default, Clone)]
pub struct Rules {
    deny: Vec<Rule>,
    ask: Vec<Rule>,
    allow: Vec<Rule>,
}

/// `*` where a tool goes: every tool, the reading `*` has in the other
/// position too.
const EVERY: &str = "*";

/// One rule: a tool, and what it may act on.
#[derive(Debug, Clone)]
struct Rule {
    /// The tool this is about, as it was written. `*` is all of them.
    tool: Box<str>,
    pattern: Pattern,
}

impl Rule {
    /// Whether this rule is about a call to `tool`.
    ///
    /// Case is not part of the answer. Every tool is named in lower case, so a
    /// capital can only be somebody spelling one of them the way a sentence
    /// would — and a rule accepted, written down and then matched against
    /// nothing is the failure this mechanism cannot survive.
    fn about(&self, tool: &str) -> bool {
        &*self.tool == EVERY || self.tool.eq_ignore_ascii_case(tool)
    }
}

/// What a rule says about the thing a call acts on.
#[derive(Debug, Clone)]
enum Pattern {
    /// `*`, or the tool named on its own: everything that tool could do.
    ///
    /// The one pattern that needs no parse of what is about to happen. A rule
    /// saying *everything* has nothing to be misled about, so it covers a
    /// command nobody could read and a path that did not resolve alike.
    Blanket,

    /// A glob, compiled both ways it will be needed.
    Glob {
        /// `*` stops at a path separator, so `src/*` names the files in `src`
        /// and `src/**` names what is below it.
        path: GlobMatcher,
        /// `*` spans everything, because a command is not a path and `git *`
        /// has to reach `git add src/main.rs`.
        text: GlobMatcher,
        /// Whether the pattern was written as an absolute path, which decides
        /// which spelling of a target it is matched against.
        absolute: bool,
    },
}

/// The patterns a call must refuse for itself, whatever it reaches.
///
/// Empty for almost every call, which is what the walk checks first: rules
/// nobody wrote cost nothing to honour.
#[derive(Debug, Default, Clone)]
pub(super) struct Denials(Box<[Pattern]>);

impl Denials {
    /// Whether one of them names this path.
    pub(super) fn names(&self, target: &Target) -> bool {
        self.0.iter().any(|pattern| pattern.covers_path(target))
    }

    /// Whether there is nothing here to check.
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Rules {
    /// No rules at all, which is what an empty configuration means.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads one rule of one kind.
    ///
    /// # Errors
    ///
    /// [`RuleError`] naming the text that could not be read. The caller knows
    /// which file and which position it came from; this does not.
    pub fn add(&mut self, kind: Disposition, text: &str) -> Result<(), RuleError> {
        let rule = parse::rule(text)?;
        match kind {
            Disposition::Allow => self.allow.push(rule),
            Disposition::Ask => self.ask.push(rule),
            Disposition::Deny => self.deny.push(rule),
        }
        Ok(())
    }

    /// Takes on every rule another set holds.
    ///
    /// Concatenation is the only merge a set of rules can have. Configuration
    /// arrives in layers, and a nearer one that *replaced* a farther one would
    /// let a checked-out repository discard the `deny` somebody wrote at home.
    /// Appending is safe in the other direction precisely because the kind
    /// decides which rule wins: another `allow` cannot qualify a `deny`, so
    /// what a layer can add to is what may happen and never what may not.
    pub fn absorb(&mut self, other: Self) {
        self.deny.extend(other.deny);
        self.ask.extend(other.ask);
        self.allow.extend(other.allow);
    }

    /// Whether any rule at all was read, which is what decides if the
    /// documented default arm is the only thing in play.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.ask.is_empty() && self.allow.is_empty()
    }

    /// Every rule about `tool` that ends a read.
    ///
    /// Handed to a tool along with the proof it may run, for the calls that
    /// reach more files than the one they were decided about: a search is
    /// settled once, about the directory it walks, so a rule written about a
    /// file below that directory has no other moment to be honoured in.
    ///
    /// `ask` rules come along with the `deny` ones because a read is never put
    /// to the user — an `ask` about one has already become a refusal by the
    /// time any tool sees it, and a walk honouring only half the rules written
    /// about it would leave the other half looking like protection.
    pub(super) fn denials(&self, tool: &str) -> Denials {
        Denials(
            self.deny
                .iter()
                .chain(self.ask.iter())
                .filter(|rule| rule.about(tool))
                .map(|rule| rule.pattern.clone())
                .collect(),
        )
    }

    /// What the rules say about this call, if they say anything.
    ///
    /// Kinds are tried in the order `deny`, `ask`, `allow` and the first to
    /// speak wins, whatever the patterns look like. `deny` and `ask` fire when
    /// *any* part of a call matches; `allow` only when every part does, so a
    /// command with one constituent nobody wrote a rule about still falls
    /// through to be asked.
    pub(super) fn stated(&self, call: &ToolCall, sensitivity: &Sensitivity) -> Option<Disposition> {
        if matches::any(&self.deny, call, sensitivity) {
            Some(Disposition::Deny)
        } else if matches::any(&self.ask, call, sensitivity) {
            Some(Disposition::Ask)
        } else if matches::all(&self.allow, call, sensitivity) {
            Some(Disposition::Allow)
        } else {
            None
        }
    }
}
