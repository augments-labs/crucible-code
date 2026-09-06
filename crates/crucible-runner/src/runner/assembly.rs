//! Typed session facts assembled exactly once for one provider pass.
//!
//! The live owners stay where their authority belongs: permission memory on
//! the runner, descriptors in one immutable tool snapshot, and the model on
//! the agent definition. This module only borrows their model-visible
//! projections. It records rendered fragments through [`Runner::record`], so
//! request load and durable history observe the same bytes, then records the
//! merge patch that makes those words replayable as typed state.

use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_core::{
    ContextSection, ContextSnapshot, EnvironmentSection, Fragment, ModelSection,
    PermissionsSection, Seen, Skill, SkillsSection, ToolsSection, Transcript, TurnError,
    WorkspaceSection,
};

use super::Runner;

/// Stable inputs one run's composition root knows and the runner does not.
///
/// Live facts are deliberately absent. Permission, tools, model, effort, date,
/// OS, and architecture are read from their owner once per pass instead.
pub struct ContextInputs {
    workspace: PathBuf,
    skills: Vec<Skill>,
    date: Option<Box<str>>,
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
    pub(super) fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }

    #[cfg(test)]
    pub(super) fn dated(mut self, date: &str) -> Self {
        self.date = Some(date.into());
        self
    }

    fn date(&self) -> String {
        if let Some(date) = &self.date {
            return date.to_string();
        }
        utc_date(SystemTime::now())
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

impl Runner {
    /// Reconciles and records every section for the exact pass about to send.
    pub(super) fn assemble_context(
        &mut self,
        ancestry: crucible_core::Ancestry,
    ) -> Result<(), TurnError> {
        let (fragments, patch) = {
            let empty = ContextSnapshot::new();
            let persisted = self.session.context_snapshot();
            let prior = persisted.unwrap_or(&empty);
            let unknown = persisted.is_none();
            let transcript = &self.transcript;
            let date = self.context.date();
            let mut assembly = Assembly {
                unknown,
                prior,
                transcript,
                current: ContextSnapshot::new(),
                fragments: Vec::new(),
            };

            // Stable sections first. The three live/mid-turn sections follow,
            // with permissions last because an approval can change inside the
            // tool pass immediately preceding this request.
            assembly.resolve(&WorkspaceSection::new(&self.context.workspace))?;
            assembly.resolve(&SkillsSection::new(&self.context.skills))?;
            assembly.resolve(&EnvironmentSection::new(
                &date,
                std::env::consts::OS,
                std::env::consts::ARCH,
            ))?;
            assembly.resolve(&ModelSection::new(
                &self.spec.model.name,
                self.spec.model.effort,
            ))?;
            assembly.resolve(&ToolsSection::new(&self.tools))?;
            assembly.resolve(&PermissionsSection::new(&self.permission))?;

            let patch = assembly.current.patch_from(prior);
            (assembly.fragments, patch)
        };

        // Words first, state second. A crash between them replays as Unknown;
        // the opposite order could claim the model saw words never retained.
        for fragment in fragments {
            self.record(ancestry, crucible_core::Message::Context(fragment))?;
        }
        if let Some(patch) = patch {
            self.session.contextual(&patch)?;
        }

        Ok(())
    }
}

struct Assembly<'a> {
    unknown: bool,
    prior: &'a ContextSnapshot,
    transcript: &'a Transcript,
    current: ContextSnapshot,
    fragments: Vec<Fragment>,
}

impl Assembly<'_> {
    fn resolve(
        &mut self,
        section: &impl ContextSection,
    ) -> Result<(), crucible_core::ContextError> {
        let seen = if self.unknown {
            Seen::Unknown
        } else {
            self.prior.seen(section, self.transcript)
        };
        if let Some(fragment) = section.render(seen) {
            self.fragments.push(fragment);
        }
        self.current.capture(section)
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
    fn context_inputs_debug_redacts_workspace_and_skills() {
        let input = ContextInputs::new(PathBuf::from("/private/context-input-canary"));
        let shown = format!("{input:?}");

        assert!(!shown.contains("context-input-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
