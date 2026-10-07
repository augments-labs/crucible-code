//! The confinement this machine can enforce, and the processes it runs.
//!
//! [`crucible_sandbox`] states what a backend has to satisfy; everything here
//! is one machine's answer to it. Linux uses a verified system Bubblewrap
//! executable, macOS uses the system Seatbelt launcher, and Windows uses its
//! provisioned dedicated account. Disabled policies use the same lifecycle
//! wrapper and report that execution is unconfined; enabled policies refuse an
//! unavailable native backend.
//!
//! [`conformance`] is published alongside them: it is what a backend outside
//! this tree has to answer before it may be selected here, and it asks the
//! native backends below exactly the same questions.
//!
//! Nothing here decides that a command should run. A binary composition root
//! hands [`LocalSandbox`] to whatever asks for confinement, which is what lets
//! the tool that runs a shell command name no operating system at all.

mod local;
#[cfg(any(target_os = "linux", target_os = "macos", test))]
mod network;
#[cfg(not(any(target_os = "linux", target_os = "macos", test)))]
#[path = "network_unavailable.rs"]
mod network;
mod platform;
pub(crate) mod process;
mod redaction;

pub mod conformance;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

// How the Windows broker is started holds no Windows call, so its tests run on
// every platform.
#[cfg(any(target_os = "windows", test))]
#[path = "windows/helper.rs"]
mod windows_helper;

pub use local::{LocalSandbox, ObservedVersion, SandboxObservation};

/// Settles, as the process starts, where its sandbox broker is looked for.
///
/// The broker is the file of the broker's name in the directory of the
/// executable this process was started from, every link in that path resolved
/// now, and the file of that name in the directory above it after that. A
/// managed install's release directory is therefore the one this process
/// belongs to, whichever release its installer makes active later. Called
/// before anything else runs; a process that never calls it settles the
/// directory at its first lookup instead. A path that could not be resolved
/// is held too, and every lookup reports it as its refusal.
pub fn hold_broker_directory() {
    #[cfg(target_os = "linux")]
    linux::hold_broker_directory();
}

#[cfg(test)]
mod sample;

/// Sorts at most the remaining scan budget plus one entry to detect overflow.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn directory_entries(
    entries: impl Iterator<Item = std::io::Result<std::fs::DirEntry>>,
    remaining: usize,
) -> std::io::Result<Vec<std::fs::DirEntry>> {
    let mut entries = entries
        .take(remaining.saturating_add(1))
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    #[test]
    fn directory_scans_consume_only_the_remaining_budget_and_one_overflow_entry() {
        let sample = crate::sample::Sample::new("sandbox-directory-budget");
        for number in 0..8 {
            sample.write(&format!("entry-{number}"), "fixture");
        }
        for remaining in [0, 3, 8] {
            let consumed = std::cell::Cell::new(0);
            let entries = std::fs::read_dir(sample.root()).unwrap().inspect(|_| {
                consumed.set(consumed.get() + 1);
            });
            let entries = super::directory_entries(entries, remaining).unwrap();
            assert_eq!(consumed.get(), (remaining + 1).min(8));
            assert_eq!(entries.len(), consumed.get());
            let names: Vec<_> = entries.iter().map(std::fs::DirEntry::file_name).collect();
            assert!(names.is_sorted());
        }
    }
}
