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

use super::Sensitivity;

mod matches;
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
#[derive(Debug, Default)]
pub struct Rules {
    deny: Vec<Rule>,
    ask: Vec<Rule>,
    allow: Vec<Rule>,
}

/// One rule: a tool, and what it may act on.
#[derive(Debug)]
struct Rule {
    /// Matched against the call's name exactly. A rule is about one tool.
    tool: Box<str>,
    pattern: Pattern,
}

/// What a rule says about the thing a call acts on.
#[derive(Debug)]
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

    /// Whether any rule at all was read, which is what decides if the
    /// documented default arm is the only thing in play.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.ask.is_empty() && self.allow.is_empty()
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
