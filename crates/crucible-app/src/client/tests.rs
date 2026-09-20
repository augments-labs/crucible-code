//! What a client's word can and cannot settle.
//!
//! Every test here answers a pending action with something other than the one
//! decision that fits it, and watches what the permission engine would have
//! been handed. The front end is scripted and holds nothing but the replies it
//! was given, so a yes that reaches the engine got there through `settle`.

use std::collections::VecDeque;

use crucible_client_api::{
    Capabilities, Capability, Decision, ErrorCode, Lasting, Pending, PendingId, Picked, Progress,
    Refusal, Ruling, Said,
};
use crucible_tools::{Ask, Remember, Sensitivity, Target, Verdict};
use crucible_types::{Answer, Question, ToolArgs, ToolCall, ToolId};

use super::deciding::{Deciding, Front, Minting, Shown, questions};
use super::reading::progress;

/// What a scripted front end says next about whatever it is put.
enum Reply {
    /// A ruling naming the action it was put.
    Fitting(Ruling, Lasting),
    /// A ruling naming some other action.
    Naming(u64, Ruling),
    /// Answers naming the action it was put, this many of them.
    Answers(usize),
    /// A declining naming the action it was put.
    Declining,
}

/// Answers from a script, and remembers what it was put and what was refused.
#[derive(Default)]
struct Scripted {
    replies: VecDeque<Reply>,
    put: Vec<Pending>,
    refused: Vec<ErrorCode>,
}

impl Scripted {
    fn saying(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: replies.into_iter().collect(),
            ..Self::default()
        }
    }
}

impl Front for Scripted {
    fn put(&mut self, pending: &Pending, _shown: Shown<'_>) -> Option<Decision> {
        self.put.push(pending.clone());
        let id = pending.id();

        // Out of replies is nobody answering, which is how every script ends.
        Some(match self.replies.pop_front()? {
            Reply::Fitting(ruling, lasting) => Decision::Ruled {
                id,
                ruling,
                lasting,
            },
            Reply::Naming(number, ruling) => Decision::Ruled {
                id: PendingId::new(number),
                ruling,
                lasting: Lasting::Session,
            },
            Reply::Answers(many) => Decision::Answered {
                id,
                answers: (0..many)
                    .map(|_| {
                        Ok(Picked {
                            chosen: vec![Said::new("yes")?],
                            note: Said::new("a note")?,
                        })
                    })
                    .collect::<Result<_, Refusal>>()
                    .ok()?,
            },
            Reply::Declining => Decision::Declined { id },
        })
    }

    fn refused(&mut self, refusal: Refusal) {
        self.refused.push(refusal.code());
    }
}

fn call() -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: "write".into(),
        args: ToolArgs::new("{}"),
    }
}

fn changing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// What the engine is handed when `front` is asked about one call.
fn asked(front: &mut Scripted, minting: &Minting, has: Capabilities) -> (Verdict, Remember) {
    Deciding::new(front, minting, has).ask(&call(), &changing())
}

fn one_question() -> Vec<Question> {
    vec![Question::new(
        "Colour",
        "Which colour?",
        [Answer::new("yes"), Answer::new("no")],
    )]
}

#[test]
fn a_ruling_that_fits_is_the_verdict_and_lasts_only_as_long_as_it_said() {
    let minting = Minting::new();

    let mut once = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert_eq!(
        asked(&mut once, &minting, Capabilities::every()),
        (Verdict::Allow, Remember::Never)
    );

    let mut session = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Session)]);
    assert_eq!(
        asked(&mut session, &minting, Capabilities::every()),
        (Verdict::Allow, Remember::Session)
    );

    let mut no = Scripted::saying([Reply::Fitting(Ruling::Deny, Lasting::Session)]);
    assert_eq!(
        asked(&mut no, &minting, Capabilities::every()),
        (Verdict::Deny, Remember::Session)
    );
    assert!(once.refused.is_empty() && session.refused.is_empty() && no.refused.is_empty());
}

#[test]
fn a_yes_naming_another_action_settles_nothing_and_the_action_stays_pending() {
    let minting = Minting::new();
    let mut front = Scripted::saying([Reply::Naming(9_000, Ruling::Allow)]);

    let handed = asked(&mut front, &minting, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(front.refused, [ErrorCode::StaleDecision]);
    assert_eq!(front.put.len(), 2, "put again after the refusal");
    assert_eq!(front.put.first(), front.put.get(1), "and unchanged");
}

#[test]
fn a_yes_naming_an_action_already_settled_names_nothing_afterwards() {
    // The first call is allowed for real. Its identity is then replayed at the
    // second call, which is what a client holding an old yes would send.
    let minting = Minting::new();
    let mut first = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert_eq!(
        asked(&mut first, &minting, Capabilities::every()).0,
        Verdict::Allow
    );
    let settled = first.put.first().map(Pending::id).expect("one was put");

    let mut second = Scripted::saying([
        Reply::Naming(settled.number(), Ruling::Allow),
        Reply::Naming(settled.number(), Ruling::Allow),
    ]);
    let handed = asked(&mut second, &minting, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(
        second.refused,
        [ErrorCode::StaleDecision, ErrorCode::StaleDecision]
    );
    assert_ne!(second.put.first().map(Pending::id), Some(settled));
}

#[test]
fn an_answer_to_the_other_kind_of_question_settles_no_permission() {
    let minting = Minting::new();
    let mut front = Scripted::saying([Reply::Answers(1), Reply::Declining]);

    let handed = asked(&mut front, &minting, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(
        front.refused,
        [ErrorCode::WrongDecision, ErrorCode::WrongDecision]
    );
}

#[test]
fn a_client_that_never_said_it_answers_permissions_is_not_asked_and_the_call_is_denied() {
    let minting = Minting::new();
    let mut front = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Session)]);
    let without = Capabilities::none()
        .with(Capability::Questions)
        .with(Capability::Progress);

    let handed = asked(&mut front, &minting, without);

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert!(front.put.is_empty(), "the yes it had ready was never heard");
}

#[test]
fn a_refused_decision_leaves_the_action_for_the_one_that_fits() {
    let minting = Minting::new();
    let mut front = Scripted::saying([
        Reply::Naming(0, Ruling::Allow),
        Reply::Declining,
        Reply::Fitting(Ruling::Allow, Lasting::Once),
    ]);

    let handed = asked(&mut front, &minting, Capabilities::every());

    assert_eq!(handed, (Verdict::Allow, Remember::Never));
    assert_eq!(
        front.refused,
        [ErrorCode::StaleDecision, ErrorCode::WrongDecision]
    );
}

#[test]
fn questions_are_settled_only_by_one_answer_each_under_their_own_identity() {
    let minting = Minting::new();
    let asking = one_question();

    let mut ruled = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert!(questions(&minting, Capabilities::every(), &mut ruled, &asking).is_none());
    assert_eq!(ruled.refused, [ErrorCode::WrongDecision]);

    let mut short = Scripted::saying([Reply::Answers(2), Reply::Answers(1)]);
    let given =
        questions(&minting, Capabilities::every(), &mut short, &asking).expect("the second fits");
    assert_eq!(short.refused, [ErrorCode::InvalidArgument]);
    assert_eq!(given.len(), 1);
    let answer = given.first().expect("one answer");
    assert_eq!(answer.chosen().collect::<Vec<_>>(), ["yes"]);
    assert_eq!(answer.note(), "a note");

    let mut declined = Scripted::saying([Reply::Declining]);
    assert!(questions(&minting, Capabilities::every(), &mut declined, &asking).is_none());
    assert!(declined.refused.is_empty());

    let mut unheard = Scripted::saying([Reply::Answers(1)]);
    let without = Capabilities::none().with(Capability::Permissions);
    assert!(questions(&minting, without, &mut unheard, &asking).is_none());
    assert!(unheard.put.is_empty());
}

#[test]
fn an_identity_is_never_minted_twice() {
    let minting = Minting::new();
    let other = minting.clone();
    let mut front = Scripted::default();

    asked(&mut front, &minting, Capabilities::every());
    asked(&mut front, &other, Capabilities::every());
    questions(&minting, Capabilities::every(), &mut front, &one_question());

    let mut ids: Vec<u64> = front.put.iter().map(|put| put.id().number()).collect();
    assert_eq!(ids.len(), 3);
    ids.dedup();
    assert_eq!(ids.len(), 3, "{ids:?}");
}

/// Answers every action it is put with a yes naming some other one, for as long
/// as it is asked. It gives up after far more puts than anything should take, so
/// that a host which never stops asking fails a test instead of hanging one.
#[derive(Default)]
struct Stubborn {
    puts: usize,
    refused: Vec<&'static str>,
}

impl Stubborn {
    /// More puts than a host that stops asking ever makes.
    const PATIENCE: usize = 1_000;
}

impl Front for Stubborn {
    fn put(&mut self, _pending: &Pending, _shown: Shown<'_>) -> Option<Decision> {
        self.puts += 1;
        (self.puts < Self::PATIENCE).then_some(Decision::Ruled {
            id: PendingId::new(u64::MAX),
            ruling: Ruling::Allow,
            lasting: Lasting::Session,
        })
    }

    fn refused(&mut self, refusal: Refusal) {
        self.refused.push(refusal.code().as_str());
    }
}

#[test]
fn a_front_end_that_never_fits_its_answer_is_asked_a_few_times_and_then_no_more() {
    let minting = Minting::new();

    let mut permission = Stubborn::default();
    let handed =
        Deciding::new(&mut permission, &minting, Capabilities::every()).ask(&call(), &changing());
    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert!(permission.puts <= 8, "put {} times", permission.puts);
    assert_eq!(permission.refused.last(), Some(&"abandoned"));

    let mut asking = Stubborn::default();
    let given = questions(
        &minting,
        Capabilities::every(),
        &mut asking,
        &one_question(),
    );
    assert!(given.is_none());
    assert!(asking.puts <= 8, "put {} times", asking.puts);
    assert_eq!(asking.refused.last(), Some(&"abandoned"));
}

#[test]
fn a_client_that_never_asked_for_progress_is_handed_none() {
    let retrying = crucible_runner::Event::Retrying;

    assert_eq!(
        progress(Capabilities::every(), &retrying),
        Some(Progress::Retrying)
    );

    let without = Capabilities::none()
        .with(Capability::Permissions)
        .with(Capability::Questions);
    assert_eq!(progress(without, &retrying), None);
    assert_eq!(progress(Capabilities::none(), &retrying), None);
}
