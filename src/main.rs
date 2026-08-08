//! The `crucible` binary.
//!
//! Nothing here but the entry point. What gets built and how it is wired lives
//! in [`cli`], which is the only place concrete types meet.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::start()
}
