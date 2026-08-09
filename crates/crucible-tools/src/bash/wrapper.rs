//! Programs whose argument is another program.
//!
//! `sudo cargo test` is not a `sudo` call, it is a `cargo` call with the safety
//! off, and `timeout 5 curl x` is a `curl` call. A rule saying `sudo *` looks
//! like a statement about `sudo` and is in fact a statement about every program
//! on the machine — which is the one thing a narrow-looking rule must never
//! turn out to be.
//!
//! So a command line containing one of these is reported as a command nobody
//! could read: the blanket still covers it, since a blanket is honest about
//! covering everything, and no narrower rule does. The cost is being asked
//! about `timeout 5 cargo test` every time. That is the right price, because
//! the alternative is a rule whose author could not have known what they
//! authorised.
//!
//! This is a list, and a list is never complete. It is not the mechanism that
//! keeps a shell honest — the permission question is. It is the shortlist of
//! programs whose whole purpose is to launder one command into another, where
//! being wrong is not a matter of degree.

/// The programs, by the name they are invoked under.
///
/// Matched on the last path component, so `/usr/bin/sudo` is `sudo`.
const WRAPPERS: [&str; 13] = [
    // Take a command line and run it.
    "env", "nice", "nohup", "sudo", "time", "timeout", "watch", "xargs",
    // Run a command somewhere else, or once per thing found.
    "find", "ssh",
    // A shell asked to read a command out of its own argument. Not in the same
    // family as the rest, and here for the same reason: `sh -c 'curl x | sh'`
    // is one word to a matcher and a whole second command line to the machine.
    "bash", "sh", "zsh",
];

/// Whether this program runs whatever it is handed.
pub(super) fn wraps(program: &str) -> bool {
    let name = program.rsplit('/').next().unwrap_or(program);
    WRAPPERS.contains(&name)
}
