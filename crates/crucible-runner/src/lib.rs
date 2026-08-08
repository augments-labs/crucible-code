//! Drives turns to completion.
//!
//! The runner streams deltas from a provider, dispatches the tool calls the
//! model asks for, feeds the results back, and repeats until the model yields
//! or the user cancels.
//!
//! It depends on `crucible-core` alone. Every collaborator arrives as a trait
//! object chosen during wiring, so the loop never names Anthropic, `OpenAI`,
//! `grep`, or a renderer. Swapping any of them is a change in `main.rs`.
