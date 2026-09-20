//! What `--sandbox` prints.
//!
//! The confinement a command would be run under, written out before any
//! command is run under it. The flag exists for the question nobody can
//! currently answer from outside crucible — *is the thing that says it confines
//! actually confining, and what did it settle for where it could not?* — and
//! that question is only worth asking if it can be asked without starting
//! anything. So [`confinement`] prepares a session and reads it; nothing is
//! materialized, no program is spawned, and the session is dropped where the
//! report is built.
//!
//! Every path in the report is a digest. The record this is written from
//! redacts them at the source, and that is the right bargain rather than an
//! inconvenience: this is a listing people paste into an issue, and a home
//! directory is a name.
//!
//! Built as one string and written once, like the extension listing beside it:
//! by the time this runs there is no session, no screen and nothing to protect.
//!
//! And what `/sandbox enable` and `/sandbox disable` decide, in [`choosing`]:
//! whether the choice is allowed, whether this machine can enforce it, whether
//! it could be written down, and only then the switch itself.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crucible_config::{Home, Settings};
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxCleanup,
    SandboxEnablement, SandboxError, SandboxFeature, SandboxInspection, SandboxManifest,
    SandboxPlanInspection, SandboxRequest, SandboxResourceLimits, SandboxService as _,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::{Ancestry, SandboxId, ToolId};
use crucible_workspace::Workspace;

use crate::{AppError, remember};

/// How far the backend got when it was asked about this workspace.
///
/// Three answers rather than a `Result`, because the middle one is the
/// interesting one and it is not a failure to probe: a backend that is present,
/// says what it can hold, and still will not take this policy is exactly what
/// somebody reaches for this flag to see. Folding it into either neighbour
/// would lose the capability matrix that explains it.
#[derive(Debug)]
pub enum Probe<'a> {
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

/// The confinement a command started in `here` would run under, as a report.
///
/// The workspace is opened because the confinement is made out of it: the roots
/// a command may reach are this checkout's roots, and a report assembled
/// without one would be describing a sandbox nobody is going to get. Nothing
/// beyond that is built — no credential is read, no session file is written, no
/// manifest is materialized and no program is spawned. The sandbox session
/// exists to be asked what it negotiated and is dropped on the way out.
///
/// A backend that will not take this policy is not a failure. It is the
/// answer, so it is part of the report with everything that explains it;
/// somebody asking because their commands are being refused would learn nothing
/// from the same refusal arriving again as an error.
///
/// # Errors
///
/// The directory cannot be worked in, crucible's files cannot be read, or no
/// policy can be built for the directory at all.
pub fn confinement(here: &Path, home: &Home) -> Result<String, AppError> {
    let workspace = Workspace::open(here)?;
    let settings = Settings::read(home, workspace.root())?;
    // Widened the way a run widens it, and for the same reason the run gives:
    // the extra directories are part of the reach, so a report that left them
    // out would understate what a command can touch.
    let workspace = workspace.reaching(settings.extra_directories())?;

    let service = LocalSandbox::new();
    let probed = service.probe();
    let policy = settings.sandbox().policy(&workspace)?;
    let prepared = service.prepare(SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        // The call this policy would be built for. Nothing is called: the
        // request needs a name and this is the honest one.
        ToolId::new("sandbox"),
        policy,
        SandboxManifest::empty(),
    ));

    let probe = match (&prepared, &probed) {
        (Ok(session), _) => Probe::Prepared(session.inspection()),
        (Err(why), Ok((backend, capabilities))) => Probe::Refused {
            backend,
            capabilities,
            why,
        },
        (Err(_), Err(why)) => Probe::Absent(why),
    };
    Ok(report(workspace.root(), settings.sandbox_enabled(), &probe))
}

/// The report, as one block of text ending in a newline.
///
/// `at` is the workspace root, and it is the one path printed unredacted: it is
/// the directory the person running this is standing in, so it tells them which
/// checkout they asked about rather than telling anybody something they did not
/// already have.
pub fn report(at: &Path, enabled: bool, probe: &Probe<'_>) -> String {
    let mut said = String::new();
    let _ = writeln!(
        said,
        "sandbox {} in {}",
        if enabled { "enabled" } else { "disabled" },
        at.display()
    );

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

/// Turns confinement on or off for the commands and hosted processes
/// prepared from here on, and writes the choice down in `file`.
///
/// In this order, and stopping at the first that fails: a project that
/// requires confinement cannot have it turned off; turning it on needs a
/// backend that can enforce this workspace's policy; the choice is written
/// down; and only then does the running session change. A choice that could
/// not be saved is therefore one that was not made, which is what keeps this
/// run and the next one agreeing about what is confined.
///
/// # Errors
///
/// The sentence for whichever of those stopped it, ready to follow
/// "sandbox unchanged:".
pub fn choosing(
    settings: &Settings,
    workspace: &Workspace,
    file: &Path,
    enabled: bool,
) -> Result<(), String> {
    choose(
        &settings.sandbox().enablement(),
        enabled,
        || enforceable(settings, workspace),
        || remember::sandboxing(file, enabled).map_err(|problem| problem.to_string()),
    )
}

/// The order [`choosing`] decides in, over checks handed in so that each can
/// be failed without a backend or a disk.
fn choose(
    control: &SandboxEnablement,
    enabled: bool,
    verify: impl FnOnce() -> Result<(), String>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !enabled && control.required() {
        return Err("project configuration requires confinement".into());
    }
    if enabled {
        verify()?;
    }
    save()?;
    control
        .set_enabled(enabled)
        .map_err(|problem| problem.to_string())
}

/// Whether this machine can enforce the policy `settings` asks for here.
fn enforceable(settings: &Settings, workspace: &Workspace) -> Result<(), String> {
    let policy = settings
        .sandbox()
        .enforcing_policy(workspace)
        .map_err(|problem| problem.to_string())?;
    let service = LocalSandbox::new();
    // Preparation checks exact backend capability and filesystem policy. It
    // does not materialize or start a user command; dropping releases admission.
    let prepared = service
        .prepare(SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("sandbox-inspection"),
            policy,
            SandboxManifest::empty(),
        ))
        .map_err(|problem| problem.to_string())?;
    drop(prepared);
    Ok(())
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
    if let Some(reason) = inspection.disabled_reason() {
        let _ = writeln!(said, "  disabled  {reason}");
    }
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
    let _ = writeln!(said, "  enabled   {}", plan.enabled());
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
            crucible_sandbox::SandboxNetworkInspection::Closed => String::new(),
            crucible_sandbox::SandboxNetworkInspection::Domains {
                allowed,
                denied,
                local_binding,
                unix_sockets,
            } => format!(
                ", {allowed} allowed, {denied} denied, local binding {}, {unix_sockets} Unix sockets",
                yes(local_binding)
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

/// A wall-clock span without rounding, and what it is a span of.
fn wall(span: Duration, of: &str) -> String {
    if span.subsec_nanos() == 0 {
        return format!("{} {of}", seconds(span.as_secs()));
    }

    // Integer fields preserve both nanoseconds and large durations; converting
    // to floating-point seconds could round a stated ceiling to another value.
    let fraction = format!("{:09}", span.subsec_nanos());
    format!(
        "{}.{}s {of}",
        span.as_secs(),
        fraction.trim_end_matches('0')
    )
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
