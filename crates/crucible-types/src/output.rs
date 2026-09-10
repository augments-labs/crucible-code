//! The durable, model-visible representation of what a tool produced.
//!
//! A tool returns a live value with the authority its approval carried. What
//! persistence acknowledges, and what a transcript replays, is this — the same
//! words, the same failure flag, the same files, and a two-integer header where
//! the change lines used to be. It is data and nothing else: holding one grants
//! no permission to run anything, and there is no way back from here to the
//! live value a tool returns.

use std::fmt;

use crate::transcript::Attachment;

/// The most encoded bytes one retained tool result may occupy.
///
/// Owned here because both invocation and request-load reservation enforce the
/// same boundary. The encoded JSON string is measured, including its quotes
/// and escapes, rather than the raw UTF-8 that can grow during serialization.
pub const TOOL_RESULT_BYTES: usize = 30_000;

/// The smallest descriptor-local result limit that can always state elision.
pub const TOOL_RESULT_MIN_BYTES: usize = 160;

/// How many lines a call changed, once the lines themselves are gone.
///
/// The two numbers a change header is written from — `Added 3 lines`, and the
/// rest of that wording — and nothing else. A diff is the detail under that
/// header and is for the reader alone; this is the header itself, and it names
/// no file and holds no line, which is what lets it outlive the diff and be
/// written down where a diff may never go.
///
/// Not `dropped`. That is a fact about a block of lines that was drawn, and
/// somewhere with no lines to draw it would let a header claim rows nothing is
/// showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Changed {
    added: usize,
    removed: usize,
}

impl Changed {
    /// The counts a call ended with.
    #[must_use]
    pub fn new(added: usize, removed: usize) -> Self {
        Self { added, removed }
    }

    /// How many lines the change put in.
    #[must_use]
    pub fn added(&self) -> usize {
        self.added
    }

    /// How many it took out.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }

    /// Whether the call left the file exactly as it was.
    ///
    /// The same question a diff's own emptiness answers and the same answer, so
    /// a call that changed nothing reads as nothing to say from either side of
    /// the moment the lines were dropped.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// Capture-time elision a process tool already stated in its own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureElision {
    original: usize,
    omitted: usize,
}

impl CaptureElision {
    /// Records one capture-time elision, or nothing where none happened.
    #[must_use]
    pub const fn new(original: usize, omitted: usize) -> Option<Self> {
        if omitted > 0 {
            Some(Self { original, omitted })
        } else {
            None
        }
    }
}

/// What one encoded-result bound retained and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolOutputRetention {
    original: usize,
    retained: usize,
    omitted: usize,
}

impl ToolOutputRetention {
    /// The original JSON-string size, including quotes and escapes.
    #[must_use]
    pub const fn original(self) -> usize {
        self.original
    }

    /// The final JSON-string size retained for the model.
    #[must_use]
    pub const fn retained(self) -> usize {
        self.retained
    }

    /// Encoded source bytes removed from the middle.
    #[must_use]
    pub const fn omitted(self) -> usize {
        self.omitted
    }
}

/// What a tool produced, as the transcript, the journal and a checkpoint keep
/// it.
///
/// The text is what the model is sent. The files are what it may look at. The
/// header is what a reader's row is drawn from once the change lines it
/// summarises are gone — two integers naming no file, which is what lets it be
/// written somewhere a diff may never go.
///
/// There is no constructor here that turns one of these back into the live
/// value a tool returns. Files arrive by two doors: the protected restore
/// below, and [`Self::recorded`], which the live value walks out through on its
/// way to being kept. This crate cannot name the permission proof the live
/// constructor demands for the same files, so what stands in for the type is a
/// repository check holding `recorded` to that one caller. A record somebody
/// edited can therefore put a readable path into a request, exactly as a prompt
/// line already can, and can grant nothing further.
#[derive(Clone, PartialEq, Eq)]
pub struct RecordedToolOutput {
    text: Box<str>,
    failed: bool,
    capture: Option<CaptureElision>,
    changed: Option<Changed>,
    attachments: Box<[Attachment]>,
}

impl fmt::Debug for RecordedToolOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordedToolOutput")
            .field("text", &"[redacted]")
            .field("failed", &self.failed)
            .field("capture", &self.capture)
            .field("changed", &self.changed)
            .field("attachments", &self.attachments)
            .finish()
    }
}

impl RecordedToolOutput {
    /// The smallest result worth clearing, in bytes.
    ///
    /// Under it the placeholder costs more than the result did, and clearing
    /// would grow the very thing it is meant to shrink. On the type rather than
    /// in a module, so the caller that estimates what a pass would recover
    /// reads the same figure the clearing enforces — two copies would drift
    /// apart the first time one moved.
    pub const MIN_PRUNE_BYTES: usize = 64;

    /// A successful result.
    #[must_use]
    pub fn ok(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: false,
            capture: None,
            changed: None,
            attachments: Box::new([]),
        }
    }

    /// A result the model should treat as a failure it can react to — a
    /// missing file, a non-zero exit status, a pattern that matched nothing.
    #[must_use]
    pub fn failed(text: impl Into<Box<str>>) -> Self {
        Self {
            text: text.into(),
            failed: true,
            capture: None,
            changed: None,
            attachments: Box::new([]),
        }
    }

    /// The one-way finalization a live tool result crosses on its way to being
    /// recorded.
    ///
    /// Called by the live value's own conversion, which is where the change
    /// lines are dropped and their header kept. The parameters are already
    /// bounded and already approved by the time they arrive, so this adds no
    /// judgement of its own; there is deliberately no reverse of it.
    #[must_use]
    pub fn recorded(
        text: impl Into<Box<str>>,
        failed: bool,
        capture: Option<CaptureElision>,
        changed: Option<Changed>,
        attachments: impl Into<Box<[Attachment]>>,
    ) -> Self {
        Self {
            text: text.into(),
            failed,
            capture,
            changed,
            attachments: attachments.into(),
        }
    }

    /// The same result, with the header a reader was shown and no lines under
    /// it.
    ///
    /// A result read back from somewhere a diff may not go arrives already
    /// parted from its lines, and this is how it says so — there is nothing to
    /// draw a header from otherwise, and nothing to work one out from either.
    #[must_use]
    pub fn counting(mut self, changed: Changed) -> Self {
        self.changed = Some(changed);
        self
    }

    /// The files this result showed, restored from protected persistence this
    /// build wrote.
    ///
    /// The one way files reach a result without the proof that admitted them,
    /// and it is here because that proof is not a thing a log can hold: a
    /// verdict is reached about a call, and the call is long over. What stands
    /// in its place is where the record came from — an owner-only session or
    /// checkpoint that this build wrote only after the engine had allowed the
    /// call. Reading back what was recorded is not deciding it again.
    ///
    /// A log somebody has edited can put any readable path into a request this
    /// way. It can already do that with a prompt line, which carries no proof
    /// either, so what this rests on is the log file's own boundary rather than
    /// a new one. What must stay true is that nothing else calls it, and that
    /// is not left to a comment: `scripts/sh/check.sh` holds it to the one
    /// module that replays a log.
    #[must_use]
    pub fn replayed(mut self, attachments: impl Into<Box<[Attachment]>>) -> Self {
        self.attachments = attachments.into();
        self
    }

    /// The same result, saying again what it said before something replaced
    /// its text.
    ///
    /// What replaces it is a pruning: a long session gives back the room its
    /// oldest results are taking by putting a sentence in place of what they
    /// held, so the model stops being sent them. The reader never stopped being
    /// shown them — the rows went down when the calls answered, and are still
    /// what the session looks like — so a screen drawing that session again puts
    /// the words back on the row and leaves the transcript as it is.
    ///
    /// Only the text. Whether the call failed, and what it changed, are what
    /// they always were: a pruning takes the words and touches nothing else, so
    /// nothing else is worth putting back.
    #[must_use]
    pub fn saying(mut self, text: impl Into<Box<str>>) -> Self {
        self.text = text.into();
        self
    }

    /// The files this result asks the model to look at.
    #[must_use]
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// The text the model sees.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The same text, out of the result and into whatever asked for it.
    ///
    /// For the reader's copy of a result, which arrives owned and is otherwise
    /// dropped once the row for it has been drawn. A reader who asks to see the
    /// whole of a result that was cut down to a row is asking for text that has
    /// already been allocated twice — once for the transcript the model is
    /// replayed, once for the event that drew it — and this is what keeps the
    /// answer from being a third copy.
    #[must_use]
    pub fn into_text(self) -> Box<str> {
        self.text
    }

    /// Whether the provider should mark this result as an error.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// How much it changed, where the lines are no longer here to be counted.
    #[must_use]
    pub fn changed(&self) -> Option<Changed> {
        self.changed
    }

    /// Records capture-time process elision for the encoded limiter.
    ///
    /// Process tools already state this in their returned text. Retaining the
    /// counts here lets a later encoded-size pass repeat the fact if it must
    /// replace that middle marker with its own.
    #[must_use]
    pub const fn with_capture_elision(mut self, original: usize, omitted: usize) -> Self {
        self.capture = CaptureElision::new(original, omitted);
        self
    }

    /// Elides the middle until the JSON-encoded text fits `maximum`.
    ///
    /// Both ends survive: the head carries setup and the tail usually carries
    /// the failure or final status. When anything is removed, the inserted
    /// model-visible note states the original encoded size and the encoded
    /// bytes omitted. Callers must supply at least [`TOOL_RESULT_MIN_BYTES`],
    /// which descriptor construction enforces for local limits.
    #[must_use]
    pub fn limit_encoded(&mut self, maximum: usize) -> ToolOutputRetention {
        let (text, retention) = limit_encoded(&self.text, self.capture, maximum);
        if let Some(text) = text {
            self.text = text;
        }
        retention
    }

    /// Replaces the text with a placeholder saying it was cleared, and says how
    /// much that freed.
    ///
    /// The lightest-touch form of compaction: a result deep enough in the
    /// transcript that the model has long since used it is bulk it will never
    /// read again, and a placeholder of a few words answers the only question
    /// the gap could raise — the call has a result, and the result is gone on
    /// purpose. The original is untouched in the session log, which is the
    /// record; this is only what the model is sent from here on. Returns the
    /// bytes freed, so the caller can decide whether clearing paid.
    ///
    /// A result small enough that the placeholder would cost more than it saves
    /// is left alone and frees nothing — clearing is for the results that
    /// dominate a transcript, and churning the small ones buys nothing.
    pub fn prune(&mut self) -> usize {
        let freed = self.text.len();
        if freed < Self::MIN_PRUNE_BYTES {
            return 0;
        }

        self.text = format!("[cleared to make room — {freed} bytes]").into();
        self.capture = None;

        // The files go with the words. They cost the transcript almost
        // nothing — an attachment is a path — but a request reads every one it
        // still holds, so a result nobody will read again would go on sending
        // whole pictures for a sentence saying it is gone.
        self.attachments = Box::new([]);
        freed
    }

    /// Replaces the output text with a placeholder unconditionally.
    pub fn clear(&mut self, notice: &str) -> usize {
        let freed = self.text.len();
        self.text = notice.into();
        self.capture = None;
        self.attachments = Box::new([]);
        freed
    }
}

/// Elides the middle of `text` until its JSON encoding fits `maximum`.
///
/// Shared by the live and the recorded result, which enforce the same encoded
/// ceiling at different moments of one call's life. Returns the replacement
/// text only where something was removed, so an untouched result reallocates
/// nothing.
#[must_use]
pub fn limit_encoded(
    text: &str,
    capture: Option<CaptureElision>,
    maximum: usize,
) -> (Option<Box<str>>, ToolOutputRetention) {
    let original = encoded_string_bytes(text);
    if original <= maximum {
        return (
            None,
            ToolOutputRetention {
                original,
                retained: original,
                omitted: 0,
            },
        );
    }

    // Use the largest possible omission count to reserve the marker. The
    // actual count can have fewer digits but never more, so the final text
    // remains at or below the requested encoded ceiling without a sizing
    // loop whose answer could oscillate at a decimal boundary.
    let reserved = elision(original, original, capture);
    let source = maximum
        .saturating_sub(2)
        .saturating_sub(encoded_content_bytes(&reserved));
    let (head, tail, kept) = encoded_ends(text, source);
    let omitted = original.saturating_sub(2).saturating_sub(kept);
    let marker = elision(original, omitted, capture);
    let limited: Box<str> = format!("{head}{marker}{tail}").into();
    let retained = encoded_string_bytes(&limited);

    debug_assert!(maximum < TOOL_RESULT_MIN_BYTES || retained <= maximum);
    (
        Some(limited),
        ToolOutputRetention {
            original,
            retained,
            omitted,
        },
    )
}

fn elision(original: usize, omitted: usize, capture: Option<CaptureElision>) -> String {
    let captured = capture.map_or_else(String::new, |capture| {
        format!(
            "process output was {} bytes; {} bytes omitted during capture; ",
            capture.original, capture.omitted
        )
    });
    format!(
        "\n\n[{captured}tool result was {original} encoded bytes; {omitted} encoded bytes omitted from the middle]\n\n"
    )
}

fn encoded_ends(text: &str, budget: usize) -> (&str, &str, usize) {
    let head_budget = budget / 2;
    let tail_budget = budget.saturating_sub(head_budget);

    let mut head_end = 0;
    let mut head_cost = 0_usize;
    for (at, character) in text.char_indices() {
        let cost = encoded_character_bytes(character);
        if head_cost.saturating_add(cost) > head_budget {
            break;
        }
        head_cost = head_cost.saturating_add(cost);
        head_end = at.saturating_add(character.len_utf8());
    }

    let mut tail_start = text.len();
    let mut tail_cost = 0_usize;
    for (relative, character) in text[head_end..].char_indices().rev() {
        let cost = encoded_character_bytes(character);
        if tail_cost.saturating_add(cost) > tail_budget {
            break;
        }
        tail_cost = tail_cost.saturating_add(cost);
        tail_start = head_end.saturating_add(relative);
    }

    (
        text.get(..head_end).unwrap_or_default(),
        text.get(tail_start..).unwrap_or_default(),
        head_cost.saturating_add(tail_cost),
    )
}

fn encoded_string_bytes(text: &str) -> usize {
    2_usize.saturating_add(encoded_content_bytes(text))
}

fn encoded_content_bytes(text: &str) -> usize {
    text.chars().fold(0_usize, |bytes, character| {
        bytes.saturating_add(encoded_character_bytes(character))
    })
}

const fn encoded_character_bytes(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{08}' | '\u{0c}' | '\n' | '\r' | '\t' => 2,
        '\u{00}'..='\u{1f}' => 6,
        other => other.len_utf8(),
    }
}
/// The closed final state recorded for one tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutcome {
    /// The executor returned a successful result.
    Succeeded,
    /// The executor or one of its pipeline stages failed.
    Failed,
    /// Standing permission policy forbade the call.
    Forbidden,
    /// The user refused the call.
    Refused,
    /// The run was cancelled before the call could finish.
    Cancelled,
    /// The descriptor's cooperative deadline elapsed.
    TimedOut,
    /// The call was invalid, unknown, or stale before effects.
    Rejected,
    /// An earlier call ended the turn before this call could run.
    NotRun,
    /// The per-turn retained-output allowance was exhausted.
    OutputLimit,
    /// The executor panicked and was contained by the scheduler.
    Panicked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_is_bounded_by_its_encoded_size_and_names_what_was_omitted() {
        let source = format!("HEAD{}TAIL", "\"\\\n".repeat(12_000));
        assert!(source.len() > TOOL_RESULT_BYTES);
        let original = encoded_string_bytes(&source);
        let mut output = RecordedToolOutput::ok(source);

        let retained = output.limit_encoded(TOOL_RESULT_BYTES);

        assert_eq!(retained.original(), original);
        assert!(retained.omitted() > 0);
        assert!(retained.retained() <= TOOL_RESULT_BYTES);
        assert_eq!(encoded_string_bytes(output.text()), retained.retained());
        assert!(output.text().starts_with("HEAD"), "{}", output.text());
        assert!(output.text().ends_with("TAIL"), "{}", output.text());
        assert!(
            output.text().contains(&original.to_string()),
            "{}",
            output.text()
        );
        assert!(
            output.text().contains(&retained.omitted().to_string()),
            "{}",
            output.text()
        );
    }

    #[test]
    fn escaping_can_cross_the_result_ceiling_when_raw_bytes_do_not() {
        let source = "\"".repeat(TOOL_RESULT_BYTES / 2 + 1);
        assert!(source.len() < TOOL_RESULT_BYTES);
        assert!(encoded_string_bytes(&source) > TOOL_RESULT_BYTES);
        let mut output = RecordedToolOutput::ok(source);

        let retained = output.limit_encoded(TOOL_RESULT_BYTES);

        assert!(retained.omitted() > 0);
        assert!(encoded_string_bytes(output.text()) <= TOOL_RESULT_BYTES);
    }
}
