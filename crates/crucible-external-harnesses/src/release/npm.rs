//! A package tree an agent that Node runs is installed as: read from the lock
//! the agent catalog keeps for it, and unpacked from its packages' tarballs by
//! the rules npm unpacks a package by.
//!
//! # The lock
//!
//! A lock is a catalog asset of at most [`ASSET_BYTES`], which the intake
//! writes once for a version and every client reads. It is one JSON object:
//!
//! - `unpack_rules`: the rules below, which are [`UNPACK_RULES`]. A lock for
//!   any other number, or for none, is refused, since its tree would be built
//!   by rules this reader does not hold.
//! - `packages`: every package of the tree, in the order they are unpacked,
//!   each an object of:
//!   - `location`: where it is installed, from the tree's root:
//!     `node_modules/` and a package's name, then any number of
//!     `/node_modules/` and another's, where a name is one component or a
//!     scope beginning `@` and one component under it. No two locations are
//!     alike, compared case-folded.
//!   - `name`: its name in the registry, in the same form, which is not the
//!     last of its location for a package installed under an alias.
//!   - `version`: its version, printable ASCII with no space, at most
//!     [`SEMVER_BYTES`] long.
//!   - `in_bundle`: `true` for a package that the tarball of a package the
//!     lock installs above it holds, which is the nearest above it unpacked
//!     from a tarball of its own; it then gives none of the fields below, its
//!     files being counted in that package's. Left out, or `false`, for every
//!     other.
//!   - `url`: where the registry publishes its tarball, beginning exactly
//!     `https://registry.npmjs.org/`, then printable ASCII with no space.
//!   - `size`: the tarball's size, in bytes.
//!   - `sha512`: the SHA-512 the intake computed over the tarball it
//!     downloaded, in lowercase hexadecimal, which is its pin. A `sha1` given
//!     without it is refused, since a SHA-1 pins nothing; the registry's
//!     `integrity` is the intake's to check, and is not read here.
//!   - `files`: the most files the tarball writes.
//!   - `unpacked`: the most bytes the tarball writes.
//!
//!   Any other field of a package is ignored.
//!
//! Every number is whole and fits in 64 bits, and the tree's are summed with
//! checked arithmetic as the lock is read: its packages write at most
//! [`TREE_FILES`] files and [`TREE_BYTES`] bytes together, and their tarballs
//! hold at most [`TREE_BYTES`].
//!
//! # Unpacking
//!
//! Each tarball is read from the directory it was downloaded into, named by
//! its SHA-512, and believed only once its size and SHA-512 are its pin's;
//! nothing of it is unpacked before. It is then held to the rules every
//! gzip-compressed tar is held to — one gzip stream and nothing after it, no
//! entry describing a member the tarball does not hold, no name written
//! twice — with npm's in place of the ones that differ:
//!
//! - The first component of every name is stripped, whatever it is, and a
//!   member with nothing after it is skipped, so `package/index.js` is written
//!   as `index.js` under the package's location.
//! - A file is written only where the lock installs from its tarball: the
//!   deepest location the lock gives above it, compared case-folded, is its
//!   package's own or a bundled package's that tarball holds, and what is
//!   below that location does not begin with a `node_modules` component,
//!   whatever its case. So a tarball writes no package the lock does not list,
//!   nothing into another package's location and no file straight into a
//!   `node_modules` directory, while a `node_modules` deeper in a package's
//!   own files, as a test's fixtures may keep one, is written as npm writes
//!   it.
//! - An ordinary file is written, with the directories above it, each made
//!   so only its owner may reach it, as a staged release's are. A directory
//!   entry, a symbolic link and a hard link are skipped and write nothing. A
//!   device, a pipe, a sparse file and every other kind are refused.
//! - A file is runnable exactly when the tarball's mode for it has any execute
//!   bit, and is made on Unix as `0o755`, every other as `0o644`, less what the
//!   umask takes away. Nothing else is made runnable, not even what a
//!   package's `bin` names; no link to it is made, and a `.gitignore` is
//!   written under that name.
//! - Every name, once stripped and again under its location, is held to the
//!   rules every member's name is: printable ASCII alone; relative, with no
//!   backslash, colon or character Windows refuses; no empty, `.` or `..`
//!   component; and none that ends in a dot or a space or names a device
//!   Windows reserves. Since nothing but ASCII is written, no two names can be
//!   spellings of one that Unicode normalises alike, so no such collision can
//!   arise.
//! - The names written are compared case-folded across the whole tree, every
//!   package's together, so two that one file system takes for one are
//!   refused, and so is a name written twice, or as a file and as a directory.
//! - A path is at most [`TREE_PATH_BYTES`] long from the tree's root. Under a
//!   root of at most 255 bytes, which is the installer's to hold it to, with
//!   the separator between them and the NUL that ends it, a path fits in the
//!   1024 bytes macOS and FreeBSD allow one. On Windows the standard library
//!   reaches a path that long through `\\?\`, so 260 characters is no
//!   ceiling there.
//! - A package writes no more files and bytes than its entry gives it, and no
//!   more is read out of the tree's tarballs than [`TREE_DECOMPRESSED`], which
//!   bounds their headers, their padding and what is skipped as well.
//!
//! Each file is given as it was written, with its size, exec flag and the
//! SHA-256 computed as it was, as a version's receipt records it.
//!
//! The intake builds a tree with this same reader, through
//! `crucible agents unpack-check`, so the tree it compared with npm's is the
//! tree a client builds.
//!
//! The root is the caller's: this writes only under it, makes nothing that is
//! already there, and leaves removing it after a refusal to its owner.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, Read as _, Seek as _};
use std::path::{Path, PathBuf};

use flate2::bufread::GzDecoder;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha512};
use tar::EntryType;

use super::unpack::{
    Ahead, Bounded, Budget, Held, MEMBERS, Names, copied, ended, made, named, shown,
};
use super::{RELEASE_BYTES, UnpackError, hex};
use crate::catalog::{ASSET_BYTES, File, UNPACK_RULES};

/// The most files a tree writes, all its packages together: the most names
/// the unpacker writes for one artifact, which keeps a tree's receipt the size
/// of a release's.
pub(super) const TREE_FILES: u64 = MEMBERS as u64;

/// The most names a tree holds, each directory a file only implies counted:
/// as many again as it writes files.
const TREE_NAMES: usize = 2 * MEMBERS;

/// The most bytes a tree writes, all its packages together, and the most its
/// tarballs hold: as much as a release may.
pub(super) const TREE_BYTES: u64 = RELEASE_BYTES;

/// The most bytes read out of a tree's tarballs once decompressed: twice what
/// it writes, for their headers, their padding and what is skipped.
const TREE_DECOMPRESSED: u64 = 2 * TREE_BYTES;

/// The longest path in a tree, from its root, in bytes.
pub(super) const TREE_PATH_BYTES: usize = 767;

/// The longest version, in bytes, which is as long as npm reads one.
const SEMVER_BYTES: usize = 256;

/// Where every tarball is fetched from.
const REGISTRY: &str = "https://registry.npmjs.org/";

/// The length of a SHA-512 in hexadecimal.
const SHA512_DIGITS: usize = 128;

/// A lock as the agent catalog keeps one: every package of one tree, where
/// each is installed and the tarball it is unpacked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmTree {
    packages: Vec<NpmPackage>,
}

/// One package of a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmPackage {
    /// Where it is installed, from the tree's root.
    pub location: Box<str>,
    /// Its name in the registry, which an alias installs under another.
    pub name: Box<str>,
    /// Its version.
    pub version: Box<str>,
    /// The tarball it is unpacked from; none for a package that the tarball
    /// of a package above it holds.
    pub tarball: Option<NpmTarball>,
}

/// The tarball a package is unpacked from, and what it may write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmTarball {
    /// Where the registry publishes it.
    pub url: Box<str>,
    /// Its size, in bytes.
    pub size: u64,
    /// Its SHA-512, in lowercase hexadecimal, and the name it is kept under
    /// once downloaded.
    pub sha512: Box<str>,
    /// The most files it writes.
    pub files: u64,
    /// The most bytes it writes.
    pub unpacked: u64,
}

/// Why a lock was not read, or its tree not unpacked.
#[derive(Debug, thiserror::Error)]
pub enum NpmError {
    /// The lock is larger than a catalog asset may be.
    #[error("the package lock is larger than the {ceiling} bytes a catalog asset may be")]
    TooLarge {
        /// The most it may be.
        ceiling: u64,
    },
    /// The lock is not JSON.
    #[error("the package lock is not JSON")]
    Json(#[source] serde_json::Error),
    /// The lock is for rules of unpacking this reader does not hold.
    #[error(
        "the package lock is for unpack rules {given:?}, and only rules {UNPACK_RULES} are known"
    )]
    Rules {
        /// The rules it gives, cut short.
        given: String,
    },
    /// A field of the lock is not what the format says.
    #[error("the package lock's {field} {why}")]
    Lock {
        /// The field.
        field: String,
        /// What is wrong with it.
        why: &'static str,
    },
    /// A name the lock gives is not one a package may be installed under.
    #[error("the package lock's {field} is not a name a package may be installed under")]
    Name {
        /// The field.
        field: String,
        /// The rule it breaks.
        #[source]
        source: UnpackError,
    },
    /// A package is pinned by a SHA-1 alone, which pins nothing.
    #[error("the package at {location:?} is pinned by a SHA-1 alone, which pins nothing")]
    Sha1 {
        /// Where it is installed, cut short.
        location: String,
    },
    /// A package's tarball is not fetched from the registry.
    #[error("the package at {location:?} is not fetched from {REGISTRY}")]
    Registry {
        /// Where it is installed, cut short.
        location: String,
    },
    /// The tree is past one of its ceilings.
    #[error("the tree holds more than the {ceiling} {what} it may")]
    Tree {
        /// What it holds too much of.
        what: &'static str,
        /// The most it may hold.
        ceiling: u64,
    },
    /// A path in the tree is longer than one may be.
    #[error("the tree holds a path longer than {ceiling} bytes, {name:?}")]
    Path {
        /// The path, cut short.
        name: String,
        /// The most it may be.
        ceiling: usize,
    },
    /// A tarball could not be read.
    #[error("the tarball of the package at {location:?} could not be read from {}", .path.display())]
    Read {
        /// Where its package is installed, cut short.
        location: String,
        /// Where it was read from.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
    /// A tarball's size is not its pin's.
    #[error(
        "the tarball of the package at {location:?} is {held} bytes, not the {pinned} it was pinned at"
    )]
    Size {
        /// Where its package is installed, cut short.
        location: String,
        /// What it holds, read no further than one byte past its pin.
        held: u64,
        /// What the lock says.
        pinned: u64,
    },
    /// A tarball's SHA-512 is not its pin's.
    #[error("the tarball of the package at {location:?} is not the one the lock pinned")]
    Digest {
        /// Where its package is installed, cut short.
        location: String,
    },
    /// A package writes more than its entry gives it.
    #[error("the package at {location:?} writes more than the {ceiling} {what} the lock gives it")]
    Package {
        /// Where it is installed, cut short.
        location: String,
        /// What it writes too much of.
        what: &'static str,
        /// The most the lock gives it.
        ceiling: u64,
    },
    /// A tarball writes where the lock does not install from it.
    #[error(
        "the tarball of the package at {location:?} writes {path:?}, which the lock does not install from it"
    )]
    Outside {
        /// Where its package is installed, cut short.
        location: String,
        /// What it writes, from the tree's root, cut short.
        path: String,
    },
    /// A tarball breaks a rule every archive is held to.
    #[error("the tarball of the package at {location:?} was refused")]
    Tarball {
        /// Where its package is installed, cut short.
        location: String,
        /// The rule it breaks.
        #[source]
        source: UnpackError,
    },
}

impl NpmTree {
    /// The tree the lock `bytes` describes, once every field of it is what the
    /// format says and the tree is within its ceilings.
    ///
    /// # Errors
    ///
    /// [`NpmError::TooLarge`] and [`NpmError::Json`] for a lock that cannot
    /// be read, [`NpmError::Rules`] for one for other rules, and for the
    /// first field that is not what the format says: [`NpmError::Lock`],
    /// [`NpmError::Name`], [`NpmError::Path`], [`NpmError::Sha1`] or
    /// [`NpmError::Registry`]. [`NpmError::Tree`] for a tree past a ceiling.
    pub fn parse(bytes: &[u8]) -> Result<Self, NpmError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > ASSET_BYTES {
            return Err(NpmError::TooLarge {
                ceiling: ASSET_BYTES,
            });
        }
        let document: Value = serde_json::from_slice(bytes).map_err(NpmError::Json)?;
        let Value::Object(document) = document else {
            return Err(wrong("lock", "is not a JSON object"));
        };
        match document.get("unpack_rules") {
            None => return Err(wrong("unpack_rules", "is missing")),
            Some(rules) if rules.as_u64() != Some(UNPACK_RULES) => {
                return Err(NpmError::Rules {
                    given: shown(rules.to_string().as_bytes()),
                });
            }
            Some(_) => {}
        }
        let Some(Value::Array(listed)) = document.get("packages") else {
            return Err(wrong("packages", "is not a list"));
        };
        let mut packages = Vec::with_capacity(listed.len());
        let mut locations = HashSet::with_capacity(listed.len());
        for (index, package) in listed.iter().enumerate() {
            let at = format!("packages[{index}]");
            let Value::Object(fields) = package else {
                return Err(wrong(&at, "is not an object"));
            };
            let package = read_package(&Fields { fields, at }, &mut locations)?;
            packages.push(package);
        }
        let tree = Self { packages };
        tree.bundled()?;
        tree.within()?;
        Ok(tree)
    }

    /// Every package, in the lock's order.
    #[must_use]
    pub fn packages(&self) -> &[NpmPackage] {
        &self.packages
    }

    /// Unpacks every package's tarball, each kept in `tarballs` under its
    /// SHA-512, into its location under `root`, and gives every file written,
    /// in the order it was.
    ///
    /// # Errors
    ///
    /// [`NpmError::Read`], [`NpmError::Size`] and [`NpmError::Digest`] for a
    /// tarball that is not its pin, [`NpmError::Path`] for a path past its
    /// ceiling, [`NpmError::Outside`] for a tarball that writes where the lock
    /// does not install from it, [`NpmError::Package`] for a package that
    /// writes more than its entry gives it, and [`NpmError::Tarball`] for a
    /// tarball that breaks a rule every archive is held to, or that could not
    /// be written. What was written before is left under `root`.
    pub fn unpack(&self, tarballs: &Path, root: &Path) -> Result<Vec<File>, NpmError> {
        let unpacked = self.unpacked();
        let mut unpacking = Unpacking {
            root,
            budget: Budget::new(TREE_DECOMPRESSED),
            names: Names::new(TREE_NAMES),
            writers: self
                .packages
                .iter()
                .filter_map(|package| {
                    let writer = holder(&unpacked, package)?;
                    Some((package.location.to_ascii_lowercase(), writer))
                })
                .collect(),
            files: Vec::new(),
        };
        for package in &self.packages {
            let Some(tarball) = &package.tarball else {
                continue;
            };
            let location = shown(package.location.as_bytes());
            let path = tarballs.join(&*tarball.sha512);
            let unread = |source| NpmError::Read {
                location: location.clone(),
                path: path.clone(),
                source,
            };
            let mut file = fs::File::open(&path).map_err(unread)?;
            pinned(&mut file, tarball, &location).map_err(|error| match error {
                Pinned::Read(source) => unread(source),
                Pinned::Npm(error) => error,
            })?;
            let reader = io::BufReader::new(file.take(tarball.size));
            unpacking.package(package, tarball, reader)?;
        }
        Ok(unpacking.files)
    }

    /// Where every package the lock unpacks from a tarball is installed.
    fn unpacked(&self) -> HashSet<&str> {
        self.packages
            .iter()
            .filter(|package| package.tarball.is_some())
            .map(|package| &*package.location)
            .collect()
    }

    /// Refuses a bundled package that no package the lock installs above it
    /// can hold.
    fn bundled(&self) -> Result<(), NpmError> {
        let unpacked = self.unpacked();
        for (index, package) in self.packages.iter().enumerate() {
            if holder(&unpacked, package).is_none() {
                return Err(wrong(
                    &format!("packages[{index}].location"),
                    "is bundled in no package the lock installs above it",
                ));
            }
        }
        Ok(())
    }

    /// Refuses a tree whose packages write more files or bytes than a tree
    /// may, or whose tarballs hold more, each summed with checked arithmetic.
    fn within(&self) -> Result<(), NpmError> {
        let tarballs = self
            .packages
            .iter()
            .filter_map(|package| package.tarball.as_ref());
        let summed = |what, ceiling, counted: fn(&NpmTarball) -> u64| {
            tarballs
                .clone()
                .try_fold(0_u64, |total, tarball| {
                    total
                        .checked_add(counted(tarball))
                        .filter(|total| *total <= ceiling)
                })
                .map(drop)
                .ok_or(NpmError::Tree { what, ceiling })
        };
        summed("files", TREE_FILES, |tarball| tarball.files)?;
        summed("bytes", TREE_BYTES, |tarball| tarball.unpacked)?;
        summed("bytes of tarballs", TREE_BYTES, |tarball| tarball.size)
    }
}

/// Where the package whose tarball writes `package` is installed: its own
/// location for a package with a tarball, and for a bundled one the nearest
/// above it of `unpacked`, where the lock installs a package from a tarball,
/// if there is one.
fn holder<'p>(unpacked: &HashSet<&str>, package: &'p NpmPackage) -> Option<&'p str> {
    if package.tarball.is_some() {
        return Some(&package.location);
    }
    let mut above = &*package.location;
    loop {
        let (parent, _) = above.rsplit_once("/node_modules/")?;
        if unpacked.contains(parent) {
            return Some(parent);
        }
        above = parent;
    }
}

/// A refusal of the lock's `field`, for `why`.
fn wrong(field: &str, why: &'static str) -> NpmError {
    NpmError::Lock {
        field: field.to_owned(),
        why,
    }
}

/// One package's object in the lock, and where in the lock it is.
struct Fields<'a> {
    fields: &'a Map<String, Value>,
    at: String,
}

impl Fields<'_> {
    /// The name of `field` in the lock.
    fn field(&self, field: &str) -> String {
        format!("{}.{field}", self.at)
    }

    /// A refusal of `field`, for `why`.
    fn wrong(&self, field: &str, why: &'static str) -> NpmError {
        wrong(&self.field(field), why)
    }

    /// The text `field` holds.
    fn text(&self, field: &str) -> Result<&str, NpmError> {
        self.fields
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| self.wrong(field, "is not text"))
    }

    /// The whole number `field` holds.
    fn number(&self, field: &str) -> Result<u64, NpmError> {
        self.fields
            .get(field)
            .and_then(Value::as_u64)
            .ok_or_else(|| self.wrong(field, "is not a whole number"))
    }

    /// Whether `field` is given.
    fn has(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }
}

/// The package `fields` describes, its location not one of `locations`, which
/// it is then added to case-folded.
fn read_package(
    fields: &Fields<'_>,
    locations: &mut HashSet<String>,
) -> Result<NpmPackage, NpmError> {
    let location = installed(fields)?;
    if !locations.insert(location.to_ascii_lowercase()) {
        return Err(fields.wrong(
            "location",
            "is another package's, or differs from one only in case",
        ));
    }
    let name = fields.text("name")?;
    if package_name(name).is_none_or(|rest| !rest.is_empty()) {
        return Err(fields.wrong("name", "is not a package's name"));
    }
    named(name.as_bytes(), false).map_err(|source| NpmError::Name {
        field: fields.field("name"),
        source,
    })?;
    let version = fields.text("version")?;
    if version.is_empty()
        || version.len() > SEMVER_BYTES
        || !version.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(fields.wrong("version", "is not a version"));
    }
    let in_bundle = match fields.fields.get("in_bundle") {
        None => false,
        Some(Value::Bool(in_bundle)) => *in_bundle,
        Some(_) => return Err(fields.wrong("in_bundle", "is not true or false")),
    };
    let tarball = if in_bundle {
        if ["url", "size", "sha512", "sha1", "files", "unpacked"]
            .iter()
            .any(|field| fields.has(field))
        {
            return Err(fields.wrong("in_bundle", "is true, and the package gives a tarball"));
        }
        None
    } else {
        Some(read_tarball(fields, &location)?)
    };
    Ok(NpmPackage {
        location: location.into(),
        name: name.into(),
        version: version.into(),
        tarball,
    })
}

/// The tarball `fields` pins for the package installed at `location`.
fn read_tarball(fields: &Fields<'_>, location: &str) -> Result<NpmTarball, NpmError> {
    let location = || shown(location.as_bytes());
    if fields.has("sha1") && !fields.has("sha512") {
        return Err(NpmError::Sha1 {
            location: location(),
        });
    }
    let url = fields.text("url")?;
    let Some(path) = url.strip_prefix(REGISTRY) else {
        return Err(NpmError::Registry {
            location: location(),
        });
    };
    if path.is_empty() || !path.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(fields.wrong("url", "is not where the registry publishes a tarball"));
    }
    let sha512 = fields.text("sha512")?;
    if sha512.len() != SHA512_DIGITS
        || !sha512
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(fields.wrong("sha512", "is not a SHA-512 in lowercase hexadecimal"));
    }
    Ok(NpmTarball {
        url: url.into(),
        size: fields.number("size")?,
        sha512: sha512.into(),
        files: fields.number("files")?,
        unpacked: fields.number("unpacked")?,
    })
}

/// Where `fields` says its package is installed, once that is a location the
/// format allows.
fn installed(fields: &Fields<'_>) -> Result<String, NpmError> {
    let raw = fields.text("location")?;
    if raw.len() > TREE_PATH_BYTES {
        return Err(NpmError::Path {
            name: shown(raw.as_bytes()),
            ceiling: TREE_PATH_BYTES,
        });
    }
    let mut rest = raw;
    loop {
        let after = rest
            .strip_prefix("node_modules/")
            .and_then(package_name)
            .ok_or_else(|| fields.wrong("location", "is not where npm installs a package"))?;
        match after.strip_prefix('/') {
            None if after.is_empty() => break,
            Some(more) => rest = more,
            None => return Err(fields.wrong("location", "is not where npm installs a package")),
        }
    }
    named(raw.as_bytes(), false).map_err(|source| NpmError::Name {
        field: fields.field("location"),
        source,
    })
}

/// What follows the package's name `text` begins with: one component, or a
/// scope of `@` and more and one component under it.
fn package_name(text: &str) -> Option<&str> {
    let (first, rest) = text.split_once('/').unwrap_or((text, ""));
    if first.is_empty() || first == "node_modules" {
        return None;
    }
    if !first.starts_with('@') {
        return Some(text.get(first.len()..).unwrap_or_default());
    }
    if first.len() == 1 {
        return None;
    }
    let second = rest.split('/').next().unwrap_or(rest);
    if second.is_empty() {
        return None;
    }
    text.get(first.len() + 1 + second.len()..)
}

/// Why a tarball was not held to be its pin.
enum Pinned {
    /// It could not be read.
    Read(io::Error),
    /// It is not its pin.
    Npm(NpmError),
}

/// Whether what `file` holds is `tarball`'s pin, read from its start within
/// one byte past the pinned size, and the file is left at its start again.
fn pinned(file: &mut fs::File, tarball: &NpmTarball, location: &str) -> Result<(), Pinned> {
    let mut digest = Sha512::new();
    let mut held: u64 = 0;
    let mut reading = io::Read::take(&mut *file, tarball.size.saturating_add(1));
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = match reading.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Pinned::Read(error)),
        };
        digest.update(chunk.get(..read).unwrap_or_default());
        held = held.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    if held != tarball.size {
        return Err(Pinned::Npm(NpmError::Size {
            location: location.to_owned(),
            held,
            pinned: tarball.size,
        }));
    }
    if hex(&digest.finalize()) != *tarball.sha512 {
        return Err(Pinned::Npm(NpmError::Digest {
            location: location.to_owned(),
        }));
    }
    file.seek(io::SeekFrom::Start(0)).map_err(Pinned::Read)?;
    Ok(())
}

/// A tree being unpacked: where, what has been read out of its tarballs and
/// what has been written, every package's together, and for every location,
/// case-folded, where the package whose tarball writes it is installed.
struct Unpacking<'r> {
    root: &'r Path,
    budget: Budget,
    names: Names,
    writers: HashMap<String, &'r str>,
    files: Vec<File>,
}

impl Unpacking<'_> {
    /// Unpacks `package` from `reader`, its `tarball` once pinned.
    fn package<R: BufRead>(
        &mut self,
        package: &NpmPackage,
        tarball: &NpmTarball,
        reader: R,
    ) -> Result<(), NpmError> {
        let Self {
            root,
            budget,
            names,
            writers,
            files,
        } = self;
        let budget = &*budget;
        let location = &*package.location;
        let refused = |source| NpmError::Tarball {
            location: shown(location.as_bytes()),
            source,
        };
        let past = |what, ceiling| NpmError::Package {
            location: shown(location.as_bytes()),
            what,
            ceiling,
        };
        let mut archive = tar::Archive::new(Bounded {
            inner: GzDecoder::new(reader),
            budget,
        });
        let mut ahead = Ahead::default();
        let entries = archive
            .entries()
            .map_err(|error| refused(budget.failed(error)))?
            .raw(true);
        let (mut written, mut bytes) = (0_u64, 0_u64);
        for entry in entries {
            let mut entry = entry.map_err(|error| refused(budget.failed(error)))?;
            let kind = entry.header().entry_type();
            if ahead.took(kind, &mut entry, budget).map_err(refused)? {
                continue;
            }
            let (raw, _) = ahead.named(&entry).map_err(refused)?;
            let not_written = |kind| {
                refused(UnpackError::Kind {
                    name: shown(&raw),
                    kind,
                })
            };
            match kind {
                EntryType::Regular => {}
                // npm makes the directories a file needs, and no link.
                EntryType::Directory | EntryType::Symlink | EntryType::Link => continue,
                // What follows a sparse member's header is read by `tar` as
                // more of its map, so it is refused before anything after it
                // is read as a member.
                EntryType::GNUSparse => return Err(not_written("a sparse file")),
                EntryType::Char | EntryType::Block => return Err(not_written("a device")),
                EntryType::Fifo => return Err(not_written("a pipe")),
                _ => return Err(not_written("of a kind no unpacker writes")),
            }
            let Some(at) = raw.iter().position(|byte| *byte == b'/') else {
                continue;
            };
            let stripped = raw.get(at + 1..).unwrap_or_default();
            if stripped.is_empty() {
                continue;
            }
            let name = named(stripped, false).map_err(refused)?;
            let path = format!("{location}/{name}");
            if path.len() > TREE_PATH_BYTES {
                return Err(NpmError::Path {
                    name: shown(path.as_bytes()),
                    ceiling: TREE_PATH_BYTES,
                });
            }
            let path = named(path.as_bytes(), false).map_err(refused)?;
            if !writes(writers, location, &path) {
                return Err(NpmError::Outside {
                    location: shown(location.as_bytes()),
                    path: shown(path.as_bytes()),
                });
            }
            let size = entry.size();
            let exec = entry
                .header()
                .mode()
                .map_err(|error| refused(UnpackError::Corrupt(error)))?
                & 0o111
                != 0;
            written = written
                .checked_add(1)
                .filter(|written| *written <= tarball.files)
                .ok_or_else(|| past("files", tarball.files))?;
            bytes = bytes
                .checked_add(size)
                .filter(|bytes| *bytes <= tarball.unpacked)
                .ok_or_else(|| past("bytes", tarball.unpacked))?;
            // Its data comes out of the tarball, so none of it is written
            // where all of it could not be read.
            if size > budget.left.get() {
                return Err(refused(UnpackError::Decompressed {
                    ceiling: budget.ceiling,
                }));
            }
            names.claim(&path, Held::Other).map_err(refused)?;
            let (above, last) = path.rsplit_once('/').unwrap_or(("", &path));
            let file = made(root, above).map_err(refused)?.join(last);
            let kind = copied(&file, &mut entry, size, exec, &|error| budget.failed(error))
                .map_err(refused)?;
            files.push(File {
                path: path.into(),
                kind,
            });
        }
        ended(archive, &ahead, budget).map_err(refused)
    }
}

/// Whether the tarball of the package installed at `location` may write
/// `path`, from the tree's root, by `writers`: the deepest location above it,
/// compared case-folded, is written from that tarball, and what is below that
/// location does not begin with a `node_modules` component, whatever its case.
fn writes(writers: &HashMap<String, &str>, location: &str, path: &str) -> bool {
    let folded = path.to_ascii_lowercase();
    // A location deeper than the package's own goes on from it with
    // `/node_modules/` and a package's name, so one is looked for only there.
    let (mut end, mut deepest) = (location.len(), location.len());
    while let Some(after) = folded
        .get(end..)
        .and_then(|rest| rest.strip_prefix("/node_modules/"))
    {
        let Some(rest) = package_name(after).filter(|rest| rest.starts_with('/')) else {
            break;
        };
        end = folded.len() - rest.len();
        if folded
            .get(..end)
            .is_some_and(|above| writers.contains_key(above))
        {
            deepest = end;
        }
    }
    let written = folded
        .get(..deepest)
        .and_then(|above| writers.get(above))
        .is_some_and(|writer| *writer == location);
    let below = folded.get(deepest + 1..).unwrap_or_default();
    written && below.split('/').next() != Some("node_modules")
}
