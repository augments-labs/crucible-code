//! A used-up plan, with a line waiting behind the turn it stopped.
//!
//! The notice says when the plan comes back, and the lines typed behind the
//! turn are the reader's to send once it has. Run as the next turn they would
//! only stop again, or go to a vendor that has said the plan is spent.

use std::time::{Duration, SystemTime};

use crucible_types::{Message, PlanWindows, Window, WindowReading};

use crate::cli::converse::{Answers, Held, Work, queueing, ran};
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::unanswerable::recorded;
use super::*;

/// The title of the notice a plan stop draws.
const NOTICE: &str = "Usage limit reached";

/// The line typed behind the last turn.
const QUEUED: &str = "and then run the tests";

/// A reading with the weekly window used up until tomorrow.
fn spent() -> PlanWindows {
    let tomorrow = SystemTime::now() + Duration::from_hours(24);
    PlanWindows::new(SystemTime::now())
        .with(Window::Weekly, WindowReading::new(100, Some(tomorrow)))
}

/// What the session came to once the queue was asked for the next turn.
struct Stopped {
    /// What the queue answered: `None` where it ran no turn.
    taken: Option<bool>,
    /// How many requests reached the vendor.
    asked: usize,
    /// How many notices were drawn.
    notices: usize,
    /// What is still waiting in the queue.
    waiting: Vec<String>,
    /// The prompts the session recorded.
    said: Vec<Message>,
}

/// Each of `prompts` sent in turn to a vendor that refuses every request as a
/// used-up plan reporting `reading`, with [`QUEUED`] typed behind the last of
/// them, and then the queue asked for the next turn as the loop asks it.
fn stopped(name: &str, reading: Option<PlanWindows>, prompts: &[&str]) -> Stopped {
    let sample = Sample::new(name);
    let session =
        Arc::new(Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session"));
    let script = Script::used_up(reading);
    let asked = script.asked();
    let mut conversation = paired(Arc::clone(&session), |session| {
        scripted(script, Tools::new(), session)
    });

    let terms = plain();
    let card = opening();
    let mut input = Cursor::new(Vec::new());
    let mut held = Held::new(
        terms.plan.clone(),
        terms.sending,
        Answers {
            input: &mut input,
            keys: false,
        },
        &card,
    );
    let mut renderer = Renderer::new(Recording::redirected(80, 24));

    for (at, prompt) in prompts.iter().enumerate() {
        if at + 1 == prompts.len() {
            let mut editor = typed(QUEUED);
            assert_eq!(held.queued.accept(&mut editor), Retained::Accepted);
        }
        let work = Work::Turn((*prompt).to_owned(), Box::default());
        let (back, leaving) =
            ran(conversation, &mut renderer, &terms, work, &mut held).expect("the turn to end");
        assert!(!leaving, "the session left");
        conversation = back;
    }

    let (conversation, taken) = queueing::taken(
        conversation,
        &mut renderer,
        &terms,
        &mut held,
        Style::plain(),
    )
    .expect("the queue to be asked");
    drop(conversation);

    let written = renderer.terminal().written().to_owned();
    Stopped {
        taken,
        asked: asked.load(Ordering::Relaxed),
        notices: written.matches(NOTICE).count(),
        waiting: held.queued.waiting_all().map(str::to_owned).collect(),
        said: recorded(&sample, session),
    }
}

#[test]
fn plan_limit_refused_holds_the_queued_line_and_sends_nothing_more() {
    // The vendor refused the turn and reported no window on its head, so
    // nothing here knows the plan is spent but the refusal itself: run as the
    // next turn, the line behind it would be one more request into it.
    let stopped = stopped("plan-limit-refused-queue", None, &["fix the build"]);

    assert_eq!(stopped.taken, None, "the queue ran a turn");
    assert_eq!(stopped.asked, 1, "a second request reached the vendor");
    assert_eq!(stopped.notices, 1, "a second notice was drawn");
    assert_eq!(stopped.waiting, vec![QUEUED.to_owned()]);
}

#[test]
fn plan_limit_before_sending_holds_the_queued_line_and_records_nothing_more() {
    // The first refusal reported the weekly window used up until tomorrow, so
    // the second prompt stops before anything is sent. The line behind it is
    // still the reader's: run, it would be recorded and stop a third time.
    let stopped = stopped(
        "plan-limit-before-sending-queue",
        Some(spent()),
        &["fix the build", "try again"],
    );

    assert_eq!(stopped.taken, None, "the queue ran a turn");
    assert_eq!(
        stopped.asked, 1,
        "a request went out past the used-up window"
    );
    assert_eq!(stopped.notices, 2, "a third notice was drawn");
    assert_eq!(stopped.waiting, vec![QUEUED.to_owned()]);
    assert_eq!(
        stopped.said,
        vec![Message::said("fix the build"), Message::said("try again")],
        "the transcript does not end on the line recorded before the stop"
    );
}
