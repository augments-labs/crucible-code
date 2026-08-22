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
//! **A session picked up starts where it stopped.** The measurement above is
//! this process's, and a transcript read off a disk arrives with none of it —
//! so what a session was last told about itself is written down beside the
//! answer it described, and read back with it. A log that never got that far,
//! or one whose reading no longer covers the request this run would send, is a
//! session that estimates until its next answer reports.
//!
//! **The estimate calibrates itself.** A response reports a true token count for
//! a transcript whose exact byte length this loop knows, so the two together are
//! this model's own bytes-per-token on this session's own text. Nothing here
//! carries a tokenizer, a divisor per vendor, or a table to keep up to date: a
//! provider added later is calibrated by its first answer.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crucible_core::{Calibration, Carried, Message, Spend, ToolSchema};

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
/// Four numbers that are only ever wanted at once — what the turn has
/// produced, what its next request would carry, how much the model accepts, and
/// how much of that must remain free for the exchange in progress.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Counting {
    /// What the turn has produced so far, every response added up.
    pub(super) spent: Spend,
    /// What the next request would carry.
    pub(super) load: Load,
    /// How much this model accepts, where anybody knows.
    pub(super) window: Option<u32>,
    /// Room kept free for the exchange in progress.
    pub(super) reserve: u64,
}

impl Counting {
    /// How much usable room remains before the compaction boundary.
    pub(super) fn left(&self) -> Option<u8> {
        self.load.left(self.window, self.reserve)
    }
}

/// What the next request would carry.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Load {
    /// What the last response said its own request carried.
    carried: u64,
    /// Whether the current response reported its request input.
    input_reported: bool,
    /// Output reported since `carried`, across every response for which no
    /// newer input count has superseded it yet.
    spent: u64,
    /// The current response's contribution to `spent`.
    ///
    /// Providers may repeat a cumulative output count as a response streams.
    /// Keeping this separately lets a later reading replace that response's
    /// earlier reading without replacing output from an earlier uncounted-input
    /// response too.
    current_spent: u64,
    /// Whether the current response has produced visible content or call data.
    output_seen: bool,
    /// Whether a provider count covers all output seen in this response so far.
    output_reported: bool,
    /// Response bytes not yet covered by the latest output-token report.
    ///
    /// Kept separately from `appended`: while a response streams it is not in
    /// the transcript yet. Recording the finished agent message moves only
    /// this uncovered suffix into `appended`, so a prefix already covered by a
    /// provider report is never estimated again.
    unreported: u64,
    /// Estimated content bytes in the request whose input count was reported.
    sent: u64,
    /// System-instruction and tool-schema bytes in that request.
    sent_overhead: u64,
    /// Identity of the fixed request content that report covered.
    sent_overhead_signature: u64,
    /// Content bytes in the request currently being answered.
    in_flight: u64,
    /// Bytes appended since, less the answer already counted by `spent`.
    appended: u64,
    /// System-instruction and advertised-tool bytes in the next request.
    overhead: u64,
    /// Identity of that fixed request content, including order and boundaries.
    overhead_signature: u64,
    /// Bytes of the whole transcript, kept as it grows.
    ///
    /// Counted a message at a time rather than measured when it is wanted: the
    /// transcript is the one thing here that grows with the session, and
    /// walking it for a length would put that growth on the turn path.
    bytes: u64,
}

impl Load {
    /// Sets the non-transcript content of the next ordinary model request.
    ///
    /// A provider's carried-input report includes both of these. They are only
    /// estimated while no report covers the request being built; once one
    /// arrives, [`Self::carried`] supersedes this estimate whole.
    pub(super) fn requesting(&mut self, system: Option<&str>, tools: &[ToolSchema]) {
        let system_bytes = system.map_or(0_u64, |text| text.len() as u64);
        let schemas = tools.iter().fold(0_u64, |bytes, tool| {
            bytes
                .saturating_add(tool.name.len() as u64)
                .saturating_add(tool.schema.len() as u64)
                // Conservative allowance for the provider-specific object and
                // field names wrapped around every function declaration.
                .saturating_add(64)
        });
        self.overhead = system_bytes.saturating_add(schemas);

        // Length alone is not identity: changing one same-sized instruction or
        // schema changes tokenization just as surely as changing its size. The
        // signature is used only inside this process to decide whether an old
        // exact report still covers the request now being built.
        let mut signature = DefaultHasher::new();
        system.hash(&mut signature);
        system.is_some().hash(&mut signature);
        tools.len().hash(&mut signature);
        for tool in tools {
            tool.name.hash(&mut signature);
            tool.schema.hash(&mut signature);
        }
        self.overhead_signature = signature.finish();
    }

    /// Opens another provider response over the request as it stands.
    pub(super) fn responding(&mut self) {
        self.input_reported = false;
        self.current_spent = 0;
        self.output_seen = false;
        self.output_reported = false;
        self.unreported = 0;
        self.in_flight = self.bytes.saturating_add(self.overhead);
    }

    /// What the request that has just been answered carried.
    pub(super) fn carried(&mut self, carried: Carried) {
        self.carried = carried.tokens();
        self.input_reported = true;
        // The count just reported is the input to the response now arriving.
        // Output from preceding responses and everything appended after them is
        // already inside that input. Preserve only an output count belonging to
        // this response in case a compatible endpoint reported it first.
        self.spent = self.current_spent;
        // What the transcript held when the request went out, which is what it
        // holds now: this arrives while the answer is still being read, before
        // any of it has been recorded.
        self.sent = if self.in_flight == 0 {
            self.bytes.saturating_add(self.overhead)
        } else {
            self.in_flight
        };
        self.sent_overhead = self.overhead;
        self.sent_overhead_signature = self.overhead_signature;
        self.appended = 0;
    }

    /// What a session picked up was last told about itself.
    ///
    /// The recount that precedes this estimated the whole transcript, because a
    /// log holds messages and nothing beside them says what any of it cost.
    /// This is the one line that does, and it covers exactly the transcript
    /// just walked — so what was estimated becomes measured, and the row says
    /// how much window is left the moment a session comes back rather than one
    /// answer later.
    ///
    /// The fixed content of a request is compared by its **length** here, and
    /// not by the signature the rest of this file compares. That signature is a
    /// hash this build computes, and another build computing it differently
    /// would make every log written by the first one read as covering something
    /// else — so it is never written down, and length is what a log can carry.
    /// What length cannot tell apart is two same-sized sets of instructions,
    /// which moves the reading by the difference between two texts of one size
    /// and only until the next response reports for itself. Every ordinary way
    /// the fixed content changes — another model, another tool, another mode —
    /// changes its size, and is refused here and estimated as before.
    pub(super) fn measured(&mut self, calibration: Calibration) {
        if self.overhead != calibration.overhead {
            return;
        }

        self.carried = calibration.carried.tokens();
        self.spent = calibration.spent.tokens();
        self.current_spent = self.spent;
        self.sent = calibration.sent;
        self.sent_overhead = calibration.overhead;
        self.sent_overhead_signature = self.overhead_signature;
        self.input_reported = true;
        self.output_reported = true;
        self.unreported = 0;
        self.appended = 0;
    }

    /// What this load would have a session remember, where it knows exactly.
    ///
    /// `None` wherever anything has happened that the last report does not
    /// cover. Persistence is stricter than display: a conservative estimate is
    /// useful on screen, while writing it down as a measurement would make a
    /// later session trust a number no provider proved until the next
    /// successful provider report corrects it.
    pub(super) fn calibrated(&self) -> Option<Calibration> {
        self.exact().then(|| Calibration {
            carried: Carried::new(self.carried),
            spent: Spend::new(self.spent),
            sent: self.sent,
            overhead: self.sent_overhead,
        })
    }

    /// What that response produced.
    pub(super) fn spent(&mut self, spent: Spend) {
        let spent = spent.tokens();
        self.spent = self
            .spent
            .saturating_sub(self.current_spent)
            .saturating_add(spent);
        self.current_spent = spent;
        self.output_reported = true;
        self.unreported = 0;
    }

    /// Notes response bytes whose output-token count has not caught up yet.
    pub(super) fn produced(&mut self, bytes: usize) {
        self.output_seen = true;
        self.output_reported = false;
        self.unreported = self.unreported.saturating_add(bytes as u64);
    }

    fn exact(&self) -> bool {
        self.input_reported
            && self.appended == 0
            && self.overhead == self.sent_overhead
            && self.overhead_signature == self.sent_overhead_signature
            && (!self.output_seen || self.output_reported)
    }

    /// A message added to the transcript since.
    ///
    /// The agent's own message adds to the transcript's length like any other.
    /// What joins the estimate is only the suffix not covered by its latest
    /// output report. Where no output was observed separately, the complete
    /// message is that suffix.
    pub(super) fn recorded(&mut self, message: &Message) {
        let bytes = Self::bytes(message);
        let estimated = match message {
            Message::Agent { .. } if self.output_reported => 0,
            Message::Agent { .. } if self.output_seen => self.unreported,
            Message::Agent { .. } | Message::User { .. } | Message::ToolResults(_) => bytes,
        };

        self.bytes = self.bytes.saturating_add(bytes);
        self.appended = self.appended.saturating_add(estimated);
        if matches!(message, Message::User { .. } | Message::ToolResults(_)) {
            // The next response has not reported the request containing this
            // message, nor any output it may go on to produce.
            self.input_reported = false;
            self.output_reported = false;
        }
        if matches!(message, Message::Agent { .. }) {
            self.unreported = 0;
        }
    }

    /// A message already standing when the load's calibration was reset.
    ///
    /// Unlike [`Self::recorded`], an agent answer here has no matching `spent`
    /// count left beside it. It must therefore be estimated with every other
    /// message; otherwise model changes and compaction make all earlier agent
    /// prose disappear from the next-request load.
    pub(super) fn recounted(&mut self, message: &Message) {
        let bytes = Self::bytes(message);
        self.bytes = self.bytes.saturating_add(bytes);
        self.appended = self.appended.saturating_add(bytes);
    }

    fn bytes(message: &Message) -> u64 {
        (match message {
            Message::Agent { text, calls, .. } => text.len().saturating_add(
                calls
                    .iter()
                    .map(|call| {
                        call.id
                            .as_str()
                            .len()
                            .saturating_add(call.name.len())
                            .saturating_add(call.args.as_str().len())
                    })
                    .sum::<usize>(),
            ),
            Message::User { text: said, .. } => said.len(),
            Message::ToolResults(results) => results
                .iter()
                .map(|result| {
                    result
                        .id
                        .as_str()
                        .len()
                        .saturating_add(result.output.text().len())
                })
                .sum::<usize>(),
        }) as u64
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
        let bytes = if self.sent == 0 || self.carried == 0 {
            // No exact request exists to include its fixed request content.
            self.appended
                .saturating_add(self.overhead)
                .saturating_add(self.unreported)
        } else {
            // The exact request already included its own overhead. Only growth
            // since then belongs beside it. A decrease is deliberately not
            // subtracted from an exact token count using a byte estimate: that
            // conservatively overcounts until the next report supersedes it.
            let changed = self.overhead != self.sent_overhead
                || self.overhead_signature != self.sent_overhead_signature;
            self.appended
                .saturating_add(if changed { self.overhead } else { 0 })
                .saturating_add(self.unreported)
        };

        if self.sent == 0 || self.carried == 0 {
            return Self::ceiling(bytes, UNCALIBRATED);
        }

        // `bytes * carried / sent` — new request bytes at the rate the last
        // request's true token count implies. Rounded up: this count protects a
        // request boundary, so dropping a fractional token would spend room the
        // runner does not know it has.
        Self::ceiling(bytes.saturating_mul(self.carried), self.sent)
    }

    /// Unreported request text at the deliberately cautious initial rate.
    ///
    /// A standalone recap prompt is not part of the transcript a prior provider
    /// report calibrated, so using that ratio could amplify hidden request
    /// overhead into thousands of invented tokens. Three bytes per token is the
    /// conservative rate used before any report exists.
    #[must_use]
    pub(super) fn cautious(bytes: u64) -> u64 {
        Self::ceiling(bytes, UNCALIBRATED)
    }

    /// What `bytes` of transcript is estimated to cost the window, in tokens.
    ///
    /// At this model's own rate where a response has calibrated one, and at the
    /// deliberately pessimistic uncalibrated rate before that. It is the
    /// arithmetic of [`Self::estimated`] lifted out, so the compaction walk can
    /// measure a message it is deciding whether to keep at the same rate the
    /// load measures what was appended.
    #[must_use]
    pub(super) fn bytes_to_tokens(&self, bytes: u64) -> u64 {
        if self.sent == 0 || self.carried == 0 {
            return Self::ceiling(bytes, UNCALIBRATED);
        }

        // `bytes * carried / sent` — the bytes at the rate the last request's
        // true token count implies. As above, a conservative estimate rounds up.
        Self::ceiling(bytes.saturating_mul(self.carried), self.sent)
    }

    fn ceiling(numerator: u64, denominator: u64) -> u64 {
        numerator
            .saturating_add(denominator.saturating_sub(1))
            .checked_div(denominator)
            .unwrap_or(0)
    }

    /// How much usable room is left before compaction, as a percentage.
    ///
    /// The reserve is outside the usable window: it is the room the exchange in
    /// progress may still need for its answer and tool results. This makes the
    /// reading reach zero at the same boundary [`Self::full`] uses. The usable
    /// capacity is the denominator, so a fresh ordinary session still reads as
    /// one hundred percent.
    ///
    /// Rounded down, so a reading of `1%` is never drawn over a usable window
    /// that has already run out.
    #[must_use]
    pub(super) fn left(&self, window: Option<u32>, reserve: u64) -> Option<u8> {
        let window = u64::from(window?);
        let usable = window.saturating_sub(reserve);
        let used = self.tokens().min(usable);

        u8::try_from((usable - used) * 100 / usable.max(1)).ok()
    }

    /// Whether there is no longer room for another exchange.
    #[must_use]
    pub(super) fn full(&self, window: Option<u32>, reserve: u64) -> bool {
        window.is_some_and(|window| self.tokens().saturating_add(reserve) >= u64::from(window))
    }

    /// Starts again over a transcript that has been replaced.
    ///
    /// Everything measured describes messages that are no longer there, so none
    /// of it carries over — including the calibration, which described a
    /// request this session will now never send.
    pub(super) fn replaced(&mut self) {
        *self = Self::default();
    }

    /// Invalidates provider-specific usage while keeping transcript bytes.
    ///
    /// Used for model and provider changes. It is the same conservative recount
    /// as [`Self::recounted`], but constant-time because no message changed and
    /// the byte total is already maintained as the transcript grows.
    pub(super) fn reestimated(&mut self) {
        let bytes = self.bytes;
        let overhead = self.overhead;
        let overhead_signature = self.overhead_signature;
        *self = Self {
            appended: bytes,
            overhead,
            overhead_signature,
            bytes,
            ..Self::default()
        };
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
