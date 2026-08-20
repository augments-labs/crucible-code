//! What the next request would carry, and the room kept free for the one after.
//!
//! Two numbers and the arithmetic between them. The load is what a request
//! about to go out would carry, in tokens; the reserve is how much of the
//! model's window must stay free for the answer and the tools it calls. When
//! the first crosses the window less the second, there is no room for another
//! exchange and something has to give.
//!
//! **The load is measured, not guessed, for all but its last stretch.** Every
//! provider reports what the request it just answered carried, and that covers
//! the transcript as it stood when the request went out. What is not covered is
//! whatever was appended since — the answer, and the tool results under it —
//! and only that part is estimated.
//!
//! **The estimate calibrates itself.** A response reports a true token count for
//! a transcript whose exact byte length this loop knows, so the two together are
//! this model's own bytes-per-token on this session's own text. Nothing here
//! carries a tokenizer, a divisor per vendor, or a table to keep up to date: a
//! provider added later is calibrated by its first answer.

use crucible_core::{Carried, Message, Spend};

/// Bytes per token before any response has been seen.
///
/// Only ever used for the first request of a session, where nothing has
/// reported a true count yet. Deliberately low — it over-states the load rather
/// than under-stating it, and the direction matters: compacting a little early
/// costs some context, while noticing too late costs the turn.
const UNCALIBRATED: u64 = 3;

/// The most one tool is allowed to say, in bytes.
///
/// Stated here rather than read from the crate that enforces it, because this
/// loop reaches no tool crate by design and must not start. It is a figure the
/// two have to agree on, and the reserve is only as good as that agreement.
const TOOL_RESULT_BYTES: u64 = 30_000;

/// How many tool results one pass is expected to carry.
///
/// Two, which is a judgement rather than a measurement: one answer plus a pair
/// of calls is the ordinary shape of a turn that is getting work done. Set low
/// on purpose — reserving too little costs one refused request, which is
/// visible and recoverable, and reserving too much throws away session nobody
/// sees go.
const RESULTS_PER_PASS: u64 = 2;

/// What one turn is counting, carried together through the read path.
///
/// Three numbers that are only ever wanted at once — what the turn has
/// produced, what its next request would carry, and how much the model accepts.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Counting {
    /// What the turn has produced so far, every response added up.
    pub(super) spent: Spend,
    /// What the next request would carry.
    pub(super) load: Load,
    /// How much this model accepts, where anybody knows.
    pub(super) window: Option<u32>,
}

/// What the next request would carry.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Load {
    /// What the last response said its own request carried.
    carried: u64,
    /// What that response produced, which is in the transcript now.
    spent: u64,
    /// Bytes of the transcript when that request went out.
    sent: u64,
    /// Bytes appended since, less the answer already counted by `spent`.
    appended: u64,
    /// Bytes of the whole transcript, kept as it grows.
    ///
    /// Counted a message at a time rather than measured when it is wanted: the
    /// transcript is the one thing here that grows with the session, and
    /// walking it for a length would put that growth on the turn path.
    bytes: u64,
}

impl Load {
    /// What the request that has just been answered carried.
    pub(super) fn carried(&mut self, carried: Carried) {
        self.carried = carried.tokens();
        // What the transcript held when the request went out, which is what it
        // holds now: this arrives while the answer is still being read, before
        // any of it has been recorded.
        self.sent = self.bytes;
        self.appended = 0;
    }

    /// What that response produced.
    pub(super) fn spent(&mut self, spent: Spend) {
        self.spent = spent.tokens();
    }

    /// A message added to the transcript since.
    ///
    /// The agent's own message adds to the transcript's length like any other,
    /// and is not added to what has to be *estimated*: what it cost is reported
    /// exactly and is already held as `spent`, so estimating its bytes as well
    /// would count one answer twice.
    pub(super) fn recorded(&mut self, message: &Message) {
        let (bytes, estimated) = match message {
            Message::Agent { text, .. } => (text.len(), false),
            Message::User(said) => (said.len(), true),
            Message::ToolResults(results) => (
                results
                    .iter()
                    .map(|result| result.output.text().len())
                    .sum::<usize>(),
                true,
            ),
        };

        self.bytes = self.bytes.saturating_add(bytes as u64);
        if estimated {
            self.appended = self.appended.saturating_add(bytes as u64);
        }
    }

    /// The whole of what the next request would carry, in tokens.
    #[must_use]
    pub(super) fn tokens(&self) -> u64 {
        self.carried
            .saturating_add(self.spent)
            .saturating_add(self.estimated())
    }

    /// What has been appended since the last response, in tokens.
    ///
    /// At this model's own rate where one response has been seen, and at a
    /// deliberately pessimistic one before that.
    fn estimated(&self) -> u64 {
        if self.sent == 0 || self.carried == 0 {
            return self.appended / UNCALIBRATED;
        }

        // `appended * carried / sent` — the appended bytes at the rate the last
        // request's true token count implies. Multiplied before dividing so a
        // short append does not round to nothing.
        self.appended
            .saturating_mul(self.carried)
            .checked_div(self.sent)
            .unwrap_or(0)
    }

    /// How much of the window is left, as a percentage, where one is known.
    ///
    /// Rounded down, so a reading of `1%` is never drawn over a window that has
    /// already run out.
    #[must_use]
    pub(super) fn left(&self, window: Option<u32>) -> Option<u8> {
        let window = u64::from(window?);
        let used = self.tokens().min(window);

        u8::try_from((window - used) * 100 / window.max(1)).ok()
    }

    /// Whether there is no longer room for another exchange.
    #[must_use]
    pub(super) fn full(&self, window: Option<u32>, reserve: u64) -> bool {
        window.is_some_and(|window| self.tokens() + reserve >= u64::from(window))
    }

    /// Starts again over a transcript that has been replaced.
    ///
    /// Everything measured describes messages that are no longer there, so none
    /// of it carries over — including the calibration, which described a
    /// request this session will now never send.
    pub(super) fn replaced(&mut self) {
        *self = Self::default();
    }
}

/// How much of the window must stay free for the next exchange, in tokens.
///
/// The answer this model may produce, plus the tool results a pass is likely to
/// carry back. Derived rather than written down: raising what an answer may be
/// raises what has to be kept for it, and the two cannot drift apart.
///
/// **Held to half the window.** A small model whose answer ceiling is most of
/// what it accepts would otherwise reserve more than it has, which is a session
/// that compacts on its first turn and every turn after. Half of a small window
/// is little to work in; none of it is nothing.
#[must_use]
pub(super) fn reserve(max_tokens: u32, window: Option<u32>, configured: Option<u64>) -> u64 {
    let asked = configured.unwrap_or_else(|| {
        u64::from(max_tokens)
            .saturating_add(RESULTS_PER_PASS.saturating_mul(TOOL_RESULT_BYTES / UNCALIBRATED))
    });

    window.map_or(asked, |window| asked.min(u64::from(window) / 2))
}

#[cfg(test)]
mod tests;
