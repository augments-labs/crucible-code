//! What a yes is allowed to become when it is written down.

use super::super::super::{Command, Sensitivity, Target};
use super::super::{Disposition, Rules};
use super::*;
use crate::ids::ToolId;
use crate::tool::ToolArgs;
use crate::workspace::Workspace;

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolId::new("call-1"),
        name: name.into(),
        args: ToolArgs::new("{}"),
    }
}

fn writing(absolute: &str, below_root: Option<&str>) -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::at(absolute, below_root),
    }
}

fn running(parts: &[&str]) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            // A line these parts would in fact decompose from. Nothing here is
            // about what a question shows, but a fixture that spelled the two
            // out of step would be one nobody could read that from.
            sent: parts.join(" && ").into(),
            parts: parts.iter().map(|part| (*part).into()).collect(),
        },
    }
}

/// What the rule reads as, for the assertions about its text.
fn minted(tool: &str, sensitivity: &Sensitivity) -> Option<String> {
    narrowest(&call(tool), sensitivity).map(|rule| rule.as_str().to_owned())
}

/// What that rule then says about a call — the only question that matters
/// about it, since it is written to be read back.
fn covers(text: &str, tool: &str, sensitivity: &Sensitivity) -> bool {
    let mut rules = Rules::new();
    rules
        .add(Disposition::Allow, text)
        .expect("a minted rule is a readable one");

    rules.stated(&call(tool), sensitivity) == Some(Disposition::Allow)
}

#[test]
fn a_command_is_written_down_as_the_command_and_not_the_program() {
    // `bash(cargo *)` would be the generalisation: agreeing to run the tests
    // is not agreeing to `cargo publish`.
    assert_eq!(
        minted("bash", &running(&["cargo test"])).as_deref(),
        Some("bash(cargo test)")
    );
}

#[test]
fn a_file_is_written_down_as_the_file_and_not_the_tree_it_sits_in() {
    assert_eq!(
        minted("edit", &writing("/w/src/main.rs", Some("src/main.rs"))).as_deref(),
        Some("edit(src/main.rs)")
    );
}

#[test]
fn a_file_below_the_root_is_written_down_the_way_it_was_shown() {
    // Two spellings name it, and the rule takes the one somebody would
    // recognise reading their own configuration back.
    assert_eq!(
        minted("write", &writing("/w/notes.md", Some("notes.md"))).as_deref(),
        Some("write(notes.md)")
    );
}

#[test]
fn a_file_no_relative_spelling_can_name_is_written_absolutely() {
    // A file in an extra directory has no path below the working directory, so
    // the absolute one is not a fallback but the only name it has.
    assert_eq!(
        minted("edit", &writing("/elsewhere/notes.md", None)).as_deref(),
        Some("edit(/elsewhere/notes.md)")
    );
}

#[test]
fn a_rule_minted_from_a_file_the_workspace_resolved_covers_that_file() {
    // Through the resolution a real call goes through, rather than through
    // `Target::at`, which is these tests spelling a path themselves. The
    // separator is picked up there and nowhere else, and a rule minted from a
    // path still spelled the way Windows spells one escapes it as a literal
    // character — so this passes everywhere while saying `[\]` on Windows, and
    // the file it was minted from stops matching the moment it is written down.
    let base = std::env::temp_dir().join(format!("crucible-mint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("src")).expect("a writable temporary directory");
    std::fs::write(base.join("src").join("main.rs"), "").expect("a writable temporary directory");

    let workspace = Workspace::open(&base).expect("a directory that is there");
    let path = workspace
        .existing("src/main.rs")
        .expect("a file inside the workspace");
    let target = Target::resolved(&workspace, &path);

    // The other spelling, which a file in an extra directory has only, so no
    // rule about one can be written any other way. Resolving a path on Windows
    // returns it extended-length — `\\?\C:\...` — and a rule somebody wrote as
    // `C:/...` is matched against whatever this holds.
    let absolute = target.absolute().expect("a resolved path").to_owned();
    assert!(
        !absolute.starts_with("//?/"),
        "{absolute} keeps the prefix resolving put on it"
    );
    assert!(
        !absolute.contains('\\'),
        "{absolute} keeps a separator a pattern reads as an escape"
    );

    let sensitivity = Sensitivity::MutatesFile { target };
    let text = minted("edit", &sensitivity).expect("a resolved file can be written down");
    assert!(
        covers(&text, "edit", &sensitivity),
        "{text} does not cover the file it was minted from"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// A file whose name holds a backslash, where one can be named that.
///
/// Unix reads it as an ordinary character, so the escaping is exactly what
/// keeps a rule minted from such a file from being a pattern. Windows reads it
/// as a separator, no file may be named with one, and `Target` has turned it
/// into `/` long before a rule is minted — so there is nothing to prove there
/// and a case asserting otherwise would be asserting about a path that cannot
/// occur.
#[cfg(not(windows))]
const BACKSLASH: [(&str, &str); 1] = [("src/back\\slash.rs", "src/backslash.rs")];

/// Nothing: see the Unix half.
#[cfg(windows)]
const BACKSLASH: [(&str, &str); 0] = [];

#[test]
fn a_minted_rule_covers_what_it_came_from_and_nothing_beside_it() {
    // The property the whole module is for, and the one place the escaping is
    // observable: a file may be named with the characters a glob reads, and a
    // rule minted from one must not have quietly become a pattern.
    //
    // The space cases are the reader's half of the same property: `parse`
    // trims inside the brackets, so a space left plain at either end would be
    // gone by the time the rule is read back and the rule would name the
    // neighbour it is paired with here.
    let cases = [
        ("src/plain.rs", "src/plainer.rs"),
        ("src/a*.rs", "src/abc.rs"),
        ("src/a?.rs", "src/ab.rs"),
        ("src/a[xy].rs", "src/ax.rs"),
        ("src/a{x,y}.rs", "src/ax.rs"),
        ("src/a]b.rs", "src/ab.rs"),
        (" secret.txt", "secret.txt"),
        ("notes.txt ", "notes.txt"),
    ];

    for (shown, sibling) in cases.into_iter().chain(BACKSLASH) {
        let sensitivity = writing(&format!("/w/{shown}"), Some(shown));
        let text = minted("edit", &sensitivity).expect("a resolved file can be written down");

        assert!(
            covers(&text, "edit", &sensitivity),
            "{text} does not cover {shown}, which it was minted from"
        );
        assert!(
            !covers(
                &text,
                "edit",
                &writing(&format!("/w/{sibling}"), Some(sibling))
            ),
            "{text} covers {sibling} as well, so it is wider than the yes it came from"
        );
    }
}

#[test]
fn a_file_whose_name_is_nothing_but_a_space_still_mints_a_rule_that_reads_back() {
    // The degenerate end of the same case. Written plainly this is `edit( )`,
    // and the reader trims the brackets empty and refuses the whole rule — a
    // line crucible put into its own configuration that then stops it starting
    // until somebody finds and deletes it.
    let sensitivity = writing("/w/ ", Some(" "));
    let text = minted("edit", &sensitivity).expect("a resolved file can be written down");

    assert_eq!(text, "edit([ ])");
    assert!(
        covers(&text, "edit", &sensitivity),
        "{text} does not cover the file it was minted from"
    );
}

#[test]
fn a_command_holding_a_glob_character_is_written_down_as_that_command() {
    // `rm *.tmp` is a line somebody runs. Written unescaped it would be a rule
    // about `rm <anything>.tmp`, and in a command pattern `*` spans separators
    // too — so it would reach `rm -rf /` as well.
    let sensitivity = running(&["rm *.tmp"]);
    let text = minted("bash", &sensitivity).expect("one command can be written down");

    assert!(covers(&text, "bash", &sensitivity));
    assert!(
        !covers(&text, "bash", &running(&["rm anything.tmp"])),
        "{text} covers a second command, so it is wider than the yes it came from"
    );
    assert!(
        !covers(&text, "bash", &running(&["rm -rf /"])),
        "{text} reaches past the line it was minted from"
    );
}

#[test]
fn nothing_is_written_down_for_a_line_that_is_more_than_one_command() {
    // A rule per constituent would let each of them run alone from here on,
    // which is not what agreeing to the line said.
    assert_eq!(
        minted(
            "bash",
            &running(&["git status", "curl http://example.invalid"])
        ),
        None
    );
}

#[test]
fn nothing_is_written_down_for_a_command_nobody_could_read() {
    assert_eq!(
        minted(
            "bash",
            &Sensitivity::SpawnsProcess {
                command: Command::Opaque("git $(cat payload)".into()),
            }
        ),
        None
    );
}

#[test]
fn nothing_is_written_down_for_a_path_that_did_not_resolve() {
    assert_eq!(
        minted(
            "edit",
            &Sensitivity::MutatesFile {
                target: Target::unresolved(),
            }
        ),
        None
    );
}

#[test]
fn a_rule_minted_from_a_host_names_the_host_and_not_the_page() {
    // A yes to one page is not a yes to the site, and a rule naming the page is
    // a rule that never matches again — the next request carries another path.
    // The host is the narrowest subject two calls can actually share.
    let minted = narrowest(
        &call("web_fetch"),
        &Sensitivity::ReachesNetwork {
            host: Host::Named {
                sent: "https://docs.rs/serde/latest/serde/".into(),
                host: "docs.rs".into(),
            },
        },
    )
    .expect("a host to mint a rule");

    assert_eq!(minted.to_string(), "web_fetch(docs.rs)");
}

#[test]
fn nothing_is_written_down_for_a_url_nobody_could_read() {
    assert!(
        narrowest(
            &call("web_fetch"),
            &Sensitivity::ReachesNetwork {
                host: Host::Opaque("https://docs.rs@evil.example/".into()),
            },
        )
        .is_none()
    );
}
