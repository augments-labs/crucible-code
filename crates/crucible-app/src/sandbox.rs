//! What `crucible sandbox inspect` and `--sandbox` print.
//!
//! The confinement a command would be run under, written out before any
//! command is run under it. The command exists for the question nobody can
//! currently answer from outside crucible — *is the thing that says it confines
//! actually confining, and what did it settle for where it could not?* — and
//! that question is only worth asking if it can be asked without starting
//! anything. So [`inspection`] observes rather than prepares or probes: the
//! backend a preparation would use is found by the trust checks preparation
//! applies and measured by digest, its declared matrix is negotiated against
//! this directory's policy, and no process is started, no manifest is
//! materialized and no credential is read. Proving a backend can mean starting
//! it, so what only running it could tell — a version only it reports, that it
//! starts on this host — is reported as unverified, with the reason, rather
//! than learned.
//!
//! Every path in the report is a digest. The record this is written from
//! redacts them at the source, and that is the right bargain rather than an
//! inconvenience: this is a listing people paste into an issue, and a home
//! directory is a name.
//!
//! One [`Observed`] is written two ways. [`Observed::human`] is one string
//! written once, like the extension listing beside it: by the time this runs
//! there is no session, no screen and nothing to protect.
//! [`Observed::json`] is the `crucible_client_api` inspection document a
//! script reads, translated field by field by hand.
//!
//! And what `/sandbox enable` and `/sandbox disable` decide, in [`choosing`]:
//! whether the choice is allowed, whether this machine can enforce it, whether
//! it could be written down, and only then the switch itself.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::inspection::{
    Backend, BackendVersion, Capability, Ceiling, Claim, Digest, Inspected, Inspection, Network,
    Plan, Requirement, Root, Unit,
};
use crucible_client_api::{Name, Refusal, Text};
use crucible_config::{Home, Settings};
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCapability, SandboxEnablement,
    SandboxError, SandboxFeature, SandboxInspection, SandboxManifest, SandboxNetworkInspection,
    SandboxPlanInspection, SandboxPolicy, SandboxRequest, SandboxResourceLimits, SandboxService,
    confined_inspection, plan_inspection, unconfined_inspection,
};
use crucible_sandbox_local::{LocalSandbox, ObservedVersion, SandboxObservation};
use crucible_types::{Ancestry, SandboxId, ToolId};
use crucible_workspace::Workspace;

use crate::{AppError, remember};

/// A sandbox service that can say what would confine a request without
/// starting anything.
///
/// The service itself rather than a finder beside it, because the service is
/// what an inspection must be shown never to drive: whatever observes could
/// also probe and prepare, and the tests hold [`inspection`] to doing neither.
pub trait Observing: SandboxService {
    /// What would confine `request`, found without starting a process,
    /// materializing anything or reading a credential.
    ///
    /// # Errors
    ///
    /// No backend would be found, in the words preparation would use.
    fn observe(&self, request: &SandboxRequest) -> Result<SandboxObservation, SandboxError>;
}

impl Observing for LocalSandbox {
    fn observe(&self, request: &SandboxRequest) -> Result<SandboxObservation, SandboxError> {
        Self::observe(request)
    }
}

/// The reason a preparation records for a policy that turns confinement off.
const DISABLED: &str = "sandbox disabled by effective policy";

/// What an inspection of one directory found.
#[derive(Debug)]
pub struct Observed {
    /// The workspace root, the one path either form names.
    at: PathBuf,
    /// Whether configuration turns confinement on.
    enabled: bool,
    /// Whether the project's configuration forbids turning it off.
    required: bool,
    /// The request a command here would be prepared from.
    request: SandboxRequest,
    /// The backend that would confine it, or why none was found.
    found: Result<Found, SandboxError>,
}

/// A backend found, and what a preparation that it accepted would record.
#[derive(Debug)]
struct Found {
    observation: SandboxObservation,
    /// What a command would run under, where the backend takes the policy.
    settled: Option<SandboxInspection>,
    /// Why the record could not be made, where the matrix took the policy and
    /// the record's own rule did not.
    unrecorded: Option<SandboxError>,
}

impl Found {
    /// `observation`, settled against `request` the way a preparation settles
    /// a backend that negotiated: the record's own rule decides `confined`.
    fn settled(observation: SandboxObservation, request: &SandboxRequest) -> Self {
        if observation.refusal().is_some() {
            return Self {
                observation,
                settled: None,
                unrecorded: None,
            };
        }
        let version = match observation.version() {
            ObservedVersion::Stated(version) => version,
            ObservedVersion::Unverified(_) => "unverified",
        };
        let capabilities = observation.capabilities().clone();
        let recorded = SandboxBackendIdentity::new(
            observation.id().clone(),
            version,
            observation.provenance(),
            observation.digest(),
        )
        .map_err(|_| SandboxError::InvalidInspection)
        .and_then(|identity| {
            if request.policy().enabled() {
                confined_inspection(identity, capabilities, request)
            } else {
                unconfined_inspection(identity, capabilities, request, DISABLED)
            }
        });
        let (settled, unrecorded) = match recorded {
            Ok(inspection) => (Some(inspection), None),
            Err(why) => (None, Some(why)),
        };
        Self {
            observation,
            settled,
            unrecorded,
        }
    }

    /// Why no command could be run, where none could.
    fn refusal(&self) -> Option<&SandboxError> {
        self.observation.refusal().or(self.unrecorded.as_ref())
    }
}

/// What would confine a command started in `here`, found on this machine
/// without starting anything.
///
/// The workspace is opened because the confinement is made out of it: the
/// roots a command may reach are this checkout's roots, and a report assembled
/// without one would be describing a sandbox nobody is going to get. Beyond
/// that, crucible's configuration is read and nothing else: no credential, no
/// session file, no manifest and no program.
///
/// A backend that will not take this policy, and no backend at all, are not
/// failures. They are the answer, so they are part of the report with
/// everything that explains them.
///
/// # Errors
///
/// The directory cannot be worked in, crucible's files cannot be read, no
/// policy can be built for the directory at all.
pub fn inspection(here: &Path, home: &Home) -> Result<Observed, AppError> {
    inspecting(&LocalSandbox::new(), here, home)
}

/// [`inspection`], asked of a service handed in so that one which counts what
/// it is asked can stand in for this machine's.
fn inspecting(service: &dyn Observing, here: &Path, home: &Home) -> Result<Observed, AppError> {
    let workspace = Workspace::open(here)?;
    let settings = Settings::read(home, workspace.root())?;
    // Widened the way a run widens it, and for the same reason the run gives:
    // the extra directories are part of the reach, so a report that left them
    // out would understate what a command can touch.
    let workspace = workspace.reaching(settings.extra_directories())?;
    let policy = settings.sandbox().policy(&workspace)?;
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        // The call this policy would be built for. Nothing is called: the
        // request needs a name and this is the honest one.
        ToolId::new("sandbox"),
        policy,
        SandboxManifest::empty(),
    );
    let found = service
        .observe(&request)
        .map(|observation| Found::settled(observation, &request));
    Ok(Observed {
        at: workspace.root().to_owned(),
        enabled: settings.sandbox_enabled(),
        required: settings.sandbox().enablement().required(),
        request,
        found,
    })
}

impl Observed {
    /// The report, as one block of text ending in a newline.
    ///
    /// The workspace root is the one path printed unredacted: it is the
    /// directory the person running this is standing in, so it tells them
    /// which checkout they asked about rather than telling anybody something
    /// they did not already have.
    #[must_use]
    pub fn human(&self) -> String {
        let mut said = String::new();
        let _ = writeln!(
            said,
            "sandbox {} in {}",
            if self.enabled { "enabled" } else { "disabled" },
            self.at.display()
        );
        let _ = writeln!(
            said,
            "  mode      {}",
            if self.required {
                "required by project configuration"
            } else {
                "optional"
            }
        );

        let found = match &self.found {
            Ok(found) => found,
            Err(why) => {
                // No matrix, because there is nobody whose claims those would
                // be. An empty one would read as a backend that holds nothing,
                // which is a different and much worse thing to be told.
                let _ = writeln!(said, "\nno sandbox backend was found\n  {why}");
                return said;
            }
        };
        backend(&mut said, &found.observation);
        matrix(&mut said, found.observation.capabilities());
        match (&found.settled, found.refusal()) {
            (Some(inspection), _) => settled(&mut said, inspection),
            (None, why) => {
                let _ = writeln!(said, "\nwhat was asked for:");
                plan(
                    &mut said,
                    &plan_inspection(self.request.requested_policy(), self.request.manifest()),
                    found.observation.capabilities(),
                );
                // The matrix above is the explanation, so the refusal is
                // printed after it rather than at the top: a feature this
                // backend calls unsupported and the policy asks to be enforced
                // is the whole of most refusals, and it reads as an answer only
                // in that order.
                let _ = write!(said, "\nno command could be run here");
                match why {
                    Some(why) => {
                        let _ = writeln!(said, "\n  {why}");
                    }
                    None => said.push('\n'),
                }
            }
        }
        if let Some(unchecked) = found.observation.unchecked() {
            let _ = writeln!(
                said,
                "\nnot checked, since checking would start something:\n  {unchecked}"
            );
        }
        said
    }

    /// The report as `crucible sandbox inspect --json` writes it: one JSON
    /// document on one line, ending in a newline.
    ///
    /// # Errors
    ///
    /// [`Unwritten`] for a report the document cannot carry: a word from the
    /// sandbox that is not a name, or a list over the contract's bound.
    pub fn json(&self) -> Result<Vec<u8>, Unwritten> {
        Ok(self.contract()?.encode()?)
    }

    /// The report, as the document's value.
    fn contract(&self) -> Result<Inspection, Refusal> {
        let found = self.found.as_ref().ok();
        let capabilities = found.map(|found| found.observation.capabilities());
        let manifest = self.request.manifest();
        let requested = self.request.requested_policy();
        let effective = self.request.policy();
        let refusal = match &self.found {
            Ok(found) => found.refusal(),
            Err(why) => Some(why),
        };
        Ok(Inspection::Inspected(Box::new(Inspected {
            enabled: self.enabled,
            mode: if self.required {
                Requirement::Required
            } else {
                Requirement::Optional
            },
            requested: drafted(
                &plan_inspection(requested, manifest),
                requested.digest(),
                capabilities,
            )?,
            effective: drafted(
                &plan_inspection(effective, manifest),
                effective.digest(),
                capabilities,
            )?,
            backend: found
                .map(|found| described(&found.observation))
                .transpose()?,
            unchecked: found
                .and_then(|found| found.observation.unchecked())
                .map(Text::cut),
            refusal: refusal.map(|why| Text::cut(&why.to_string())),
            confined: found
                .and_then(|found| found.settled.as_ref())
                .is_some_and(SandboxInspection::confined),
        })))
    }
}

/// Why no report could be made, as far as the failed document says it.
#[derive(Debug, Clone, Copy)]
pub enum Unmade<'a> {
    /// The directory crucible was started in could not be read.
    Here,
    /// [`inspection`] refused.
    Inspecting(&'a AppError),
    /// A report was made and the document could not carry it.
    Unwritten(&'a Unwritten),
}

impl Unmade<'_> {
    /// The step that stopped, in words that name no file.
    ///
    /// The document is pasted where the report would have been, so it keeps
    /// the report's promise to carry no path. A workspace or configuration
    /// error leads with the file it is about, so for those the document says
    /// which step stopped and leaves the file to standard error, where the
    /// run ends with the whole sentence. A policy refusal and an unwritable
    /// report name no file, so they are said as they stand.
    fn said(self) -> String {
        match self {
            Self::Here => "the directory crucible was started in could not be read".to_owned(),
            Self::Inspecting(AppError::Workspace(_)) => {
                "this directory is not one crucible can work in; standard error says why".to_owned()
            }
            Self::Inspecting(AppError::Config(_)) => {
                "crucible's configuration could not be read; standard error names the file and why"
                    .to_owned()
            }
            Self::Inspecting(refused @ AppError::Confinement(_)) => refused.to_string(),
            // Nothing else is returned by an inspection today, and a sentence
            // not read here cannot be promised to carry no path.
            Self::Inspecting(_) => {
                "the sandbox inspection could not be made; standard error says why".to_owned()
            }
            Self::Unwritten(unwritten) => unwritten.to_string(),
        }
    }
}

/// The document a failure to inspect at all is written as, naming the step
/// that stopped, so a script reading standard output is never left with
/// nothing.
///
/// # Errors
///
/// [`Unwritten`] if even that could not be framed; the sentence is cut to its
/// bound first, so this is the refusal said rather than one expected.
pub fn failure(problem: Unmade<'_>) -> Result<Vec<u8>, Unwritten> {
    Ok(Inspection::Failed(Text::cut(&problem.said())).encode()?)
}

/// A report that was made and could not be written as its document.
///
/// It carries the contract's refusal, which names a code and nothing of the
/// report, so it can be said wherever the report itself could have been.
#[derive(Debug, thiserror::Error)]
#[error("the sandbox inspection could not be written: {0}")]
pub struct Unwritten(#[from] Refusal);

/// A word the sandbox bounded, as the contract carries it.
fn named(word: &str) -> Result<Name, Refusal> {
    Name::new(word)
}

/// A claim, in the contract's words.
const fn claimed(claim: SandboxCapability) -> Claim {
    match claim {
        SandboxCapability::Enforced => Claim::Enforced,
        SandboxCapability::Observed => Claim::Observed,
        SandboxCapability::Unsupported => Claim::Unsupported,
    }
}

/// Who would confine, in the contract's words.
fn described(observation: &SandboxObservation) -> Result<Backend, Refusal> {
    Ok(Backend {
        name: named(observation.id().as_str())?,
        version: match observation.version() {
            ObservedVersion::Stated(version) => BackendVersion::Stated(named(version)?),
            ObservedVersion::Unverified(why) => BackendVersion::Unverified(Text::cut(why)),
        },
        provenance: named(observation.provenance().as_str())?,
        build: observation.digest().map(Digest),
        capabilities: observation
            .capabilities()
            .iter()
            .map(|(feature, claim)| {
                Ok(Capability {
                    feature: named(feature.as_str())?,
                    claim: claimed(claim),
                })
            })
            .collect::<Result<_, Refusal>>()?,
    })
}

/// One plan, in the contract's words: at most [`ITEMS`] roots and a count of
/// the rest, and each ceiling with the claim it rests on, unsupported where no
/// backend was found to hold it.
fn drafted(
    plan: &SandboxPlanInspection,
    policy: [u8; 32],
    capabilities: Option<&SandboxCapabilities>,
) -> Result<Plan, Refusal> {
    let roots = plan
        .roots()
        .iter()
        .take(ITEMS)
        .map(|root| {
            Ok(Root {
                access: named(root.access().as_str())?,
                provenance: named(root.provenance().as_str())?,
                identity: Digest(root.identity()),
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let ceilings = stated(plan.limits())
        .into_iter()
        .map(|ceiling| {
            let (amount, nanos, unit) = match ceiling.amount {
                Amount::Span(span) => (span.as_secs(), span.subsec_nanos(), Unit::Seconds),
                Amount::Bytes(count) => (count, 0, Unit::Bytes),
                Amount::Count(count) => (count, 0, Unit::Count),
                Amount::Micros(count) => (count, 0, Unit::Micros),
            };
            Ok(Ceiling {
                feature: named(ceiling.feature.as_str())?,
                amount,
                nanos,
                unit,
                claim: capabilities.map_or(Claim::Unsupported, |held| {
                    claimed(held.claim(ceiling.feature))
                }),
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let count = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
    Ok(Plan {
        enabled: plan.enabled(),
        policy: Digest(policy),
        cwd: Digest(plan.working_directory()),
        omitted: count(plan.roots().len().saturating_sub(roots.len())),
        roots,
        hidden: count(plan.unreadable_patterns()),
        network: match plan.network() {
            SandboxNetworkInspection::Closed => Network::Closed,
            SandboxNetworkInspection::Domains {
                allowed,
                denied,
                local_binding,
                unix_sockets,
            } => Network::Domains {
                allowed: count(allowed),
                denied: count(denied),
                local_binding,
                unix_sockets: count(unix_sockets),
            },
        },
        ceilings,
        commands: Digest(plan.command_policy()),
        staged: count(plan.manifest_entries()),
        persistent: plan.persistent(),
        snapshots: plan.snapshots(),
    })
}

/// Who would be doing the confining, and whether crucible measured it.
fn backend(said: &mut String, observation: &SandboxObservation) {
    let _ = writeln!(
        said,
        "  backend   {}, {}",
        observation.id().as_str(),
        observation.provenance().as_str(),
    );
    let _ = writeln!(
        said,
        "  version   {}",
        match observation.version() {
            ObservedVersion::Stated(version) => String::from(version),
            ObservedVersion::Unverified(why) => format!("unverified; {why}"),
        }
    );
    // A backend crucible took a digest over is one it can say it is still
    // looking at; one it did not is not an accusation, it is a fact about how
    // this backend was found, and leaving the line out would read like the
    // digest matched.
    let _ = writeln!(
        said,
        "  build     {}",
        match observation.digest() {
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
/// have said no, and this command exists to be able to say no.
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
    let _ = writeln!(said, "  policy    {}", hex(inspection.policy_digest()));
    let _ = writeln!(said, "  manifest  {}", hex(inspection.manifest_digest()));

    // Only where the two differ. Here they never do — the policy is built here
    // and handed straight over — but the record carries both, and a report
    // that printed only the effective half would be unable to show a narrowing
    // on the day something starts narrowing.
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
            SandboxNetworkInspection::Closed => String::new(),
            SandboxNetworkInspection::Domains {
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
    let stated = stated(limits);
    if stated.is_empty() {
        // A policy with no ceilings at all is a real configuration and not an
        // error, and it is worth a word rather than a blank: nothing here
        // bounds a runaway.
        let _ = writeln!(said, "  ceilings  none");
        return;
    }

    let _ = writeln!(said, "  ceilings");
    for ceiling in stated {
        let claim = capabilities.claim(ceiling.feature);
        let _ = writeln!(
            said,
            "    {:<24}{}{}",
            ceiling.written,
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

/// How much a ceiling allows, in the unit it is stated in.
#[derive(Debug, Clone, Copy)]
enum Amount {
    /// A span of time.
    Span(Duration),
    /// A number of bytes.
    Bytes(u64),
    /// A number of things.
    Count(u64),
    /// A cost, in millionths of its currency.
    Micros(u64),
}

/// One ceiling a plan states.
struct Stated {
    /// The capability that decides whether the number does anything.
    feature: SandboxFeature,
    /// The number, for the document.
    amount: Amount,
    /// The number, for a person.
    written: String,
}

/// Every ceiling `limits` states, in one table both forms of the report are
/// written from, so the two cannot come to disagree about which exist.
fn stated(limits: SandboxResourceLimits) -> Vec<Stated> {
    let count = |of: Option<u64>, feature, write: fn(u64) -> String| {
        of.map(|count| (feature, Amount::Count(count), write(count)))
    };
    let size = |of: Option<u64>, feature, write: fn(u64) -> String| {
        of.map(|count| (feature, Amount::Bytes(count), write(count)))
    };
    let span = |of: Option<Duration>, feature, per: &str| {
        of.map(|span| (feature, Amount::Span(span), wall(span, per)))
    };
    [
        limits.cpu_seconds.map(|count| {
            (
                SandboxFeature::CpuLimit,
                Amount::Span(Duration::from_secs(count)),
                format!("cpu {}", seconds(count)),
            )
        }),
        size(limits.memory_bytes, SandboxFeature::MemoryLimit, |count| {
            format!("memory {}", bytes(count))
        }),
        size(limits.disk_bytes, SandboxFeature::DiskLimit, |count| {
            format!("disk {}", bytes(count))
        }),
        count(limits.processes, SandboxFeature::ProcessLimit, |count| {
            format!("{count} processes")
        }),
        count(limits.open_files, SandboxFeature::OpenFileLimit, |count| {
            format!("{count} open files")
        }),
        size(
            limits.outbound_bytes,
            SandboxFeature::OutboundByteLimit,
            |count| format!("{} out", bytes(count)),
        ),
        size(limits.output_bytes, SandboxFeature::OutputLimit, |count| {
            format!("{} captured", bytes(count))
        }),
        count(
            limits.concurrent_commands,
            SandboxFeature::ConcurrencyLimit,
            |count| format!("{count} at once"),
        ),
        span(
            limits.command_time,
            SandboxFeature::CommandTimeLimit,
            "per command",
        ),
        span(
            limits.session_time,
            SandboxFeature::SessionTimeLimit,
            "per session",
        ),
        limits.cost_micros.map(|cost| {
            (
                SandboxFeature::CostLimit,
                Amount::Micros(cost),
                format!("{cost} cost micros"),
            )
        }),
    ]
    .into_iter()
    .flatten()
    .map(|(feature, amount, written)| Stated {
        feature,
        amount,
        written,
    })
    .collect()
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
/// Whichever of those stopped it, displayed as a sentence ready to follow
/// "sandbox unchanged:".
pub async fn choosing(
    settings: &Settings,
    workspace: &Workspace,
    file: &Path,
    enabled: bool,
) -> Result<(), Unchanged> {
    choose(
        &settings.sandbox().enablement(),
        enabled,
        enforceable(settings, workspace),
        || remember::sandboxing(file, enabled).map_err(|problem| problem.to_string()),
    )
    .await
}

/// Why [`choosing`] changed nothing: a check stopped the choice, and this is
/// what it said.
#[derive(Debug, thiserror::Error)]
pub enum Unchanged {
    /// A check stopped the choice, and this is what it said.
    #[error("{0}")]
    Stopped(String),
}

/// The order [`choosing`] decides in, over checks handed in so that each can
/// be failed without a backend or a disk. `verify` is awaited only when the
/// choice turns confinement on.
async fn choose(
    control: &SandboxEnablement,
    enabled: bool,
    verify: impl Future<Output = Result<(), Unchanged>>,
    save: impl FnOnce() -> Result<(), String>,
) -> Result<(), Unchanged> {
    if !enabled && control.required() {
        return Err(Unchanged::Stopped(
            "project configuration requires confinement".into(),
        ));
    }
    if enabled {
        verify.await?;
    }
    save().map_err(Unchanged::Stopped)?;
    control
        .set_enabled(enabled)
        .map_err(|problem| Unchanged::Stopped(problem.to_string()))
}

/// Whether this machine can enforce the policy `settings` asks for here.
async fn enforceable(settings: &Settings, workspace: &Workspace) -> Result<(), Unchanged> {
    let policy = settings
        .sandbox()
        .enforcing_policy(workspace)
        .map_err(|problem| Unchanged::Stopped(problem.to_string()))?;
    admitted(&LocalSandbox::new(), policy).await
}

/// Whether `service` would take `policy`, asked of a service handed in so that
/// one which answers only after waiting can stand in for this machine's.
async fn admitted(service: &dyn SandboxService, policy: SandboxPolicy) -> Result<(), Unchanged> {
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
        .await
        .map_err(|problem| Unchanged::Stopped(problem.to_string()))?;
    drop(prepared);
    Ok(())
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
