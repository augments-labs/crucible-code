//! The `crucible` binary.
//!
//! This is the only place concrete types meet. Providers, tools, the renderer
//! and the runner are constructed here and handed to each other as trait
//! objects; every crate below is free of the others as a result.

use std::process::ExitCode;

use clap::Parser;

/// The command-line surface.
///
/// Unstable for the whole 0.0.x line: flags may be renamed or removed in any
/// 0.0.x release without a deprecation period.
#[derive(Debug, Parser)]
#[command(
    name = "crucible",
    version,
    about = "The harness where agents are forged."
)]
struct Cli {}

fn main() -> ExitCode {
    let Cli {} = Cli::parse();
    ExitCode::SUCCESS
}
