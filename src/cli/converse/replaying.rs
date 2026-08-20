//! Putting a session picked up back on the screen.
//!
//! A resumed session is one the model can see and the reader cannot: the
//! transcript goes back into every request, and the terminal it is being read
//! in is either empty or holds somebody else's scrollback. Nothing here changes
//! what is sent — this is the screen catching up with what the session already
//! is.
//!
//! **It is not a re-run.** No tool is called again, nothing is asked of a
//! provider, and no file is read. What goes down is what the log recorded, in
//! the order it recorded it, marked so a reader can tell it from what happens
//! next.
//!
//! **Tool calls are named rather than replayed whole.** What a tool said can be
//! megabytes, it is already in the transcript the model reads, and a reader
//! picking a session back up wants to know what was done rather than to read
//! every line of it again. The prompts and the answers go down in full, because
//! those are the conversation.

use crucible_core::{Message, RECAP, StopReason, Transcript};
use crucible_tui::{Renderer, Row, Slot, Terminal, clip, fold};

use crate::cli::Fatal;
use crate::cli::style::Style;

/// What stands in front of a line that happened before this run.
///
/// One mark, quiet, on the left of the rows that came back — so the point where
/// the old session stops and this one starts is a thing a reader can see rather
/// than a thing they have to remember.
const BEFORE: &str = "│ ";

/// The heading over the lot.
const OPENED: &str = "picking up where this left off";

/// Puts what a session already said back on the screen.
///
/// Committed rather than drawn live: this is the record of what happened, which
/// is exactly what scrollback is for, and the terminal owns it from here.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be written to.
pub(super) fn replayed<T: Terminal>(
    renderer: &mut Renderer<T>,
    transcript: &Transcript,
    style: Style,
) -> Result<(), Fatal> {
    if transcript.is_empty() {
        return Ok(());
    }

    let columns = renderer.columns();
    let glyphs = style.glyphs();

    // One call with the lot, not one per row: presenting settles what came
    // before it, so a row at a time would put each line where the last one was
    // and leave only the final message standing.
    let rule = || Row::new().then(Slot::Quiet, glyphs.horizontal().repeat(columns));
    let mut shown = vec![rule(), Row::new().then(Slot::Quiet, clip(OPENED, columns))];

    for message in transcript.messages() {
        shown.extend(rows(message, columns, style));
    }
    shown.push(rule());

    renderer.commit("")?;
    renderer.present(&shown, style.palette())?;
    renderer.commit("")?;

    Ok(())
}

/// One message, as the rows that say it happened.
fn rows(message: &Message, columns: usize, style: Style) -> Vec<Row> {
    let glyphs = style.glyphs();
    let room = columns.saturating_sub(BEFORE.chars().count());

    match message {
        // The notes a compaction left standing. They ride a user message
        // because the closed set of messages has no variant for them and no
        // provider would know what to do with one — but they are the model's
        // own words, and drawing them behind the mark somebody's typing wears
        // would say the user wrote them.
        Message::User(said) if said.starts_with(RECAP) => {
            let notes = said.strip_prefix(RECAP).unwrap_or(said);
            let mut rows = vec![
                Row::new()
                    .then(Slot::Quiet, BEFORE)
                    .then(Slot::Quiet, clip("notes on everything before this", room)),
            ];
            rows.extend(prose(notes, room, Slot::Quiet));
            rows
        }

        // What was asked, behind the mark the box puts in front of a line being
        // typed, so a reader finds their own words the way they left them.
        Message::User(said) => said
            .lines()
            .flat_map(|line| {
                let mark = format!("{} ", glyphs.caret());
                fold(line, room.saturating_sub(mark.chars().count()))
                    .into_iter()
                    .map(|part| {
                        Row::new()
                            .then(Slot::Quiet, BEFORE)
                            .then(Slot::Quiet, mark.clone())
                            .then(Slot::Plain, part.to_owned())
                    })
                    .collect::<Vec<_>>()
            })
            .collect(),

        Message::Agent { text, calls, stop } => {
            let mut rows = prose(text, room, Slot::Plain);

            // Named, not replayed: what a tool said is in the transcript the
            // model reads, and a reader wants what was done.
            for call in calls {
                rows.push(
                    Row::new()
                        .then(Slot::Quiet, BEFORE)
                        .then(Slot::Accent, glyphs.called())
                        .then(Slot::Quiet, format!(" {}", clip(&call.name, room))),
                );
            }

            // An answer that did not end the way the model meant it to is worth
            // the same line here it got the first time: a half answer read back
            // as a whole one is the one thing a transcript may not do.
            if let Some(said) = stop.and_then(cut) {
                rows.push(
                    Row::new()
                        .then(Slot::Quiet, BEFORE)
                        .then(Slot::Quiet, clip(said, room)),
                );
            }

            rows
        }

        // Counted rather than shown, for the reason the module doc gives.
        Message::ToolResults(results) => {
            let said = match results.len() {
                1 => "1 result".to_owned(),
                many => format!("{many} results"),
            };

            vec![
                Row::new()
                    .then(Slot::Quiet, BEFORE)
                    .then(Slot::Quiet, format!("  {said}")),
            ]
        }
    }
}

/// Prose, wrapped to the room it has.
///
/// **Folded, never clipped.** A transcript put back with its right-hand edge
/// cut off is a transcript somebody has to go and read the log to understand —
/// which is the whole of what this exists to save them.
fn prose(text: &str, room: usize, slot: Slot) -> Vec<Row> {
    text.lines()
        .flat_map(|line| {
            // A blank line in the middle of an answer is a paragraph break, and
            // folding drops it — so it is kept here rather than lost.
            if line.trim().is_empty() {
                return vec![Row::new().then(Slot::Quiet, BEFORE)];
            }

            fold(line, room)
                .into_iter()
                .map(|part| {
                    Row::new()
                        .then(Slot::Quiet, BEFORE)
                        .then(slot, part.to_owned())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What an ending is worth saying about, replayed.
const fn cut(stop: StopReason) -> Option<&'static str> {
    match stop {
        StopReason::Yielded | StopReason::WantsTools => None,
        StopReason::OutOfTokens => Some("  (cut off at the token ceiling)"),
        StopReason::WindowExceeded => Some("  (the request did not fit)"),
        StopReason::Filtered => Some("  (cut short by the provider's filter)"),
        StopReason::Paused => Some("  (paused, and never carried on)"),
        StopReason::Cancelled => Some("  (stopped)"),
        StopReason::Unknown => Some("  (ended for an unknown reason)"),
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{ToolCall, ToolId, ToolOutput, ToolResult};

    use super::*;

    /// A transcript with one of everything in it.
    fn said() -> Transcript {
        let mut transcript = Transcript::new();
        transcript.push(Message::User(
            "read the config and tell me what it says".into(),
        ));
        transcript.push(Message::Agent {
            text: "I will look at it.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: crucible_core::ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
        });
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: ToolOutput::ok("a very long file, most of which nobody wants again"),
        }]));
        transcript.push(Message::Agent {
            text: "It sets the theme and nothing else.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        transcript
    }

    #[test]
    fn a_resumed_session_is_put_back_on_the_screen() {
        let drawn: Vec<String> = said()
            .messages()
            .iter()
            .flat_map(|message| rows(message, 80, Style::plain()))
            .map(|row| row.text())
            .collect();
        let screen = drawn.join("\n");
        println!("\n{screen}");

        // What was asked and what was answered, in full: those are the
        // conversation, and a reader picking it up is looking for both.
        assert!(screen.contains("read the config"), "{screen}");
        assert!(screen.contains("It sets the theme"), "{screen}");

        // The tool named rather than replayed whole — what it said is already
        // in what the model reads, and it can be megabytes.
        assert!(screen.contains("read"), "{screen}");
        assert!(
            !screen.contains("most of which nobody wants again"),
            "{screen}"
        );
        assert!(screen.contains("1 result"), "{screen}");
    }

    #[test]
    fn a_long_answer_is_wrapped_rather_than_cut_off_at_the_edge() {
        // A transcript put back with its right-hand edge missing is one
        // somebody has to open the log to understand, which is the whole of
        // what this exists to save them.
        let long = "a ".repeat(120);
        let mut transcript = Transcript::new();
        transcript.push(Message::Agent {
            text: long.clone().into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });

        let drawn: Vec<String> = transcript
            .messages()
            .iter()
            .flat_map(|message| rows(message, 40, Style::plain()))
            .map(|row| row.text())
            .collect();

        assert!(drawn.len() > 1, "it was cut instead of wrapped: {drawn:?}");
        for row in &drawn {
            assert!(crucible_tui::columns(row) <= 40, "{row:?}");
        }

        // And nothing of it went missing on the way.
        let back: String = drawn
            .iter()
            .map(|row| row.trim_start_matches(['│', ' ']))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(back.split_whitespace().count() >= long.split_whitespace().count());
    }

    #[test]
    fn the_notes_a_compaction_left_are_not_drawn_as_something_somebody_typed() {
        // They ride a user message because the closed set has no variant for
        // them — but they are the model's own words, and the mark a typed line
        // wears would say otherwise.
        let mut transcript = Transcript::new();
        transcript.push(Message::User(
            format!("{RECAP}what was decided, and what is left").into(),
        ));

        let drawn: Vec<String> = transcript
            .messages()
            .iter()
            .flat_map(|message| rows(message, 80, Style::plain()))
            .map(|row| row.text())
            .collect();
        let screen = drawn.join("\n");
        println!("\n{screen}");

        assert!(
            screen.contains("notes on everything before this"),
            "{screen}"
        );
        assert!(screen.contains("what was decided"), "{screen}");
        assert!(
            !screen.contains('›'),
            "the notes are behind a prompt mark: {screen}"
        );
    }

    #[test]
    fn an_answer_that_did_not_finish_says_so_the_second_time_too() {
        // A half answer read back as a whole one is the one thing a transcript
        // may not do, and replaying it is exactly where that would happen.
        let mut transcript = Transcript::new();
        transcript.push(Message::Agent {
            text: "half a th".into(),
            calls: Vec::new(),
            stop: Some(StopReason::OutOfTokens),
        });

        let screen: String = transcript
            .messages()
            .iter()
            .flat_map(|message| rows(message, 80, Style::plain()))
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(screen.contains("token ceiling"), "{screen}");
    }
}
