//! What the contract refuses, and what every value in it may carry.
//!
//! The second half walks the whole public set rather than one example of it:
//! [`specimens`] holds a value for every arm of every enum that crosses, and
//! the `*_arm` functions beside it match each enum exhaustively, so an arm
//! added without a specimen either fails to compile or fails
//! [`every_arm_that_crosses_has_a_specimen`].

use std::collections::BTreeSet;

use crucible_types::SessionId;
use serde_json::{Value, json};

use crate::bounds::{
    DEPTH, FRAME_BYTES, ITEMS, NAME_BYTES, PROMPT_BYTES, SAID_BYTES, TEXT_BYTES, VALUES,
};
use crate::*;

/// Words no `Debug` of any value here may show.
const MARKER: &str = "hunter2-marker";

/// Every field name a frame may hold.
///
/// An allow-list, so a field added to any value is a field somebody read here.
const KEYS: [&str; 71] = [
    "ambiguous",
    "answers",
    "asks",
    "cache",
    "call",
    "capabilities",
    "carrying",
    "choices",
    "chosen",
    "cleaned",
    "cleared",
    "code",
    "command",
    "commands",
    "correlation",
    "decision",
    "deleted",
    "effect",
    "effort",
    "enabled",
    "exclusive",
    "expires_at",
    "failed",
    "guard",
    "heading",
    "id",
    "inspected",
    "isolation",
    "kind",
    "lasting",
    "left",
    "login",
    "logout",
    "message",
    "messages",
    "mode",
    "model",
    "name",
    "note",
    "orphaned",
    "outcome",
    "part",
    "pending",
    "problem",
    "progress",
    "protocol",
    "provider",
    "questions",
    "replaced",
    "resources",
    "resumed",
    "room",
    "ruling",
    "sandbox",
    "says",
    "session",
    "several",
    "snapshot",
    "standing",
    "state",
    "stop",
    "subject",
    "summary",
    "text",
    "theme",
    "tokens",
    "tool",
    "truncated",
    "turn",
    "turns",
    "unclosed",
];

/// Further field names, kept apart so neither list outgrows a screen.
const MORE_KEYS: [&str; 4] = ["unwritten", "variable", "version", "why"];

/// What a field name may not say, whatever else it says.
///
/// A field called any of these is a field holding authority: a secret, a place
/// on the host's disk, or a handle to something the host owns.
const FORBIDDEN: [&str; 12] = [
    "secret",
    "credential",
    "password",
    "key",
    "bearer",
    "authorization",
    "path",
    "dir",
    "home",
    "handle",
    "workspace",
    "argument",
];

fn marked() -> Text {
    Text::cut(MARKER)
}

fn problem() -> Problem {
    Problem {
        code: ErrorCode::Failed,
        message: marked(),
    }
}

fn name(word: &str) -> Name {
    Name::new(word).unwrap()
}

fn retained() -> Retained {
    Retained {
        ambiguous: 1,
        orphaned: 2,
    }
}

fn permission() -> Pending {
    Pending::Permission {
        id: PendingId::new(7),
        tool: marked(),
        effect: Effect::SpawnsProcess,
        subject: marked(),
    }
}

fn questions() -> Pending {
    Pending::Questions {
        id: PendingId::new(8),
        questions: vec![Asked {
            heading: marked(),
            asks: marked(),
            several: true,
            choices: vec![Choice {
                name: marked(),
                says: marked(),
            }],
        }],
    }
}

fn decisions() -> Vec<Decision> {
    vec![
        Decision::Ruled {
            id: PendingId::new(7),
            ruling: Ruling::Allow,
            lasting: Lasting::Session,
        },
        Decision::Ruled {
            id: PendingId::new(7),
            ruling: Ruling::Deny,
            lasting: Lasting::Once,
        },
        Decision::Answered {
            id: PendingId::new(8),
            answers: vec![Picked {
                chosen: vec![Said::new(MARKER).unwrap()],
                note: Said::new(MARKER).unwrap(),
            }],
        },
        Decision::Declined {
            id: PendingId::new(8),
        },
    ]
}

fn commands() -> Vec<Command> {
    let mut commands = vec![
        Command::Prompt(Prompt::new(MARKER).unwrap()),
        Command::Compact,
        Command::Cancel,
        Command::Clear,
        Command::Resume(SessionId::new()),
        Command::SelectModel {
            provider: name("anthropic"),
            model: name("claude-sonnet-5"),
            effort: Some(Rung::High),
        },
        Command::SelectModel {
            provider: name("anthropic"),
            model: name("claude-sonnet-5"),
            effort: None,
        },
        Command::CycleMode,
        Command::Login {
            provider: name("anthropic"),
        },
        Command::Logout {
            provider: name("anthropic"),
        },
        Command::InspectCache,
        Command::CleanCache,
        Command::Sandbox { enabled: true },
        Command::Sandbox { enabled: false },
        Command::Theme(Theme::Drawing(Palette::Dark)),
        Command::Theme(Theme::Syntax(name("base16"))),
        Command::Help,
        Command::Exit,
    ];
    commands.extend(decisions().into_iter().map(Command::Decide));
    commands.extend(Rung::EVERY.into_iter().map(Command::SetEffort));
    commands.extend(Mode::EVERY.into_iter().map(Command::SetMode));
    commands
}

fn turn_outcomes() -> Vec<TurnOutcome> {
    let mut turns = vec![
        TurnOutcome::Rejected {
            guard: marked(),
            why: marked(),
            stop: Some(Stop::Yielded),
        },
        TurnOutcome::Rejected {
            guard: marked(),
            why: marked(),
            stop: None,
        },
        TurnOutcome::Undecided {
            problem: problem(),
            stop: None,
        },
        TurnOutcome::Undecided {
            problem: problem(),
            stop: Some(Stop::Cancelled),
        },
        TurnOutcome::Failed(problem()),
    ];
    turns.extend(
        Stop::EVERY
            .into_iter()
            .map(|stop| TurnOutcome::Ran { stop }),
    );
    turns
}

fn login_outcomes() -> Vec<Outcome> {
    vec![
        Outcome::Login(LoginOutcome::Unusable(problem())),
        Outcome::Login(LoginOutcome::Elsewhere),
        Outcome::Login(LoginOutcome::CacheHeld(problem())),
        Outcome::Login(LoginOutcome::Serving {
            retained: retained(),
            unwritten: Some(problem()),
        }),
        Outcome::Login(LoginOutcome::Serving {
            retained: retained(),
            unwritten: None,
        }),
        Outcome::Logout(LogoutOutcome::CacheHeld(problem())),
        Outcome::Logout(LogoutOutcome::Unforgotten {
            retained: retained(),
            problem: problem(),
        }),
        Outcome::Logout(LogoutOutcome::Kept),
        Outcome::Logout(LogoutOutcome::StillServed {
            retained: retained(),
            standing: Standing::Environment(marked()),
        }),
        Outcome::Logout(LogoutOutcome::StillServed {
            retained: retained(),
            standing: Standing::StoredKey,
        }),
        Outcome::Logout(LogoutOutcome::StillServed {
            retained: retained(),
            standing: Standing::Subscription,
        }),
        Outcome::Logout(LogoutOutcome::SignedOut {
            retained: retained(),
        }),
    ]
}

fn outcomes() -> Vec<Outcome> {
    let mut outcomes = vec![
        Outcome::Room(RoomOutcome::Made { replaced: 12 }),
        Outcome::Room(RoomOutcome::Nothing),
        Outcome::Room(RoomOutcome::Stopped),
        Outcome::Room(RoomOutcome::Failed(problem())),
        Outcome::Cancelling,
        Outcome::Cleared(ClearOutcome::Nothing),
        Outcome::Cleared(ClearOutcome::Started {
            unclosed: Some(problem()),
        }),
        Outcome::Cleared(ClearOutcome::Started { unclosed: None }),
        Outcome::Cleared(ClearOutcome::Failed(problem())),
        Outcome::Resumed(ResumeOutcome::Same),
        Outcome::Resumed(ResumeOutcome::Picked { unclosed: None }),
        Outcome::Resumed(ResumeOutcome::Picked {
            unclosed: Some(problem()),
        }),
        Outcome::Resumed(ResumeOutcome::Unknown),
        Outcome::Resumed(ResumeOutcome::Failed(problem())),
        Outcome::Model(ModelOutcome::Unsupported),
        Outcome::Model(ModelOutcome::Unreachable(problem())),
        Outcome::Model(ModelOutcome::CacheHeld(problem())),
        Outcome::Model(ModelOutcome::Taken {
            retained: retained(),
            unwritten: None,
        }),
        Outcome::Model(ModelOutcome::Taken {
            retained: retained(),
            unwritten: Some(problem()),
        }),
        Outcome::Effort(EffortOutcome::Unasked),
        Outcome::Effort(EffortOutcome::Unsupported),
        Outcome::Effort(EffortOutcome::Taken {
            unwritten: Some(problem()),
        }),
        Outcome::Effort(EffortOutcome::Taken { unwritten: None }),
        Outcome::Cache(CacheOutcome::Listed {
            resources: vec![
                Resource {
                    state: marked(),
                    expires_at: Some(1_900_000_000),
                    isolation: marked(),
                    exclusive: true,
                    protocol: marked(),
                },
                Resource {
                    state: marked(),
                    expires_at: None,
                    isolation: marked(),
                    exclusive: false,
                    protocol: marked(),
                },
            ],
            truncated: false,
        }),
        Outcome::Cache(CacheOutcome::Failed(problem())),
        Outcome::Cleaned(CleanOutcome::Counted {
            inspected: 3,
            deleted: 1,
            ambiguous: 1,
            orphaned: 1,
        }),
        Outcome::Cleaned(CleanOutcome::Failed(problem())),
        Outcome::Sandbox(SandboxOutcome::Set { enabled: false }),
        Outcome::Sandbox(SandboxOutcome::Unchanged(problem())),
        Outcome::Theme(ThemeOutcome::Remembered),
        Outcome::Theme(ThemeOutcome::Unwritten(problem())),
        Outcome::help(),
        Outcome::Leaving,
    ];
    outcomes.extend(login_outcomes());
    outcomes.extend(turn_outcomes().into_iter().map(Outcome::Turn));
    outcomes.extend(Mode::EVERY.into_iter().map(Outcome::Mode));
    outcomes.extend(
        ErrorCode::EVERY
            .into_iter()
            .map(|code| Outcome::Refused(code.into())),
    );
    outcomes
}

/// Every outcome as the answer to a request, and one as the answer to a frame
/// that never said which request it was.
fn responses() -> Vec<Response> {
    let mut responses: Vec<Response> = outcomes()
        .into_iter()
        .map(|outcome| Response {
            correlation: Some(Correlation::new(3)),
            outcome,
        })
        .collect();
    responses.push(Response {
        correlation: None,
        outcome: Outcome::Refused(ErrorCode::Malformed.into()),
    });
    responses
}

fn progress() -> Vec<Progress> {
    vec![
        Progress::Started { turn: 1 },
        Progress::Delta { text: marked() },
        Progress::ToolRequested {
            call: marked(),
            tool: marked(),
            summary: marked(),
        },
        Progress::ToolFinished {
            call: marked(),
            failed: true,
        },
        Progress::Retrying,
        Progress::Compacting { part: 2 },
        Progress::Compacted { replaced: 9 },
        Progress::Spent { tokens: 1234 },
        Progress::Finished {
            turn: 1,
            stop: Stop::Cancelled,
        },
        Progress::Failed(problem()),
    ]
}

fn snapshots() -> Vec<Snapshot> {
    [Some(permission()), Some(questions()), None]
        .into_iter()
        .map(|pending| Snapshot {
            session: pending.as_ref().map(|_| SessionId::new()),
            provider: pending.as_ref().map(|_| name("anthropic")),
            model: marked(),
            effort: pending.as_ref().map(|_| Rung::Max),
            mode: Mode::AllowEdits,
            messages: 4,
            turns: 2,
            carrying: 900,
            left: pending.as_ref().map(|_| 71),
            pending,
        })
        .collect()
}

/// One value of the public set, as it crossed and as it prints.
struct Specimen {
    what: String,
    frame: Vec<u8>,
    debug: String,
}

/// Every value that crosses, each already proven to read back as itself.
fn specimens() -> Vec<Specimen> {
    let mut all = Vec::new();

    for (number, command) in commands().into_iter().enumerate() {
        let request = Request::new(
            Capabilities::every(),
            Correlation::new(number as u64),
            command,
        );
        let frame = request.encode().unwrap();
        assert_eq!(Request::decode(&frame).unwrap(), request);
        all.push(Specimen {
            what: format!("request {}", request.command().kind()),
            debug: format!("{request:?}"),
            frame,
        });
    }

    for response in responses() {
        let frame = response.encode().unwrap();
        assert_eq!(Response::decode(&frame).unwrap(), response);
        all.push(Specimen {
            what: format!("response {}", response.outcome.kind()),
            debug: format!("{response:?}"),
            frame,
        });
    }

    for one in progress() {
        let frame = one.encode().unwrap();
        assert_eq!(Progress::decode(&frame).unwrap(), one);
        all.push(Specimen {
            what: format!("progress {}", one.kind()),
            debug: format!("{one:?}"),
            frame,
        });
    }

    for snapshot in snapshots() {
        let frame = snapshot.encode().unwrap();
        assert_eq!(Snapshot::decode(&frame).unwrap(), snapshot);
        all.push(Specimen {
            what: "snapshot".to_owned(),
            debug: format!("{snapshot:?}"),
            frame,
        });
    }

    all
}

/// Every field name and every string in `value`, wherever it is nested.
fn walk(value: &Value, keys: &mut BTreeSet<String>, strings: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                keys.insert(key.clone());
                walk(inner, keys, strings);
            }
        }
        Value::Array(items) => items.iter().for_each(|inner| walk(inner, keys, strings)),
        Value::String(text) => strings.push(text.clone()),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

// The arms of every enum that crosses. Each match is exhaustive, so an arm
// added to the contract stops this file compiling until it is counted here.

const fn decision_arm(one: &Decision) -> (usize, usize) {
    match one {
        Decision::Ruled {
            ruling: Ruling::Allow,
            ..
        } => (0, 4),
        Decision::Ruled {
            ruling: Ruling::Deny,
            ..
        } => (1, 4),
        Decision::Answered { .. } => (2, 4),
        Decision::Declined { .. } => (3, 4),
    }
}

const fn pending_arm(one: &Pending) -> (usize, usize) {
    match one {
        Pending::Permission { .. } => (0, 2),
        Pending::Questions { .. } => (1, 2),
    }
}

/// An optional field counts twice: once with something in it, once without.
const fn turn_arm(one: &TurnOutcome) -> (usize, usize) {
    match one {
        TurnOutcome::Ran { .. } => (0, 6),
        TurnOutcome::Rejected { stop: Some(_), .. } => (1, 6),
        TurnOutcome::Rejected { stop: None, .. } => (2, 6),
        TurnOutcome::Undecided { stop: Some(_), .. } => (3, 6),
        TurnOutcome::Undecided { stop: None, .. } => (4, 6),
        TurnOutcome::Failed(_) => (5, 6),
    }
}

const fn command_arm(one: &Command) -> (usize, usize) {
    match one {
        Command::Prompt(_) => (0, 21),
        Command::Compact => (1, 21),
        Command::Cancel => (2, 21),
        Command::Decide(_) => (3, 21),
        Command::Clear => (4, 21),
        Command::Resume(_) => (5, 21),
        Command::SelectModel {
            effort: Some(_), ..
        } => (6, 21),
        Command::SelectModel { effort: None, .. } => (7, 21),
        Command::SetEffort(_) => (8, 21),
        Command::SetMode(_) => (9, 21),
        Command::CycleMode => (10, 21),
        Command::Login { .. } => (11, 21),
        Command::Logout { .. } => (12, 21),
        Command::InspectCache => (13, 21),
        Command::CleanCache => (14, 21),
        Command::Sandbox { enabled: true } => (15, 21),
        Command::Sandbox { enabled: false } => (16, 21),
        Command::Theme(Theme::Drawing(_)) => (17, 21),
        Command::Theme(Theme::Syntax(_)) => (18, 21),
        Command::Help => (19, 21),
        Command::Exit => (20, 21),
    }
}

const fn logout_arm(one: &LogoutOutcome) -> (usize, usize) {
    match one {
        LogoutOutcome::CacheHeld(_) => (0, 7),
        LogoutOutcome::Unforgotten { .. } => (1, 7),
        LogoutOutcome::Kept => (2, 7),
        LogoutOutcome::StillServed {
            standing: Standing::Environment(_),
            ..
        } => (3, 7),
        LogoutOutcome::StillServed {
            standing: Standing::StoredKey,
            ..
        } => (4, 7),
        LogoutOutcome::StillServed {
            standing: Standing::Subscription,
            ..
        } => (5, 7),
        LogoutOutcome::SignedOut { .. } => (6, 7),
    }
}

/// Which arm of its own enum an outcome's payload is, and of how many.
const fn inner_arm(one: &Outcome) -> (usize, usize) {
    match one {
        Outcome::Refused(_)
        | Outcome::Cancelling
        | Outcome::Mode(_)
        | Outcome::Help(_)
        | Outcome::Leaving => (0, 1),
        Outcome::Turn(turn) => turn_arm(turn),
        Outcome::Room(room) => match room {
            RoomOutcome::Made { .. } => (0, 4),
            RoomOutcome::Nothing => (1, 4),
            RoomOutcome::Stopped => (2, 4),
            RoomOutcome::Failed(_) => (3, 4),
        },
        Outcome::Cleared(cleared) => match cleared {
            ClearOutcome::Nothing => (0, 4),
            ClearOutcome::Started { unclosed: Some(_) } => (1, 4),
            ClearOutcome::Started { unclosed: None } => (2, 4),
            ClearOutcome::Failed(_) => (3, 4),
        },
        Outcome::Resumed(resumed) => match resumed {
            ResumeOutcome::Same => (0, 5),
            ResumeOutcome::Picked { unclosed: Some(_) } => (1, 5),
            ResumeOutcome::Picked { unclosed: None } => (2, 5),
            ResumeOutcome::Unknown => (3, 5),
            ResumeOutcome::Failed(_) => (4, 5),
        },
        Outcome::Model(model) => match model {
            ModelOutcome::Unsupported => (0, 5),
            ModelOutcome::Unreachable(_) => (1, 5),
            ModelOutcome::CacheHeld(_) => (2, 5),
            ModelOutcome::Taken {
                unwritten: Some(_), ..
            } => (3, 5),
            ModelOutcome::Taken {
                unwritten: None, ..
            } => (4, 5),
        },
        Outcome::Effort(effort) => match effort {
            EffortOutcome::Unasked => (0, 4),
            EffortOutcome::Unsupported => (1, 4),
            EffortOutcome::Taken { unwritten: Some(_) } => (2, 4),
            EffortOutcome::Taken { unwritten: None } => (3, 4),
        },
        Outcome::Login(login) => match login {
            LoginOutcome::Unusable(_) => (0, 5),
            LoginOutcome::Elsewhere => (1, 5),
            LoginOutcome::CacheHeld(_) => (2, 5),
            LoginOutcome::Serving {
                unwritten: Some(_), ..
            } => (3, 5),
            LoginOutcome::Serving {
                unwritten: None, ..
            } => (4, 5),
        },
        Outcome::Logout(logout) => logout_arm(logout),
        Outcome::Cache(cache) => match cache {
            CacheOutcome::Listed { .. } => (0, 2),
            CacheOutcome::Failed(_) => (1, 2),
        },
        Outcome::Cleaned(cleaned) => match cleaned {
            CleanOutcome::Counted { .. } => (0, 2),
            CleanOutcome::Failed(_) => (1, 2),
        },
        Outcome::Sandbox(sandbox) => match sandbox {
            SandboxOutcome::Set { .. } => (0, 2),
            SandboxOutcome::Unchanged(_) => (1, 2),
        },
        Outcome::Theme(theme) => match theme {
            ThemeOutcome::Remembered => (0, 2),
            ThemeOutcome::Unwritten(_) => (1, 2),
        },
    }
}

/// Fails unless `seen` holds every arm `0..of` for each name in it.
fn whole(seen: &BTreeSet<(String, usize, usize)>) {
    for (what, _, of) in seen {
        for arm in 0..*of {
            assert!(
                seen.contains(&(what.clone(), arm, *of)),
                "{what} has no specimen for arm {arm} of {of}"
            );
        }
    }
}

#[test]
fn every_arm_that_crosses_has_a_specimen() {
    let kinds = |words: Vec<&'static str>| words.into_iter().collect::<BTreeSet<_>>();

    assert_eq!(
        kinds(commands().iter().map(Command::kind).collect()),
        kinds(Command::KINDS.to_vec())
    );
    assert_eq!(
        kinds(outcomes().iter().map(Outcome::kind).collect()),
        kinds(Outcome::KINDS.to_vec())
    );
    assert_eq!(
        kinds(progress().iter().map(Progress::kind).collect()),
        kinds(Progress::KINDS.to_vec())
    );

    let mut seen = BTreeSet::new();
    let mut either = |what: &str, there: bool| {
        seen.insert((what.to_owned(), usize::from(there), 2));
    };
    for response in responses() {
        either("response.correlation", response.correlation.is_some());
        if let Outcome::Cache(CacheOutcome::Listed { resources, .. }) = &response.outcome {
            for resource in resources {
                either("resource.expires_at", resource.expires_at.is_some());
            }
        }
    }
    for snapshot in snapshots() {
        either("snapshot.session", snapshot.session.is_some());
        either("snapshot.provider", snapshot.provider.is_some());
        either("snapshot.effort", snapshot.effort.is_some());
        either("snapshot.left", snapshot.left.is_some());
        either("snapshot.pending", snapshot.pending.is_some());
    }
    // Named here, so that a field no specimen sets either way is missed too.
    for what in [
        "response.correlation",
        "resource.expires_at",
        "snapshot.session",
        "snapshot.provider",
        "snapshot.effort",
        "snapshot.left",
        "snapshot.pending",
    ] {
        for (arm, form) in [(0, "without"), (1, "with")] {
            assert!(
                seen.contains(&(what.to_owned(), arm, 2)),
                "no specimen {form} {what}"
            );
        }
    }

    for outcome in outcomes() {
        let (arm, of) = inner_arm(&outcome);
        seen.insert((outcome.kind().to_owned(), arm, of));
    }
    for command in commands() {
        let (arm, of) = command_arm(&command);
        seen.insert(("command".to_owned(), arm, of));
    }
    for decision in decisions() {
        let (arm, of) = decision_arm(&decision);
        seen.insert(("decision".to_owned(), arm, of));
    }
    for snapshot in snapshots() {
        if let Some(pending) = &snapshot.pending {
            let (arm, of) = pending_arm(pending);
            seen.insert(("pending".to_owned(), arm, of));
        }
    }
    whole(&seen);
}

#[test]
fn no_value_that_crosses_names_a_field_for_a_secret_a_path_or_a_handle() {
    let allowed: BTreeSet<&str> = KEYS.into_iter().chain(MORE_KEYS).collect();
    for word in &allowed {
        for stem in FORBIDDEN {
            assert!(!word.contains(stem), "the allowed field {word} says {stem}");
        }
    }

    let mut used = BTreeSet::new();
    for specimen in specimens() {
        let value: Value = serde_json::from_slice(&specimen.frame).unwrap();
        let mut keys = BTreeSet::new();
        walk(&value, &mut keys, &mut Vec::new());
        for key in keys {
            assert!(
                allowed.contains(key.as_str()),
                "{} carries the field {key}, which nobody allowed",
                specimen.what
            );
            used.insert(key);
        }
    }

    // Two-way, so a field that stops crossing leaves the list as well.
    for word in allowed {
        assert!(used.contains(word), "no value carries the field {word}");
    }
}

#[test]
fn no_value_that_crosses_prints_the_words_it_carries() {
    for specimen in specimens() {
        assert!(
            !specimen.debug.contains(MARKER),
            "{} prints what it carries: {}",
            specimen.what,
            specimen.debug
        );
    }
}

#[test]
fn every_value_that_crosses_fits_its_ceilings() {
    for specimen in specimens() {
        assert!(specimen.frame.len() <= FRAME_BYTES, "{}", specimen.what);

        let value: Value = serde_json::from_slice(&specimen.frame).unwrap();
        let mut strings = Vec::new();
        walk(&value, &mut BTreeSet::new(), &mut strings);
        for text in strings {
            assert!(text.len() <= TEXT_BYTES, "{}", specimen.what);
        }
    }
}

#[test]
fn words_over_the_ceiling_are_cut_and_say_so() {
    let long = "é".repeat(TEXT_BYTES);
    let cut = Text::cut(&long);

    assert!(cut.truncated());
    assert!(cut.as_str().len() <= TEXT_BYTES);
    assert!(long.starts_with(cut.as_str()));
    assert!(!Text::cut("short").truncated());

    let progress = Progress::Delta { text: cut };
    let frame = progress.encode().unwrap();
    assert_eq!(Progress::decode(&frame).unwrap(), progress);
}

#[test]
fn what_a_person_answered_travels_whole_or_is_refused_and_is_never_cut() {
    // Longer than words for a reader may be, and well within what a person can
    // type: every byte of it has to come out the other side.
    let pasted = "é".repeat(TEXT_BYTES);
    assert!(pasted.len() > TEXT_BYTES);
    let decision = Decision::Answered {
        id: PendingId::new(8),
        answers: vec![Picked {
            chosen: vec![Said::new(&pasted).unwrap()],
            note: Said::new(&pasted).unwrap(),
        }],
    };
    let request = Request::new(
        Capabilities::every(),
        Correlation::new(41),
        Command::Decide(decision),
    );
    let back = Request::decode(&request.encode().unwrap()).unwrap();
    assert_eq!(back, request);
    let Command::Decide(Decision::Answered { answers, .. }) = back.command() else {
        panic!("a decision went in");
    };
    let answer = answers.first().unwrap();
    assert_eq!(answer.chosen.first().map(Said::as_str), Some(&*pasted));
    assert_eq!(answer.note.as_str(), pasted);

    assert!(Said::new(&"s".repeat(SAID_BYTES)).is_ok());
    assert_eq!(
        Said::new(&"s".repeat(SAID_BYTES + 1)).unwrap_err().code(),
        ErrorCode::TooLarge
    );
    assert!(Said::new("").is_ok(), "a note nobody wrote");
    assert!(!format!("{:?}", Said::new(MARKER).unwrap()).contains(MARKER));
}

/// A request frame made by hand, so a test can say anything in one.
fn framed(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn asking(command: &Value) -> Value {
    json!({"version": 1, "capabilities": [], "correlation": 41, "command": command})
}

/// `frame` with `field` saying `value` instead.
fn with(mut frame: Value, field: &str, value: Value) -> Value {
    frame
        .as_object_mut()
        .expect("a frame is an object")
        .insert(field.to_owned(), value);
    frame
}

fn refused(bytes: &[u8]) -> (Option<u64>, ErrorCode) {
    let refused = Request::decode(bytes).unwrap_err();
    (
        refused.correlation.map(Correlation::number),
        refused.refusal.code(),
    )
}

#[test]
fn a_version_this_build_does_not_speak_is_refused_by_name() {
    for version in [0, 2, 65_535, 65_536, u64::MAX] {
        let frame = with(asking(&json!({"kind": "help"})), "version", json!(version));
        assert_eq!(
            refused(&framed(&frame)),
            (Some(41), ErrorCode::UnsupportedVersion),
            "version {version}"
        );
    }

    // Before anything else in the frame is believed: the command here is one
    // this build has never heard of, and the version is still what is said.
    let frame = with(asking(&json!({"kind": "teleport"})), "version", json!(9));
    assert_eq!(
        refused(&framed(&frame)),
        (Some(41), ErrorCode::UnsupportedVersion)
    );
}

#[test]
fn a_capability_this_build_does_not_know_is_refused_by_name() {
    for claimed in [
        json!(["telepathy"]),
        json!(["progress", "root"]),
        json!([7]),
    ] {
        let frame = with(asking(&json!({"kind": "help"})), "capabilities", claimed);
        assert_eq!(
            refused(&framed(&frame)),
            (Some(41), ErrorCode::UnknownCapability)
        );
    }

    let frame = with(
        asking(&json!({"kind": "help"})),
        "capabilities",
        json!(["permissions", "progress"]),
    );
    let request = Request::decode(&framed(&frame)).unwrap();
    assert!(request.capabilities().has(Capability::Permissions));
    assert!(request.capabilities().has(Capability::Progress));
    assert!(!request.capabilities().has(Capability::Questions));
}

#[test]
fn a_command_that_does_not_ship_is_refused_by_name() {
    for kind in ["teleport", "modes", "models", "sessions", "shell", ""] {
        assert_eq!(
            refused(&framed(&asking(&json!({"kind": kind})))),
            (Some(41), ErrorCode::UnknownCommand),
            "{kind}"
        );
    }
}

#[test]
fn a_frame_that_is_not_one_whole_request_is_malformed() {
    let broken: Vec<(Vec<u8>, Option<u64>)> = vec![
        (b"".to_vec(), None),
        (b"not json".to_vec(), None),
        (b"[1, 2]".to_vec(), None),
        (b"{\"version\": 1".to_vec(), None),
        (framed(&json!({"version": 1})), None),
        (
            framed(&json!({"version": 1, "capabilities": [], "correlation": -1, "command": {}})),
            None,
        ),
        (
            framed(
                &json!({"version": "1", "capabilities": [], "correlation": 41,
                "command": {"kind": "help"}}),
            ),
            Some(41),
        ),
        (framed(&asking(&json!("help"))), Some(41)),
        (framed(&asking(&json!({"kind": 7}))), Some(41)),
        (framed(&asking(&json!({"kind": "prompt"}))), Some(41)),
        (
            framed(&asking(&json!({"kind": "prompt", "text": 7}))),
            Some(41),
        ),
        // A field nobody asked for, at either depth.
        (
            framed(&asking(&json!({"kind": "help", "also": true}))),
            Some(41),
        ),
        (
            framed(&json!({"version": 1, "capabilities": [], "correlation": 41,
                "command": {"kind": "help"}, "also": true})),
            Some(41),
        ),
    ];

    for (frame, correlation) in broken {
        assert_eq!(
            refused(&frame),
            (correlation, ErrorCode::Malformed),
            "{}",
            String::from_utf8_lossy(&frame)
        );
    }
}

#[test]
fn an_argument_outside_its_closed_set_is_invalid() {
    let invalid = [
        json!({"kind": "set_mode", "mode": "root"}),
        json!({"kind": "set_effort", "effort": "ludicrous"}),
        json!({"kind": "prompt", "text": ""}),
        json!({"kind": "login", "provider": ""}),
        json!({"kind": "login", "provider": "a\nb"}),
        json!({"kind": "resume", "session": "../../etc/passwd"}),
        json!({"kind": "theme", "part": "sound", "name": "dark"}),
        json!({"kind": "theme", "part": "drawing", "name": "sepia"}),
        json!({"kind": "decide", "decision": {"kind": "approved", "id": 7}}),
        json!({"kind": "decide", "decision":
            {"kind": "ruled", "id": 7, "ruling": "always", "lasting": "once"}}),
        json!({"kind": "decide", "decision":
            {"kind": "ruled", "id": 7, "ruling": "allow", "lasting": "always"}}),
    ];

    for command in invalid {
        assert_eq!(
            refused(&framed(&asking(&command))),
            (Some(41), ErrorCode::InvalidArgument),
            "{command}"
        );
    }
}

#[test]
fn a_frame_over_the_ceiling_is_refused_by_its_length_alone() {
    // Not JSON at all. Were it parsed first this would be malformed; it is too
    // large because its length is the only thing that was looked at.
    let garbage = vec![b'x'; FRAME_BYTES + 1];
    assert_eq!(refused(&garbage), (None, ErrorCode::TooLarge));
    assert_eq!(
        Response::decode(&garbage).unwrap_err().code(),
        ErrorCode::TooLarge
    );
    assert_eq!(
        Progress::decode(&garbage).unwrap_err().code(),
        ErrorCode::TooLarge
    );
    assert_eq!(
        Snapshot::decode(&garbage).unwrap_err().code(),
        ErrorCode::TooLarge
    );

    // And one byte under is judged for what it says.
    let under = vec![b'x'; FRAME_BYTES];
    assert_eq!(refused(&under), (None, ErrorCode::Malformed));
}

#[test]
fn a_list_over_its_ceiling_is_refused_while_the_frame_is_read() {
    // What follows the one entry too many is not JSON. A reader that built the
    // whole list before counting it would reach that and call the frame
    // malformed; too large is what a reader that stopped at the ceiling says.
    let mut stopped = b"[".to_vec();
    stopped.extend_from_slice("0,".repeat(ITEMS + 1).as_bytes());
    stopped.extend_from_slice(b"}}} not json");
    assert_eq!(refused(&stopped), (None, ErrorCode::TooLarge));

    let mut members = b"{".to_vec();
    for at in 0..=ITEMS {
        members.extend_from_slice(format!("\"k{at}\":0,").as_bytes());
    }
    members.extend_from_slice(b"]]] not json");
    assert_eq!(refused(&members), (None, ErrorCode::TooLarge));

    // A frame of a million one-byte entries fits the frame ceiling.
    let million = format!("[{}0]", "0,".repeat(999_999)).into_bytes();
    assert!(million.len() <= FRAME_BYTES);
    assert_eq!(refused(&million), (None, ErrorCode::TooLarge));
    for decode in [
        |bytes: &[u8]| Response::decode(bytes).map(drop),
        |bytes: &[u8]| Progress::decode(bytes).map(drop),
        |bytes: &[u8]| Snapshot::decode(bytes).map(drop),
    ] {
        assert_eq!(decode(&million).unwrap_err().code(), ErrorCode::TooLarge);
    }

    // The ceiling itself is judged for what it says.
    let at = format!("[{}0]", "0,".repeat(ITEMS - 1)).into_bytes();
    assert_eq!(refused(&at), (None, ErrorCode::Malformed));
}

#[test]
fn a_frame_nested_past_its_ceiling_is_refused_while_it_is_read() {
    fn depth(value: &Value) -> usize {
        match value {
            Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
            Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
            _ => 0,
        }
    }

    let mut deep = "[".repeat(DEPTH + 1).into_bytes();
    deep.extend_from_slice(b"}}} not json");
    assert_eq!(refused(&deep), (None, ErrorCode::TooLarge));

    let within = format!("{}{}", "[".repeat(DEPTH), "]".repeat(DEPTH)).into_bytes();
    assert_eq!(refused(&within), (None, ErrorCode::Malformed));

    // Nothing that crosses comes near it.
    for specimen in specimens() {
        let value: Value = serde_json::from_slice(&specimen.frame).unwrap();
        assert!(depth(&value) * 2 <= DEPTH, "{}", specimen.what);
    }
}

/// How many values `value` is made of, itself included.
fn values(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(values).sum::<usize>(),
        Value::Object(map) => 1 + map.values().map(values).sum::<usize>(),
        _ => 1,
    }
}

#[test]
fn a_frame_of_more_values_than_any_needs_is_refused_while_it_is_read() {
    // No list over its ceiling, three levels deep, under the frame ceiling —
    // and a million values all the same.
    let row = format!("[{}0]", "0,".repeat(ITEMS - 1));
    let block = format!("[{}{row}]", format!("{row},").repeat(ITEMS - 1));
    let nested = format!("[{}{block}]", format!("{block},").repeat(62)).into_bytes();
    assert!(nested.len() <= FRAME_BYTES);
    assert_eq!(refused(&nested), (None, ErrorCode::TooLarge));
    for decode in [
        |bytes: &[u8]| Response::decode(bytes).map(drop),
        |bytes: &[u8]| Progress::decode(bytes).map(drop),
        |bytes: &[u8]| Snapshot::decode(bytes).map(drop),
    ] {
        assert_eq!(decode(&nested).unwrap_err().code(), ErrorCode::TooLarge);
    }

    // What follows the second block is not JSON, and the budget runs out in
    // the eighth: a reader that stopped where it ran out never sees that.
    let stopped = format!("[{block},{block}}}}}}} not json").into_bytes();
    assert_eq!(refused(&stopped), (None, ErrorCode::Malformed));
    let stopped = format!("[{}}}}}}} not json", format!("{block},").repeat(9)).into_bytes();
    assert_eq!(refused(&stopped), (None, ErrorCode::TooLarge));

    // The ceiling is the same one written and read: a list of eight lists of
    // 127 rows is one value over it, and with one entry fewer is exactly at it.
    let row = || Value::Array(vec![json!(0); ITEMS]);
    let over = Value::Array(vec![Value::Array(vec![row(); ITEMS - 1]); 8]);
    assert_eq!(values(&over), VALUES + 1);
    assert_eq!(
        crate::wire::frame(&over).unwrap_err().code(),
        ErrorCode::TooLarge
    );
    assert_eq!(
        crate::wire::parsed(&serde_json::to_vec(&over).unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::TooLarge
    );

    let mut at = over;
    let last = at
        .as_array_mut()
        .and_then(|blocks| blocks.last_mut())
        .and_then(Value::as_array_mut)
        .and_then(|rows| rows.last_mut())
        .and_then(Value::as_array_mut)
        .unwrap();
    last.pop();
    assert_eq!(values(&at), VALUES);
    let written = crate::wire::frame(&at).unwrap();
    assert_eq!(crate::wire::parsed(&written).unwrap(), at);
}

#[test]
fn the_fullest_value_that_crosses_is_within_the_value_ceiling() {
    let choice = || Choice {
        name: Text::cut("n"),
        says: Text::cut("s"),
    };
    let asked = || Asked {
        heading: Text::cut("h"),
        asks: Text::cut("a"),
        several: true,
        choices: vec![choice(); ITEMS],
    };
    let fullest = Snapshot {
        session: Some(SessionId::new()),
        provider: Some(name("anthropic")),
        model: marked(),
        effort: Some(Rung::Max),
        mode: Mode::AllowEdits,
        messages: 4,
        turns: 2,
        carrying: 900,
        left: Some(71),
        pending: Some(Pending::Questions {
            id: PendingId::new(8),
            questions: vec![asked(); ITEMS],
        }),
    };

    let frame = fullest.encode().unwrap();
    let needs = values(&serde_json::from_slice(&frame).unwrap());
    assert!(needs <= VALUES, "{needs}");
    // Derived, not guessed: nothing the ceiling allows for is unused by half.
    assert!(needs * 2 > VALUES, "{needs}");
    assert_eq!(Snapshot::decode(&frame).unwrap(), fullest);

    for specimen in specimens() {
        let value: Value = serde_json::from_slice(&specimen.frame).unwrap();
        assert!(values(&value) <= VALUES, "{}", specimen.what);
    }
}

#[test]
fn a_key_said_twice_is_refused_rather_than_one_of_them_believed() {
    let twice = [
        r#"{"version":1,"capabilities":[],"correlation":41,"correlation":42,"command":{"kind":"help"}}"#,
        r#"{"version":1,"capabilities":[],"correlation":41,"command":{"kind":"interrupt","kind":"help"}}"#,
        r#"{"version":1,"capabilities":[],"correlation":41,"command":{"kind":"decide","decision":{"kind":"ruled","id":7,"id":8,"ruling":"allow","lasting":"once"}}}"#,
        r#"{"version":1,"capabilities":[],"correlation":41,"command":{"kind":"decide","decision":{"kind":"ruled","id":7,"ruling":"deny","ruling":"allow","lasting":"once"}}}"#,
    ];
    for frame in twice {
        assert_eq!(
            refused(frame.as_bytes()),
            (None, ErrorCode::Malformed),
            "{frame}"
        );
    }

    let once = r#"{"version":1,"capabilities":[],"correlation":41,"command":{"kind":"help"}}"#;
    assert!(Request::decode(once.as_bytes()).is_ok());
}

#[test]
fn a_list_over_its_ceiling_is_not_encoded_either() {
    let one = Picked {
        chosen: Vec::new(),
        note: Said::new("").unwrap(),
    };
    let deciding = |answers: Vec<Picked>| {
        Request::new(
            Capabilities::every(),
            Correlation::new(41),
            Command::Decide(Decision::Answered {
                id: PendingId::new(8),
                answers,
            }),
        )
    };

    let over = deciding(vec![one.clone(); ITEMS + 1]).encode();
    assert_eq!(over.err().map(Refusal::code), Some(ErrorCode::TooLarge));

    let chosen = deciding(vec![Picked {
        chosen: vec![Said::new("a").unwrap(); ITEMS + 1],
        ..one.clone()
    }]);
    assert_eq!(
        chosen.encode().err().map(Refusal::code),
        Some(ErrorCode::TooLarge)
    );

    // Whatever is encoded, this build decodes.
    let at = deciding(vec![one; ITEMS]);
    assert_eq!(Request::decode(&at.encode().unwrap()).unwrap(), at);
}

#[test]
fn a_field_over_its_own_ceiling_is_refused_inside_a_frame_that_fits() {
    let oversized = [
        json!({"kind": "prompt", "text": "p".repeat(PROMPT_BYTES + 1)}),
        json!({"kind": "login", "provider": "n".repeat(NAME_BYTES + 1)}),
        json!({"kind": "decide", "decision": {"kind": "answered", "id": 8, "answers":
            [{"chosen": [], "note": "t".repeat(SAID_BYTES + 1)}]}}),
        json!({"kind": "decide", "decision": {"kind": "answered", "id": 8, "answers":
            [{"chosen": ["t".repeat(SAID_BYTES + 1)], "note": ""}]}}),
    ];

    for command in oversized {
        let frame = framed(&asking(&command));
        assert!(frame.len() <= FRAME_BYTES);
        assert_eq!(refused(&frame), (Some(41), ErrorCode::TooLarge));
    }

    // A list over its ceiling is refused where it is read, which is before the
    // frame has said which request it is.
    let many = json!({"kind": "decide", "decision": {"kind": "answered", "id": 8, "answers":
        vec![json!({"chosen": [], "note": ""}); ITEMS + 1]}});
    assert_eq!(
        refused(&framed(&asking(&many))),
        (None, ErrorCode::TooLarge)
    );

    assert_eq!(
        Prompt::new(&"p".repeat(PROMPT_BYTES + 1))
            .unwrap_err()
            .code(),
        ErrorCode::TooLarge
    );
    assert!(Prompt::new(&"p".repeat(PROMPT_BYTES)).is_ok());
}

#[test]
fn a_value_too_large_for_one_frame_is_not_encoded() {
    let many = vec![
        Asked {
            heading: Text::cut(&"h".repeat(TEXT_BYTES)),
            asks: Text::cut(&"a".repeat(TEXT_BYTES)),
            several: false,
            choices: Vec::new(),
        };
        ITEMS
    ];
    let snapshot = Snapshot {
        pending: Some(Pending::Questions {
            id: PendingId::new(1),
            questions: many,
        }),
        ..snapshots().pop().unwrap()
    };

    assert_eq!(snapshot.encode().unwrap_err().code(), ErrorCode::TooLarge);
}

#[test]
fn a_refusal_says_nothing_of_what_it_refused() {
    let frame = framed(&asking(&json!({"kind": MARKER, "text": MARKER})));
    let refused = Request::decode(&frame).unwrap_err();

    assert!(!format!("{refused:?}").contains(MARKER));
    assert!(!refused.refusal.to_string().contains(MARKER));

    // Every code is one stable word and one fixed sentence.
    let words: BTreeSet<&str> = ErrorCode::EVERY
        .into_iter()
        .map(ErrorCode::as_str)
        .collect();
    assert_eq!(words.len(), ErrorCode::EVERY.len());
    for code in ErrorCode::EVERY {
        assert_eq!(ErrorCode::named(code.as_str()), Some(code));
        assert!(
            code.as_str()
                .chars()
                .all(|one| one.is_ascii_lowercase() || one == '_')
        );
    }
}

#[test]
fn progress_and_a_snapshot_cannot_be_read_as_each_other() {
    for one in progress() {
        let frame = one.encode().unwrap();
        assert!(Snapshot::decode(&frame).is_err(), "{}", one.kind());
        assert!(Response::decode(&frame).is_err(), "{}", one.kind());
    }

    for snapshot in snapshots() {
        let frame = snapshot.encode().unwrap();
        assert!(Progress::decode(&frame).is_err());
        assert!(Response::decode(&frame).is_err());
    }
}
