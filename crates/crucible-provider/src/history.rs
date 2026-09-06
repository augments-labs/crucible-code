//! Visible history read as context, never transplanted as native signed output.
//!
//! Recaps and incompatible histories describe old calls instead of asserting
//! an executable call/result protocol. Borrowed pieces go straight into the
//! destination JSON string; no second transcript or private payload is built.

use crucible_core::{Message, StopReason};

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
