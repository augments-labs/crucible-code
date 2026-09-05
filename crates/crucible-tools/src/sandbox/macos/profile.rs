//! Deterministic Seatbelt profiles derived from an immutable sandbox policy.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crucible_core::{SandboxError, SandboxFilesystemAccess, SandboxPolicy};

const MAX_DEFINITIONS: usize = 256;

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
(allow mach-lookup (global-name \"com.apple.system.opendirectoryd.libinfo\"))\n\
(allow mach-lookup (global-name \"com.apple.PowerManagement.control\"))\n\
(allow file-read-data (literal \"/\"))\n\
(allow file-read-metadata file-test-existence\n\
  (literal \"/\")\n\
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
                    let _ = write!(
                        text,
                        "(allow file-write* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))\n\
                         (deny file-write-unlink (require-all (literal (param \"{key}\")) (vnode-type DIRECTORY)))\n"
                    );
                    writes = writes.saturating_add(1);
                }
                SandboxFilesystemAccess::Protected => {
                    let key = format!("DENY_WRITE_{denied_writes}");
                    push_definition(&mut definitions, &key, rule.path())?;
                    let _ = writeln!(
                        text,
                        "(deny file-write* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
                    );
                    denied_writes = denied_writes.saturating_add(1);
                }
                SandboxFilesystemAccess::Unreadable => {
                    let key = format!("DENY_READ_{denied_reads}");
                    push_definition(&mut definitions, &key, rule.path())?;
                    let _ = writeln!(
                        text,
                        "(deny file-read* file-write* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
                    );
                    denied_reads = denied_reads.saturating_add(1);
                }
                SandboxFilesystemAccess::ReadOnly => {}
            }
        }

        for path in discovered_protected {
            let key = format!("DENY_WRITE_{denied_writes}");
            push_definition(&mut definitions, &key, path)?;
            let _ = writeln!(
                text,
                "(deny file-write* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
            );
            denied_writes = denied_writes.saturating_add(1);
        }

        for path in linked_metadata {
            let read_key = format!("LINKED_READ_{}", definitions.len());
            push_definition(&mut definitions, &read_key, path)?;
            let _ = writeln!(
                text,
                "(allow file-read* (require-any (literal (param \"{read_key}\")) (subpath (param \"{read_key}\"))))"
            );
            let write_key = format!("DENY_WRITE_{denied_writes}");
            push_definition(&mut definitions, &write_key, path)?;
            let _ = writeln!(
                text,
                "(deny file-write* (require-any (literal (param \"{write_key}\")) (subpath (param \"{write_key}\"))))"
            );
            denied_writes = denied_writes.saturating_add(1);
        }

        for path in expanded_unreadable {
            let key = format!("DENY_READ_{denied_reads}");
            push_definition(&mut definitions, &key, path)?;
            let _ = writeln!(
                text,
                "(deny file-read* file-write* (require-any (literal (param \"{key}\")) (subpath (param \"{key}\"))))"
            );
            denied_reads = denied_reads.saturating_add(1);
        }
        for pattern in policy.unreadable_patterns() {
            let regex = unreadable_pattern_regex(pattern.pattern())?;
            let _ = writeln!(text, "(deny file-read* file-write* (regex #\"{regex}\"#))");
        }

        // Seatbelt's literal path filters have differed across case-insensitive
        // APFS releases for rename destinations. Keep protected control-plane
        // names closed with one case-insensitive, root-anchored predicate too.
        for rule in policy
            .filesystem()
            .iter()
            .filter(|rule| rule.access() == SandboxFilesystemAccess::ReadWrite)
        {
            let regex = protected_metadata_regex(rule.path())?;
            let _ = writeln!(text, "(deny file-write* (regex #\"{regex}\"#))");
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
                "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal (param \"{key}\"))))"
            );
        }

        Ok(Self {
            policy: text,
            definitions,
        })
    }

    pub(super) fn with_scratch(mut self, path: &std::path::Path) -> Result<Self, SandboxError> {
        push_definition(&mut self.definitions, "SCRATCH", path)?;
        self.policy.push_str(
            "(allow file-write* (require-any (literal (param \"SCRATCH\")) (subpath (param \"SCRATCH\"))))\n\
             (deny file-write-unlink (require-all (literal (param \"SCRATCH\")) (vnode-type DIRECTORY)))\n",
        );
        Ok(self)
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
    if definitions.len() >= MAX_DEFINITIONS {
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
    use crucible_core::{
        SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
        SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits, SandboxUnreadablePattern,
    };

    use super::Profile;
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
        assert_eq!(profile.definitions.len(), 6);
    }

    #[test]
    fn an_unreadable_glob_denies_new_case_aliases_and_descendants() {
        let sample = Sample::new("macos-seatbelt-unreadable-pattern");
        let policy = SandboxPolicy::standard(&sample.workspace())
            .expect("standard policy")
            .with_unreadable_patterns([SandboxUnreadablePattern::new(
                sample.root().join("**/*.pem"),
                SandboxFilesystemProvenance::Descendant,
            )
            .expect("unreadable pattern")])
            .expect("effective policy");

        let profile = Profile::build(&policy, &[], &[], &[]).expect("Seatbelt profile");

        assert!(profile.policy.contains("([^/]+/)*[^/]*\\.[pP][eE][mM]"));
        assert!(
            profile
                .policy
                .contains("(deny file-read* file-write* (regex")
        );
    }
}
