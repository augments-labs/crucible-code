//! Filesystem, search and process tools the agent can call.
//!
//! Depends on `crucible-core` alone, and is a sibling of `crucible-provider`:
//! neither may reach the other. Each tool implements `Tool` from core, so the
//! runner dispatches to them without naming any of them.
//!
//! Every tool that mutates a file or spawns a process takes a `Grant` as an
//! argument rather than asking for one. A `Grant` is minted only by the
//! permission engine, so permission is impossible to forget and equally
//! impossible to fake: code that has not obtained one cannot call the
//! operation.
