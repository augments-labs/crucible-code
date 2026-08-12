//! The arm no rule matched, which is the only thing a mode decides — and the
//! two ways a mode is spelled for the person it is decided in front of.
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

#[test]
fn the_ring_goes_one_way_and_closes() {
    // One key steps it, so there is no end to reach and no direction to
    // choose. Three presses from anywhere is where you started.
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        assert_eq!(mode.next().next().next(), mode, "{mode}");
        assert_ne!(mode.next(), mode, "{mode}");
    }

    // In the order they are written, which is least permissive first.
    assert_eq!(Mode::Ask.next(), Mode::AllowEdits);
    assert_eq!(Mode::AllowEdits.next(), Mode::FullAccess);
}

#[test]
fn a_mode_is_spelled_one_way_to_type_and_another_way_to_read() {
    // The row under the box is read while the mode is in force; the
    // configuration file is typed. Neither string is the other's shortening.
    assert_eq!(Mode::Ask.to_string(), "ask");
    assert_eq!(Mode::Ask.sentence(), "ask mode on");

    assert_eq!(Mode::AllowEdits.to_string(), "allowEdits");
    assert_eq!(Mode::AllowEdits.sentence(), "allow edits on");

    assert_eq!(Mode::FullAccess.to_string(), "fullAccess");
    assert_eq!(Mode::FullAccess.sentence(), "full access mode on");
}
