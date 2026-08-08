//! Events, turned into what a terminal shows.
//!
//! Committed lines carry no escape sequences. The live tail measures what it
//! holds in display columns to know where to move the cursor back to, and a
//! colour code is bytes it would count as width. Colour therefore goes only
//! where nothing is ever redrawn: the prompt mark, written straight through.

use crucible_core::{Event, Sensitivity, ToolCall, ToolOutput, Workspace};
use crucible_tui::{Renderer, Terminal, TerminalError};

/// How much of a tool's arguments a line shows.
const ARGS: usize = 56;

/// How much of a tool's output a line shows.
const OUTPUT: usize = 96;

/// Dim, then back. Only for text that is written once and never redrawn.
const DIM: &str = "\x1b[2m";
const PLAIN: &str = "\x1b[0m";

/// The two lines a session opens with.
///
/// The root is there because every tool path is relative to it, and a user who
/// started crucible in the wrong directory should find out before the first
/// tool call rather than after it.
pub(crate) fn opening<T: Terminal>(
    renderer: &mut Renderer<T>,
    model: &str,
    workspace: &Workspace,
) -> Result<(), TerminalError> {
    renderer.commit(&format!("crucible {} · {model}", env!("CARGO_PKG_VERSION")))?;
    renderer.commit(&workspace.root().display().to_string())?;
    renderer.commit("")
}

/// Draws one event.
pub(crate) fn event<T: Terminal>(
    renderer: &mut Renderer<T>,
    event: Event,
) -> Result<(), TerminalError> {
    match event {
        // The turn number is in the title bar, not in the transcript: a line
        // per turn saying which turn it is crowds out the turn itself.
        Event::TurnStarted { .. } => Ok(()),

        Event::Delta { text } => renderer.stream(&text),

        Event::ToolRequested { call } => {
            // Whatever the model was saying is finished; it said it to explain
            // the call that follows.
            renderer.settle()?;
            renderer.commit(&requested(&call))
        }

        Event::ToolFinished { output, .. } => renderer.commit(&finished(&output)),

        Event::TurnFinished { .. } => renderer.settle(),

        Event::Failed { error } => {
            renderer.settle()?;
            renderer.commit(&format!("! {error}"))
        }
    }
}

/// Draws a permission question and leaves the cursor where the answer goes.
pub(crate) fn question<T: Terminal>(
    renderer: &mut Renderer<T>,
    call: &ToolCall,
    sensitivity: &Sensitivity,
) -> Result<(), TerminalError> {
    renderer.settle()?;
    renderer.commit(&asked(call, sensitivity))?;
    mark(renderer, "  [y]es  [a]lways  [n]o › ")
}

/// Writes something the user is expected to type after.
///
/// Straight through the terminal rather than through `commit`, because what is
/// wanted here is a line with no newline on the end.
pub(crate) fn mark<T: Terminal>(
    renderer: &mut Renderer<T>,
    text: &str,
) -> Result<(), TerminalError> {
    renderer.settle()?;

    let terminal = renderer.terminal();
    if terminal.is_terminal() {
        terminal.write(DIM)?;
        terminal.write(text)?;
        terminal.write(PLAIN)?;
    } else {
        terminal.write(text)?;
    }
    terminal.flush()
}

/// The line for a call about to run.
///
/// The arguments are shown as the model wrote them. Parsing them again here to
/// pull out a path would be a second reading of a schema the tool already owns,
/// and the two would drift.
fn requested(call: &ToolCall) -> String {
    format!("· {} {}", call.name, clipped(call.args.as_str(), ARGS))
}

/// The line for a call that finished.
fn finished(output: &ToolOutput) -> String {
    let text = output.text();
    let mut lines = text.lines();
    let first = clipped(lines.next().unwrap_or_default(), OUTPUT);
    let rest = lines.count();

    let mark = if output.is_failed() { "  ✗ " } else { "  " };
    match rest {
        0 => format!("{mark}{first}"),
        more => format!("{mark}{first} (+{more} lines)"),
    }
}

/// What the user is being asked to allow.
fn asked(call: &ToolCall, sensitivity: &Sensitivity) -> String {
    match sensitivity {
        // Never reached through the permission engine, which allows reads
        // without asking. Here so that a tool reclassified later still has a
        // question to show rather than a blank one.
        Sensitivity::ReadOnly => format!("? {} {}", call.name, clipped(call.args.as_str(), ARGS)),

        Sensitivity::MutatesFile => format!(
            "? {} wants to change a file: {}",
            call.name,
            clipped(call.args.as_str(), ARGS)
        ),

        Sensitivity::SpawnsProcess { program } => {
            format!("? {} wants to run: {program}", call.name)
        }
    }
}

/// One line, at most `width` characters of it.
///
/// Control characters become spaces: a newline inside JSON arguments would
/// otherwise break the line in two, and the tail would be counting rows that
/// the caller did not mean to write.
fn clipped(text: &str, width: usize) -> String {
    let mut clipped: String = text
        .trim()
        .chars()
        .take(width)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();

    if text.trim().chars().nth(width).is_some() {
        clipped.push('…');
    }
    clipped
}

#[cfg(test)]
mod tests {
    use crucible_core::{ToolArgs, ToolId};

    use super::*;

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new("a"),
            name: name.into(),
            args: ToolArgs::new(args),
        }
    }

    #[test]
    fn a_requested_call_shows_the_arguments_the_model_wrote() {
        let line = requested(&call("read", r#"{"path":"src/main.rs"}"#));

        assert_eq!(line, r#"· read {"path":"src/main.rs"}"#);
    }

    #[test]
    fn long_arguments_are_clipped_rather_than_wrapped() {
        let long = format!(r#"{{"command":"{}"}}"#, "x".repeat(200));

        let line = requested(&call("bash", &long));

        assert!(line.ends_with('…'), "{line}");
        assert!(line.chars().count() <= ARGS + "· bash …".len(), "{line}");
    }

    #[test]
    fn a_newline_in_arguments_does_not_become_a_second_line() {
        // The tail counts rows to know where to put the cursor back. A line
        // that is secretly two rows leaves it one row too high, and the next
        // frame erases something the user was meant to keep.
        let line = requested(&call("write", "{\"text\":\"a\nb\"}"));

        assert!(!line.contains('\n'), "{line}");
    }

    #[test]
    fn output_shows_its_first_line_and_says_how_much_more_there_was() {
        let output = ToolOutput::ok("one\ntwo\nthree");

        assert_eq!(finished(&output), "  one (+2 lines)");
    }

    #[test]
    fn a_single_line_of_output_gets_no_count() {
        assert_eq!(finished(&ToolOutput::ok("done")), "  done");
    }

    #[test]
    fn a_failure_is_marked_as_one() {
        // Without this a tool that failed reads exactly like one that worked,
        // and the user goes looking for the mistake in the wrong place.
        let line = finished(&ToolOutput::failed("no such file"));

        assert!(line.contains('✗'), "{line}");
    }

    #[test]
    fn no_output_at_all_is_still_a_line() {
        assert_eq!(finished(&ToolOutput::ok("")), "  ");
    }

    #[test]
    fn a_question_about_a_process_names_the_program_not_the_json() {
        // The user is deciding whether to let something run. `{"command":...}`
        // is the wrong thing to put that decision on.
        let asking = asked(
            &call("bash", r#"{"command":"rm -rf build"}"#),
            &Sensitivity::SpawnsProcess {
                program: "rm".into(),
            },
        );

        assert_eq!(asking, "? bash wants to run: rm");
    }

    #[test]
    fn a_question_about_a_file_shows_what_the_call_asked_for() {
        let asking = asked(
            &call("write", r#"{"path":"x.rs"}"#),
            &Sensitivity::MutatesFile,
        );

        assert!(asking.contains("x.rs"), "{asking}");
    }

    #[test]
    fn clipping_stops_at_a_character_not_a_byte() {
        // Slicing by byte here would panic on the first non-ASCII path a user
        // has, which is a crash on someone else's alphabet.
        assert_eq!(clipped("héllo wörld", 5), "héllo…");
    }
}
