//! Bounded expansion of the core unreadable wildcard grammar.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

use crucible_core::{SandboxError, SandboxUnreadablePattern};

pub(super) fn expand(patterns: &[SandboxUnreadablePattern]) -> Result<Vec<PathBuf>, SandboxError> {
    const MAX_ENTRIES: usize = 262_144;
    const MAX_DEPTH: usize = 64;

    let mut grouped: BTreeMap<PathBuf, Vec<&SandboxUnreadablePattern>> = BTreeMap::new();
    for pattern in patterns {
        grouped
            .entry(pattern.scan_root().to_path_buf())
            .or_default()
            .push(pattern);
    }

    let mut matched = Vec::new();
    let mut inspected = 0_usize;
    for (root, patterns) in grouped {
        let named = fs::symlink_metadata(&root)
            .map_err(|source| failed("unreadable pattern scan root is unavailable", source))?;
        if named.file_type().is_symlink() || !named.is_dir() {
            return Err(refused(
                "unreadable pattern scan root is not a real directory",
            ));
        }
        let canonical = root.canonicalize().map_err(|source| {
            failed(
                "unreadable pattern scan root could not be canonicalized",
                source,
            )
        })?;
        if canonical != root {
            return Err(refused(
                "unreadable pattern scan root changed after policy resolution",
            ));
        }
        let device = named.dev();
        let mut pending = VecDeque::from([(root, 0_usize)]);
        while let Some((directory, depth)) = pending.pop_front() {
            let entries = fs::read_dir(&directory)
                .map_err(|source| failed("unreadable pattern scan failed", source))?;
            let entries =
                super::super::directory_entries(entries, MAX_ENTRIES.saturating_sub(inspected))
                    .map_err(|source| {
                        failed("unreadable pattern entry could not be read", source)
                    })?;
            for entry in entries {
                inspected = inspected.saturating_add(1);
                if inspected > MAX_ENTRIES {
                    return Err(refused("unreadable pattern expansion exceeded its bound"));
                }
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).map_err(|source| {
                    failed("unreadable pattern path changed during expansion", source)
                })?;
                let selected = patterns.iter().any(|pattern| pattern.matches(&path));
                if metadata.file_type().is_symlink() {
                    if selected {
                        return Err(refused("unreadable pattern selected a symbolic link"));
                    }
                    continue;
                }
                if metadata.dev() != device {
                    return Err(refused(
                        "unreadable pattern scan crossed a filesystem boundary",
                    ));
                }
                if !metadata.is_dir() && !metadata.is_file() {
                    return Err(refused(
                        "unreadable pattern scan encountered a special file",
                    ));
                }
                if metadata.is_file() && metadata.nlink() != 1 {
                    return Err(refused(
                        "unreadable pattern scan encountered a hard-linked file",
                    ));
                }
                if selected {
                    matched.push(path.clone());
                }
                if metadata.is_dir() {
                    if depth >= MAX_DEPTH {
                        return Err(refused(
                            "unreadable pattern expansion exceeded its depth bound",
                        ));
                    }
                    pending.push_back((path, depth.saturating_add(1)));
                }
            }
        }
    }
    matched.sort();
    matched.dedup();
    Ok(matched)
}

fn refused(problem: &'static str) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: None,
    }
}

fn failed(problem: &'static str, source: std::io::Error) -> SandboxError {
    SandboxError::Materialization {
        problem: problem.into(),
        source: Some(source),
    }
}

#[cfg(test)]
mod tests {
    use crucible_core::{SandboxFilesystemProvenance, SandboxUnreadablePattern};

    #[test]
    fn expansion_selects_only_the_matching_existing_paths() {
        let sample = crate::sample::Sample::new("macos-unreadable-expansion");
        sample.write("nested/secret.pem", "secret");
        sample.write("nested/visible.txt", "visible");
        let root = sample
            .root()
            .canonicalize()
            .expect("canonical fixture root");
        let pattern = SandboxUnreadablePattern::new(
            root.join("**/*.pem"),
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("pattern");

        assert_eq!(
            super::expand(&[pattern]).expect("expanded"),
            [root.join("nested/secret.pem")]
        );
    }
}
