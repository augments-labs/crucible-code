//! Private session logs and bounded replay.
//!
//! One file per session, one line per message, appended in order and held
//! owner-only on disk. The runner records through [`Session`]; everything
//! else meets the pieces through the runner's re-exports, so the file format
//! and the platform boundary live here and nowhere else.

mod session;

#[cfg(test)]
mod sample;

pub use session::{
    Glimpse, PROMPTS, Recorded, Session, SessionError, glimpse, prompts, recent, remember, retitle,
};
