//! Bounded framing, and the lifetime of a program crucible talks to.
//!
//! Two protocols in this workspace run a conversation over somebody else's
//! process: an extension's and MCP's. Neither owns the half underneath, and
//! that half is the whole of this crate — newline-delimited frames with a
//! ceiling on them, the four values a command is spoken to and read back
//! through, how a process is waited out and stopped, and whether one that ended
//! may be started again.
//!
//! Nothing here reads a method name, a request id or a manifest. A parser
//! branch for one of the two protocols would make this the place a third
//! protocol has to be added to, and the point of the split is that it is not:
//! what arrives is bytes between newlines, and what they mean is decided above.
//!
//! Nothing here starts a runtime either. What a hosted program says, what it
//! is told and what it mutters beside both are each read or written by a task
//! on the runtime the host hands over, held by the value the host reads, writes
//! or asks through and ended when that value is dropped. A host awaits what
//! those tasks hand over — frames through [`Frames::next_frame_async`] and
//! [`Written::send_async`], and how the program ended through
//! [`Finish::after_async`] — or, while it is still synchronous, waits for the
//! same from its own thread; both ride the same tasks and the same bounded
//! queues.
//!
//! Nothing here starts a process either. What to run, under what confinement
//! and on whose authority is a lifecycle with its own owner; what reaches this
//! crate is a process that has already been through all of that.

mod finish;
mod framing;
mod heard;
mod muttered;
mod owned;
mod pipes;
mod restarts;
mod said;
#[cfg(test)]
mod testing;

pub use finish::Finish;
pub use framing::{FRAME_BYTES, FrameError, Frames, Written};
pub use heard::Heard;
pub use muttered::Muttered;
pub use pipes::{Absent, Pipes, Unspoken};
pub use restarts::{Ambiguity, NoRestart, Restarting, Restarts};
pub use said::Said;
