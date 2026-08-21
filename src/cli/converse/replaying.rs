//! Putting a session picked up back on the screen.
//!
//! A resumed session is one the model can see and the reader cannot: the
//! transcript goes back into every request, and the screen it is being read on
//! was opened empty a moment ago. Nothing here changes what is sent — this is
//! the screen catching up with what the session already is.
//!
//! **It is not a re-run.** No tool is called again, nothing is asked of a
//! provider, and no file is read. What goes down is what the log recorded, in
//! the order it recorded it.
//!
//! **And it is drawn by the code that drew it the first time.** Every row here
//! comes out of `draw` and the components under it — the prompt row the box
//! commits, the call line the footing settles into, the result row, the model's
//! prose through the same markdown the live path renders it with. A second set
//! of row builders for the same messages would be a second answer to what a
//! session looks like, and the two would disagree the first time either was
//! touched: the theme somebody chose, the mark in front of a prompt, the colour
//! a tool's name is in. So there is one set, and this walks messages into it.
//!
//! What this module adds to the screen is a rule and a heading, saying where the
//! session picked up stops and this one starts. That is the one thing the
//! messages cannot say for themselves.
//!
//! One thing does not come back, and it is the record's doing rather than this
//! module's: a diff reaches no log, for the reason `crucible-core` gives beside
//! the type, so a call that changed a file replays as the result it returned
//! rather than as the lines it moved.

use crucible_core::{Message, RECAP};
use crucible_runner::Runner;
use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::draw;
use crate::cli::style::Style;

/// The heading over the lot.
const OPENED: &str = "picking up where this left off";

/// What stands over the notes a compaction left.
///
/// They ride a user message because the closed set of messages has no variant
/// for them and no provider would know what to do with one — so without a line
/// saying otherwise they would go down behind the mark a typed line wears,
/// which would say the user wrote them.
const NOTES: &str = "notes on everything before this";

/// Puts what a session already said back on the screen.
///
/// Committed rather than drawn live: this is the record of what happened, which
/// is exactly what the transcript holds, and it is scrolled back to like
/// anything else said this session.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be written to.
pub(super) fn replayed<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    style: Style,
) -> Result<(), Fatal> {
    if runner.transcript().is_empty() {
        return Ok(());
    }

    renderer.commit("")?;
    renderer.present(&opened(renderer.columns(), style))?;

    for message in runner.transcript().messages() {
        said(renderer, runner, message, style)?;
    }

    // Whatever the last message left live, ended: a session whose last turn was
    // the model talking leaves a tail in the region the renderer owns, and the
    // rule below belongs under it rather than over it.
    renderer.settle()?;
    renderer.apart()?;
    renderer.present(&[rule(renderer.columns(), style)])?;
    renderer.commit("")?;

    Ok(())
}

/// The rule and the heading that open it.
fn opened(columns: usize, style: Style) -> Vec<Row> {
    vec![
        rule(columns, style),
        Row::new().then(Slot::Quiet, clip(OPENED, columns)),
    ]
}

/// One rule across the window.
fn rule(columns: usize, style: Style) -> Row {
    Row::new().then(Slot::Quiet, style.glyphs().horizontal().repeat(columns))
}

/// One message, put back the way it went down.
///
/// The arms are in the order a turn produces them, which is the order the
/// transcript holds them in — so walking it hands the renderer the same calls
/// in the same order the turn did, and the picture is the picture.
fn said<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &Runner,
    message: &Message,
    style: Style,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    match message {
        // The notes a compaction left standing, under a line saying whose words
        // they are, and through the same door the model's prose goes through —
        // because that is what they are.
        Message::User(said) if said.starts_with(RECAP) => {
            renderer.apart()?;
            renderer.present(&[Row::new().then(Slot::Quiet, clip(NOTES, columns))])?;
            renderer.stream(said.strip_prefix(RECAP).unwrap_or(said))?;
            renderer.settle()?;
        }

        // What was asked, in the row the box commits when it is typed: the mark,
        // the ground behind it, the break at the column rather than at a space.
        // A reader finds their own words the way they left them.
        Message::User(said) => draw::queued(renderer, said, style)?,

        Message::Agent { text, calls, stop } => {
            if !text.trim().is_empty() {
                renderer.apart()?;
                renderer.stream(text)?;
            }

            // Settled whether or not anything was said, because what follows is
            // presented, and a line still open is one the row under it would be
            // written into the middle of.
            renderer.settle()?;

            // The line the footing was drawing while the tool was out, with the
            // motion gone — which is the line that joined the transcript when it
            // answered. What the call was about is asked of the tool that owns
            // the arguments, the same way it was asked the first time.
            for call in calls {
                draw::returned(renderer, &draw::called(call, &runner.about(call)), style)?;
            }

            // An answer that did not end the way the model meant it to is worth
            // the same line here it got the first time: a half answer read back
            // as a whole one is the one thing a transcript may not do.
            if let Some(said) = stop.and_then(draw::notice) {
                renderer.apart()?;
                renderer.commit(said)?;
            }
        }

        // Under the call line above it, which is where a reader asking what a
        // call did is already looking.
        Message::ToolResults(results) => {
            for result in results {
                renderer.present(&draw::finished_rows(&result.output, columns, style))?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Cancel, Effort, StopReason, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult, Transcript,
        Workspace,
    };
    use crucible_runner::{Model, Session, Tools};
    use crucible_tui::{Picture, Recording, Renderer};

    use crate::cli::fake::Script;

    use super::*;

    /// A runner with the real `read` tool on it, so what a call is about is
    /// answered by the tool that owns the arguments rather than invented here.
    fn resumed(transcript: Transcript) -> Runner {
        let mut offered = Tools::new();
        offered.add(Box::new(crucible_tools::Read::new(
            Workspace::open(std::env::current_dir().expect("a directory")).expect("a workspace"),
            Cancel::new(),
            crucible_tools::Ledger::default(),
        )));

        Runner::new(
            Box::new(Script::new(Vec::new())),
            offered,
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                system: None,
                effort: None::<Effort>,
            },
            Session::nowhere(),
        )
        .resuming(transcript)
    }

    /// A transcript with one of everything in it.
    fn everything() -> Transcript {
        let mut transcript = Transcript::new();
        transcript.push(Message::User(
            "read the config and tell me what it says".into(),
        ));
        transcript.push(Message::Agent {
            text: "I will look at it.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        });
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: ToolOutput::ok("theme = midnight\nand nine hundred lines after it"),
        }]));
        transcript.push(Message::Agent {
            text: "It sets the theme and nothing else.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        transcript
    }

    /// What a terminal `columns` wide is left holding, having replayed it.
    fn screen(transcript: Transcript, columns: usize) -> String {
        painted(transcript, columns, Style::plain())
    }

    /// The same, in `style` — and dressed in it, the way the run dresses the
    /// renderer once the style is settled. The markers in the model's markdown
    /// are read or left alone according to that, so a replay judged on a
    /// renderer nobody told would be judged with the colour switched off.
    fn painted(transcript: Transcript, columns: usize, style: Style) -> String {
        let runner = resumed(transcript);
        let mut renderer = Renderer::new(Recording::new(columns, 24));
        renderer.wears(style.palette());

        replayed(&mut renderer, &runner, style).expect("a recording cannot fail");

        renderer.terminal().written().to_string()
    }

    #[test]
    fn a_resumed_session_is_put_back_on_the_screen() {
        let screen = screen(everything(), 80);
        println!("\n{screen}");

        // What was asked and what was answered: those are the conversation, and
        // a reader picking it up is looking for both.
        assert!(screen.contains("read the config"), "{screen}");
        assert!(screen.contains("It sets the theme"), "{screen}");
        assert!(screen.contains(OPENED), "{screen}");
    }

    #[test]
    fn a_call_replays_as_the_line_it_was_drawn_as_rather_than_as_its_bare_name() {
        // Live, a call is the tool's name with what it was about beside it. A
        // session picked up has to show the same line, or it is a stranger's.
        let screen = screen(everything(), 80);
        println!("\n{screen}");

        assert!(
            screen.contains("Read("),
            "no arguments on the call: {screen}"
        );
        assert!(screen.contains("crucible.json"), "{screen}");
    }

    #[test]
    fn a_result_replays_as_the_rows_the_live_path_draws_for_it() {
        // Held to the live builder itself rather than to words copied out of
        // it: what this keeps true is that the two agree, and a second list of
        // expected strings here would be a second thing to keep in step.
        let output = ToolOutput::ok("theme = midnight\nand nine hundred lines after it");
        let live = draw::finished_rows(&output, 80, Style::plain());
        let screen = screen(everything(), 80);
        println!("\n{screen}");

        for row in live.iter().map(Row::text) {
            let row = row.trim_end();
            assert!(!row.is_empty() && screen.contains(row), "missing {row:?}");
        }
    }

    #[test]
    fn nothing_of_a_long_answer_goes_missing_on_the_way_back() {
        // A transcript put back with its right-hand edge cut off is one somebody
        // has to open the log to understand, which is the whole of what this
        // exists to save them.
        //
        // Counted a character at a time, because the answer is broken at the
        // column on its way down and a word count would be counting the breaks.
        let long = "x".repeat(300);
        let mut transcript = Transcript::new();
        transcript.push(Message::Agent {
            text: long.clone().into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });

        let screen = screen(transcript, 40);

        assert!(
            screen.matches('x').count() >= long.len(),
            "{} of {} came back",
            screen.matches('x').count(),
            long.len()
        );
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

        let screen = screen(transcript, 80);
        println!("\n{screen}");

        assert!(screen.contains(NOTES), "{screen}");
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

        assert!(screen(transcript, 80).contains("token ceiling"));
    }

    #[test]
    fn a_resumed_session_comes_back_in_the_colours_it_was_drawn_in() {
        // The whole of what drawing it through the live builders buys. A
        // transcript put back in the reader's foreground, or with the theme
        // they chose taken out of it, is a second answer to what a session
        // looks like — and the one they are looking at is the one that is
        // wrong.
        // Grounded rather than merely coloured: the mark in front of a prompt
        // and the ground behind it are worked out from the reader's own
        // background, and a palette that was never told one has nothing to
        // paint them with.
        let style = Style::grounded((12, 12, 12));
        let palette = style.palette();
        let screen = painted(everything(), 80, style);

        for (slot, text) in [
            (Slot::PromptMark, style.glyphs().caret()),
            (Slot::Accent, style.glyphs().called()),
            (Slot::Strong, "Read"),
            (Slot::Quiet, "(crucible.json)"),
        ] {
            let wanted = format!("{}{text}{}", palette.open(slot), palette.close());

            assert!(screen.contains(&wanted), "{screen:?} is missing {wanted:?}");
        }

        // And the ground behind what was asked, which is a slot rather than a
        // word: the band down the side of a prompt is what a reader picks their
        // own lines out by, and a transcript that came back without it is one
        // where nothing marks where they were.
        let ground = palette.open(Slot::Prompt).to_string();
        assert!(
            screen.contains(&ground),
            "nothing behind the prompt: {screen:?}"
        );
    }

    #[test]
    fn the_prose_of_a_resumed_session_is_read_as_the_markdown_it_is() {
        // Through the same door the live path streams it through, which is
        // what puts a heading in the weight a heading is drawn in. A transcript
        // put back as plain text is one where every answer the model formatted
        // reads as the markers it was formatted with.
        let style = Style::coloured();
        let mut transcript = Transcript::new();
        transcript.push(Message::Agent {
            text: "# Heading\n\nand a word.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });

        let screen = painted(transcript, 80, style);

        assert!(!screen.contains("# Heading"), "the markers are still in it");
        assert!(screen.contains("Heading"), "{screen:?}");
    }

    #[test]
    fn no_row_of_it_is_wider_than_the_terminal_it_was_drawn_for() {
        // The failure `responsive-components.md` is about: a row past the last
        // column is one the terminal wraps itself, so a band given one row is
        // written two and the band under it loses the first of its own.
        for columns in [40, 60, 80, 120] {
            let shown = Picture::of(&screen(everything(), columns), columns, 24);
            for row in shown.rows() {
                assert!(crucible_tui::columns(&row) <= columns, "{columns}: {row:?}");
            }
        }
    }
}
