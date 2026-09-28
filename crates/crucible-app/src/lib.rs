//! What a run of crucible is assembled from, and the conversation it then owns.
//!
//! This is the composition root. Every concrete thing a run needs — the
//! provider a name resolves to, the credential it is given, the tools, the
//! sandbox, the hosted servers, the session on disk — is built here and handed
//! downward as a trait object, which is what leaves every crate below free of
//! the others. What comes back up is a [`Conversation`]: a runner and the
//! session it records to, driven by inputs no terminal has to supply and
//! answering with values no terminal has to draw.
//!
//! A front end drives that conversation through [`client`], with the requests
//! `crucible-client-api` defines: the terminal is one client, a consumer with
//! no terminal is another, and what either is told is translated there from
//! the runner's own events and errors rather than serialized from them.
//!
//! What it deliberately does not know: how the command line was spelled, which
//! stays in the binary's own `cli` module with the terminal it adapts to; and
//! how anything is drawn, which is `crucible-tui`'s. The binary's probes and
//! generators reach past this crate to the concrete owners they measure. The
//! command line may not: the few concrete names it still spells are a list the
//! repository checks hold, and only let shrink.
//!
//! Nothing in here reads the environment or the disk on its own account
//! except through a parameter it was handed: the lookup, the home, the
//! workspace. That is what lets a startup be failed every way it can fail
//! without a key or a home directory anywhere near the test.

pub mod branching;
pub mod client;
mod conversation;
mod error;
pub mod extensions;
mod models;
pub mod providers;
pub mod remember;
pub mod runtime;
#[cfg(test)]
mod sample;
pub mod sandbox;
pub mod selecting;
pub mod services;
pub mod startup;
pub mod subscription;
pub mod switching;

pub use conversation::Conversation;
pub use error::AppError;
