//! How much a tool may say.
//!
//! One figure, in bytes, for every tool. What a tool returns goes into the next
//! request whole, so the bound has to be the bytes: a count of lines is not the
//! same promise, and the difference is not small. Two hundred matching lines of
//! four hundred characters is eighty kilobytes against the thirty `bash` holds
//! itself to, and the caller that chose the two hundred is the model.
//!
//! It lives here rather than in the tool that first held itself to it, so that
//! the next tool to need one finds this figure instead of choosing its own.

/// The most one tool may say, in bytes.
///
/// Where it came from is what a command's output was already held to, and one
/// answer to "how much may a tool say" is worth more than a figure fitted to
/// each tool: a turn spends its context on whichever tool it called, and a
/// budget that moves with that is not a budget.
pub(crate) const OUTPUT: usize = 30_000;
