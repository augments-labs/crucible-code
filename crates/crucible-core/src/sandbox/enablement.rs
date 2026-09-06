//! Host-owned interactive enablement, sampled when a process is prepared.
//!
//! A control changes future preparations only. Every prepared process retains
//! its immutable policy, digest and audit facts; toggling cannot change a
//! running process or lift a project requirement.

use std::sync::atomic::{AtomicBool, Ordering};

use super::SandboxPolicyError;

/// Shared host control for future sandbox preparations.
///
/// Keep this handle in trusted composition code. Tools and descendants receive
/// a policy snapshot, never authority to change this control.
#[derive(Debug, Default)]
pub struct SandboxEnablement {
    enabled: AtomicBool,
    required: bool,
}

impl SandboxEnablement {
    /// Establishes the user choice and any project requirement.
    /// A requirement always wins over a disabled initial choice.
    #[must_use]
    pub const fn new(enabled: bool, required: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled || required),
            required,
        }
    }

    /// The choice a new process preparation should use.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Whether a project requirement prevents a host interactive opt-out.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.required
    }

    /// Applies an explicit host choice to subsequent preparations.
    ///
    /// # Errors
    ///
    /// Refuses disabling a requirement established by project configuration.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), SandboxPolicyError> {
        if self.required && !enabled {
            return Err(SandboxPolicyError::ConfinementDisabled);
        }
        self.enabled.store(enabled, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_choice_cannot_disable_a_project_requirement() {
        let required = SandboxEnablement::new(false, true);
        assert!(required.enabled());
        assert_eq!(
            required.set_enabled(false),
            Err(SandboxPolicyError::ConfinementDisabled)
        );
        assert!(required.enabled());
        assert!(required.set_enabled(true).is_ok());
    }

    #[test]
    fn a_control_changes_only_future_samples() {
        let control = SandboxEnablement::new(false, false);
        let before = control.enabled();
        control.set_enabled(true).unwrap();
        assert!(!before);
        assert!(control.enabled());
        control.set_enabled(false).unwrap();
        assert!(!control.enabled());
    }
}
