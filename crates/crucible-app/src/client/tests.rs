//! What a client's word can and cannot settle.
//!
//! Every test here answers a pending action with something other than the one
//! decision that fits it, and watches what the permission engine would have
//! been handed. The front end is scripted and holds nothing but the replies it
//! was given, so a yes that reaches the engine got there through `settle`.

use std::collections::VecDeque;

use crucible_client_api::bounds::{ITEMS, TEXT_BYTES};
use crucible_client_api::{
    Capabilities, Capability, Decision, ErrorCode, Lasting, Pending, PendingId, Picked, Progress,
    Refusal, Ruling, Said,
};
use crucible_tools::{Ask, Remember, Sensitivity, Target, Verdict};
use crucible_types::{Answer, Question, ToolArgs, ToolCall, ToolId};

use super::deciding::{Deciding, Front, Shown, questions};
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
fn asked(front: &mut Scripted, has: Capabilities) -> (Verdict, Remember) {
    Deciding::new(front, has).ask(&call(), &changing())
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
    let mut once = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert_eq!(
        asked(&mut once, Capabilities::every()),
        (Verdict::Allow, Remember::Never)
    );

    let mut session = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Session)]);
    assert_eq!(
        asked(&mut session, Capabilities::every()),
        (Verdict::Allow, Remember::Session)
    );

    let mut no = Scripted::saying([Reply::Fitting(Ruling::Deny, Lasting::Session)]);
    assert_eq!(
        asked(&mut no, Capabilities::every()),
        (Verdict::Deny, Remember::Session)
    );
    assert!(once.refused.is_empty() && session.refused.is_empty() && no.refused.is_empty());
}

#[test]
fn a_yes_naming_another_action_settles_nothing_and_the_action_stays_pending() {
    let mut front = Scripted::saying([Reply::Naming(9_000, Ruling::Allow)]);

    let handed = asked(&mut front, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(front.refused, [ErrorCode::StaleDecision]);
    assert_eq!(front.put.len(), 2, "put again after the refusal");
    assert_eq!(front.put.first(), front.put.get(1), "and unchanged");
}

#[test]
fn a_yes_naming_an_action_already_settled_names_nothing_afterwards() {
    // The first call is allowed for real. Its identity is then replayed at the
    // second call, which is what a client holding an old yes would send.
    let mut first = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert_eq!(asked(&mut first, Capabilities::every()).0, Verdict::Allow);
    let settled = first.put.first().map(Pending::id).expect("one was put");

    let mut second = Scripted::saying([
        Reply::Naming(settled.number(), Ruling::Allow),
        Reply::Naming(settled.number(), Ruling::Allow),
    ]);
    let handed = asked(&mut second, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(
        second.refused,
        [ErrorCode::StaleDecision, ErrorCode::StaleDecision]
    );
    assert_ne!(second.put.first().map(Pending::id), Some(settled));
}

#[test]
fn an_answer_to_the_other_kind_of_question_settles_no_permission() {
    let mut front = Scripted::saying([Reply::Answers(1), Reply::Declining]);

    let handed = asked(&mut front, Capabilities::every());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert_eq!(
        front.refused,
        [ErrorCode::WrongDecision, ErrorCode::WrongDecision]
    );
}

#[test]
fn a_client_that_never_said_it_answers_permissions_is_not_asked_and_the_call_is_denied() {
    let mut front = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Session)]);
    let without = Capabilities::none()
        .with(Capability::Questions)
        .with(Capability::Progress);

    let handed = asked(&mut front, without);

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert!(front.put.is_empty(), "the yes it had ready was never heard");
}

#[test]
fn a_refused_decision_leaves_the_action_for_the_one_that_fits() {
    let mut front = Scripted::saying([
        Reply::Naming(0, Ruling::Allow),
        Reply::Declining,
        Reply::Fitting(Ruling::Allow, Lasting::Once),
    ]);

    let handed = asked(&mut front, Capabilities::every());

    assert_eq!(handed, (Verdict::Allow, Remember::Never));
    assert_eq!(
        front.refused,
        [ErrorCode::StaleDecision, ErrorCode::WrongDecision]
    );
}

#[test]
fn questions_are_settled_only_by_one_answer_each_under_their_own_identity() {
    let asking = one_question();

    let mut ruled = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Once)]);
    assert!(questions(Capabilities::every(), &mut ruled, &asking).is_none());
    assert_eq!(ruled.refused, [ErrorCode::WrongDecision]);

    let mut short = Scripted::saying([Reply::Answers(2), Reply::Answers(1)]);
    let given = questions(Capabilities::every(), &mut short, &asking).expect("the second fits");
    assert_eq!(short.refused, [ErrorCode::InvalidArgument]);
    assert_eq!(given.len(), 1);
    let answer = given.first().expect("one answer");
    assert_eq!(answer.chosen().collect::<Vec<_>>(), ["yes"]);
    assert_eq!(answer.note(), "a note");

    let mut declined = Scripted::saying([Reply::Declining]);
    assert!(questions(Capabilities::every(), &mut declined, &asking).is_none());
    assert!(declined.refused.is_empty());

    let mut unheard = Scripted::saying([Reply::Answers(1)]);
    let without = Capabilities::none().with(Capability::Permissions);
    assert!(questions(without, &mut unheard, &asking).is_none());
    assert!(unheard.put.is_empty());
}

#[test]
fn a_question_that_cannot_be_put_whole_is_not_put_short() {
    // An answer that was not offered cannot be chosen, and a name that was cut
    // is not the name the asker reads back: either way what was shown is not
    // the question, so nobody is asked it.
    let many = (0..=ITEMS).map(|number| Answer::new(format!("answer {number}")));
    let crowded = vec![Question::new("Colour", "Which colour?", many)];
    let mut front = Scripted::saying([Reply::Answers(1)]);
    assert!(questions(Capabilities::every(), &mut front, &crowded).is_none());
    assert!(front.put.is_empty(), "put with answers left out");

    let long = "n".repeat(TEXT_BYTES + 1);
    let named = vec![Question::new(
        "Colour",
        "Which colour?",
        [Answer::new(long), Answer::new("no")],
    )];
    let mut front = Scripted::saying([Reply::Answers(1)]);
    assert!(questions(Capabilities::every(), &mut front, &named).is_none());
    assert!(front.put.is_empty(), "put with an answer's name cut");

    let full = (0..ITEMS).map(|number| Answer::new(format!("answer {number}")));
    let fitting = vec![Question::new("Colour", "Which colour?", full)];
    let mut front = Scripted::saying([Reply::Answers(1)]);
    assert!(questions(Capabilities::every(), &mut front, &fitting).is_some());
}

#[test]
fn questions_declined_without_being_put_spend_no_identity() {
    /// Far more than every other test in this process mints between them, so
    /// the count below is about this test whatever runs beside it.
    const DECLINED: u64 = 10_000;

    let minted = || {
        let mut front = Scripted::default();
        asked(&mut front, Capabilities::every());
        front.put.first().map(|put| put.id().number())
    };
    let many = (0..=ITEMS).map(|number| Answer::new(format!("answer {number}")));
    let crowded = vec![Question::new("Colour", "Which colour?", many)];

    let before = minted().expect("a permission question is put");
    for _ in 0..DECLINED {
        let mut front = Scripted::default();
        assert!(questions(Capabilities::every(), &mut front, &crowded).is_none());
        assert!(front.put.is_empty());
    }
    let after = minted().expect("a permission question is put");

    assert!(
        after - before < DECLINED,
        "{} identities went to questions nobody was put",
        after - before
    );
}

/// A call to run a line too long for the words a pending action carries.
fn running_a_long_line() -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: crucible_tools::Command::Opaque("x".repeat(TEXT_BYTES + 1).into()),
    }
}

#[test]
fn a_call_whose_subject_would_be_cut_is_denied_rather_than_put_short() {
    // A yes to half a command line is a yes to a line nobody read, so a front
    // end that reads the pending action alone is not asked, and the call is
    // refused as it is where nobody answers.
    let mut front = Scripted::saying([Reply::Fitting(Ruling::Allow, Lasting::Session)]);
    let handed =
        Deciding::new(&mut front, Capabilities::every()).ask(&call(), &running_a_long_line());

    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert!(front.put.is_empty(), "put with its subject cut");

    // One that draws the call itself reads all of it, and is asked as ever.
    let mut whole = Whole(Scripted::saying([Reply::Fitting(
        Ruling::Allow,
        Lasting::Once,
    )]));
    let handed =
        Deciding::new(&mut whole, Capabilities::every()).ask(&call(), &running_a_long_line());
    assert_eq!(handed, (Verdict::Allow, Remember::Never));
    assert_eq!(whole.0.put.len(), 1);
}

/// A scripted front end that draws from the whole value it is lent.
struct Whole(Scripted);

impl Front for Whole {
    fn put(&mut self, pending: &Pending, shown: Shown<'_>) -> Option<Decision> {
        self.0.put(pending, shown)
    }

    fn refused(&mut self, refusal: Refusal) {
        self.0.refused(refusal);
    }

    fn draws_whole(&self) -> bool {
        true
    }
}

#[test]
fn an_identity_is_never_minted_twice() {
    let mut front = Scripted::default();

    asked(&mut front, Capabilities::every());
    asked(&mut front, Capabilities::every());
    questions(Capabilities::every(), &mut front, &one_question());

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
    let mut permission = Stubborn::default();
    let handed = Deciding::new(&mut permission, Capabilities::every()).ask(&call(), &changing());
    assert_eq!(handed, (Verdict::Deny, Remember::Never));
    assert!(permission.puts <= 8, "put {} times", permission.puts);
    assert_eq!(permission.refused.last(), Some(&"abandoned"));

    let mut asking = Stubborn::default();
    let given = questions(Capabilities::every(), &mut asking, &one_question());
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

/// What a client is told of a turn that ended `turned`.
fn told(turned: crucible_runner::Turned) -> crucible_client_api::TurnOutcome {
    match super::Ended::Turn(Ok(turned)).outcome() {
        crucible_client_api::Outcome::Turn(outcome) => outcome,
        other => panic!("a turn was told as {other:?}"),
    }
}

#[test]
fn words_a_guardrail_ceiling_cut_reach_a_client_said_to_be_cut() {
    // Both ceilings are under the frame's own, so the frame's cut never
    // happens to these words and cannot be what says so.
    let reason = "r".repeat(crucible_agents::GUARDRAIL_REASON_BYTES + 1);
    let name = "n".repeat(crucible_agents::GUARDRAIL_NAME_BYTES + 1);
    assert!(reason.len() < TEXT_BYTES);

    let crucible_client_api::TurnOutcome::Rejected { guard, why, .. } =
        told(crucible_runner::Turned::Rejected {
            rejection: crucible_runner::Rejection::new(&name, &reason),
            stop: None,
        })
    else {
        panic!("a refusal was told as something else");
    };
    assert!(guard.truncated(), "a cut name was sent as whole");
    assert!(why.truncated(), "a cut reason was sent as whole");

    let crucible_client_api::TurnOutcome::Rejected { guard, why, .. } =
        told(crucible_runner::Turned::Rejected {
            rejection: crucible_runner::Rejection::new("no-secrets", "it ends in [cut]"),
            stop: None,
        })
    else {
        panic!("a refusal was told as something else");
    };
    assert!(
        !guard.truncated() && !why.truncated(),
        "whole words were sent as cut"
    );

    for (name, reason) in [("no-secrets", reason.as_str()), (name.as_str(), "no")] {
        let crucible_client_api::TurnOutcome::Undecided { problem, .. } =
            told(crucible_runner::Turned::Undecided {
                problem: crucible_runner::GuardrailError::undecided(name, reason),
                stop: None,
            })
        else {
            panic!("a check that could not decide was told as something else");
        };
        assert!(problem.message.truncated(), "cut words were sent as whole");
    }
}
