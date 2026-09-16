//! Typed session facts assembled exactly once for one provider pass.
//!
//! The live owners stay where their authority belongs: permission memory on
//! the runner, descriptors in one immutable tool snapshot, and the model on
//! the agent definition. Assembly only borrows their model-visible
//! projections, in one fixed order, and hands back the fragments to record and
//! the merge patch that makes those words replayable as typed state. Recording
//! them is the caller's: words first, state second, so a crash between the two
//! replays as unknown rather than claiming the model saw words never retained.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_models::Effort;
use crucible_tools::{Permission, ToolSnapshot};
use crucible_types::{ContextError, ContextPatch, ContextSnapshot, Fragment, Seen, Transcript};

use crate::{
    ContextSection, EnvironmentSection, ModelSection, PermissionsSection, Skill, SkillsSection,
    ToolsSection, WorkspaceSection, capture, seen,
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

    // Stable sections first. The three live/mid-turn sections follow, with
    // permissions last because an approval can change inside the tool pass
    // immediately preceding this request.
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

    let patch = assembly.current.patch_from(assembly.prior);
    Ok(Assembled {
        fragments: assembly.fragments,
        patch,
    })
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

    use super::*;

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
