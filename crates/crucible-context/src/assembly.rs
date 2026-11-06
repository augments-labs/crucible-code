//! Typed session facts assembled exactly once for one provider pass.
//!
//! The live owners stay where their authority belongs: permission memory on
//! the runner, descriptors in one immutable tool snapshot, and the model on
//! the agent definition. Assembly only borrows their model-visible
//! projections, in one fixed order, and hands back the fragments to record and
//! the merge patch that makes those words replayable as typed state. Recording
//! them is the caller's: words first, state second, so a crash between the two
//! never claims the model saw words that were not retained. After such a crash
//! the session replays against the state recorded before those words: a
//! section told for the first time is unknown and restated whole, and a later
//! change is compared against the state it changed from.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_models::Effort;
use crucible_tools::{Deferred, Permission, ToolSnapshot};
use crucible_types::{ContextError, ContextPatch, ContextSnapshot, Fragment, Seen, Transcript};
use serde_json::Value;

use crate::{
    ContextSection, EnvironmentSection, ModelSection, PermissionsSection, QuestionsSection, Skill,
    SkillsSection, ToolsSection, WorkspaceSection, capture, seen,
};

/// Stable inputs one run's composition root knows and the runner does not.
///
/// Live facts are deliberately absent. Permission, tools, model, effort, date,
/// OS, and architecture are read from their owner once per pass instead.
pub struct ContextInputs {
    workspace: PathBuf,
    skills: Vec<Skill>,
    date: Option<SystemTime>,
}

impl ContextInputs {
    /// A run in `workspace`, with no discovered skills yet.
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            skills: Vec::new(),
            date: None,
        }
    }

    /// Adds the already-bounded skill candidates discovered during wiring.
    ///
    /// The section applies its own retained entry and description bounds; this
    /// input keeps the discovery result rather than a second rendered copy.
    #[must_use]
    pub fn with_skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }

    /// Workspace identity input, consumed only by local scope hashing.
    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Fixes the moment whose UTC date the environment section states,
    /// instead of reading the clock.
    ///
    /// For a run whose words must be reproducible byte for byte, such as a
    /// recorded fixture; an ordinary run reads the clock at each pass. Taking
    /// a time rather than text keeps the section a calendar date.
    #[must_use]
    pub fn dated(mut self, at: SystemTime) -> Self {
        self.date = Some(at);
        self
    }

    fn date(&self) -> String {
        utc_date(self.date.unwrap_or_else(SystemTime::now))
    }
}

impl fmt::Debug for ContextInputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextInputs")
            .field("workspace", &"[redacted]")
            .field(
                "skills",
                &format_args!("{} entries redacted", self.skills.len()),
            )
            .field("date", &self.date.as_ref().map(|_| "[fixed date redacted]"))
            .finish()
    }
}

/// The facts assembly reads from their live owners at each pass.
#[derive(Clone, Copy)]
pub struct Live<'a> {
    /// The model the pass is sent to.
    pub model: &'a str,
    /// The effort the pass asks for, where one was set.
    pub effort: Option<Effort>,
    /// The tools the pass advertises.
    pub tools: &'a ToolSnapshot,
    /// What the permission engine currently allows.
    pub permission: &'a Permission,
    /// The questions asked without waiting, where this run can hold any.
    pub deferred: Option<&'a Deferred>,
}

impl fmt::Debug for Live<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Live")
            .field("model", &self.model)
            .field("effort", &self.effort)
            .finish_non_exhaustive()
    }
}

/// What one pass must record before its request is sent.
#[derive(Debug)]
pub struct Assembled {
    /// The words the model has not yet been told, in section order.
    pub fragments: Vec<Fragment>,
    /// The typed state those words establish, where it changed.
    pub patch: Option<ContextPatch>,
}

/// Reconciles every section for the exact pass about to send.
///
/// `prior` is the recorded snapshot, or `None` where the session has none to
/// trust, in which case every section is treated as unknown and restated.
///
/// # Errors
///
/// [`ContextError`] when a section's state cannot be recorded.
pub fn assemble(
    inputs: &ContextInputs,
    prior: Option<&ContextSnapshot>,
    transcript: &Transcript,
    live: Live<'_>,
) -> Result<Assembled, ContextError> {
    let empty = ContextSnapshot::new();
    let date = inputs.date();
    let mut assembly = Assembly {
        unknown: prior.is_none(),
        prior: prior.unwrap_or(&empty),
        transcript,
        current: ContextSnapshot::new(),
        fragments: Vec::new(),
    };

    // Stable sections first. The live/mid-turn sections follow, with
    // permissions and then the waiting questions last, because an approval can
    // change inside the tool pass immediately preceding this request and a
    // question can be asked or answered inside it too.
    assembly.resolve(&WorkspaceSection::new(&inputs.workspace))?;
    assembly.resolve(&SkillsSection::new(&inputs.skills))?;
    assembly.resolve(&EnvironmentSection::new(
        &date,
        std::env::consts::OS,
        std::env::consts::ARCH,
    ))?;
    assembly.resolve(&ModelSection::new(live.model, live.effort))?;
    assembly.resolve(&ToolsSection::new(live.tools))?;
    assembly.resolve(&PermissionsSection::new(live.permission))?;
    assembly.resolve(&QuestionsSection::new(live.deferred))?;

    let patch = assembly.current.patch_from(assembly.prior);
    Ok(Assembled {
        fragments: assembly.fragments,
        patch,
    })
}

/// The merge patch that records every question batch asked since the state
/// `prior` holds, with the number the next batch is given.
///
/// Taken where a batch can have been asked since the last assembly, before
/// the words that name its id are recorded: a session picked up from a log
/// that holds those words then has each batch they name waiting again, and
/// never hands its id out a second time. Recorded without words, the state is
/// stale to the next assembly, which tells the waiting batches whole.
///
/// Every batch `prior` records stays recorded, even one `held` no longer
/// lists as waiting: an answer settled since then is delivered at the next
/// assembly, and until that assembly records its own state, a log that ends
/// has to bring the batch back rather than lose the answer. `None` where no
/// batch was asked since.
///
/// # Errors
///
/// [`ContextError`] when the state cannot be recorded.
pub fn asked(
    prior: Option<&ContextSnapshot>,
    held: &Deferred,
) -> Result<Option<ContextPatch>, ContextError> {
    let empty = ContextSnapshot::new();
    let prior = prior.unwrap_or(&empty);
    let Value::Object(mut now) = held.state() else {
        return Ok(None);
    };
    let next = now.remove("next").unwrap_or_else(|| Value::from(0_u64));
    let recorded = prior.get(QuestionsSection::ID);
    if recorded.and_then(|state| state.get("next")) == Some(&next) {
        return Ok(None);
    }
    // A registry that never numbered anything has nothing for a log to keep.
    if recorded.is_none() && next == 0_u64 {
        return Ok(None);
    }

    let mut waiting = recorded
        .and_then(|state| state.get("waiting"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let new: Vec<Value> = match now.remove("waiting") {
        Some(Value::Array(asked)) => asked
            .into_iter()
            .filter(|batch| {
                let id = batch.get("id");
                !waiting.iter().any(|kept| kept.get("id") == id)
            })
            .collect(),
        _ => Vec::new(),
    };
    waiting.extend(new);

    let mut numbered = prior.clone();
    numbered.record(
        QuestionsSection::ID,
        serde_json::json!({ "next": next, "waiting": waiting }),
    )?;
    Ok(numbered.patch_from(prior))
}

struct Assembly<'a> {
    unknown: bool,
    prior: &'a ContextSnapshot,
    transcript: &'a Transcript,
    current: ContextSnapshot,
    fragments: Vec<Fragment>,
}

impl Assembly<'_> {
    fn resolve(&mut self, section: &impl ContextSection) -> Result<(), ContextError> {
        let seen = if self.unknown {
            Seen::Unknown
        } else {
            seen(self.prior, section, self.transcript)
        };
        if let Some(fragment) = section.render(seen) {
            self.fragments.push(fragment);
        }
        capture(&mut self.current, section)
    }
}

/// The Gregorian UTC date containing `at`, without a runtime dependency.
fn utc_date(at: SystemTime) -> String {
    let days = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 86_400;
    let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(i64::MAX));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Converts days since 1970-01-01 to a proleptic Gregorian calendar date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days.saturating_add(719_468);
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crucible_types::{Answer, Question, QuestionId, Reply};
    use serde_json::json;

    use super::*;

    fn asking(held: &Deferred, heading: &str) -> u64 {
        held.register(vec![Question::new(
            heading,
            "Which one?",
            [Answer::new("This"), Answer::new("That")],
        )])
        .expect("a batch fits")
        .number()
    }

    /// The ids of the batches a recorded questions state holds, in order.
    fn ids(state: &Value) -> Vec<&str> {
        state
            .get("waiting")
            .and_then(Value::as_array)
            .map(|waiting| {
                waiting
                    .iter()
                    .filter_map(|batch| batch.get("id").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn questions_asked_records_each_new_batch_and_keeps_every_one_recorded_before() {
        let held = Deferred::default();
        let first = asking(&held, "First");
        let mut prior = ContextSnapshot::new();
        prior
            .record(QuestionsSection::ID, held.state())
            .expect("the recorded state");
        // Answered but not yet delivered: a log that ends now still has to
        // bring it back.
        held.settle(QuestionId::numbered(first), vec![Reply::Declined])
            .expect("settled");
        asking(&held, "Second");

        let patch = asked(Some(&prior), &held)
            .expect("recordable")
            .expect("a batch was asked since");
        let recorded = patch.apply(&prior).expect("the patch applies");
        let state = recorded.get(QuestionsSection::ID).expect("still recorded");

        assert_eq!(state.get("next"), Some(&json!(2)));
        assert_eq!(ids(state), ["q0", "q1"]);
        assert_eq!(recorded.sections().len(), 1, "only the questions changed");
    }

    #[test]
    fn questions_asked_with_nothing_recorded_saves_a_state_restore_reads() {
        let held = Deferred::default();
        asking(&held, "First");

        let patch = asked(None, &held)
            .expect("recordable")
            .expect("a batch was asked");
        let recorded = patch
            .apply(&ContextSnapshot::new())
            .expect("the patch applies");
        let state = recorded.get(QuestionsSection::ID).expect("recorded");
        assert_eq!(state, &held.state());

        let resumed = Deferred::default();
        resumed.restore(state).expect("restore reads it");
        assert_eq!(resumed.waiting().len(), 1, "the batch asked was lost");
        assert_eq!(asking(&resumed, "Later"), 1, "an id handed out came back");
    }

    #[test]
    fn questions_asked_is_nothing_where_no_batch_was_asked_since() {
        let held = Deferred::default();
        let first = asking(&held, "First");
        let mut prior = ContextSnapshot::new();
        prior
            .record(QuestionsSection::ID, held.state())
            .expect("the recorded state");
        held.settle(QuestionId::numbered(first), vec![Reply::Declined])
            .expect("settled");

        assert_eq!(asked(Some(&prior), &held).expect("recordable"), None);
        assert_eq!(
            asked(None, &Deferred::default()).expect("recordable"),
            None,
            "a registry that never asked anything records nothing"
        );
    }

    #[test]
    fn utc_calendar_conversion_covers_epoch_and_leap_day() {
        assert_eq!(utc_date(UNIX_EPOCH), "1970-01-01");
        assert_eq!(
            utc_date(UNIX_EPOCH + Duration::from_hours(474_768)),
            "2024-02-29"
        );
    }

    #[test]
    fn a_fixed_moment_is_stated_as_its_utc_date() {
        let late = UNIX_EPOCH + Duration::from_secs(1_789_516_800 + 86_399);
        let inputs = ContextInputs::new("/work").dated(late);

        assert_eq!(inputs.date(), "2026-09-16");
    }

    #[test]
    fn context_inputs_debug_redacts_workspace_and_skills() {
        let input = ContextInputs::new(PathBuf::from("/private/context-input-canary"));
        let shown = format!("{input:?}");

        assert!(!shown.contains("context-input-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
