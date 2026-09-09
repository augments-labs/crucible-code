//! Applying a secret to an outgoing request without exposing it.
//!
//! A credential is an open set: adding one must not edit this crate. What is
//! fixed here is the shape of applying one — the headers it writes, the exact
//! outgoing representations it registers for redaction, and the opaque scope
//! identity it is cached under. A credential-bearing value redacts its `Debug`,
//! stays out of `Display` and errors, and never reaches a session log.
//!
//! An implementation living outside this crate meets the same terms — it writes
//! its own headers, registers every exact outgoing representation for
//! redaction, and answers with a scope it derived rather than one it was
//! handed:
//!
//! ```
//! use crucible_credentials::{Credential, CredentialError, Outgoing};
//! use crucible_types::CredentialScopeId;
//!
//! /// A token this crate never sees the inside of.
//! struct Minted {
//!     token: String,
//!     scope: CredentialScopeId,
//! }
//!
//! impl std::fmt::Debug for Minted {
//!     fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         out.write_str("Minted(<redacted>)")
//!     }
//! }
//!
//! impl Credential for Minted {
//!     fn scope(&self) -> CredentialScopeId {
//!         self.scope
//!     }
//!
//!     fn authorize(&self, request: &mut Outgoing) -> Result<(), CredentialError> {
//!         let value = format!("Bearer {}", self.token);
//!         request.protect(value.clone());
//!         request.set_header("authorization", value);
//!         Ok(())
//!     }
//! }
//! ```

mod credential;

pub use credential::{
    ApiKey, Credential, CredentialError, Header, HeaderKey, Outgoing, Redactions,
};
