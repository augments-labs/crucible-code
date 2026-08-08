//! Permission, as a value that cannot be forged.
//!
//! A function that mutates a file or spawns a process takes a [`Grant`]. A
//! bare verdict argument would not be enough — nothing stops a caller passing
//! `Deny` and carrying on — so the argument is a token whose only field is
//! private to this module. [`Permission::decide`] is the only thing in the
//! program that can mint one, and it mints only after a verdict came back yes.
//!
//! Read-only calls take the same path. They are approved without a prompt, but
//! they still leave through `decide`, so there is one route to running a tool
//! rather than a guarded one and an unguarded one.

use std::collections::HashSet;
use std::fmt;

use crate::tool::ToolCall;

/// How much damage a particular call could do.
///
/// A closed set: a new kind of danger must break every `match` that decides
/// what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sensitivity {
    /// Reads only. Cannot change the workspace or the machine.
    ReadOnly,
    /// Creates, changes or removes a file inside the workspace.
    MutatesFile,
    /// Runs another program, which can do anything the user can.
    SpawnsProcess {
        /// What is about to run, named the way the user would name it.
        ///
        /// The tool works this out, because the tool is the only thing that
        /// knows how to read its own arguments. It is what the prompt shows
        /// and what a session-wide allow remembers, so a tool that reports a
        /// vague program gets a vague question and a wide grant.
        program: Box<str>,
    },
}

/// A permission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Yes, for this call only.
    AllowOnce,
    /// Yes, and stop asking for calls like this one until the session ends.
    AllowSession,
    /// No.
    Deny,
}

/// Proof that a decision was made and that it was yes.
///
/// The field is private to this module, so a `Grant` cannot be constructed
/// anywhere else — not in another crate, and not in another module of this
/// one. A tool therefore cannot run without a verdict having been reached.
#[derive(Debug)]
pub struct Grant(());

impl Grant {
    /// Private on purpose: see the module documentation. Only
    /// [`Permission::decide`] calls this.
    fn issue(verdict: Verdict) -> Option<Self> {
        match verdict {
            Verdict::AllowOnce | Verdict::AllowSession => Some(Self(())),
            Verdict::Deny => None,
        }
    }
}

/// Asks the user for a verdict.
///
/// Implemented by the renderer, which owns the terminal. Kept as a trait so
/// core does not depend on how the question is asked, and so the runner can be
/// tested against an answer that is decided in advance.
pub trait Ask {
    /// Puts the call to the user and returns what they said.
    fn ask(&mut self, call: &ToolCall, sensitivity: &Sensitivity) -> Verdict;
}

/// The permission engine, and the session's memory of what was already
/// allowed.
///
/// The memory lives as long as the process and is never written to disk. A
/// grant that outlives a session is a different feature.
#[derive(Debug, Default)]
pub struct Permission {
    remembered: HashSet<Box<str>>,
}

impl Permission {
    /// A session that has been asked nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decides whether a call may run, asking the user if it has to.
    ///
    /// Returns the token on yes and `None` on no. There is no third answer and
    /// no way to obtain the token except from here.
    pub fn decide(
        &mut self,
        call: &ToolCall,
        sensitivity: &Sensitivity,
        ask: &mut dyn Ask,
    ) -> Option<Grant> {
        if *sensitivity == Sensitivity::ReadOnly {
            return Grant::issue(Verdict::AllowOnce);
        }

        let scope = Self::scope(call, sensitivity);
        if self.remembered.contains(&scope) {
            return Grant::issue(Verdict::AllowOnce);
        }

        let verdict = ask.ask(call, sensitivity);
        if verdict == Verdict::AllowSession {
            self.remembered.insert(scope);
        }
        Grant::issue(verdict)
    }

    /// What "allow for this session" remembers.
    ///
    /// For a file tool it is the tool name: `write` and `edit` are already
    /// confined to the workspace, so remembering the name does not widen what
    /// they can reach. For a process tool it is the tool name *and* the
    /// program, because a session-wide grant to spawn anything is exactly the
    /// hole this is meant to avoid — allowing `cargo` should not also allow
    /// `curl`.
    fn scope(call: &ToolCall, sensitivity: &Sensitivity) -> Box<str> {
        match sensitivity {
            Sensitivity::ReadOnly | Sensitivity::MutatesFile => call.name.clone(),
            Sensitivity::SpawnsProcess { program } => format!("{}:{program}", call.name).into(),
        }
    }
}

impl fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => f.write_str("read"),
            Self::MutatesFile => f.write_str("change files"),
            Self::SpawnsProcess { program } => write!(f, "run {program}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ToolId;
    use crate::tool::ToolArgs;

    /// An answer decided in advance, plus a count of how often it was needed.
    struct Answer {
        verdict: Verdict,
        asked: usize,
    }

    impl Answer {
        fn new(verdict: Verdict) -> Self {
            Self { verdict, asked: 0 }
        }
    }

    impl Ask for Answer {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> Verdict {
            self.asked += 1;
            self.verdict
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new("call-1"),
            name: name.into(),
            args: ToolArgs::new("{}"),
        }
    }

    fn running(program: &str) -> Sensitivity {
        Sensitivity::SpawnsProcess {
            program: program.into(),
        }
    }

    #[test]
    fn deny_yields_no_grant() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::Deny);

        let grant = permission.decide(&call("write"), &Sensitivity::MutatesFile, &mut answer);

        assert!(grant.is_none(), "a denied call must not produce a grant");
    }

    #[test]
    fn allow_once_yields_a_grant_and_is_not_remembered() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::AllowOnce);
        let call = call("write");

        assert!(
            permission
                .decide(&call, &Sensitivity::MutatesFile, &mut answer)
                .is_some()
        );
        assert!(
            permission
                .decide(&call, &Sensitivity::MutatesFile, &mut answer)
                .is_some()
        );

        assert_eq!(answer.asked, 2, "allow-once must ask every time");
    }

    #[test]
    fn allow_session_stops_the_asking() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::AllowSession);
        let call = call("write");

        for _ in 0..3 {
            assert!(
                permission
                    .decide(&call, &Sensitivity::MutatesFile, &mut answer)
                    .is_some()
            );
        }

        assert_eq!(answer.asked, 1, "allow-session must ask exactly once");
    }

    #[test]
    fn a_read_only_call_is_never_put_to_the_user() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::Deny);

        let grant = permission.decide(&call("read"), &Sensitivity::ReadOnly, &mut answer);

        assert!(grant.is_some(), "reads are allowed");
        assert_eq!(answer.asked, 0, "reads must not prompt");
    }

    #[test]
    fn allowing_one_program_for_the_session_does_not_allow_another() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::AllowSession);
        let call = call("bash");

        permission.decide(&call, &running("cargo"), &mut answer);
        assert_eq!(answer.asked, 1);

        // Same tool, different program. This is the case a tool-name-only
        // memory would wave through.
        permission.decide(&call, &running("curl"), &mut answer);
        assert_eq!(answer.asked, 2, "a different program must be asked about");

        // The first program stays remembered.
        permission.decide(&call, &running("cargo"), &mut answer);
        assert_eq!(answer.asked, 2, "the allowed program stays allowed");
    }

    #[test]
    fn allowing_a_file_tool_does_not_allow_a_process_tool() {
        let mut permission = Permission::new();
        let mut answer = Answer::new(Verdict::AllowSession);

        permission.decide(&call("write"), &Sensitivity::MutatesFile, &mut answer);
        permission.decide(&call("bash"), &running("ls"), &mut answer);

        assert_eq!(answer.asked, 2);
    }

    #[test]
    fn the_question_names_the_program_rather_than_the_category() {
        // "run a program" tells the user nothing they can decide on.
        assert_eq!(running("cargo").to_string(), "run cargo");
        assert_eq!(Sensitivity::MutatesFile.to_string(), "change files");
    }
}
