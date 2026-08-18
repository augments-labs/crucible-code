//! Testing a call against a list of rules of one kind.
//!
//! Two questions, because `deny` and `allow` are not mirror images. A denial
//! only has to catch one part of a call to be worth acting on; an allowance has
//! to account for all of it, or the part nobody wrote a rule about runs
//! unwatched alongside the part somebody did.

use crate::tool::ToolCall;

use super::super::sensitivity::Wanted;
use super::super::{Command, Host, Sensitivity, Target};
use super::{Pattern, Rule};

/// Whether any rule covers some part of this call.
pub(super) fn any(rules: &[Rule], call: &ToolCall, sensitivity: &Sensitivity) -> bool {
    match sensitivity {
        Sensitivity::ReadOnly { target } | Sensitivity::MutatesFile { target } => {
            covered(rules, call, target)
        }
        Sensitivity::SpawnsProcess {
            command: Command::Understood { parts, .. },
        } => parts.iter().any(|part| runs(rules, call, part)),
        Sensitivity::ReachesNetwork {
            host: Host::Named { host, .. },
        } => runs(rules, call, host),

        // Spelled together rather than merged away: both are a call nobody
        // could read, and a blanket is the only rule that can speak about
        // something with no subject to match. Written with `|` so a third
        // unreadable shape still stops the build.
        Sensitivity::SpawnsProcess {
            command: Command::Opaque(_),
        }
        | Sensitivity::ReachesNetwork {
            host: Host::Opaque(_),
        } => blanketed(rules, call),
    }
}

/// Whether the rules together cover the whole of this call.
pub(super) fn all(rules: &[Rule], call: &ToolCall, sensitivity: &Sensitivity) -> bool {
    match sensitivity {
        Sensitivity::ReadOnly { target } | Sensitivity::MutatesFile { target } => {
            covered(rules, call, target)
        }
        Sensitivity::SpawnsProcess {
            command: Command::Understood { parts, .. },
        } => {
            // A command that decomposed into nothing is not a command every
            // rule covers, it is one nobody can say anything about.
            !parts.is_empty() && parts.iter().all(|part| runs(rules, call, part))
        }
        // One host, so `any` and `all` are the same question — unlike a command
        // line, which decomposes into parts a rule can cover only some of.
        Sensitivity::ReachesNetwork {
            host: Host::Named { host, .. },
        } => runs(rules, call, host),

        Sensitivity::SpawnsProcess {
            command: Command::Opaque(_),
        }
        | Sensitivity::ReachesNetwork {
            host: Host::Opaque(_),
        } => blanketed(rules, call),
    }
}

/// Whether a rule for this tool names this path.
fn covered(rules: &[Rule], call: &ToolCall, target: &Target) -> bool {
    for_tool(rules, call).any(|pattern| pattern.covers_path(target))
}

/// Whether a rule for this tool names this simple command.
fn runs(rules: &[Rule], call: &ToolCall, part: &str) -> bool {
    for_tool(rules, call).any(|pattern| pattern.covers_text(part))
}

/// Whether a rule for this tool is a blanket — the only kind that can speak
/// about something nobody could read.
fn blanketed(rules: &[Rule], call: &ToolCall) -> bool {
    for_tool(rules, call).any(|pattern| matches!(pattern, Pattern::Blanket))
}

/// The patterns of every rule written about the tool this call names.
fn for_tool<'a>(rules: &'a [Rule], call: &'a ToolCall) -> impl Iterator<Item = &'a Pattern> {
    rules
        .iter()
        .filter(|rule| rule.about(&call.name))
        .map(|rule| &rule.pattern)
}

impl Pattern {
    /// Whether this pattern names the path a call acts on.
    pub(super) fn covers_path(&self, target: &Target) -> bool {
        match self {
            Self::Blanket => true,

            // An absolute pattern is about the resolved path; a relative one
            // is about the path below the workspace root, which is the
            // spelling somebody writing `src/**` has in mind. A file in a
            // directory the workspace merely reaches has no second spelling,
            // so only an absolute pattern reaches it — which is the honest
            // answer, `src/**` meaning nothing there.
            Self::Glob {
                path,
                absolute: true,
                ..
            } => target.absolute().is_some_and(|at| path.is_match(at)),

            Self::Glob {
                path,
                absolute: false,
                ..
            } => target.below_root().is_some_and(|at| path.is_match(at)),
        }
    }

    /// Which spelling of a target [`Pattern::covers_path`] will ask for.
    ///
    /// Directly beside its only reason for existing, matching on the same
    /// thing in the same order, because the two are one statement: a walk
    /// spells each file it reaches only the ways this claims, so an arm above
    /// that read a spelling this did not name would be a rule that quietly
    /// stopped matching. A new variant stops the build in both.
    pub(super) fn reads(&self) -> Wanted {
        match self {
            Self::Blanket => Wanted::NEITHER,
            Self::Glob { absolute: true, .. } => Wanted::ABSOLUTE,
            Self::Glob {
                absolute: false, ..
            } => Wanted::BELOW_ROOT,
        }
    }

    /// Whether this pattern names one simple command.
    fn covers_text(&self, part: &str) -> bool {
        match self {
            Self::Blanket => true,
            Self::Glob { text, .. } => text.is_match(part),
        }
    }
}
