//! A line protocol of the proofs' own, which no vendor speaks.
//!
//! One word, a space, and the rest: `say`, `file`, `session`, `ask`, `fail`,
//! `end` and `cancelled` from the program, and `turn`, `allow`, `deny`,
//! `answer` and `cancel` to it. Every package that drives the fake peer without
//! a vendor's dialect speaks this one, so a proof of the host or of a backend
//! over it is not a proof of a wire written for that test alone.

use crucible_execution::{
    Authentication, Decision, Detail, Event, Failure, Outcome, Pending, Text, TurnRequest,
};

use crate::{Account, Act, Dialect, Ran, Reading, Request, Shape};

/// A line protocol: one word, a space, and the rest.
#[derive(Debug, Default)]
pub struct Plain;

impl Dialect for Plain {
    fn program(&self) -> &'static str {
        "plain"
    }

    fn request(&self, _shape: Shape<'_>) -> Request {
        Request {
            arguments: vec!["--print".to_owned()],
            environment: Vec::new(),
        }
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        Ok(vec![format!("turn {}", turn.prompt())])
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        let (word, rest) = frame.split_once(' ').unwrap_or((frame, ""));
        Ok(vec![match word {
            "say" => Act::Say(Event::Delta(Text::new(rest))),
            "file" => Act::Say(Event::FileChanged {
                path: Text::new(rest),
            }),
            "session" => Act::Session(rest.to_owned()),
            "ask" => {
                let (native, summary) = rest.split_once(' ').unwrap_or((rest, ""));
                Act::Ask {
                    native: native.to_owned(),
                    pending: Pending::Permission {
                        tool: Text::new("read"),
                        summary: Text::new(summary),
                        detail: Detail::Reads,
                    },
                }
            }
            "fail" => Act::Fail(Failure::new(rest)),
            "end" => Act::End(Outcome::Answered),
            "cancelled" => Act::End(Outcome::Cancelled),
            _ => return Err(Failure::new("not a frame of this protocol")),
        }])
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        Ok(match decision {
            Decision::Allow(_) => format!("allow {native}"),
            Decision::Deny => format!("deny {native}"),
            Decision::Answer(text) => format!("answer {native} {}", text.as_str()),
        })
    }

    fn cancel(&mut self) -> Option<String> {
        Some("cancel".to_owned())
    }

    fn account(&self, step: Account, _shape: Shape<'_>) -> Option<Request> {
        let argument = match step {
            Account::Version => "--version",
            Account::Status => "status",
            Account::SignIn => "login",
            Account::SignOut => "logout",
        };
        Some(Request {
            arguments: vec![argument.to_owned()],
            environment: Vec::new(),
        })
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        match (step, ran.said.first().map(String::as_str)) {
            (Account::Version, Some(version)) => Reading::Version(Text::new(version)),
            (_, Some("in")) => Reading::Signed(Authentication::SignedIn),
            (_, Some("out")) => Reading::Signed(Authentication::SignedOut),
            _ => Reading::Unreadable(Failure::new("it said nothing this protocol reads")),
        }
    }
}
