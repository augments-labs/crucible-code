//! Building the runner the loop drives.
//!
//! Everything the command line and the configuration files decided arrives here
//! as a [`Startup`], and leaves as a `Runner` holding a provider, its tools, a
//! model and a session. This is where a provider's *name* becomes a type: each
//! record in the provider registry names a [`Factory`] here, so adding one is a
//! factory and a record, and nothing in any crate below.
//!
//! Nothing in here reads the environment or the disk on its own account: the
//! lookup is a parameter, which is what lets a startup be failed both ways it
//! can fail without a key or a home directory anywhere near the test.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use crucible_auth::StoredCredentials;
use crucible_config::Settings;
use crucible_core::{
    AgentId, ApiKey, Credential, DescribeTool, Effort, Fetch, Header, HeaderKey, Message,
    Modalities, Mode, ModelCapabilities, Provider, Revealed, Search, SessionId, Tool, ToolsetError,
    Transcript, Workspace,
};
use crucible_provider::{
    Anthropic, AnthropicWeb, Endpoint, Https, Moonshot, MoonshotWeb, OpenAi, OpenAiWeb, Unavailable,
};
use crucible_runner::{
    AgentSpec, Bounds, Compaction, ContextInputs, Model, RunPolicy, Runner, Session, Tools,
};
use crucible_tools::{
    AskUser, Background, Bash, Edit, Glob, Grep, Held, Ledger, LocalSandbox, Plan, Read, TodoWrite,
    ToolSearch, WebFetch, WebSearch, Write,
};

use super::hosting::{Hosting, selecting};
use super::seen::Putting;
use super::standing;
use super::subscription::Subscriptions;
use super::{Fatal, Providers, Served};

/// The most crucible will ask any model to produce in one answer, in tokens.
///
/// A ceiling over the model's own rather than a number to use instead of it:
/// what is asked for is whichever is smaller. Models now serve far more than
/// this, and taking all of it would cost more than it buys — most vendors
/// require the request and this ceiling to fit the window together, so every
/// token reserved for an answer is a token of session that cannot be used, and
/// the room kept free for the next exchange is worked out from this figure.
/// Sixteen thousand is a long answer or a large edit, and a fraction of even a
/// small window.
pub(super) const CEILING: u32 = 16_000;

/// And where this build knows nothing about the model at all.
///
/// Lower, deliberately. An unknown name is one no table has limits for, so
/// asking for a long answer risks a vendor refusing the request outright — and
/// a conservative ceiling costs a truncated answer at worst, where an optimistic
/// one costs the turn.
pub(super) const UNKNOWN_CEILING: u32 = 8192;

/// The name the tool that writes the plan is called by.
///
/// Here rather than beside the panel, because this is the file allowed to know
/// which tool is which: a resumed session is seeded by finding that tool's last
/// call in the transcript, and the loop that draws the panel never learns there
/// is a tool behind it at all.
const PLANNING: &str = "todo_write";

/// Which earlier session, if any, a run picks back up.
///
/// One value rather than two flags, because the command line already refuses
/// `--continue` and `--resume` together: by the time a startup is being built
/// the three answers are one decision, and an enum is what keeps a fourth
/// combination from ever being wired.
pub(super) enum Resuming {
    /// Start a new session.
    No,
    /// Carry on the newest session for this directory: `--continue`.
    Newest,
    /// Pick up the exact session this id names: `--resume`.
    Exact(SessionId),
}

/// Everything the wiring needs to build a runner.
///
/// A struct rather than a parameter list because most of these are parameters
/// only so that a test can supply them: `sessions`, `settings` and `from` each
/// let a startup be pointed somewhere disposable and failed either way it can
/// fail, and eight of those in a row is a call nobody can read.
pub(super) struct Startup<'a> {
    /// The generation of the provider registry every name here was read
    /// against, and the one a model's limits are read out of. Taken by the
    /// caller rather than here, so the provider that was resolved and the
    /// record its limits come from are the same generation.
    pub(super) providers: &'a Providers,
    /// Which provider, after the command line and the files have both spoken.
    /// `None` where this machine holds no usable credential for any of them.
    pub(super) provider: Option<Served>,
    /// The exact missing-choice sentence the provider that answers nothing
    /// refuses with, resolved once from the same credential set the opening
    /// drew it from.
    pub(super) unasked: &'static str,
    /// Which model of it, resolved the same way. `None` where nothing named
    /// one, which is a session that can do everything but take a turn.
    pub(super) model: Option<&'a str>,
    /// How hard to ask it to think, resolved the same way again. `None` sends
    /// no such field at all, which is the vendor's own default for the model.
    pub(super) effort: Option<Effort>,
    /// Which earlier session, if any, this run picks back up.
    pub(super) resuming: Resuming,
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
    /// Which files have been read. Made by the caller because the loop holds
    /// one too, and the commands that leave a session empty it.
    pub(super) ledger: &'a Ledger,
    /// Which deferred tools this session has looked up. Held by the caller for
    /// the same reason the ledger is: `/clear` empties it, and a session that
    /// has not looked anything up must not inherit the last one's answers.
    pub(super) revealed: &'a Revealed,
    /// The plan the agent is working to. Made by the caller for the same reason
    /// the ledger is: the loop draws it above the box, the tool writes into it,
    /// and `/clear` empties it.
    pub(super) plan: &'a Plan,
    /// Where a command left running is kept. Made by the caller for the reason
    /// the two above are, with one more: it is what ends every one of those
    /// processes when the run is over, so the value that ends them has to outlive
    /// every tool that started one.
    pub(super) leaving: &'a Background,
    /// Where a tool's questions reach the thread that draws them. Made by the
    /// caller for the reason the plan is: the loop holds the other end.
    pub(super) putting: &'a Putting,
    /// Which MCP servers written down under `mcp.servers` this run hosts, by
    /// name and in the order the command line gave them. Empty is the ordinary
    /// run: a configuration file lists servers somebody could start, and this
    /// is the moment one of them is chosen.
    pub(super) hosting: &'a [String],
    /// Whether there is anybody at a keyboard to be asked.
    ///
    /// A redirected run has nobody, and a tool that can only ever answer "there
    /// is no one here" is a schema spent saying so.
    pub(super) terminal: bool,
    /// Reads the environment. A parameter because the real one cannot be
    /// written from a test: writing to it is `unsafe` in edition 2024, which
    /// this workspace forbids.
    pub(super) from: &'a dyn Fn(&str) -> Option<String>,
    /// What `/login` wrote down. Read once by the caller, because the same
    /// answer is what decided which provider this run is for.
    pub(super) stored: &'a StoredCredentials,
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
        startup.unasked,
        ProviderAuth {
            settings,
            from: startup.from,
            stored: startup.stored,
            subscriptions: startup.subscriptions,
        },
    )?;

    // Beside the provider, and for its reason: naming a server nobody wrote
    // down, or one whose program is not on this machine, is a run that cannot
    // do what it was asked, and the session file must not exist yet when that
    // is found out. Nothing is started here — this resolves the selection and
    // reaches no process.
    let chosen = selecting::selected(startup.hosting, settings, workspace, |name| {
        (startup.from)(name).map(OsString::from)
    })?;

    // Resolved the same way the provider's was, and separately: a source is a
    // request crucible makes on the user's behalf and needs its own
    // authorisation. Going through the same resolver is what keeps the two
    // answers the same credential — a second, simpler lookup here would have
    // billed a plan session's searches to whatever key the shell carried.
    let reaching = web(startup, settings);

    let (session, earlier) = match &startup.resuming {
        Resuming::Newest => {
            let (session, transcript) = Session::resume(sessions, workspace)?;
            (session, Some(transcript))
        }
        Resuming::Exact(id) => {
            let (session, transcript) = reopening(sessions, workspace, id)?;
            (session, Some(transcript))
        }
        Resuming::No => {
            let branch = super::branching::current(workspace.root());
            (
                Session::start(sessions, workspace, branch.as_deref())?,
                None,
            )
        }
    };

    // Build the registry before the runner, because its exact immutable
    // generation is one of the typed facts the first pass assembles.
    let sandbox: Arc<dyn crucible_core::SandboxService> = Arc::new(LocalSandbox::new());
    let offering = tools(startup, settings, reaching, Arc::clone(&sandbox))?;

    // Operator-authored instructions are the stable request prefix. Everything
    // that can move within the session is read by context assembly instead.
    let name = startup.model.unwrap_or_default();
    let asked = standing::under(settings);

    // Read before the provider is handed over, because which vendor is being
    // written to is what says which model's limits are being asked about.
    let asking = coding(startup, provider.name(), name, &asked);

    // Two constructors rather than one always-hosting toolset, because a run
    // that named no server must reach the runner as the built-in generation
    // itself: an empty [`Hosting`] would be a live toolset whose first exact
    // generation is prepared at admission, which is a different thing to be
    // for every run that never asked for one.
    let context = ContextInputs::new(workspace.root());
    let permission = settings.permission(startup.mode);
    let mut runner = if chosen.is_empty() {
        Runner::new(provider, offering, asking, context, session)
    } else {
        Runner::with_toolset(
            provider,
            Hosting::new(offering, sandbox, chosen),
            asking,
            context,
            session,
        )
    }
    .permitting(permission)
    .under(policy(settings));
    if let Some(transcript) = earlier {
        planned(startup.plan, &transcript);
        runner = runner.resuming(transcript);
    }

    Ok(runner)
}

/// The session `--resume` named, and everything it already holds.
///
/// A name nothing here answers to gets [`Fatal::NoSession`]'s sentence rather
/// than the session crate's: the id came off the command line a moment ago,
/// and what the user needs to hear is that the address is wrong in this
/// workspace, not which file was looked for.
pub(super) fn reopening(
    sessions: &Path,
    workspace: &Workspace,
    id: &SessionId,
) -> Result<(Session, Transcript), Fatal> {
    use crucible_runner::SessionError;

    Session::reopen(sessions, workspace, id).map_err(|problem| match problem {
        SessionError::Unknown { id, .. } => Fatal::NoSession(id),
        other => Fatal::Session(other),
    })
}

/// Fills the plan from the last time the session wrote one.
///
/// A session log records what happened and nothing else, so there is no plan
/// stored anywhere to read back: what there is, is the call that wrote it. The
/// most recent one is the whole plan — the tool replaces the list every time —
/// so the search stops at the first it finds from the end.
///
/// Nothing is said where there is none, and nothing is said where the call
/// cannot be read: this is a picture of the work, drawn again from the record,
/// and a session that is picked up without one opens the way a new session does.
pub(super) fn planned(plan: &Plan, transcript: &Transcript) {
    let called = transcript.messages().iter().rev().find_map(|message| {
        let Message::Agent { calls, .. } = message else {
            return None;
        };

        calls.iter().rev().find(|call| &*call.name == PLANNING)
    });

    if let Some(call) = called {
        plan.replay(&call.args);
    }
}

/// Refuses a provider name this build has nothing for, and hands back the entry
/// for one it has.
///
/// The name and nothing else: no key is looked up, no agent is built and no
/// file is touched, which is what lets the caller run this before it draws
/// anything. A record cannot be registered without its factory, so a name this
/// finds is one [`provider`] can build.
///
/// The entry comes back because the model to fall back on is written beside the
/// name, and the caller has just proved which name it is.
pub(super) fn served(providers: &Providers, named: &str) -> Result<Served, Fatal> {
    providers
        .find(named)
        .map(|arm| arm.served)
        .ok_or_else(|| Fatal::Provider {
            named: named.into(),
            has: super::names(providers).into(),
        })
}

/// Credential sources provider construction resolves as one boundary.
#[derive(Clone, Copy)]
pub(crate) struct ProviderAuth<'a> {
    /// What the configuration files said.
    pub(crate) settings: &'a Settings,
    /// Reads the environment.
    pub(crate) from: &'a dyn Fn(&str) -> Option<String>,
    /// What `/login` wrote down.
    pub(crate) stored: &'a StoredCredentials,
    /// The subscription logins compiled into this binary.
    pub(crate) subscriptions: &'a Subscriptions,
}

/// Everything a factory is handed to build one provider or its web sources.
///
/// Resolved once by the caller from the settings and the record, so no factory
/// reads a setting or pairs a name with a variable on its own: the variable is
/// the configured one or the record's usual name, and the address is the one a
/// setting moved requests to, already parsed at that boundary.
pub(crate) struct Wiring<'a> {
    /// The provider's registered name, and the key `/login` wrote it under.
    pub(crate) named: &'static str,
    /// The environment variable the key is read from.
    pub(crate) variable: &'a str,
    /// Where a setting says requests should go, where one does.
    pub(crate) sending: Option<Endpoint>,
    /// The credential sources, as one boundary.
    pub(crate) auth: ProviderAuth<'a>,
}

/// Builds one provider from its wiring.
///
/// Fails the start where the credential is missing or a subscription cannot be
/// used at the configured address, with the sentence the vendor's arm owns.
pub(crate) type Factory = fn(Wiring<'_>) -> Result<Box<dyn Provider>, Fatal>;

/// Builds what answers the two web tools for one provider and model, or
/// nothing.
pub(crate) type Reach = fn(Wiring<'_>, &str) -> Reaching;

/// The provider that serves the chosen model.
///
/// Where a provider's name becomes a type: the registry hands back the record
/// the name was registered under, and the record's own factory builds it.
/// Adding another is a factory here, a `Credential` beside it and a record —
/// nothing in any crate below has to learn that it exists.
///
/// `None` is a machine with no usable credential for any provider, and it gets
/// the provider that answers nothing. Ending the run instead would take away
/// the session the credential is about to be set up from, and the sentence it
/// refuses with is the one already drawn under the welcome — `unasked` is that
/// sentence, resolved by the caller from the same credential set.
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
    unasked: &'static str,
    auth: ProviderAuth<'_>,
) -> Result<Box<dyn Provider>, Fatal> {
    let Some(serving) = serving else {
        return Ok(Box::new(Unavailable::new(unasked)));
    };

    (serving.build)(wiring(serving, auth)?)
}

/// The wiring one record's factories are handed, resolved from the settings.
fn wiring(serving: Served, auth: ProviderAuth<'_>) -> Result<Wiring<'_>, Fatal> {
    let named = serving.name;
    Ok(Wiring {
        named,
        variable: auth.settings.api_key_env(named).unwrap_or(serving.key),
        sending: sending_to(auth.settings, named)?,
        auth,
    })
}

/// Anthropic's Messages API, signed with a key in its own header.
///
/// Two protocols, one credential kind pointed at different headers.
/// Authentication is a separate axis, and this is what that buys.
pub(crate) fn anthropic(wiring: Wiring<'_>) -> Result<Box<dyn Provider>, Fatal> {
    Ok(Box::new(Anthropic::at(
        wiring.sending.unwrap_or(Anthropic::VENDOR),
        key(
            wiring.variable,
            Header::bare("x-api-key"),
            wiring.auth.from,
            wiring.auth.stored.get(wiring.named),
        )?,
        Box::new(Https::new()),
    )))
}

/// `MoonshotAI`, at the coding console unless a setting moved it.
///
/// The one provider with two vendor addresses. A key is issued against one
/// console and refused by the other, and nothing in the key says which, so the
/// choice cannot be made by reading it. This build asks the coding console,
/// which is the plan sold for what crucible does; a key from the open platform
/// sets `providers.moonshot.baseUrl` to the other address, and that is what the
/// help text and the docs say.
pub(crate) fn moonshot(wiring: Wiring<'_>) -> Result<Box<dyn Provider>, Fatal> {
    let (endpoint, credential) = credential(
        ApiAudience {
            provider: wiring.named,
            variable: wiring.variable,
            vendor: Moonshot::CODING,
        },
        wiring.sending,
        wiring.auth,
    )?;
    Ok(Box::new(Moonshot::at(
        endpoint,
        credential,
        Box::new(Https::new()),
    )))
}

/// OpenAI's Responses API, with a key or a plan login.
pub(crate) fn openai(wiring: Wiring<'_>) -> Result<Box<dyn Provider>, Fatal> {
    let (endpoint, credential) = credential(
        ApiAudience {
            provider: wiring.named,
            variable: wiring.variable,
            vendor: OpenAi::VENDOR,
        },
        wiring.sending,
        wiring.auth,
    )?;
    Ok(Box::new(OpenAi::at(
        endpoint,
        credential,
        Box::new(Https::new()),
    )))
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
/// A stored subscription answers first, and only at the vendor's own address:
/// an account authorized through `/login` is a deliberate choice, made after
/// any variable the shell happened to inherit, and `baseUrl` is set by somebody
/// with a reason not to reach the vendor — a plan's token is the vendor's, so
/// one configured to go elsewhere is no credential at all. Below it the order
/// is the one [`key`] keeps for every provider: the variable first, then the
/// key `/login` wrote down.
fn credential(
    audience: ApiAudience<'_>,
    sending: Option<Endpoint>,
    auth: ProviderAuth<'_>,
) -> Result<(Endpoint, Box<dyn Credential>), Fatal> {
    if sending.is_none()
        && let Some(subscribed) = auth
            .subscriptions
            .credential(audience.provider, auth.stored)
    {
        return Ok((subscribed.endpoint, subscribed.credential));
    }
    match ApiKey::from_lookup(audience.variable, auth.from) {
        Ok(exported) => Ok((
            sending.unwrap_or(audience.vendor),
            Box::new(HeaderKey::new(exported, Header::bearer())),
        )),
        Err(absent) => {
            if let Some(written) = auth.stored.get(audience.provider) {
                return Ok((
                    sending.unwrap_or(audience.vendor),
                    Box::new(HeaderKey::new(written, Header::bearer())),
                ));
            }
            if auth
                .subscriptions
                .credential(audience.provider, auth.stored)
                .is_some()
            {
                // Reachable only with `sending` set: without an address
                // configured, the first arm above has already answered.
                return Err(Fatal::SubscriptionAddress {
                    provider: audience.provider.into(),
                });
            }
            Err(absent.into())
        }
    }
}

/// What answers the two web tools, where this session has anything to.
///
/// Two halves because the vendors do not serve one capability: Anthropic serves
/// search and fetch, OpenAI serves search alone — reading a page is an action
/// inside its search tool rather than a tool of its own. A session gets exactly
/// the tools something can answer.
///
/// `None` on either side is a tool that is not advertised at all, which is the
/// honest answer where nothing can serve it: a tool that is registered and
/// fails every call teaches the model to keep trying it.
///
/// Nothing here fails the start. A source that cannot be built is a session
/// without web tools, not a session that refuses to open — the user asked for a
/// coding agent, and losing search is not losing that.
pub(crate) struct Reaching {
    searching: Option<Arc<dyn Search>>,
    fetching: Option<Arc<dyn Fetch>>,
}

impl Reaching {
    /// No web tools at all: the answer for a provider that serves neither, and
    /// for a credential that could not be resolved.
    fn nothing() -> Self {
        Self {
            searching: None,
            fetching: None,
        }
    }

    /// One source answering both tools.
    fn both<S: Search + Fetch + 'static>(source: Arc<S>) -> Self {
        Self {
            searching: Some(source.clone()),
            fetching: Some(source),
        }
    }
}

fn web(startup: &Startup<'_>, settings: &Settings) -> Reaching {
    let (Some(serving), Some(model)) = (startup.provider, startup.model) else {
        // A side request has to name a model, and the one it names is the
        // session's. Nothing is chosen yet in the state `/model` leaves open.
        return Reaching::nothing();
    };

    let auth = ProviderAuth {
        settings,
        from: startup.from,
        stored: startup.stored,
        subscriptions: startup.subscriptions,
    };
    let Ok(wiring) = wiring(serving, auth) else {
        return Reaching::nothing();
    };

    (serving.reach)(wiring, model)
}

/// Anthropic's own search and fetch, on the session's model.
pub(crate) fn anthropic_web(wiring: Wiring<'_>, model: &str) -> Reaching {
    let Ok(credential) = key(
        wiring.variable,
        Header::bare("x-api-key"),
        wiring.auth.from,
        wiring.auth.stored.get(wiring.named),
    ) else {
        return Reaching::nothing();
    };

    Reaching::both(Arc::new(AnthropicWeb::new(
        wiring.sending.unwrap_or(Anthropic::VENDOR),
        credential,
        Box::new(Https::new()),
        model,
    )))
}

/// OpenAI's search and fetch, whichever service the credential is for.
///
/// Plan or published API. An earlier version excluded the plan's backend on
/// the grounds that it refuses an unimplemented field with a 400 that ends the
/// turn — true of the *provider's* request, and not of this one. A source makes
/// its own request, so a refusal there is a failed tool result and the turn
/// carries on. Withholding the tool bought nothing and cost every plan session
/// its search.
///
/// Resolved through `credential`, which is the same resolution the provider
/// used, rather than by reaching for the variable directly. Those two answer
/// differently: a session logged in with `/login` runs its turns on the plan,
/// and a key resolver would have found an `OPENAI_API_KEY` the shell happened
/// to carry and billed every search to it — a credential the user did not
/// choose for this session, at $10 per thousand, silently. Which credential
/// answers is exactly what decides whether there is a source at all.
pub(crate) fn openai_web(wiring: Wiring<'_>, model: &str) -> Reaching {
    let Ok((endpoint, credential)) = credential(
        ApiAudience {
            provider: wiring.named,
            variable: wiring.variable,
            vendor: OpenAi::VENDOR,
        },
        wiring.sending,
        wiring.auth,
    ) else {
        return Reaching::nothing();
    };

    Reaching::both(Arc::new(OpenAiWeb::new(
        endpoint,
        credential,
        Box::new(Https::new()),
        model,
    )))
}

/// Kimi Code's own two services, which are what this vendor's own client
/// reaches.
///
/// Not the `$web_search` builtin: that one answers with the model's prose
/// rather than with addresses, so it fits no seam a result travels through.
///
/// They belong to the coding platform, and a key issued against the open
/// platform is refused by them — so a session whose address was moved
/// elsewhere gets no web tools rather than two that always fail.
pub(crate) fn moonshot_web(wiring: Wiring<'_>, _model: &str) -> Reaching {
    let Ok((endpoint, credential)) = credential(
        ApiAudience {
            provider: wiring.named,
            variable: wiring.variable,
            vendor: Moonshot::CODING,
        },
        wiring.sending,
        wiring.auth,
    ) else {
        return Reaching::nothing();
    };

    if endpoint.as_str() != Moonshot::CODING.as_str() {
        return Reaching::nothing();
    }

    Reaching::both(Arc::new(MoonshotWeb::new(
        credential,
        Box::new(Https::new()),
    )))
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
/// tends to reach for them: read before write, search before either. The plan
/// comes after those, being the one that does nothing to the workspace — and
/// the two web tools last of all, because they are the only ones that are not
/// always there and the only ones that leave the machine.
fn tools(
    startup: &Startup<'_>,
    settings: &Settings,
    reaching: Reaching,
    sandbox: Arc<dyn crucible_core::SandboxService>,
) -> Result<Tools, super::Fatal> {
    // Read off the wiring rather than taken one by one. Five things a tool is
    // built with is five arguments beside the settings, which is a call nobody
    // can read — and every one of them is already a field of the value that
    // describes how this run was set up.
    let Startup {
        workspace,
        ledger: seen,
        plan,
        leaving,
        putting,
        terminal,
        ..
    } = *startup;
    // Registered and advertised are two different things. Everything the coding
    // loop needs at once is shown; the rest is registered and left out until the
    // model looks it up, because a schema it can see is one it pays for on every
    // request of every turn and most sessions never touch most tools.
    let mut tools = Tools::looking_up(startup.revealed.clone());
    let mut held: Vec<Held> = Vec::new();

    // Which files have been read is learned by one tool and asked by another,
    // and this is the only place that may know they share it. The record itself
    // comes from the caller: `/clear` and `/resume` empty it when they leave
    // the session those files were read in, and neither tool can reach the
    // other to be told.
    tools.add_builtin(Read::new(workspace.clone(), seen.clone()))?;
    tools.add_builtin(Grep::new(workspace.clone()))?;
    tools.add_builtin(Glob::new(workspace.clone()))?;
    tools.add_builtin(Edit::new(workspace.clone()))?;
    tools.add_builtin(Write::new(workspace.clone(), seen.clone()))?;

    // The `env` block goes to the commands crucible runs and nowhere else.
    // crucible cannot put a variable in its own environment — writing to one is
    // `unsafe` in edition 2024 — and would not want to: what the block is for
    // is what `cargo test` sees, not what this process sees.
    // And the other end of the row under the box. The clone shares one registry
    // rather than copying it, which is what lets the loop draw what is running and
    // stop one — and what makes the caller's copy the thing that ends them all.
    tools.add_builtin(
        Bash::new(workspace.clone())
            .under_policy(sandbox, settings.sandbox().enforcing_policy(workspace)?)
            .following_enablement(settings.sandbox().enablement())
            .exporting(settings.env())
            .leaving(leaving.clone()),
    )?;

    // The other end of the panel above the prompt. The clone shares one plan
    // rather than copying it, which is what makes a call on the worker thread
    // something the drawing thread sees on its next frame.
    // Deferred from here down. `todo_write` is the largest of them and the one
    // a short session never reaches for; the two web tools are the ones a
    // session without a question about the world never touches at all.
    defer(&mut tools, &mut held, TodoWrite::new(plan.clone()))?;

    // Last, and only where this session has a source. One `Arc` serves both:
    // the two tools ask it different questions, and a session whose vendor
    // answers only one of them registers only that one the day such a source
    // exists.
    if let Some(searching) = reaching.searching {
        defer(&mut tools, &mut held, WebSearch::new(searching))?;
    }
    if let Some(fetching) = reaching.fetching {
        defer(&mut tools, &mut held, WebFetch::new(fetching))?;
    }

    // Advertised rather than deferred, and that is the one place this tool
    // differs from every other one held back. A model that cannot see it will
    // not go looking for it at the moment it realises it should ask, and that
    // moment is the only thing it exists for — a tool nobody can find when they
    // need it is a tool that is not there.
    //
    // And only where somebody is at a keyboard. A redirected run has nobody to
    // ask, so the schema would be spent saying there is no one here — the same
    // argument the search below makes about a session that defers nothing.
    if terminal {
        tools.add_builtin(AskUser::new(Arc::new(putting.clone())))?;
    }

    // Last, and only where there is anything to find. A search that can only
    // ever answer "nothing" is a schema spent saying so.
    let looking = ToolSearch::new(held, startup.revealed.clone());
    if !looking.is_empty() {
        tools.add_builtin(looking)?;
    }

    Ok(tools)
}

/// Registers `tool` unadvertised, and records how a search would find it.
///
/// The two go together because they cannot disagree: a tool held back that no
/// search knows about is one the model can never reach, and an entry with no
/// tool behind it is a search that offers something that will not run.
fn defer<T>(tools: &mut Tools, held: &mut Vec<Held>, tool: T) -> Result<(), ToolsetError>
where
    T: DescribeTool + Tool + 'static,
{
    held.push(Held {
        name: tool.name().into(),
        about: about(tool.schema()),
    });
    tools.defer_builtin(tool)
}

/// The first sentence of what a schema says the tool does.
///
/// A sentence rather than the whole description, because this is what a search
/// prints for every match and the descriptions run to paragraphs. The whole of
/// it arrives with the schema a moment later, which is the point.
fn about(schema: &str) -> Box<str> {
    let said = serde_json::from_str::<serde_json::Value>(schema)
        .ok()
        .and_then(|schema| {
            schema
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();

    match said.split_once(". ") {
        Some((first, _)) => format!("{first}.").into(),
        None => said.into(),
    }
}

/// The one agent crucible runs, as a definition its loop can be handed.
///
/// Built here rather than read from a document because there is one of these
/// and it is this program: an agent nobody can choose between needs no way to
/// be named in a file. What is configurable about it — the model, the effort,
/// the window — is already configurable, and arrives here resolved.
///
/// What a turn is asked under is [`standing::under`], read again before every
/// turn because half of it is about the model in force. This is the read that
/// seeds the definition, not the one any turn goes out under: the first turn
/// writes its own before it asks.
///
/// An unnamed model is the empty name, which is what the loop reads to find out
/// that there is nothing to ask yet. It is the same absence [`Startup::model`]
/// carries, spelled the way a `Model` can hold it — the alternative is an
/// `Option` threaded through every turn to describe a state no turn is taken in.
///
/// The effort stays an `Option` for the opposite reason: there is no rung that
/// means "nobody said", and the field left off is what a vendor reads as its own
/// default.
fn coding(startup: &Startup<'_>, provider: &str, name: &str, asked: &str) -> AgentSpec {
    let mut spec = AgentSpec::new(
        AgentId::new("coding"),
        Model {
            name: name.into(),
            max_tokens: ceiling(startup.providers, provider, name),
            window: startup
                .provider
                .map(|serving| window(startup.providers, serving, name, startup.settings)),
            accepts: accepts(startup.providers, provider, name),
            effort: startup.effort,
        },
    );
    spec.named("Coding");
    spec.describing("Reads, changes and checks the code in this workspace.");
    spec.told(asked);
    spec
}

/// What the documents together say one run may spend, and what it does when
/// the window fills.
///
/// Resolved here, whole, so the loop is handed an answer rather than learning
/// that any of this has a spelling in a file. `keep` is the one figure with a
/// default of crucible's own: a session carried on from needs enough of the
/// recent turns to say what it is doing and how it got there, which is what
/// "carry on from here" means, and nothing about a document makes that number.
///
/// The two byte ceilings and the retry policy are not configurable and are not
/// read here: they bound this program's own memory and its own patience with a
/// provider, and a document that could raise them could raise them past what
/// the machine has.
fn policy(settings: &Settings) -> RunPolicy {
    let said = settings.compaction();
    let asked = RunPolicy::default();

    RunPolicy {
        bounds: Bounds {
            spend: said.spend_ceiling,
            ..asked.bounds
        },
        compaction: Compaction {
            automatic: said.when.automatic(),
            reserve: said.reserve,
            keep_tokens: said.keep.unwrap_or(asked.compaction.keep_tokens),
            recap_tokens: said.recap.unwrap_or(asked.compaction.recap_tokens),
            ask_on_resume: said.ask_on_resume,
        },
        retry: asked.retry,
        tools: asked.tools,
        prompt_cache: settings.prompt_cache(),
    }
}

/// The context-window size this session manages against.
///
/// A configured figure is explicit and wins, including one that opts into a
/// model's million-token window. Without one, known native limits are held under
/// the provider record's conservative default: long context is available, but
/// using it is a choice rather than the starting behavior. The record's cap
/// also covers names released after this build. A provider this build has no
/// record for never reaches here: the name was refused at the registry.
pub(super) fn window(
    providers: &Providers,
    serving: Served,
    model: &str,
    settings: &Settings,
) -> u32 {
    settings
        .context_window(serving.name, model)
        .unwrap_or_else(|| {
            let native = super::capabilities(providers, serving.name, model)
                .map_or(serving.window, ModelCapabilities::window);
            native.min(serving.window)
        })
}

/// What this model reads, where this build has heard of it.
///
/// The model's half alone. The other half is the provider's, and the loop asks
/// the provider itself rather than being handed an answer resolved here: what a
/// wire module can write today and what a vendor's table says are two facts
/// that drift apart, and only one of them is in this table.
pub(super) fn accepts(providers: &Providers, provider: &str, model: &str) -> Option<Modalities> {
    super::capabilities(providers, provider, model).map(ModelCapabilities::accepts)
}

/// How long an answer to ask this model for.
///
/// The model's own limit held under [`CEILING`], or [`UNKNOWN_CEILING`] where
/// this build has never heard of it. Read off the name exactly as it was asked
/// for — a name one word from a listed one is a name nothing is known about,
/// and borrowing the neighbour's figure is how a request comes to be refused
/// for a reason nobody can see.
pub(super) fn ceiling(providers: &Providers, provider: &str, model: &str) -> u32 {
    super::capabilities(providers, provider, model)
        .map_or(UNKNOWN_CEILING, |one| one.output().min(CEILING))
}

#[cfg(test)]
mod tests;
