//! What every turn is asked under, assembled from what this run knows.
//!
//! The instructions themselves are [`crucible_core::SystemPrompt`], because
//! what they say is a fact about crucible and not about this binary. What is
//! here is the other half: the four things only the composition root can
//! answer — which layers the reader wrote, where the workspace is, what is
//! answering, and which tools were actually registered — put onto that value
//! and rendered.
//!
//! It is built again for every turn rather than once at startup. Three of those
//! four change while a session runs: `/model` and `/effort` move the identity,
//! `tool_search` grows the roster, and a command left running ends between one
//! turn and the next. A prompt written once would go on describing the session
//! the first turn was taken in.
//!
//! The tools are named here and described nowhere. Their schemas travel with
//! every request already, and the list comes off the registry rather than out
//! of a constant, so a tool this build stops offering cannot go on being
//! advertised by a sentence nobody thought to edit.

use std::fmt::Write as _;

use crucible_config::Settings;
use crucible_core::{Effort, SystemPrompt, Workspace};
use crucible_tools::Ended;

use crate::cli::draw::spelled;

/// Everything about this run that the instructions themselves cannot hold.
///
/// A struct rather than six arguments: three of these are strings or slices
/// that would read identically at a call site and mean entirely different
/// things, which is the argument list nobody can check by eye.
pub(crate) struct Standing<'a> {
    /// The layers, for the tone and for anything the reader would rather crucible
    /// said instead.
    pub(crate) settings: &'a Settings,
    /// The model, as the provider spells it. Empty where nothing has chosen one.
    pub(crate) model: &'a str,
    /// The rung, where one was named.
    pub(crate) effort: Option<Effort>,
    /// Where every tool path is taken from.
    pub(crate) workspace: &'a Workspace,
    /// The tools this session is advertising, off the registry that holds them.
    pub(crate) tools: Vec<String>,
}

/// The whole of what a turn is asked under.
///
/// An unnamed model gets no identity at all — there is nothing true to say yet,
/// and a sentence about nothing is worse than silence.
pub(crate) fn under(standing: Standing<'_>) -> String {
    let Standing {
        settings,
        model,
        effort,
        workspace,
        tools,
    } = standing;

    SystemPrompt {
        tone: settings.tone(),
        custom: settings.custom_prompt().map(str::to_owned),
        append: settings.appended_prompt().map(str::to_owned),
        tools,
        root: Some(workspace.root().to_path_buf()),
        identity: (!model.is_empty()).then(|| crucible_core::Identity {
            model: model.to_owned(),
            effort,
        }),
        ..SystemPrompt::default()
    }
    .text()
}

/// What has ended, in the words the model is told it in.
///
/// `None` where nothing has, which is almost every turn: a note about nothing is
/// a sentence the model has to read past to find the ones that mean something.
///
/// One wording for three ways of arriving. Under a turn being started, into a
/// turn already running, or — where nobody is at the keyboard and nothing is
/// running — as the turn itself; see `converse::typing`. A model that read one
/// sentence about a build that fell over and a different one about the same
/// build depending on who happened to get there first would be reading about
/// two builds. So it says "have ended" rather than naming a boundary: which
/// turn it lands in is not a fact about the command.
///
/// It opens by saying who is speaking because two of those three ways record it
/// as a message from the developer — that is the only channel a turn already
/// running has — and a model that took this for something a person typed would
/// answer the person about it.
pub(crate) fn said(ended: &[Ended]) -> Option<String> {
    if ended.is_empty() {
        return None;
    }

    let mut said = String::from(
        "crucible, not the developer: commands you left running have ended. They are \
         gone; nothing is waiting on them, and starting one again is a new call:",
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
