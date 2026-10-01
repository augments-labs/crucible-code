//! One OpenAI response, as deltas.
//!
//! The loop belongs to [`crate::stream`] and is the same for every provider,
//! and which events mean something is [`crate::responses::wire`]'s, in
//! [`super::Gpt`]'s dialect. What is here is for tests alone: the pairing of
//! the two under the name OpenAI's tests read a recorded response with.

#[cfg(test)]
use crate::openai::wire::Responses;
#[cfg(test)]
use crate::stream::Response;

/// A response being read, as this endpoint narrates one.
#[cfg(test)]
pub(super) type Stream = Response<Responses>;

#[cfg(test)]
pub(super) mod tests;
