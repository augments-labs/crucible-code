//! The words a request is built from.
//!
//! A turn is sent under a system prompt and a set of context fragments that say
//! what is true of the session: its workspace, the skills it found, the
//! environment, the model, the tools and the permissions. This crate owns how
//! those facts become words — [`ContextSection`], the order a pass assembles
//! them in, the prompt around them — and what a compaction asks the model for
//! in place of the history it replaces.
//!
//! What it does not own is as deliberate. The persisted half of a context —
//! [`Fragment`](crucible_types::Fragment), [`ContextSnapshot`](crucible_types::ContextSnapshot)
//! and the patch between two of them — is shared data in `crucible-types`, so a
//! session replay needs nothing here. The loop that sends the request and
//! records the result is the runner's, and a provider receives the assembled
//! words as data without naming this crate.
//!
//! ```
//! use crucible_context::{ContextInputs, Live, assemble};
//! use crucible_tools::{Permission, ToolSnapshot};
//! use crucible_types::Transcript;
//! use std::time::{Duration, UNIX_EPOCH};
//!
//! let inputs = ContextInputs::new("/work").dated(UNIX_EPOCH + Duration::from_hours(497_088));
//! let tools = ToolSnapshot::empty();
//! let permission = Permission::default();
//! let live = Live {
//!     model: "a-model",
//!     effort: None,
//!     tools: &tools,
//!     permission: &permission,
//! };
//!
//! // A session with no recorded state states every section.
//! let first = assemble(&inputs, None, &Transcript::new(), live)?;
//! assert!(!first.fragments.is_empty());
//! assert!(first.patch.is_some());
//! # Ok::<(), crucible_types::ContextError>(())
//! ```

mod assembly;
pub mod compaction;
mod prompt;
mod sections;

pub use assembly::{Assembled, ContextInputs, Live, assemble};
pub use compaction::Room;
pub use prompt::{
    EnvironmentSection, Identity, ModelSection, PermissionsSection, Skill, SkillsSection,
    SystemPrompt, ToolsSection, WorkspaceSection,
};
pub use sections::{ContextSection, capture, seen};
