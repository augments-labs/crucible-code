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
use crucible_client_api::{
    Capabilities, ClearOutcome, Command, Correlation, Decision, ErrorCode, Lasting, Mode, Outcome,
    Palette, Pending, PendingId, Progress, Prompt, Refusal, Request, Response, ResumeOutcome,
    Ruling, Snapshot, Stop, Theme, TurnOutcome,
};
use crucible_models::Delta;
use crucible_runner::{EventEnvelope, Runner, Tools};
use crucible_runtime::{Aside, Cancel, Steer};
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

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ToolOutput::ok("done"))
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
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools.add_builtin(Counting(Arc::clone(&ran)))?;
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");
    let conversation = Conversation::recording(session, None, |session| {
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
    fn put(&mut self, pending: &Pending, _shown: Shown<'_>) -> Option<Decision> {
        self.put.push(pending.clone());
        let (id, ruling) = match self.script.next()? {
            Saying::Fitting(ruling) => (pending.id(), ruling),
            Saying::Elsewhere => (PendingId::new(pending.id().number() + 40), Ruling::Allow),
            Saying::Naming(id) => (id, Ruling::Allow),
            Saying::Gone => return None,
        };
        let sent = self
            .wire
            .sent(Command::Decide(Decision::Ruled {
                id,
                ruling,
                lasting: Lasting::Once,
            }))
            .ok()?;

        match sent.command() {
            Command::Decide(decision) => Some(decision.clone()),
            _ => None,
        }
    }

    fn refused(&mut self, refusal: Refusal) {
        self.refused.push(refusal);
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
        client::turn(conversation, request, Box::default(), remote, &run)
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
    };
    let request = Wire::default().sent(Command::Decide(Decision::Ruled {
        id: PendingId::new(1),
        ruling: Ruling::Allow,
        lasting: Lasting::Session,
    }))?;

    let performed = client::perform(&mut conversation, &request, &desk);

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
    };
    let request = Wire::default().sent(Command::Decide(Decision::Ruled {
        id: PendingId::new(1),
        ruling: Ruling::Allow,
        lasting: Lasting::Session,
    }))?;
    let stale = Outcome::Refused(ErrorCode::StaleDecision.into());

    // Whichever door it is handed in at, no action is pending there for it to
    // be about, and a client is told that in one word rather than two.
    let performed = client::perform(&mut conversation, &request, &desk);
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
    };
    let mut wire = Wire::default();
    let invalid = Outcome::Refused(ErrorCode::InvalidArgument.into());

    let unread = crucible_client_api::Name::new("no theme by this name")?;
    let request = wire.sent(Command::Theme(Theme::Syntax(unread)))?;
    let performed = client::perform(&mut conversation, &request, &desk);
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
    };
    let mut wire = Wire::default();
    let mut answered = |conversation: &mut Conversation, command: Command| {
        let request = wire.sent(command)?;
        let performed = client::perform(conversation, &request, &desk);
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
        let performed = client::perform(&mut conversation, &request, &desk);
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
    };
    let mut wire = Wire::default();
    let mut read = Vec::new();

    for palette in Palette::EVERY {
        let request = wire.sent(Command::Theme(Theme::Drawing(palette)))?;
        client::perform(&mut conversation, &request, &desk);
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
