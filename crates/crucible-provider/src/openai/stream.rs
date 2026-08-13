//! One OpenAI response, as deltas.
//!
//! The loop belongs to [`crate::stream`] and is the same for every provider.
//! What is this provider's is which events mean something, which lives in
//! [`super::wire`]; what is here is the pairing of the two, and the tests that
//! read a recorded response end to end.

use crate::openai::wire::Responses;
use crate::stream::Response;

/// A response being read, as this endpoint narrates one.
pub(super) type Stream = Response<Responses>;

#[cfg(test)]
pub(super) mod tests;
