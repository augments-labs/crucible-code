//! `/clear`: starting a new session, and leaving this one where `/resume` can
//! find it.
//!
//! An empty context is what is asked for, and the shortest way to one is a
//! session that has said nothing yet. So this is [`resume`] with the session
//! swapped: a fresh log rather than a reopened one, handed to the same
//! [`Runner::pick_up`], which is also what drops the permission answers given
//! for the rest of a session that is now over. The record of what has been read
//! is emptied here for the same reason, exactly as `/resume` empties it, and so
//! is the plan the panel above the box is drawn from.
//!
//! What was said is not deleted. The log it was written to is closed and stays
//! on the disk, so the session is on `/resume`'s list like any other and
//! nothing about this command is destructive — which is why it asks nothing
//! before running.
//!
//! [`resume`]: super::resume

use crucible_core::Transcript;
use crucible_runner::{Runner, Session};
use crucible_tui::{Glyphs, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;

use super::Terms;

/// Runs it.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    terms: &Terms,
) -> Result<(), Fatal> {
    let columns = renderer.columns();
    let held = runner.transcript().len();

    // A session that has said nothing is already the empty one this would go
    // and open. Starting a second log here would leave two files for a session
    // that never happened, and `--continue` offers the newer of them.
    if held == 0 {
        let rows = [Row::new().then(Slot::Quiet, clip("nothing had been said", columns))];
        return Ok(renderer.present(&rows, terms.style().palette())?);
    }

    let session = match Session::start(&terms.sessions, &terms.workspace) {
        Ok(session) => session,
        // A path is in every one of these, so it is committed rather than
        // presented — the same as `/resume`'s. Nothing else changes: the
        // session in hand is still being recorded, and the loop carries on
        // with it.
        Err(problem) => return Ok(renderer.commit(&format!("! {problem}"))?),
    };

    let left = runner.pick_up(session, Transcript::new());

    // The files remembered were read by a session this is no longer in, and
    // `write` replaces a file on the strength of that record. Emptying it costs
    // a read; leaving it standing would cost the file.
    terms.ledger.forget();

    // And the plan for the same reason said the other way round: it costs
    // nothing to leave, and what it would leave is a panel above the prompt
    // listing work the agent under it has no memory of.
    terms.plan.forget();

    // The tools looked up belong to the conversation that looked them up. Left
    // standing they would be advertised to a session that never asked, which is
    // the schema cost this whole mechanism exists to avoid — paid, and for a
    // model with no memory of why.
    terms.revealed.forget();

    // The last chance to say that the log of the session being left stopped
    // being written. After this there is no session to say it about.
    if let Some(problem) = left.finish() {
        renderer.commit(&format!("! {problem}"))?;
    }

    renderer.present(
        &started(held, columns, terms.style().glyphs()),
        terms.style().palette(),
    )?;
    Ok(())
}

/// What `/clear` says, having left `held` messages behind.
///
/// Three rows for three things a user would otherwise have to find out. The
/// first is what happened. The second is that nothing was destroyed, and where
/// it went — this command reads like a delete and is not one. The third is that
/// what is above the box is the terminal's scrollback and stays exactly where
/// it is: this program never took that screen and is not about to hand it back
/// empty.
fn started(held: usize, columns: usize, glyphs: Glyphs) -> Vec<Row> {
    let said = match held {
        1 => "1 message".to_owned(),
        _ => format!("{held} messages"),
    };

    vec![
        Row::new().then(Slot::Plain, clip("started a new session", columns)),
        Row::new().then(
            Slot::Quiet,
            clip(
                &format!(
                    "{said} left behind {} /resume picks that session up again",
                    glyphs.dot()
                ),
                columns,
            ),
        ),
        Row::new().then(
            Slot::Quiet,
            clip("what is on screen stays where it is", columns),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crucible_auth::Store;
    use crucible_core::{Cancel, Message, Revealed, StopReason, ToolArgs, Transcript};
    use crucible_runner::{Model, Runner, Session, Tools, recent};
    use crucible_tools::{Ledger, Plan};
    use crucible_tui::{Glyphs, Recording, Renderer};

    use crate::cli::Fatal;
    use crate::cli::fake::Script;
    use crate::cli::sample::Sample;
    use crate::cli::style::Style;

    use super::super::Terms;
    use super::{run, started};

    /// What a list of rows says, row by row.
    fn art(rows: &[crucible_tui::Row]) -> Vec<String> {
        rows.iter().map(crucible_tui::Row::text).collect()
    }

    /// The terms a clear is run under: a session directory of the sample's own,
    /// and the two things the tools of such a run would have been built with.
    fn terms(sample: &Sample, ledger: &Ledger, plan: &Plan) -> Terms {
        Terms {
            style: std::cell::Cell::new(Style::plain()),
            chosen: std::cell::Cell::new(None),
            reading: std::cell::RefCell::default(),
            cancel: Cancel::new(),
            steer: crucible_core::Steer::new(),
            ledger: ledger.clone(),
            revealed: Revealed::new(),
            plan: plan.clone(),
            putting: crate::cli::seen::Putting::new(),
            leaving: crucible_tools::Background::new(),
            provider: std::cell::Cell::new(Some("anthropic")),
            choosing: sample.root().join("unwritten-home.json"),
            logins: Store::in_home(&sample.root()),
            subscriptions: crate::cli::subscription::Subscriptions::production(),

            // `/clear` never reaches it, and these terms have no provider to
            // build one from either — the loop they drive answers from a
            // script.
            serving: Box::new(|named, _| {
                Err(Fatal::Provider {
                    named: named.name.into(),
                })
            }),
            sessions: sample.logs(),
            workspace: sample.workspace(),
            sending: crucible_tui::Sending::default(),
        }
    }

    /// A runner recording to a session of `sample`'s, holding one exchange.
    ///
    /// The log and the transcript hold the same two messages, because that is
    /// what a session that took a turn looks like: what `/clear` leaves behind
    /// is read back off the disk, and a log the transcript disagrees with would
    /// prove nothing about either.
    fn talking(sample: &Sample, asked: &str) -> Runner {
        let answered = Message::Agent {
            text: "an answer".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        };

        let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
        session.append(&Message::User(asked.into()));
        session.append(&answered);

        let mut transcript = Transcript::new();
        transcript.push(Message::User(asked.into()));
        transcript.push(answered);

        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                system: None,
                effort: None,
            },
            session,
        )
        .resuming(transcript)
    }

    /// Runs `/clear` against `runner`, and says what reached the terminal.
    fn clearing(terms: &Terms, runner: &mut Runner) -> String {
        let mut renderer = Renderer::new(Recording::new(80, 24));

        run(&mut renderer, runner, terms).expect("the terminal to be written");

        renderer.terminal().written().to_string()
    }

    /// How long the list is given to hold the session that was left.
    ///
    /// The same wait `/resume`'s tests take, and for the same reason: a session
    /// reaches the list when its first prompt reaches its log, and that log is
    /// written by the thread that owns its queue.
    const SETTLING: Duration = Duration::from_secs(5);

    /// What `/resume` would list for `sample`, once `of` sessions are on it.
    fn listed(sample: &Sample, of: usize) -> Vec<String> {
        let since = Instant::now();

        loop {
            let found = recent(&sample.logs(), &sample.workspace(), 9);

            if found.len() == of {
                return found
                    .iter()
                    .map(|session| session.asked().to_owned())
                    .collect();
            }

            assert!(
                since.elapsed() < SETTLING,
                "{} of {of} sessions reached the list",
                found.len()
            );

            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn clearing_starts_a_new_session_and_leaves_the_old_one_where_it_can_be_found() {
        // The whole of what `/clear` is now: the session in hand is not the one
        // it was, what the model is told is nothing, and what was said is still
        // somewhere — which is the one outcome this command must never lose.
        let sample = Sample::new("clear-starts-a-session");
        let mut runner = talking(&sample, "what was said before");
        let left = runner.session().id().cloned().expect("a recorded session");

        clearing(&terms(&sample, &Ledger::new(), &Plan::new()), &mut runner);

        let now = runner.session().id().cloned().expect("a recorded session");
        assert_ne!(now, left, "the session in hand is the one that was left");
        assert_eq!(runner.transcript().len(), 0, "the transcript came with it");
        assert_eq!(listed(&sample, 1), ["what was said before"]);
    }

    #[test]
    fn a_plan_written_before_a_clear_is_not_standing_over_the_session_after_it() {
        // The panel above the box is drawn from this, and the tasks in it were
        // written by a session that is now over. Left standing, it would list
        // work above a prompt whose agent has never heard of any of it.
        let sample = Sample::new("clear-forgets-the-plan");
        let mut runner = talking(&sample, "what was said before");
        let plan = Plan::new();

        plan.replay(&ToolArgs::new(
            r#"{"tasks":[{"task":"Write the contributor guide","state":"doing"}]}"#,
        ));
        assert_eq!(plan.tasks().len(), 1);

        clearing(&terms(&sample, &Ledger::new(), &plan), &mut runner);

        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn what_was_said_before_a_clear_comes_back_when_that_session_is_picked_up() {
        // The log the clear left is closed rather than abandoned: an unfinished
        // one still holds a claim, and `/resume` would refuse it as open in
        // another crucible — which names this crucible.
        let sample = Sample::new("clear-then-resume");
        let mut runner = talking(&sample, "what was said before");
        let terms = terms(&sample, &Ledger::new(), &Plan::new());

        clearing(&terms, &mut runner);
        assert_eq!(listed(&sample, 1), ["what was said before"]);

        let mut renderer = Renderer::new(Recording::new(80, 24));
        super::super::resume::run("1", &mut renderer, &mut runner, &terms, false)
            .expect("the terminal to be written");

        let written = renderer.terminal().written().to_string();
        assert!(written.contains("what was said before"), "{written}");
        assert_eq!(runner.transcript().len(), 2, "{written}");
    }

    #[test]
    fn a_session_that_said_nothing_is_left_where_it_is() {
        // Starting a second log here would leave the first one empty and the
        // second one about to be, which is two files for a session that never
        // happened -- and `--continue` picks the newest of them.
        let sample = Sample::new("clear-said-nothing");
        let session = Session::start(&sample.logs(), &sample.workspace()).expect("a new session");
        let held = session.id().cloned().expect("a recorded session");
        let mut runner = Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                system: None,
                effort: None,
            },
            session,
        );

        let written = clearing(&terms(&sample, &Ledger::new(), &Plan::new()), &mut runner);

        assert!(written.contains("nothing had been said"), "{written}");
        assert_eq!(runner.session().id(), Some(&held), "{written}");
    }

    #[test]
    fn a_new_session_that_cannot_be_started_leaves_the_one_in_hand_running() {
        // The session being recorded is the one thing a failure here must not
        // cost. Reported the way every other path with a filename in it is, and
        // the loop carries on with the session it had.
        let sample = Sample::new("clear-cannot-start");
        let mut runner = talking(&sample, "what was said before");
        let held = runner.session().id().cloned().expect("a recorded session");

        let blocked = sample.root().join("not-a-directory");
        std::fs::write(&blocked, "").expect("a file where a directory is wanted");
        let terms = Terms {
            sessions: blocked,
            ..terms(&sample, &Ledger::new(), &Plan::new())
        };

        let written = clearing(&terms, &mut runner);

        assert!(written.contains("! "), "{written}");
        assert!(!written.contains("started a new session"), "{written}");
        assert_eq!(runner.session().id(), Some(&held), "{written}");
    }

    #[test]
    fn what_it_says_is_that_a_session_started_and_where_the_last_one_went() {
        assert_eq!(
            art(&started(12, 70, Glyphs::Unicode)),
            [
                "started a new session",
                "12 messages left behind · /resume picks that session up again",
                "what is on screen stays where it is",
            ]
        );

        // One of them is one message. A count is read as a count, and
        // `1 messages` reads as a program that did not expect the number it
        // printed.
        assert_eq!(
            art(&started(1, 70, Glyphs::Unicode))
                .get(1)
                .map(String::as_str),
            Some("1 message left behind · /resume picks that session up again")
        );
    }

    #[test]
    fn what_stands_between_the_two_halves_comes_out_of_the_glyph_set() {
        // The row is one sentence and a second beside it, and what says they
        // are two is the mark between them. A terminal that cannot draw that
        // mark gets the one the setting names in its place rather than a
        // question mark in the middle of the sentence it was separating.
        assert_eq!(
            art(&started(12, 70, Glyphs::Ascii))
                .get(1)
                .map(String::as_str),
            Some("12 messages left behind - /resume picks that session up again")
        );
    }

    #[test]
    fn clearing_forgets_the_tools_that_were_looked_up() {
        // They belong to the conversation that looked them up. Left standing they
        // would be advertised to a session that never asked — the schema cost this
        // whole mechanism exists to avoid, paid for a model with no memory of why.
        let sample = Sample::new("clear-forgets-the-lookups");
        let mut runner = talking(&sample, "what was said before");
        let terms = terms(&sample, &Ledger::new(), &Plan::new());

        terms.revealed.reveal("web_search");
        assert!(terms.revealed.holds("web_search"));

        clearing(&terms, &mut runner);

        assert!(!terms.revealed.holds("web_search"));
    }

    #[test]
    fn nothing_it_says_is_wider_than_the_window_it_was_said_in() {
        // A row over the width would wrap, and a wrapped row leaves the cursor
        // a row below where the next frame expects it.
        for columns in 1..=70 {
            for row in started(12, columns, Glyphs::Unicode) {
                assert!(row.columns() <= columns, "at {columns}: {row:?}");
            }
        }
    }
}
