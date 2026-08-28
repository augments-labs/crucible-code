//! `/clear`: starting a new session, and leaving this one where `/resume` can
//! find it.
//!
//! An empty context is what is asked for, and the shortest way to one is a
//! session that has said nothing yet. So this is [`resume`] with the session
//! swapped: a fresh log rather than a reopened one, handed to the same
//! [`Runner::pick_up`], which is also what drops the permission answers given
//! for the rest of a session that is now over. The record of what has been read
//! is emptied here for the same reason, exactly as `/resume` empties it, and so
//! are the images pasted at the prompt, the plan the panel above the box is
//! drawn from and the screen itself —
//! an empty context drawn under a full transcript would read as a conversation
//! the agent has no memory of. What goes back up is the opening card and
//! nothing else, so the screen after a clear is the screen a fresh start
//! draws.
//!
//! What was said is not deleted. The log it was written to is closed and stays
//! on the disk, so the session is on `/resume`'s list like any other and
//! nothing about this command is destructive — which is why it asks nothing
//! before running.
//!
//! [`resume`]: super::resume

use crucible_core::Transcript;
use crucible_runner::{Runner, Session};
use crucible_tui::{Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;

use super::super::Held;
use super::Terms;

/// Runs it.
pub(super) fn run<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<(), Fatal> {
    let columns = renderer.columns();

    // A session that has said nothing is already the empty one this would go
    // and open. Starting a second log here would leave two files for a session
    // that never happened, and `--continue` offers the newer of them. The
    // screen is still emptied: whatever a command printed belongs behind the
    // clear the same as a conversation would.
    if runner.transcript().is_empty() {
        held.kept.forget();
        held.images.clear();
        renderer.empties()?;
        held.opening.commit(renderer)?;

        let rows = [Row::new().then(Slot::Quiet, clip("nothing had been said", columns))];
        return Ok(renderer.present(&rows)?);
    }

    // Read again rather than carried from startup, because a conversation can
    // outlive a checkout: the session starting now records where the user is
    // now.
    let branch = crate::cli::branching::current(terms.workspace.root());
    let session = match Session::start(&terms.sessions, &terms.workspace, branch.as_deref()) {
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

    // A model picked mid-turn was picked for the session being left, and is
    // held on the session, so it goes with it rather than landing on the new
    // one it was never chosen for.
    terms.pending_model.take();

    // A mode stepped to mid-turn was stepped for the session being left, and
    // goes with it rather than landing on the new one it was never chosen for.
    terms.pending_mode.take();

    // The last chance to say that the log of the session being left stopped
    // being written. After this there is no session to say it about.
    if let Some(problem) = left.finish() {
        renderer.commit(&format!("! {problem}"))?;
    }

    // The transcript on screen was said by the session just left, and an empty
    // context is what was asked for — a screen still holding it would read as a
    // conversation the agent under the box has no memory of. What was held of
    // that session's results goes with the rows that offered them: a key
    // opening what is behind a row nobody can see is worse than no offer.
    held.kept.forget();

    // The images pasted go too: the markers naming them were in prompts of the
    // session just left, and the numbering starts over with the session. Held
    // on, one would ride the first prompt after the clear that says `[Image #1]`.
    held.images.clear();
    renderer.empties()?;

    // The opening again, and nothing else: a screen that looks exactly like a
    // fresh start is the whole of what says one happened. The card's facts
    // were read at launch, so its list of recent sessions does not yet name
    // the one just left — the price of a card that never disagrees with the
    // one the launch drew.
    held.opening.commit(renderer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crucible_auth::Store;
    use crucible_core::{Cancel, Message, Revealed, StopReason, ToolArgs, Transcript};
    use crucible_runner::{Model, Runner, Session, Tools, recent};
    use crucible_tools::{Ledger, Plan};
    use crucible_tui::{Recording, Renderer};

    use crate::cli::Fatal;
    use crate::cli::converse::{Answers, Held};
    use crate::cli::draw::opening::{Opening, Standing};
    use crate::cli::fake::Script;
    use crate::cli::sample::Sample;
    use crate::cli::style::Style;

    use super::super::Terms;
    use super::run;

    /// What a session holds, for a session holding nothing — the same holder
    /// `/resume`'s tests lend their runs.
    fn lent<'a>(input: &'a mut dyn std::io::BufRead, opening: &'a Standing) -> Held<'a> {
        Held::new(
            crucible_tools::Plan::new(),
            crucible_tui::Sending::default(),
            Answers { input, keys: false },
            opening,
        )
    }

    /// The opening card a clear puts back, read off `sample`'s workspace.
    fn standing(sample: &Sample) -> Standing {
        Standing::new(
            &Opening {
                credential: None,
                model: Some("script"),
                provider: None,
                unasked: "",
                trouble: None,
                workspace: &sample.workspace(),
                sessions: &[],
                update: None,
                style: Style::plain(),
            },
            std::time::SystemTime::now(),
        )
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
            pending_model: std::cell::Cell::new(None),
            pending_mode: std::cell::Cell::new(None),
            settings: crucible_config::Settings::default(),
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

        let session =
            Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
        session.append(&Message::said(asked));
        session.append(&answered);

        let mut transcript = Transcript::new();
        transcript.push(Message::said(asked));
        transcript.push(answered);

        Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                system: None,
                effort: None,
            },
            session,
        )
        .resuming(transcript)
    }

    /// Runs `/clear` against `runner`, and says what reached the terminal.
    fn clearing(sample: &Sample, terms: &Terms, runner: &mut Runner) -> String {
        let mut renderer = Renderer::new(Recording::new(80, 24));
        let mut input = std::io::empty();
        let opening = standing(sample);
        let mut held = lent(&mut input, &opening);

        run(&mut renderer, runner, &mut held, terms).expect("the terminal to be written");

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

        clearing(
            &sample,
            &terms(&sample, &Ledger::new(), &Plan::new()),
            &mut runner,
        );

        let now = runner.session().id().cloned().expect("a recorded session");
        assert_ne!(now, left, "the session in hand is the one that was left");
        assert_eq!(runner.transcript().len(), 0, "the transcript came with it");
        assert_eq!(listed(&sample, 1), ["what was said before"]);
    }

    #[test]
    fn clearing_takes_the_screen_and_what_was_held_behind_it() {
        // An empty context is what `/clear` promises, and the screen is part of
        // it: rows left standing were said by a session the agent has no memory
        // of, and what was held behind those rows goes with them — a key
        // opening what is behind a row nobody can see is worse than no offer.
        let sample = Sample::new("clear-empties-the-screen");
        let mut runner = talking(&sample, "what was said before");
        let terms = terms(&sample, &Ledger::new(), &Plan::new());

        let mut renderer = Renderer::new(Recording::new(80, 24));
        renderer
            .commit("a row of the session being left")
            .expect("the terminal to be written");

        let mut input = std::io::empty();
        let opening = standing(&sample);
        let mut held = lent(&mut input, &opening);
        let call = crucible_core::ToolId::new("call-1");
        held.kept.calling(call.clone(), "read".into());
        held.kept
            .finished(&call, "what the row had no room for".into(), 3);
        assert_eq!(held.kept.newest().count(), 1);

        run(&mut renderer, &mut runner, &mut held, &terms).expect("the terminal to be written");

        let picture = renderer.terminal().picture().rows().join("\n");
        assert!(
            !picture.contains("a row of the session being left"),
            "{picture}"
        );
        assert!(
            !picture.contains("started a new session"),
            "the screen after a clear is a fresh start, not an announcement: {picture}"
        );
        assert!(
            picture.contains("Tips"),
            "the opening card stands where the transcript was: {picture}"
        );
        assert_eq!(held.kept.newest().count(), 0);
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

        clearing(&sample, &terms(&sample, &Ledger::new(), &plan), &mut runner);

        assert!(plan.tasks().is_empty());
    }

    #[test]
    fn an_image_pasted_before_a_clear_is_not_attached_after_it() {
        // The paste put `[Image #1]` in a prompt of the session being left, and
        // the numbering starts over with the session. An image still held here
        // would be attached to the first prompt after the clear that says the
        // marker — a picture the agent was never shown and the user never sent
        // it.
        let sample = Sample::new("clear-forgets-the-images");
        let mut runner = talking(&sample, "what was said before");
        let terms = terms(&sample, &Ledger::new(), &Plan::new());

        let mut renderer = Renderer::new(Recording::new(80, 24));
        let mut input = std::io::empty();
        let opening = standing(&sample);
        let mut held = lent(&mut input, &opening);
        held.images.push("a-picture.png".into());

        run(&mut renderer, &mut runner, &mut held, &terms).expect("the terminal to be written");

        assert!(held.images.is_empty());
    }

    #[test]
    fn what_was_said_before_a_clear_comes_back_when_that_session_is_picked_up() {
        // The log the clear left is closed rather than abandoned: an unfinished
        // one still holds a claim, and `/resume` would refuse it as open in
        // another crucible — which names this crucible.
        let sample = Sample::new("clear-then-resume");
        let mut runner = talking(&sample, "what was said before");
        let terms = terms(&sample, &Ledger::new(), &Plan::new());

        clearing(&sample, &terms, &mut runner);
        assert_eq!(listed(&sample, 1), ["what was said before"]);
        let picked = recent(&sample.logs(), &sample.workspace(), 1)
            .first()
            .map(|session| session.id().as_str().to_owned())
            .expect("the cleared session is on the list");

        let mut renderer = Renderer::new(Recording::new(80, 24));
        let mut input = std::io::empty();
        let opening = standing(&sample);
        let mut held = crate::cli::converse::Held::new(
            Plan::new(),
            crucible_tui::Sending::default(),
            crate::cli::converse::Answers {
                input: &mut input,
                keys: false,
            },
            &opening,
        );
        super::super::resume::run(&picked, &mut renderer, &mut runner, &mut held, &terms)
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
        let session =
            Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
        let held = session.id().cloned().expect("a recorded session");
        let mut runner = Runner::new(
            Box::new(Script::new(Vec::new())),
            Tools::new(),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                system: None,
                effort: None,
            },
            session,
        );

        let written = clearing(
            &sample,
            &terms(&sample, &Ledger::new(), &Plan::new()),
            &mut runner,
        );

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

        let written = clearing(&sample, &terms, &mut runner);

        assert!(written.contains("! "), "{written}");
        assert!(
            !written.contains("Tips"),
            "a failed clear leaves the screen exactly as it was: {written}"
        );
        assert_eq!(runner.session().id(), Some(&held), "{written}");
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

        clearing(&sample, &terms, &mut runner);

        assert!(!terms.revealed.holds("web_search"));
    }
}
