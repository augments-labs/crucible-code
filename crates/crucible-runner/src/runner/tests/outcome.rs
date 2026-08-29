//! What the loop hands back about the run it just took.

use super::*;

/// The harness's services as one run, under the shipped policy.
macro_rules! run {
    ($scripted:expr) => {
        RunContext::new(
            RunPolicy::default(),
            &$scripted.events,
            &$scripted.cancel,
            &$scripted.steer,
            &$scripted.aside,
        )
    };
}

#[test]
fn a_finished_run_reports_the_run_it_was_given_and_what_it_spent() {
    // Two responses, so the total is something no single one of them says.
    let script = Script::new(vec![
        vec![
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "read".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Spent(Spend::new(90)),
            Delta::Stopped(StopReason::WantsTools),
        ],
        vec![
            Delta::Text("found it".into()),
            Delta::Spent(Spend::new(30)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);

    let run = run!(scripted);
    let result = scripted
        .runner
        .exchange(&mut scripted.says, &run)
        .expect("a finished run");

    assert_eq!(result.run(), run.run(), "the result named a different run");
    assert_eq!(result.status(), RunStatus::Completed);
    assert_eq!(result.stop(), StopReason::Yielded);
    assert_eq!(result.spent().tokens(), 120);
}

#[test]
fn a_stopped_run_says_a_person_ended_it_rather_than_that_it_finished() {
    // Stopped while the call it asked for was waiting to run, which is where
    // a turn already under way notices the flag.
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let mut scripted = Scripted::new(script, tools([Fixed::new("read")]), Verdict::Allow);
    scripted.cancel.request();

    let run = run!(scripted);
    let result = scripted
        .runner
        .exchange(&mut scripted.says, &run)
        .expect("a stopped run is not a failure");

    assert_eq!(result.status(), RunStatus::Cancelled);
    assert_eq!(result.stop(), StopReason::Cancelled);
}
