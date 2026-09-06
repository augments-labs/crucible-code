//! Visible history read as context, never transplanted as native signed output.
//!
//! Recaps and incompatible histories describe old calls instead of asserting
//! an executable call/result protocol. Borrowed pieces go straight into the
//! destination JSON string; no second transcript or private payload is built.

use crucible_core::{Message, StopReason};

/// Older wire readers have no native continuation decoder. Preserve their
/// existing unsigned history, but describe a foreign native answer and all of
/// its following tool results rather than asserting that this model made it.
#[derive(Default)]
pub(crate) struct LegacyHistory {
    foreign_results: bool,
}

impl LegacyHistory {
    pub(crate) fn neutral(&mut self, message: &Message) -> bool {
        match message {
            Message::Agent { continuation, .. } => {
                self.foreign_results = continuation.is_some();
                self.foreign_results
            }
            Message::ToolResults(_) => self.foreign_results,
            Message::User { .. } | Message::Context(_) => false,
        }
    }
}

pub(crate) fn visible(message: &Message, write: &mut dyn FnMut(&str)) {
    match message {
        Message::Context(fragment) => {
            write("Context:\n");
            write(fragment.text());
        }
        Message::User { text, .. } => {
            write("User:\n");
            write(text);
        }
        Message::Agent {
            text, calls, stop, ..
        } => {
            write("Assistant:\n");
            write(text);
            for call in calls {
                write("\nHistorical tool request ");
                write(call.id.as_str());
                write(" (");
                write(&call.name);
                write("):\n");
                write(call.args.as_str());
            }
            if let Some(cut) = StopReason::cut(*stop) {
                write("\n");
                write(cut);
            }
        }
        Message::ToolResults(results) => {
            write("Historical tool results:\n");
            for result in results {
                write(result.id.as_str());
                write(if result.output.is_failed() {
                    " (failed):\n"
                } else {
                    ":\n"
                });
                write(result.output.text());
                write("\n");
            }
        }
    }
}
