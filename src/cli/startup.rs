//! Building the runner the loop drives.
//!
//! Everything the command line and the configuration files decided arrives here
//! as a [`Startup`], and leaves as a `Runner` holding a provider, its tools, a
//! model and a session. This is where a provider's *name* becomes a type, so
//! adding one is an arm in [`provider`] and nothing in any crate below.
//!
//! Nothing in here reads the environment or the disk on its own account: the
//! lookup is a parameter, which is what lets a startup be failed both ways it
//! can fail without a key or a home directory anywhere near the test.

use std::path::Path;
use std::sync::Arc;

use crucible_auth::StoredCredentials;
use crucible_config::Settings;
use crucible_core::{
    ApiKey, Cancel, Credential, Effort, Fetch, Header, HeaderKey, Message, Mode, Provider,
    Revealed, Search, Tool, Transcript, Workspace,
};
use crucible_provider::{
    Anthropic, AnthropicWeb, Endpoint, Https, Moonshot, MoonshotWeb, OpenAi, OpenAiWeb, Unavailable,
};
use crucible_runner::{Compaction, Model, Runner, Session, Tools};
use crucible_tools::{
    AskUser, Background, Bash, Edit, Glob, Grep, Held, Ledger, Plan, Read, TodoWrite, ToolSearch,
    WebFetch, WebSearch, Write,
};

use super::seen::Putting;
use super::standing;
use super::subscription::Subscriptions;
use super::{Fatal, PROVIDERS, Served};

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
const CEILING: u32 = 16_000;

/// And where this build knows nothing about the model at all.
///
/// Lower, deliberately. An unknown name is one no table has limits for, so
/// asking for a long answer risks a vendor refusing the request outright — and
/// a conservative ceiling costs a truncated answer at worst, where an optimistic
/// one costs the turn.
const UNKNOWN_CEILING: u32 = 8192;

/// The name the tool that writes the plan is called by.
///
/// Here rather than beside the panel, because this is the file allowed to know
/// which tool is which: a resumed session is seeded by finding that tool's last
/// call in the transcript, and the loop that draws the panel never learns there
/// is a tool behind it at all.
const PLANNING: &str = "todo_write";

/// Everything the wiring needs to build a runner.
///
/// A struct rather than a parameter list because most of these are parameters
/// only so that a test can supply them: `sessions`, `settings` and `from` each
/// let a startup be pointed somewhere disposable and failed either way it can
/// fail, and eight of those in a row is a call nobody can read.
pub(super) struct Startup<'a> {
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
    /// Which files have been read. Made by the caller for the same reason the
    /// cancel is: the loop holds one too, and the commands that leave a
    /// session empty it.
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

    // Resolved the same way the provider's was, and separately: a source is a
    // request crucible makes on the user's behalf and needs its own
    // authorisation. Going through the same resolver is what keeps the two
    // answers the same credential — a second, simpler lookup here would have
    // billed a plan session's searches to whatever key the shell carried.
    let reaching = web(startup, settings);

    let (session, earlier) = if startup.resuming {
        let (session, transcript) = Session::resume(sessions, workspace)?;
        (session, Some(transcript))
    } else {
        (Session::start(sessions, workspace)?, None)
    };

    // Read before the provider is handed over, because which vendor is being
    // written to is what says which model's limits are being asked about.
    let asking = model(
        provider.name(),
        startup.model,
        startup.effort,
        workspace,
        settings,
    );

    let mut runner = Runner::new(
        provider,
        tools(startup, settings, reaching),
        asking,
        session,
    )
    .permitting(settings.permission(startup.mode))
    .compacting(compacting(settings));
    if let Some(transcript) = earlier {
        planned(startup.plan, &transcript);
        runner = runner.resuming(transcript);
    }

    Ok(runner)
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
fn planned(plan: &Plan, transcript: &Transcript) {
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

/// Credential sources provider construction resolves as one boundary.
#[derive(Clone, Copy)]
pub(super) struct ProviderAuth<'a> {
    /// What the configuration files said.
    pub(super) settings: &'a Settings,
    /// Reads the environment.
    pub(super) from: &'a dyn Fn(&str) -> Option<String>,
    /// What `/login` wrote down.
    pub(super) stored: &'a StoredCredentials,
    /// The subscription logins compiled into this binary.
    pub(super) subscriptions: &'a Subscriptions,
}

/// The provider that serves the chosen model.
///
/// The one place in the program where a provider's name becomes a type. Adding
/// another is an arm here and a `Credential` beside it — nothing in any crate
/// below has to learn that it exists.
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

    let named = serving.name;
    let variable = auth.settings.api_key_env(named).unwrap_or(serving.key);
    let written = auth.stored.get(named);
    let sending = sending_to(auth.settings, named)?;

    match named {
        // Two protocols, one credential kind pointed at different headers.
        // Authentication is a separate axis, and this is what that buys.
        "anthropic" => Ok(Box::new(Anthropic::at(
            sending.unwrap_or(Anthropic::VENDOR),
            key(variable, Header::bare("x-api-key"), auth.from, written)?,
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
                auth,
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
                auth,
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
struct Reaching {
    searching: Option<Arc<dyn Search>>,
    fetching: Option<Arc<dyn Fetch>>,
}

fn web(startup: &Startup<'_>, settings: &Settings) -> Reaching {
    let nothing = Reaching {
        searching: None,
        fetching: None,
    };

    let (Some(serving), Some(model)) = (startup.provider, startup.model) else {
        // A side request has to name a model, and the one it names is the
        // session's. Nothing is chosen yet in the state `/model` leaves open.
        return nothing;
    };

    let variable = settings.api_key_env(serving.name).unwrap_or(serving.key);
    let Ok(configured) = sending_to(settings, serving.name) else {
        return nothing;
    };

    match serving.name {
        "anthropic" => {
            let Ok(credential) = key(
                variable,
                Header::bare("x-api-key"),
                startup.from,
                startup.stored.get(serving.name),
            ) else {
                return nothing;
            };

            let source = Arc::new(AnthropicWeb::new(
                configured.unwrap_or(Anthropic::VENDOR),
                credential,
                Box::new(Https::new()),
                model,
            ));

            Reaching {
                searching: Some(source.clone()),
                fetching: Some(source),
            }
        }

        // Whichever service the credential is for, plan or published API. An
        // earlier version excluded the plan's backend on the grounds that it
        // refuses an unimplemented field with a 400 that ends the turn — true
        // of the *provider's* request, and not of this one. A source makes its
        // own request, so a refusal there is a failed tool result and the turn
        // carries on. Withholding the tool bought nothing and cost every plan
        // session its search.
        //
        // Resolved through `credential`, which is the same resolution the
        // provider used, rather than by reaching for the variable directly.
        // Those two answer differently: a session logged in with `/login` runs
        // its turns on the plan, and a key resolver would have found an
        // `OPENAI_API_KEY` the shell happened to carry and billed every search
        // to it — a credential the user did not choose for this session, at
        // $10 per thousand, silently. Which credential answers is exactly what
        // decides whether there is a source at all.
        "openai" => {
            let Ok((endpoint, credential)) = credential(
                ApiAudience {
                    provider: serving.name,
                    variable,
                    vendor: OpenAi::VENDOR,
                },
                configured,
                ProviderAuth {
                    settings,
                    from: startup.from,
                    stored: startup.stored,
                    subscriptions: startup.subscriptions,
                },
            ) else {
                return nothing;
            };

            let source = Arc::new(OpenAiWeb::new(
                endpoint,
                credential,
                Box::new(Https::new()),
                model,
            ));

            Reaching {
                searching: Some(source.clone()),
                fetching: Some(source),
            }
        }

        // Kimi Code's own two services, which are what this vendor's own client
        // reaches. Not the `$web_search` builtin: that one answers with the
        // model's prose rather than with addresses, so it fits no seam a result
        // travels through.
        //
        // They belong to the coding platform, and a key issued against the open
        // platform is refused by them — so a session whose address was moved
        // elsewhere gets no web tools rather than two that always fail.
        "moonshot" => {
            let Ok((endpoint, credential)) = credential(
                ApiAudience {
                    provider: serving.name,
                    variable,
                    vendor: Moonshot::CODING,
                },
                configured,
                ProviderAuth {
                    settings,
                    from: startup.from,
                    stored: startup.stored,
                    subscriptions: startup.subscriptions,
                },
            ) else {
                return nothing;
            };

            if endpoint.as_str() != Moonshot::CODING.as_str() {
                return nothing;
            }

            let source = Arc::new(MoonshotWeb::new(credential, Box::new(Https::new())));

            Reaching {
                searching: Some(source.clone()),
                fetching: Some(source),
            }
        }

        _ => nothing,
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
/// tends to reach for them: read before write, search before either. The plan
/// comes after those, being the one that does nothing to the workspace — and
/// the two web tools last of all, because they are the only ones that are not
/// always there and the only ones that leave the machine.
fn tools(startup: &Startup<'_>, settings: &Settings, reaching: Reaching) -> Tools {
    // Read off the wiring rather than taken one by one. Five things a tool is
    // built with is five arguments beside the settings, which is a call nobody
    // can read — and every one of them is already a field of the value that
    // describes how this run was set up.
    let Startup {
        workspace,
        cancel,
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
    // comes from the caller, the same as the cancel: `/clear` and `/resume`
    // empty it when they leave the session those files were read in, and
    // neither tool can reach the other to be told.
    tools.add(Box::new(Read::new(
        workspace.clone(),
        cancel.clone(),
        seen.clone(),
    )));
    tools.add(Box::new(Grep::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Glob::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Edit::new(workspace.clone(), cancel.clone())));
    tools.add(Box::new(Write::new(workspace.clone(), seen.clone())));

    // The `env` block goes to the commands crucible runs and nowhere else.
    // crucible cannot put a variable in its own environment — writing to one is
    // `unsafe` in edition 2024 — and would not want to: what the block is for
    // is what `cargo test` sees, not what this process sees.
    // And the other end of the row under the box. The clone shares one registry
    // rather than copying it, which is what lets the loop draw what is running and
    // stop one — and what makes the caller's copy the thing that ends them all.
    tools.add(Box::new(
        Bash::new(workspace.clone(), cancel.clone())
            .exporting(settings.env())
            .leaving(leaving.clone()),
    ));

    // The other end of the panel above the prompt. The clone shares one plan
    // rather than copying it, which is what makes a call on the worker thread
    // something the drawing thread sees on its next frame.
    // Deferred from here down. `todo_write` is the largest of them and the one
    // a short session never reaches for; the two web tools are the ones a
    // session without a question about the world never touches at all.
    defer(
        &mut tools,
        &mut held,
        Box::new(TodoWrite::new(plan.clone())),
    );

    // Last, and only where this session has a source. One `Arc` serves both:
    // the two tools ask it different questions, and a session whose vendor
    // answers only one of them registers only that one the day such a source
    // exists.
    if let Some(searching) = reaching.searching {
        defer(
            &mut tools,
            &mut held,
            Box::new(WebSearch::new(searching, cancel.clone())),
        );
    }
    if let Some(fetching) = reaching.fetching {
        defer(
            &mut tools,
            &mut held,
            Box::new(WebFetch::new(fetching, cancel.clone())),
        );
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
        tools.add(Box::new(AskUser::new(Arc::new(putting.clone()))));
    }

    // Last, and only where there is anything to find. A search that can only
    // ever answer "nothing" is a schema spent saying so.
    let looking = ToolSearch::new(held, startup.revealed.clone());
    if !looking.is_empty() {
        tools.add(Box::new(looking));
    }

    tools
}

/// Registers `tool` unadvertised, and records how a search would find it.
///
/// The two go together because they cannot disagree: a tool held back that no
/// search knows about is one the model can never reach, and an entry with no
/// tool behind it is a search that offers something that will not run.
fn defer(tools: &mut Tools, held: &mut Vec<Held>, tool: Box<dyn Tool>) {
    held.push(Held {
        name: tool.name().into(),
        about: about(tool.schema()),
    });
    tools.defer(tool);
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
fn model(
    provider: &str,
    name: Option<&str>,
    effort: Option<Effort>,
    workspace: &Workspace,
    settings: &Settings,
) -> Model {
    let name = name.unwrap_or_default();

    Model {
        // Nothing has ended yet: this is the note the first turn of a run is
        // asked under, and no command has been left running to end.
        system: Some(standing::under(name, effort, workspace, &[]).into()),
        name: name.into(),
        max_tokens: ceiling(provider, name),
        window: window(provider, name, settings),
        effort,
    }
}

/// What the documents together say to do when the window fills.
///
/// Resolved here, whole, so the loop is handed an answer rather than learning
/// that any of this has a spelling in a file. `keep` is the one figure with a
/// default of crucible's own: a session carried on from needs the turn it is in
/// the middle of and the one before it, which is what "carry on from here"
/// means, and nothing about a document makes that number.
fn compacting(settings: &Settings) -> Compaction {
    let said = settings.compaction();
    let asked = Compaction::default();

    Compaction {
        automatic: said.when.automatic(),
        reserve: said.reserve,
        keep: said
            .keep
            .and_then(|keep| usize::try_from(keep).ok())
            .unwrap_or(asked.keep),
        spend_ceiling: said.spend_ceiling,
        ask_on_resume: said.ask_on_resume,
    }
}

/// How much this model accepts at once, in tokens, or nothing where nobody
/// knows.
///
/// What a layer wrote down first, then what the generated table says. There is
/// no third answer and deliberately no default: a window invented here would be
/// wrong by a factor nobody could see, and a session would throw most of itself
/// away — or die at the vendor — before anybody could tell why. Where nothing is
/// known the turn simply runs without a proactive bound, and the provider
/// refusing is what makes room instead.
fn window(provider: &str, model: &str, settings: &Settings) -> Option<u32> {
    settings
        .context_window(provider, model)
        .or_else(|| super::facts(provider, model).map(|facts| facts.window))
}

/// How long an answer to ask this model for.
///
/// The model's own limit held under [`CEILING`], or [`UNKNOWN_CEILING`] where
/// this build has never heard of it. Read off the name exactly as it was asked
/// for — a name one word from a listed one is a name nothing is known about,
/// and borrowing the neighbour's figure is how a request comes to be refused
/// for a reason nobody can see.
fn ceiling(provider: &str, model: &str) -> u32 {
    super::facts(provider, model).map_or(UNKNOWN_CEILING, |facts| facts.output.min(CEILING))
}

#[cfg(test)]
mod tests;
