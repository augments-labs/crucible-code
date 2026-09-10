//! What a confined process says while it runs, and how it ends.
//!
//! The contracts describing the boundary itself live in `crucible-sandbox`.
//! What stays here is the other half: the four values a caller reads off a
//! running command — what it said, what it heard, what it muttered, and how it
//! finished — which are shaped by the transport a command speaks over rather
//! than by the confinement it runs under.

mod finish;
mod heard;
mod muttered;
mod said;

pub use finish::Finish;
pub use heard::Heard;
pub use muttered::Muttered;
pub use said::Said;
