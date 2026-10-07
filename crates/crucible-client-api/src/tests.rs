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
const KEYS: [&str; 73] = [
    "ambiguous",
    "answers",
    "summary_kind",
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
    "recommended",
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
const MORE_KEYS: [&str; 52] = [
    "added",
    "api_ms",
    "cache_read",
    "cache_write",
    "context",
    "cost",
    "currency",
    "input",
    "limits",
    "more_limits",
    "minutes",
    "micros",
    "output",
    "removed",
    "resets_at",
    "total",
    "windows",
    "usage",
    "used",
    "wall_ms",
    "count",
    "date",
    "free",
    "groups",
    "instructions",
    "mcp",
    "missing",
    "newest",
    "notes",
    "release",
    "releases",
    "reserve",
    "route",
    "running",
    "sentence",
    "served",
    "shown",
    "source",
    "speed",
    "system",
    "tools",
    "unwritten",
    "variable",
    "version",
    "why",
    "window",
    "setting",
    "value",
    "by",
    "asked",
    "sent",
    "left_running",
];

/// The field names of the inspection document, kept apart for the same reason.
const INSPECTION_KEYS: [&str; 31] = [
    "access",
    "allowed",
    "amount",
    "backend",
    "build",
    "ceilings",
    "claim",
    "confined",
    "cwd",
    "denied",
    "effective",
    "feature",
    "format_version",
    "hidden",
    "identity",
    "local_binding",
    "nanos",
    "network",
    "omitted",
    "persistent",
    "policy",
    "provenance",
    "refusal",
    "requested",
    "roots",
    "snapshots",
    "staged",
    "status",
    "unchecked",
    "unit",
    "unix_sockets",
];

/// The field names of the doctor's report that no other value carries.
const DOCTOR_KEYS: [&str; 3] = ["checks", "reason", "remedy"];

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

/// A permission question for every arm of what it can be about.
fn permissions() -> [Pending; 3] {
    let about = |effect, asked| Pending::Permission {
        id: PendingId::new(7),
        tool: marked(),
        effect,
        subject: marked(),
        asked,
    };
    [
        about(
            Effect::SpawnsProcess,
            pending::Operation::Command {
                sent: marked(),
                left_running: true,
            },
        ),
        about(
            Effect::ReachesNetwork,
            pending::Operation::Network { sent: marked() },
        ),
        about(Effect::MutatesFile, pending::Operation::Other),
    ]
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
                recommended: true,
            }],
        }],
    }
}

#[test]
fn a_choice_marked_recommended_crosses_as_a_field_of_its_own_beside_its_unchanged_name() {
    let choice = |name: &str, recommended| Choice {
        name: Text::cut(name),
        says: Text::cut(""),
        recommended,
    };
    let pending = Pending::Questions {
        id: PendingId::new(8),
        questions: vec![Asked {
            heading: Text::cut("h"),
            asks: Text::cut("a"),
            several: false,
            choices: vec![choice("Typed flag", true), choice("Enum value", false)],
        }],
    };

    let written = pending.written();
    let first = "/questions/0/choices/0";
    assert_eq!(
        written.pointer(&format!("{first}/recommended")),
        Some(&json!(true))
    );
    assert_eq!(written.pointer("/questions/0/choices/1/recommended"), None);
    assert_eq!(
        written.pointer(&format!("{first}/name/text")),
        Some(&json!("Typed flag"))
    );

    assert_eq!(Pending::read(written.clone()).unwrap(), pending);

    // A choice written before the flag existed has none, and reads as not
    // recommended; one that says something else is not a flag.
    let mut bare = written.clone();
    bare.pointer_mut(first)
        .and_then(Value::as_object_mut)
        .unwrap()
        .remove("recommended");
    let read = Pending::read(bare).unwrap();
    assert!(matches!(&read, Pending::Questions { questions, .. }
        if questions.iter().flat_map(|one| &one.choices).all(|one| !one.recommended)));

    let mut odd = written;
    *odd.pointer_mut(&format!("{first}/recommended")).unwrap() = json!("yes");
    assert_eq!(Pending::read(odd).unwrap_err().code(), ErrorCode::Malformed);
}

fn warning() -> Pending {
    Pending::Warning {
        id: PendingId::new(9),
        route: marked(),
        shown: marked(),
        sentence: marked(),
        source: marked(),
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
        Decision::Accepted {
            id: PendingId::new(9),
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
        Command::Setting {
            name: name("output.scrollRail"),
            value: name("false"),
        },
        Command::Help,
        Command::ReleaseNotes { version: None },
        Command::ReleaseNotes {
            version: Some(name("v0.41.1")),
        },
        Command::Context,
        Command::Usage,
        Command::AskLimits,
        Command::Exit,
    ];
    commands.extend(decisions().into_iter().map(Command::Decide));
    commands.extend(Rung::EVERY.into_iter().map(Command::SetEffort));
    commands.extend(Pace::EVERY.into_iter().map(Command::SetSpeed));
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
        TurnOutcome::Warned {
            route: marked(),
            sentence: marked(),
            source: marked(),
        },
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

/// How a window is spent, where everything about it is known, and where none
/// of what may be left out is there.
fn contexts() -> [Context; 2] {
    [
        Context {
            model: Model::new(MARKER),
            window: Some(200_000),
            left: Percent::new(62),
            system: 3_100,
            instructions: 1_200,
            tools: 9_400,
            mcp: 4_800,
            messages: 41_600,
            reserve: 36_000,
            free: 103_900,
        },
        Context {
            model: None,
            window: None,
            left: None,
            system: 3_100,
            instructions: 0,
            tools: 9_400,
            mcp: 0,
            messages: 41_600,
            reserve: 0,
            free: 0,
        },
    ]
}

/// The plan-wide limit and one kept for a model, between them every window
/// and every reading there is, and more the vendor reported than were kept.
fn plan_limits() -> Limits {
    let limit = |window, reading, resets_at| Limit {
        window,
        reading,
        resets_at,
    };
    let percent = |used| Reading::Percent(Percent::new(used).unwrap());
    Limits {
        groups: vec![
            LimitGroup {
                model: None,
                limits: vec![
                    limit(Window::FiveHour, percent(23), Some(1_700_000_000)),
                    limit(Window::Daily, Reading::Unlimited, None),
                    limit(Window::Weekly, percent(42), None),
                    limit(Window::Monthly, percent(100), Some(1_702_000_000)),
                ],
            },
            LimitGroup {
                model: Some(name("GPT-5.3-Codex-Spark")),
                limits: vec![
                    limit(
                        Window::Lasting { minutes: 180 },
                        Reading::Counted {
                            used: 412,
                            total: 1_500,
                        },
                        Some(1_700_000_000),
                    ),
                    limit(Window::Yearly, percent(7), None),
                ],
            },
        ],
        more: true,
    }
}

/// What a session has used, by position: priced with every window reported,
/// not priced with none, known only as a floor, and before anything was asked
/// with one window. Callers destructure it by position, so keep this order.
fn usages() -> [Usage; 4] {
    let [context, unknown] = contexts();
    let used = Used {
        cost: Cost::Priced {
            currency: name("USD"),
            micros: 1_840_000,
        },
        api_ms: 252_000,
        wall_ms: 4_920_000,
        added: 214,
        removed: 37,
        input: 1_420_000,
        output: 38_100,
        cache_read: 1_210_000,
        cache_write: 92_400,
    };
    let counted = Usage {
        used: used.clone(),
        context,
        limits: plan_limits(),
    };
    [
        counted.clone(),
        Usage {
            used: Used {
                cost: Cost::NotPriced,
                ..used.clone()
            },
            limits: Limits::default(),
            ..counted.clone()
        },
        Usage {
            used: Used {
                cost: Cost::AtLeast {
                    currency: name("USD"),
                    micros: 400_000,
                },
                ..used.clone()
            },
            ..counted.clone()
        },
        Usage {
            used: Used {
                cost: Cost::Unspent,
                ..used
            },
            context: unknown,
            limits: Limits {
                groups: vec![LimitGroup {
                    model: None,
                    limits: vec![Limit {
                        window: Window::Weekly,
                        reading: Reading::Percent(Percent::new(0).unwrap()),
                        resets_at: Some(1_700_000_000),
                    }],
                }],
                more: false,
            },
        },
    ]
}

/// A release, with its words where there are some.
fn release(text: Option<Text>) -> Release {
    Release {
        version: name("0.41.1"),
        date: name("2026-09-14"),
        groups: vec![
            Group {
                kind: name("security"),
                count: 1,
            },
            Group {
                kind: name("fixed"),
                count: 3,
            },
        ],
        text,
    }
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
        Outcome::Speed(SpeedOutcome::Unasked),
        Outcome::Speed(SpeedOutcome::Unsupported),
        Outcome::Speed(SpeedOutcome::Own),
        Outcome::Speed(SpeedOutcome::Taken {
            unwritten: Some(problem()),
        }),
        Outcome::Speed(SpeedOutcome::Taken { unwritten: None }),
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
        Outcome::Setting(SettingOutcome::Remembered),
        Outcome::Setting(SettingOutcome::Forced(Forced::Environment)),
        Outcome::Setting(SettingOutcome::Forced(Forced::Project)),
        Outcome::Setting(SettingOutcome::Unwritten(problem())),
        Outcome::help(),
        Outcome::Notes(NotesOutcome::Listed {
            releases: vec![release(None), release(Some(marked()))],
            running: name("0.43.3"),
            truncated: true,
        }),
        Outcome::Notes(NotesOutcome::One(release(Some(marked())))),
        Outcome::Notes(NotesOutcome::Unknown {
            newest: name("0.43.3"),
        }),
        Outcome::Notes(NotesOutcome::NotAVersion {
            newest: name("0.43.3"),
        }),
        Outcome::Leaving,
    ];
    outcomes.extend(contexts().into_iter().map(Outcome::Context));
    outcomes.extend(usages().into_iter().map(Outcome::Usage));
    outcomes.extend(login_outcomes());
    outcomes.extend(turn_outcomes().into_iter().map(Outcome::Turn));
    outcomes.extend(Mode::EVERY.into_iter().map(Outcome::Mode));
    outcomes.extend(Missing::EVERY.into_iter().map(Outcome::Unasked));
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
    let [whole, unknown] = contexts();
    let [counted, unpriced, _, unspent] = usages();
    vec![
        Progress::Started { turn: 1 },
        Progress::Delta { text: marked() },
        Progress::ToolRequested {
            call: marked(),
            tool: marked(),
            summary: marked(),
            summary_kind: SummaryKind::Path,
        },
        Progress::ToolRequested {
            call: marked(),
            tool: marked(),
            summary: marked(),
            summary_kind: SummaryKind::Address,
        },
        Progress::ToolRequested {
            call: marked(),
            tool: marked(),
            summary: marked(),
            summary_kind: SummaryKind::Command,
        },
        Progress::ToolRequested {
            call: marked(),
            tool: marked(),
            summary: marked(),
            summary_kind: SummaryKind::Other,
        },
        Progress::ToolFinished {
            call: marked(),
            failed: true,
        },
        Progress::Retrying,
        Progress::Compacting { part: 2 },
        Progress::Compacted { replaced: 9 },
        Progress::Spent { tokens: 1234 },
        // Streamed with the model left out: it is the snapshot's to say.
        Progress::Context(Context {
            model: None,
            ..whole
        }),
        Progress::Context(unknown),
        // Every cost and every window, each streamed as `/usage` reads it.
        Progress::Used(counted.used),
        Progress::Used(unpriced.used),
        Progress::Used(unspent.used),
        Progress::Limits(counted.limits),
        Progress::Limits(unspent.limits),
        Progress::Finished {
            turn: 1,
            stop: Stop::Cancelled,
        },
        Progress::Failed(problem()),
    ]
}

fn snapshots() -> Vec<Snapshot> {
    permissions()
        .map(Some)
        .into_iter()
        .chain([Some(questions()), Some(warning()), None])
        .map(|pending| Snapshot {
            session: pending.as_ref().map(|_| SessionId::new()),
            provider: pending.as_ref().map(|_| name("anthropic")),
            model: pending.as_ref().and_then(|_| Model::new(MARKER)),
            effort: pending.as_ref().map(|_| Rung::Max),
            speed: if pending.is_some() {
                Pace::Fast
            } else {
                Pace::Standard
            },
            served: pending.as_ref().map(|_| Pace::Standard),
            mode: Mode::AllowEdits,
            messages: 4,
            turns: 2,
            carrying: 900,
            left: pending.as_ref().and_then(|_| Percent::new(71)),
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

    for one in inspection::tests::inspections(&marked()) {
        let frame = one.encode().unwrap();
        assert_eq!(inspection::Inspection::decode(&frame).unwrap(), one);
        all.push(Specimen {
            what: format!("inspection {}", one.status()),
            debug: format!("{one:?}"),
            frame,
        });
    }

    for one in doctor::tests::reports(&marked()) {
        let frame = one.encode().unwrap();
        assert_eq!(doctor::Report::decode(&frame).unwrap(), one);
        all.push(Specimen {
            what: format!("doctor {}", one.status()),
            debug: format!("{one:?}"),
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
        } => (0, 5),
        Decision::Ruled {
            ruling: Ruling::Deny,
            ..
        } => (1, 5),
        Decision::Answered { .. } => (2, 5),
        Decision::Declined { .. } => (3, 5),
        Decision::Accepted { .. } => (4, 5),
    }
}

const fn pending_arm(one: &Pending) -> (usize, usize) {
    match one {
        Pending::Permission { .. } => (0, 3),
        Pending::Questions { .. } => (1, 3),
        Pending::Warning { .. } => (2, 3),
    }
}

const fn operation_arm(one: &pending::Operation) -> (usize, usize) {
    match one {
        pending::Operation::Command { .. } => (0, 3),
        pending::Operation::Network { .. } => (1, 3),
        pending::Operation::Other => (2, 3),
    }
}

/// An optional field counts twice: once with something in it, once without.
const fn turn_arm(one: &TurnOutcome) -> (usize, usize) {
    match one {
        TurnOutcome::Ran { .. } => (0, 7),
        TurnOutcome::Rejected { stop: Some(_), .. } => (1, 7),
        TurnOutcome::Rejected { stop: None, .. } => (2, 7),
        TurnOutcome::Undecided { stop: Some(_), .. } => (3, 7),
        TurnOutcome::Undecided { stop: None, .. } => (4, 7),
        TurnOutcome::Failed(_) => (5, 7),
        TurnOutcome::Warned { .. } => (6, 7),
    }
}

const fn command_arm(one: &Command) -> (usize, usize) {
    match one {
        Command::Prompt(_) => (0, 28),
        Command::Compact => (1, 28),
        Command::Cancel => (2, 28),
        Command::Decide(_) => (3, 28),
        Command::Clear => (4, 28),
        Command::Resume(_) => (5, 28),
        Command::SelectModel {
            effort: Some(_), ..
        } => (6, 28),
        Command::SelectModel { effort: None, .. } => (7, 28),
        Command::SetEffort(_) => (8, 28),
        Command::SetMode(_) => (9, 28),
        Command::CycleMode => (10, 28),
        Command::Login { .. } => (11, 28),
        Command::Logout { .. } => (12, 28),
        Command::InspectCache => (13, 28),
        Command::CleanCache => (14, 28),
        Command::Sandbox { enabled: true } => (15, 28),
        Command::Sandbox { enabled: false } => (16, 28),
        Command::Theme(Theme::Drawing(_)) => (17, 28),
        Command::Theme(Theme::Syntax(_)) => (18, 28),
        Command::Help => (19, 28),
        Command::ReleaseNotes { version: None } => (20, 28),
        Command::ReleaseNotes { version: Some(_) } => (21, 28),
        Command::Exit => (22, 28),
        Command::SetSpeed(_) => (23, 28),
        Command::Context => (24, 28),
        Command::Usage => (25, 28),
        Command::Setting { .. } => (26, 28),
        Command::AskLimits => (27, 28),
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
        | Outcome::Context(_)
        | Outcome::Leaving => (0, 1),
        Outcome::Usage(usage) => match usage.used.cost {
            Cost::Unspent => (0, 4),
            Cost::Priced { .. } => (1, 4),
            Cost::NotPriced => (2, 4),
            Cost::AtLeast { .. } => (3, 4),
        },
        Outcome::Turn(turn) => turn_arm(turn),
        Outcome::Unasked(missing) => match missing {
            Missing::Credential => (0, 3),
            Missing::Provider => (1, 3),
            Missing::Model => (2, 3),
        },
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
        Outcome::Speed(speed) => match speed {
            SpeedOutcome::Unasked => (0, 5),
            SpeedOutcome::Unsupported => (1, 5),
            SpeedOutcome::Own => (2, 5),
            SpeedOutcome::Taken { unwritten: Some(_) } => (3, 5),
            SpeedOutcome::Taken { unwritten: None } => (4, 5),
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
        Outcome::Setting(setting) => match setting {
            SettingOutcome::Remembered => (0, 4),
            SettingOutcome::Forced(Forced::Environment) => (1, 4),
            SettingOutcome::Forced(Forced::Project) => (2, 4),
            SettingOutcome::Unwritten(_) => (3, 4),
        },
        Outcome::Notes(notes) => match notes {
            NotesOutcome::Listed { .. } => (0, 4),
            NotesOutcome::One(_) => (1, 4),
            NotesOutcome::Unknown { .. } => (2, 4),
            NotesOutcome::NotAVersion { .. } => (3, 4),
        },
    }
}

/// Every arm of every enum the inspection document carries, and each of its
/// optional fields with and without, as `seen` counts them.
fn inspection_arms(seen: &mut BTreeSet<(String, usize, usize)>) {
    use inspection::{BackendVersion, Claim, Inspection, Network, Requirement, Unit};

    let mut count = |what: &str, (arm, of): (usize, usize)| {
        seen.insert((what.to_owned(), arm, of));
    };
    let claim = |claim: Claim| match claim {
        Claim::Enforced => (0, 3),
        Claim::Observed => (1, 3),
        Claim::Unsupported => (2, 3),
    };
    for one in inspection::tests::inspections(&marked()) {
        let inspected = match one {
            Inspection::Inspected(inspected) => {
                count("inspection", (0, 2));
                inspected
            }
            Inspection::Failed(_) => {
                count("inspection", (1, 2));
                continue;
            }
        };
        count(
            "inspection.mode",
            match inspected.mode {
                Requirement::Optional => (0, 2),
                Requirement::Required => (1, 2),
            },
        );
        count(
            "inspection.backend",
            (usize::from(inspected.backend.is_some()), 2),
        );
        count(
            "inspection.unchecked",
            (usize::from(inspected.unchecked.is_some()), 2),
        );
        count(
            "inspection.refusal",
            (usize::from(inspected.refusal.is_some()), 2),
        );
        for plan in [&inspected.requested, &inspected.effective] {
            count(
                "inspection.network",
                match plan.network {
                    Network::Closed => (0, 2),
                    Network::Domains { .. } => (1, 2),
                },
            );
            for ceiling in &plan.ceilings {
                count("inspection.ceiling.claim", claim(ceiling.claim));
                count(
                    "inspection.unit",
                    match ceiling.unit {
                        Unit::Seconds => (0, 4),
                        Unit::Bytes => (1, 4),
                        Unit::Count => (2, 4),
                        Unit::Micros => (3, 4),
                    },
                );
            }
        }
        if let Some(backend) = inspected.backend {
            count(
                "inspection.build",
                (usize::from(backend.build.is_some()), 2),
            );
            count(
                "inspection.version",
                match backend.version {
                    BackendVersion::Stated(_) => (0, 2),
                    BackendVersion::Unverified(_) => (1, 2),
                },
            );
            for capability in backend.capabilities {
                count("inspection.capability.claim", claim(capability.claim));
            }
        }
    }
}

/// Every arm of a doctor's check, and its remedy with and without, as `seen`
/// counts them.
fn doctor_arms(seen: &mut BTreeSet<(String, usize, usize)>) {
    use doctor::Status;

    for report in doctor::tests::reports(&marked()) {
        for check in report.checks {
            let arm = match check.status {
                Status::Ok => (0, 4),
                Status::Warning => (1, 4),
                Status::Failed => (2, 4),
                Status::Unavailable => (3, 4),
            };
            seen.insert(("doctor.status".to_owned(), arm.0, arm.1));
            seen.insert((
                "doctor.remedy".to_owned(),
                usize::from(check.remedy.is_some()),
                2,
            ));
        }
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
    for context in contexts() {
        either("context.model", context.model.is_some());
        either("context.window", context.window.is_some());
        either("context.left", context.left.is_some());
    }
    for snapshot in snapshots() {
        either("snapshot.session", snapshot.session.is_some());
        either("snapshot.provider", snapshot.provider.is_some());
        either("snapshot.model", snapshot.model.is_some());
        either("snapshot.effort", snapshot.effort.is_some());
        either("snapshot.left", snapshot.left.is_some());
        either("snapshot.pending", snapshot.pending.is_some());
    }
    // Named here, so that a field no specimen sets either way is missed too.
    for what in [
        "response.correlation",
        "resource.expires_at",
        "context.model",
        "context.window",
        "context.left",
        "snapshot.session",
        "snapshot.provider",
        "snapshot.model",
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
    inspection_arms(&mut seen);
    doctor_arms(&mut seen);
    for snapshot in snapshots() {
        if let Some(pending) = &snapshot.pending {
            let (arm, of) = pending_arm(pending);
            seen.insert(("pending".to_owned(), arm, of));
            if let Pending::Permission { asked, .. } = pending {
                let (arm, of) = operation_arm(asked);
                seen.insert(("asked".to_owned(), arm, of));
            }
        }
    }
    whole(&seen);
}

#[test]
fn no_value_that_crosses_names_a_field_for_a_secret_a_path_or_a_handle() {
    let allowed: BTreeSet<&str> = KEYS
        .into_iter()
        .chain(MORE_KEYS)
        .chain(INSPECTION_KEYS)
        .chain(DOCTOR_KEYS)
        .collect();
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
fn words_their_owner_already_cut_say_so_under_this_ceiling_too() {
    // An owner with a lower ceiling hands over words that fit this one, and a
    // mark inside them is only more words.
    assert!(Text::cut_again("kept short [cut]", true).truncated());
    assert!(!Text::cut_again("ends in [cut]", false).truncated());
    assert_eq!(Text::cut_again("whole", false), Text::cut("whole"));

    let long = "é".repeat(TEXT_BYTES);
    assert_eq!(Text::cut_again(&long, false), Text::cut(&long));
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
    json!({"version": 3, "capabilities": [], "correlation": 41, "command": command})
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
    for version in [0, 1, 2, 4, 65_535, 65_536, u64::MAX] {
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
        (b"{\"version\": 2".to_vec(), None),
        (framed(&json!({"version": 3})), None),
        (
            framed(&json!({"version": 3, "capabilities": [], "correlation": -1, "command": {}})),
            None,
        ),
        (
            framed(
                &json!({"version": "2", "capabilities": [], "correlation": 41,
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
            framed(&json!({"version": 3, "capabilities": [], "correlation": 41,
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
        json!({"kind": "set_speed", "speed": "ludicrous"}),
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
    // A question marks at most one of its choices, and the first.
    let choice = |recommended| Choice {
        name: Text::cut("n"),
        says: Text::cut("s"),
        recommended,
    };
    let asked = || Asked {
        heading: Text::cut("h"),
        asks: Text::cut("a"),
        several: true,
        choices: std::iter::once(choice(true))
            .chain(vec![choice(false); ITEMS - 1])
            .collect(),
    };
    let fullest = Snapshot {
        session: Some(SessionId::new()),
        provider: Some(name("anthropic")),
        model: Model::new(MARKER),
        effort: Some(Rung::Max),
        speed: Pace::Standard,
        served: Some(Pace::Standard),
        mode: Mode::AllowEdits,
        messages: 4,
        turns: 2,
        carrying: 900,
        left: Percent::new(71),
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
        r#"{"version":3,"capabilities":[],"correlation":41,"correlation":42,"command":{"kind":"help"}}"#,
        r#"{"version":3,"capabilities":[],"correlation":41,"command":{"kind":"interrupt","kind":"help"}}"#,
        r#"{"version":3,"capabilities":[],"correlation":41,"command":{"kind":"decide","decision":{"kind":"ruled","id":7,"id":8,"ruling":"allow","lasting":"once"}}}"#,
        r#"{"version":3,"capabilities":[],"correlation":41,"command":{"kind":"decide","decision":{"kind":"ruled","id":7,"ruling":"deny","ruling":"allow","lasting":"once"}}}"#,
    ];
    for frame in twice {
        assert_eq!(
            refused(frame.as_bytes()),
            (None, ErrorCode::Malformed),
            "{frame}"
        );
    }

    let once = r#"{"version":3,"capabilities":[],"correlation":41,"command":{"kind":"help"}}"#;
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

/// `frame` without `field`.
fn without(mut frame: Value, field: &str) -> Value {
    frame
        .as_object_mut()
        .expect("a frame is an object")
        .remove(field);
    frame
}

#[test]
fn progress_and_a_snapshot_say_their_version_and_another_is_refused_by_name() {
    type Reading = fn(&[u8]) -> Result<(), Refusal>;
    let mut frames: Vec<(Value, Reading)> = Vec::new();
    for one in progress() {
        let frame = serde_json::from_slice(&one.encode().unwrap()).unwrap();
        frames.push((frame, |bytes| Progress::decode(bytes).map(drop)));
    }
    for snapshot in snapshots() {
        let frame = serde_json::from_slice(&snapshot.encode().unwrap()).unwrap();
        frames.push((frame, |bytes| Snapshot::decode(bytes).map(drop)));
    }

    for (frame, read) in frames {
        assert_eq!(
            frame.get("version"),
            Some(&json!(Version::CURRENT.number())),
            "{frame}"
        );

        for version in [0, 1, 2, 4, 65_536, u64::MAX] {
            let other = with(frame.clone(), "version", json!(version));
            assert_eq!(
                read(&framed(&other)).unwrap_err().code(),
                ErrorCode::UnsupportedVersion,
                "version {version}"
            );
        }

        // A frame that does not say is not taken for one this build speaks.
        let silent = without(frame, "version");
        assert_eq!(
            read(&framed(&silent)).unwrap_err().code(),
            ErrorCode::Malformed
        );
    }
}

#[test]
fn a_snapshot_says_no_percentage_over_a_hundred_and_no_model_by_saying_none() {
    let standing = |snapshot: &Snapshot| -> Value {
        serde_json::from_slice(&snapshot.encode().unwrap()).unwrap()
    };
    let inside = |mut frame: Value, field: &str, value: Option<Value>| -> Vec<u8> {
        let fields = frame
            .get_mut("snapshot")
            .and_then(Value::as_object_mut)
            .expect("a snapshot is an object");
        match value {
            Some(value) => fields.insert(field.to_owned(), value),
            None => fields.remove(field),
        };
        framed(&frame)
    };
    let whole = standing(&snapshots().remove(0));

    assert_eq!(Percent::new(100), Some(Percent::WHOLE));
    assert_eq!(Percent::new(101), None);
    assert_eq!(Percent::new(71).map(Percent::get), Some(71));

    for left in [0, 71, 100] {
        let frame = inside(whole.clone(), "left", Some(json!(left)));
        assert!(Snapshot::decode(&frame).is_ok(), "{left}");
    }
    for left in [101, 255, 256, u64::MAX] {
        let frame = inside(whole.clone(), "left", Some(json!(left)));
        assert_eq!(
            Snapshot::decode(&frame).unwrap_err().code(),
            ErrorCode::Malformed,
            "{left}"
        );
    }

    // No model is said by leaving the field out, and an empty one is refused
    // rather than read as a second way to say the same thing.
    let none = inside(whole.clone(), "model", None);
    assert!(Snapshot::decode(&none).is_ok());
    let empty = inside(
        whole,
        "model",
        Some(json!({"text": "", "truncated": false})),
    );
    assert_eq!(
        Snapshot::decode(&empty).unwrap_err().code(),
        ErrorCode::Malformed
    );

    // Nor can one be written: there is no model made of no words, so a frame
    // this crate writes is one it reads.
    assert_eq!(Model::new(""), None);
    let mut named = snapshots().remove(0);
    named.model = Model::new("a-model");
    assert!(named.model.is_some());
    let frame = named.encode().unwrap();
    assert_eq!(Snapshot::decode(&frame).map_err(Refusal::code), Ok(named));
}

/// Every place a field sits in `value`, as the names on the way down to it.
fn places(value: &Value, under: &str, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let here = format!("{under}/{key}");
                places(inner, &here, found);
                found.insert(here);
            }
        }
        Value::Array(items) => {
            let here = format!("{under}[]");
            for inner in items {
                places(inner, &here, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[test]
fn the_version_moves_with_what_a_frame_is_made_of() {
    // What each kind of frame is made of, from the same specimens that hold
    // every arm: one line a kind, the places its fields sit, in order.
    let mut kinds = std::collections::BTreeMap::<String, BTreeSet<String>>::new();
    for specimen in specimens() {
        let value: Value = serde_json::from_slice(&specimen.frame).unwrap();
        places(&value, "", kinds.entry(specimen.what).or_default());
    }
    let made_of: Vec<String> = kinds
        .iter()
        .map(|(kind, found)| {
            let found: Vec<&str> = found.iter().map(String::as_str).collect();
            format!("{kind}: {}", found.join(" "))
        })
        .collect();
    let made_of = made_of.join("\n");

    // Written out here so that the number cannot change with the toolchain.
    let digest = made_of
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |sum, byte| {
            (sum ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });

    // The two are pinned together because nothing else holds them together: a
    // field added, renamed or taken away under the same number is a build that
    // says it speaks a revision and refuses its frames as malformed.
    //
    // The digest sees that a field is there and where, and nothing else. A
    // value changing type, a required field becoming optional or the reverse,
    // and whatever sits inside an array that is empty in every specimen all
    // leave it as it was; those still need the number moved by hand.
    assert_eq!(
        (Version::CURRENT.number(), digest),
        (3, 14_318_635_380_701_603_224),
        "what a frame is made of moved. Once a release speaks this contract, \
         move Version::CURRENT with it; then write the pair here.\n{made_of}"
    );
}

#[test]
fn a_context_with_no_window_says_no_room_left_and_none_free() {
    // Both are read against the window, so without one a frame that states
    // either states a figure nothing measured.
    let [_, unknown] = contexts();
    let response = Response {
        correlation: Some(Correlation::new(3)),
        outcome: Outcome::Context(unknown),
    };
    let frame: Value = serde_json::from_slice(&response.encode().unwrap()).unwrap();
    let saying = |field: &str, value: Value| -> Vec<u8> {
        let mut frame = frame.clone();
        frame
            .pointer_mut("/outcome/context")
            .and_then(Value::as_object_mut)
            .expect("a context is an object")
            .insert(field.to_owned(), value);
        framed(&frame)
    };

    assert_eq!(
        Response::decode(&framed(&frame)).map_err(Refusal::code),
        Ok(response)
    );
    assert!(Response::decode(&saying("free", json!(0))).is_ok());
    for (field, value) in [("left", json!(62)), ("free", json!(1))] {
        assert_eq!(
            Response::decode(&saying(field, value)).unwrap_err().code(),
            ErrorCode::Malformed,
            "{field}"
        );
    }
}

#[test]
fn a_context_streamed_during_a_turn_names_no_model() {
    // The model is the snapshot's to say; a second place saying it is a
    // second figure for one fact.
    let streamed = progress()
        .into_iter()
        .find(|one| matches!(one, Progress::Context(context) if context.window.is_some()))
        .expect("a context specimen with a window");
    let mut frame: Value = serde_json::from_slice(&streamed.encode().unwrap()).unwrap();
    assert_eq!(
        Progress::decode(&framed(&frame)).map_err(Refusal::code),
        Ok(streamed)
    );

    frame
        .pointer_mut("/context")
        .and_then(Value::as_object_mut)
        .expect("a context is an object")
        .insert(
            "model".to_owned(),
            json!({"text": "a-model", "truncated": false}),
        );
    assert_eq!(
        Progress::decode(&framed(&frame)).unwrap_err().code(),
        ErrorCode::Malformed
    );
}

#[test]
fn an_unasked_outcome_naming_nothing_it_knows_is_malformed() {
    let response = Response {
        correlation: Some(Correlation::new(3)),
        outcome: Outcome::Unasked(Missing::Credential),
    };
    let frame: Value = serde_json::from_slice(&response.encode().unwrap()).unwrap();
    assert_eq!(
        frame.get("outcome"),
        Some(&json!({"kind": "unasked", "missing": "credential"})),
        "{frame}"
    );
    assert_eq!(
        Response::decode(&framed(&frame)).map_err(Refusal::code),
        Ok(response)
    );

    let nothing = with(
        frame,
        "outcome",
        json!({"kind": "unasked", "missing": "nothing"}),
    );
    assert_eq!(
        Response::decode(&framed(&nothing)).unwrap_err().code(),
        ErrorCode::Malformed
    );
}

#[test]
fn a_setting_crosses_as_the_name_of_its_key_and_the_word_for_its_value() {
    let request = Request::new(
        Capabilities::every(),
        Correlation::new(4),
        Command::Setting {
            name: name("output.theme"),
            value: name("light"),
        },
    );
    let frame: Value = serde_json::from_slice(&request.encode().unwrap()).unwrap();
    assert_eq!(
        frame.get("command"),
        Some(&json!({"kind": "setting", "name": "output.theme", "value": "light"})),
        "{frame}"
    );
    assert_eq!(
        Request::decode(&framed(&frame)).map_err(|refused| refused.refusal.code()),
        Ok(request)
    );

    let response = Response {
        correlation: Some(Correlation::new(4)),
        outcome: Outcome::Setting(SettingOutcome::Forced(Forced::Project)),
    };
    let frame: Value = serde_json::from_slice(&response.encode().unwrap()).unwrap();
    assert_eq!(
        frame.get("outcome"),
        Some(&json!({"kind": "setting", "setting": {"kind": "forced", "by": "project"}})),
        "{frame}"
    );
    let elsewhere = with(
        frame,
        "outcome",
        json!({"kind": "setting", "setting": {"kind": "forced", "by": "a-neighbour"}}),
    );
    assert_eq!(
        Response::decode(&framed(&elsewhere)).unwrap_err().code(),
        ErrorCode::Malformed
    );
}

#[test]
fn plan_limit_crosses_as_a_word_of_its_own_with_a_fixed_sentence() {
    assert!(ErrorCode::EVERY.contains(&ErrorCode::PlanLimit));
    assert_eq!(ErrorCode::PlanLimit.as_str(), "plan_limit");
    assert_eq!(ErrorCode::named("plan_limit"), Some(ErrorCode::PlanLimit));
    assert_eq!(
        Refusal::new(ErrorCode::PlanLimit).to_string(),
        "the plan's usage limit is reached until one of its windows resets"
    );
}

#[test]
fn plan_limit_a_failed_turn_carrying_it_reads_back_as_it_was_written() {
    let one = Progress::Failed(Problem {
        code: ErrorCode::PlanLimit,
        message: Text::cut("usage limit reached on the weekly window"),
    });

    let frame = one.encode().unwrap();
    let value: Value = serde_json::from_slice(&frame).unwrap();

    assert_eq!(
        value.pointer("/problem/code"),
        Some(&json!("plan_limit")),
        "{value}"
    );
    assert_eq!(Progress::decode(&frame).unwrap(), one);
}

#[test]
fn plan_limit_is_spoken_under_the_second_revision_and_not_the_first() {
    assert!(Version::CURRENT.number() >= 2);
    assert!(Version::CURRENT.spoken());
    assert!(!Version::numbered(1).spoken());

    let frame = with(asking(&json!({"kind": "help"})), "version", json!(1));
    assert_eq!(
        refused(&framed(&frame)),
        (Some(41), ErrorCode::UnsupportedVersion)
    );
}

#[test]
fn limit_every_window_and_reading_reads_back_as_it_was_written() {
    let limits = plan_limits();
    let one = Progress::Limits(limits.clone());

    let frame = one.encode().unwrap();
    let value: Value = serde_json::from_slice(&frame).unwrap();

    assert_eq!(Progress::decode(&frame).unwrap(), one);
    assert_eq!(
        value.pointer("/limits/1/model"),
        Some(&json!("GPT-5.3-Codex-Spark")),
        "{value}"
    );
    assert_eq!(
        value.pointer("/limits/1/windows/0"),
        Some(&json!({
            "window": {"kind": "lasting", "minutes": 180},
            "used": {"kind": "counted", "used": 412, "total": 1500},
            "resets_at": 1_700_000_000,
        })),
        "{value}"
    );
    assert_eq!(value.pointer("/limits/0/model"), None, "{value}");
    assert_eq!(value.pointer("/more_limits"), Some(&json!(true)), "{value}");
    assert!(!limits.is_empty());
    assert!(Limits::default().is_empty());

    let whole = Progress::Limits(Limits::default());
    let frame = whole.encode().unwrap();
    let value: Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(
        value.pointer("/more_limits"),
        Some(&json!(false)),
        "{value}"
    );
    assert_eq!(Progress::decode(&frame).unwrap(), whole);
}

#[test]
fn limit_a_usage_that_left_limits_out_reads_back_saying_so() {
    let [counted, _, _, _] = usages();
    assert!(counted.limits.more);
    let response = Response {
        correlation: Some(Correlation::new(5)),
        outcome: Outcome::Usage(counted),
    };
    let frame = response.encode().unwrap();
    let value: Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(
        value.pointer("/outcome/usage/more_limits"),
        Some(&json!(true)),
        "{value}"
    );
    assert_eq!(Response::decode(&frame).unwrap(), response);
}

/// A progress frame carrying `limits` as they were written.
fn limits_frame(limits: &Value) -> Vec<u8> {
    let frame = Progress::Limits(Limits::default()).encode().unwrap();
    let mut value: Value = serde_json::from_slice(&frame).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("limits".to_owned(), limits.clone());
    framed(&value)
}

#[test]
fn limit_lists_over_the_plans_ceilings_are_refused() {
    let window = json!({
        "window": {"kind": "weekly"},
        "used": {"kind": "percent", "used": 5},
    });
    let group = |windows: usize| json!({"windows": vec![window.clone(); windows]});

    let groups = json!(vec![group(1); crucible_types::MAX_LIMIT_GROUPS + 1]);
    let windows = json!([group(crucible_types::MAX_GROUP_WINDOWS + 1)]);
    let named = json!([{
        "model": "m".repeat(crucible_types::MAX_LIMIT_NAME_BYTES + 1),
        "windows": [],
    }]);
    for over in [groups, windows, named] {
        assert_eq!(
            Progress::decode(&limits_frame(&over)).map_err(Refusal::code),
            Err(ErrorCode::TooLarge),
            "{over}"
        );
    }

    let full = json!(vec![
        group(crucible_types::MAX_GROUP_WINDOWS);
        crucible_types::MAX_LIMIT_GROUPS
    ]);
    assert!(Progress::decode(&limits_frame(&full)).is_ok());
}

#[test]
fn limit_readings_no_vendor_could_give_are_malformed() {
    let reading =
        |window: Value, used: Value| json!([{"windows": [{"window": window, "used": used}]}]);
    let weekly = json!({"kind": "weekly"});
    for wrong in [
        reading(weekly.clone(), json!({"kind": "percent", "used": 101})),
        reading(
            weekly.clone(),
            json!({"kind": "counted", "used": 3, "total": 0}),
        ),
        reading(
            weekly.clone(),
            json!({"kind": "counted", "used": 4, "total": 3}),
        ),
        reading(weekly.clone(), json!({"kind": "spent"})),
        reading(
            json!({"kind": "lasting", "minutes": 0}),
            json!({"kind": "unlimited"}),
        ),
        reading(json!({"kind": "fortnightly"}), json!({"kind": "unlimited"})),
        json!([{"model": "", "windows": []}]),
        json!({"groups": []}),
    ] {
        assert!(Progress::decode(&limits_frame(&wrong)).is_err(), "{wrong}");
    }
}

#[test]
fn limit_asking_crosses_as_a_command_of_its_own() {
    let frame = asking(&json!({"kind": "ask_limits"}));
    let request = Request::decode(&framed(&frame)).unwrap();

    assert_eq!(request.command(), &Command::AskLimits);
    assert_eq!(Command::AskLimits.kind(), "ask_limits");
    assert!(Command::KINDS.contains(&"ask_limits"));
    assert_eq!(
        Request::decode(&request.encode().unwrap()).unwrap(),
        request
    );
}
