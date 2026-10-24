//! What an account command writes, read by the commands after it in the
//! same run.
//!
//! Each case here holds one run as the terminal holds it: the settings its
//! launch read, and what its own writes changed of them since. Nothing
//! rewrites the user file between two commands, so each is decided from what
//! the commands before it wrote, as it is for someone typing them one after
//! another. A sign-out of an agent whose route has no yes is asked about
//! first, and a no leaves the same files and runs it found.
//!
//! The last cases are an instance of a catalog agent, which serves its own
//! key: no provider is reached through it and no model is offered on its
//! route, a session opened on it is bound to its key and goes on only on that
//! agent, and a sign-out of it, which this build cannot run, is refused before
//! anything is written.

use std::path::Path;
use std::sync::Arc;

use crucible_app::Conversation;
use crucible_app::access::Rewritten;
use crucible_app::backends::{Registry, Row};
use crucible_app::client::{self, Front};
use crucible_app::launching::Steps;
use crucible_app::outside::Installed;
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, LoginOutcome, LogoutOutcome, ModelOutcome, Name,
    Outcome, Pending, Request, ResumeOutcome, Retained, Standing, Text,
};
use crucible_config::{Access, Driver, ExternalAgent, Settings};
use crucible_external_harnesses::fake_peer::{self, Reply, scripts::claude};
use crucible_runner::Turned;
use crucible_sandbox::SandboxEnablement;
use crucible_session::{RouteBinding, Session};
use crucible_types::{AgentKey, Message, SessionId, Transcript};

use super::{
    Asked, CONNECTED_FILE, Choosing, INSTALLED, Outside, Picking, SHARED, UNCONNECTED, UNINSTALLED,
    accounting, accounting_on, answered_off, connected, launching, recorded_on, resuming_at,
    runtime, started_native, stepped, turn,
};
use crate::{Failed, Script, Tree};

/// One run: the settings its launch read, and what its account writes have
/// changed of them since.
struct Run {
    launch: Settings,
    rewritten: Rewritten,
}

impl Run {
    /// A run launched on the user file of `tree` holding `file`.
    fn launched(tree: &Tree, file: &str) -> Result<Self, Failed> {
        let user = crucible_config::user(&tree.home()?);
        if let Some(directory) = user.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(&user, file)?;
        Ok(Self {
            launch: crate::written(tree)?,
            rewritten: Rewritten::new(),
        })
    }

    /// What `command` is answered with by `conversation`, decided at `desk`
    /// from what this run reads now.
    fn perform(
        &self,
        desk: crate::Desk,
        tree: &Tree,
        conversation: &mut Conversation,
        command: Command,
    ) -> Result<Outcome, Failed> {
        self.asked(desk, (tree, conversation), command, None)
    }

    /// [`Run::perform`], of the account operation `command`, asked of a
    /// client that answers as `front` does.
    fn settle(
        &self,
        desk: crate::Desk,
        held: (&Tree, &mut Conversation),
        command: Command,
        front: &mut dyn Front,
    ) -> Result<Outcome, Failed> {
        self.asked(desk, held, command, Some(front))
    }

    fn asked(
        &self,
        desk: crate::Desk,
        (tree, conversation): (&Tree, &mut Conversation),
        command: Command,
        front: Option<&mut dyn Front>,
    ) -> Result<Outcome, Failed> {
        let desk = crate::Desk {
            settings: self.rewritten.over(&self.launch).into_owned(),
            rewritten: self.rewritten.clone(),
            ..desk
        };
        let (workspace, sessions) = (tree.workspace()?, tree.sessions());
        let at = client::Desk {
            switching: desk.with(),
            sessions: &sessions,
            workspace: &workspace,
            reads: |_| false,
            environment: crate::unset,
            notes: |_| Err(crucible_client_api::ErrorCode::InvalidArgument.into()),
        };
        let request = Request::new(Asking::every(), Correlation::new(1), command);
        let performed = runtime()?.block_on(async {
            match front {
                Some(front) => client::account(conversation, &request, &at, front).await,
                None => client::perform(conversation, &request, &at).await,
            }
        });
        Ok(performed.response(&request).outcome)
    }
}

/// A connect through `instance`, or through none.
fn connect(instance: Option<&str>) -> Result<Command, Failed> {
    Ok(Command::Connect {
        instance: instance.map(Name::new).transpose()?,
    })
}

/// A key logged in for `provider`, stored first.
fn keyed(tree: &Tree, provider: &str) -> Result<(crate::Desk, Command), Failed> {
    let desk = crate::Desk::stored(tree)?;
    desk.logins.keep(provider, "a-key-no-vendor-issued")?;
    let login = Command::Login {
        provider: Name::new(provider)?,
    };
    Ok((desk, login))
}

/// Whether the last run of the peer kept in `peer` was started on `model`.
fn started_on(peer: &Path, model: &str) -> bool {
    fake_peer::argued(peer)
        .last()
        .is_some_and(|start| start.windows(2).any(|pair| pair == ["--model", model]))
}

/// What the user file of `tree` holds now.
fn file(tree: &Tree) -> Result<String, Failed> {
    Ok(std::fs::read_to_string(crucible_config::user(
        &tree.home()?,
    ))?)
}

/// What `outcome` says, with the text of its problem where it has one, which
/// its debug form leaves out.
fn told(outcome: &Outcome) -> String {
    let problem = match outcome {
        Outcome::Model(ModelOutcome::Unreachable(problem))
        | Outcome::Resumed(ResumeOutcome::Failed(problem))
        | Outcome::Login(LoginOutcome::ConnectUnsaved { problem, .. }) => Some(problem),
        _ => None,
    };
    match problem {
        Some(problem) => format!("{outcome:?}: {}", problem.message.as_str()),
        None => format!("{outcome:?}"),
    }
}

/// A model of the provider a connect routed through an agent, taken once a
/// login moved the conversation off the agent onto another provider, moves
/// it back onto the agent, on that model: the route the connect wrote is how
/// the provider is reached for the rest of the run, though the files read at
/// launch had none.
#[test]
fn a_model_of_the_provider_a_connect_routed_moves_back_onto_the_agent() -> Result<(), Failed> {
    let tree = Tree::new("routing-model-back")?;
    let run = Run::launched(&tree, UNCONNECTED)?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let script = connected([claude::signed_in(), claude::clean()]);
    let (mut conversation, peer) = stepped(&tree, native, &script)?;
    let onto = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(Some("work"))?,
        &mut Picking::of([]),
    )?;
    assert!(
        matches!(
            onto,
            Outcome::Login(LoginOutcome::AgentConnected { onto: Some(_), .. })
        ),
        "{onto:?}"
    );
    let (desk, login) = keyed(&tree, "openai")?;
    let away = run.perform(desk, &tree, &mut conversation, login)?;
    assert!(
        matches!(away, Outcome::Login(LoginOutcome::MovedOff { .. })),
        "{away:?}"
    );

    let back = run.perform(
        crate::Desk::stored(&tree)?,
        &tree,
        &mut conversation,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;

    assert_eq!(
        back,
        Outcome::Model(ModelOutcome::MovedOnto {
            onto: Text::cut("work"),
            unwritten: None,
        }),
        "{}",
        told(&back)
    );
    let (turned, _) = crate::turn(&mut conversation, "back")?;
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert!(
        started_on(&peer, "claude-sonnet-5-5"),
        "{:?}",
        fake_peer::argued(&peer)
    );
    Ok(())
}

/// A second connect naming no instance, once a login moved the conversation
/// off the agent a first connect installed, goes through the instance the
/// first wrote rather than installing it again.
#[test]
fn a_second_connect_goes_through_the_instance_the_first_installed() -> Result<(), Failed> {
    let tree = Tree::new("routing-connect-again")?;
    let run = Run::launched(&tree, UNINSTALLED)?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let script = connected([claude::signed_in(), claude::signed_in()]);
    let (mut conversation, _peer) = stepped(&tree, native, &script)?;
    let first = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(None)?,
        &mut Picking::of([Some("Claude Code")]),
    )?;
    assert!(
        matches!(
            first,
            Outcome::Login(LoginOutcome::AgentConnected { onto: Some(_), .. })
        ),
        "{first:?}"
    );
    let (desk, login) = keyed(&tree, "anthropic")?;
    let away = run.perform(desk, &tree, &mut conversation, login)?;
    assert!(
        matches!(away, Outcome::Login(LoginOutcome::MovedOff { .. })),
        "{away:?}"
    );

    let again = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(None)?,
        &mut Picking::of([Some("Claude Code")]),
    )?;

    assert_eq!(
        again,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: Text::cut(&client::accounts::moved_onto("claude")),
            onto: Some(Name::new("claude")?),
        }),
        "{}",
        told(&again)
    );
    let written = crate::written(&tree)?;
    assert_eq!(
        written.access("anthropic"),
        Some(Access::ExternalAgent {
            instance: "claude".into()
        })
    );
    assert_eq!(written.external_agents().len(), 1, "{}", file(&tree)?);
    Ok(())
}

/// A session an agent ran under an instance a disconnect earlier in the run
/// turned off is refused saying so, as it is where the run was launched with
/// the instance off, and nothing is started or opened.
#[test]
fn a_session_on_an_instance_disconnected_during_the_run_is_refused() -> Result<(), Failed> {
    let tree = Tree::new("routing-resume-disconnected")?;
    let run = Run::launched(&tree, INSTALLED)?;
    let id = recorded_on(&tree, "work", "before")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let current = native.session().id().cloned();
    let (mut conversation, peer) = stepped(&tree, native, &connected([]))?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::DISCONNECT),
        put: Vec::new(),
    };
    let off = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        Command::Disconnect {
            instance: Name::new("work")?,
        },
        &mut choosing,
    )?;
    assert!(
        matches!(off, Outcome::Logout(LogoutOutcome::Disconnected { .. })),
        "{off:?}"
    );
    let started = fake_peer::played(&peer);

    let resumed = run.perform(
        crate::Desk::new(&tree, &[])?,
        &tree,
        &mut conversation,
        Command::Resume(id),
    )?;

    let said = told(&resumed);
    assert!(
        matches!(resumed, Outcome::Resumed(ResumeOutcome::Failed(_)))
            && said.contains("work cannot be started"),
        "{said}"
    );
    assert_eq!(conversation.session().id(), current.as_ref());
    assert_eq!(
        fake_peer::played(&peer),
        started,
        "the agent's program was started"
    );
    Ok(())
}

/// A model taken on an agent's route is the one the agent is started on when
/// a connect later in the run moves the conversation back onto it.
#[test]
fn a_connect_back_onto_an_agent_starts_it_on_the_model_taken_on_it() -> Result<(), Failed> {
    let tree = Tree::new("routing-connect-model")?;
    let run = Run::launched(&tree, UNCONNECTED)?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let script = connected([claude::signed_in(), claude::signed_in(), claude::clean()]);
    let (mut conversation, peer) = stepped(&tree, native, &script)?;
    let onto = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(Some("work"))?,
        &mut Picking::of([]),
    )?;
    assert!(
        matches!(
            onto,
            Outcome::Login(LoginOutcome::AgentConnected { onto: Some(_), .. })
        ),
        "{onto:?}"
    );
    let taken = run.perform(
        crate::Desk::new(&tree, &[])?,
        &tree,
        &mut conversation,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;
    assert!(
        matches!(taken, Outcome::Model(ModelOutcome::Taken { .. })),
        "{taken:?}"
    );
    let (desk, login) = keyed(&tree, "openai")?;
    let away = run.perform(desk, &tree, &mut conversation, login)?;
    assert!(
        matches!(away, Outcome::Login(LoginOutcome::MovedOff { .. })),
        "{away:?}"
    );
    let back = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(Some("work"))?,
        &mut Picking::of([]),
    )?;
    assert!(
        matches!(
            back,
            Outcome::Login(LoginOutcome::AgentConnected { onto: Some(_), .. })
        ),
        "{back:?}"
    );

    let (turned, _) = crate::turn(&mut conversation, "back")?;

    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert!(
        started_on(&peer, "claude-sonnet-5-5"),
        "{:?}",
        fake_peer::argued(&peer)
    );
    Ok(())
}

/// An instance a connect installed during the run can be disconnected in it.
#[test]
fn an_instance_a_connect_installed_can_be_disconnected_in_the_same_run() -> Result<(), Failed> {
    let tree = Tree::new("routing-disconnect-installed")?;
    let run = Run::launched(&tree, UNINSTALLED)?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let (mut conversation, _peer) = stepped(&tree, native, &connected([claude::signed_in()]))?;
    let installed = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(None)?,
        &mut Picking::of([Some("Claude Code")]),
    )?;
    assert!(
        matches!(
            installed,
            Outcome::Login(LoginOutcome::AgentConnected { .. })
        ),
        "{installed:?}"
    );
    let mut choosing = Choosing {
        choice: Some(client::accounts::DISCONNECT),
        put: Vec::new(),
    };

    let off = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        Command::Disconnect {
            instance: Name::new("claude")?,
        },
        &mut choosing,
    )?;

    assert!(
        matches!(off, Outcome::Logout(LogoutOutcome::Disconnected { .. })),
        "{off:?}"
    );
    let agents = crate::written(&tree)?.external_agents();
    assert!(
        !agents.is_empty() && agents.iter().all(|agent| !agent.enabled()),
        "{}",
        file(&tree)?
    );
    Ok(())
}

/// A session an agent ran under the instance a connect installed during the
/// run resumes on it, once a login has moved the conversation off it.
#[test]
fn a_session_on_an_instance_a_connect_installed_resumes_on_it() -> Result<(), Failed> {
    let tree = Tree::new("routing-resume-installed")?;
    let run = Run::launched(&tree, UNINSTALLED)?;
    let id = recorded_on(&tree, "claude", "before")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let (mut conversation, _peer) = stepped(&tree, native, &connected([claude::signed_in()]))?;
    let installed = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(None)?,
        &mut Picking::of([Some("Claude Code")]),
    )?;
    assert!(
        matches!(
            installed,
            Outcome::Login(LoginOutcome::AgentConnected { .. })
        ),
        "{installed:?}"
    );
    let (desk, login) = keyed(&tree, "openai")?;
    let away = run.perform(desk, &tree, &mut conversation, login)?;
    assert!(
        matches!(away, Outcome::Login(LoginOutcome::MovedOff { .. })),
        "{away:?}"
    );

    let resumed = run.perform(
        crate::Desk::new(&tree, &[])?,
        &tree,
        &mut conversation,
        Command::Resume(id.clone()),
    )?;

    assert!(
        matches!(resumed, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{}",
        told(&resumed)
    );
    assert_eq!(conversation.session().id(), Some(&id));
    assert_eq!(
        conversation
            .session()
            .route()
            .map(|route| route.instance().to_owned()),
        Some("claude".to_owned())
    );
    Ok(())
}

/// A key logged in, after a connect moved the conversation onto the agent,
/// for the provider it was moved off is written down as how that provider is
/// reached, though the files read at launch had no route, and moves the
/// conversation back onto it.
#[test]
fn a_key_for_the_provider_a_connect_moved_off_moves_back_onto_it() -> Result<(), Failed> {
    let tree = Tree::new("routing-connect-then-key")?;
    let run = Run::launched(&tree, UNCONNECTED)?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let script = connected([claude::signed_in(), claude::clean()]);
    let (mut conversation, _peer) = stepped(&tree, native, &script)?;
    let moved = run.settle(
        crate::Desk::new(&tree, &[])?,
        (&tree, &mut conversation),
        connect(Some("work"))?,
        &mut Picking::of([]),
    )?;
    assert!(
        matches!(moved, Outcome::Login(LoginOutcome::AgentConnected { .. })),
        "{moved:?}"
    );
    let connected = file(&tree)?;
    let (desk, login) = keyed(&tree, "anthropic")?;

    let outcome = run.perform(desk, &tree, &mut conversation, login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("anthropic"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    assert!(conversation.session().route().is_none());
    assert_eq!(connected, CONNECTED_FILE);
    let keyed: serde_json::Value = serde_json::from_str(&file(&tree)?)?;
    assert_eq!(
        keyed.pointer("/providers/anthropic/access/type"),
        Some(&serde_json::json!("apiKey"))
    );
    assert_eq!(
        keyed.pointer("/provider"),
        Some(&serde_json::json!("anthropic"))
    );
    Ok(())
}

/// A session a provider's model ran, on the disk of `tree`, holding `said`.
fn recorded_natively(tree: &Tree, said: &str) -> Result<SessionId, Failed> {
    let session = Session::start(&tree.sessions(), &tree.workspace()?, None)?;
    let id = session.id().cloned().ok_or("an id")?;
    session.append(&Message::User {
        text: said.into(),
        attachments: Box::new([]),
    });
    drop(session);
    Ok(id)
}

/// A key logged out of on an agent's route, for the provider the
/// conversation asked before a connect moved it onto the route, is gone from
/// what was kept beside the route too: a session a provider's model ran
/// waits for the next launch rather than resuming on the forgotten key.
/// That holds where the route stands in for the provider, and where it
/// stands in for another.
#[test]
fn a_key_logged_out_of_on_an_agent_s_route_is_not_resumed_on() -> Result<(), Failed> {
    for (provider, routed) in [("anthropic", true), ("openai", false)] {
        let tree = Tree::new(&format!("routing-logout-{provider}"))?;
        let run = Run::launched(&tree, UNCONNECTED)?;
        let id = recorded_natively(&tree, "before")?;
        crate::Desk::stored(&tree)?
            .logins
            .keep(provider, "a-key-no-vendor-issued")?;
        let native = crate::conversing(
            &tree,
            Script::new(Vec::new()),
            &crate::Guard::Nothing,
            Some(provider),
        )?;
        let script = connected([claude::signed_in(), claude::clean()]);
        let (mut conversation, _peer) = stepped(&tree, native, &script)?;
        let onto = run.settle(
            crate::Desk::new(&tree, &[])?,
            (&tree, &mut conversation),
            connect(Some("work"))?,
            &mut Picking::of([]),
        )?;
        assert!(
            matches!(onto, Outcome::Login(LoginOutcome::AgentConnected { .. })),
            "{onto:?}"
        );
        let current = conversation.session().id().cloned();

        let out = run.perform(
            crate::Desk::stored(&tree)?,
            &tree,
            &mut conversation,
            Command::Logout {
                provider: Name::new(provider)?,
            },
        )?;
        let resumed = run.perform(
            crate::Desk::stored(&tree)?,
            &tree,
            &mut conversation,
            Command::Resume(id),
        )?;

        if routed {
            assert_eq!(
                out,
                Outcome::Logout(LogoutOutcome::StillServed {
                    retained: Retained::default(),
                    standing: Standing::External {
                        instance: Text::cut("work"),
                    },
                }),
                "{provider}"
            );
        } else {
            assert!(
                matches!(out, Outcome::Logout(LogoutOutcome::Kept { .. })),
                "{provider}: {out:?}"
            );
        }
        assert_eq!(
            resumed,
            Outcome::Resumed(ResumeOutcome::NextLaunch),
            "{provider}: {}",
            told(&resumed)
        );
        assert_eq!(conversation.session().id(), current.as_ref(), "{provider}");
    }
    Ok(())
}

/// Whether `put` holds the question for the route of an installed agent.
fn asked_route(put: &[Pending]) -> bool {
    put.iter().any(|pending| {
        matches!(pending, Pending::Warning { route, .. } if route.as_str() == "agent:claude")
    })
}

/// A sign-out of an agent whose route has no yes puts the route to the client
/// before anything changes: a yes is written down and the sign-out goes on; a
/// no stops no instance, writes nothing and starts nothing of the agent's.
#[test]
fn a_sign_out_of_an_unanswered_agent_asks_its_route_before_anything_changes() -> Result<(), Failed>
{
    for (case, answer) in [("declined", None), ("accepted", Some("Accept"))] {
        let tree = Tree::new(&format!("external-sign-out-asked-{case}"))?;
        let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
        let mut outside = Outside::new(&tree, &script)?;
        let consent = outside.conversation.consent().ok_or("a consent")?.clone();
        consent.keeps_in(crate::Desk::new(&tree, &[])?.choosing);
        consent.withdraw("agent:claude");
        // The conversation's run is started as it opens, and is counted once
        // the peer has read its script: after the probe, the status check and
        // it.
        for _ in 0..500 {
            if fake_peer::played(&outside.peer) >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let before = fake_peer::played(&outside.peer);
        let asked = Asked {
            installed: (SHARED, "work"),
            command: &|instance| Command::SignOut { instance },
            asking: Asking::every(),
            after: None,
        };
        let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), answer]);

        let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;
        let started = fake_peer::played(&outside.peer) - before;

        assert!(
            asked_route(&picking.put),
            "{case}: the client is not asked about the agent's route: {:?}",
            picking.put
        );
        if answer.is_some() {
            assert!(
                matches!(
                    outcome,
                    Outcome::Logout(LogoutOutcome::AgentSignedOut { .. })
                ),
                "{case}: {outcome:?}"
            );
            assert!(file.contains("agent:claude"), "{case}: {file}");
            assert!(file.contains(r#""enabled": false"#), "{case}: {file}");
            assert_eq!(started, 2, "{case}: a logout and a status step");
        } else {
            assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided), "{case}");
            assert_eq!(file, SHARED, "{case}: an instance was written off");
            assert_eq!(started, 0, "{case}: the agent's program was started");
            let (turned, _events) = turn(&mut outside, "again", false)?;
            assert!(turned.is_ok(), "{case}: the run was stopped: {turned:?}");
        }
    }
    Ok(())
}

/// A sign-out of an agent whose route has no yes, accepted where the yes
/// cannot be written down because the consent has no file, ends as a connect
/// does in that case: the client is told the yes could not be written, with
/// the problem, and nothing is stopped, written or started.
#[test]
fn a_sign_out_whose_yes_cannot_be_written_says_nothing_changed() -> Result<(), Failed> {
    let tree = Tree::new("external-sign-out-yes-unwritten")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
    let mut outside = Outside::new(&tree, &script)?;
    outside.unanswer();
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);
    let asked = Asked {
        installed: (SHARED, "work"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), Some("Accept")]);

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut picking)?;
    let started = fake_peer::played(&outside.peer) - before;

    match &outcome {
        Outcome::Logout(LogoutOutcome::HoldUnkept { said, problem }) => {
            assert_eq!(said.as_str(), client::accounts::HOLD_UNKEPT);
            assert!(!problem.message.as_str().is_empty(), "{problem:?}");
        }
        other => return Err(format!("{other:?}").into()),
    }
    assert!(asked_route(&picking.put), "{:?}", picking.put);
    assert_eq!(file, SHARED, "an instance was written off");
    assert_eq!(started, 0, "the agent's program was started");
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_ok(), "the run was stopped: {turned:?}");
    Ok(())
}

/// The user file holding the one instance `work`, of the catalog agent
/// `agent`.
fn catalog_instance(agent: &str) -> String {
    serde_json::json!({"externalAgents": {
        "work": {"driver": "catalog", "agent": agent, "executable": "goose"}
    }})
    .to_string()
}

/// A session a catalog agent ran is bound to that agent's key, so once its
/// instance names another agent the session is refused as run by an agent
/// this run is not using, while an instance still naming the same agent is
/// its route; nothing is started or opened either way.
#[test]
fn a_session_a_catalog_agent_ran_does_not_resume_on_another_agent() -> Result<(), Failed> {
    let tree = Tree::new("external-resume-catalog-repointed")?;
    let opened = RouteBinding::new("catalog:goose", "work", None)?;
    let session = Session::start_routed(&tree.sessions(), &tree.workspace()?, None, &opened)?;
    let id = session.id().cloned().ok_or("an id")?;
    session.append(&Message::User {
        text: "before".into(),
        attachments: Box::new([]),
    });
    drop(session);
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let current = native.session().id().cloned();
    let (mut conversation, peer) = stepped(&tree, native, &connected([]))?;
    let started = fake_peer::played(&peer);

    for (agent, says) in [
        ("codex", "run by the installed agent work"),
        ("goose", "work cannot be started"),
    ] {
        let outcome = resuming_at(
            &tree,
            &mut conversation,
            &catalog_instance(agent),
            id.clone(),
        )?;
        let told = match &outcome {
            Outcome::Resumed(ResumeOutcome::Failed(problem)) => problem.message.as_str().to_owned(),
            other => format!("{other:?}"),
        };
        assert!(told.contains(says), "{agent}: {told}");
    }

    assert_eq!(conversation.session().id(), current.as_ref());
    assert_eq!(
        fake_peer::played(&peer),
        started,
        "the agent's program was started"
    );
    Ok(())
}

/// A provider whose access names an instance of a catalog agent is refused
/// naming the agent and no provider to write it under, since a catalog agent
/// reaches no provider; and a conversation on a catalog agent's route offers
/// no models, since the agent chooses its own.
#[test]
fn a_catalog_agent_reaches_no_provider_and_offers_no_model() -> Result<(), Failed> {
    let tree = Tree::new("external-catalog-reaches-none")?;
    let mut native = started_native(&tree, Script::new(Vec::new()))?;
    let through = serde_json::json!({
        "externalAgents": {"work": {"driver": "catalog", "agent": "goose", "executable": "goose"}},
        "providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}}
    })
    .to_string();

    let refused = answered_off(
        &tree,
        &mut native,
        &through,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    let said = problem.message.as_str();
    assert!(
        said.contains("providers.anthropic.access names the instance work")
            && said.contains("catalog agent goose"),
        "{said}"
    );
    assert!(!said.contains("write it under"), "{said}");

    let route = RouteBinding::new("catalog:goose", "work", None)?;
    let (launching, _peer) = launching(&tree, &connected([]))?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &route,
    )?);
    let mut conversation = crate::conversation(&tree, Script::new(Vec::new()), false)?;
    let installed = Installed {
        backend: None,
        relaunch: Box::new(launching),
        route,
    };
    runtime()?.block_on(async {
        conversation
            .external(session, Transcript::new(), installed)
            .await
            .map_err(|refused| refused.reason().as_str().to_owned())
    })?;

    assert_eq!(
        client::snapshot(&conversation).execution,
        crucible_client_api::Execution::Instance(Name::new("work")?)
    );
    assert_eq!(client::reaching(&conversation), None);
    Ok(())
}

/// A catalog agent has no sign-out this build can run, so a sign-out of one
/// is said to be refused once it is chosen, before its route is asked about
/// or anything is written: neither its own instance nor another catalog
/// agent's is taken out of use. The question it was chosen from names only
/// the instances of its own agent as sharing its sign-in.
#[test]
fn a_sign_out_of_a_catalog_agent_is_refused_and_writes_nothing() -> Result<(), Failed> {
    const TWO_AGENTS: &str = r#"{
  "externalAgents": {
    "goose": {"driver": "catalog", "agent": "goose", "executable": "goose"},
    "codex": {"driver": "catalog", "agent": "codex", "executable": "codex"}
  }
}
"#;
    let tree = Tree::new("external-sign-out-catalog")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let (mut conversation, peer) = stepped(&tree, native, &connected([]))?;
    let started = fake_peer::played(&peer);
    let asked = Asked {
        installed: (TWO_AGENTS, "goose"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after: None,
    };
    let mut picking = Picking::of([Some(client::accounts::SIGN_OUT), Some("Go back")]);

    let (outcome, file) = accounting_on(&tree, &mut conversation, &asked, &mut picking)?;

    assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided), "{file}");
    assert_eq!(file, TWO_AGENTS, "an instance was written off");
    assert_eq!(
        fake_peer::played(&peer),
        started,
        "an agent's program was started"
    );
    let facts: Vec<Vec<&str>> = picking
        .put
        .iter()
        .filter_map(|pending| match pending {
            Pending::Account { facts, .. } => Some(facts.iter().map(Text::as_str).collect()),
            _ => None,
        })
        .collect();
    let [chosen, refused] = facts.as_slice() else {
        return Err(format!("{facts:?}").into());
    };
    assert!(
        chosen.contains(&"Sharing: goose use this profile, and each is disconnected first."),
        "{chosen:?}"
    );
    assert!(
        refused.iter().any(|fact| fact.contains("cannot run")),
        "{refused:?}"
    );
    assert!(
        !picking
            .put
            .iter()
            .any(|pending| matches!(pending, Pending::Warning { .. })),
        "the route was asked about: {:?}",
        picking.put
    );
    Ok(())
}

/// A new session opened on a catalog instance is bound to the agent its key
/// names, `catalog:<key>` on the session's route line, so it can go on only
/// on that agent. The catalog driver's row here is Claude Code's, played by
/// the peer, standing in for the row a catalog entry will give.
#[test]
fn a_session_opened_on_a_catalog_agent_is_bound_to_its_key() -> Result<(), Failed> {
    let tree = Tree::new("routing-catalog-bound")?;
    let (launching, peer) = launching(&tree, &connected([]))?;
    let row: &'static Row = Box::leak(Box::new(Row {
        driver: Driver::Catalog,
        ..*launching.row
    }));
    let steps = Steps::new(
        launching.launcher(),
        Arc::new(SandboxEnablement::new(false, false)),
        Box::leak(Box::new(Registry::new(std::slice::from_ref(row)))),
        Some(peer.clone().into_os_string()),
    );
    let goose = AgentKey::new("goose")?;
    let entry = ExternalAgent::unwritten("work", Driver::Catalog, "claude", None, Some(&goose))
        .ok_or("a catalog instance naming its agent")?;
    let mut conversation = crate::conversation(&tree, Script::new(Vec::new()), false)?
        .consenting(launching.consent.clone())
        .stepping(steps);
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);

    runtime()?.block_on(conversation.onto(&sessions, &workspace, None, &entry))?;

    let bound = conversation
        .session()
        .route()
        .map(|route| route.driver().to_owned());
    assert_eq!(bound.as_deref(), Some("catalog:goose"));
    let id = conversation.session().id().cloned().ok_or("an id")?;
    let (_, opened) = crucible_session::pick_routed(&sessions, &workspace, Some(&id))?;
    assert_eq!(
        opened.map(|route| route.driver().to_owned()).as_deref(),
        Some("catalog:goose"),
        "the route line on the disk"
    );
    Ok(())
}
