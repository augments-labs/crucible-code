//! Speaks the Model Context Protocol to a server somebody else wrote.
//!
//! Everything a server *is* was decided before this crate is reached: a record
//! in the configuration file said which program to start, and something above
//! selected it for this run. What is left is the part that has a protocol in
//! it — saying hello, agreeing which version of MCP both ends are speaking, and
//! reading back the tools the server offers.
//!
//! Core alone, deliberately. This crate knows a wire and nothing about which
//! server is on the other end of it: it takes a reader and a writer, so what
//! started the process, under what confinement, and what a settings document
//! said are all decided above it.
//!
//! Nothing here decides that a tool may be called. Reading a catalogue is
//! reading a list of names and schemas somebody else's program wrote, and the
//! whole of it is treated as hostile input — bounded on arrival, and inert
//! until something above turns it into tools the model can see.

mod catalogue;
mod hosted;
mod talking;
mod wire;

pub use catalogue::{
    ABOUT_BYTES, Greeting, NAME_BYTES, Offered, PAGES, Rebuffed, SCHEMA_BYTES, TOOLS, VERSIONS,
    hello, tools,
};
pub use hosted::{Ended, Hosted, Unstarted};
pub use talking::{ASIDES, Talking, Trouble};
pub use wire::{Call, Garbled, Heard, NO_SUCH_METHOD, RPC, Reply, SAID_BYTES, Sent};
