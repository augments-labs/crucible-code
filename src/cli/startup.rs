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

use crucible_auth::Keys;
use crucible_config::Settings;
use crucible_core::{
    ApiKey, Cancel, Credential, Effort, Header, HeaderKey, Mode, Provider, Workspace,
};
use crucible_provider::{Anthropic, Endpoint, Https, Moonshot, OpenAi, Unavailable};
use crucible_runner::{Model, Runner, Session, Tools};
use crucible_tools::{Bash, Edit, Glob, Grep, Read, Write};

use super::standing;
use super::subscription::Subscriptions;
use super::{Fatal, NOTHING_TO_ASK, PROVIDERS, Served};

/// Ceiling on one response, in tokens.
const MAX_TOKENS: u32 = 8192;

/// Everything the wiring needs to build a runner.
///
/// A struct rather than a parameter list because most of these are parameters
/// only so that a test can supply them: `sessions`, `settings` and `from` each
/// let a startup be pointed somewhere disposable and failed either way it can
/// fail, and eight of those in a row is a call nobody can read.
pub(super) struct Startup<'a> {
    /// Which provider, after the command line and the files have both spoken.
    /// `None` where this machine holds no key for any of them.
    pub(super) provider: Option<Served>,
    /// Which model of it, resolved the same way. `None` where nothing named
    /// one, which is a session that can do everything but take a turn.
    pub(super) model: Option<&'a str>,
    /// How hard to ask it to think, resolved the same way again. `None` sends
    /// no such field at all, which is the vendor's own default for the model.
    pub(super) effort: Option<Effort>,
    /// Whether to carry on the most recent session for this directory.
    pub(super) resuming: bool,
    /// The mode the permission engine starts in. The caller resolves it once
    /// and gives the same value to the prompt line, which is what keeps the
    /// mode on screen the mode in force.
    pub(super) mode: Mode,
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
    /// What `/login` wrote down. Read once by the caller, because the same
    /// answer is what decided which provider this run is for.
    pub(super) stored: &'a Keys,
    /// The subscription logins compiled into this binary, which is what pairs
    /// a stored account credential with the one address its tokens are issued
    /// for.
    pub(super) subscriptions: &'a Subscriptions,
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
    let provider = provider(
        startup.provider,
        settings,
        startup.from,
        startup.stored,
        startup.subscriptions,
    )?;

    let (session, earlier) = if startup.resuming {
        let (session, transcript) = Session::resume(sessions, workspace)?;
        (session, Some(transcript))
    } else {
        (Session::start(sessions, workspace)?, None)
    };

    let mut runner = Runner::new(
        provider,
        tools(workspace, startup.cancel, settings),
        model(startup.model, startup.effort, workspace),
        session,
    )
    .permitting(settings.permission(startup.mode));
    if let Some(transcript) = earlier {
        runner = runner.resuming(transcript);
    }

    Ok(runner)
}

/// Refuses a provider name this build has nothing for, and hands back the entry
/// for one it has.
///
/// The name and nothing else: no key is looked up, no agent is built and no
/// file is touched, which is what lets the caller run this before it draws
/// anything. [`provider`] settles the same question again on its way past, so a
/// name added to the list without an arm behind it still fails — later, and
/// with the same sentence.
///
/// The entry comes back because the model to fall back on is written beside the
/// name, and the caller has just proved which name it is.
pub(super) fn served(named: &str) -> Result<Served, Fatal> {
    PROVIDERS
        .into_iter()
        .find(|one| one.name == named)
        .ok_or_else(|| Fatal::Provider {
            named: named.into(),
        })
}

/// The provider that serves the chosen model.
///
/// The one place in the program where a provider's name becomes a type. Adding
/// another is an arm here and a `Credential` beside it — nothing in any crate
/// below has to learn that it exists.
///
/// `None` is a machine with no key for any provider, and it gets the provider
/// that answers nothing. Ending the run instead would take away the session the
/// key is about to be set up from, and the sentence it refuses with is the one
/// already drawn under the welcome.
///
/// `from` reads the environment. It is a parameter because the pairing below is
/// worth a test and the real environment cannot be set from one: writing to it
/// is `unsafe` in edition 2024, which this workspace forbids.
///
/// Which variable holds the key is configuration, so a file may name a
/// different one — somebody with a work key and a personal key has two
/// variables and one of them is not the vendor's usual name. Failing that it is
/// the vendor's usual name, written beside the provider in `PROVIDERS`. The
/// *value* stays where it always was: read once, here, and applied to a header.
///
/// `stored` is the other place a credential can be, and the one `/login`
/// writes to: an API key, or the renewable state of an account login. Reachable
/// from the wiring root for exactly that: a credential written at the prompt is
/// a provider this run can be handed, and building it anywhere else would put
/// the names below in a second file.
pub(super) fn provider(
    serving: Option<Served>,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    stored: &Keys,
    subscriptions: &Subscriptions,
) -> Result<Box<dyn Provider>, Fatal> {
    let Some(serving) = serving else {
        return Ok(Box::new(Unavailable::new(NOTHING_TO_ASK)));
    };

    let named = serving.name;
    let variable = settings.api_key_env(named).unwrap_or(serving.key);
    let written = stored.get(named);
    let sending = sending_to(settings, named)?;

    match named {
        // Two protocols, one credential kind pointed at different headers.
        // Authentication is a separate axis, and this is what that buys.
        "anthropic" => Ok(Box::new(Anthropic::at(
            sending.unwrap_or(Anthropic::VENDOR),
            key(variable, Header::bare("x-api-key"), from, written)?,
            Box::new(Https::new()),
        ))),

        // The one provider with two vendor addresses. A key is issued against
        // one console and refused by the other, and nothing in the key says
        // which, so the choice cannot be made by reading it. This build asks
        // the coding console, which is the plan sold for what crucible does;
        // a key from the open platform sets `providers.moonshot.baseUrl` to
        // the other address, and that is what the help text and the docs say.
        "moonshot" => {
            let (endpoint, credential) = credential(
                ApiAudience {
                    provider: named,
                    variable,
                    vendor: Moonshot::CODING,
                },
                sending,
                from,
                stored,
                subscriptions,
            )?;
            Ok(Box::new(Moonshot::at(
                endpoint,
                credential,
                Box::new(Https::new()),
            )))
        }

        "openai" => {
            let (endpoint, credential) = credential(
                ApiAudience {
                    provider: named,
                    variable,
                    vendor: OpenAi::VENDOR,
                },
                sending,
                from,
                stored,
                subscriptions,
            )?;
            Ok(Box::new(OpenAi::at(
                endpoint,
                credential,
                Box::new(Https::new()),
            )))
        }

        named => Err(Fatal::Provider {
            named: named.into(),
        }),
    }
}

/// The vendor audience a credential is issued against.
///
/// The provider's name, the variable its key is read from and the address its
/// vendor signs at travel together because no call site may pair them by hand.
struct ApiAudience<'a> {
    provider: &'static str,
    variable: &'a str,
    vendor: Endpoint,
}

/// A credential for one provider, and the address it is issued against.
///
/// The two come back as one pair because they are one fact: a plan's token is
/// issued against the vendor's fixed audience, and handing the halves back
/// separately would let a later endpoint choice send it somewhere it was never
/// meant to go.
///
/// The order is the one [`key`] keeps for every provider: the variable first,
/// then the key `/login` wrote down. A stored subscription answers last, and
/// only at the vendor's own address — `baseUrl` is set by somebody with a
/// reason not to reach the vendor, and a plan's token is the vendor's, so one
/// configured to go elsewhere is no credential at all.
fn credential(
    audience: ApiAudience<'_>,
    sending: Option<Endpoint>,
    from: &dyn Fn(&str) -> Option<String>,
    stored: &Keys,
    subscriptions: &Subscriptions,
) -> Result<(Endpoint, Box<dyn Credential>), Fatal> {
    match ApiKey::from_lookup(audience.variable, from) {
        Ok(exported) => Ok((
            sending.unwrap_or(audience.vendor),
            Box::new(HeaderKey::new(exported, Header::bearer())),
        )),
        Err(absent) => {
            if let Some(written) = stored.get(audience.provider) {
                return Ok((
                    sending.unwrap_or(audience.vendor),
                    Box::new(HeaderKey::new(written, Header::bearer())),
                ));
            }
            if sending.is_none()
                && let Some(subscribed) = subscriptions.credential(audience.provider, stored)
            {
                return Ok((subscribed.endpoint, subscribed.credential));
            }
            Err(absent.into())
        }
    }
}

/// Where a setting says this provider's requests should go, where one does.
///
/// The address is parsed here rather than carried as the string it was written
/// as: this is the boundary, and what it decides is who receives the key. A
/// value that cannot be one ends the run — a provider quietly left pointing at
/// the vendor would be a setting that looks applied and does nothing, and this
/// particular one is set by somebody who has a reason to not reach the vendor.
fn sending_to(settings: &Settings, named: &str) -> Result<Option<Endpoint>, Fatal> {
    settings
        .base_url(named)
        .map(|written| {
            Endpoint::parse(written).map_err(|source| Fatal::Address {
                provider: named.into(),
                source,
            })
        })
        .transpose()
}

/// A key, ready to sign a request with.
///
/// The variable's name is what is configured; the value is read once, here, and
/// goes no further than the header it is applied to.
///
/// The variable first and what `/login` wrote down second. A key exported into
/// this run is the one whoever started it chose for this run — it is how a
/// second account, a work key, or a key that has just been rotated is used
/// without touching what is on the disk, and it lasts exactly as long as the
/// shell it was exported in. `written` is the standing answer underneath it.
fn key(
    variable: &str,
    header: Header,
    from: &dyn Fn(&str) -> Option<String>,
    written: Option<ApiKey>,
) -> Result<Box<dyn Credential>, Fatal> {
    let key = match ApiKey::from_lookup(variable, from) {
        Ok(exported) => exported,

        // Unset, or set to blank — which is how a shell turns a provider off.
        // Off for the variable rather than for crucible: somebody who ran
        // `/login` said so once and for every run after it, and a blank export
        // is what the machine has to say about the variable it is blanking.
        Err(absent) => written.ok_or(absent)?,
    };

    Ok(Box::new(HeaderKey::new(key, header)))
}

/// Everything the model may call.
///
/// The order is the order they are advertised in, which is the order a model
/// tends to reach for them: read before write, search before either.
fn tools(workspace: &Workspace, cancel: &Cancel, settings: &Settings) -> Tools {
    let mut tools = Tools::new();

    tools.add(Box::new(Read::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Grep::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Glob::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Edit::new(workspace.clone(), cancel.clone())));
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

/// Which model to ask, and what it is asked under.
///
/// What a turn is asked under is [`standing::under`], read again before every
/// turn because half of it is about the model in force — this is only the
/// first of those reads, for the turn a session might take before anything
/// changes.
///
/// An unnamed model is the empty name, which is what the loop reads to find out
/// that there is nothing to ask yet. It is the same absence [`Startup::model`]
/// carries, spelled the way a `Model` can hold it — the alternative is an
/// `Option` threaded through every turn to describe a state no turn is taken in.
///
/// The effort stays an `Option` for the opposite reason: there is no rung that
/// means "nobody said", and the field left off is what a vendor reads as its own
/// default.
fn model(name: Option<&str>, effort: Option<Effort>, workspace: &Workspace) -> Model {
    let name = name.unwrap_or_default();

    Model {
        system: Some(standing::under(name, effort, workspace).into()),
        name: name.into(),
        max_tokens: MAX_TOKENS,
        effort,
    }
}

#[cfg(test)]
mod tests;
