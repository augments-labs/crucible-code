//! `/logout`: the providers a credential was written down for, and forgetting
//! one.
//!
//! The list is what is *there* rather than what could be. `/login` offers the
//! account and key routes this build can store; this one offers providers with
//! a stored credential, because the rest have nothing this command can remove.
//! That is the whole difference between the two panels, and it is why this
//! reads the store before it draws anything.
//!
//! What it reaches is that file and only that file. A key inherited from the
//! launching environment is not something a child process can take away from
//! its parent shell. The command therefore names the source that remains and
//! tells the user where it has to be removed; it never says the provider is
//! signed out while that source is active.
//!
//! An installed agent the user's settings name is listed too, after the
//! credentials, though nothing of Crucible's is stored for it. Taking one
//! offers what can be done with it, and each of those is the application's
//! account operation: its own questions are drawn as it puts them, and
//! nothing changes until one of them is answered with a way to go on.

use crucible_app::Conversation;
use crucible_app::client::Performed;
use crucible_app::client::accounts::{CONNECT, DISCONNECT, REMOVE, SIGN_OUT};
use crucible_app::switching::{LoggedOut, Retained};
use crucible_client_api::{Command, Name};
use crucible_config::ExternalAgent;
use crucible_tui::{Offered, Panel, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::converse::picking::{self, Picked, Taken};
use crucible_app::providers::{CredentialSource, Providers, Served, offered};
use crucible_app::startup::served;

use super::login::{answered, ended, executing};
use super::{Terms, about, say};

/// The sentence under the panel's title: which of the two places a key can be
/// this reaches, since a panel of names cannot say it.
const SAID: &str = concat!(
    "Choose a credential stored by Crucible to remove. Environment credentials ",
    "are inherited from the launching shell and cannot be removed here."
);

/// The panel's title where an agent is listed.
const ACTED: &str = "Sign out or disconnect";

/// The sentence under it where credentials are listed beside the agents.
const BESIDE: &str = concat!(
    "Choose a credential stored by Crucible to remove, or an installed agent to act on. ",
    "Environment credentials are inherited from the launching shell and cannot be ",
    "removed here."
);

/// The sentence under it where only agents are listed.
const AGENTS: &str = "Choose an installed agent to act on.";

/// What an agent's row is, beside what it runs.
const INSTANCE: &str = "external agent instance";

/// The sentence under an agent's own panel.
const ACTING: &str = "Choose what to do with this agent.";

/// Said where an agent's question had no room to stand.
const CRAMPED: &str = "! this needs an answer first; make the window taller and try /logout again";

/// Said where an agent's own panel had no room to stand.
const UNROOMED: &str = "! this needs more room; make the window taller and try /logout again";

/// What is left to say once a key has been forgotten.
const KEPT: &str = "the active provider is unchanged";

/// What escape leaves behind, in place of the rows it used to write.
const LEFT: &str = "cancelled, nothing signed out";

/// Runs it: the one named forgotten, one chosen off the panel forgotten, or
/// what there is to choose from.
///
/// `keys` is whether there is a keyboard to walk a panel with. Down a pipe
/// there is not, and a panel waiting for a key nobody can press is a session
/// that stopped — so what a piped run gets is the names as rows.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Result<(), Fatal> {
    let providers = terms.providers.snapshot();
    let stored = terms.logins.read();
    // By provider, under any name a row of theirs writes a credential under,
    // and never a name this build does not write, which no row here gave.
    let names: Vec<&str> = offered(&providers)
        .map(|one| one.name)
        .filter(|name| stored.held(name).is_some())
        .collect();
    let held = held(&providers, &names);
    let agents = terms.settings.external_agents();

    // Before anything is drawn: a panel of nothing has no entry to take and no
    // reason to be stood up, and the rows underneath would be none as well.
    if held.is_empty() && agents.is_empty() {
        let remaining = conversation
            .serving()
            .and_then(|provider| served(&providers, provider).ok())
            .and_then(|provider| (terms.serving)(provider, &stored).ok());
        if let Some(remaining) = remaining {
            return say(
                renderer,
                &not_stored(
                    conversation.serving().unwrap_or_default(),
                    &remaining.source,
                ),
            );
        }
        return say(
            renderer,
            "nothing is stored by Crucible; unset any environment keys in the shell",
        );
    }

    if let Some(named) = held.iter().copied().find(|one| one.name == said) {
        return forgetting(named, renderer, conversation, terms);
    }
    // An agent's ways on are questions, which want a keyboard to answer them.
    if let Some(named) = agents.iter().find(|one| keys && one.name() == said) {
        return acting(named, renderer, conversation, terms);
    }

    // Nobody named and a keyboard to walk a list with: the panel, and what comes
    // off it is the same fact as a name typed on the line.
    if keys && said.is_empty() {
        let targets: Vec<Target<'_>> = held
            .iter()
            .copied()
            .map(Target::Stored)
            .chain(agents.iter().map(Target::Agent))
            .collect();
        match chosen(&targets, renderer, terms)? {
            Taken::Took(Target::Stored(taken)) => {
                return forgetting(taken, renderer, conversation, terms);
            }
            Taken::Took(Target::Agent(taken)) => {
                return acting(taken, renderer, conversation, terms);
            }
            // Escape asked for the screen that was there before the panel. The
            // rows under it would be the same question put a second time.
            Taken::Left => return say(renderer, LEFT),
            Taken::Cramped => {}
        }
    }

    // The word came off the line and was never shape-checked — anything at all
    // can follow `/logout ` — so it goes out the way arrived text goes out. It
    // is one sentence for a name this build never heard of and for a provider
    // holding no key here, because it is true of both and the difference is not
    // one the answer underneath leaves anybody guessing at.
    if !said.is_empty() {
        renderer.commit(&format!("! no credential for {said} is stored by Crucible"))?;
    }

    let columns = renderer.transcript_columns();
    let rows: Vec<Row> = held
        .into_iter()
        .map(|one| {
            let said = about(
                &format!("/logout {}", one.name),
                &reaches(one),
                terms.style().glyphs(),
            );

            Row::new().then(Slot::Quiet, clip(&said, columns))
        })
        .chain(agents.iter().map(|one| {
            let said = about(
                &format!("/logout {}", one.name()),
                &runs(one),
                terms.style().glyphs(),
            );
            Row::new().then(Slot::Quiet, clip(&said, columns))
        }))
        .collect();

    Ok(renderer.present(&rows)?)
}

/// The providers this build serves among `stored`, the providers the store
/// holds a credential for.
///
/// In registry order rather than the store's, so this list and `/login`'s
/// read the same way down. A name in the file that this build does not serve is
/// not among them: it names no provider here, so there is nothing to draw it as
/// and no session that could have asked with it. The file is where it came from
/// and the file is where it stays.
fn held(providers: &Providers, stored: &[&str]) -> Vec<Served> {
    offered(providers)
        .filter(|one| stored.contains(&one.name))
        .collect()
}

/// What forgetting one provider's key reaches, and what it leaves.
///
/// The one thing that differs between entries that would otherwise read
/// identically, and the thing worth knowing before pressing Enter: the variable
/// is named so it can be seen not to be in this.
fn reaches(one: Served) -> String {
    format!("stored by Crucible; not inherited from {}", one.key)
}

/// One row of the panel: a credential Crucible stored, or an agent the
/// settings name.
#[derive(Clone, Copy)]
enum Target<'a> {
    Stored(Served),
    Agent(&'a ExternalAgent),
}

/// What an agent's row says: what it is, and the program it runs.
fn runs(agent: &ExternalAgent) -> String {
    format!("{INSTANCE}; run as {}", agent.executable())
}

/// Stands the panel where the prompt box was, and says what was taken off it.
///
/// `None` is a panel that was left and a window with no room to stand one in
/// alike. Neither is a provider, and both come out as the rows above — which is
/// the answer `/logout` always has, and the only one a short window can be
/// given.
fn chosen<'a, T: Terminal>(
    targets: &[Target<'a>],
    renderer: &mut Renderer<T>,
    terms: &Terms,
) -> Result<Taken<Target<'a>>, Fatal> {
    let says: Vec<String> = targets
        .iter()
        .map(|target| match target {
            Target::Stored(one) => reaches(*one),
            Target::Agent(agent) => runs(agent),
        })
        .collect();

    let shown: Vec<Offered<'_>> = targets
        .iter()
        .zip(&says)
        .map(|(target, says)| Offered {
            name: match target {
                Target::Stored(one) => one.shown,
                Target::Agent(agent) => agent.name(),
            },
            says,
        })
        .collect();

    // The panel of credentials alone reads as it always has.
    let agents = targets
        .iter()
        .any(|target| matches!(target, Target::Agent(_)));
    let stored = targets
        .iter()
        .any(|target| matches!(target, Target::Stored(_)));
    let (title, said) = match (agents, stored) {
        (false, _) => ("Remove stored credential", SAID),
        (true, true) => (ACTED, BESIDE),
        (true, false) => (ACTED, AGENTS),
    };
    let panel = Panel {
        source: None,
        title,
        said: Some(said),
        told: &[],
        shown: &shown,
        chosen: 0,
        // The one key worth naming: the arrows and Enter are what a list with a
        // mark on it is already saying.
        footer: "esc to cancel",
    };

    Ok(picking::pick(renderer, terms.style(), panel)?.of(targets))
}

/// What can be done with an agent, as its panel offers it.
#[derive(Clone, Copy)]
enum Act {
    Connect,
    Disconnect,
    SignOut,
    Remove,
}

/// Offers what can be done with `agent`, and carries out what is taken
/// through the application, drawing each question it puts.
fn acting<T: Terminal>(
    agent: &ExternalAgent,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    let instance = match Name::new(agent.name()) {
        Ok(instance) => instance,
        Err(refusal) => return say(renderer, &astray(&Performed::Refused(refusal))),
    };
    // A connect reaches the provider being asked, so it is offered only
    // where there is one.
    let provider = conversation
        .serving()
        .and_then(|provider| Name::new(provider).ok());
    let acts: Vec<(Act, &str, &str)> = [
        (
            Act::Connect,
            CONNECT,
            "Reach the provider being asked through this agent",
        ),
        (
            Act::Disconnect,
            DISCONNECT,
            "Crucible sends it no new work; its sign-in is kept",
        ),
        (
            Act::SignOut,
            SIGN_OUT,
            "Runs its own sign-out; every instance sharing that sign-in takes no new work",
        ),
        (
            Act::Remove,
            REMOVE,
            "Crucible forgets its settings; its sign-in and history are kept",
        ),
    ]
    .into_iter()
    // A connect reaches a provider, and only an enabled agent takes work
    // a disconnect could stop.
    .filter(|(act, _, _)| match act {
        Act::Connect => provider.is_some(),
        Act::Disconnect => agent.enabled(),
        Act::SignOut | Act::Remove => true,
    })
    .collect();
    let shown: Vec<Offered<'_>> = acts
        .iter()
        .map(|(_, name, says)| Offered { name, says })
        .collect();
    let panel = Panel {
        source: None,
        title: agent.name(),
        said: Some(ACTING),
        told: &[],
        shown: &shown,
        chosen: 0,
        footer: "esc to cancel",
    };
    let act = match picking::pick(renderer, terms.style(), panel)? {
        Picked::Took(at) => acts.get(at).map(|(act, _, _)| *act),
        Picked::Left => return say(renderer, LEFT),
        Picked::Cramped => None,
    };
    let command = match (act, provider) {
        (None, _) => return say(renderer, UNROOMED),
        (Some(Act::Connect), Some(provider)) => Command::Connect {
            provider,
            instance: Some(instance),
        },
        (Some(Act::Connect), None) => return say(renderer, LEFT),
        (Some(Act::Disconnect), _) => Command::Disconnect { instance },
        (Some(Act::SignOut), _) => Command::SignOut { instance },
        (Some(Act::Remove), _) => Command::RemoveIntegration { instance },
    };

    let style = terms.style();
    let mut cramped = false;
    let performed = terms.operate(conversation, command, |question| {
        let replied = answered(renderer, style, question)?;
        cramped |= replied.is_none();
        Ok::<_, Fatal>(replied)
    })?;
    if cramped {
        return say(renderer, CRAMPED);
    }
    match ended(&performed) {
        None => say(renderer, LEFT),
        Some((line, connected)) => {
            say(renderer, &line)?;
            if connected {
                executing(renderer, conversation, style)?;
            }
            Ok(())
        }
    }
}

/// Forgets `named`'s key, and says what that reached.
fn forgetting<T: Terminal>(
    named: Served,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
) -> Result<(), Fatal> {
    // Retired before it is forgotten, forgotten before anything is set up
    // again: the order is the conversation's. What is here is what each way it
    // can end is said as.
    let asking = |provider| Command::Logout { provider };
    let logged_out = match terms.perform_naming(conversation, named.name, asking) {
        Performed::Logout(logged_out) => logged_out,
        other => return say(renderer, &astray(&other)),
    };
    let (retained, status) = match logged_out {
        LoggedOut::CacheHeld(problem) => {
            return super::cache::held(renderer, super::Laid::Hung, &problem);
        }
        LoggedOut::Unforgotten { retained, problem } => {
            super::cache::retained(renderer, super::Laid::Hung, retained)?;
            return say(renderer, &format!("! {problem}"));
        }
        LoggedOut::Kept => (Retained::default(), KEPT.to_owned()),
        LoggedOut::Routed { instance } => (
            Retained::default(),
            format!(
                "{} is still reached through the agent {instance}",
                named.name
            ),
        ),
        LoggedOut::StillServed { retained, source } => (retained, not_stored(named.name, &source)),
        LoggedOut::SignedOut { retained } => {
            (retained, "the active session is now signed out".to_owned())
        }
    };
    super::cache::retained(renderer, super::Laid::Hung, retained)?;

    let columns = renderer.transcript_columns();
    let said = format!("removed the stored credential for {}", named.name);
    let rows = [
        Row::new().then(Slot::Plain, clip(&said, columns)),
        Row::new().then(Slot::Quiet, clip(&status, columns)),
    ];

    Ok(renderer.present(&rows)?)
}

/// What remains after the stored credential moved, without ever carrying the
/// credential value itself.
fn not_stored(provider: &str, source: &CredentialSource) -> String {
    match source {
        CredentialSource::Environment(variable) => {
            format!("{provider} still uses {variable}; unset it in the shell before restarting")
        }
        CredentialSource::StoredKey => {
            format!("{provider} is still authenticated by another stored API key")
        }
        CredentialSource::Subscription => {
            format!("{provider} is still authenticated by another stored account")
        }
    }
}

#[cfg(test)]
mod tests;
