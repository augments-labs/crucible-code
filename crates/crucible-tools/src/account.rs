//! What a call says it is for, read out of the call.
//!
//! Two arguments, spelled `description` and `explanation`, in every schema that
//! invites them — and that sameness is the whole design rather than a
//! coincidence worth tidying later. The panel a call waits behind draws these
//! the same way whichever tool is about to run, so a tool free to name the
//! fields its own way would be a panel that is uniform by luck. Two names, and
//! one place that reads them.
//!
//! They stay two rather than becoming one because they are shown at different
//! times: the line is on the panel from the moment it opens, and the paragraphs
//! are behind a key somebody has to press. Joining them would put a page of
//! prose where a caption goes, and asking a model to fit both jobs into one
//! string is asking it to guess where the panel will cut.
//!
//! Which is why this is a function of the crate rather than a method on the
//! trait: a method would hand every tool the freedom the paragraph above says
//! nothing may have. What a tool decides is whether to invite an account at
//! all, and it decides that where it belongs — in the schema it publishes.
//!
//! Nothing is validated on the way through, for the reason [`crate::summary`]
//! is not: what comes back is shown to a person and never acted on, and a call
//! whose arguments do not hold up is refused a moment later by the tool that
//! owns them.

use crucible_core::{Account, ToolArgs};

use crate::args::Args;
use crate::schema::{Field, Shape};

/// The argument a call accounts for itself in.
pub(crate) const SAID: &str = "description";

/// The argument it explains itself at length in.
pub(crate) const TOLD: &str = "explanation";

/// The argument a call asks to be left running in.
pub(crate) const LEFT: &str = "background";

/// The two argument declarations every schema that invites an account carries,
/// spelled here so the names above and the prose around them cannot drift
/// between tools.
///
/// `under` is what the panel draws the line beneath — a path, a command.
/// `lead` opens the longer account — "What the change does" — and `tail` is
/// the one sentence that differs by what the tool can do wrong.
pub(crate) fn fields(under: &str, lead: &str, tail: &str) -> [Field; 2] {
    [
        Field {
            name: SAID,
            about: format!(
                "One short line saying what this call is for, shown under the {under} to the \
                 person deciding whether to allow it. It is cut to one row rather than folded, so \
                 lead with the point and keep it near fifty characters."
            ),
            needed: false,
            shape: Shape::Text,
        },
        Field {
            name: TOLD,
            about: format!(
                "{lead}, why you want it, and what it costs if it is wrong. One string per \
                 paragraph, opened with a key by the same person, so write it for them rather \
                 than for yourself and do not restate the {under}: it is already on screen above \
                 this. {tail}"
            ),
            needed: false,
            shape: Shape::List {
                of: Box::new(Shape::Text),
                fewest: None,
                most: None,
            },
        },
    ]
}

/// The name a rejection would carry if one could get out of here.
///
/// None can: every failure below is dropped, because the reading is for a
/// question somebody is about to be asked and a question that says what it can
/// beats one that says nothing. The tool that owns these arguments reports the
/// same failure properly, a moment later, to the model that can act on it.
const NOBODY: &str = "";

/// What this call said it was for, or nothing where it said nothing.
///
/// Optional whatever the schema says, because the schema is not what arrived.
/// Each field is read on its own and dropped on its own, so a call that got one
/// of them wrong still shows the other: the two answer different questions and
/// nothing here has to hold them to a bargain the schema never made.
#[must_use]
pub fn of(args: &ToolArgs) -> Account {
    let Ok(args) = Args::parse(NOBODY, args) else {
        return Account::none();
    };

    let said = args.optional_text(SAID).ok().flatten().unwrap_or_default();
    let told = args.texts(TOLD).unwrap_or_default();

    Account::explained(said, told)
}

/// Whether the call asked for its command to be left running.
///
/// Read here for the reason the account above is: the panel where somebody decides
/// whether the command may run has no provider to ask and no tool in reach, and
/// what it needs to say is a fact about the arguments. A command that will outlive
/// the turn is a different thing to consent to from one that will not, and a panel
/// that did not say so would be asking about the wrong thing.
///
/// `false` for every tool that invites no such argument, and for a call whose
/// arguments cannot be read at all — that one is refused by the tool a moment
/// later, and a panel is not the place to learn it.
#[must_use]
pub fn backgrounded(args: &ToolArgs) -> bool {
    Args::parse("", args).is_ok_and(|args| args.flag(LEFT, false).unwrap_or(false))
}

#[cfg(test)]
mod tests;
