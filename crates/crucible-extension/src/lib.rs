//! What an extension is, whether it may run, and talking to one that does.
//!
//! An extension is somebody else's program, and the three questions about one
//! are answered in that order. A manifest says what it claims to be, and is
//! read from bounded text without anything being started. A trust decision says
//! whether that claim is acted on, and is made against the identity half of the
//! manifest rather than against what it asked for. Only then is a process
//! started, spoken to, and heard from.
//!
//! The protocol lives here whole. Its state machine is its own — what a
//! conversation may say next, which call an answer belongs to, and how one
//! ends — and it is deliberately not shared with the other program crucible
//! talks to over a pipe. Both run newline-delimited JSON-RPC over
//! `crucible-transport`, and that is where the sharing stops: a framing is a
//! framing whoever is on the other end, while what a frame means is a protocol
//! decision that has to be able to differ.
//!
//! Nothing here reads a settings document or names a sandbox backend. What to
//! discover is handed in as a directory, and what to run it under is handed in
//! as a started process, so the crate that decides either of those can change
//! without this one being consulted.

mod calls;
mod conversation;
mod discovery;
mod hosted;
mod manifest;
mod parse;
mod speaking;
mod spoken;
#[cfg(test)]
mod testing;
mod trust;

pub use calls::{Asked, CallError, EXTENSION_CALLS, Serving};
pub use conversation::{Broken, Conversation, Next};
pub use discovery::{Extensions, Installed, MAX_EXTENSIONS, Refusal};
pub use hosted::{Ended, Hosted, Unstarted};
pub use manifest::{
    EXTENSION_ID_BYTES, EXTENSION_MANIFEST_BYTES, EXTENSION_REQUESTS, EXTENSION_TEXT_BYTES,
    ExtensionCapability, ExtensionContribution, ExtensionError, ExtensionIdentity,
    ExtensionManifest, ExtensionProtocol, ExtensionRequests, ExtensionUnhosted,
};
pub use speaking::{Asking, Over, Speaking, Turn};
pub use spoken::{CallId, EXTENSION_SAID_BYTES, Malformed, Outcome, Spoken, SpokenError, Trouble};
pub use trust::{ExtensionDecision, ExtensionTrusted, ExtensionUntrusted};
