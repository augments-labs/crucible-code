//! The one refusal that precedes rules and modes: no tool writes the files
//! the engine is configured from.

use super::*;

#[test]
fn no_mode_lets_a_tool_write_the_permission_configuration() {
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        let mut permission = with(mode, &[]);
        let mut answer = Answer::once(Verdict::Allow);

        assert!(
            matches!(
                permission.decide(
                    &call("write"),
                    &writing(".crucible/config.json"),
                    &mut answer
                ),
                Settled::Forbidden
            ),
            "{mode} let the permission configuration be written"
        );
        assert_eq!(answer.asked, 0, "no answer could make this allowed");
    }
}

#[test]
fn no_rule_lets_a_tool_write_the_permission_configuration() {
    // The refusal comes before the rules, because the rules are what a write
    // here would rewrite. Both files feed the same engine on the next start,
    // so both are covered.
    let mut permission = with(Mode::Ask, &[(Disposition::Allow, "write(**)")]);
    let mut answer = Answer::once(Verdict::Allow);

    for file in [".crucible/config.json", ".crucible/config.local.json"] {
        assert!(
            matches!(
                permission.decide(&call("write"), &writing(file), &mut answer),
                Settled::Forbidden
            ),
            "{file} was written under an allow rule"
        );
    }
    assert_eq!(answer.asked, 0);
}

#[test]
fn the_configuration_is_covered_wherever_the_crucible_directory_is() {
    // The home file has no spelling below the workspace root, and a directory
    // deeper in the tree is what a session started there would read. The match
    // is on the resolved path's own components, so both are the same case.
    let mut permission = with(Mode::FullAccess, &[]);
    let mut answer = Answer::once(Verdict::Allow);

    let home = Sensitivity::MutatesFile {
        target: Target::at("/home/somebody/.crucible/config.json", None),
    };
    assert!(matches!(
        permission.decide(&call("write"), &home, &mut answer),
        Settled::Forbidden
    ));

    assert!(matches!(
        permission.decide(
            &call("write"),
            &writing("tools/agent/.crucible/config.local.json"),
            &mut answer
        ),
        Settled::Forbidden
    ));
}

#[test]
fn only_the_configuration_itself_is_refused() {
    // The neighbours stay ordinary: a directory that merely ends in
    // `.crucible`, another file inside the real one, and reading the
    // configuration, which is how every session starts.
    let mut permission = with(Mode::FullAccess, &[]);
    let mut answer = Answer::once(Verdict::Deny);

    assert!(
        permission
            .decide(
                &call("write"),
                &writing("x.crucible/config.json"),
                &mut answer
            )
            .ran()
    );
    assert!(
        permission
            .decide(
                &call("write"),
                &writing(".crucible/notes.json"),
                &mut answer
            )
            .ran()
    );
    assert!(
        permission
            .decide(
                &call("read"),
                &reading(".crucible/config.json"),
                &mut answer
            )
            .ran()
    );
}
