//! The keys crucible was given, written down where only this user can read them.
//!
//! An API key reaches crucible two ways. An environment variable is the one
//! that has always worked, and it is read fresh every launch by whoever set it.
//! The other is `/login`, which takes a key once and writes it here so the next
//! launch does not have to ask again. The protected document can also hold the
//! renewable secret state used by account credentials. Authorization methods
//! meet the binary through the provider-neutral [`SubscriptionLogin`] trait;
//! request renewal remains separate from this storage boundary.
//!
//! Two rules shape every line of it. **A secret this program wrote down is this
//! program's fault if it leaks**, so the value inside the file is never returned
//! as a string — it comes back as [`crucible_core::ApiKey`], which can be
//! applied to a request and not read. And **a store that cannot be read never
//! costs somebody a session**: absent, truncated, or written by a version that
//! does not exist yet all resolve to *no stored credential is available*, said
//! once in [`StoredCredentials::trouble`], and the launch continues. That is
//! why [`Store::read`] has no `Result` to propagate.
//!
//! `crucible-privacy` owns the platform mechanism that makes the directory,
//! store, partial and lock owner-only. Auth owns which files exist and what
//! they mean; it does not carry a second implementation of Unix modes or
//! Windows access control lists.
//!
//! Where the file lives is not decided here. [`Store::in_home`] is handed the
//! directory, because `crucible_config::Home` is the one place that answers
//! where anything is.

mod error;
mod oauth;
mod store;

pub use error::AuthError;
pub use oauth::{
    KimiCredential, KimiOAuth, LoginAttempt, LoginMethod, LoginUpdate, OAuthError,
    OpenAiCredential, OpenAiOAuth, SubscriptionLogin,
};
pub use store::{Store, StoredCredentials};
