//! The keys crucible was given, written down where only this user can read them.
//!
//! An API key reaches crucible two ways. An environment variable is the one
//! that has always worked, and it is read fresh every launch by whoever set it.
//! The other is `/login`, which takes a key once and writes it here so the next
//! launch does not have to ask again — and that is the whole of what this crate
//! is for. It holds no flow, no token and no clock: a key does not expire, so
//! there is nothing here that renews, nothing that has to mutate itself in the
//! middle of a request, and nothing that reaches the network at all.
//!
//! Two rules shape every line of it. **A secret this program wrote down is this
//! program's fault if it leaks**, so the value inside the file is never returned
//! as a string — it comes back as [`crucible_core::ApiKey`], which can be
//! applied to a request and not read. And **a store that cannot be read never
//! costs somebody a session**: absent, truncated, or written by a version that
//! does not exist yet all resolve to *nobody is logged in*, said once in
//! [`Keys::trouble`], and the launch continues. That is why [`Store::read`] has
//! no `Result` to propagate.
//!
//! Where the file lives is not decided here. [`Store::in_home`] is handed the
//! directory, because `crucible_config::Home` is the one place that answers
//! where anything is.

mod error;
mod store;

pub use error::AuthError;
pub use store::{Keys, Store};
