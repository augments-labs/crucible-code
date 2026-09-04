//! What `--sandbox` prints.
//!
//! The confinement a command would be run under, written out before any
//! command is run under it. The flag exists for the question nobody can
//! currently answer from outside crucible — *is the thing that says it confines
//! actually confining, and what did it settle for where it could not?* — and
//! that question is only worth asking if it can be asked without starting
//! anything. So a session is prepared and read; nothing is materialized, no
//! program is spawned, and the session is dropped where the report is built.
//!
//! Every path in the report is a digest. The record this is written from
//! redacts them at the source, and that is the right bargain rather than an
//! inconvenience: this is a listing people paste into an issue, and a home
//! directory is a name.
//!
//! Built as one string and written once, like the extension listing beside it:
//! by the time this runs there is no session, no screen and nothing to protect.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crucible_core::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxCleanup, SandboxError,
    SandboxFeature, SandboxInspection, SandboxMode, SandboxPlanInspection, SandboxResourceLimits,
};

/// How far the backend got when it was asked about this workspace.
///
/// Three answers rather than a `Result`, because the middle one is the
/// interesting one and it is not a failure to probe: a backend that is present,
/// says what it can hold, and still will not take this policy is exactly what
/// somebody reaches for this flag to see. Folding it into either neighbour
/// would lose the capability matrix that explains it.
pub(crate) enum Probe<'a> {
    /// A session was negotiated, and this is what it settled on.
    Prepared(&'a SandboxInspection),
    /// The backend answered, but would not take this workspace's policy.
    Refused {
        /// Who answered.
        backend: &'a SandboxBackendIdentity,
        /// What it said it could hold, which is what explains the refusal.
        capabilities: &'a SandboxCapabilities,
        /// What it refused, in its own words.
        why: &'a SandboxError,
    },
    /// Nothing answered, so there is no matrix and no plan to print.
    Absent(&'a SandboxError),
}

/// The report, as one block of text ending in a newline.
///
/// `at` is the workspace root, and it is the one path printed unredacted: it is
/// the directory the person running this is standing in, so it tells them which
/// checkout they asked about rather than telling anybody something they did not
/// already have.
pub(crate) fn report(at: &Path, asked: SandboxMode, probe: &Probe<'_>) -> String {
    let mut said = String::new();
    let _ = writeln!(said, "sandbox {} in {}", asked.as_str(), at.display());

    match probe {
        Probe::Prepared(inspection) => {
            backend(&mut said, inspection.backend());
            matrix(&mut said, inspection.capabilities());
            settled(&mut said, inspection);
        }
        Probe::Refused {
            backend: identity,
            capabilities,
            why,
        } => {
            backend(&mut said, identity);
            matrix(&mut said, capabilities);
            // The matrix above is the explanation, so the refusal is printed
            // after it rather than at the top: a feature this backend calls
            // unsupported and the policy asks to be enforced is the whole of
            // most refusals, and it reads as an answer only in that order.
            let _ = writeln!(said, "\nno command could be run here\n  {why}");
        }
        Probe::Absent(why) => {
            let _ = writeln!(said, "\nno sandbox backend answered\n  {why}");
        }
    }

    said
}

/// Who is doing the confining, and whether crucible measured it.
fn backend(said: &mut String, identity: &SandboxBackendIdentity) {
    let _ = writeln!(
        said,
        "  backend   {} {}, {}",
        identity.id().as_str(),
        identity.version(),
        identity.provenance().as_str(),
    );
    // A backend crucible took a digest over is one it can say it is still
    // looking at; one it did not is not an accusation, it is a fact about how
    // this backend was found, and leaving the line out would read like the
    // digest matched.
    let _ = writeln!(
        said,
        "  build     {}",
        match identity.digest() {
            Some(digest) => hex(digest),
            None => String::from("not measured"),
        }
    );
}

/// Every feature, and how strongly this backend claims it.
///
/// All of them, including the ones this policy never asks for. The matrix is
/// what makes a later "enforced" claim mean anything: a report that printed
/// only the claims a passing policy relied on would be a report that could not
/// have said no, and this flag exists to be able to say no.
fn matrix(said: &mut String, capabilities: &SandboxCapabilities) {
    let _ = writeln!(said, "\nwhat this backend can hold:");
    for (feature, claim) in capabilities.iter() {
        let _ = writeln!(said, "  {:<21} {}", feature.as_str(), claim.as_str());
    }
}

/// What a command would actually run under, and what it was asked to be.
fn settled(said: &mut String, inspection: &SandboxInspection) {
    let _ = writeln!(said, "\nwhat a command would run under:");
    plan(said, inspection.plan(), inspection.capabilities());

    let _ = writeln!(
        said,
        "  confined  {}",
        if inspection.confined() { "yes" } else { "no" },
    );
    // Said whether or not anything was given up. "nothing given up" is the
    // sentence somebody running with confinement required needs to read, and
    // a line that appears only on the bad day is one whose absence proves
    // nothing.
    let _ = writeln!(
        said,
        "  gave up   {}",
        inspection.degradation().unwrap_or("nothing"),
    );
    let _ = writeln!(
        said,
        "  cleanup   {}",
        match inspection.cleanup() {
            // The usual answer here, and not a complaint: this session was
            // prepared to be read and is dropped as this line is written, so
            // there is nothing yet to have finished cleaning.
            SandboxCleanup::Pending => "pending; nothing was run and nothing was staged",
            SandboxCleanup::Complete => "complete",
            SandboxCleanup::Failed => "could not be confirmed",
        }
    );
    let _ = writeln!(said, "  policy    {}", hex(inspection.policy_digest()));
    let _ = writeln!(said, "  manifest  {}", hex(inspection.manifest_digest()));

    // Only where the two differ. On this flag they never do — the policy is
    // built here and handed straight over — but the record carries both, and a
    // report that printed only the effective half would be unable to show a
    // narrowing on the day something starts narrowing.
    if inspection.requested_policy_digest() != inspection.policy_digest() {
        let _ = writeln!(
            said,
            "\nwhat was asked for, which is not what it settled on:"
        );
        plan(said, inspection.requested_plan(), inspection.capabilities());
        let _ = writeln!(
            said,
            "  policy    {}",
            hex(inspection.requested_policy_digest())
        );
    }
}

/// One plan: reach, network, ceilings and what is staged into it.
fn plan(said: &mut String, plan: &SandboxPlanInspection, capabilities: &SandboxCapabilities) {
    let _ = writeln!(said, "  mode      {}", plan.mode().as_str());
    let _ = writeln!(said, "  cwd       {}", hex(plan.working_directory()));

    // The digest is not a path anybody can read back, which is the point; the
    // access and the reason are the part a person judges, and they are what a
    // wrong reach looks wrong in.
    let _ = writeln!(
        said,
        "  reach     {}",
        match plan.roots().len() {
            0 => String::from("nowhere"),
            1 => String::from("1 place, named by digest"),
            many => format!("{many} places, named by digest"),
        }
    );
    for root in plan.roots() {
        let _ = writeln!(
            said,
            "    {:<12}{:<20}{}",
            root.access().as_str(),
            root.provenance().as_str(),
            hex(root.identity()),
        );
    }
    let _ = writeln!(
        said,
        "  hidden    {}",
        match plan.unreadable_patterns() {
            1 => String::from("1 pattern"),
            many => format!("{many} patterns"),
        }
    );

    let network = plan.network();
    let _ = writeln!(
        said,
        "  network   {}{}",
        network.as_str(),
        match network {
            crucible_core::SandboxNetworkInspection::Closed => String::new(),
            crucible_core::SandboxNetworkInspection::Exact { .. } => format!(
                ", {} endpoints, dns {}, forwarding {}",
                network.endpoints(),
                yes(network.dns()),
                yes(network.forwarding()),
            ),
        }
    );

    ceilings(said, plan.limits(), capabilities);

    let _ = writeln!(
        said,
        "  staged    {}",
        match plan.manifest_entries() {
            0 => String::from("nothing"),
            1 => String::from("1 entry"),
            many => format!("{many} entries"),
        }
    );
    let _ = writeln!(said, "  outlives  {}", yes(plan.persistent()));
    let _ = writeln!(said, "  snapshots {}", yes(plan.snapshots()));
}

/// Each ceiling this plan states, beside the claim it rests on.
///
/// The pairing is the whole point of the section. A number on its own says what
/// was asked for; a number the backend only observes is a number a command can
/// walk past while a supervisor writes it down, and the two read identically
/// until they are printed on one line.
fn ceilings(said: &mut String, limits: SandboxResourceLimits, capabilities: &SandboxCapabilities) {
    let stated: Vec<(String, SandboxFeature)> = [
        (
            limits
                .cpu_seconds
                .map(|count| format!("cpu {}", seconds(count))),
            SandboxFeature::CpuLimit,
        ),
        (
            limits
                .memory_bytes
                .map(|count| format!("memory {}", bytes(count))),
            SandboxFeature::MemoryLimit,
        ),
        (
            limits
                .disk_bytes
                .map(|count| format!("disk {}", bytes(count))),
            SandboxFeature::DiskLimit,
        ),
        (
            limits.processes.map(|count| format!("{count} processes")),
            SandboxFeature::ProcessLimit,
        ),
        (
            limits.open_files.map(|count| format!("{count} open files")),
            SandboxFeature::OpenFileLimit,
        ),
        (
            limits
                .outbound_bytes
                .map(|count| format!("{} out", bytes(count))),
            SandboxFeature::OutboundByteLimit,
        ),
        (
            limits
                .output_bytes
                .map(|count| format!("{} captured", bytes(count))),
            SandboxFeature::OutputLimit,
        ),
        (
            limits
                .concurrent_commands
                .map(|count| format!("{count} at once")),
            SandboxFeature::ConcurrencyLimit,
        ),
        (
            limits.command_time.map(|span| wall(span, "per command")),
            SandboxFeature::CommandTimeLimit,
        ),
        (
            limits.session_time.map(|span| wall(span, "per session")),
            SandboxFeature::SessionTimeLimit,
        ),
        (
            limits.cost_micros.map(|cost| format!("{cost} cost micros")),
            SandboxFeature::CostLimit,
        ),
    ]
    .into_iter()
    .filter_map(|(stated, feature)| stated.map(|stated| (stated, feature)))
    .collect();

    if stated.is_empty() {
        // A policy with no ceilings at all is a real configuration and not an
        // error, and it is worth a word rather than a blank: nothing here
        // bounds a runaway.
        let _ = writeln!(said, "  ceilings  none");
        return;
    }

    let _ = writeln!(said, "  ceilings");
    for (written, feature) in stated {
        let claim = capabilities.claim(feature);
        let _ = writeln!(
            said,
            "    {written:<24}{}{}",
            claim.as_str(),
            match claim {
                // Named where it matters and nowhere else. "observed" is a word
                // somebody could read as a weaker kind of ceiling rather than
                // as no ceiling, and this is the one place to settle that.
                SandboxCapability::Observed => ", so it is recorded rather than imposed",
                SandboxCapability::Unsupported => ", so this number does nothing",
                SandboxCapability::Enforced => "",
            }
        );
    }
}

/// A wall-clock span, in whole seconds, and what it is a span of.
fn wall(span: Duration, of: &str) -> String {
    format!("{} {of}", seconds(span.as_secs()))
}

/// Whole seconds, in minutes where they divide evenly into them.
fn seconds(count: u64) -> String {
    match count {
        count if count >= 60 && count % 60 == 0 => format!("{}m", count / 60),
        count => format!("{count}s"),
    }
}

/// A byte count, in the largest binary unit it divides evenly into.
///
/// Evenly or not at all, because a ceiling is a number somebody wrote down and
/// a rounded one is a different number: a report that said "1 MiB" over
/// 1000000 would be a report whose reader could not tell which was configured.
fn bytes(count: u64) -> String {
    for (unit, scale) in [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)] {
        if count >= scale && count.is_multiple_of(scale) {
            return format!("{} {unit}", count / scale);
        }
    }

    format!("{count} bytes")
}

/// A flag, as the answer to the question its label asks.
const fn yes(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

/// A redacted fact, named the way every other digest crucible prints is.
///
/// The algorithm is spelled out rather than left to be inferred, because these
/// are digests somebody compares against one they took themselves and the two
/// have to be the same kind of thing before they can disagree.
fn hex(digest: [u8; 32]) -> String {
    let mut out = String::with_capacity(digest.len() * 2 + 7);
    out.push_str("sha256:");
    for byte in digest {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests;
