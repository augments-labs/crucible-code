//! What the layers together say about permission.
//!
//! Two halves that arrive by different routes and meet here. The rules were
//! read into the engine's own types while each file was still one file, so what
//! is left of them is a [`Rules`]. Everything else in the block is a string the
//! merged document still holds, read the way any other setting is.
//!
//! [`Rules`]: crucible_core::Rules

use crucible_core::{Mode, Permission};
use serde_json::Value;

use super::Settings;

impl Settings {
    /// Which mode to start in, when the command line does not say.
    ///
    /// Never from either project file, which the document refused while it was
    /// still one file. Nothing is re-checked here: what a layer may say is
    /// decided where the file is open and the position can be pointed at.
    #[must_use]
    pub fn mode(&self) -> Option<Mode> {
        read(self.permissions("mode")?.as_str()?)
    }

    /// Directories outside the working directory that tools may reach.
    ///
    /// Absolute, and already refused by name and position if one is not. The
    /// workspace resolves them and refuses them a second time, because a
    /// workspace that trusted its caller would be a workspace whose containment
    /// check depended on who built it.
    pub fn extra_directories(&self) -> impl Iterator<Item = &str> {
        self.permissions("extraDirectories")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
    }

    /// The permission engine these files describe, in the mode decided above.
    ///
    /// The mode is an argument rather than a lookup: the files are one layer of
    /// four and the command line is nearer than all of them, so what to start
    /// in is the wiring's answer and not this crate's.
    #[must_use]
    pub fn permission(&self, mode: Mode) -> Permission {
        // Cloned because `Settings` outlives the engine it describes — the
        // wiring keeps reading providers and output from it. It holds what
        // configuration stated and does not grow with the session.
        Permission::with(mode, self.rules.clone())
    }

    /// One key out of the `permissions` block.
    fn permissions(&self, key: &str) -> Option<&Value> {
        self.value.get("permissions")?.get(key)
    }
}

/// Reads one of [`shape::MODE`](crate::shape::MODE).
///
/// `None` for anything else, which the shape refused before this could be
/// reached. The test below is what keeps that true as the set changes.
fn read(found: &str) -> Option<Mode> {
    match found {
        "ask" => Some(Mode::Ask),
        "allowEdits" => Some(Mode::AllowEdits),
        "fullAccess" => Some(Mode::FullAccess),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
