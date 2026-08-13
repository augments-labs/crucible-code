use super::super::{Command, Target};
use super::*;
use crate::ids::ToolId;
use crate::tool::ToolArgs;

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("call-1"),
        name: name.into(),
        args: ToolArgs::new("{}"),
    }
}

/// A target as a tool would build it: resolved, in both spellings.
fn at(absolute: &str, below_root: Option<&str>) -> Target {
    Target::at(absolute, below_root)
}

/// An absolute path, spelled the way a rule about one is written here.
///
/// A drive on Windows, because that is what `is_absolute` asks for and a
/// pattern is sorted into absolute or relative by it. The separator stays `/`
/// on both, which is what the pattern language uses and what a resolved target
/// carries.
fn rooted(path: &str) -> String {
    if cfg!(windows) {
        format!("C:/{path}")
    } else {
        format!("/{path}")
    }
}

fn reading(absolute: &str, below_root: Option<&str>) -> Sensitivity {
    Sensitivity::ReadOnly {
        target: at(absolute, below_root),
    }
}

fn writing(absolute: &str, below_root: Option<&str>) -> Sensitivity {
    Sensitivity::MutatesFile {
        target: at(absolute, below_root),
    }
}

fn running(parts: &[&str]) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            parts: parts.iter().map(|part| (*part).into()).collect(),
        },
    }
}

fn opaque(text: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Opaque(text.into()),
    }
}

fn rules(written: &[(Disposition, &str)]) -> Rules {
    let mut rules = Rules::new();
    for (kind, text) in written {
        rules.add(*kind, text).expect("the test wrote a valid rule");
    }
    rules
}

#[test]
fn nothing_is_said_about_a_call_no_rule_names() {
    let rules = rules(&[(Disposition::Allow, "read(src/**)")]);

    assert_eq!(
        rules.stated(&call("read"), &reading("/w/docs/a.md", Some("docs/a.md"))),
        None,
        "the mode decides when the rules are silent"
    );
}

#[test]
fn a_rule_is_about_one_tool() {
    let rules = rules(&[(Disposition::Allow, "read(src/**)")]);

    assert_eq!(
        rules.stated(&call("write"), &writing("/w/src/a.rs", Some("src/a.rs"))),
        None,
        "a rule written about read says nothing about write"
    );
}

#[test]
fn deny_beats_allow_however_much_narrower_the_allow_is() {
    // The specific pattern loses to the general one, which is the whole point:
    // precedence is by kind so a deny list can be read on its own.
    let rules = rules(&[
        (Disposition::Deny, "read(**)"),
        (Disposition::Allow, "read(src/main.rs)"),
    ]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &reading("/w/src/main.rs", Some("src/main.rs"))
        ),
        Some(Disposition::Deny)
    );
}

#[test]
fn ask_beats_allow() {
    let rules = rules(&[
        (Disposition::Ask, "write(**)"),
        (Disposition::Allow, "write(src/**)"),
    ]);

    assert_eq!(
        rules.stated(&call("write"), &writing("/w/src/a.rs", Some("src/a.rs"))),
        Some(Disposition::Ask)
    );
}

#[test]
fn a_star_stops_at_a_directory_boundary_in_a_path() {
    let rules = rules(&[(Disposition::Allow, "read(src/*)")]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &reading("/w/src/main.rs", Some("src/main.rs"))
        ),
        Some(Disposition::Allow)
    );
    assert_eq!(
        rules.stated(
            &call("read"),
            &reading("/w/src/cli/args.rs", Some("src/cli/args.rs"))
        ),
        None,
        "src/* names the files in src, not the tree under it"
    );
}

#[test]
fn a_double_star_reaches_the_whole_tree() {
    let rules = rules(&[(Disposition::Allow, "read(src/**)")]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &reading("/w/src/cli/args.rs", Some("src/cli/args.rs"))
        ),
        Some(Disposition::Allow)
    );
}

#[test]
fn a_relative_pattern_is_matched_below_the_root() {
    let rules = rules(&[(Disposition::Deny, "read(.env)")]);

    // The same file named absolutely. A relative pattern must not accidentally
    // match against the absolute spelling, or `.env` would also mean
    // `/anywhere/.env`.
    assert_eq!(
        rules.stated(&call("read"), &reading("/w/.env", Some(".env"))),
        Some(Disposition::Deny)
    );
    assert_eq!(
        rules.stated(&call("read"), &reading("/elsewhere/.env", None)),
        None
    );
}

#[test]
fn an_absolute_pattern_is_matched_against_the_resolved_path() {
    // Whether a pattern is absolute is `is_absolute`, which wants a drive or a
    // share on Windows rather than a leading slash — so a rule hard-coding the
    // Unix spelling would be read as a relative one there and this test would
    // be about the wrong branch entirely.
    let rules = rules(&[(Disposition::Deny, &format!("read({}/**)", rooted("etc")))]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &reading(&format!("{}/shadow", rooted("etc")), None)
        ),
        Some(Disposition::Deny),
        "a file no relative spelling can name is still reachable by an absolute rule"
    );
}

#[test]
fn a_path_that_did_not_resolve_matches_no_pattern() {
    let rules = rules(&[(Disposition::Allow, "read(**)")]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &Sensitivity::ReadOnly {
                target: Target::unresolved()
            }
        ),
        None,
        "a rule may not allow something nobody could name"
    );
}

#[test]
fn a_blanket_covers_even_what_could_not_be_named() {
    let rules = rules(&[(Disposition::Deny, "read")]);

    assert_eq!(
        rules.stated(
            &call("read"),
            &Sensitivity::ReadOnly {
                target: Target::unresolved()
            }
        ),
        Some(Disposition::Deny),
        "a rule saying everything has nothing to be misled about"
    );
}

#[test]
fn a_tool_named_alone_and_a_star_are_the_same_rule() {
    let named = rules(&[(Disposition::Allow, "bash")]);
    let starred = rules(&[(Disposition::Allow, "bash(*)")]);
    let command = running(&["curl http://example.invalid"]);

    assert_eq!(
        named.stated(&call("bash"), &command),
        starred.stated(&call("bash"), &command)
    );
    assert_eq!(
        named.stated(&call("bash"), &command),
        Some(Disposition::Allow)
    );
}

#[test]
fn a_star_where_the_tool_goes_is_every_tool() {
    // The same reading as inside the brackets: `*` is everything the position
    // it sits in can hold. A rule accepted here and matched against nothing
    // would be the one failure this mechanism cannot survive.
    let rules = rules(&[(Disposition::Deny, "*(.env)")]);
    let secret = reading("/w/.env", Some(".env"));

    assert_eq!(
        rules.stated(&call("read"), &secret),
        Some(Disposition::Deny)
    );
    assert_eq!(
        rules.stated(&call("grep"), &secret),
        Some(Disposition::Deny)
    );
}

#[test]
fn a_tool_spelled_with_a_capital_is_the_same_tool() {
    // Every tool is named in lower case, so `Bash` is somebody writing one of
    // them the way a sentence would rather than naming a second tool. Reading
    // it as a second tool accepts the rule and then matches it against
    // nothing, which is the shape of protection without any.
    let rules = rules(&[(Disposition::Deny, "Bash(*)")]);

    assert_eq!(
        rules.stated(&call("bash"), &running(&["curl http://example.invalid"])),
        Some(Disposition::Deny)
    );
}

#[test]
fn every_constituent_of_a_command_has_to_be_allowed() {
    let rules = rules(&[(Disposition::Allow, "bash(git status)")]);

    assert_eq!(
        rules.stated(&call("bash"), &running(&["git status"])),
        Some(Disposition::Allow)
    );
    assert_eq!(
        rules.stated(
            &call("bash"),
            &running(&["git status", "curl http://example.invalid"])
        ),
        None,
        "the part nobody wrote a rule about is what the question is for"
    );
}

#[test]
fn one_denied_constituent_denies_the_whole_command() {
    let rules = rules(&[
        (Disposition::Deny, "bash(curl *)"),
        (Disposition::Allow, "bash(*)"),
    ]);

    assert_eq!(
        rules.stated(
            &call("bash"),
            &running(&["git status", "curl http://example.invalid"])
        ),
        Some(Disposition::Deny)
    );
}

#[test]
fn a_star_spans_separators_in_a_command() {
    // A command is not a path: `cargo *` has to reach `cargo test --all
    // --manifest-path crates/crucible-core/Cargo.toml`.
    let rules = rules(&[(Disposition::Allow, "bash(cargo *)")]);

    assert_eq!(
        rules.stated(
            &call("bash"),
            &running(&["cargo test --manifest-path crates/crucible-core/Cargo.toml"])
        ),
        Some(Disposition::Allow)
    );
}

#[test]
fn a_command_that_decomposed_into_nothing_is_not_allowed() {
    let rules = rules(&[(Disposition::Allow, "bash(git *)")]);

    assert_eq!(
        rules.stated(&call("bash"), &running(&[])),
        None,
        "vacuous truth must not become an allow"
    );
}

#[test]
fn only_a_blanket_speaks_about_a_command_nobody_could_read() {
    let narrow = rules(&[(Disposition::Allow, "bash(git *)")]);
    let blanket = rules(&[(Disposition::Allow, "bash(*)")]);
    let command = opaque("git $(cat payload)");

    assert_eq!(narrow.stated(&call("bash"), &command), None);
    assert_eq!(
        blanket.stated(&call("bash"), &command),
        Some(Disposition::Allow)
    );
}

#[test]
fn a_deny_blanket_stops_a_command_nobody_could_read() {
    let rules = rules(&[(Disposition::Deny, "bash(*)")]);

    assert_eq!(
        rules.stated(&call("bash"), &opaque("$(printf rm) -rf /")),
        Some(Disposition::Deny)
    );
}

#[test]
fn no_rules_at_all_says_so() {
    assert!(Rules::new().is_empty());
    assert!(!rules(&[(Disposition::Allow, "read")]).is_empty());
}

#[test]
fn text_that_is_not_shaped_like_a_rule_is_refused() {
    let mut rules = Rules::new();

    for text in ["read(src/**", "", "()", "read()"] {
        assert!(
            rules.add(Disposition::Allow, text).is_err(),
            "{text} is not a rule and must not be read as one"
        );
    }
}

#[test]
fn a_rule_naming_no_tool_is_refused() {
    let mut rules = Rules::new();

    let err = rules
        .add(Disposition::Allow, "(src/**)")
        .expect_err("a rule about no tool matches nothing");

    assert!(err.to_string().contains("names no tool"));
}

#[test]
fn a_pattern_that_is_not_a_glob_is_refused() {
    let mut rules = Rules::new();

    let err = rules
        .add(Disposition::Deny, "read(src/[)")
        .expect_err("an uncompilable glob cannot be left to fail silently");

    assert!(err.to_string().contains("read(src/[)"));
}
