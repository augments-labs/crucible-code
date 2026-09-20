//! What the checks on a definition do to a turn, and what two definitions
//! running beside each other can reach of one another.

use crucible_agents::{
    AgentBuilder, AgentContext, Availability, Decision, GuardrailError, InputGuardrail,
    OutputGuardrail, Undecided,
};

use super::*;

/// A check that answers the same way every time and remembers what it saw.
///
/// Implemented outside the crate the traits live in, which is the point: a
/// check is something a caller writes, and everything it is given to decide
/// with arrives on the context it is handed.
#[derive(Debug)]
struct Check {
    name: &'static str,
    answer: Answer,
    saw: Arc<Mutex<Vec<Saw>>>,
}

/// What a check was given, one entry per time it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Saw {
    agent: String,
    said: String,
}

/// What a check is scripted to make of it.
#[derive(Debug, Clone, Copy)]
enum Answer {
    Allow,
    Refuse(&'static str),
    Cannot(&'static str),
}

impl Check {
    fn new(name: &'static str, answer: Answer) -> (Arc<Self>, Arc<Mutex<Vec<Saw>>>) {
        let saw = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(Self {
                name,
                answer,
                saw: Arc::clone(&saw),
            }),
            saw,
        )
    }

    fn answering(&self, context: &AgentContext<'_>, said: &str) -> Result<Decision, Undecided> {
        self.saw.lock().unwrap().push(Saw {
            agent: context.agent().as_str().to_owned(),
            said: said.to_owned(),
        });
        match self.answer {
            Answer::Allow => Ok(Decision::Allowed),
            Answer::Refuse(why) => Ok(Decision::rejected(why)),
            Answer::Cannot(why) => Err(Undecided::because(why)),
        }
    }
}

impl InputGuardrail for Check {
    fn name(&self) -> &str {
        self.name
    }

    fn checking(&self, context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        self.answering(context, context.said())
    }
}

impl OutputGuardrail for Check {
    fn name(&self) -> &str {
        self.name
    }

    fn checking(&self, context: &AgentContext<'_>, candidate: &str) -> Result<Decision, Undecided> {
        self.answering(context, candidate)
    }
}

/// A definition called `id`, with whatever the test wants said about it.
pub(super) fn agent(id: &str) -> AgentBuilder {
    AgentBuilder::new(
        AgentId::new(id),
        Model {
            name: "claude-test".into(),
            max_tokens: 1024,
            window: None,
            accepts: Some(READS),
            effort: None,
        },
    )
}

/// The tool names one request advertised.
pub(super) fn offered(request: &crate::fake::SentRequest) -> Vec<String> {
    request
        .tools
        .iter()
        .map(|schema| schema.name.to_string())
        .collect()
}

/// One response that says `text` and yields.
pub(super) fn answering(text: &str) -> Vec<Delta> {
    vec![
        Delta::Text(text.into()),
        Delta::Stopped(StopReason::Yielded),
    ]
}

#[test]
fn a_prompt_an_input_check_refuses_never_reaches_the_provider() {
    // The whole point of an input check: the words are not sent. A refusal
    // that arrived after the request would be a report about something that
    // had already happened.
    let script = Script::new(vec![answering("answered anyway")]);
    let sent = script.sent();
    let (check, saw) = Check::new("no-secrets", Answer::Refuse("the prompt carries a key"));
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted
        .turned("here is my key")
        .expect("a refusal is not a failure");

    assert!(
        matches!(&turned, Turned::Rejected { rejection, stop: None } if rejection.guard() == "no-secrets"),
        "a refused prompt came back as something else: {turned:?}"
    );
    assert!(
        sent.lock().unwrap().is_empty(),
        "a prompt a check refused was sent to the provider anyway"
    );
    assert!(
        conversation(scripted.runner.transcript()).is_empty(),
        "a prompt a check refused was written to the transcript"
    );
    assert_eq!(
        saw.lock().unwrap().as_slice(),
        &[Saw {
            agent: "test".into(),
            said: "here is my key".into()
        }],
        "the check was not given the agent and the words it was asked about"
    );
}

#[test]
fn a_turn_refused_on_the_way_in_posts_neither_a_start_nor_an_ending() {
    // Nothing ran, so there is nothing on screen to explain. The pair of
    // events a cancelled turn posts is for a turn the reader had already seen
    // begin; this one never did.
    let (check, _saw) = Check::new("no-secrets", Answer::Refuse("no"));
    let mut scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    drop(scripted.turned("go").expect("a refusal is not a failure"));

    assert!(
        !scripted.seen.try_iter().any(|event| matches!(
            event,
            Event::TurnStarted { .. } | Event::TurnFinished { .. }
        )),
        "a turn nothing ran announced itself starting or finishing"
    );
}

#[test]
fn a_prompt_every_check_allows_runs_the_turn_as_though_none_were_declared() {
    let script = Script::new(vec![answering("done")]);
    let sent = script.sent();
    let (first, seen_first) = Check::new("one", Answer::Allow);
    let (second, seen_second) = Check::new("two", Answer::Allow);
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_input(first)
            .expect("a name no other check has")
            .checking_input(second)
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted.turned("go").expect("the turn to finish");

    assert_eq!(turned.stop(), Some(StopReason::Yielded));
    assert_eq!(sent.lock().unwrap().len(), 1, "the request never went out");
    assert_eq!(seen_first.lock().unwrap().len(), 1);
    assert_eq!(
        seen_second.lock().unwrap().len(),
        1,
        "a check after the first one was never asked"
    );
}

#[test]
fn a_session_with_no_checks_declared_neither_asks_nor_remembers_anything() {
    // The empty default, which is every session that ships today. Nothing is
    // copied and nothing is committed, so a session that declared no check
    // runs exactly as it did before there were any.
    let script = Script::new(vec![answering("done")]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);

    let turned = scripted.turned("go").expect("the turn to finish");

    assert_eq!(turned.stop(), Some(StopReason::Yielded));
    assert!(
        scripted.runner.state.checked.is_none(),
        "a session with no checks declared committed a decision anyway"
    );
}

#[test]
fn a_check_that_could_not_decide_stops_the_turn_without_refusing_it() {
    // Not a refusal. A scanner that could not be reached has said nothing
    // about the prompt, and reporting that as "the prompt was rejected" would
    // put words in its mouth that a reader would act on.
    let script = Script::new(vec![answering("unused")]);
    let sent = script.sent();
    let (check, _saw) = Check::new("no-secrets", Answer::Cannot("the scanner was unreachable"));
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted
        .turned("go")
        .expect("an undecided check is not a failure");

    assert!(
        matches!(&turned, Turned::Undecided { stop: None, .. }),
        "a check that could not decide came back as something else: {turned:?}"
    );
    assert!(
        sent.lock().unwrap().is_empty(),
        "a prompt no check had decided about was sent anyway"
    );
    assert!(
        scripted.runner.state.checked.is_none(),
        "a check that reached no decision committed one"
    );
}

#[test]
fn the_same_words_after_a_turn_that_failed_keep_the_decision_that_was_committed() {
    // The invocation is still the one that was checked: it did not finish, and
    // the caller is trying the same words again. The decision was committed
    // once, and a check that has since changed its mind must not be able to
    // overturn an invocation already under way.
    let (check, saw) = Check::new("one", Answer::Allow);
    let mut scripted = Scripted::under(
        Script::failing(),
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    drop(
        scripted
            .turned("go")
            .expect_err("a provider that refuses everything"),
    );
    drop(
        scripted
            .turned("go")
            .expect_err("a provider that refuses everything"),
    );

    assert_eq!(
        saw.lock().unwrap().len(),
        1,
        "an invocation already under way was put to the checks a second time"
    );
}

#[test]
fn the_same_words_after_a_completed_turn_are_a_new_invocation() {
    // A turn that finished ended the invocation the decision was committed
    // for. Typing the same words again is a new one, and a check exists to be
    // asked about it — including a check whose answer depends on something
    // that changed in between.
    let script = Script::new(vec![answering("first"), answering("second")]);
    let (check, saw) = Check::new("one", Answer::Allow);
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    drop(scripted.turned("go").expect("the first turn"));
    drop(scripted.turned("go").expect("the second turn"));

    let saw = saw.lock().unwrap();
    assert_eq!(
        saw.len(),
        2,
        "words typed again after the turn they were checked for had finished reused its decision"
    );
    assert_eq!(saw.get(1).expect("a second check").said, "go");
}

#[test]
fn a_session_picked_up_puts_the_same_words_to_the_checks_again() {
    // A committed decision belongs to the invocation it was reached for, and
    // it is forgotten where what the last session allowed is forgotten. The
    // words are the same words, but the session they are said to is not.
    let (check, saw) = Check::new("one", Answer::Allow);
    let mut scripted = Scripted::under(
        Script::failing(),
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    drop(
        scripted
            .turned("go")
            .expect_err("a provider that refuses everything"),
    );
    scripted
        .runner
        .pick_up(Recording::nowhere(), Transcript::new());
    drop(
        scripted
            .turned("go")
            .expect_err("a provider that refuses everything"),
    );

    assert_eq!(
        saw.lock().unwrap().len(),
        2,
        "a decision committed in the session just left behind answered for the one picked up"
    );
}

#[test]
fn different_words_are_a_new_invocation_and_get_their_own_checks() {
    let script = Script::new(vec![answering("first"), answering("second")]);
    let (check, saw) = Check::new("one", Answer::Allow);
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );

    drop(scripted.turned("go").expect("the first turn"));
    drop(scripted.turned("and now this").expect("the second turn"));

    let saw = saw.lock().unwrap();
    assert_eq!(saw.len(), 2, "a different prompt reused the last decision");
    assert_eq!(saw.get(1).expect("a second check").said, "and now this");
}

#[test]
fn a_turn_the_reader_stopped_is_never_put_to_the_checks() {
    // The flag was already up when the turn arrived, so there is no
    // invocation to check — and a check with an effect of its own would have
    // had one anyway.
    let (check, saw) = Check::new("one", Answer::Refuse("no"));
    let mut scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        Tools::new(),
        agent("test")
            .checking_input(check)
            .expect("a name no other check has")
            .build(),
    );
    scripted.cancel.request();

    let turned = scripted
        .turned("go")
        .expect("a stopped turn is not a failure");

    assert_eq!(turned.stop(), Some(StopReason::Cancelled));
    assert!(
        saw.lock().unwrap().is_empty(),
        "a turn nobody was going to take was put to the checks"
    );
}

#[test]
fn an_answer_an_output_check_refuses_is_neither_recorded_nor_accepted() {
    // The candidate is judged before it is accepted, so a refused answer
    // leaves no trace in the transcript for the next request to carry. What
    // streamed is provisional and the reader has already seen it; what is
    // written down is not.
    let script = Script::new(vec![answering("the competitor is better")]);
    let (check, saw) = Check::new(
        "house-style",
        Answer::Refuse("the answer names a competitor"),
    );
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_output(check)
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted.turned("go").expect("a refusal is not a failure");

    assert!(
        matches!(
            &turned,
            Turned::Rejected { rejection, stop: Some(StopReason::Yielded) }
                if rejection.guard() == "house-style"
        ),
        "a refused answer came back as something else: {turned:?}"
    );
    assert!(
        !conversation(scripted.runner.transcript())
            .iter()
            .any(|message| matches!(message, Message::Agent { .. })),
        "an answer a check refused was written to the transcript"
    );
    assert_eq!(
        saw.lock().unwrap().first().map(|saw| saw.said.clone()),
        Some("the competitor is better".to_owned()),
        "the check was not given the candidate answer"
    );
}

#[test]
fn an_answer_no_output_check_refuses_is_recorded_and_the_turn_ends_as_it_did() {
    let script = Script::new(vec![answering("done")]);
    let (check, saw) = Check::new("house-style", Answer::Allow);
    let mut scripted = Scripted::under(
        script,
        Tools::new(),
        agent("test")
            .checking_output(check)
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted.turned("go").expect("the turn to finish");

    assert_eq!(turned.stop(), Some(StopReason::Yielded));
    assert_eq!(saw.lock().unwrap().len(), 1);
    assert!(
        conversation(scripted.runner.transcript())
            .iter()
            .any(|message| matches!(message, Message::Agent { .. })),
        "an answer every check allowed was left out of the transcript"
    );
}

#[test]
fn an_agent_offered_only_some_tools_advertises_only_those() {
    // A definition declares what it may reach. Narrowing happens against the
    // exact generation the pass admitted, so the roster the request carries
    // and the roster a call is admitted through cannot come apart.
    let script = Script::new(vec![answering("done")]);
    let sent = script.sent();
    let mut scripted = Scripted::under(
        script,
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("test")
            .offering(Availability::Named(Box::new(["read".into()])))
            .build(),
    );

    drop(scripted.turned("go").expect("the turn to finish"));

    let sent = sent.lock().unwrap();
    let asked = sent.first().expect("the turn's request");
    assert_eq!(
        offered(asked),
        vec!["read".to_owned()],
        "an agent was offered a tool its definition never declared"
    );
}

#[test]
fn an_agent_offered_only_some_tools_says_so_before_it_has_taken_a_turn() {
    // What the row under the box reads, in a session where nobody has typed
    // anything yet. Naming a tool there that the definition never declared
    // would promise the reader something the first turn then takes away.
    let scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("test")
            .offering(Availability::Named(Box::new(["read".into()])))
            .build(),
    );

    assert_eq!(
        scripted.runner.offering(),
        vec!["read".to_owned()],
        "a session advertised a tool its definition never declared"
    );
}

#[test]
fn an_agent_offered_only_some_tools_is_counted_as_carrying_only_those() {
    // What the next request would carry is read before anybody has typed
    // anything: it is the figure under the box, and the one a session picked up
    // is asked about. A definition declaring one tool out of two sends one
    // schema, so it is counted exactly as a session wired with that one tool
    // is, and not as though the whole roster were going out.
    let narrowed = Scripted::under(
        Script::new(vec![answering("unused")]),
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("test")
            .offering(Availability::Named(Box::new(["read".into()])))
            .build(),
    );
    let wired_with_one = Scripted::under(
        Script::new(vec![answering("unused")]),
        tools([Fixed::new("read")]),
        agent("test").build(),
    );

    assert_eq!(
        narrowed.runner.carrying(),
        wired_with_one.runner.carrying(),
        "a session was counted as carrying tools its definition never declared"
    );
}

#[test]
fn two_agents_run_beside_each_other_under_their_own_definitions() {
    // Two runs, two definitions, one process. Nothing either does reaches the
    // other: not the instructions each is asked under, not the model each is
    // aimed at, and not the roster each is offered.
    let one = Scripted::under(
        Script::new(vec![answering("from one")]),
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("one")
            .telling("You are one.")
            .offering(Availability::Named(Box::new(["read".into()])))
            .build(),
    );
    let two = Scripted::under(
        Script::new(vec![answering("from two")]),
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("two").telling("You are two.").build(),
    );
    let (mut one, mut two) = (one, two);
    let (first, second) = (one.sent.clone(), two.sent.clone());

    drop(one.turned("go").expect("the first agent's turn"));
    two.runner.ask("other", 2048, None, Some(READS));
    drop(two.turned("go").expect("the second agent's turn"));

    let first = first.lock().unwrap();
    let first = first.first().expect("the first agent's request");
    let second = second.lock().unwrap();
    let second = second.first().expect("the second agent's request");

    assert_eq!(&*first.model, "claude-test");
    assert_eq!(&*second.model, "other", "the second agent was not re-aimed");
    assert_eq!(one.runner.instructions(), Some("You are one."));
    assert_eq!(
        two.runner.instructions(),
        Some("You are two."),
        "one agent's instructions reached the other"
    );
    assert_eq!(offered(first), vec!["read".to_owned()]);
    assert_eq!(
        offered(second),
        vec!["read".to_owned(), "write".to_owned()],
        "one agent's declared toolset narrowed the other's"
    );
}

#[test]
fn re_aiming_a_session_leaves_the_definition_a_sibling_holds_alone() {
    // The same definition value handed to two sessions. Changing model on one
    // builds another and selects it; the sibling goes on asking the model it
    // opened under, because nothing wrote to the value they shared.
    let shared = agent("shared").telling("mind the workspace").build();
    let mut one = Scripted::under(
        Script::new(vec![answering("from one")]),
        Tools::new(),
        shared.clone(),
    );
    let mut two = Scripted::under(
        Script::new(vec![answering("from two")]),
        Tools::new(),
        shared,
    );

    one.runner.ask("other", 4096, Some(1_000_000), Some(READS));
    one.runner.telling("answer only in French");
    one.runner.think(Effort::Low);

    assert_eq!(two.runner.model(), "claude-test");
    assert_eq!(two.runner.effort(), None);
    assert_eq!(two.runner.instructions(), Some("mind the workspace"));
    assert_eq!(two.runner.context_window(), None);

    drop(two.turned("go").expect("the sibling's turn"));

    let sent = two.sent.lock().unwrap();
    let asked = sent.first().expect("the sibling's request");
    assert_eq!(
        &*asked.model, "claude-test",
        "re-aiming one session re-aimed the request its sibling sent"
    );
}

/// A tool that reports it has started and then waits to be let go.
///
/// The one place a turn stops of its own accord for as long as a test needs it
/// to: while a call is out. Holding a turn there is how the test below gets two
/// runs genuinely overlapping rather than merely interleaved on one thread.
pub(super) struct Gate {
    /// What it answers to, which is what the definition below declares.
    name: &'static str,
    /// Told once the call is out, so the test knows the turn is under way.
    started: Sender<()>,
    /// Waited on until the test has done what it wanted done mid-turn.
    go: Mutex<Receiver<()>>,
}

impl Gate {
    pub(super) fn new(started: Sender<()>, go: Receiver<()>) -> Self {
        Self {
            name: "gate",
            started,
            go: Mutex::new(go),
        }
    }
}

impl DescribeTool for Gate {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Gate {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("waiting to be let go")
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.started
            .send(())
            .expect("the test to be waiting for this call");
        self.go
            .lock()
            .unwrap()
            .recv()
            .expect("the test to let this call go");
        Ok(ToolOutput::ok("let go"))
    }
}

#[test]
fn two_definitions_running_at_once_keep_their_own_state_and_cancellation() {
    // The claim the split of definition from run state exists for, made where
    // it can actually fail: two runs at the same time in one process. One is
    // held open inside a tool call and stopped there; the other has to finish
    // anyway, under its own instructions, its own model and its own roster,
    // having been checked by its own guard and carrying none of the other's
    // words.
    let (started, mid_turn) = channel();
    let (release, go) = channel();
    let mut held = Tools::new();
    held.add_builtin(Gate::new(started, go)).unwrap();
    held.add_builtin(Fixed::new("read")).unwrap();

    let (watching_one, saw_one) = Check::new("one", Answer::Allow);
    let (watching_two, saw_two) = Check::new("two", Answer::Allow);
    let mut one = Scripted::under(
        Script::new(vec![
            calling("c1", "gate", "{}"),
            answering("one got there"),
        ]),
        held,
        agent("one")
            .telling("You are one.")
            .offering(Availability::Named(Box::new(["gate".into()])))
            .checking_input(watching_one)
            .expect("a name no other check has")
            .build(),
    );
    let mut two = Scripted::under(
        Script::new(vec![answering("from two")]),
        tools([Fixed::new("read"), Fixed::new("write")]),
        agent("two")
            .telling("You are two.")
            .checking_input(watching_two)
            .expect("a name no other check has")
            .build(),
    );
    two.runner.ask("other", 2048, None, Some(READS));
    let (first, second) = (one.sent.clone(), two.sent.clone());
    let stopping = one.cancel.clone();

    let (one, stopped) = thread::scope(|scope| {
        let running = scope.spawn(move || {
            let stopped = one.turned("one's words");
            (one, stopped)
        });

        mid_turn
            .recv()
            .expect("the first run to reach its tool call");
        let carried_on = two
            .turned("two's words")
            .expect("the second run to finish while the first one is held open");
        assert_eq!(
            carried_on.stop(),
            Some(StopReason::Yielded),
            "a run beside one that was stopped did not finish"
        );

        stopping.request();
        release.send(()).expect("the first run to still be waiting");
        running.join().expect("the first run's thread")
    });
    let stopped = stopped.expect("a stopped turn is not a failure");

    assert_eq!(
        stopped.stop(),
        Some(StopReason::Cancelled),
        "the run the test stopped ended some other way"
    );
    assert_eq!(
        saw_one.lock().unwrap().as_slice(),
        &[Saw {
            agent: "one".into(),
            said: "one's words".into()
        }],
        "one run's guard was asked about the other's words"
    );
    assert_eq!(
        saw_two.lock().unwrap().as_slice(),
        &[Saw {
            agent: "two".into(),
            said: "two's words".into()
        }],
        "one run's guard was asked about the other's words"
    );

    let first = first.lock().unwrap();
    let first = first.first().expect("the first run's request");
    let second = second.lock().unwrap();
    let second = second.first().expect("the second run's request");
    assert_eq!(&*first.model, "claude-test");
    assert_eq!(&*second.model, "other", "one run's model reached the other");
    assert_eq!(offered(first), vec!["gate".to_owned()]);
    assert_eq!(
        offered(second),
        vec!["read".to_owned(), "write".to_owned()],
        "one run's declared toolset narrowed the other's"
    );
    assert_eq!(one.runner.instructions(), Some("You are one."));
    assert_eq!(
        two.runner.instructions(),
        Some("You are two."),
        "one run's instructions reached the other"
    );
    assert!(
        !conversation(two.runner.transcript()).iter().any(
            |message| matches!(message, Message::User { text, .. } if &**text == "one's words")
        ),
        "one run's transcript holds what was said to the other"
    );
    assert!(
        !conversation(one.runner.transcript()).iter().any(
            |message| matches!(message, Message::User { text, .. } if &**text == "two's words")
        ),
        "one run's transcript holds what was said to the other"
    );
}

#[test]
fn a_committed_decision_never_shows_the_words_it_was_reached_about() {
    // The words are the reader's prompt, kept so that a retry can tell the same
    // invocation from another. The transcript redacts them once they are a
    // message, and a run's state written into a log line does the same.
    let mut state = RunState::new(None);
    state.commit("asked-debug-canary", Judged::Allowed);

    let shown = format!("{state:?}");
    assert!(!shown.contains("asked-debug-canary"), "{shown}");
    assert!(shown.contains("asked: \"[redacted]\""), "{shown}");
}

/// A check that refuses, and has only a reason to say so with.
#[derive(Debug)]
struct Borrowing;

impl InputGuardrail for Borrowing {
    fn name(&self) -> &'static str {
        "borrowing"
    }

    fn checking(&self, _context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        Ok(Decision::rejected("no-secrets says no"))
    }
}

#[test]
fn a_refusal_names_the_check_that_made_it_whatever_the_check_says() {
    // A check answers with a reason and nothing else, so the name a reader is
    // shown is the one the runner read off the check it asked.
    let mut scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        Tools::new(),
        agent("test")
            .checking_input(Arc::new(Borrowing))
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted.turned("go").expect("a refusal is not a failure");

    assert!(
        matches!(&turned, Turned::Rejected { rejection, .. }
            if rejection.guard() == "borrowing" && rejection.why() == "no-secrets says no"),
        "a refusal was put under a name the check that made it does not have: {turned:?}"
    );
}

/// A check that could not decide, and has only a reason to say so with.
#[derive(Debug)]
struct Shrugging;

impl InputGuardrail for Shrugging {
    fn name(&self) -> &'static str {
        "shrugging"
    }

    fn checking(&self, _context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        Err(Undecided::because("no-secrets is away"))
    }
}

#[test]
fn a_check_that_could_not_decide_is_named_by_who_asked_it_whatever_it_says() {
    let mut scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        Tools::new(),
        agent("test")
            .checking_input(Arc::new(Shrugging))
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted
        .turned("go")
        .expect("an undecided check is not a failure");

    assert!(
        matches!(&turned, Turned::Undecided { problem: GuardrailError::Undecided { guard, problem }, .. }
            if &**guard == "shrugging" && &**problem == "no-secrets is away"),
        "a check that could not decide was put under a name it does not have: {turned:?}"
    );
}

/// A check that starts answering to another check's name once it has been
/// asked.
#[derive(Debug, Default)]
struct Turncoat {
    asked: std::sync::atomic::AtomicBool,
}

impl InputGuardrail for Turncoat {
    fn name(&self) -> &str {
        if self.asked.load(std::sync::atomic::Ordering::SeqCst) {
            "no-secrets"
        } else {
            "turncoat"
        }
    }

    fn checking(&self, _context: &AgentContext<'_>) -> Result<Decision, Undecided> {
        self.asked.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Decision::rejected("no"))
    }
}

#[test]
fn a_refusal_is_written_under_the_name_its_check_was_declared_with() {
    // The name is taken once, when the check is declared. One that answers to
    // another's name after it has been asked is still written under its own.
    let mut scripted = Scripted::under(
        Script::new(vec![answering("unused")]),
        Tools::new(),
        agent("test")
            .checking_input(Arc::new(Turncoat::default()))
            .expect("a name no other check has")
            .build(),
    );

    let turned = scripted.turned("go").expect("a refusal is not a failure");

    assert!(
        matches!(&turned, Turned::Rejected { rejection, .. } if rejection.guard() == "turncoat"),
        "a check put its refusal under a name it was not declared with: {turned:?}"
    );
}
