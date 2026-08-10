//! The arm no rule matched, which is the only thing a mode decides.
//!
//! Each of these is a promise made in a word somebody typed once and then
//! stopped thinking about, so the shape worth testing is the boundary: what
//! `allowEdits` covers, and what it still stops at.

use super::*;

/// A command something read closely enough to say it stays inside.
fn confined(parts: &[&str]) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            parts: parts.iter().map(|part| (*part).into()).collect(),
            reach: Reach::Workspace,
        },
    }
}

#[test]
fn a_read_is_allowed_without_asking_in_every_mode() {
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        let mut permission = with(mode, &[]);
        let mut answer = Answer::once(Verdict::Deny);

        assert!(
            permission
                .decide(&call("read"), &reading("src/a.rs"), &mut answer)
                .ran(),
            "{mode} must allow a read"
        );
        assert_eq!(answer.asked, 0, "{mode} must not prompt for a read");
    }
}

#[test]
fn a_deny_rule_holds_under_full_access() {
    let mut permission = with(Mode::FullAccess, &[(Disposition::Deny, "bash(curl *)")]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(matches!(
        permission.decide(
            &call("bash"),
            &running(&["curl http://example.invalid"]),
            &mut answer
        ),
        Settled::Forbidden
    ));
    assert_eq!(answer.asked, 0);
}

#[test]
fn an_ask_rule_holds_under_full_access() {
    let mut permission = with(Mode::FullAccess, &[(Disposition::Ask, "bash(git push *)")]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(
        permission
            .decide(&call("bash"), &running(&["git push --force"]), &mut answer)
            .ran()
    );
    assert_eq!(
        answer.asked, 1,
        "a mode decides the arm no rule matched, and nothing else"
    );
}

#[test]
fn allow_edits_writes_without_asking_but_still_asks_before_running_anything() {
    let mut permission = with(Mode::AllowEdits, &[]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(
        permission
            .decide(&call("write"), &writing("src/a.rs"), &mut answer)
            .ran()
    );
    assert_eq!(answer.asked, 0);

    assert!(
        permission
            .decide(&call("bash"), &running(&["ls"]), &mut answer)
            .ran()
    );
    assert_eq!(answer.asked, 1);
}

#[test]
fn allow_edits_runs_a_command_that_reaches_no_further_than_an_edit() {
    // The promise is about the workspace, not about which tool did it. A
    // `mkdir` proved to land inside changes exactly what `write` may change,
    // and asking about one while waving the other through is a distinction the
    // person who typed `allowEdits` did not make.
    let mut permission = with(Mode::AllowEdits, &[]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(
        permission
            .decide(&call("bash"), &confined(&["mkdir src/net"]), &mut answer)
            .ran()
    );
    assert_eq!(answer.asked, 0);
}

#[test]
fn ask_still_asks_about_a_command_confined_to_the_workspace() {
    // `ask` means ask. The reach is what `allowEdits` reads, and no other mode
    // is entitled to quietly start reading it too.
    let mut permission = with(Mode::Ask, &[]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(
        permission
            .decide(&call("bash"), &confined(&["mkdir src/net"]), &mut answer)
            .ran()
    );
    assert_eq!(answer.asked, 1);
}

#[test]
fn a_deny_rule_beats_a_command_confined_to_the_workspace() {
    // Staying inside the workspace is what stops a question being asked, never
    // what overrules the answer somebody wrote down in advance.
    let mut permission = with(Mode::AllowEdits, &[(Disposition::Deny, "bash(rm *)")]);
    let mut answer = Answer::once(Verdict::Allow);

    assert!(matches!(
        permission.decide(&call("bash"), &confined(&["rm -rf build"]), &mut answer),
        Settled::Forbidden
    ));
    assert_eq!(answer.asked, 0);
}

#[test]
fn full_access_asks_about_nothing() {
    let mut permission = with(Mode::FullAccess, &[]);
    let mut answer = Answer::once(Verdict::Deny);

    assert!(
        permission
            .decide(&call("bash"), &running(&["rm -rf build"]), &mut answer)
            .ran()
    );
    assert_eq!(answer.asked, 0);
}
