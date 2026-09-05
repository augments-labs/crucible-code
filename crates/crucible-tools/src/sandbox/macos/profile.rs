//! Deterministic Seatbelt profiles derived from an immutable sandbox policy.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crucible_core::{SandboxError, SandboxFilesystemAccess, SandboxPolicy};

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

pub(super) struct Profile {
    policy: String,
    definitions: Vec<OsString>,
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

        let profile = Self {
            policy: text,
            definitions,
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

    use crucible_core::{
        SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
        SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits, SandboxUnreadablePattern,
    };

    use super::{Profile, paths_with_system_alias, protected_metadata_regex};
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
            standard.mode(),
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
    fn the_final_profile_must_fit_the_broker_protocol() {
        let sample = Sample::new("macos-seatbelt-profile-bound");
        let profile = Profile {
            policy: "x".repeat(crucible_sandbox_broker::MACOS_MAX_PROFILE_BYTES),
            definitions: Vec::new(),
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
            crucible_core::SandboxMode::Required,
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
            crucible_core::SandboxMode::Required,
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
        }
    }
}
