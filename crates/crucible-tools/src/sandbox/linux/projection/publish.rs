//! Bounded changed-path staging, publication, verification, and rollback.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::{
    Entry, Root, Snapshot, authority, copy_file, digest_file, snapshot_filtered, sparse_extents,
};

type ContentKey = (u64, [u8; 32], Vec<(u64, u64)>);

pub(super) fn reconcile(
    roots: &[Root],
    broker_baselines: &[Snapshot],
    finals: &[Snapshot],
) -> io::Result<Vec<Snapshot>> {
    if roots.len() != broker_baselines.len() || roots.len() != finals.len() {
        return Err(invalid(
            "broker baseline or terminal root count does not match the immutable projection plan",
        ));
    }
    roots
        .iter()
        .zip(broker_baselines)
        .zip(finals)
        .map(|((root, broker_baseline), final_snapshot)| {
            validate_broker_baseline(root, broker_baseline)?;
            let entries = final_snapshot
                .entries
                .iter()
                .map(|(path, final_entry)| {
                    let canonical = match (
                        root.baseline.entries.get(path),
                        broker_baseline.entries.get(path),
                    ) {
                        (Some(host), Some(broker)) => merge_entry(host, broker, final_entry),
                        (None, None) => final_entry.clone(),
                        (Some(_), None) | (None, Some(_)) => {
                            return Err(invalid(
                                "broker baseline path set differs from the pinned host baseline",
                            ));
                        }
                    };
                    Ok((path.clone(), canonical))
                })
                .collect::<io::Result<BTreeMap<_, _>>>()?;
            Ok(Snapshot { entries })
        })
        .collect()
}

fn validate_broker_baseline(root: &Root, broker: &Snapshot) -> io::Result<()> {
    if root.baseline.entries.keys().ne(broker.entries.keys()) {
        return Err(invalid(
            "broker baseline path set differs from the pinned host baseline",
        ));
    }
    for ((path, host), broker) in root.baseline.entries.iter().zip(broker.entries.values()) {
        if path.as_os_str().is_empty()
            && matches!(
                (host, broker),
                (Entry::Directory { .. }, Entry::Directory { .. })
            )
        {
            continue;
        }
        if host != broker {
            return Err(invalid(
                "broker baseline semantics differ from the pinned host baseline",
            ));
        }
    }
    Ok(())
}

fn merge_entry(host: &Entry, broker: &Entry, final_entry: &Entry) -> Entry {
    match (host, broker, final_entry) {
        (
            Entry::Directory {
                mode: host_mode,
                modified: host_modified,
            },
            Entry::Directory {
                mode: broker_mode,
                modified: broker_modified,
            },
            Entry::Directory {
                mode: final_mode,
                modified: final_modified,
            },
        ) => Entry::Directory {
            mode: if final_mode == broker_mode {
                *host_mode
            } else {
                *final_mode
            },
            modified: if final_modified == broker_modified {
                *host_modified
            } else {
                *final_modified
            },
        },
        (
            Entry::File {
                mode: host_mode,
                modified: host_modified,
                length: host_length,
                digest: host_digest,
                extents: host_extents,
                linked_to: host_link,
                payload: _,
            },
            Entry::File {
                mode: broker_mode,
                modified: broker_modified,
                length: broker_length,
                digest: broker_digest,
                extents: broker_extents,
                linked_to: broker_link,
                payload: _,
            },
            Entry::File {
                mode: final_mode,
                modified: final_modified,
                length: final_length,
                digest: final_digest,
                extents: final_extents,
                linked_to: final_link,
                payload,
            },
        ) => {
            let unchanged_content = (final_length, final_digest, final_extents)
                == (broker_length, broker_digest, broker_extents);
            Entry::File {
                mode: if final_mode == broker_mode {
                    *host_mode
                } else {
                    *final_mode
                },
                modified: if final_modified == broker_modified {
                    *host_modified
                } else {
                    *final_modified
                },
                length: if unchanged_content {
                    *host_length
                } else {
                    *final_length
                },
                digest: if unchanged_content {
                    *host_digest
                } else {
                    *final_digest
                },
                extents: if unchanged_content {
                    host_extents.clone()
                } else {
                    final_extents.clone()
                },
                linked_to: if final_link == broker_link {
                    host_link.clone()
                } else {
                    final_link.clone()
                },
                payload: payload.clone(),
            }
        }
        (Entry::Symlink(host), Entry::Symlink(broker), Entry::Symlink(final_target)) => {
            if final_target == broker {
                Entry::Symlink(host.clone())
            } else {
                Entry::Symlink(final_target.clone())
            }
        }
        (_, _, _) => final_entry.clone(),
    }
}

pub(super) fn apply(roots: &[Root], stage: &Path, finals: &[Snapshot]) -> io::Result<()> {
    if roots.len() != finals.len() {
        return Err(invalid(
            "terminal scan root count does not match the immutable projection plan",
        ));
    }
    if roots
        .iter()
        .zip(finals)
        .all(|(root, final_snapshot)| &root.baseline == final_snapshot)
    {
        return Ok(());
    }
    for (root, final_snapshot) in roots.iter().zip(finals) {
        validate_root_shape(root, final_snapshot)?;
        if snapshot_filtered(&root.publication_path(), &root.exclusions)? != root.baseline {
            return Err(io::Error::other(format!(
                "writable root changed outside the sandbox before publication; terminal delta category: {}",
                difference(&root.baseline, final_snapshot)
            )));
        }
    }

    let publication = stage.join("publication");
    create_private_directory(&publication)?;
    let mut prepared = Vec::with_capacity(roots.len());
    for (index, (root, final_snapshot)) in roots.iter().zip(finals).enumerate() {
        prepared.push(Prepared::new(
            root,
            final_snapshot,
            &publication.join(index.to_string()),
        )?);
    }

    let mut applied = Vec::new();
    for (index, ((root, final_snapshot), prepared_root)) in
        roots.iter().zip(finals).zip(&prepared).enumerate()
    {
        if &root.baseline == final_snapshot {
            continue;
        }
        if let Err(problem) = apply_snapshot(
            root,
            &root.exclusions,
            final_snapshot,
            &prepared_root.contents,
        ) {
            let rollback = rollback(roots, &prepared, &applied, Some(index));
            return match rollback {
                Ok(()) => Err(problem),
                Err(rollback_problem) => Err(io::Error::other(format!(
                    "publication failed and rollback could not be proved: {problem}; {rollback_problem}"
                ))),
            };
        }
        applied.push(index);
    }
    Ok(())
}

fn difference(left: &Snapshot, right: &Snapshot) -> &'static str {
    if left.entries.len() != right.entries.len() || left.entries.keys().ne(right.entries.keys()) {
        return "path-set";
    }
    for ((path, left), right) in left.entries.iter().zip(right.entries.values()) {
        match (left, right) {
            (Entry::Directory { .. }, Entry::Directory { .. })
            | (Entry::File { .. }, Entry::File { .. })
            | (Entry::Symlink(_), Entry::Symlink(_)) => {}
            _ => return "type",
        }
        if left != right {
            return match (left, right) {
                (
                    Entry::Directory {
                        mode: left_mode,
                        modified: _,
                    },
                    Entry::Directory {
                        mode: right_mode,
                        modified: _,
                    },
                ) if path.as_os_str().is_empty() && left_mode != right_mode => {
                    "root-directory-mode"
                }
                (Entry::Directory { .. }, Entry::Directory { .. })
                    if path.as_os_str().is_empty() =>
                {
                    "root-directory-mtime"
                }
                (Entry::Directory { .. }, Entry::Directory { .. }) => "nested-directory-metadata",
                (Entry::File { .. }, Entry::File { .. }) => "file-semantics",
                (Entry::Symlink(_), Entry::Symlink(_)) => "symlink-target",
                _ => "type",
            };
        }
    }
    "none"
}

struct Prepared {
    contents: ContentStore,
}

impl Prepared {
    fn new(root: &Root, desired: &Snapshot, directory: &Path) -> io::Result<Self> {
        create_private_directory(directory)?;
        let changed = changed_paths(&root.baseline, desired);
        let publication_root = root.publication_path();
        let baseline_sources = content_sources(&publication_root, &root.baseline);
        let mut contents = ContentStore::new(directory.to_path_buf());

        for path in &changed {
            if let Some(Entry::File {
                length,
                digest,
                extents,
                ..
            }) = root.baseline.entries.get(path)
            {
                let source = root_path(&publication_root, path);
                let key = (*length, *digest, extents.clone());
                if root.directory {
                    contents.retain(key, &source)?;
                } else {
                    contents.retain_copy(key, &source)?;
                }
            }
        }
        for path in &changed {
            let Some(Entry::File {
                length,
                digest,
                extents,
                payload,
                ..
            }) = desired.entries.get(path)
            else {
                continue;
            };
            let key = (*length, *digest, extents.clone());
            if contents.contains(&key) {
                continue;
            }
            if let Some(payload) = payload {
                contents.retain(key, payload)?;
                continue;
            }
            let source = baseline_sources
                .get(&key)
                .ok_or_else(|| invalid("terminal scan omitted required changed file content"))?;
            contents.retain(key, source)?;
        }
        Ok(Self { contents })
    }
}

struct ContentStore {
    directory: PathBuf,
    files: BTreeMap<ContentKey, PathBuf>,
}

impl ContentStore {
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            files: BTreeMap::new(),
        }
    }

    fn contains(&self, key: &ContentKey) -> bool {
        self.files.contains_key(key)
    }

    fn retain(&mut self, key: ContentKey, source: &Path) -> io::Result<()> {
        self.retain_with(key, source, true)
    }

    fn retain_copy(&mut self, key: ContentKey, source: &Path) -> io::Result<()> {
        self.retain_with(key, source, false)
    }

    fn retain_with(&mut self, key: ContentKey, source: &Path, allow_link: bool) -> io::Result<()> {
        if self.contains(&key) {
            return Ok(());
        }
        let destination = self.directory.join(self.files.len().to_string());
        match allow_link.then(|| fs::hard_link(source, &destination)) {
            None => copy_file(source, &destination)?,
            Some(Ok(())) => {}
            Some(Err(problem)) if problem.raw_os_error() == Some(18) => {
                copy_file(source, &destination)?;
            }
            Some(Err(problem)) => return Err(problem),
        }
        verify_content(&destination, &key)?;
        self.files.insert(key, destination);
        Ok(())
    }

    fn get(&self, key: &ContentKey) -> io::Result<&Path> {
        self.files
            .get(key)
            .map(PathBuf::as_path)
            .ok_or_else(|| invalid("publication content was not staged"))
    }
}

fn rollback(
    roots: &[Root],
    prepared: &[Prepared],
    applied: &[usize],
    failed: Option<usize>,
) -> io::Result<()> {
    for index in failed.into_iter().chain(applied.iter().rev().copied()) {
        let root = roots
            .get(index)
            .ok_or_else(|| invalid("rollback root index is unavailable"))?;
        let contents = prepared
            .get(index)
            .ok_or_else(|| invalid("rollback content index is unavailable"))?;
        apply_snapshot(root, &root.exclusions, &root.baseline, &contents.contents)?;
        if snapshot_filtered(&root.publication_path(), &root.exclusions)? != root.baseline {
            return Err(invalid(
                "rollback did not restore the baseline semantic view",
            ));
        }
    }
    Ok(())
}

fn apply_snapshot(
    root: &Root,
    exclusions: &[PathBuf],
    desired: &Snapshot,
    contents: &ContentStore,
) -> io::Result<()> {
    let publication_root = root.publication_path();
    let current = snapshot_filtered(&publication_root, exclusions)?;
    let mut removals = current
        .entries
        .iter()
        .filter_map(|(path, entry)| {
            (!path.as_os_str().is_empty()
                && desired
                    .entries
                    .get(path)
                    .is_none_or(|wanted| entry_type(entry) != entry_type(wanted)))
            .then_some((path.clone(), matches!(entry, Entry::Directory { .. })))
        })
        .collect::<Vec<_>>();
    removals.sort_by(|(left, _), (right, _)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
    for (path, directory) in removals {
        if !root.directory {
            return Err(invalid("file-root publication contains a nested removal"));
        }
        authority::remove(root, &path, directory)?;
    }

    for (path, entry) in &desired.entries {
        if path.as_os_str().is_empty() || !matches!(entry, Entry::Directory { .. }) {
            continue;
        }
        if !matches!(current.entries.get(path), Some(Entry::Directory { .. })) {
            if !root.directory {
                return Err(invalid("file-root publication contains a directory"));
            }
            authority::create_directory(root, path)?;
        }
    }

    for (path, entry) in &desired.entries {
        match entry {
            Entry::File {
                linked_to: None, ..
            } => apply_file(root, path, entry, current.entries.get(path), contents)?,
            Entry::Symlink(target) => apply_symlink(root, path, target, current.entries.get(path))?,
            Entry::Directory { .. }
            | Entry::File {
                linked_to: Some(_), ..
            } => {}
        }
    }
    for (path, entry) in &desired.entries {
        let Entry::File {
            linked_to: Some(anchor),
            ..
        } = entry
        else {
            continue;
        };
        if !root.directory {
            return Err(invalid("file-root publication contains a hard-link alias"));
        }
        authority::hard_link(
            root,
            anchor,
            path,
            matches!(current.entries.get(path), Some(Entry::File { .. })),
        )?;
    }

    let mut directories = desired
        .entries
        .iter()
        .filter_map(|(path, entry)| {
            matches!(entry, Entry::Directory { .. }).then_some((path, entry))
        })
        .collect::<Vec<_>>();
    directories.sort_by(|(left, _), (right, _)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| right.cmp(left))
    });
    for (path, entry) in directories {
        apply_metadata(root, path, entry, true)?;
    }
    authority::sync(root)?;
    if snapshot_filtered(&publication_root, exclusions)? == *desired {
        Ok(())
    } else {
        Err(invalid(
            "published writable root does not match the terminal semantic view",
        ))
    }
}

fn apply_file(
    root: &Root,
    relative: &Path,
    entry: &Entry,
    current: Option<&Entry>,
    contents: &ContentStore,
) -> io::Result<()> {
    let Entry::File {
        length,
        digest,
        extents,
        linked_to: None,
        ..
    } = entry
    else {
        return Err(invalid("publication file anchor is invalid"));
    };
    if current == Some(entry) {
        return apply_metadata(root, relative, entry, false);
    }
    let key = (*length, *digest, extents.clone());
    let source = contents.get(&key)?;
    let (mode, modified) = entry_metadata(entry)?;
    if !root.directory {
        if !relative.as_os_str().is_empty() {
            return Err(invalid("file-root publication contains a nested file"));
        }
        return authority::replace_root_file(root, source, (mode, modified));
    }
    let replace = matches!(current, Some(Entry::File { .. }));
    authority::replace_file(root, relative, source, (mode, modified), replace)
}

fn apply_symlink(
    root: &Root,
    relative: &Path,
    target: &OsStr,
    current: Option<&Entry>,
) -> io::Result<()> {
    if current == Some(&Entry::Symlink(target.to_owned())) {
        return Ok(());
    }
    if !root.directory {
        return Err(invalid("file-root publication contains a symbolic link"));
    }
    let replace = matches!(current, Some(Entry::Symlink(_)));
    authority::replace_symlink(root, relative, target, replace)
}

fn apply_metadata(root: &Root, path: &Path, entry: &Entry, directory: bool) -> io::Result<()> {
    let (mode, modified) = entry_metadata(entry)?;
    authority::apply_metadata(root, path, mode, modified, directory)
}

fn entry_metadata(entry: &Entry) -> io::Result<(u32, Option<std::time::SystemTime>)> {
    match entry {
        Entry::Directory { mode, modified } | Entry::File { mode, modified, .. } => {
            Ok((*mode, *modified))
        }
        Entry::Symlink(_) => Err(invalid("symbolic links do not carry publication metadata")),
    }
}

fn changed_paths(baseline: &Snapshot, desired: &Snapshot) -> BTreeSet<PathBuf> {
    baseline
        .entries
        .keys()
        .chain(desired.entries.keys())
        .filter(|path| baseline.entries.get(*path) != desired.entries.get(*path))
        .cloned()
        .collect()
}

fn content_sources(host: &Path, snapshot: &Snapshot) -> BTreeMap<ContentKey, PathBuf> {
    snapshot
        .entries
        .iter()
        .filter_map(|(path, entry)| match entry {
            Entry::File {
                length,
                digest,
                extents,
                ..
            } => Some(((*length, *digest, extents.clone()), root_path(host, path))),
            Entry::Directory { .. } | Entry::Symlink(_) => None,
        })
        .collect()
}

fn validate_root_shape(root: &Root, snapshot: &Snapshot) -> io::Result<()> {
    let entry = snapshot
        .entries
        .get(Path::new(""))
        .ok_or_else(|| invalid("terminal scan omitted its root entry"))?;
    if (root.directory && matches!(entry, Entry::Directory { .. }))
        || (!root.directory && matches!(entry, Entry::File { .. }))
    {
        Ok(())
    } else {
        Err(invalid("terminal scan changed the writable root type"))
    }
}

fn verify_content(path: &Path, expected: &ContentKey) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != expected.0
        || digest_file(path)? != expected.1
        || sparse_extents(path, metadata.len())? != expected.2
    {
        return Err(invalid(
            "staged publication content does not match its seal",
        ));
    }
    Ok(())
}

fn root_path(root: &Path, relative: &Path) -> PathBuf {
    if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

fn entry_type(entry: &Entry) -> u8 {
    match entry {
        Entry::Directory { .. } => 1,
        Entry::File { .. } => 2,
        Entry::Symlink(_) => 3,
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::other(problem)
}
