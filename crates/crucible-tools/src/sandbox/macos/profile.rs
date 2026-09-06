//! Deterministic Seatbelt profiles derived from an immutable sandbox policy.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use crucible_core::{
    SandboxDomainPolicy, SandboxError, SandboxFilesystemAccess, SandboxNetworkPolicy, SandboxPolicy,
};

// macOS 26 resolves some system utilities through `/var/select` even when the
// requested executable lives under `/bin`. This fixed system runtime is
// readable only; a request-level unreadable rule still overrides it.
const BASE_POLICY: &str = "\
(version 1)\n\
(deny default)\n\
(deny network*)\n\
(allow process-exec)\n\
(allow process-fork)\n\
(allow signal (target same-sandbox))\n\
(allow process-info* (target same-sandbox))\n\
(allow sysctl-read\n\
  (sysctl-name \"hw.cputype\")\n\
  (sysctl-name \"hw.cpusubtype\")\n\
  (sysctl-name \"hw.cpufamily\")\n\
  (sysctl-name \"hw.memsize\")\n\
  (sysctl-name \"hw.ncpu\")\n\
  (sysctl-name \"hw.pagesize\")\n\
  (sysctl-name \"hw.pagesize_compat\")\n\
  (sysctl-name \"kern.argmax\")\n\
  (sysctl-name \"kern.osrelease\")\n\
  (sysctl-name \"kern.osversion\"))\n\
(allow file-read-data (literal \"/\"))\n\
(allow file-read-metadata file-test-existence\n\
  (literal \"/\")\n\
  (literal \"/var\")\n\
  (literal \"/var/select\")\n\
  (literal \"/private\")\n\
  (literal \"/private/etc\")\n\
  (literal \"/private/etc/ssl\"))\n\
(allow file-read*\n\
  (require-any\n\
    (subpath \"/System/Library\")\n\
    (subpath \"/Library/Apple\")\n\
    (subpath \"/usr/bin\")\n\
    (subpath \"/usr/sbin\")\n\
    (subpath \"/usr/lib\")\n\
    (subpath \"/usr/libexec\")\n\
    (subpath \"/bin\")\n\
    (subpath \"/sbin\")\n\
    (subpath \"/nix/store\")\n\
    (literal \"/private/etc/ssl/openssl.cnf\")\n\
    (subpath \"/private/var/db/timezone\")\n\
    (subpath \"/private/var/select\")\n\
    (subpath \"/var/select\")\n\
    (literal \"/dev/null\")\n\
    (literal \"/dev/zero\")\n\
    (literal \"/dev/random\")\n\
    (literal \"/dev/urandom\")))\n\
(allow file-write-data (require-all (literal \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n";

#[derive(Clone)]
pub(super) struct Profile {
    policy: String,
    definitions: Vec<OsString>,
    sockets: Vec<SocketIdentity>,
}

#[derive(Clone)]
struct SocketIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl Profile {
    pub(super) fn build(
        policy: &SandboxPolicy,
        discovered_protected: &[std::path::PathBuf],
        linked_metadata: &[std::path::PathBuf],
        expanded_unreadable: &[std::path::PathBuf],
    ) -> Result<Self, SandboxError> {
        let mut text = String::from(BASE_POLICY);
        let mut definitions = Vec::new();
        let mut writes = 0_usize;
        let mut denied_writes = 0_usize;
        let mut denied_reads = 0_usize;

        for rule in policy.filesystem() {
            if rule.access() != SandboxFilesystemAccess::Unreadable {
                let key = format!("READ_{}", definitions.len());
                push_definition(&mut definitions, &key, rule.path())?;
                let _ = writeln!(
                    text,
                    "(allow file-read* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
                );
            }
            match rule.access() {
                SandboxFilesystemAccess::ReadWrite => {
                    let key = format!("WRITE_{writes}");
                    push_definition(&mut definitions, &key, rule.path())?;
                    let exclusions = paths_with_system_alias(rule.path())
                        .into_iter()
                        .map(|path| {
                            protected_metadata_regex(&path)
                                .map(|regex| format!("(require-not (regex #\"{regex}\"))"))
                        })
                        .collect::<Result<Vec<_>, _>>()?
                        .join(" ");
                    let _ = write!(
                        text,
                        "(allow file-write* (require-all (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))) {exclusions}))\n\
                         (deny file-write-unlink file-write-create (require-all (literal (param \"{key}\")) (vnode-type DIRECTORY)))\n"
                    );
                    writes = writes.saturating_add(1);
                }
                SandboxFilesystemAccess::Protected => {
                    push_deny_path(
                        &mut text,
                        &mut definitions,
                        rule.path(),
                        ("DENY_WRITE", &mut denied_writes, "file-write* file-link"),
                    )?;
                }
                SandboxFilesystemAccess::Unreadable => {
                    push_deny_path(
                        &mut text,
                        &mut definitions,
                        rule.path(),
                        (
                            "DENY_READ",
                            &mut denied_reads,
                            "file-read* file-write* file-link",
                        ),
                    )?;
                }
                SandboxFilesystemAccess::ReadOnly => {}
            }
        }

        for path in discovered_protected {
            push_deny_path(
                &mut text,
                &mut definitions,
                path,
                ("DENY_WRITE", &mut denied_writes, "file-write* file-link"),
            )?;
        }

        for path in linked_metadata {
            let read_key = format!("LINKED_READ_{}", definitions.len());
            push_definition(&mut definitions, &read_key, path)?;
            let _ = writeln!(
                text,
                "(allow file-read* (require-any (literal (param \"{read_key}\")) (subpath (param \"{read_key}\"))))"
            );
            push_deny_path(
                &mut text,
                &mut definitions,
                path,
                ("DENY_WRITE", &mut denied_writes, "file-write* file-link"),
            )?;
        }

        for path in expanded_unreadable {
            push_deny_path(
                &mut text,
                &mut definitions,
                path,
                (
                    "DENY_READ",
                    &mut denied_reads,
                    "file-read* file-write* file-link",
                ),
            )?;
        }
        for pattern in policy.unreadable_patterns() {
            for path in paths_with_system_alias(pattern.pattern()) {
                let regex = unreadable_pattern_regex(&path)?;
                let _ = writeln!(
                    text,
                    "(deny file-read* file-write* file-link (regex #\"{regex}\"))"
                );
                let _ = writeln!(
                    text,
                    "(deny file-write-unlink file-write-create (regex #\"{regex}\"))"
                );
            }
        }

        // Seatbelt's literal path filters have differed across case-insensitive
        // APFS releases for rename destinations. Keep protected control-plane
        // names closed with one case-insensitive, root-anchored predicate too.
        for rule in policy
            .filesystem()
            .iter()
            .filter(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
        {
            for path in paths_with_system_alias(rule.path()) {
                let regex = protected_metadata_regex(&path)?;
                let _ = writeln!(text, "(deny file-write* file-link (regex #\"{regex}\"))");
                let _ = writeln!(
                    text,
                    "(deny file-write-unlink file-write-create (regex #\"{regex}\"))"
                );
            }
        }

        // Renaming an allowed ancestor would relocate a protected descendant
        // beyond every pathname carve-out. Deny that directory operation for
        // each ancestor between discovered metadata and its writable root.
        let mut protected_ancestors = BTreeSet::new();
        for protected in policy
            .filesystem()
            .iter()
            .filter(|rule| rule.access() == SandboxFilesystemAccess::Protected)
            .map(crucible_core::SandboxFilesystemRule::path)
            .chain(discovered_protected.iter().map(PathBuf::as_path))
        {
            let Some(root) = policy.filesystem().iter().find(|rule| {
                rule.access() == SandboxFilesystemAccess::ReadWrite
                    && protected.starts_with(rule.path())
            }) else {
                continue;
            };
            let mut ancestor = protected.parent();
            while let Some(path) = ancestor {
                if path == root.path() {
                    break;
                }
                protected_ancestors.insert(path.to_path_buf());
                ancestor = path.parent();
            }
        }
        for path in protected_ancestors {
            let key = format!("PROTECTED_ANCESTOR_{}", definitions.len());
            push_definition(&mut definitions, &key, &path)?;
            let _ = writeln!(
                text,
                "(deny file-write-unlink file-write-create (require-all (vnode-type DIRECTORY) (literal (param \"{key}\"))))"
            );
        }

        let sockets = append_static_network(&mut text, &mut definitions, policy)?;

        let profile = Self {
            policy: text,
            definitions,
            sockets,
        };
        profile.validate()?;
        Ok(profile)
    }

    pub(super) fn with_scratch(mut self, path: &std::path::Path) -> Result<Self, SandboxError> {
        push_definition(&mut self.definitions, "SCRATCH", path)?;
        self.policy.push_str(
            "(allow file-write* (require-any (literal (param \"SCRATCH\")) (subpath (param \"SCRATCH\"))))\n\
             (deny file-write-unlink file-write-create (require-all (literal (param \"SCRATCH\")) (vnode-type DIRECTORY)))\n",
        );
        self.validate()?;
        Ok(self)
    }

    /// Returns the launch profile after opening only the host-owned mediator port.
    ///
    /// The listener is created at stage time, so its ephemeral port cannot be
    /// part of the deterministic prepared profile.
    pub(super) fn with_proxy(&self, endpoint: SocketAddr) -> Result<Self, SandboxError> {
        if endpoint.ip() != Ipv4Addr::LOCALHOST || endpoint.port() == 0 {
            return Err(materialization(
                "the macOS sandbox proxy endpoint is not private loopback",
                None,
            ));
        }
        let mut profile = self.clone();
        let _ = writeln!(
            profile.policy,
            "(allow network-outbound (require-all (remote tcp \"localhost:{}\") (socket-domain AF_INET)))",
            endpoint.port()
        );
        profile.validate()?;
        Ok(profile)
    }

    pub(super) fn validate_network(&self) -> Result<(), SandboxError> {
        for expected in &self.sockets {
            let actual = validate_socket(&expected.path)?;
            if actual.device != expected.device
                || actual.inode != expected.inode
                || actual.changed_seconds != expected.changed_seconds
                || actual.changed_nanoseconds != expected.changed_nanoseconds
            {
                return Err(materialization(
                    "sandbox Unix endpoint changed after preparation",
                    None,
                ));
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), SandboxError> {
        if self.policy.is_empty()
            || self.policy.len() > crucible_sandbox_broker::MACOS_MAX_PROFILE_BYTES
        {
            return Err(SandboxError::Materialization {
                problem: "macOS sandbox profile exceeds the backend bound".into(),
                source: None,
            });
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn policy(&self) -> &str {
        &self.policy
    }

    #[cfg(target_os = "macos")]
    pub(super) fn definitions(&self) -> &[OsString] {
        &self.definitions
    }
}

fn append_static_network(
    text: &mut String,
    definitions: &mut Vec<OsString>,
    policy: &SandboxPolicy,
) -> Result<Vec<SocketIdentity>, SandboxError> {
    let domains = match policy.network() {
        SandboxNetworkPolicy::Closed => return Ok(Vec::new()),
        SandboxNetworkPolicy::Domains(domains) => domains,
    };
    let sockets = validate_network(policy)?;

    // A Seatbelt Unix grant is pathname-based rather than descriptor-pinned.
    // Capture identity now and revalidate at stage and release; these denies
    // prevent the child from replacing the socket or renaming one of its
    // writable ancestors after launch. A host process can still replace the
    // pathname after the final check, which is a platform limitation of
    // Seatbelt's public profile grammar rather than authority granted here.
    let mut denied_sockets = 0_usize;
    for path in domains.unix_sockets() {
        push_deny_path(
            text,
            definitions,
            path,
            ("DENY_SOCKET", &mut denied_sockets, "file-write* file-link"),
        )?;
    }
    append_socket_ancestor_denies(text, definitions, policy, domains)?;

    if domains.allow_local_binding() {
        text.push_str("(allow network-bind network-inbound (local ip \"localhost:*\"))\n");
    }
    if !domains.unix_sockets().is_empty() {
        text.push_str("(allow system-socket (socket-domain AF_UNIX))\n");
        for path in domains.unix_sockets() {
            let path = seatbelt_literal(path)?;
            let _ = writeln!(
                text,
                "(allow network-outbound (remote unix-socket (literal \"{path}\")))"
            );
        }
    }
    Ok(sockets)
}

fn validate_network(policy: &SandboxPolicy) -> Result<Vec<SocketIdentity>, SandboxError> {
    let SandboxNetworkPolicy::Domains(domains) = policy.network() else {
        return Ok(Vec::new());
    };
    let mut identities = Vec::with_capacity(domains.unix_sockets().len());
    for path in domains.unix_sockets() {
        if policy.filesystem().iter().any(|rule| {
            rule.access() == SandboxFilesystemAccess::Unreadable && path.starts_with(rule.path())
        }) || policy
            .unreadable_patterns()
            .iter()
            .any(|pattern| pattern.matches(path))
        {
            return Err(materialization(
                "an unreadable path cannot be granted as a Unix endpoint",
                None,
            ));
        }
        identities.push(validate_socket(path)?);
    }
    Ok(identities)
}

fn validate_socket(path: &Path) -> Result<SocketIdentity, SandboxError> {
    let inspect = || {
        fs::symlink_metadata(path).map_err(|source| {
            materialization("sandbox Unix endpoint could not be inspected", Some(source))
        })
    };
    let named = inspect()?;
    if named.file_type().is_symlink() || !named.file_type().is_socket() || named.nlink() != 1 {
        return Err(materialization(
            "sandbox Unix endpoint is not a canonical socket",
            None,
        ));
    }
    let canonical = path.canonicalize().map_err(|source| {
        materialization(
            "sandbox Unix endpoint could not be canonicalized",
            Some(source),
        )
    })?;
    if canonical != path {
        return Err(materialization(
            "sandbox Unix endpoint contains a symbolic-link or non-canonical component",
            None,
        ));
    }
    let confirmed = inspect()?;
    if confirmed.dev() != named.dev()
        || confirmed.ino() != named.ino()
        || confirmed.ctime() != named.ctime()
        || confirmed.ctime_nsec() != named.ctime_nsec()
        || confirmed.nlink() != 1
        || !confirmed.file_type().is_socket()
    {
        return Err(materialization(
            "sandbox Unix endpoint changed during validation",
            None,
        ));
    }
    Ok(SocketIdentity {
        path: path.to_path_buf(),
        device: confirmed.dev(),
        inode: confirmed.ino(),
        changed_seconds: confirmed.ctime(),
        changed_nanoseconds: confirmed.ctime_nsec(),
    })
}

fn append_socket_ancestor_denies(
    text: &mut String,
    definitions: &mut Vec<OsString>,
    policy: &SandboxPolicy,
    domains: &SandboxDomainPolicy,
) -> Result<(), SandboxError> {
    let mut ancestors = BTreeSet::new();
    for socket in domains.unix_sockets() {
        let writable_root = policy
            .filesystem()
            .iter()
            .filter(|rule| {
                rule.access() == SandboxFilesystemAccess::ReadWrite
                    && socket.starts_with(rule.path())
            })
            .max_by_key(|rule| rule.path().components().count());
        let Some(root) = writable_root else {
            continue;
        };
        let mut ancestor = socket.parent();
        while let Some(path) = ancestor {
            if path == root.path() {
                break;
            }
            ancestors.insert(path.to_path_buf());
            ancestor = path.parent();
        }
    }
    for path in ancestors {
        for path in paths_with_system_alias(&path) {
            let key = format!("SOCKET_ANCESTOR_{}", definitions.len());
            push_definition(definitions, &key, &path)?;
            let _ = writeln!(
                text,
                "(deny file-write-unlink file-write-create (require-all (vnode-type DIRECTORY) (literal (param \"{key}\"))))"
            );
        }
    }
    Ok(())
}

fn seatbelt_literal(path: &Path) -> Result<String, SandboxError> {
    let path = path.to_str().ok_or_else(|| {
        materialization("macOS sandbox Unix endpoints must be valid Unicode", None)
    })?;
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        if character.is_control() {
            return Err(materialization(
                "macOS sandbox Unix endpoints contain an unsupported control character",
                None,
            ));
        }
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Ok(escaped)
}

fn materialization(problem: &'static str, source: Option<std::io::Error>) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source,
    }
}

fn push_deny_path(
    text: &mut String,
    definitions: &mut Vec<OsString>,
    path: &Path,
    deny: (&str, &mut usize, &str),
) -> Result<(), SandboxError> {
    let (prefix, count, operations) = deny;
    for path in paths_with_system_alias(path) {
        let key = format!("{prefix}_{}", *count);
        push_definition(definitions, &key, &path)?;
        let _ = writeln!(
            text,
            "(deny {operations} (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
        );
        let _ = writeln!(
            text,
            "(deny file-write-unlink file-write-create (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
        );
        *count = count.saturating_add(1);
    }
    Ok(())
}

fn paths_with_system_alias(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let aliases = [
        (Path::new("/private/var"), Path::new("/var")),
        (Path::new("/var"), Path::new("/private/var")),
    ];
    if let Some(alias) = aliases.iter().find_map(|(source, target)| {
        path.strip_prefix(source)
            .ok()
            .map(|suffix| target.join(suffix))
    }) && alias != path
    {
        paths.push(alias);
    }
    paths
}

fn protected_metadata_regex(root: &Path) -> Result<String, SandboxError> {
    let Some(root) = root.to_str() else {
        return Err(SandboxError::Materialization {
            problem: "macOS sandbox paths must be valid Unicode for protected-name matching".into(),
            source: None,
        });
    };
    let root = root.trim_end_matches('/');
    let mut regex = String::from("^");
    push_case_insensitive_literal(&mut regex, root)?;
    regex.push_str(
        "/([^/]+/)*[.](\
         [gG][iI][tT]|\
         [aA][gG][eE][nN][tT][sS]|\
         [cC][oO][dD][eE][xX]|\
         [cC][rR][uU][cC][iI][bB][lL][eE])(/.*)?$",
    );
    Ok(regex)
}

fn unreadable_pattern_regex(pattern: &Path) -> Result<String, SandboxError> {
    let mut all = Vec::new();
    for component in pattern.components() {
        if let std::path::Component::Normal(value) = component {
            all.push(value.to_str().ok_or_else(unsupported_pattern)?);
        }
    }
    if all.is_empty() {
        return Err(unsupported_pattern());
    }
    let mut regex = String::from("^");
    for (index, component) in all.iter().enumerate() {
        regex.push('/');
        if *component == "**" {
            if index + 1 == all.len() {
                regex.push_str(".*");
            } else {
                regex.push_str("([^/]+/)*");
            }
            continue;
        }
        let mut literal = String::new();
        for part in component.split('*').enumerate() {
            if part.0 > 0 {
                literal.push_str("[^/]*");
            }
            push_case_insensitive_literal(&mut literal, part.1)?;
        }
        regex.push_str(&literal);
        if index > 0 && all.get(index.saturating_sub(1)) == Some(&"**") {
            let inserted = regex.len().saturating_sub(literal.len()).saturating_sub(1);
            regex.remove(inserted);
        }
    }
    regex.push_str("(/.*)?$");
    Ok(regex)
}

fn unsupported_pattern() -> SandboxError {
    SandboxError::Materialization {
        problem: "macOS sandbox unreadable pattern cannot be represented safely".into(),
        source: None,
    }
}

fn push_case_insensitive_literal(target: &mut String, value: &str) -> Result<(), SandboxError> {
    for character in value.chars() {
        if character.is_control() {
            return Err(SandboxError::Materialization {
                problem: "macOS sandbox paths contain an unsupported control character".into(),
                source: None,
            });
        }
        if character.is_ascii_alphabetic() {
            target.push('[');
            target.push(character.to_ascii_lowercase());
            target.push(character.to_ascii_uppercase());
            target.push(']');
        } else {
            if matches!(
                character,
                '\\' | '"'
                    | '.'
                    | '+'
                    | '*'
                    | '?'
                    | '('
                    | ')'
                    | '|'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '^'
                    | '$'
            ) {
                target.push('\\');
            }
            target.push(character);
        }
    }
    Ok(())
}

fn definition(key: &str, path: &std::path::Path) -> OsString {
    let mut definition = OsString::from(key);
    definition.push("=");
    definition.push(path);
    definition
}

fn push_definition(
    definitions: &mut Vec<OsString>,
    key: &str,
    path: &std::path::Path,
) -> Result<(), SandboxError> {
    if definitions.len() >= crucible_sandbox_broker::MACOS_MAX_DEFINITIONS {
        return Err(SandboxError::Materialization {
            problem: "macOS sandbox path definitions exceed the backend bound".into(),
            source: None,
        });
    }
    definitions.push(definition(key, path));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::net::UnixListener;
    use std::path::Path;

    use crucible_core::{
        SandboxDomainPattern, SandboxDomainPolicy, SandboxFilesystemAccess,
        SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxNetworkPolicy,
        SandboxNetworkProvenance, SandboxPolicy, SandboxResourceLimits, SandboxUnreadablePattern,
    };

    use super::{Profile, paths_with_system_alias, protected_metadata_regex, seatbelt_literal};
    use crate::sample::Sample;

    #[test]
    fn a_profile_uses_parameters_for_paths_and_keeps_network_closed() {
        let sample = Sample::new("macos-seatbelt-profile");
        sample.write(".git/config", "protected");
        sample.write("private/token", "secret");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let unreadable = SandboxFilesystemRule::new(
            sample.root().join("private"),
            SandboxFilesystemAccess::Unreadable,
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("unreadable rule");
        let policy = SandboxPolicy::new(
            standard.enabled(),
            standard.filesystem().iter().cloned().chain([unreadable]),
            standard.working_directory(),
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[])
            .expect("Seatbelt profile")
            .with_scratch(&sample.root().join(".scratch"))
            .expect("scratch definition");

        assert!(profile.policy.starts_with("(version 1)\n(deny default)\n"));
        assert!(profile.policy.contains("(allow file-read*"));
        assert!(!profile.policy.contains("(allow file-read*)"));
        assert!(profile.policy.contains("(allow process-exec)"));
        assert!(profile.policy.contains("(allow process-fork)"));
        assert!(profile.policy.contains("(subpath \"/var/select\")"));
        assert!(profile.policy.contains("(subpath \"/private/var/select\")"));
        assert!(!profile.policy.contains("(allow mach-lookup"));
        assert!(!profile.policy.contains("(allow network"));
        assert!(profile.policy.contains("(deny network*)"));
        assert!(
            profile
                .policy
                .contains("(sysctl-name \"hw.pagesize_compat\")")
        );
        assert!(!profile.policy.contains("(allow sysctl-read)\n"));
        assert!(!profile.policy.contains("(subpath \"/Applications\")"));
        assert!(!profile.policy.contains("(subpath \"/private/etc\")"));
        assert!(
            profile
                .policy
                .contains("(literal \"/private/etc/ssl/openssl.cnf\")")
        );
        assert!(
            !profile
                .policy
                .contains(&sample.root().to_string_lossy().to_string())
        );
        assert!(profile.policy.contains("(param \"WRITE_0\")"));
        assert!(profile.policy.contains("(param \"READ_0\")"));
        assert!(profile.policy.contains("(param \"DENY_WRITE_0\")"));
        assert!(profile.policy.contains("(param \"DENY_READ_0\")"));
        assert!(profile.policy.contains("(param \"SCRATCH\")"));
        assert!(profile.policy.contains("[gG][iI][tT]"));
        assert!(profile.policy.contains("([^/]+/)*"));
        assert!(profile.policy.contains("file-write* file-link"));
        assert!(
            profile
                .policy
                .contains("file-write-unlink file-write-create")
        );
        assert!(profile.policy.contains(
            "(deny file-write-unlink file-write-create (require-any (literal (param \"DENY_WRITE_0\")) (subpath (param \"DENY_WRITE_0\"))))"
        ));
        assert!(
            !profile.policy.contains("\"#)"),
            "Seatbelt regex literals have an opening sharp marker only"
        );
        let aliases = paths_with_system_alias(sample.root())
            .len()
            .saturating_sub(1);
        assert_eq!(profile.definitions.len(), 6 + aliases.saturating_mul(2));
    }

    #[test]
    fn a_domain_profile_grants_only_requested_static_local_authority() {
        let sample = Sample::socket("macos-seatbelt-domain-profile");
        let socket_path = sample.root().join("allowed.sock");
        let _socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains = SandboxDomainPolicy::new(
            [],
            [],
            true,
            [socket_path.clone()],
            SandboxNetworkProvenance::User,
        )
        .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[]).expect("Seatbelt profile");
        let socket = socket_path.to_str().expect("Unicode fixture path");

        assert!(
            profile
                .policy
                .contains("(allow network-bind network-inbound (local ip \"localhost:*\"))")
        );
        assert!(
            profile
                .policy
                .contains("(allow system-socket (socket-domain AF_UNIX))")
        );
        assert!(profile.policy.contains(&format!(
            "(allow network-outbound (remote unix-socket (literal \"{socket}\")))"
        )));
        assert!(profile.policy.contains("(param \"DENY_SOCKET_0\")"));
        assert!(profile.policy.contains("(deny network*)"));
        assert!(profile.policy.contains("(param \"WRITE_0\")"));
        assert!(!profile.policy.contains("(remote tcp"));
    }

    #[test]
    fn the_proxy_port_is_added_only_to_the_staged_profile_copy() {
        let sample = Sample::new("macos-seatbelt-proxy-profile");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains = SandboxDomainPolicy::new(
            [SandboxDomainPattern::new("127.0.0.1").expect("literal domain")],
            [],
            false,
            [],
            SandboxNetworkProvenance::User,
        )
        .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");
        let prepared = Profile::build(&policy, &[], &[], &[]).expect("prepared profile");

        let staged = prepared
            .with_proxy("127.0.0.1:43127".parse().expect("proxy endpoint"))
            .expect("staged profile");

        assert!(!prepared.policy.contains("(remote tcp"));
        assert!(
            staged
                .policy
                .contains("(allow network-outbound (require-all (remote tcp \"localhost:43127\") (socket-domain AF_INET)))")
        );
        assert!(staged.policy.starts_with(&prepared.policy));
        assert!(staged.policy.contains("(param \"WRITE_0\")"));
        for endpoint in ["0.0.0.0:43127", "127.0.0.1:0", "[::1]:43127"] {
            assert!(
                prepared
                    .with_proxy(endpoint.parse().expect("test endpoint"))
                    .is_err(),
                "accepted {endpoint}"
            );
        }
    }

    #[test]
    fn unix_endpoint_literals_cannot_inject_profile_forms() {
        assert_eq!(
            seatbelt_literal(Path::new("/tmp/a\"b\\c.sock")).expect("escaped literal"),
            "/tmp/a\\\"b\\\\c.sock"
        );
        assert!(seatbelt_literal(Path::new("/tmp/a\nb.sock")).is_err());
    }

    #[test]
    fn an_unreadable_unix_endpoint_is_refused_before_profile_grants() {
        let sample = Sample::socket("macos-seatbelt-unreadable-socket");
        let socket_path = sample.root().join("private/allowed.sock");
        std::fs::create_dir_all(socket_path.parent().expect("socket parent")).expect("parent");
        let _socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let unreadable = SandboxFilesystemRule::new(
            sample.root().join("private"),
            SandboxFilesystemAccess::Unreadable,
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("unreadable rule");
        let domains =
            SandboxDomainPolicy::new([], [], false, [socket_path], SandboxNetworkProvenance::User)
                .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned().chain([unreadable]),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let Err(problem) = Profile::build(&policy, &[], &[], &[]) else {
            panic!("unreadable socket must be refused");
        };
        assert!(problem.to_string().contains("unreadable path"));
    }

    #[test]
    fn an_unreadable_pattern_cannot_be_reopened_as_a_unix_endpoint() {
        let sample = Sample::socket("macos-seatbelt-unreadable-socket-pattern");
        let socket_path = sample.root().join("private/allowed.sock");
        std::fs::create_dir_all(socket_path.parent().expect("socket parent")).expect("parent");
        let _socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains =
            SandboxDomainPolicy::new([], [], false, [socket_path], SandboxNetworkProvenance::User)
                .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy")
        .with_unreadable_patterns([SandboxUnreadablePattern::new(
            sample.root().join("**/*.sock"),
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("unreadable pattern")])
        .expect("pattern policy");

        let Err(problem) = Profile::build(&policy, &[], &[], &[]) else {
            panic!("pattern-selected socket must be refused");
        };
        assert!(problem.to_string().contains("unreadable path"));
    }

    #[test]
    fn a_symbolic_link_cannot_supply_unix_endpoint_identity() {
        let sample = Sample::socket("macos-seatbelt-symlink-socket");
        let socket_path = sample.root().join("real.sock");
        let linked_path = sample.root().join("linked.sock");
        let _socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        crate::sample::symlink(&socket_path, &linked_path);
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains =
            SandboxDomainPolicy::new([], [], false, [linked_path], SandboxNetworkProvenance::User)
                .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let Err(problem) = Profile::build(&policy, &[], &[], &[]) else {
            panic!("symlink socket must be refused");
        };
        assert!(problem.to_string().contains("canonical socket"));
    }

    #[test]
    fn unix_endpoint_identity_is_rechecked_after_preparation() {
        let sample = Sample::socket("macos-seatbelt-replaced-socket");
        let socket_path = sample.root().join("endpoint.sock");
        let socket = UnixListener::bind(&socket_path).expect("Unix socket fixture");
        let standard = SandboxPolicy::standard(&sample.workspace()).expect("standard policy");
        let domains = SandboxDomainPolicy::new(
            [],
            [],
            false,
            [socket_path.clone()],
            SandboxNetworkProvenance::User,
        )
        .expect("domain policy");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned(),
            standard.working_directory(),
            SandboxNetworkPolicy::Domains(domains),
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");
        let profile = Profile::build(&policy, &[], &[], &[]).expect("prepared profile");

        drop(socket);
        std::fs::remove_file(&socket_path).expect("remove original socket name");
        let _replacement = UnixListener::bind(&socket_path).expect("replacement socket");

        let problem = profile
            .validate_network()
            .expect_err("replacement socket must be refused");
        assert!(problem.to_string().contains("changed after preparation"));
    }

    #[test]
    fn the_final_profile_must_fit_the_broker_protocol() {
        let sample = Sample::new("macos-seatbelt-profile-bound");
        let profile = Profile {
            policy: "x".repeat(crucible_sandbox_broker::MACOS_MAX_PROFILE_BYTES),
            definitions: Vec::new(),
            sockets: Vec::new(),
        };

        assert!(profile.with_scratch(sample.root()).is_err());
    }

    #[test]
    fn an_unreadable_glob_denies_new_case_aliases_and_descendants() {
        let sample = Sample::new("macos-seatbelt-unreadable-pattern");
        let root = sample
            .root()
            .canonicalize()
            .expect("canonical fixture root");
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("standard policy")
            .with_unreadable_patterns([SandboxUnreadablePattern::new(
                root.join("**/*.pem"),
                SandboxFilesystemProvenance::Descendant,
            )
            .expect("unreadable pattern")])
            .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[]).expect("Seatbelt profile");

        assert!(profile.policy.contains("([^/]+/)*[^/]*\\.[pP][eE][mM]"));
        assert!(
            profile
                .policy
                .contains("(deny file-read* file-write* file-link (regex")
        );
    }

    #[test]
    fn an_unreadable_private_var_path_also_denies_its_system_alias() {
        let readable = SandboxFilesystemRule::new(
            "/private/var",
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )
        .expect("readable rule");
        let unreadable = SandboxFilesystemRule::new(
            "/private/var/select",
            SandboxFilesystemAccess::Unreadable,
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("unreadable rule");
        let policy = SandboxPolicy::new(
            true,
            [readable, unreadable],
            "/private/var",
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[]).expect("Seatbelt profile");

        assert!(
            profile
                .definitions
                .contains(&OsString::from("DENY_READ_1=/var/select"))
        );
    }

    #[test]
    fn protected_names_are_denied_through_both_private_var_spellings() {
        let writable = SandboxFilesystemRule::new(
            "/private/var/folders/workspace",
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("writable rule");
        let protected = SandboxFilesystemRule::new(
            "/private/var/folders/workspace/.git",
            SandboxFilesystemAccess::Protected,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("protected rule");
        let policy = SandboxPolicy::new(
            true,
            [writable, protected],
            "/private/var/folders/workspace",
            SandboxNetworkPolicy::Closed,
            SandboxResourceLimits::confining(),
        )
        .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[]).expect("Seatbelt profile");

        for root in [
            std::path::Path::new("/private/var/folders/workspace"),
            std::path::Path::new("/var/folders/workspace"),
        ] {
            let regex = protected_metadata_regex(root).expect("protected-name regex");
            assert!(profile.policy.contains(&regex), "missing alias {root:?}");
            assert!(
                profile
                    .policy
                    .contains(&format!("(require-not (regex #\"{regex}\"))")),
                "writable rule does not exclude alias {root:?}"
            );
            assert!(
                profile.policy.contains(&format!(
                    "(deny file-write-unlink file-write-create (regex #\"{regex}\"))"
                )),
                "protected-name moves are not explicitly denied for {root:?}"
            );
        }
    }
}
