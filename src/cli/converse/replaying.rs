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
//! Which goes for what a row is *offering* as well as for what it says. A result
//! too long for its row is cut here the way it was cut live and held where the
//! key over it can reach it, so a session put back on the screen is one whose
//! rows still light and still open. A row that behaved one way live and another
//! on the way back would be the same row behaving as two, and that is what a
//! reader picking a session up would find strange first.
//!
//! It adds nothing of its own. The screen was emptied before the walk starts,
//! so what a reader is left holding is the session as they left it — a heading
//! or a rule over it would mark a join that is not there, and they would scroll
//! into the marker in the middle of their own conversation.
//!
//! One thing does not come back, and it is the record's doing rather than this
//! module's: a diff reaches no log, for the reason `crucible-core` gives beside
//! the type, so a call that changed a file replays as the result it returned
//! rather than as the lines it moved.

use crucible_core::{Message, RECAP, Workspace};
use crucible_runner::Runner;
use crucible_tui::{Recording, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::draw;
use crate::cli::kept::Kept;
use crate::cli::style::Style;

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
    workspace: &Workspace,
    kept: &mut Kept,
    style: Style,
) -> Result<(), Fatal> {
    if runner.transcript().is_empty() {
        return Ok(());
    }

    walked(
        renderer,
        runner.transcript().messages(),
        &Replay {
            runner,
            workspace,
            style,
        },
        kept,
    )
}

/// The tail of a session nobody has picked up, drawn into rows `columns` wide.
///
/// The picker's preview, and the reason this is one function rather than two:
/// what a session looks like is answered here for the screen it is resumed on
/// and for the pane it is offered in, so the pane shows what pressing Enter
/// would leave the reader holding. The walk goes onto a renderer of its own,
/// which is a screen nobody sees and a width nobody set — the pane's, which the
/// reader can change under it — and what comes back out is the last `most` rows.
///
/// Bounded by `most` for the same reason the log is read from its end: a
/// preview is a glance, and one that kept every row of a long session would
/// spend a session's memory answering it.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the rows could not be drawn, which a recording does
/// not do.
pub(super) fn glimpsed(
    messages: &[Message],
    against: &Replay<'_>,
    columns: usize,
    most: usize,
) -> Result<Vec<Row>, Fatal> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }

    let mut renderer = Renderer::new(Recording::new(columns, most.max(1)));
    renderer.wears(against.style.palette());
    renderer.draws(against.style.glyphs());

    // A held of its own, dropped with the renderer: what a key would open is
    // the business of the session on the screen, and this one is not on it.
    walked(&mut renderer, messages, against, &mut Kept::default())?;

    Ok(renderer.tail(most))
}

/// The walk itself: every message put back, then the tail ended.
fn walked<T: Terminal>(
    renderer: &mut Renderer<T>,
    messages: &[Message],
    against: &Replay<'_>,
    kept: &mut Kept,
) -> Result<(), Fatal> {
    for message in messages {
        said(renderer, against, kept, message)?;
    }

    // Whatever the last message left live, ended: a session whose last turn was
    // the model talking leaves a tail in the region the renderer owns, and what
    // is said next belongs under it rather than in the middle of it.
    renderer.settle()?;

    Ok(())
}

/// What a whole replay is drawn against, and what does not change while it
/// runs: the session being put back, the root a file it named is named
/// against, and the dress the renderer is already wearing.
///
/// One value rather than three parameters carried down the walk — what changes
/// from one call to the next is the message, and this is everything that does
/// not. It is what [`glimpsed`] is handed too, for the same reason: a caller
/// drawing a session it is not in still has to say which build's tools are
/// being named and whose files a path in one is named against.
pub(super) struct Replay<'a> {
    /// The session in hand, for what each tool's call line reads as.
    pub(super) runner: &'a Runner,
    /// The root a file a call named is named against.
    pub(super) workspace: &'a Workspace,
    /// The dress the rows are drawn in.
    pub(super) style: Style,
}

/// One message, put back the way it went down.
///
/// The arms are in the order a turn produces them, which is the order the
/// transcript holds them in — so walking it hands the renderer the same calls
/// in the same order the turn did, and the picture is the picture.
fn said<T: Terminal>(
    renderer: &mut Renderer<T>,
    against: &Replay<'_>,
    kept: &mut Kept,
    message: &Message,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let style = against.style;

    match message {
        // The notes a compaction left standing, under a line saying whose words
        // they are, and through the same door the model's prose goes through —
        // because that is what they are.
        Message::User { text: said, .. } if said.starts_with(RECAP) => {
            renderer.apart()?;
            renderer.present(&[Row::new().then(Slot::Quiet, clip(NOTES, columns))])?;
            renderer.stream(said.strip_prefix(RECAP).unwrap_or(said))?;
            renderer.settle()?;
        }

        // What was asked, in the row the box commits when it is typed: the mark,
        // the ground behind it, the break at the column rather than at a space.
        // A reader finds their own words the way they left them.
        Message::User {
            text: said,
            attachments,
        } => {
            draw::queued(renderer, said, style)?;
            draw::attached(renderer, attachments, against.workspace, style)?;
        }

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
                let line = draw::called(call, &against.runner.about(call));

                // Named before the row that answers it goes down, the same way
                // the turn named it: the expansion carries the call's line, and
                // a result whose call was never named would open under a heading
                // nobody wrote.
                kept.calling(call.id.clone(), line.clone());
                draw::returned(renderer, &line, style)?;
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
                // Through the door the turn drew it through, which is what makes
                // the row that comes back the row that went down: lit where it
                // was cut, and holding the lines it was cut from where the key
                // over it can reach them.
                // Copied rather than moved, which is the one thing this path
                // does that the turn's did not: the transcript owns this result
                // and goes on being sent, so what is held for the key to open is
                // a second copy of one result. Bounded by the tool that made it
                // and by the ceiling the record keeps, so what a replay costs is
                // the same after four hundred messages as after four.
                draw::came_back(renderer, kept, &result.id, result.output.clone(), style)?;
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
    use crucible_tui::Picture;

    use crate::cli::fake::Script;
    use crate::cli::kept::Whole;

    use super::*;

    /// The directory the run is in, which is what a replayed path is named
    /// against. Nothing here attaches a file, so it is only ever the root a
    /// name is measured from.
    fn here() -> Workspace {
        Workspace::open(std::env::current_dir().expect("a directory")).expect("a workspace")
    }

    /// What a session is drawn against in these tests: this build's tools, this
    /// directory, and no theme at all.
    fn against<'a>(runner: &'a Runner, workspace: &'a Workspace) -> Replay<'a> {
        Replay {
            runner,
            workspace,
            style: Style::plain(),
        }
    }

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
                accepts: None,
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
        transcript.push(Message::said("read the config and tell me what it says"));
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

        replayed(&mut renderer, &runner, &here(), &mut Kept::default(), style)
            .expect("a recording cannot fail");

        renderer.terminal().written().to_string()
    }

    /// What a replay left held, and the renderer it drew onto.
    fn holding(transcript: Transcript, columns: usize) -> (Kept, Renderer<Recording>) {
        let runner = resumed(transcript);
        let mut kept = Kept::default();
        let mut renderer = Renderer::new(Recording::new(columns, 24));
        renderer.wears(Style::plain().palette());

        replayed(&mut renderer, &runner, &here(), &mut kept, Style::plain())
            .expect("a recording cannot fail");

        (kept, renderer)
    }

    #[test]
    fn a_result_the_replay_had_to_cut_is_one_the_key_over_it_still_opens() {
        // The row says how many lines it could not fit and names the key that
        // gives them back. Live, pressing it works; replayed, it used to name a
        // key with nothing behind it — the same row, offering something only one
        // of the two paths could deliver.
        let (kept, _) = holding(everything(), 80);

        let whole = kept.newest().next().expect("the result that was cut");
        assert!(
            whole.text().contains("nine hundred lines after it"),
            "{:?}",
            whole.text()
        );
        assert!(
            whole.called().contains("crucible.json"),
            "the call it opens under: {:?}",
            whole.called()
        );
    }

    #[test]
    fn the_row_a_replayed_result_was_cut_on_is_the_row_a_click_lands_on() {
        // The other half of the offer, and the half a pointer uses: a click
        // becomes a row of the record, and a row of the record has to become
        // this. Off by a row and the reader opens the result above the one they
        // pointed at.
        let (kept, _) = holding(everything(), 80);

        let at = kept
            .newest()
            .next()
            .and_then(Whole::at)
            .expect("a row the offer went on");

        assert!(kept.offered(at), "row {at} made no offer");
    }

    #[test]
    fn a_result_that_fitted_leaves_nothing_behind_to_be_opened() {
        // The rule the live path keeps, kept here too: an offer to expand a
        // result the row said the whole of is an offer to show somebody what
        // they are looking at.
        let mut transcript = Transcript::new();
        transcript.push(Message::Agent {
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        });
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: ToolOutput::ok("one line and no more"),
        }]));

        let (kept, _) = holding(transcript, 80);

        assert!(kept.is_empty());
    }

    #[test]
    fn a_session_glimpsed_is_drawn_the_way_picking_it_up_would_draw_it() {
        // The picker's preview is this walk on a renderer of its own, so the
        // rows it shows are the rows the screen would hold — the prompt's mark,
        // the call line, the row the result came back on, the model's prose
        // through the same markdown. A preview built from a second set of
        // builders would be a second answer to what a session looks like.
        let runner = resumed(everything());
        let mut renderer = Renderer::new(Recording::new(60, 24));
        renderer.wears(Style::plain().palette());
        replayed(
            &mut renderer,
            &runner,
            &here(),
            &mut Kept::default(),
            Style::plain(),
        )
        .expect("a recording cannot fail");
        let live: Vec<String> = renderer.tail(64).iter().map(Row::text).collect();

        let transcript = everything();
        let shown = glimpsed(transcript.messages(), &against(&runner, &here()), 60, 64)
            .expect("a recording cannot fail");

        assert_eq!(
            shown.iter().map(Row::text).collect::<Vec<_>>(),
            live,
            "the preview and the resume drew the same session differently"
        );
        assert!(
            live.iter().any(|row| row.contains("read the config")),
            "nothing was drawn at all: {live:?}"
        );
    }

    #[test]
    fn a_glimpse_is_drawn_against_the_width_the_pane_has() {
        // The pane is half a window that the reader can resize under it, so
        // what fits is answered at the width being drawn at rather than once.
        let runner = resumed(everything());
        let transcript = everything();

        for columns in [30, 48, 96] {
            let shown = glimpsed(
                transcript.messages(),
                &against(&runner, &here()),
                columns,
                64,
            )
            .expect("a recording cannot fail");

            for row in &shown {
                assert!(
                    crucible_tui::columns(&row.text()) <= columns,
                    "a row {} wide in {columns} columns: {:?}",
                    crucible_tui::columns(&row.text()),
                    row.text()
                );
            }
        }
    }

    #[test]
    fn a_glimpse_keeps_no_more_rows_than_it_was_asked_for() {
        // A log is read from its end under a ceiling, and what is drawn from it
        // is bounded the same way: a pane cannot show more than a window of
        // rows, and a preview that kept every row of a long session would spend
        // a session's memory on a glance.
        let runner = resumed(everything());
        let mut transcript = Transcript::new();
        for nth in 0..200 {
            transcript.push(Message::said(format!("question {nth}").as_str()));
        }

        let shown = glimpsed(transcript.messages(), &against(&runner, &here()), 60, 32)
            .expect("a recording cannot fail");

        assert!(shown.len() <= 32, "{} rows kept", shown.len());
        assert!(
            shown.iter().any(|row| row.text().contains("question 199")),
            "the end of the session is what a preview is for"
        );
    }

    #[test]
    fn a_resumed_session_is_put_back_on_the_screen() {
        let screen = screen(everything(), 80);
        println!("\n{screen}");

        // What was asked and what was answered: those are the conversation, and
        // a reader picking it up is looking for both.
        assert!(screen.contains("read the config"), "{screen}");
        assert!(screen.contains("It sets the theme"), "{screen}");
    }

    #[test]
    fn nothing_marks_the_replay_as_a_replay() {
        // A session picked up is the session, not a quotation of it. The screen
        // was emptied before this went down, so a heading or a rule saying
        // where the old session stops would be marking a join that is not
        // there — and the reader would scroll into it in the middle of their
        // own conversation.
        let screen = screen(everything(), 80);

        assert!(
            !screen.contains("picking up where this left off"),
            "{screen}"
        );
        assert!(
            !screen.contains(&Style::plain().glyphs().horizontal().repeat(80)),
            "a rule across the window: {screen}"
        );
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
        transcript.push(Message::said(format!(
            "{RECAP}what was decided, and what is left"
        )));

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
