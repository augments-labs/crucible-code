//! What a client is asked and told, step by step, as it drives an installed
//! agent from a conversation on the provider's model: a connect, a prompt the
//! agent asks one permission for, an interrupt, a disconnect, and a connect
//! to an agent whose program is not installed.
//!
//! [`ACCOUNTS`] is the one statement of it: a suite that drives an agent
//! takes each step as a row says, answers what it is asked as the row says,
//! and asserts what it is told against the row, so what a client is asked and
//! told is written down once.

use crucible_client_api::{LoginOutcome, LogoutOutcome, Name, Outcome, Stop, Text, TurnOutcome};

use crate::client::accounts::{CONNECT, DISCONNECT, DISCONNECTED, USE_ACCOUNT, moved_onto};

/// The instance the chain connects, disconnects, and runs on between.
pub const INSTANCE: &str = "work";

/// The provider the chain connects through [`INSTANCE`].
pub const PROVIDER: &str = "anthropic";

/// The agent the last connect asks for, whose program is not installed.
pub const NOT_INSTALLED: &str = "Antigravity";

/// One step a client takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Connects [`PROVIDER`] through [`INSTANCE`] from the provider's model.
    Connect,
    /// Sends a prompt, which the agent asks one permission for; the client
    /// allows it.
    Prompt,
    /// Sends a prompt, and cancels the turn once it has begun.
    Interrupt,
    /// Disconnects [`INSTANCE`].
    Disconnect,
    /// Connects [`PROVIDER`] naming no instance, and picks [`NOT_INSTALLED`].
    ConnectMissing,
}

/// What the client is asked to decide in a step, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// Leave for an action the agent wants to take, which the client gives
    /// once.
    Permission,
    /// An account question offering the choice named, which the client takes.
    Choose(&'static str),
    /// That the agent's program was not found, with the facts that say why
    /// and what to install, answered with the choice named.
    Missing {
        /// The question.
        asks: &'static str,
        /// How the first fact, saying why the program cannot start, begins.
        why: &'static str,
        /// The fact naming what to install.
        install: &'static str,
        /// The choice the client takes.
        answer: &'static str,
    },
}

/// A step, what the client is asked in it, and what it is told.
#[derive(Debug, Clone, Copy)]
pub struct Expected {
    /// The step taken.
    pub step: Step,
    /// What the client is asked, in order.
    pub put: &'static [Put],
    /// What the client is told, in order.
    pub told: fn() -> Vec<Outcome>,
}

/// The chain, in the order a client takes it.
pub static ACCOUNTS: [Expected; 5] = [
    Expected {
        step: Step::Connect,
        put: &[Put::Choose(USE_ACCOUNT), Put::Choose(CONNECT)],
        told: connected,
    },
    Expected {
        step: Step::Prompt,
        put: &[Put::Permission],
        told: answered,
    },
    Expected {
        step: Step::Interrupt,
        put: &[],
        told: interrupted,
    },
    Expected {
        step: Step::Disconnect,
        put: &[Put::Choose(DISCONNECT)],
        told: disconnected,
    },
    Expected {
        step: Step::ConnectMissing,
        put: &[
            Put::Choose(NOT_INSTALLED),
            Put::Missing {
                asks: "Antigravity cannot be started.",
                why: "Not installed",
                install: "Install Antigravity, then choose Check again.",
                answer: "Go back",
            },
            Put::Choose("Cancel"),
        ],
        told: unconnected,
    },
];

/// The conversation moved onto [`INSTANCE`].
fn connected() -> Vec<Outcome> {
    vec![Outcome::Login(LoginOutcome::AgentConnected {
        said: Text::cut(&moved_onto(INSTANCE)),
        onto: Name::new(INSTANCE).ok(),
    })]
}

/// The agent's turn ran to its end.
fn answered() -> Vec<Outcome> {
    vec![Outcome::Turn(TurnOutcome::Ran {
        stop: Stop::Yielded,
    })]
}

/// The cancel was heard, and the turn stopped with nothing under way.
fn interrupted() -> Vec<Outcome> {
    vec![
        Outcome::Cancelling,
        Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Cancelled,
        }),
    ]
}

/// The instance takes no new work, and its run was stopped.
fn disconnected() -> Vec<Outcome> {
    vec![Outcome::Logout(LogoutOutcome::Disconnected {
        said: Text::cut(DISCONNECTED),
    })]
}

/// Nothing was connected: the client went back from the agent not found
/// and cancelled.
fn unconnected() -> Vec<Outcome> {
    vec![Outcome::Login(LoginOutcome::Undecided)]
}
