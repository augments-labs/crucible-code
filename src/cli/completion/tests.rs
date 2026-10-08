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
    // The tree declares none today; this holds the first one that is added.
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

/// Every long flag the tree declares, as the words that reach the command
/// holding it (none for the command line itself) and the flag with its dashes.
fn flags(command: &Command) -> BTreeSet<(Vec<String>, String)> {
    fn below(command: &Command, here: &[String], found: &mut BTreeSet<(Vec<String>, String)>) {
        for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
            if let Some(long) = arg.get_long() {
                found.insert((here.to_vec(), format!("--{long}")));
            }
        }
        for sub in command
            .get_subcommands()
            .filter(|sub| sub.get_name() != "help")
        {
            let mut deeper = here.to_vec();
            deeper.push(sub.get_name().to_owned());
            below(sub, &deeper, found);
        }
    }
    let mut found = BTreeSet::new();
    below(command, &[], &mut found);
    found
}

/// Whether `script` completes `flag` for the command reached by `path`.
///
/// Held where a shell's script keeps one command's words apart from the next:
/// bash's case arm, PowerShell's and elvish's block, fish's condition on each
/// line. zsh nests every command inside its parent's function, so there it is
/// held to the flag being offered at all, which still fails for a flag the
/// tree declares and the script never mentions.
fn completes_flag(shell: Shell, script: &str, path: &[String], flag: &str) -> bool {
    let between = |opening: &str, closing: &str| -> bool {
        script.find(opening).is_some_and(|start| {
            let rest = &script[start + opening.len()..];
            rest[..rest.find(closing).unwrap_or(rest.len())].contains(flag)
        })
    };
    let dotted = path
        .iter()
        .fold(String::new(), |joined, word| joined + ";" + word);
    match shell {
        Shell::Bash => {
            let arm = if path.is_empty() {
                "crucible".to_owned()
            } else {
                format!("crucible__subcmd__{}", path.join("__subcmd__"))
            };
            // The arm's `opts` line is the line before its `if`.
            between(&format!("\n        {arm})\n"), "\n            if")
        }
        Shell::Zsh => script.contains(&format!("{flag}[")) || script.contains(&format!("{flag}=[")),
        Shell::Fish => {
            let condition = match path {
                [] => "__fish_crucible_needs_command\"".to_owned(),
                [only] => format!("__fish_crucible_using_subcommand {only}"),
                [parent, child] => format!(
                    "__fish_crucible_using_subcommand {parent}; and __fish_seen_subcommand_from {child}\""
                ),
                _ => return false,
            };
            let long = format!(" -l {}", flag.trim_start_matches("--"));
            script.lines().any(|line| {
                line.starts_with("complete -c crucible")
                    && line.contains(&format!("-n \"{condition}"))
                    && (line.contains(&format!("{long} ")) || line.ends_with(&long))
            })
        }
        Shell::PowerShell => between(&format!("'crucible{dotted}' {{\n"), "break"),
        Shell::Elvish => between(&format!("&'crucible{dotted}'= {{\n"), "\n        }"),
        _ => false,
    }
}

/// The flags of `tree` that `script` does not complete, for `shell`.
fn missing_flags(shell: Shell, tree: &Command, script: &str) -> Vec<String> {
    flags(tree)
        .into_iter()
        .filter(|(path, flag)| !completes_flag(shell, script, path, flag))
        .map(|(path, flag)| format!("{} {flag}", path.join(" ")).trim().to_owned())
        .collect()
}

#[test]
fn the_flags_held_are_the_ones_the_command_line_is_used_through() {
    let held = flags(&Cli::command());
    let root = |flag: &str| (Vec::<String>::new(), flag.to_owned());

    for flag in [
        "--continue",
        "--resume",
        "--model",
        "--effort",
        "--with-mcp",
        "--extensions",
        "--sandbox",
    ] {
        assert!(held.contains(&root(flag)), "{flag} is not held");
    }
    let inspect = vec!["sandbox".to_owned(), "inspect".to_owned()];
    assert!(held.contains(&(inspect, "--json".to_owned())));
    let login = vec!["auth".to_owned(), "login".to_owned()];
    assert!(held.contains(&(login, "--api-key-stdin".to_owned())));
}

#[test]
fn every_shells_completion_script_completes_every_long_flag_in_the_tree() {
    for shell in SHELLS {
        let absent = missing_flags(shell, &Cli::command(), &text(shell));

        assert!(
            absent.is_empty(),
            "the {shell} completion script leaves out {absent:?}"
        );
    }
}

#[test]
fn a_flag_added_to_the_tree_after_a_script_was_made_is_reported_missing() {
    let grown = Cli::command().arg(clap::Arg::new("throwaway").long("throwaway"));
    let deeper = Cli::command().mut_subcommand("sandbox", |sandbox| {
        sandbox.mut_subcommand("inspect", |inspect| {
            inspect.arg(clap::Arg::new("throwaway").long("throwaway"))
        })
    });

    for shell in SHELLS {
        assert_eq!(
            missing_flags(shell, &grown, &text(shell)),
            ["--throwaway"],
            "the {shell} check did not see a flag its script was not made from"
        );
        // zsh's check is by flag alone, and `--throwaway` is nowhere in it.
        assert_eq!(
            missing_flags(shell, &deeper, &text(shell)),
            ["sandbox inspect --throwaway"],
            "the {shell} check did not see a flag on a subcommand"
        );
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
///
/// `bash` is not looked for outside unix: on Windows that name is the
/// launcher for the Windows Subsystem for Linux, which is not the shell a
/// Windows user's completion would be loaded by.
fn parsed_by(shell: Shell, script: &[u8]) -> Option<bool> {
    let mut command = match shell {
        Shell::Bash if cfg!(unix) => {
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

/// A script that is not one in `shell`'s syntax.
fn broken(shell: Shell) -> &'static [u8] {
    match shell {
        Shell::Fish => b"if true\n",
        Shell::PowerShell => b"if (",
        Shell::Elvish => b"if $true {",
        _ => b"if then fi (",
    }
}

#[test]
fn every_script_an_installed_shell_can_check_passes_that_shells_syntax_check() {
    let mut checked = Vec::new();
    let mut absent = Vec::new();
    for shell in SHELLS {
        // The control first: a shell whose flag checks nothing would pass
        // the script below for the wrong reason.
        let Some(accepted) = parsed_by(shell, broken(shell)) else {
            absent.push(shell.to_string());
            continue;
        };
        assert!(!accepted, "{shell} accepted a script broken in its syntax");
        assert_eq!(
            parsed_by(shell, &script(shell)),
            Some(true),
            "the {shell} completion script does not parse"
        );
        checked.push(shell.to_string());
    }

    // Shown with `--nocapture`; the pull request names the same.
    eprintln!("syntax checked: {checked:?}; shell not installed here: {absent:?}");
    if cfg!(unix) {
        assert!(
            checked.iter().any(|shell| shell == "bash"),
            "bash is on every unix this is run on, and was not checked"
        );
    }
}
