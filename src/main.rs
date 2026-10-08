//! The `crucible` binary.
//!
//! Nothing here but the entry point. What gets built and how it is wired lives
//! in [`cli`], which is the only place concrete types meet. The one thing done
//! before it is to settle where the sandbox broker is looked for, while the
//! executable's path still names the release this process was started from.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::panicked::written();
    crucible_sandbox_local::hold_broker_directory();
    cli::start()
}
