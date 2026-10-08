//! That the scripts and the parser's tree say the same thing.
//!
//! Nothing is frozen beside these tests: the scripts are generated from the
//! tree each time, and what is held is that every command the tree declares
//! reaches every shell's script, and that the tree declares only the commands
//! this file names. A command added without being named here turns the second
//! red, which is what makes adding one a decision about its completion rather
//! than an accident of it.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::process::{Command as Process, Stdio};

use clap::{Command, CommandFactory as _};
use clap_complete::Shell;

use super::*;

/// Every subcommand the tree declares, as the words that reach it, in the
/// order the tree lists them. `help` is clap's own and is left out.
const COMMANDS: &[&[&str]] = &[
    &["sandbox"],
    &["sandbox", "inspect"],
    &["sandbox", "setup"],
    &["sandbox", "uninstall"],
    &["config"],
    &["config", "check"],
    &["doctor"],
    &["auth"],
    &["auth", "status"],
    &["auth", "login"],
    &["auth", "logout"],
    &["mcp"],
    &["mcp", "list"],
    &["mcp", "get"],
    &["extensions"],
    &["extensions", "list"],
    &["sessions"],
    &["sessions", "list"],
    &["completion"],
];

const SHELLS: [Shell; 5] = [
    Shell::Bash,
    Shell::Zsh,
    Shell::Fish,
    Shell::PowerShell,
    Shell::Elvish,
];

/// Every subcommand of `command`, below it at any depth, as the words that
/// reach it.
fn paths(command: &Command) -> BTreeSet<Vec<String>> {
    fn below(command: &Command, above: &[String], found: &mut BTreeSet<Vec<String>>) {
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" {
                continue;
            }
            let mut here = above.to_vec();
            here.push(sub.get_name().to_owned());
            below(sub, &here, found);
            found.insert(here);
        }
    }
    let mut found = BTreeSet::new();
    below(command, &[], &mut found);
    found
}

/// What a generated script holds when it completes the command reached by
/// `path`, which differs by shell: the text each shell's generator writes for
/// a command it knows. Written down per shell rather than guessed at, since a
/// word that merely appears somewhere in a script proves nothing about it.
fn marker(shell: Shell, path: &[String]) -> String {
    match shell {
        Shell::Bash => format!("crucible__subcmd__{})", path.join("__subcmd__")),
        Shell::Zsh => format!("_crucible__subcmd__{}_commands()", path.join("__subcmd__")),
        Shell::Fish => match path {
            [only] => format!("-a \"{only}\""),
            [parent, child] => {
                format!(
                    "__fish_crucible_using_subcommand {parent}; and __fish_seen_subcommand_from {child}"
                )
            }
            // The fish generator stops at two levels, so a deeper command has
            // no marker, and the test that asks for one fails rather than
            // passing for a command nothing completes.
            _ => format!("fish completes no command nested {} deep", path.len()),
        },
        Shell::PowerShell => format!("'crucible;{}' {{", path.join(";")),
        Shell::Elvish => format!("&'crucible;{}'= {{", path.join(";")),
        other => format!("no marker is written for {other}"),
    }
}

/// The commands of `tree` that `script` does not complete, for `shell`.
fn missing(shell: Shell, tree: &Command, script: &str) -> Vec<String> {
    paths(tree)
        .into_iter()
        .filter(|path| !script.contains(&marker(shell, path)))
        .map(|path| path.join(" "))
        .collect()
}

fn text(shell: Shell) -> String {
    String::from_utf8(script(shell)).expect("a script is text")
}

#[test]
fn the_tree_declares_the_commands_the_completion_agreement_names() {
    let named: BTreeSet<Vec<String>> = COMMANDS
        .iter()
        .map(|path| path.iter().map(|word| (*word).to_owned()).collect())
        .collect();

    assert_eq!(
        paths(&Cli::command()),
        named,
        "a command was added to or removed from the tree; name it in COMMANDS so its completion is held"
    );
}

#[test]
fn every_shells_completion_script_completes_every_command_in_the_tree() {
    for shell in SHELLS {
        let absent = missing(shell, &Cli::command(), &text(shell));

        assert!(
            absent.is_empty(),
            "the {shell} completion script leaves out {absent:?}"
        );
    }
}

#[test]
fn every_shells_completion_script_names_the_aliases_the_tree_shows() {
    fn visible(command: &Command, found: &mut Vec<String>) {
        found.extend(command.get_visible_aliases().map(str::to_owned));
        for sub in command.get_subcommands() {
            visible(sub, found);
        }
    }
    let mut aliases = Vec::new();
    visible(&Cli::command(), &mut aliases);

    for shell in SHELLS {
        let script = text(shell);
        for alias in &aliases {
            assert!(
                script.contains(alias.as_str()),
                "the {shell} completion script never says the alias {alias}"
            );
        }
    }
}

#[test]
fn a_command_added_to_the_tree_after_a_script_was_made_is_reported_missing() {
    // The check above is only worth having if it can fail: a script made from
    // the tree as it stands, held against the tree with one more command in it.
    let grown = Cli::command().subcommand(Command::new("throwaway"));

    for shell in SHELLS {
        assert_eq!(
            missing(shell, &grown, &text(shell)),
            ["throwaway"],
            "the {shell} check did not see a command its script was not made from"
        );
    }
}

#[test]
fn every_script_is_made_for_the_name_crucible_runs_as() {
    for shell in SHELLS {
        assert!(
            text(shell).contains("crucible"),
            "the {shell} script names no command"
        );
    }
}

/// Whether `shell`'s own parser accepts `script`, or `None` where the shell
/// is not installed here.
fn parsed_by(shell: Shell, script: &[u8]) -> Option<bool> {
    let mut command = match shell {
        Shell::Bash => {
            let mut command = Process::new("bash");
            command.arg("-n");
            command
        }
        Shell::Zsh => {
            let mut command = Process::new("zsh");
            command.arg("-n");
            command
        }
        Shell::Fish => {
            let mut command = Process::new("fish");
            command.arg("--no-execute");
            command
        }
        Shell::PowerShell => {
            let mut command = Process::new("pwsh");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$null = [scriptblock]::Create([Console]::In.ReadToEnd())",
            ]);
            command
        }
        Shell::Elvish => {
            let mut command = Process::new("elvish");
            command.arg("-compileonly");
            command
        }
        _ => return None,
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A shell that has said what it thinks stops reading; the verdict is its
    // exit, not whether it took every byte.
    let _ = child.stdin.take()?.write_all(script);
    Some(child.wait().ok()?.success())
}

#[test]
fn every_script_an_installed_shell_can_check_passes_that_shells_syntax_check() {
    for shell in SHELLS {
        if let Some(parsed) = parsed_by(shell, &script(shell)) {
            assert!(parsed, "the {shell} completion script does not parse");
        }
    }
}

#[test]
fn the_syntax_check_refuses_a_script_that_is_not_one() {
    // Without this the check above passes for a shell whose flag does nothing.
    if let Some(parsed) = parsed_by(Shell::Bash, b"if then fi (") {
        assert!(!parsed, "bash -n accepted a broken script");
    }
}
