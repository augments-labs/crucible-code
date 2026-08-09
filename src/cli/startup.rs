//! Building the runner the loop drives.
//!
//! Everything the command line and the configuration files decided arrives here
//! as a [`Startup`], and leaves as a `Runner` holding a provider, six tools, a
//! model and a session. This is where a provider's *name* becomes a type, so
//! adding one is an arm in [`provider`] and nothing in any crate below.
//!
//! Nothing in here reads the environment or the disk on its own account: the
//! lookup is a parameter, which is what lets a startup be failed both ways it
//! can fail without a key or a home directory anywhere near the test.

use std::path::Path;

use crucible_config::Settings;
use crucible_core::{ApiKey, Cancel, Credential, Header, HeaderKey, Provider, Workspace};
use crucible_provider::{Anthropic, Https, OpenAi};
use crucible_runner::{Model, Runner, Session, Tools};
use crucible_tools::{Bash, Edit, Glob, Grep, Read, Write};

use super::Fatal;

/// The variables each key is read from. The *names* are what is configured
/// here; a value never appears in this repository or in a session file.
const ANTHROPIC_KEY: &str = "ANTHROPIC_API_KEY";
const OPENAI_KEY: &str = "OPENAI_API_KEY";

/// Ceiling on one response, in tokens.
const MAX_TOKENS: u32 = 8192;

/// The standing instructions every turn carries.
///
/// Written for this harness. It says how to work and how to answer, and leaves
/// what the tools do to the tools' own schemas — a system prompt that also
/// describes each tool is a second place for that description to go stale.
const SYSTEM: &str = "\
You are crucible, a coding agent working in a terminal beside a developer.

Work from what the code says rather than what it probably says: read a file \
before changing it, and search before concluding something is not there. \
Prefer the smallest change that finishes the job, and match the conventions of \
the file you are editing rather than your own habits.

Answer in plain prose, briefly. The developer is reading a terminal: put the \
conclusion first, skip the preamble, and do not read a file's contents back \
after editing it — say what changed and why.

Ask when the answer would change what you build. Otherwise decide, say which \
way you decided, and carry on.";

/// Everything the wiring needs to build a runner.
///
/// A struct rather than a parameter list because most of these are parameters
/// only so that a test can supply them: `sessions`, `settings` and `from` each
/// let a startup be pointed somewhere disposable and failed either way it can
/// fail, and eight of those in a row is a call nobody can read.
pub(super) struct Startup<'a> {
    /// Which provider, after the command line and the files have both spoken.
    pub(super) provider: &'a str,
    /// Which model of it, resolved the same way.
    pub(super) model: &'a str,
    /// Whether to carry on the most recent session for this directory.
    pub(super) resuming: bool,
    /// What the configuration files said.
    pub(super) settings: &'a Settings,
    /// Where session logs go.
    pub(super) sessions: &'a Path,
    /// The directory being worked in.
    pub(super) workspace: &'a Workspace,
    /// What stops a turn.
    pub(super) cancel: &'a Cancel,
    /// Reads the environment. A parameter because the real one cannot be
    /// written from a test: writing to it is `unsafe` in edition 2024, which
    /// this workspace forbids.
    pub(super) from: &'a dyn Fn(&str) -> Option<String>,
}

/// The runner the loop drives, built from what the startup resolved.
pub(super) fn assemble(startup: &Startup<'_>) -> Result<Runner, Fatal> {
    let Startup {
        settings,
        sessions,
        workspace,
        ..
    } = *startup;

    // Before the session, and the last thing on the way in that can fail: the
    // caller has already prepared the terminal for the same reason. Starting a
    // session writes a file, and one written for a run that never happened is
    // then the newest for this directory — which is what `--continue` would
    // offer instead of the last real session.
    let provider = provider(startup.provider, settings, startup.from)?;

    let (session, earlier) = if startup.resuming {
        let (session, transcript) = Session::resume(sessions, workspace)?;
        (session, Some(transcript))
    } else {
        (Session::start(sessions, workspace)?, None)
    };

    let mut runner = Runner::new(
        provider,
        tools(workspace, startup.cancel, settings),
        model(startup.model, workspace),
        session,
    )
    // The mode the files named, or the one that asks. `None` here is "no layer
    // said", which is a different thing from a layer that said `ask` — but the
    // answer is the same, and the distinction is the command line's to use.
    .permitting(settings.permission(settings.mode().unwrap_or_default()));
    if let Some(transcript) = earlier {
        runner = runner.resuming(transcript);
    }

    Ok(runner)
}

/// The provider that serves the chosen model.
///
/// The one place in the program where a provider's name becomes a type. Adding
/// another is an arm here and a `Credential` beside it — nothing in any crate
/// below has to learn that it exists.
///
/// `from` reads the environment. It is a parameter because the pairing below is
/// worth a test and the real environment cannot be set from one: writing to it
/// is `unsafe` in edition 2024, which this workspace forbids.
///
/// Which variable holds the key is configuration, so a file may name a
/// different one — somebody with a work key and a personal key has two
/// variables and one of them is not the vendor's usual name. The *value* stays
/// where it always was: read once, here, and applied to a header.
fn provider(
    named: &str,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
) -> Result<Box<dyn Provider>, Fatal> {
    let variable = settings.api_key_env(named);

    match named {
        // Two protocols, one credential kind pointed at different headers.
        // Authentication is a separate axis, and this is what that buys.
        "anthropic" => Ok(Box::new(Anthropic::new(
            key(
                variable.unwrap_or(ANTHROPIC_KEY),
                Header::bare("x-api-key"),
                from,
            )?,
            Box::new(Https::new()),
        ))),

        "openai" => Ok(Box::new(OpenAi::new(
            key(variable.unwrap_or(OPENAI_KEY), Header::bearer(), from)?,
            Box::new(Https::new()),
        ))),

        named => Err(Fatal::Provider {
            named: named.into(),
        }),
    }
}

/// A key from the environment, ready to sign a request with.
///
/// The variable's name is what is configured; the value is read once, here, and
/// goes no further than the header it is applied to.
fn key(
    variable: &str,
    header: Header,
    from: &dyn Fn(&str) -> Option<String>,
) -> Result<Box<dyn Credential>, Fatal> {
    let key = ApiKey::from_lookup(variable, from)?;

    Ok(Box::new(HeaderKey::new(key, header)))
}

/// Everything the model may call.
///
/// The order is the order they are advertised in, which is the order a model
/// tends to reach for them: read before write, search before either.
fn tools(workspace: &Workspace, cancel: &Cancel, settings: &Settings) -> Tools {
    let mut tools = Tools::new();

    tools.add(Box::new(Read::new(workspace.clone())));
    tools.add(Box::new(Grep::new(workspace.clone())));
    tools.add(Box::new(Glob::new(workspace.clone())));
    tools.add(Box::new(Edit::new(workspace.clone())));
    tools.add(Box::new(Write::new(workspace.clone())));

    // The `env` block goes to the commands crucible runs and nowhere else.
    // crucible cannot put a variable in its own environment — writing to one is
    // `unsafe` in edition 2024 — and would not want to: what the block is for
    // is what `cargo test` sees, not what this process sees.
    tools.add(Box::new(
        Bash::new(workspace.clone(), cancel.clone()).exporting(settings.env()),
    ));

    tools
}

/// Which model to ask, and what it is standing on.
///
/// The root goes in the system prompt because every tool takes paths relative
/// to it, and a model that has to guess the root spends its first tool call
/// finding out.
fn model(name: &str, workspace: &Workspace) -> Model {
    let system = format!(
        "{SYSTEM}\n\nThe workspace root is {}. Every tool path is relative to it.",
        workspace.root().display()
    );

    Model {
        name: name.into(),
        max_tokens: MAX_TOKENS,
        system: Some(system.into()),
    }
}

#[cfg(test)]
mod tests;
