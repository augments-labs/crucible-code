//! The same conversation, driven only by what a client could have sent.
//!
//! Every request here is written to the bytes it would travel as and read
//! back before the application sees it, and every decision about a pending
//! action arrives the same way. What the tests then read is what a client
//! would be handed: a response, a snapshot, progress — and, beside those, the
//! one thing no client is handed, whether the tool ran.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use crucible_agents::{AgentBuilder, Model};
use crucible_app::Conversation;
use crucible_app::client::{self, Ended, Front, Performed, Shown};
use crucible_app::switching::{LoggedIn, LoggedOut};
use crucible_client_api::{
    Capabilities, ClearOutcome, Command, Correlation, Decision, ErrorCode, Lasting, Missing, Mode,
    ModelOutcome, Name, NotesOutcome, Outcome, Palette, Pending, PendingId, Progress, Prompt,
    Refusal, Request, Response, ResumeOutcome, Ruling, Snapshot, Stop, Theme, TurnOutcome,
};
use crucible_models::Delta;
use crucible_runner::{EventEnvelope, Runner, Tools};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_session::Session;
use crucible_tools::{
    Approved, DescribeTool, Permission, Rules, Sensitivity, Summary, Target, Tool, ToolContext,
    ToolError, ToolOutput,
};
use crucible_types::{AgentId, StopReason, ToolArgs, ToolId};

use super::{Desk as Standing, Failed, Script, Tree, saying};

/// The name the counting tool is offered and called by.
const WRITE: &str = "write";

/// Changes a file it never names, and counts how often it was let.
#[derive(Debug)]
struct Counting(Arc<AtomicUsize>);

impl DescribeTool for Counting {
    fn name(&self) -> &str {
        WRITE
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Counting {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::MutatesFile {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ToolOutput::ok("done"))
        })
    }
}

/// One round in which the model reaches for the counting tool.
fn calling() -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: WRITE.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A conversation over `script` that asks before anything is changed, with
/// the counting tool on offer.
fn asking(tree: &Tree, script: Script) -> Result<(Conversation, Arc<AtomicUsize>), Failed> {
    asking_on(tree, script, None)
}

/// [`asking`], with the provider the registry calls `serving` answering.
fn asking_on(
    tree: &Tree,
    script: Script,
    serving: Option<&'static str>,
) -> Result<(Conversation, Arc<AtomicUsize>), Failed> {
    asking_under(tree, script, serving, None)
}

/// [`asking_on`], of a model whose window holds `window` tokens, where it said.
fn asking_under(
    tree: &Tree,
    script: Script,
    serving: Option<&'static str>,
    window: Option<u32>,
) -> Result<(Conversation, Arc<AtomicUsize>), Failed> {
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools.add_builtin(Counting(Arc::clone(&ran)))?;
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");
    let conversation = Conversation::recording(session, serving, |session| {
        Runner::new(
            Box::new(script),
            tools,
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
        .permitting(Permission::with(crucible_tools::Mode::Ask, Rules::new()))
    });

    Ok((conversation, ran))
}

/// The one syntax theme the host in these tests reads code in.
const READ: &str = "a theme this host reads";

/// Whether the host in these tests reads code in `named`.
fn reads(named: &str) -> bool {
    named == READ
}

/// The release the host in these tests has had.
const RELEASED: &str = "0.1.0";

/// What the host in these tests answers for its release notes: no release
/// where none is named, with `RELEASED` as the version running, and a version
/// named said back as it arrived.
fn notes(version: Option<&str>) -> Result<NotesOutcome, Refusal> {
    Ok(match version {
        None => NotesOutcome::Listed {
            releases: Vec::new(),
            running: Name::new(RELEASED)?,
            truncated: false,
        },
        Some(named) => NotesOutcome::Unknown {
            newest: Name::new(named)?,
        },
    })
}

/// Requests numbered in the order they were made, each one read back off the
/// bytes it travels as.
#[derive(Default)]
struct Wire(u64);

impl Wire {
    fn sent(&mut self, command: Command) -> Result<Request, Failed> {
        self.0 += 1;
        let request = Request::new(Capabilities::every(), Correlation::new(self.0), command);
        let bytes = request.encode()?;

        Request::decode(&bytes).map_err(|refused| refused.refusal.into())
    }
}

/// A response as its reader would have it: written, and read back.
fn received(response: &Response) -> Result<Response, Failed> {
    Ok(Response::decode(&response.encode()?)?)
}

/// How a client answers the next thing put to it.
#[derive(Debug, Clone, Copy)]
enum Saying {
    /// A ruling on the action that is pending.
    Fitting(Ruling),
    /// A yes that names some other action.
    Elsewhere,
    /// A yes that names this action, whichever is pending.
    Naming(PendingId),
    /// Nothing, ever.
    Gone,
    /// About a warning: send anyway, or go back.
    Heeding(bool),
}

/// A client on the far side of a wire: it keeps what was put to it, and its
/// decisions reach the application only as a `decide` request's bytes.
struct Remote {
    script: std::vec::IntoIter<Saying>,
    wire: Wire,
    put: Vec<Pending>,
    refused: Vec<Refusal>,
}

impl Remote {
    fn new(script: Vec<Saying>) -> Self {
        Self {
            script: script.into_iter(),
            wire: Wire(1000),
            put: Vec::new(),
            refused: Vec::new(),
        }
    }
}

impl Front for Remote {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Decision>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            let decision = match self.script.next()? {
                Saying::Fitting(ruling) => ruled(pending.id(), ruling),
                Saying::Elsewhere => {
                    ruled(PendingId::new(pending.id().number() + 40), Ruling::Allow)
                }
                Saying::Naming(id) => ruled(id, Ruling::Allow),
                Saying::Gone => return None,
                Saying::Heeding(true) => Decision::Accepted { id: pending.id() },
                Saying::Heeding(false) => Decision::Declined { id: pending.id() },
            };
            let sent = self.wire.sent(Command::Decide(decision)).ok()?;

            match sent.command() {
                Command::Decide(decision) => Some(decision.clone()),
                _ => None,
            }
        })
    }

    fn refused(&mut self, refusal: Refusal) {
        self.refused.push(refusal);
    }
}

/// A ruling on `id`, for this call only.
const fn ruled(id: PendingId, ruling: Ruling) -> Decision {
    Decision::Ruled {
        id,
        ruling,
        lasting: Lasting::Once,
    }
}

/// Takes `request` as a turn, and hands back the response a client would read
/// beside the progress it would have been streamed.
fn turned(
    conversation: &mut Conversation,
    request: &Request,
    remote: &mut Remote,
) -> Result<(Response, Vec<Progress>), Failed> {
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let ended: Ended = {
        let run = conversation
            .runner()
            .starting(&events, &cancel, &steer, &aside);
        super::runtime()?.block_on(client::turn(
            conversation,
            request,
            Box::default(),
            remote,
            &run,
        ))
    };
    drop(events);

    let mut streamed = Vec::new();
    for envelope in reported.try_iter() {
        if let Some(progress) = client::progress(request.capabilities(), &envelope.into_event()) {
            streamed.push(Progress::decode(&progress.encode()?)?);
        }
    }

    Ok((received(&ended.response(request))?, streamed))
}

fn prompt(words: &str) -> Result<Command, Failed> {
    Ok(Command::Prompt(Prompt::new(words)?))
}

#[test]
fn a_prompt_sent_as_bytes_is_answered_and_its_progress_streams_apart_from_where_it_stands()
-> Result<(), Failed> {
    let tree = Tree::new("client-prompt")?;
    let mut conversation = super::conversation(&tree, Script::new(vec![saying("hello")]), false)?;
    let mut wire = Wire::default();
    let request = wire.sent(prompt("hi")?)?;

    let (response, streamed) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(response.correlation, Some(request.correlation()));
    assert_eq!(
        response.outcome,
        Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Yielded
        })
    );
    let said: String = streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said, "hello");
    assert!(
        matches!(streamed.last(), Some(Progress::Finished { .. })),
        "{streamed:?}"
    );

    let standing = client::snapshot(&conversation);
    assert_eq!(Snapshot::decode(&standing.encode()?)?, standing);
    assert_eq!(standing.turns, 1);
    assert_eq!(standing.pending, None);
    assert_eq!(
        standing.session.as_ref(),
        conversation.session().id(),
        "a snapshot names the session the application is recording into"
    );
    Ok(())
}

#[test]
fn a_yes_on_the_wire_runs_the_tool_once_and_a_no_never() -> Result<(), Failed> {
    for (ruling, runs) in [(Ruling::Allow, 1), (Ruling::Deny, 0)] {
        let tree = Tree::new("client-ruling")?;
        let script = Script::new(vec![calling(), saying("after")]);
        let (mut conversation, ran) = asking(&tree, script)?;
        let mut remote = Remote::new(vec![Saying::Fitting(ruling)]);
        let request = Wire::default().sent(prompt("change it")?)?;

        let (response, _) = turned(&mut conversation, &request, &mut remote)?;

        // A no ends the turn, as it does at the terminal; what a client is
        // told is the sentence, under the one code a failed turn has.
        match (ruling, &response.outcome) {
            (Ruling::Allow, Outcome::Turn(TurnOutcome::Ran { stop })) => {
                assert_eq!(*stop, Stop::Yielded);
            }
            (Ruling::Deny, Outcome::Turn(TurnOutcome::Failed(problem))) => {
                assert_eq!(problem.code, ErrorCode::Failed);
                assert_eq!(problem.message.as_str(), "write was not allowed");
            }
            other => return Err(format!("{other:?}").into()),
        }
        assert_eq!(ran.load(Ordering::Relaxed), runs, "{ruling:?}");
        assert!(
            matches!(
                remote.put.as_slice(),
                [Pending::Permission { tool, .. }] if tool.as_str() == "write"
            ),
            "{:?}",
            remote.put
        );
        assert!(remote.refused.is_empty(), "{:?}", remote.refused);
    }
    Ok(())
}

#[test]
fn a_yes_composed_for_one_turn_settles_nothing_in_the_next() -> Result<(), Failed> {
    let tree = Tree::new("client-two-turns")?;
    let script = Script::new(vec![calling(), saying("once"), calling(), saying("twice")]);
    let (mut conversation, ran) = asking(&tree, script)?;
    let mut wire = Wire::default();

    let mut first = Remote::new(vec![Saying::Fitting(Ruling::Allow)]);
    let request = wire.sent(prompt("change it")?)?;
    turned(&mut conversation, &request, &mut first)?;
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    let earlier = first
        .put
        .first()
        .map(Pending::id)
        .ok_or("nothing was put")?;

    // The yes that let turn one's call, word for word, and then nobody.
    let mut second = Remote::new(vec![Saying::Naming(earlier), Saying::Gone]);
    let request = wire.sent(prompt("and again")?)?;
    turned(&mut conversation, &request, &mut second)?;

    assert_eq!(
        ran.load(Ordering::Relaxed),
        1,
        "turn one's yes let turn two's call"
    );
    let later = second
        .put
        .first()
        .map(Pending::id)
        .ok_or("nothing was put")?;
    assert_ne!(earlier, later, "two turns showed one identity");
    assert_eq!(
        second
            .refused
            .iter()
            .map(|refusal| refusal.code())
            .collect::<Vec<_>>(),
        [ErrorCode::StaleDecision]
    );
    Ok(())
}

#[test]
fn a_yes_naming_another_action_never_runs_the_tool_however_it_is_worded() -> Result<(), Failed> {
    let tree = Tree::new("client-stale")?;
    let script = Script::new(vec![calling(), saying("after")]);
    let (mut conversation, ran) = asking(&tree, script)?;
    // A yes for an action that is not the pending one, and then nobody: the
    // pending call was never answered, so it was never let.
    let mut remote = Remote::new(vec![Saying::Elsewhere, Saying::Gone]);
    let request = Wire::default().sent(prompt("change it")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut remote)?;

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    assert_eq!(
        remote
            .refused
            .iter()
            .map(|refusal| refusal.code())
            .collect::<Vec<_>>(),
        [ErrorCode::StaleDecision]
    );
    let ids: Vec<_> = remote.put.iter().map(Pending::id).collect();
    assert_eq!(
        ids.first(),
        ids.get(1),
        "the action that stayed pending is the one put again: {ids:?}"
    );
    assert!(matches!(response.outcome, Outcome::Turn(_)), "{response:?}");
    Ok(())
}

#[test]
fn a_decision_sent_outside_a_turn_settles_nothing() -> Result<(), Failed> {
    let tree = Tree::new("client-outside")?;
    let (mut conversation, ran) = asking(&tree, Script::new(Vec::new()))?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let request = Wire::default().sent(Command::Decide(Decision::Ruled {
        id: PendingId::new(1),
        ruling: Ruling::Allow,
        lasting: Lasting::Session,
    }))?;

    let performed = super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));

    assert_eq!(
        received(&performed.response(&request))?.outcome,
        Outcome::Refused(ErrorCode::StaleDecision.into())
    );
    assert_eq!(ran.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn a_decision_on_its_own_is_answered_the_same_at_every_door() -> Result<(), Failed> {
    let tree = Tree::new("client-lone-decision")?;
    let (mut conversation, ran) = asking(&tree, Script::new(Vec::new()))?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let request = Wire::default().sent(Command::Decide(Decision::Ruled {
        id: PendingId::new(1),
        ruling: Ruling::Allow,
        lasting: Lasting::Session,
    }))?;
    let stale = Outcome::Refused(ErrorCode::StaleDecision.into());

    // Whichever door it is handed in at, no action is pending there for it to
    // be about, and a client is told that in one word rather than two.
    let performed = super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
    assert_eq!(received(&performed.response(&request))?.outcome, stale);

    let kept = client::keep(&request, &desk);
    assert_eq!(received(&kept.response(&request))?.outcome, stale);

    let mut remote = Remote::new(vec![Saying::Fitting(Ruling::Allow)]);
    let (response, _) = turned(&mut conversation, &request, &mut remote)?;
    assert_eq!(response.outcome, stale);
    assert!(remote.put.is_empty());

    let cancel = Cancel::new();
    assert_eq!(client::interrupt(&request, &cancel), stale);
    assert!(!cancel.requested());

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn a_syntax_theme_this_host_does_not_read_is_refused_and_not_written_down() -> Result<(), Failed> {
    let tree = Tree::new("client-syntax-theme")?;
    let mut conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let mut wire = Wire::default();
    let invalid = Outcome::Refused(ErrorCode::InvalidArgument.into());

    let unread = crucible_client_api::Name::new("no theme by this name")?;
    let request = wire.sent(Command::Theme(Theme::Syntax(unread)))?;
    let performed = super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
    assert_eq!(received(&performed.response(&request))?.outcome, invalid);
    let kept = client::keep(&request, &desk);
    assert_eq!(received(&kept.response(&request))?.outcome, invalid);
    assert_eq!(super::written(&tree)?.syntax_theme(), None);

    // The one it does read is written down, through either door.
    let request = wire.sent(Command::Theme(Theme::Syntax(
        crucible_client_api::Name::new(READ)?,
    )))?;
    let kept = client::keep(&request, &desk);
    assert_eq!(
        received(&kept.response(&request))?.outcome,
        Outcome::Theme(crucible_client_api::ThemeOutcome::Remembered)
    );
    assert_eq!(super::written(&tree)?.syntax_theme(), Some(READ));
    Ok(())
}

#[test]
fn the_shipped_commands_are_carried_out_from_bytes_and_answered_in_bytes() -> Result<(), Failed> {
    let tree = Tree::new("client-commands")?;
    let mut conversation = super::conversation(&tree, Script::new(vec![saying("one")]), false)?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let mut wire = Wire::default();
    let mut answered = |conversation: &mut Conversation, command: Command| {
        let request = wire.sent(command)?;
        let performed = super::runtime()?.block_on(client::perform(conversation, &request, &desk));
        let response = received(&performed.response(&request))?;
        assert_eq!(response.correlation, Some(request.correlation()));
        Ok::<_, Failed>((performed, response.outcome))
    };

    // Nothing said yet: there is nothing to clear.
    let (_, outcome) = answered(&mut conversation, Command::Clear)?;
    assert_eq!(outcome, Outcome::Cleared(ClearOutcome::Nothing));

    let first = conversation.session().id().cloned();
    let request = Wire(500).sent(prompt("hi")?)?;
    turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;
    let (_, outcome) = answered(&mut conversation, Command::Clear)?;
    assert!(
        matches!(outcome, Outcome::Cleared(ClearOutcome::Started { .. })),
        "{outcome:?}"
    );
    assert_eq!(client::snapshot(&conversation).turns, 0);

    let first = first.ok_or("the first session had no identity")?;
    let (_, outcome) = answered(&mut conversation, Command::Resume(first.clone()))?;
    assert!(
        matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{outcome:?}"
    );
    let resumed = client::snapshot(&conversation);
    assert_eq!(resumed.session, Some(first.clone()));
    assert_eq!(resumed.turns, 1);
    let (_, outcome) = answered(&mut conversation, Command::Resume(first))?;
    assert_eq!(outcome, Outcome::Resumed(ResumeOutcome::Same));

    let (_, outcome) = answered(&mut conversation, Command::SetMode(Mode::AllowEdits))?;
    assert_eq!(outcome, Outcome::Mode(Mode::AllowEdits));
    let (_, outcome) = answered(&mut conversation, Command::CycleMode)?;
    assert_eq!(outcome, Outcome::Mode(Mode::FullAccess));
    assert_eq!(client::snapshot(&conversation).mode, Mode::FullAccess);

    let (_, outcome) = answered(
        &mut conversation,
        Command::Theme(Theme::Drawing(Palette::Dark)),
    )?;
    assert_eq!(
        outcome,
        Outcome::Theme(crucible_client_api::ThemeOutcome::Remembered)
    );
    assert_eq!(
        super::written(&tree)?.theme(),
        Some(crucible_config::ThemeChoice::Dark)
    );

    let (_, outcome) = answered(&mut conversation, Command::Help)?;
    assert_eq!(outcome, Outcome::help());
    let (performed, outcome) = answered(&mut conversation, Command::Exit)?;
    assert!(matches!(performed, Performed::Leaving), "{performed:?}");
    assert_eq!(outcome, Outcome::Leaving);
    Ok(())
}

/// A conversation asking `anthropic`, one turn in, keeping its persistent
/// cache records under the tree's home: what a switch has a cache to retire
/// for.
fn cached(tree: &Tree) -> Result<Conversation, Failed> {
    let mut conversation = super::conversing(
        tree,
        Script::named("anthropic"),
        &super::Guard::Nothing,
        Some("anthropic"),
    )?
    .remembering_caches_in(tree.home()?.path());
    let request = Wire(700).sent(prompt("hi")?)?;
    turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    Ok(conversation)
}

fn haiku() -> Result<Command, Failed> {
    Ok(Command::SelectModel {
        provider: Name::new("anthropic")?,
        model: Name::new("claude-haiku-4-5")?,
        effort: None,
    })
}

#[test]
fn a_cache_that_cannot_be_retired_holds_the_model_where_it_was() -> Result<(), Failed> {
    let tree = Tree::new("client-cache-held")?;
    let mut conversation = cached(&tree)?;
    // Records nobody can read: whether one of them is this session's cannot
    // be told, so the cache cannot be said to have been retired.
    let kept = tree.home()?.path().join("prompt-cache");
    std::fs::create_dir_all(&kept)?;
    std::fs::write(kept.join("resources-v1.json"), "not records")?;
    let standing = Standing::new(&tree, &["anthropic"])?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };

    let request = Wire::default().sent(haiku()?)?;
    let response = received(
        &super::runtime()?
            .block_on(client::perform(&mut conversation, &request, &desk))
            .response(&request),
    )?;

    let Outcome::Model(ModelOutcome::CacheHeld(problem)) = &response.outcome else {
        return Err(format!("{:?}", response.outcome).into());
    };
    assert_eq!(problem.code, ErrorCode::Failed);
    assert_eq!(conversation.runner().model(), "script");
    assert_eq!(
        super::written(&tree)?.model("anthropic"),
        None,
        "a switch that did not happen is not what the next start reads"
    );
    Ok(())
}

#[test]
fn a_choice_that_could_not_be_written_down_is_still_taken_and_says_so() -> Result<(), Failed> {
    let tree = Tree::new("client-unwritten")?;
    let mut conversation = cached(&tree)?;
    let standing = Standing::new(&tree, &["anthropic"])?;
    // A settings file that is not configuration is never written over.
    let settings = crucible_config::user(&tree.home()?);
    std::fs::create_dir_all(settings.parent().ok_or("settings with no directory")?)?;
    std::fs::write(&settings, "model = [")?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };

    let request = Wire::default().sent(haiku()?)?;
    let response = received(
        &super::runtime()?
            .block_on(client::perform(&mut conversation, &request, &desk))
            .response(&request),
    )?;

    let Outcome::Model(ModelOutcome::Taken {
        unwritten: Some(problem),
        ..
    }) = &response.outcome
    else {
        return Err(format!("{:?}", response.outcome).into());
    };
    assert_eq!(problem.code, ErrorCode::Failed);
    assert_eq!(conversation.runner().model(), "claude-haiku-4-5");
    assert_eq!(
        std::fs::read_to_string(&settings)?,
        "model = [",
        "what the file said is left as it was"
    );
    Ok(())
}

#[test]
fn what_cannot_be_carried_out_is_refused_by_code_and_changes_nothing() -> Result<(), Failed> {
    let tree = Tree::new("client-refused")?;
    let mut conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let before = client::snapshot(&conversation);
    let nobody = crucible_client_api::Name::new("nobody-by-this-name")?;
    let mut wire = Wire::default();

    for (command, code) in [
        (prompt("hi")?, ErrorCode::Busy),
        (Command::Compact, ErrorCode::Busy),
        (Command::Cancel, ErrorCode::Busy),
        (
            Command::Login {
                provider: nobody.clone(),
            },
            ErrorCode::UnknownProvider,
        ),
        (
            Command::Logout { provider: nobody },
            ErrorCode::UnknownProvider,
        ),
    ] {
        let request = wire.sent(command)?;
        let performed =
            super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
        assert_eq!(
            received(&performed.response(&request))?.outcome,
            Outcome::Refused(code.into()),
            "{request:?}"
        );
    }

    assert_eq!(client::snapshot(&conversation), before);
    assert!(standing.reached().is_empty());
    Ok(())
}

#[test]
fn only_a_cancel_interrupts_and_it_does_so_through_the_turn_s_own_control() -> Result<(), Failed> {
    let mut wire = Wire::default();
    let cancel = Cancel::new();

    let outcome = client::interrupt(&wire.sent(Command::Help)?, &cancel);
    assert_eq!(outcome, Outcome::Refused(ErrorCode::Busy.into()));
    assert!(!cancel.requested());

    let outcome = client::interrupt(&wire.sent(Command::Cancel)?, &cancel);
    assert_eq!(outcome, Outcome::Cancelling);
    assert!(cancel.requested());
    Ok(())
}

#[test]
fn the_release_notes_are_what_the_host_answers_and_wait_for_the_turn() -> Result<(), Failed> {
    let tree = Tree::new("client-release-notes")?;
    let mut conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let mut wire = Wire::default();
    let before = client::snapshot(&conversation);

    for version in [None, Some("v0.41.1")] {
        let command = Command::ReleaseNotes {
            version: version.map(Name::new).transpose()?,
        };
        let request = wire.sent(command.clone())?;
        let performed =
            super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
        // The version reaches the host as the client wrote it, `v` and all.
        assert_eq!(
            received(&performed.response(&request))?.outcome,
            Outcome::Notes(notes(version)?)
        );

        // While a turn has the conversation, it is asked again afterwards.
        let request = wire.sent(command)?;
        let busy = Outcome::Refused(ErrorCode::Busy.into());
        assert_eq!(client::keep(&request, &desk).outcome(), busy);
        assert_eq!(client::interrupt(&request, &Cancel::new()), busy);
    }

    assert_eq!(client::snapshot(&conversation), before);
    assert!(standing.reached().is_empty());
    Ok(())
}

#[test]
fn every_palette_a_client_can_name_is_one_the_settings_file_reads_back() -> Result<(), Failed> {
    let tree = Tree::new("client-palettes")?;
    let mut conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let mut wire = Wire::default();
    let mut read = Vec::new();

    for palette in Palette::EVERY {
        let request = wire.sent(Command::Theme(Theme::Drawing(palette)))?;
        super::runtime()?.block_on(client::perform(&mut conversation, &request, &desk));
        let choice = super::written(&tree)?
            .theme()
            .ok_or_else(|| format!("{} is not a theme the settings read", palette.as_str()))?;
        if !read.contains(&choice) {
            read.push(choice);
        }
    }

    assert_eq!(read.len(), Palette::EVERY.len(), "{read:?}");
    Ok(())
}

/// A send on a route whose vendor uses what is sent, with no yes to it, is put
/// to the client as a pending action before anything is sent. A yes on the
/// wire is written into the user's own file and the turn goes; going back, or
/// nobody answering, ends it with the route, what the vendor says and the page,
/// and sends nothing.
#[test]
fn a_send_on_a_warned_route_is_put_to_the_client_and_only_a_yes_sends_it() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    for (said, sends) in [
        (Saying::Heeding(true), 1),
        (Saying::Heeding(false), 0),
        (Saying::Gone, 0),
    ] {
        let tree = Tree::new("client-warned")?;
        let script = Script::new(vec![saying("answered")]);
        let asked = Arc::clone(&script.asked);
        let (conversation, _) = asking_on(&tree, script, Some("google"))?;
        let file = tree.0.join("config.json");
        let consent = Consent::new(Routes::production());
        consent.keeps_in(file.clone());
        let mut conversation = conversation.consenting(consent.clone());
        consent.served(
            "google",
            Some(Serving {
                route: Some("key:google".to_owned()),
                at: None,
            }),
        );
        let mut remote = Remote::new(vec![said]);
        let request = Wire::default().sent(prompt("hello")?)?;

        let (response, _) = turned(&mut conversation, &request, &mut remote)?;

        assert_eq!(asked.load(Ordering::Relaxed), sends, "{said:?}");
        assert!(
            matches!(
                remote.put.as_slice(),
                [Pending::Warning { route, sentence, source, .. }]
                    if route.as_str() == "key:google"
                        && sentence.as_str().starts_with("On unpaid quota")
                        && source.as_str() == "Gemini API terms, 30 Sep 2026"
            ),
            "{:?}",
            remote.put
        );
        let written = std::fs::read_to_string(&file).unwrap_or_default();
        match (sends, &response.outcome) {
            (1, Outcome::Turn(TurnOutcome::Ran { .. })) => {
                assert!(written.contains("key:google"), "{written}");
            }
            (0, Outcome::Turn(TurnOutcome::Warned { route, .. })) => {
                assert_eq!(route.as_str(), "key:google");
                assert!(!written.contains("key:google"), "{written}");
            }
            other => return Err(format!("{said:?}: {other:?}").into()),
        }
    }
    Ok(())
}

/// Two routes with no yes hold the origin a send goes to: each is put to the
/// client in turn, and the send goes once, after both yeses.
#[test]
fn a_send_two_routes_hold_puts_each_before_it_goes() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    let tree = Tree::new("client-warned-two")?;
    let script = Script::new(vec![saying("answered")]);
    let asked = Arc::clone(&script.asked);
    let (conversation, _) = asking_on(&tree, script, Some("google"))?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(tree.0.join("config.json"));
    let mut conversation = conversation.consenting(consent.clone());
    consent.served(
        "google",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    // Another provider sent to the same origin on a route of its own.
    consent.served(
        "another",
        Some(Serving {
            route: Some("key:moonshot".to_owned()),
            at: crucible_http::Origin::of("https://generativelanguage.googleapis.com/v1beta"),
        }),
    );
    let mut remote = Remote::new(vec![Saying::Heeding(true), Saying::Heeding(true)]);
    let request = Wire::default().sent(prompt("hello")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut remote)?;

    assert_eq!(asked.load(Ordering::Relaxed), 1);
    let routes: Vec<&str> = remote
        .put
        .iter()
        .filter_map(|put| match put {
            Pending::Warning { route, .. } => Some(route.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(routes.len(), 2, "{:?}", remote.put);
    assert!(routes.contains(&"key:google") && routes.contains(&"key:moonshot"));
    assert!(
        matches!(&response.outcome, Outcome::Turn(TurnOutcome::Ran { .. })),
        "{:?}",
        response.outcome
    );
    Ok(())
}

/// A compaction asks the model too, so on a warned route it is put to the
/// client the same way, and going back sends nothing.
#[test]
fn a_compaction_on_a_warned_route_is_put_to_the_client_first() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    let tree = Tree::new("client-warned-compact")?;
    let script = Script::new(vec![saying("summary")]);
    let asked = Arc::clone(&script.asked);
    let (conversation, _) = asking_on(&tree, script, Some("google"))?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(tree.0.join("config.json"));
    let mut conversation = conversation.consenting(consent.clone());
    consent.served(
        "google",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    let mut remote = Remote::new(vec![Saying::Heeding(false)]);
    let request = Wire::default().sent(Command::Compact)?;

    let (response, _) = turned(&mut conversation, &request, &mut remote)?;

    assert_eq!(asked.load(Ordering::Relaxed), 0);
    assert!(
        matches!(
            remote.put.as_slice(),
            [Pending::Warning { route, .. }] if route.as_str() == "key:google"
        ),
        "{:?}",
        remote.put
    );
    assert!(
        matches!(
            &response.outcome,
            Outcome::Turn(TurnOutcome::Warned { route, .. }) if route.as_str() == "key:google"
        ),
        "{:?}",
        response.outcome
    );
    Ok(())
}

/// A yes on the wire that cannot be written down is not a going back: the turn
/// fails naming why, and nothing is sent.
#[test]
fn a_yes_that_cannot_be_written_down_fails_the_turn_and_says_why() -> Result<(), Failed> {
    use crucible_app::content_use::{Consent, Routes, Serving};

    let tree = Tree::new("client-warned-unwritten")?;
    let script = Script::new(vec![saying("answered")]);
    let asked = Arc::clone(&script.asked);
    let (conversation, _) = asking_on(&tree, script, Some("google"))?;
    // A file that is not configuration cannot have a yes written into it.
    let file = tree.0.join("config.json");
    std::fs::write(&file, "{ not configuration")?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(file.clone());
    consent.served(
        "google",
        Some(Serving {
            route: Some("key:google".to_owned()),
            at: None,
        }),
    );
    let mut conversation = conversation.consenting(consent);
    let mut remote = Remote::new(vec![Saying::Heeding(true)]);
    let request = Wire::default().sent(prompt("hello")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut remote)?;

    assert_eq!(asked.load(Ordering::Relaxed), 0);
    match &response.outcome {
        Outcome::Turn(TurnOutcome::Failed(problem)) => {
            assert!(
                problem.message.as_str().contains("config.json"),
                "{problem:?}"
            );
        }
        other => return Err(format!("{other:?}").into()),
    }
    Ok(())
}

/// A conversation recording into a session started in `tree`, asking
/// `provider` for `model` under the registry name `serving`.
fn unserved(
    tree: &Tree,
    provider: impl crucible_models::Provider + 'static,
    model: &str,
    serving: Option<&'static str>,
) -> Result<Conversation, Failed> {
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: model.into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");

    Ok(Conversation::recording(session, serving, |session| {
        Runner::new(
            Box::new(provider),
            Tools::new(),
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
    }))
}

/// How many prompts and answers the log of `conversation`'s session holds,
/// read back off the disk once the session has let it go.
fn recorded(tree: &Tree, conversation: Conversation) -> Result<usize, Failed> {
    let id = conversation
        .session()
        .id()
        .cloned()
        .ok_or("a started session has an id")?;
    drop(conversation);
    let (_, transcript) = Session::reopen(&tree.sessions(), &tree.workspace()?, &id)?;

    Ok(transcript
        .messages()
        .iter()
        .filter(|message| {
            matches!(
                message,
                crucible_types::Message::User { .. } | crucible_types::Message::Agent { .. }
            )
        })
        .count())
}

/// A provider chosen and no model of it: the prompt is answered with what is
/// missing, and nothing is sent or written down.
#[test]
fn a_prompt_with_no_model_chosen_is_answered_unasked_and_nothing_is_recorded() -> Result<(), Failed>
{
    let tree = Tree::new("client-unasked-model")?;
    let script = Script::new(vec![saying("never asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = unserved(&tree, script, "", Some("anthropic"))?;
    let request = Wire::default().sent(prompt("hello")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(response.outcome, Outcome::Unasked(Missing::Model));
    assert_eq!(asked.load(Ordering::Relaxed), 0, "nothing was sent");
    assert_eq!(recorded(&tree, conversation)?, 0, "nothing was recorded");
    Ok(())
}

/// A model named with nothing set up to serve it is the stand-in's to refuse,
/// and a client is told so before anything is written down.
#[test]
fn a_prompt_for_a_model_nothing_serves_is_answered_unasked_and_nothing_is_recorded()
-> Result<(), Failed> {
    let tree = Tree::new("client-unasked-stand-in")?;
    let mut conversation = unserved(
        &tree,
        crucible_provider::Unavailable::new(crucible_app::providers::NOTHING_TO_ASK),
        "foo",
        None,
    )?;
    let request = Wire::default().sent(prompt("hello")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(response.outcome, Outcome::Unasked(Missing::Credential));
    assert_eq!(recorded(&tree, conversation)?, 0, "nothing was recorded");
    Ok(())
}

/// Signed out with other providers reachable is a provider to choose, and
/// signed out with none is a credential to set up: the two sentences a
/// terminal says apart are two answers here too.
#[test]
fn a_prompt_after_signing_out_says_whether_a_provider_or_a_credential_is_missing()
-> Result<(), Failed> {
    for (reachable, missing) in [
        (&["google"][..], Missing::Provider),
        (&[][..], Missing::Credential),
    ] {
        let tree = Tree::new("client-unasked-signed-out")?;
        let desk = Standing::new(&tree, reachable)?;
        desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
        let mut conversation = unserved(&tree, Script::named("anthropic"), "m", Some("anthropic"))?;
        super::runtime()?.block_on(conversation.log_out(desk.one("anthropic")?, &desk.with()));
        let request = Wire::default().sent(prompt("hello")?)?;

        let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

        assert_eq!(response.outcome, Outcome::Unasked(missing), "{reachable:?}");
        assert_eq!(recorded(&tree, conversation)?, 0, "nothing was recorded");
    }
    Ok(())
}

/// What is missing follows the store after the session signed out: the last
/// key forgotten while nobody was being asked leaves a credential to set up,
/// as a terminal reading the store again says.
#[test]
fn a_prompt_after_the_last_key_is_forgotten_says_a_credential_is_missing() -> Result<(), Failed> {
    let tree = Tree::new("client-unasked-last-key")?;
    let desk = Standing::stored(&tree)?;
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
    desk.logins.keep("google", "a-key-no-vendor-issued")?;
    let mut conversation = unserved(&tree, Script::named("anthropic"), "m", Some("anthropic"))?;
    let runtime = super::runtime()?;

    let first = runtime.block_on(conversation.log_out(desk.one("anthropic")?, &desk.with()));
    assert!(matches!(first, LoggedOut::SignedOut { .. }), "{first:?}");
    assert_eq!(
        conversation.missing(),
        Some(crucible_app::providers::Missing::Provider)
    );
    let second = runtime.block_on(conversation.log_out(desk.one("google")?, &desk.with()));
    assert!(matches!(second, LoggedOut::Kept), "{second:?}");
    let request = Wire::default().sent(prompt("hello")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(response.outcome, Outcome::Unasked(Missing::Credential));
    assert_eq!(recorded(&tree, conversation)?, 0, "nothing was recorded");
    Ok(())
}

/// A key that cannot be used is still a change to the store, and what is
/// missing is read off the store as it is now: here another crucible took the
/// last usable key away before it was written.
#[test]
fn a_login_nothing_can_use_reads_again_what_is_missing() -> Result<(), Failed> {
    let tree = Tree::new("client-unasked-unusable")?;
    let desk = Standing::reaching(&tree, |one, stored| {
        one.name != "openai" && stored.has_key(one.name)
    })?;
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;
    desk.logins.keep("google", "a-key-no-vendor-issued")?;
    let mut conversation = unserved(&tree, Script::named("anthropic"), "m", Some("anthropic"))?;
    let runtime = super::runtime()?;
    runtime.block_on(conversation.log_out(desk.one("anthropic")?, &desk.with()));
    assert_eq!(
        conversation.missing(),
        Some(crucible_app::providers::Missing::Provider)
    );

    desk.logins.forget("google")?;
    desk.logins.keep("openai", "a-key-no-vendor-issued")?;
    let logged = runtime.block_on(conversation.logged_in(desk.one("openai")?, &desk.with()));

    assert!(matches!(logged, LoggedIn::Unusable(_)), "{logged:?}");
    assert_eq!(
        conversation.missing(),
        Some(crucible_app::providers::Missing::Credential)
    );
    Ok(())
}

/// Room is made by asking the model for a recap, so with no model to ask it
/// is answered the way a prompt is.
#[test]
fn a_request_for_room_with_no_model_to_ask_is_answered_unasked() -> Result<(), Failed> {
    let tree = Tree::new("client-unasked-compact")?;
    let script = Script::new(vec![saying("never asked")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = unserved(&tree, script, "", Some("anthropic"))?;
    let request = Wire::default().sent(Command::Compact)?;

    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(response.outcome, Outcome::Unasked(Missing::Model));
    assert_eq!(asked.load(Ordering::Relaxed), 0, "nothing was sent");
    assert_eq!(recorded(&tree, conversation)?, 0, "nothing was recorded");
    Ok(())
}

/// The guard is about a session nothing can answer, and no other: one with a
/// provider and a model takes the turn and writes it down.
#[test]
fn a_prompt_a_model_can_answer_still_takes_the_turn_and_is_recorded() -> Result<(), Failed> {
    let tree = Tree::new("client-answered-recorded")?;
    let script = Script::new(vec![saying("hello")]);
    let asked = Arc::clone(&script.asked);
    let mut conversation = unserved(&tree, script, "script", Some("anthropic"))?;
    let request = Wire::default().sent(prompt("hi")?)?;

    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;

    assert_eq!(
        response.outcome,
        Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Yielded
        })
    );
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert_eq!(
        recorded(&tree, conversation)?,
        2,
        "the prompt and its answer"
    );
    Ok(())
}

/// What `/context` is answered with, asked for over the wire.
fn context(conversation: &mut Conversation, tree: &Tree) -> Result<Outcome, Failed> {
    let standing = Standing::new(tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let request = Wire::default().sent(Command::Context)?;
    let performed = super::runtime()?.block_on(client::perform(conversation, &request, &desk));
    assert!(standing.reached().is_empty());

    Ok(received(&performed.response(&request))?.outcome)
}

#[test]
fn context_is_the_load_the_runner_counts_and_leaves_what_the_prompt_line_reads()
-> Result<(), Failed> {
    let tree = Tree::new("client-context")?;
    let script = Script::new(vec![saying("hello")]);
    let (mut conversation, _) = asking_under(&tree, script, None, Some(200_000))?;
    let request = Wire(500).sent(prompt("hi")?)?;
    turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;
    let before = client::snapshot(&conversation);

    let Outcome::Context(context) = context(&mut conversation, &tree)? else {
        return Err("/context was not answered with a context".into());
    };

    let runner = conversation.runner();
    assert_eq!(context.model, before.model);
    assert!(context.model.is_some(), "{context:?}");
    assert_eq!(context.window, Some(200_000));
    let held =
        context.system + context.instructions + context.tools + context.mcp + context.messages;
    assert_eq!(held, runner.carrying(), "{context:?}");
    assert!(context.tools > 0 && context.messages > 0, "{context:?}");
    assert_eq!(
        held + context.reserve + context.free,
        200_000,
        "{context:?}"
    );
    assert_eq!(context.left, before.left, "{context:?}");
    assert!(context.left.is_some(), "{context:?}");
    assert_eq!(client::snapshot(&conversation), before);
    Ok(())
}

#[test]
fn context_of_a_model_with_no_reported_window_leaves_the_window_and_free_room_out()
-> Result<(), Failed> {
    let tree = Tree::new("client-context-unknown")?;
    let (mut conversation, _) = asking(&tree, Script::new(Vec::new()))?;

    let Outcome::Context(context) = context(&mut conversation, &tree)? else {
        return Err("/context was not answered with a context".into());
    };

    assert_eq!(context.window, None, "{context:?}");
    assert_eq!(context.left, None, "{context:?}");
    assert_eq!(context.free, 0, "{context:?}");
    let held =
        context.system + context.instructions + context.tools + context.mcp + context.messages;
    assert_eq!(held, conversation.runner().carrying(), "{context:?}");
    Ok(())
}

#[test]
fn context_waits_for_the_turn_at_every_door_a_running_turn_leaves_open() -> Result<(), Failed> {
    let tree = Tree::new("client-context-busy")?;
    let standing = Standing::new(&tree, &[])?;
    let workspace = tree.workspace()?;
    let sessions = tree.sessions();
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let request = Wire::default().sent(Command::Context)?;
    let busy = Outcome::Refused(ErrorCode::Busy.into());

    assert_eq!(client::keep(&request, &desk).outcome(), busy);
    assert_eq!(client::interrupt(&request, &Cancel::new()), busy);
    let mut conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let (response, _) = turned(&mut conversation, &request, &mut Remote::new(Vec::new()))?;
    assert_eq!(response.outcome, busy);
    Ok(())
}
