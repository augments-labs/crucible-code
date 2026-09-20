//! How each command's outcome is written and read.
//!
//! One `written` and one `read` a type, each matching every arm, so an arm
//! added to an outcome does not compile until it has a spelling both ways.

use serde_json::Value;

use super::{
    CacheOutcome, CleanOutcome, ClearOutcome, EffortOutcome, LoginOutcome, LogoutOutcome,
    ModelOutcome, Problem, Resource, ResumeOutcome, Retained, RoomOutcome, SandboxOutcome,
    Standing, Stop, ThemeOutcome, TurnOutcome, maybe_problem,
};
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing};

/// `object`, with `problem` under `key` where there is one.
fn noting(object: Writing, key: &str, problem: Option<&Problem>) -> Writing {
    object.maybe(key, problem.map(Problem::written))
}

/// The problem an arm is nothing but.
fn failed(kind: &str, problem: &Problem) -> Writing {
    Writing::kind(kind).with("problem", problem.written())
}

fn problem(fields: &mut Fields) -> Result<Problem, Refusal> {
    Problem::read(fields.take("problem")?)
}

impl TurnOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Ran { stop } => Writing::kind("ran").with("stop", stop.as_str()),
            Self::Rejected { guard, why, stop } => Writing::kind("rejected")
                .text("guard", guard)
                .text("why", why)
                .maybe("stop", stop.map(Stop::as_str)),
            Self::Undecided { problem, stop } => {
                failed("undecided", problem).maybe("stop", stop.map(Stop::as_str))
            }
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let turn = match fields.kind()?.as_str() {
            "ran" => Self::Ran {
                stop: Stop::named(&fields.string("stop")?)?,
            },
            "rejected" => Self::Rejected {
                guard: fields.text("guard")?,
                why: fields.text("why")?,
                stop: Stop::maybe(&mut fields)?,
            },
            "undecided" => Self::Undecided {
                problem: problem(&mut fields)?,
                stop: Stop::maybe(&mut fields)?,
            },
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(turn)
    }
}

impl RoomOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Made { replaced } => Writing::kind("made").with("replaced", *replaced),
            Self::Nothing => Writing::kind("nothing"),
            Self::Stopped => Writing::kind("stopped"),
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let room = match fields.kind()?.as_str() {
            "made" => Self::Made {
                replaced: fields.number("replaced")?,
            },
            "nothing" => Self::Nothing,
            "stopped" => Self::Stopped,
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(room)
    }
}

impl ClearOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Nothing => Writing::kind("nothing"),
            Self::Started { unclosed } => {
                noting(Writing::kind("started"), "unclosed", unclosed.as_ref())
            }
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let cleared = match fields.kind()?.as_str() {
            "nothing" => Self::Nothing,
            "started" => Self::Started {
                unclosed: maybe_problem(&mut fields, "unclosed")?,
            },
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(cleared)
    }
}

impl ResumeOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Same => Writing::kind("same"),
            Self::Picked { unclosed } => {
                noting(Writing::kind("picked"), "unclosed", unclosed.as_ref())
            }
            Self::Unknown => Writing::kind("unknown"),
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let resumed = match fields.kind()?.as_str() {
            "same" => Self::Same,
            "picked" => Self::Picked {
                unclosed: maybe_problem(&mut fields, "unclosed")?,
            },
            "unknown" => Self::Unknown,
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(resumed)
    }
}

impl ModelOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Unsupported => Writing::kind("unsupported"),
            Self::Unreachable(problem) => failed("unreachable", problem),
            Self::CacheHeld(problem) => failed("cache_held", problem),
            Self::Taken {
                retained,
                unwritten,
            } => noting(
                retained.onto(Writing::kind("taken")),
                "unwritten",
                unwritten.as_ref(),
            ),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let model = match fields.kind()?.as_str() {
            "unsupported" => Self::Unsupported,
            "unreachable" => Self::Unreachable(problem(&mut fields)?),
            "cache_held" => Self::CacheHeld(problem(&mut fields)?),
            "taken" => Self::Taken {
                retained: Retained::from(&mut fields)?,
                unwritten: maybe_problem(&mut fields, "unwritten")?,
            },
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(model)
    }
}

impl EffortOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Unasked => Writing::kind("unasked"),
            Self::Unsupported => Writing::kind("unsupported"),
            Self::Taken { unwritten } => {
                noting(Writing::kind("taken"), "unwritten", unwritten.as_ref())
            }
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let effort = match fields.kind()?.as_str() {
            "unasked" => Self::Unasked,
            "unsupported" => Self::Unsupported,
            "taken" => Self::Taken {
                unwritten: maybe_problem(&mut fields, "unwritten")?,
            },
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(effort)
    }
}

impl LoginOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Unusable(problem) => failed("unusable", problem),
            Self::Elsewhere => Writing::kind("elsewhere"),
            Self::CacheHeld(problem) => failed("cache_held", problem),
            Self::Serving {
                retained,
                unwritten,
            } => noting(
                retained.onto(Writing::kind("serving")),
                "unwritten",
                unwritten.as_ref(),
            ),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let login = match fields.kind()?.as_str() {
            "unusable" => Self::Unusable(problem(&mut fields)?),
            "elsewhere" => Self::Elsewhere,
            "cache_held" => Self::CacheHeld(problem(&mut fields)?),
            "serving" => Self::Serving {
                retained: Retained::from(&mut fields)?,
                unwritten: maybe_problem(&mut fields, "unwritten")?,
            },
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(login)
    }
}

impl Standing {
    fn written(&self) -> Value {
        match self {
            Self::Environment(variable) => Writing::kind("environment").text("variable", variable),
            Self::StoredKey => Writing::kind("stored_key"),
            Self::Subscription => Writing::kind("subscription"),
        }
        .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let standing = match fields.kind()?.as_str() {
            "environment" => Self::Environment(fields.text("variable")?),
            "stored_key" => Self::StoredKey,
            "subscription" => Self::Subscription,
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(standing)
    }
}

impl LogoutOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::CacheHeld(problem) => failed("cache_held", problem),
            Self::Unforgotten { retained, problem } => {
                retained.onto(failed("unforgotten", problem))
            }
            Self::Kept => Writing::kind("kept"),
            Self::StillServed { retained, standing } => retained
                .onto(Writing::kind("still_served"))
                .with("standing", standing.written()),
            Self::SignedOut { retained } => retained.onto(Writing::kind("signed_out")),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let logout = match fields.kind()?.as_str() {
            "cache_held" => Self::CacheHeld(problem(&mut fields)?),
            "unforgotten" => Self::Unforgotten {
                retained: Retained::from(&mut fields)?,
                problem: problem(&mut fields)?,
            },
            "kept" => Self::Kept,
            "still_served" => Self::StillServed {
                retained: Retained::from(&mut fields)?,
                standing: Standing::read(fields.take("standing")?)?,
            },
            "signed_out" => Self::SignedOut {
                retained: Retained::from(&mut fields)?,
            },
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(logout)
    }
}

impl Resource {
    fn written(&self) -> Value {
        Writing::new()
            .text("state", &self.state)
            .maybe("expires_at", self.expires_at)
            .text("isolation", &self.isolation)
            .with("exclusive", self.exclusive)
            .text("protocol", &self.protocol)
            .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let resource = Self {
            state: fields.text("state")?,
            expires_at: fields.maybe_number("expires_at")?,
            isolation: fields.text("isolation")?,
            exclusive: fields.flag("exclusive")?,
            protocol: fields.text("protocol")?,
        };
        fields.done()?;
        Ok(resource)
    }
}

impl CacheOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Listed {
                resources,
                truncated,
            } => Writing::kind("listed")
                .with(
                    "resources",
                    resources.iter().map(Resource::written).collect::<Vec<_>>(),
                )
                .with("truncated", *truncated),
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let cache = match fields.kind()?.as_str() {
            "listed" => Self::Listed {
                resources: fields
                    .list("resources")?
                    .into_iter()
                    .map(Resource::read)
                    .collect::<Result<_, _>>()?,
                truncated: fields.flag("truncated")?,
            },
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(cache)
    }
}

impl CleanOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Counted {
                inspected,
                deleted,
                ambiguous,
                orphaned,
            } => Writing::kind("counted")
                .with("inspected", *inspected)
                .with("deleted", *deleted)
                .with("ambiguous", *ambiguous)
                .with("orphaned", *orphaned),
            Self::Failed(problem) => failed("failed", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let cleaned = match fields.kind()?.as_str() {
            "counted" => Self::Counted {
                inspected: fields.number("inspected")?,
                deleted: fields.number("deleted")?,
                ambiguous: fields.number("ambiguous")?,
                orphaned: fields.number("orphaned")?,
            },
            "failed" => Self::Failed(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(cleaned)
    }
}

impl SandboxOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Set { enabled } => Writing::kind("set").with("enabled", *enabled),
            Self::Unchanged(problem) => failed("unchanged", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let sandbox = match fields.kind()?.as_str() {
            "set" => Self::Set {
                enabled: fields.flag("enabled")?,
            },
            "unchanged" => Self::Unchanged(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(sandbox)
    }
}

impl ThemeOutcome {
    pub(super) fn written(&self) -> Value {
        match self {
            Self::Remembered => Writing::kind("remembered"),
            Self::Unwritten(problem) => failed("unwritten", problem),
        }
        .finish()
    }

    pub(super) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let theme = match fields.kind()?.as_str() {
            "remembered" => Self::Remembered,
            "unwritten" => Self::Unwritten(problem(&mut fields)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(theme)
    }
}
