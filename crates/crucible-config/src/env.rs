//! crucible's own environment namespace.
//!
//! Every environment variable crucible reads to configure *itself* begins with
//! [`NAMESPACE`], and nothing else crucible reads does. The variables it reads
//! that are outside the namespace — `ANTHROPIC_API_KEY`, `HOME`, `NO_COLOR` —
//! belong to a vendor or to the operating system, and crucible only ever reads
//! those, never decides what they mean.
//!
//! The `env` block in a configuration file is the environment: what it sets is
//! there for the processes the bash tool starts, and crucible's own settings
//! live in it under this prefix.
//!
//! ```json
//! { "env": { "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12" } }
//! ```
//!
//! The division earns its keep in workspace files. Either project filename can
//! be committed, so an arbitrary variable is refused in both. A variable in
//! this namespace is not arbitrary: it is a knob crucible declares and whose
//! meaning crucible fixes, so a project can set one for everybody who clones it
//! and that is still not a way to ship somebody's key. The prefix is what makes
//! "this is a crucible setting" checkable rather than a matter of trust.
//!
//! It is also the one block of a document whose values are nobody's business
//! but the user's, which is why [`Redacted`] lives here beside it.

use std::fmt;

use serde_json::Value;

/// What stands in a printed document where a value must not.
const REDACTED: &str = "<redacted>";

/// The prefix on every environment variable crucible reads for itself.
///
/// Written once. Anything that reads a variable of crucible's own, or decides
/// whether a name is one, builds it from this rather than spelling it again.
pub(crate) const NAMESPACE: &str = "CRUCIBLE_CODE_";

/// Whether an environment variable is one of crucible's own.
pub(crate) fn ours(name: &str) -> bool {
    name.starts_with(NAMESPACE)
}

/// Whether crucible has already read this variable by the time it opens a file.
///
/// A variable in this list is one of crucible's own and is still refused in
/// every layer, because it is read *to find* the files: by the time one of them
/// could set it, the answer has been used. Refusing it is what stops a setting
/// that looks applied from doing nothing.
///
/// One name so far. When a second arrives it joins this list, which is the only
/// place the question is asked.
pub(crate) fn too_late(name: &str) -> bool {
    name == crate::home::HOME
}

/// A document, as `Debug` may write it: every `env` value replaced.
///
/// The block is the environment, so what a user puts in it is whatever the
/// commands they run need — a token among them, in the two layers that are
/// allowed to hold one. Every type here that holds a parsed document formats
/// itself through this rather than deriving `Debug`, because a derive is how a
/// value reaches a log line, an error, or a panic payload without anybody
/// having decided that it should.
///
/// The names stay. A name is what makes a diagnostic worth reading, and it is
/// already what the refusals in [`crate::error`] are allowed to say.
pub(crate) struct Redacted<'a>(pub(crate) &'a Value);

impl fmt::Debug for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Copied rather than filtered on the way past, because redaction is a
        // property of what is printed and not of what is held: the commands
        // crucible starts are still handed the values the user wrote. A
        // document is kilobytes and nothing here is on the turn path.
        let mut shown = self.0.clone();
        if let Some(vars) = shown.get_mut("env").and_then(Value::as_object_mut) {
            for value in vars.values_mut() {
                *value = Value::String(REDACTED.to_owned());
            }
        }
        shown.fmt(f)
    }
}
