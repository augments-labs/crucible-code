//! What a call says it is for, read out of the call.
//!
//! One argument, spelled `description`, in every schema that invites one — and
//! that sameness is the whole design rather than a coincidence worth tidying
//! later. The panel a call waits behind draws this the same way whichever tool
//! is about to run, so a tool free to name the field its own way would be a
//! panel that is uniform by luck. One name, and one place that reads it.
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

/// The argument a call accounts for itself in.
const FIELD: &str = "description";

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
#[must_use]
pub fn of(args: &ToolArgs) -> Account {
    let said = Args::parse(NOBODY, args)
        .ok()
        .and_then(|args| args.optional_text(FIELD).ok().flatten().map(str::to_owned));

    Account::new(said.unwrap_or_default())
}

#[cfg(test)]
mod tests;
