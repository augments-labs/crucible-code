//! A used-up plan, with a line waiting behind the turn it stopped.
//!
//! The notice says when the plan comes back, and the lines typed behind the
//! turn are the reader's to send once it has. Run as the next turn they would
//! only stop again, or go to a vendor that has said the plan is spent.

use std::time::{Duration, SystemTime};

use crucible_types::{Message, PlanWindows, Window, WindowReading};

use crucible_app::Conversation;
use crucible_tui::Recalled;

use crate::cli::converse::queueing::{self, Offer};
use crate::cli::converse::typing::{self, Opened};
use crate::cli::converse::{Answers, Held, Terms, Work, ran};
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
    stopped_then(name, reading, prompts, |_, _, _| ()).0
}

/// [`stopped`], and then `then` at the prompt the loop asks for next, with the
/// session as the stop left it.
fn stopped_then<T>(
    name: &str,
    reading: Option<PlanWindows>,
    prompts: &[&str],
    then: impl FnOnce(&Conversation, &mut Held<'_>, &Terms) -> T,
) -> (Stopped, T) {
    stopped_after(name, reading, prompts, None, then)
}

/// [`stopped_then`], with `after` run once the prompts have, before the queue
/// is asked for the next turn: work that ends some other way than the stop.
fn stopped_after<T>(
    name: &str,
    reading: Option<PlanWindows>,
    prompts: &[&str],
    after: Option<Work>,
    then: impl FnOnce(&Conversation, &mut Held<'_>, &Terms) -> T,
) -> (Stopped, T) {
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
        terms.sending.get(),
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
    if let Some(work) = after {
        let (back, leaving) =
            ran(conversation, &mut renderer, &terms, work, &mut held).expect("the work to end");
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
    let then = then(&conversation, &mut held, &terms);
    drop(conversation);

    let written = renderer.terminal().written().to_owned();
    let stopped = Stopped {
        taken,
        asked: asked.load(Ordering::Relaxed),
        notices: written.matches(NOTICE).count(),
        waiting: held.queued.waiting_all().map(str::to_owned).collect(),
        said: recorded(&sample, session),
    };
    (stopped, then)
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

#[test]
fn plan_limit_hold_lets_go_once_the_next_work_ends_another_way() {
    // The hold is about the turn that stopped, not the session: room made
    // because the window filled, with nothing behind the turns it keeps whole,
    // ends without a stop, and the line held since is the next turn again
    // rather than waiting over the box for the rest of the session.
    let (stopped, ()) = stopped_after(
        "plan-limit-hold-lets-go",
        None,
        &["fix the build"],
        Some(Work::Room(Compacting::Full)),
        |_, _, _| (),
    );

    assert!(
        stopped.taken.is_some(),
        "the held line was not run as a turn"
    );
    assert!(stopped.waiting.is_empty(), "the line is still queued");
    assert_eq!(stopped.asked, 2, "the held line never reached the vendor");
}

/// The idle prompt's first frame, drawn as [`typing::ask`] draws it before it
/// reads a key: the box, and what stands over it.
fn idle(conversation: &Conversation, held: &Held<'_>) -> Vec<String> {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    typing::draw(
        &mut renderer,
        &held.editor,
        Style::plain(),
        typing::around(
            &held.planning,
            &Opened::default(),
            &typing::saying(conversation.runner()),
            Recalled::default(),
            &held.queued,
        ),
    )
    .expect("the box to be drawn");

    renderer.terminal().picture().rows()
}

#[test]
fn plan_limit_stands_the_held_line_in_the_queue_panel_over_the_idle_prompt() {
    // Held and unseen, the line would run behind whatever the reader sends
    // next without anything on screen having said it was still there. The
    // panel that names it while a turn runs names it here, with the same keys,
    // and the box stays under it.
    let (_, rows) = stopped_then(
        "plan-limit-idle-box",
        None,
        &["fix the build"],
        |conversation, held, _| idle(conversation, held),
    );

    let rule = "\u{2500}".repeat(80);
    let opens = rows
        .iter()
        .position(|row| *row == rule)
        .unwrap_or_else(|| panic!("no queue panel over the idle prompt: {rows:#?}"));

    assert_eq!(
        rows.get(opens..opens + 9),
        Some(
            &[
                rule.clone(),
                String::new(),
                "1 queued \u{b7} ctrl+enter to send all now".to_owned(),
                String::new(),
                format!("\u{203a} {QUEUED}"),
                String::new(),
                "\u{2191}\u{2193} to walk \u{b7} ctrl+e to edit \u{b7} ctrl+x to delete \u{b7} ctrl+s to send now"
                    .to_owned(),
                String::new(),
                // The prompt's own reading row, blank while no window has been
                // reported, as it is under a running turn.
                String::new(),
            ][..]
        ),
        "{rows:#?}"
    );
    assert_eq!(
        rows.get(opens.wrapping_sub(1)),
        Some(&String::new()),
        "the rule is kept off what stands above it: {rows:#?}"
    );
    assert!(
        rows.get(opens + 9)
            .is_some_and(|row| row.starts_with('\u{256d}')),
        "the prompt does not stand directly under the queue panel: {rows:#?}"
    );
}

#[test]
fn plan_limit_ctrl_x_at_the_idle_box_deletes_the_held_line_and_the_panel_goes() {
    // The panel names the key, so the key has to work where the panel stands.
    // It drops the line from the queue the stop held it in, and with nothing
    // left the box stands under the transcript again.
    let (_, (deleted, waiting, rows)) = stopped_then(
        "plan-limit-idle-delete",
        None,
        &["fix the build"],
        |conversation, held, _terms| {
            let deleted = held.queued.delete(Offer::Nowhere);
            (
                deleted,
                held.queued.waiting_count(),
                idle(conversation, held),
            )
        },
    );

    assert!(deleted, "ctrl+x at the idle box found nothing to delete");
    assert_eq!(waiting, 0, "ctrl+x left the line queued");
    assert!(
        !rows.iter().any(|row| row.contains("queued")),
        "the panel stood on over an empty queue: {rows:#?}"
    );
}
