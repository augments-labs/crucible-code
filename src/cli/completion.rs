//! Shell completion scripts, written from the command tree the parser holds.
//!
//! `crucible completion SHELL` writes one script to standard output and stops.
//! The script is generated from [`Cli`]'s own tree each time it is asked for,
//! so a flag renamed or a subcommand added is in the next script without
//! anyone regenerating anything, and nothing is committed that could disagree
//! with the parser. Answered before configuration is read, a terminal opened
//! or a runtime built, for the reason [`super::checked`] gives: someone sets
//! completion up from a shell's start-up file, where a command that touched
//! the home directory or the network would slow every new terminal.
//!
//! The script is built in memory and written once. The generator treats a
//! failed write as unrecoverable and panics, and standard output closing early
//! is `head` on the far side of a pipe, which is not a failure of this run.

use std::io::{self, Write as _};
use std::process::ExitCode;

use clap::CommandFactory as _;
use clap_complete::Shell;

use super::Cli;

/// The script `shell` completes `crucible` with, as the bytes to write.
fn script(shell: Shell) -> Vec<u8> {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, name, &mut script);
    script
}

/// Writes the script for `shell` to standard output, and stops.
///
/// A write that fails is dropped for the reason [`super::listed`] drops one.
pub(super) fn completed(shell: Shell) -> ExitCode {
    let _ = io::stdout().write_all(&script(shell));
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests;
