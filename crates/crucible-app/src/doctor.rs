//! What `crucible doctor` says: whether this host is ready to run a
//! conversation, found without sending, renewing, starting or writing
//! anything.
//!
//! Each check in [`CHECKS`] looks at one thing through the narrowest reader
//! that answers it, and the reader is chosen for what it cannot do. The store
//! is read by [`crucible_auth::Store::inventory`], which holds names and never
//! a key, and protects nothing: every other read tightens the file first, and
//! a report about whether the store is private has to see it as it is. Which
//! credential a provider would use is settled by the rule a launch uses, from
//! those names and from whether an environment variable is set — never by
//! asking for the credential a request would be signed with, because an
//! account login's is renewed on the way out, and renewing is a request. So
//! no login is built here at all: whether a provider signs in by subscription
//! is read off the `/login` rows, which are names, and a test holds the rows
//! to the login registry a launch asks. The sandbox backend is observed the
//! way `crucible sandbox inspect` observes it, so it is measured and not
//! started. A declared MCP server is counted and left alone. No probe, no
//! session directory and no write is reached from here, and the tests beside
//! this stand a counting backend and a server that leaves a mark in front of
//! it to show it.
//!
//! A check that rests on something which failed is not made: it is said to be
//! unavailable, with what it rests on. The configuration is the large case.
//! Where it cannot be read, what reads it — the credentials, the provider, the
//! policy, the servers — is unavailable, and everything that stands on the
//! home directory or the backend alone still runs.
//!
//! Every reason is a sentence that names no path and no value read from the
//! environment, so the report can be pasted where people will read it; a
//! name from a configuration file may appear, with its control and format
//! characters replaced, bounded like every other word here by [`Text`].

use std::path::Path;

use crucible_auth::{Inventory, Store};
use crucible_client_api::doctor::{Check, Report, Status};
use crucible_client_api::inspection::Inspection;
use crucible_client_api::{Name, Text};
use crucible_config::{CheckReport, ConfigError, FileState, Home, Settings};
use crucible_extension::{ExtensionDecision, ExtensionUntrusted, Extensions};
use crucible_sandbox::SandboxPolicy;
use crucible_sandbox_local::{LocalSandbox, ObservedVersion};
use crucible_workspace::{PathError, Workspace};

use crate::AppError;
use crate::providers::{self, Providers, Rows, Served};
use crate::sandbox::{self, Observing};

/// Every check, by the id a script holds on to, in the order they are made.
///
/// The ids are the interface: one is never renamed or reused, and a new check
/// is a new id.
pub const CHECKS: [&str; 12] = [
    "home",
    "workspace",
    "config",
    "private-state",
    "credential-store",
    "credentials",
    "provider",
    "sandbox-backend",
    "sandbox-policy",
    "extension-discovery",
    "extension-trust",
    "mcp",
];

/// What the doctor is handed: everything it reads from outside, as values.
#[derive(Clone, Copy)]
pub struct Host<'a> {
    /// The directory crucible was started in, where it could be read.
    pub here: Option<&'a Path>,
    /// crucible's home directory, or why none was found.
    pub home: Result<&'a Home, &'a ConfigError>,
    /// The environment, asked only whether a key's variable is set.
    pub from: &'a dyn Fn(&str) -> Option<String>,
    /// The version of crucible running, which an extension may need.
    pub running: &'a str,
}

/// Written by hand, since the environment is a lookup and not a value: what
/// it would answer is never printed.
impl std::fmt::Debug for Host<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Host")
            .field("here", &self.here)
            .field("home", &self.home)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

/// This host, examined with this machine's sandbox and this build's rows.
#[must_use]
pub fn examine(host: Host<'_>) -> Report {
    examining(&LocalSandbox::new(), host)
}

/// [`examine`], asked of a sandbox service handed in, so that one which
/// counts can stand in for this machine's.
pub(crate) fn examining(service: &dyn Observing, host: Host<'_>) -> Report {
    let home = host.home.ok();
    let workspace = host.here.map(Workspace::open);
    let opened = workspace.as_ref().and_then(|opened| opened.as_ref().ok());
    let settings = match (home, opened) {
        (Some(home), Some(opened)) => Some(Read::of(home, opened.root())),
        _ => None,
    };
    let readable = settings.as_ref().and_then(Read::settings);
    let rows = Rows::production();
    let inventory = home.map(|home| Store::in_home(home.path()).naming(rows.names()).inventory());
    let registry = providers::providers().map(|registry| registry.snapshot());
    let extensions = home.map(|home| Extensions::discover(home.path()));

    let findings = [
        homed(host.home),
        worked(workspace.as_ref()),
        configured(settings.as_ref()),
        private(home, inventory.as_ref()),
        stocked(inventory.as_ref()),
        credentialed(readable, &registry, host.from, inventory.as_ref(), &rows),
        provided(readable, &registry, host.from, inventory.as_ref(), &rows),
        backed(service, opened),
        confined(service, host.here, home, readable),
        discovered(extensions.as_ref()),
        trusted(home, extensions.as_ref(), host.running),
        served(readable),
    ];
    Report {
        checks: CHECKS
            .into_iter()
            .zip(findings)
            .filter_map(|(id, finding)| {
                Some(Check {
                    id: Name::new(id).ok()?,
                    status: finding.status,
                    reason: said(&finding.reason),
                    remedy: finding.remedy.as_deref().map(said),
                })
            })
            .collect(),
    }
}

/// The report as a person reads it: the overall status, then each check on a
/// line of its own with what to do beneath it.
///
/// Words cut at the contract's ceiling end in `[cut]`, and a report with any
/// opens by saying so, since a reason cut short can read as a whole one; the
/// document says the same in each check's `truncated`.
#[must_use]
pub fn human(report: &Report) -> String {
    use std::fmt::Write as _;

    let marked = |words: &Text| {
        let shown = clean(words.as_str());
        if words.truncated() {
            format!("{shown}{CUT}")
        } else {
            shown
        }
    };
    let mut out = format!("crucible doctor: {}\n", report.status());
    let any_cut = report.checks.iter().any(|check| {
        check.reason.truncated() || check.remedy.as_ref().is_some_and(Text::truncated)
    });
    if any_cut {
        let _ = writeln!(out, "  some words were cut short, each where it says{CUT}");
    }
    for check in &report.checks {
        let _ = writeln!(
            out,
            "  {:<11} {}: {}",
            check.status.as_str(),
            check.id.as_str(),
            marked(&check.reason),
        );
        if let Some(remedy) = &check.remedy {
            let _ = writeln!(out, "              {}", marked(remedy));
        }
    }
    out
}

/// What the text form puts after words cut at their ceiling.
const CUT: &str = " [cut]";

/// How one check came out, before it is bounded into the contract's words.
struct Finding {
    status: Status,
    reason: String,
    remedy: Option<String>,
}

impl Finding {
    fn ok(reason: impl Into<String>) -> Self {
        Self {
            status: Status::Ok,
            reason: reason.into(),
            remedy: None,
        }
    }

    fn warning(reason: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            status: Status::Warning,
            reason: reason.into(),
            remedy: Some(remedy.into()),
        }
    }

    fn failed(reason: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            status: Status::Failed,
            reason: reason.into(),
            remedy: Some(remedy.into()),
        }
    }

    /// Not made, because what it rests on is missing; the reason says what.
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: Status::Unavailable,
            reason: reason.into(),
            remedy: None,
        }
    }
}

/// The configuration, as the check and as the settings a launch would read.
struct Read {
    checked: CheckReport,
    settings: Result<Settings, ConfigError>,
}

impl Read {
    fn of(home: &Home, root: &Path) -> Self {
        Self {
            checked: crucible_config::check(home, root),
            settings: Settings::read(home, root),
        }
    }

    /// The settings, where every file parsed and they resolved.
    fn settings(&self) -> Option<&Settings> {
        if self.checked.valid() {
            self.settings.as_ref().ok()
        } else {
            None
        }
    }
}

/// Run a check's words through the bound as one line: words written over
/// several lines, as the sandbox probe writes what it refused, are joined in
/// order, and every other character that could move a cursor, end a line, or
/// reorder or hide what is read is replaced, as [`unshown`] lists them.
fn said(words: &str) -> Text {
    let mut line = String::new();
    for part in words.lines().map(str::trim).filter(|part| !part.is_empty()) {
        if !line.is_empty() {
            line.push_str(if line.ends_with(':') { " " } else { "; " });
        }
        line.push_str(&clean(part));
    }
    Text::cut(&line)
}

fn clean(words: &str) -> String {
    words
        .chars()
        .map(|one| if unshown(one) { '\u{fffd}' } else { one })
        .collect()
}

/// Whether `character` is replaced before a person reads it: a control
/// character; a Unicode format character (general category `Cf`), the bidi
/// marks, embeddings, overrides and isolates, the zero-width characters and
/// the byte order mark among them, any of which reorders or hides the text
/// around it; or the line or paragraph separator, which some terminals and
/// viewers end a line at. U+2065, unassigned between the invisible operators
/// and the isolates, is taken with them.
///
/// The format characters are the ones a limit's name drops, listed by hand
/// as it lists them, since the standard library has no Unicode table; a test
/// beside this holds the two lists to each other.
const fn unshown(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

const NO_HOME: &str = "crucible's home directory was not found";
const NO_CONFIG: &str = "the configuration could not be read";
const NO_WORKSPACE: &str = "the directory crucible was started in could not be worked in";
const CONFIG_CHECK: &str = "run `crucible config check` to see which file and why";
const INSPECT: &str = "run `crucible sandbox inspect` to see the whole policy and the backend";

/// Whether there is a home directory to read anything from.
fn homed(home: Result<&Home, &ConfigError>) -> Finding {
    match home {
        Ok(_) => Finding::ok("crucible's home directory was found"),
        // The one refusal finding a home can give names a variable, never a
        // path; anything else is said without its words.
        Err(why @ ConfigError::Homeless { .. }) => Finding::failed(
            why.to_string(),
            "set HOME, or CRUCIBLE_CODE_HOME, to an absolute path",
        ),
        Err(_) => Finding::failed(
            NO_HOME,
            "set HOME, or CRUCIBLE_CODE_HOME, to an absolute path",
        ),
    }
}

/// Whether the directory crucible was started in is one it can work in.
fn worked(workspace: Option<&Result<Workspace, PathError>>) -> Finding {
    let remedy = "start crucible from a directory that exists and is named in UTF-8 text";
    match workspace {
        None => Finding::failed(
            "the directory crucible was started in could not be read",
            remedy,
        ),
        Some(Ok(_)) => Finding::ok("the directory crucible was started in can be worked in"),
        Some(Err(PathError::Missing { .. })) => Finding::failed(
            "the directory crucible was started in does not exist or cannot be resolved",
            remedy,
        ),
        Some(Err(PathError::NotText { .. })) => Finding::failed(
            "the directory crucible was started in is not named in UTF-8 text",
            remedy,
        ),
        // Every other refusal leads with the path it is about.
        Some(Err(_)) => Finding::failed(NO_WORKSPACE, remedy),
    }
}

/// Whether every configuration file parsed, layer by layer.
fn configured(read: Option<&Read>) -> Finding {
    let Some(read) = read else {
        return Finding::unavailable(
            "not checked: it is read from the home directory and the directory crucible was \
             started in, and one of them is missing",
        );
    };
    let layers = read
        .checked
        .files()
        .iter()
        .map(|file| {
            let state = match file.state() {
                FileState::Absent => "absent",
                FileState::Valid => "valid",
                FileState::Invalid => "invalid",
            };
            format!("{} file {state}", file.layer())
        })
        .collect::<Vec<_>>()
        .join(", ");
    if !read.checked.valid() {
        let invalid = read
            .checked
            .files()
            .iter()
            .any(|file| file.state() == FileState::Invalid);
        let reason = if invalid {
            format!("a configuration file was refused: {layers}")
        } else {
            format!("the configuration files disagree with each other: {layers}")
        };
        return Finding::failed(reason, CONFIG_CHECK);
    }
    match &read.settings {
        Ok(_) => Finding::ok(layers),
        Err(_) => Finding::failed(
            format!("the configuration could not be resolved: {layers}"),
            CONFIG_CHECK,
        ),
    }
}

/// Whether what crucible keeps is its owner's alone.
#[cfg(unix)]
fn private(home: Option<&Home>, inventory: Option<&Inventory>) -> Finding {
    use std::os::unix::fs::PermissionsExt as _;

    let Some(home) = home else {
        return Finding::unavailable(NO_HOME);
    };
    let user = crucible_config::user(home);
    let kept = [
        ("the home directory", home.path()),
        ("the sessions directory", home.sessions()),
        ("the user configuration file", user.as_path()),
    ];
    let mut seen = 0_usize;
    let mut open = Vec::new();
    for (named, at) in kept {
        // Read, never followed by a change: the mode is the answer.
        let Ok(metadata) = std::fs::metadata(at) else {
            continue;
        };
        seen += 1;
        if metadata.permissions().mode() & 0o077 != 0 {
            open.push(named);
        }
    }
    if let Some(exposed) = inventory.and_then(Inventory::exposed) {
        seen += 1;
        if exposed {
            open.push("the credential store");
        }
    }
    if !open.is_empty() {
        return Finding::warning(
            format!("others can read or write {}", open.join(", ")),
            "remove access for others, as `chmod go-rwx` does, from each one named",
        );
    }
    if seen == 0 {
        Finding::ok("nothing crucible keeps is there yet")
    } else {
        Finding::ok("what crucible keeps is its owner's alone")
    }
}

/// Whether what crucible keeps is its owner's alone: said by an access list
/// on this platform, which this check does not read.
#[cfg(not(unix))]
fn private(_home: Option<&Home>, _inventory: Option<&Inventory>) -> Finding {
    Finding::unavailable("not checked: this platform says who may read a file with an access list")
}

/// Whether the credential store can be read, and how much it holds, by name.
fn stocked(inventory: Option<&Inventory>) -> Finding {
    let Some(inventory) = inventory else {
        return Finding::unavailable(NO_HOME);
    };
    if let Some(trouble) = inventory.trouble() {
        return Finding::warning(
            format!("no stored credential can be used: {trouble}"),
            "move auth.json out of crucible's home directory and use /login again",
        );
    }
    match (inventory.present(), inventory.count()) {
        (false, _) | (true, 0) => Finding::ok("no credential is stored"),
        (true, 1) => Finding::ok("1 stored credential"),
        (true, many) => Finding::ok(format!("{many} stored credentials")),
    }
}

/// The source of every provider's credential, settled as a launch settles it
/// from names alone, and never the credential.
fn credentialed(
    settings: Option<&Settings>,
    registry: &Result<Providers, impl std::fmt::Display>,
    from: &dyn Fn(&str) -> Option<String>,
    inventory: Option<&Inventory>,
    rows: &Rows,
) -> Finding {
    let Some(settings) = settings else {
        return Finding::unavailable(NO_CONFIG);
    };
    let Ok(registry) = registry else {
        return Finding::unavailable("this build's providers could not be assembled");
    };
    let sources: Vec<String> = providers::offered(registry)
        .filter_map(|one| {
            source(one, settings, from, inventory, rows)
                .map(|source| format!("{} from {source}", one.name))
        })
        .collect();
    if sources.is_empty() {
        Finding::warning(
            "no provider has a credential",
            "use /login, or set a provider's API key environment variable",
        )
    } else {
        Finding::ok(sources.join(", "))
    }
}

/// Where `one`'s credential would come from, by the launch's own rule.
fn source(
    one: Served,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    inventory: Option<&Inventory>,
    rows: &Rows,
) -> Option<providers::CredentialSource> {
    let held = inventory.and_then(|inventory| inventory.held(one.name));
    providers::sourced(one, settings, from, held, rows.subscribes(one.name))
}

/// Which provider a launch would open, and whether it could ask it anything.
///
/// Nothing is sent, so whether the vendor takes the credential is not known
/// and is said not to be.
fn provided(
    settings: Option<&Settings>,
    registry: &Result<Providers, impl std::fmt::Display>,
    from: &dyn Fn(&str) -> Option<String>,
    inventory: Option<&Inventory>,
    rows: &Rows,
) -> Finding {
    let Some(settings) = settings else {
        return Finding::unavailable(NO_CONFIG);
    };
    let registry = match registry {
        Ok(registry) => registry,
        Err(why) => {
            return Finding::failed(
                format!("this build's providers could not be assembled: {why}"),
                "report this: it is a defect in this build",
            );
        }
    };
    let usable = |one: Served| source(one, settings, from, inventory, rows).is_some();
    let chosen = match providers::choosing(registry, settings, usable) {
        Ok(chosen) => chosen,
        Err(why @ AppError::Provider { .. }) => {
            return Finding::failed(why.to_string(), "set `provider` to one this build serves");
        }
        Err(_) => {
            return Finding::failed(
                "the configured provider could not be settled",
                "set `provider` to one this build serves",
            );
        }
    };
    if let Some(one) = chosen {
        let from_where = source(one, settings, from, inventory, rows)
            .map_or_else(String::new, |source| format!(", from {source}"));
        return match settings
            .model(one.name)
            .map(str::trim)
            .filter(|m| !m.is_empty())
        {
            Some(model) => Finding::ok(format!(
                "{} with model {model}{from_where}; nothing was sent, so whether the vendor \
                 accepts the credential is not known",
                one.name
            )),
            None => Finding::warning(
                format!(
                    "{} would open{from_where}, and no model is chosen for it",
                    one.name
                ),
                format!(
                    "use /model, set `providers.{}.model`, or start with `--model`",
                    one.name
                ),
            ),
        };
    }
    if let Some(named) = settings.provider() {
        let variable = providers::offered(registry)
            .find(|one| one.name == named)
            .map_or("its API key variable", |one| {
                settings.api_key_env(one.name).unwrap_or(one.key)
            });
        return Finding::failed(
            format!("the configuration chooses {named}, and no credential for it was found"),
            format!("use /login, or set {variable}"),
        );
    }
    if providers::offered(registry).any(usable) {
        return Finding::warning(
            "more than one provider has a credential, and none is chosen",
            "use /model to choose one, or set `provider`",
        );
    }
    Finding::failed(
        "no provider has a credential, so nothing can be asked",
        "use /login, or set a provider's API key environment variable",
    )
}

/// Whether this machine has a backend that would confine a command here
/// under the standard policy, observed and not started.
fn backed(service: &dyn Observing, workspace: Option<&Workspace>) -> Finding {
    let Some(workspace) = workspace else {
        return Finding::unavailable(NO_WORKSPACE);
    };
    let Ok(policy) = SandboxPolicy::standard(workspace) else {
        return Finding::failed(
            "no confinement policy could be built for this directory",
            INSPECT,
        );
    };
    match service.observe(&sandbox::asking(policy)) {
        Err(why) => Finding::warning(
            format!("no backend that confines commands was found: {why}"),
            INSPECT,
        ),
        Ok(found) => {
            if let Some(why) = found.refusal() {
                return Finding::warning(
                    format!(
                        "{} was found and would not take this directory's policy: {why}",
                        found.id().as_str()
                    ),
                    INSPECT,
                );
            }
            let version = match found.version() {
                ObservedVersion::Stated(version) => version.to_owned(),
                ObservedVersion::Unverified(why) => format!("unverified, since {why}"),
            };
            let unchecked = found
                .unchecked()
                .map_or_else(String::new, |why| format!("; not checked: {why}"));
            Finding::ok(format!(
                "{} from {}, version {version}{unchecked}",
                found.id().as_str(),
                found.provenance().as_str(),
            ))
        }
    }
}

/// Whether the policy this directory's configuration asks for would confine
/// a command, as `crucible sandbox inspect` settles it.
fn confined(
    service: &dyn Observing,
    here: Option<&Path>,
    home: Option<&Home>,
    settings: Option<&Settings>,
) -> Finding {
    let (Some(here), Some(home), Some(_)) = (here, home, settings) else {
        return Finding::unavailable(NO_CONFIG);
    };
    let observed = match sandbox::inspecting(service, here, home) {
        Ok(observed) => observed,
        Err(why @ AppError::Confinement(_)) => return Finding::failed(why.to_string(), INSPECT),
        Err(_) => {
            return Finding::failed(
                "the sandbox policy could not be built for this directory",
                INSPECT,
            );
        }
    };
    let inspected = match observed.contract() {
        Ok(Inspection::Inspected(inspected)) => inspected,
        Ok(Inspection::Failed(why)) => return Finding::failed(why.as_str(), INSPECT),
        Err(why) => return Finding::failed(why.to_string(), INSPECT),
    };
    if !inspected.enabled {
        return Finding::ok(
            "confinement is off, so commands run as ordinary processes; `/sandbox enable` or \
             `sandbox.enabled` turns it on",
        );
    }
    match (&inspected.refusal, inspected.confined) {
        (None, true) => Finding::ok("commands here would run inside the backend's boundary"),
        (Some(why), _) => Finding::failed(
            format!(
                "confinement is on and no command could run here: {}",
                why.as_str()
            ),
            format!("{INSPECT}, or `/sandbox disable` to run commands unconfined"),
        ),
        (None, false) => Finding::failed(
            "confinement is on and a command here would not run inside the backend's boundary",
            format!("{INSPECT}, or `/sandbox disable` to run commands unconfined"),
        ),
    }
}

/// Whether the extensions directory could be read, counted and not named.
fn discovered(extensions: Option<&Extensions>) -> Finding {
    let Some(extensions) = extensions else {
        return Finding::unavailable(NO_HOME);
    };
    let remedy = "`crucible --extensions` says which, and why";
    if extensions.stopped() {
        return Finding::warning(
            "the extensions directory holds more than crucible reads, so none was read",
            remedy,
        );
    }
    let found = extensions.found().len();
    let refused = extensions.refused().len();
    if refused > 0 {
        return Finding::warning(
            format!("{found} extension(s) read, and {refused} that could not be"),
            remedy,
        );
    }
    match found {
        0 => Finding::ok("no extension is installed"),
        1 => Finding::ok("1 extension is installed"),
        many => Finding::ok(format!("{many} extensions are installed")),
    }
}

/// Whether what was decided about each extension still holds, from the home
/// file alone, since no checkout may turn one on.
fn trusted(home: Option<&Home>, extensions: Option<&Extensions>, running: &str) -> Finding {
    let (Some(home), Some(extensions)) = (home, extensions) else {
        return Finding::unavailable(NO_HOME);
    };
    let Ok(decided) = Settings::read_home(home) else {
        return Finding::unavailable("the home configuration file could not be read");
    };
    let (mut allowed, mut off, mut stale, mut unhosted) = (0_usize, 0_usize, 0_usize, 0_usize);
    for one in extensions.found() {
        let manifest = one.manifest();
        let id = &manifest.identity().id;
        let decision = ExtensionDecision {
            enabled: decided.extension_enabled(id),
            digest: decided.extension_digest(id),
        };
        match manifest.trusted(decision) {
            Err(ExtensionUntrusted::Undecided) => off += 1,
            Err(ExtensionUntrusted::Unpinned | ExtensionUntrusted::Changed { .. }) => stale += 1,
            Ok(_) if manifest.hosted(running).is_err() => unhosted += 1,
            Ok(_) => allowed += 1,
        }
    }
    let counted = format!(
        "{allowed} may run, {off} not turned on, {stale} turned on without a digest that \
         matches, {unhosted} this crucible cannot host"
    );
    if stale + unhosted > 0 {
        Finding::warning(
            counted,
            "`crucible --extensions` says which, and the digest each one's key must hold",
        )
    } else {
        Finding::ok(counted)
    }
}

/// The MCP servers declared, counted and never started.
fn served(settings: Option<&Settings>) -> Finding {
    let Some(settings) = settings else {
        return Finding::unavailable(NO_CONFIG);
    };
    let servers = settings.mcp_servers();
    let required = servers.iter().filter(|server| server.required()).count();
    Finding::ok(format!(
        "{} declared, {required} required; none is started without --with-mcp and none was \
         started here, so whether each starts is not checked",
        servers.len()
    ))
}

#[cfg(test)]
mod tests;
