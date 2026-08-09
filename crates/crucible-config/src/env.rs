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
//! The division earns its keep in the layer that travels. `.crucible/config.json`
//! is checked in, so a value written there reaches everyone who clones the
//! repository — which is why an arbitrary variable is refused in it. A variable
//! in this namespace is not arbitrary: it is a knob crucible declares and whose
//! meaning crucible fixes, so a project can set one for everybody who clones it
//! and that is still not a way to ship somebody's key. The prefix is what makes
//! "this is a crucible setting" checkable rather than a matter of trust.

/// The prefix on every environment variable crucible reads for itself.
///
/// Written once. Anything that reads a variable of crucible's own, or decides
/// whether a name is one, builds it from this rather than spelling it again.
pub const NAMESPACE: &str = "CRUCIBLE_CODE_";

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
