//! What a session picked up puts back on the screen.

use std::sync::Arc;

use crucible_core::{
    AgentId, Cancel, Effort, Fetch, Host, Page, RecordedToolOutput, Search, SearchResponse,
    SourceError, StopReason, ToolArgs, ToolCall, ToolId, ToolResult, Transcript, Workspace,
};
use crucible_runner::{Agent, Model, Tools};
use crucible_tui::Picture;

use crate::cli::fake::Script;
use crate::cli::kept::Whole;

use super::*;

/// What a session is drawn against in these tests: this build's tools and
/// no theme at all.
fn against<'a>(runner: &'a Runner, pruned: &'a Pruned) -> Replay<'a> {
    Replay {
        runner,
        pruned,
        style: Style::plain(),
    }
}

/// A search that never answers, for a tool whose call lines are all that
/// is asked of it.
struct Nowhere;

impl Search for Nowhere {
    fn name(&self) -> &'static str {
        "nowhere"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search(&self, _: &str, _: &Cancel) -> Result<SearchResponse, SourceError> {
        Ok(SearchResponse::results(Vec::new()))
    }
}

impl Fetch for Nowhere {
    fn name(&self) -> &'static str {
        "nowhere"
    }
    fn reaches(&self, url: &str) -> Host {
        Host::Named {
            sent: url.into(),
            host: "example.com".into(),
        }
    }
    fn fetch(&self, url: &str, _: &Cancel) -> Result<Page, SourceError> {
        Ok(Page {
            url: url.into(),
            title: None,
            text: "page".into(),
        })
    }
}

/// A runner with the real `read` tool on it, so what a call is about is
/// answered by the tool that owns the arguments rather than invented here.
/// And the real `web_search`, held back the way the build holds it back:
/// a session put back on the screen has to name calls to tools the model
/// looked up in a turn that is over.
fn resumed(transcript: Transcript) -> Runner {
    let mut offered = Tools::new();
    offered
        .add_builtin(crucible_builtins::Read::new(
            Workspace::open(std::env::current_dir().expect("a directory")).expect("a workspace"),
            crucible_builtins::Ledger::default(),
        ))
        .unwrap();
    offered
        .defer_builtin(crucible_builtins::WebSearch::new(std::sync::Arc::new(
            Nowhere,
        )))
        .unwrap();

    offered
        .defer_builtin(crucible_builtins::WebFetch::new(std::sync::Arc::new(
            Nowhere,
        )))
        .unwrap();

    Runner::new(
        Box::new(Script::new(Vec::new())),
        offered,
        Agent::new(
            AgentId::new("test"),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None::<Effort>,
            },
        ),
        crucible_context::ContextInputs::new(std::env::temp_dir()),
        Arc::new(Session::nowhere()),
    )
    .resuming(transcript)
}

#[test]
fn legacy_reset_preserves_visible_history_and_opening() {
    use std::io::Write as _;
    let sample = crate::cli::sample::Sample::new("replay-legacy-reset");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    for message in everything().messages() {
        session.append(message);
    }
    session.append(&Message::said("before legacy reset"));
    let path = session.path().to_path_buf();
    assert!(session.finish().is_none());
    // Finishing ends the recording; the claim on the file goes with the
    // last holder of the session, and the resume below is another holder
    // asking for it.
    drop(session);
    let mut log = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(log, "{{\"forgotten\":true}}").unwrap();
    drop(log);
    let (session, model) = Session::resume(&sample.logs(), &sample.workspace()).unwrap();
    assert!(model.messages().is_empty());
    session.append(&Message::said("after legacy reset"));
    let history = session.display_history().unwrap().unwrap();
    let runner = resumed(model);
    let mut renderer = Renderer::new(Recording::new(100, 30));
    renderer.commit("opening card").unwrap();
    let mut kept = Kept::default();
    streamed(
        &mut renderer,
        history,
        &against(&runner, &Pruned::default()),
        &Session::nowhere(),
        &mut kept,
    )
    .unwrap();
    let visible = renderer
        .tail(100)
        .iter()
        .map(Row::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("opening card"), "{visible}");
    assert!(visible.contains("before legacy reset"), "{visible}");
    assert!(visible.contains("after legacy reset"), "{visible}");
    assert!(
        kept.newest()
            .any(|whole| whole.text().contains("nine hundred lines after it")),
        "historical expansion survives model reset"
    );
}

/// A transcript with one of everything in it.
fn everything() -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("read the config and tell me what it says"))
        .expect("valid fixture transcript");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "I will look at it.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: RecordedToolOutput::ok("theme = midnight\nand nine hundred lines after it"),
        }]))
        .expect("valid fixture transcript");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "It sets the theme and nothing else.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");
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
    let session = Session::nowhere();
    let mut renderer = Renderer::new(Recording::new(columns, 24));
    renderer.wears(style.palette());

    replayed(
        &mut renderer,
        &Replay {
            runner: &runner,
            pruned: &Pruned::default(),
            style,
        },
        &session,
        &mut Kept::default(),
    )
    .expect("a recording cannot fail");

    renderer.terminal().written().to_string()
}

/// What a replay left held, and the renderer it drew onto.
fn holding(transcript: Transcript, columns: usize) -> (Kept, Renderer<Recording>) {
    let runner = resumed(transcript);
    let mut kept = Kept::default();
    let mut renderer = Renderer::new(Recording::new(columns, 24));
    renderer.wears(Style::plain().palette());

    replayed(
        &mut renderer,
        &against(&runner, &Pruned::default()),
        &Session::nowhere(),
        &mut kept,
    )
    .expect("a recording cannot fail");

    (kept, renderer)
}

/// A turn that read `count` files, one call and one result to a message,
/// the way a model walking a tree writes them.
/// A turn that asked for `count` reads at once and was answered.
///
/// One batch rather than one call per exchange, because a run is one round
/// trip at most: a model that wants four files asks for all four in one
/// response, and a fixture that asked for them one at a time would be
/// four runs of one and would fold nothing.
fn walked_a_tree(count: usize) -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("what is in here?"))
        .expect("valid fixture transcript");

    let ids: Vec<ToolId> = (1..=count)
        .map(|one| ToolId::new(format!("c-{one}")))
        .collect();

    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: ids
                .iter()
                .enumerate()
                .map(|(at, id)| ToolCall {
                    id: id.clone(),
                    name: "read".into(),
                    args: ToolArgs::new(format!(r#"{{"path":"file-{}.rs"}}"#, at + 1)),
                })
                .collect(),
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");

    transcript
        .push(Message::ToolResults(
            ids.iter()
                .enumerate()
                .map(|(at, id)| ToolResult {
                    id: id.clone(),
                    output: RecordedToolOutput::ok(format!(
                        "line one of {}\nand nine hundred after it",
                        at + 1
                    )),
                })
                .collect(),
        ))
        .expect("valid fixture transcript");

    transcript
}

#[test]
fn a_run_of_calls_that_only_looked_around_replays_as_the_line_it_settled_into() {
    // The picture a reader left is the picture they come back to. A turn
    // that folded three reads into one line while it ran may not put three
    // rows back on the screen when the session is picked up.
    let screen = screen(walked_a_tree(3), 80);

    assert!(screen.contains("Read 3 files"), "{screen:?}");
    assert!(!screen.contains("Read(file-1.rs)"), "{screen:?}");
}

#[test]
fn a_call_that_looked_around_alone_replays_as_the_row_it_always_had() {
    // One call is not a run. It went down as its own row live and it comes
    // back as its own row, because a count of one says less than the name
    // it replaced.
    let screen = screen(walked_a_tree(1), 80);

    assert!(screen.contains("Read(file-1.rs)"), "{screen:?}");
    assert!(!screen.contains("Read 1 file"), "{screen:?}");
}

#[test]
fn a_failed_lookup_is_not_hidden_in_a_replayed_group() {
    let source = walked_a_tree(3);
    let mut transcript = Transcript::new();
    for mut message in source.messages().iter().cloned() {
        if let Message::ToolResults(results) = &mut message {
            results.get_mut(1).unwrap().output = RecordedToolOutput::failed("file missing");
        }
        transcript.push(message).unwrap();
    }
    let screen = screen(transcript, 80);
    assert!(screen.contains("file missing"), "{screen}");
    assert!(!screen.contains("Read 3 files"), "{screen}");
}

#[test]
fn web_research_replays_as_one_clickable_group_without_an_expansion_hint() {
    let source = walked_a_tree(4);
    let mut transcript = Transcript::new();
    for mut message in source.messages().iter().cloned() {
        if let Message::Agent { calls, .. } = &mut message {
            for (at, call) in calls.iter_mut().enumerate() {
                let (name, args) = if at < 2 {
                    ("web_search", r#"{"query":"rust reference"}"#)
                } else {
                    ("web_fetch", r#"{"url":"https://example.com/reference"}"#)
                };
                call.name = name.into();
                call.args = ToolArgs::new(args);
            }
        }
        transcript.push(message).unwrap();
    }
    let (kept, renderer) = holding(transcript, 100);
    let picture = renderer.terminal().picture().said().join("\n");
    assert!(
        picture.contains("Searched the web 2 times, fetched 2 pages"),
        "{picture}"
    );
    assert!(!picture.contains("ctrl+o"), "{picture}");
    let rows: Vec<_> = kept.newest().map(Whole::at).collect();
    assert_eq!(rows.len(), 4);
    let row = rows.first().copied().flatten().unwrap();
    assert!(rows.iter().all(|at| *at == Some(row)));
    assert!(kept.offered(row));
}

#[test]
fn every_result_in_a_replayed_run_opens_from_the_line_that_folded_it() {
    // Folding is about rows scrolled past and never about what is still
    // reachable, so all three results answer to the one line, and opening
    // it opens all of them.
    let (kept, _) = holding(walked_a_tree(3), 80);
    let rows: Vec<Option<usize>> = kept.newest().map(Whole::at).collect();
    let first = rows.first().copied().flatten().expect("a row for the run");

    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.iter().all(|at| *at == Some(first)), "{rows:?}");
    assert!(kept.offered(first), "{rows:?}");
}

#[test]
fn a_result_a_pruning_cleared_replays_as_what_the_reader_was_shown() {
    // The two halves of the same row, and they answer to different owners.
    // The transcript holds the placeholder, because that is what the model
    // is being sent and a resumed session may not undo the pruning that
    // made room for it. The screen holds the words, because the reader
    // watched them come back and a session picked up is meant to look like
    // the session they left.
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("read the config and tell me what it says"))
        .expect("valid fixture transcript");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "I will look at it.".into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: RecordedToolOutput::ok("[cleared to make room — 4096 bytes]"),
        }]))
        .expect("valid fixture transcript");

    let mut pruned = Pruned::default();
    pruned.keep(ToolId::new("c-1"), "theme = midnight".to_owned());

    let runner = resumed(transcript);
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer.wears(Style::plain().palette());
    replayed(
        &mut renderer,
        &against(&runner, &pruned),
        &Session::nowhere(),
        &mut Kept::default(),
    )
    .expect("a recording cannot fail");

    let shown = renderer.terminal().written().to_string();
    assert!(
        shown.contains("theme = midnight"),
        "the row forgot what the reader was shown: {shown}"
    );
    assert!(
        !shown.contains("cleared to make room"),
        "the row said out loud what the model is being sent instead: {shown}"
    );
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
    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("c-1"),
                name: "read".into(),
                args: ToolArgs::new(r#"{"path":"crucible.json"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("c-1"),
            output: RecordedToolOutput::ok("one line and no more"),
        }]))
        .expect("valid fixture transcript");

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
        &against(&runner, &Pruned::default()),
        &Session::nowhere(),
        &mut Kept::default(),
    )
    .expect("a recording cannot fail");
    let live: Vec<String> = renderer.tail(64).iter().map(Row::text).collect();

    let transcript = everything();
    let shown = glimpsed(
        transcript.messages(),
        &against(&runner, &Pruned::default()),
        60,
        64,
    )
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
            &against(&runner, &Pruned::default()),
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
        transcript
            .push(Message::said(format!("question {nth}").as_str()))
            .expect("valid fixture transcript");
    }

    let shown = glimpsed(
        transcript.messages(),
        &against(&runner, &Pruned::default()),
        60,
        32,
    )
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

/// Two single searches in separate round trips: each keeps its own row.
fn searched_twice() -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("what are people saying?"))
        .unwrap();
    for (number, question, answer) in [
        (1, "first question", "First answer"),
        (2, "second question", "Second answer"),
    ] {
        let id = ToolId::new(format!("s-{number}"));
        transcript
            .push(Message::Agent {
                continuation: None,
                text: String::new().into(),
                calls: vec![ToolCall {
                    id: id.clone(),
                    name: "web_search".into(),
                    args: ToolArgs::new(serde_json::json!({"query":question}).to_string()),
                }],
                stop: Some(StopReason::WantsTools),
            })
            .unwrap();
        transcript
            .push(Message::ToolResults(vec![ToolResult {
                id,
                output: RecordedToolOutput::ok(format!("1. {answer}\n   https://example.com\n")),
            }]))
            .unwrap();
    }
    transcript
}

#[test]
fn a_call_to_a_tool_held_back_replays_with_what_it_was_about() {
    // The model looked the tool up in a turn that is over, and the fresh
    // session has not looked it up again. The call still happened, and it
    // was still about something -- a bare name is what a refused call
    // looks like, and this one was answered.
    let screen = screen(searched_twice(), 80);
    println!("\n{screen}");

    assert!(screen.contains("first question"), "{screen}");
    assert!(screen.contains("second question"), "{screen}");
}

#[test]
fn each_replayed_call_stands_directly_over_its_own_result() {
    // Live, a call's line joins the transcript when it answers, with the
    // result under it. Two lines and then two results would be a picture
    // nobody watched, and would hang the second result under the wrong
    // question.
    let screen = screen(searched_twice(), 80);
    println!("\n{screen}");

    let at = |needle: &str| screen.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at("first question") < at("First answer"), "{screen}");
    assert!(at("First answer") < at("second question"), "{screen}");
    assert!(at("second question") < at("Second answer"), "{screen}");
}

#[test]
fn a_call_the_log_never_answered_still_goes_down() {
    // The session ended while the call was out. The line the reader
    // watched go up is still part of what they left, so it comes back.
    let mut transcript = everything();
    transcript
        .push(Message::said("what are people saying?"))
        .expect("valid fixture transcript");
    transcript
        .push(Message::Agent {
            continuation: None,
            text: String::new().into(),
            calls: vec![ToolCall {
                id: ToolId::new("s-1"),
                name: "web_search".into(),
                args: ToolArgs::new(r#"{"query":"an open question"}"#),
            }],
            stop: Some(StopReason::WantsTools),
        })
        .expect("valid fixture transcript");
    let (kept, renderer) = holding(transcript, 80);
    let screen = renderer.terminal().written();
    assert!(screen.contains("an open question"), "{screen}");
    assert!(
        kept.heading(&ToolId::new("s-1")).is_none(),
        "historical calls are no longer pending"
    );
    assert!(
        kept.newest()
            .any(|whole| whole.text().contains("nine hundred lines after it")),
        "completed expansions survive"
    );
}

#[test]
fn a_result_replays_as_the_rows_the_live_path_draws_for_it() {
    // Held to the live builder itself rather than to words copied out of
    // it: what this keeps true is that the two agree, and a second list of
    // expected strings here would be a second thing to keep in step.
    let output = draw::Shown::replayed(
        RecordedToolOutput::ok("theme = midnight\nand nine hundred lines after it"),
        None,
    );
    let live = draw::finished_rows(&output, 80, Style::plain(), false);
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
    transcript
        .push(Message::Agent {
            continuation: None,
            text: long.clone().into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");

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
    transcript
        .push(Message::said(format!(
            "{RECAP}what was decided, and what is left"
        )))
        .expect("valid fixture transcript");

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
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "half a th".into(),
            calls: Vec::new(),
            stop: Some(StopReason::OutOfTokens),
        })
        .expect("valid fixture transcript");

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
    transcript
        .push(Message::Agent {
            continuation: None,
            text: "# Heading\n\nand a word.".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        })
        .expect("valid fixture transcript");

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
