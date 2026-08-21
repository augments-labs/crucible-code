//! What every turn is asked under.
//!
//! The instructions are written for this harness, and beside them go the two
//! things the session knows and the model cannot: where it is standing, and
//! what it is. Both are read off the runner rather than written down once,
//! because `/model` and `/effort` change the second of them mid-session.
//!
//! It says how to work and how to answer, and leaves what the tools do to the
//! tools' own schemas — a system prompt that also describes each tool is a
//! second place for that description to go stale.

use std::fmt::Write as _;

use crucible_core::{Effort, Workspace};
use crucible_tools::Ended;

use crate::cli::draw::spelled;

/// The standing instructions every turn carries.
const SYSTEM: &str = "\
You are crucible, a coding agent working in a terminal beside a developer.

Work from what the code says rather than what it probably says: read a file \
before changing it, and search before concluding something is not there. \
Prefer the smallest change that finishes the job, and match the conventions of \
the file you are editing rather than your own habits.

Answer in plain prose, briefly. The developer is reading a terminal: put the \
conclusion first, skip the preamble, and do not read a file's contents back \
after editing it — say what changed and why.

Ask when the answer would change what you build. Otherwise decide, say which \
way you decided, and carry on.";

/// How the identity reads where nobody named a rung.
///
/// The field is left off the request entirely in that state, so what answers is
/// whatever the vendor does by default for that model — which is a fact about
/// the vendor, and not a rung this program picked.
const UNSAID: &str = "the vendor's own default effort";

/// The whole of what a turn is asked under: the instructions, the root, and
/// what is answering.
///
/// The root is here because every tool takes paths relative to it, and a model
/// that has to guess spends its first tool call finding out.
///
/// The identity is here because a model has no way to look at either half of
/// it. Its own name it would answer from training, which for a name is a guess
/// that reads like a fact and is wrong the moment a session switches models.
/// The rung it was asked to think at is a field on a request it never sees.
/// Both are what somebody asking what they are talking to is asking about, so
/// both are said rather than left to be invented.
///
/// An unnamed model is a session that cannot take a turn, and it gets no
/// identity at all — there is nothing true to say yet, and a sentence about
/// nothing is worse than silence.
pub(crate) fn under(
    model: &str,
    effort: Option<Effort>,
    workspace: &Workspace,
    ended: &[Ended],
) -> String {
    let root = workspace.root().display();
    let standing =
        format!("{SYSTEM}\n\nThe workspace root is {root}. Every tool path is relative to it.");

    // At the top of the turn rather than pushed into the last one: a turn already
    // in flight has nowhere to put a new fact, and a command that fell over is
    // something the model needs before it answers rather than after.
    let standing = match said(ended) {
        Some(said) => format!("{standing}\n\n{said}"),
        None => standing,
    };

    if model.is_empty() {
        return standing;
    }

    let rung = effort.map_or_else(
        || UNSAID.to_owned(),
        |effort| format!("{} effort", effort.as_str()),
    );

    format!(
        "{standing}\n\nYou are {model}, asked at {rung}. That is what to say when \
         somebody asks which model they are talking to or how hard you are \
         thinking. Neither is something you can find out for yourself, and both \
         can change partway through a session."
    )
}

/// What ended since the last turn, in the words the model is told it in.
///
/// `None` where nothing did, which is almost every turn: a note about nothing is
/// a sentence the model has to read past to find the ones that mean something.
///
/// One wording for two ways of arriving. Where a turn is already being started
/// it goes under that turn, and where none is, it *is* the turn — see
/// [`super::converse::typing`]. A model that read one sentence about a build
/// that fell over and a different one about the same build depending on who
/// happened to type first would be reading about two builds.
pub(crate) fn said(ended: &[Ended]) -> Option<String> {
    if ended.is_empty() {
        return None;
    }

    let mut said = String::from(
        "Commands you left running have ended since your last turn. They are gone; \
         nothing is waiting on them, and starting one again is a new call:",
    );

    for one in ended {
        let how = match one.code {
            Some(0) => "finished".to_owned(),
            Some(code) => format!("failed with exit status {code}"),
            None => "was killed".to_owned(),
        };

        let _ = write!(
            said,
            "\n- #{} {} — {how} after printing {} lines.",
            one.number,
            spelled(one.tool, &one.called),
            one.lines
        );
    }

    Some(said)
}

#[cfg(test)]
mod tests;
