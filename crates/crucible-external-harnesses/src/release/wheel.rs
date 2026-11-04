//! The wheels of a release a pinned interpreter starts, unpacked into the
//! staging directory its version is made in, and the launcher it is started
//! through.
//!
//! A wheel is a zip, and [`Wheels`] walks it with the walk [`super::Unpacker`]
//! walks a zip with, so each member is held to every rule an archive's is,
//! its pin first, under the name it is installed as rather than the one it
//! has in the wheel:
//!
//! - A member at the wheel's root, its `.dist-info` directory among them, is
//!   installed under `site/`, and so is one under the `purelib` or `platlib`
//!   part of its `.data` directory, with `<name>.data/<part>/` taken off. Any
//!   other part, such as `scripts`, `headers` or `data`, is refused, naming
//!   the part: none of them is a module, and nothing else of a wheel is
//!   installed.
//! - Two members installed on one path are refused, as two an archive names
//!   alike are, and so is a member installed where another artifact of the
//!   version wrote already, since nothing is created that is there.
//! - The file list it is held to, and the list a survey records, name the
//!   paths as installed.
//! - Its `RECORD`, the one file `RECORD` at the top of a `.dist-info`
//!   directory at its root, must agree with what was written: every file
//!   written but that one, `RECORD.jws` and `RECORD.p7s` has exactly one row,
//!   whose hash is `sha256=` and its SHA-256 in URL-safe base64 without
//!   padding and whose size is its size, and every row names a file written.
//!   It is read once written, so it is bounded as every member is, and each
//!   row by [`ROW_BYTES`]. It is checked for consistency alone: what makes the
//!   bytes believed is the pin.
//!
//! [`LaunchScript`] is the one file crucible writes into such a version,
//! [`LAUNCHER`] at its top and not runnable. It is a fixed template into
//! which two values go, each as a Python string literal: the agent's key,
//! whose characters need none escaped, and the entry point, held before
//! anything is written to `module:attribute`, each side one or more ASCII
//! identifiers joined by single dots. It is written only for an interpreter
//! the catalog gives exactly `-I` and nothing else, which starts it isolated,
//! so that what the workspace holds is never on its path, and runs the
//! launcher before anything else; [`LaunchScript::admits`] decides that for
//! the install and for every start alike. It finds its own directory from its
//! own `__file__`, adds `site/` there as a site directory, sets `sys.argv` to
//! the key and the arguments it was started with after its own path, then
//! imports the module, resolves the attribute and exits with what calling it
//! returns, as a console script does.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead as _, Cursor, Read as _, Write as _};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use crucible_types::AgentKey;
use sha2::{Digest as _, Sha256};

use super::unpack::{NAME_BYTES, Walk, created, listing, pinned, pinned_file, shown};
use super::{Staged, UnpackError, hex};
use crate::catalog::{Artifact, File, FileKind, Files};

/// The launcher's name, at the top of the version's directory.
pub const LAUNCHER: &str = "launch.py";

/// The directory under the version's that every wheel is installed into.
const SITE: &str = "site";

/// The most a row of `RECORD` may hold: a path at its longest, quoted, and
/// room for a hash, a size and the line's end.
const ROW_BYTES: usize = NAME_BYTES + 128;

/// The files of a `.dist-info` directory that need no row of their own, and
/// whose rows are not checked: `RECORD` itself and its signatures.
const UNRECORDED: [&str; 3] = ["RECORD", "RECORD.jws", "RECORD.p7s"];

/// What the launcher holds before the two values written into it.
const LAUNCHER_HEAD: &str = "\
# Written by crucible: the pinned interpreter runs this file, which starts
# the agent from the wheels unpacked into site/ beside it.
import importlib
import os
import site
import sys

";

/// What the launcher holds after them.
const LAUNCHER_TAIL: &str = "
here = os.path.dirname(os.path.abspath(__file__))
site.addsitedir(os.path.join(here, \"site\"))
sys.argv = [AGENT] + sys.argv[1:]
module, _, attribute = ENTRY_POINT.partition(\":\")
target = importlib.import_module(module)
for part in attribute.split(\".\"):
    target = getattr(target, part)
sys.exit(target())
";

/// The reader for wheels.
#[derive(Debug, Clone, Copy)]
pub struct Wheels;

/// The launcher of one version of a release a pinned interpreter starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchScript {
    text: String,
}

impl Wheels {
    /// Unpacks `bytes`, the wheel `artifact` pins, into `staging`, a
    /// directory the caller made and may already have unpacked other
    /// artifacts of the version into, exactly as its file list gives it.
    ///
    /// # Errors
    ///
    /// [`UnpackError`] naming the first way the download is not the wheel
    /// listed, or could not be read or written. What was written before the
    /// refusal stays under `staging` for its owner to remove.
    pub fn unpack(
        bytes: &[u8],
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned(bytes, artifact)?;
        let Files::Listed(files) = &artifact.files else {
            return Err(UnpackError::Asset);
        };
        let list = listing(files, artifact.unpacked)?;
        walked(artifact, staging, Some(list), Cursor::new(bytes))
    }

    /// Unpacks the wheel `artifact` pins, read from `file` rather than held in
    /// memory, into `staging`, exactly as [`Wheels::unpack`] does: the file is
    /// read through once for its size and SHA-256 before anything else is
    /// done with it, and then read again as the wheel.
    ///
    /// # Errors
    ///
    /// As [`Wheels::unpack`], and [`UnpackError::Corrupt`] where the file
    /// could not be read.
    pub fn unpack_from(
        file: &mut fs::File,
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned_file(file, artifact)?;
        let Files::Listed(files) = &artifact.files else {
            return Err(UnpackError::Asset);
        };
        let list = listing(files, artifact.unpacked)?;
        walked(artifact, staging, Some(list), io::BufReader::new(file))
    }

    /// Unpacks `bytes`, the wheel `artifact` pins, into `staging` by every
    /// rule [`Wheels::unpack`] holds a wheel to but its file list, which is
    /// not read, and gives the list it would then accept.
    ///
    /// # Errors
    ///
    /// [`UnpackError`] naming the first rule the download breaks, or what could
    /// not be read or written.
    pub fn survey(
        bytes: &[u8],
        artifact: &Artifact,
        staging: &Path,
    ) -> Result<Staged, UnpackError> {
        pinned(bytes, artifact)?;
        walked(artifact, staging, None, Cursor::new(bytes))
    }
}

/// `artifact`, a wheel read from `reader`, walked into `staging` and held to
/// `list` where there is one, with each member installed where [`installed`]
/// puts it, once its `RECORD` agrees with what was written.
fn walked<R: io::BufRead + io::Seek>(
    artifact: &Artifact,
    staging: &Path,
    list: Option<HashMap<&str, &FileKind>>,
    reader: R,
) -> Result<Staged, UnpackError> {
    // Each file's name in the wheel, by the path it is installed as.
    let mut origins: HashMap<String, String> = HashMap::new();
    let staged = {
        let mut place = |name: &str, directory: bool| {
            let placed = installed(name, directory)?;
            if let Some(placed) = placed.as_ref().filter(|_| !directory) {
                origins.insert(placed.clone(), name.to_owned());
            }
            Ok(placed)
        };
        Walk::new(artifact, staging, list)
            .placing(&mut place)
            .run(reader)?
    };
    recorded(&staged, &origins)?;
    Ok(staged)
}

/// Where the member a wheel names `name` is installed, under the staging
/// directory: none for a directory that is the `.data` directory or its
/// `purelib` or `platlib`, which hold what is installed and are not. A top
/// directory ending `.data` in any case is taken for the `.data` directory,
/// so a part of it is not installed under `site/` by being spelt otherwise.
fn installed(name: &str, directory: bool) -> Result<Option<String>, UnpackError> {
    let (top, rest) = name
        .split_once('/')
        .map_or((name, None), |(top, rest)| (top, Some(rest)));
    let data = top
        .rsplit_once('.')
        .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("data"));
    if !data {
        return Ok(Some(format!("{SITE}/{name}")));
    }
    let refused = |part: &str| UnpackError::Data {
        name: shown(name.as_bytes()),
        part: shown(part.as_bytes()),
    };
    let Some(rest) = rest else {
        return if directory {
            Ok(None)
        } else {
            Err(refused(top))
        };
    };
    let (part, path) = rest
        .split_once('/')
        .map_or((rest, None), |(part, path)| (part, Some(path)));
    if !matches!(part, "purelib" | "platlib") {
        return Err(refused(part));
    }
    match path {
        Some(path) => Ok(Some(format!("{SITE}/{path}"))),
        None if directory => Ok(None),
        None => Err(refused(part)),
    }
}

/// Whether the `RECORD` of the wheel `staged` holds agrees with what was
/// written, each file named as `origins` gives its name in the wheel.
fn recorded(staged: &Staged, origins: &HashMap<String, String>) -> Result<(), UnpackError> {
    // Each file written, by its name in the wheel, with its size and SHA-256.
    let mut unrecorded: HashMap<&str, (u64, &str)> = staged
        .files()
        .iter()
        .filter_map(|file| match &file.kind {
            FileKind::File { size, sha256, .. } => origins
                .get(&*file.path)
                .map(|origin| (origin.as_str(), (*size, &**sha256))),
            FileKind::Dir | FileKind::Link { .. } => None,
        })
        .collect();
    let records: Vec<&str> = unrecorded
        .keys()
        .copied()
        .filter(|name| {
            name.split_once('/')
                .is_some_and(|(top, file)| top.ends_with(".dist-info") && file == "RECORD")
        })
        .collect();
    let record = match records.as_slice() {
        [record] => *record,
        [] => return Err(UnpackError::Record { why: "is missing" }),
        _ => {
            return Err(UnpackError::Record {
                why: "is given in more than one .dist-info directory",
            });
        }
    };
    let top = record.split_once('/').map_or("", |(top, _)| top);
    let exempt: Vec<String> = UNRECORDED
        .iter()
        .map(|file| format!("{top}/{file}"))
        .collect();
    unrecorded.retain(|name, _| !exempt.iter().any(|exempt| exempt == name));
    let path = record
        .split('/')
        .fold(staged.directory().join(SITE), |path, part| path.join(part));
    let mut reader = io::BufReader::new(fs::File::open(&path).map_err(UnpackError::Corrupt)?);
    let mut seen: HashSet<String> = HashSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(
                u64::try_from(ROW_BYTES)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_until(b'\n', &mut line)
            .map_err(UnpackError::Corrupt)?;
        if read == 0 {
            break;
        }
        if line.len() > ROW_BYTES {
            return Err(UnpackError::Record {
                why: "holds a row longer than a row may be",
            });
        }
        let row = line.strip_suffix(b"\n").unwrap_or(&line);
        let row = row.strip_suffix(b"\r").unwrap_or(row);
        if row.is_empty() {
            continue;
        }
        let (name, hash, size) = fields(row)?;
        let disagrees = |why| UnpackError::Recorded {
            name: shown(name.as_bytes()),
            why,
        };
        if exempt.iter().any(|exempt| exempt == name) {
            continue;
        }
        let Some((written, sha256)) = unrecorded.remove(name) else {
            return Err(disagrees(if seen.contains(name) {
                "is given twice"
            } else {
                "names no file the wheel installs"
            }));
        };
        seen.insert(name.to_owned());
        let Some(digest) = hash.strip_prefix("sha256=") else {
            return Err(disagrees("gives a hash other than sha256"));
        };
        let digest = URL_SAFE_NO_PAD
            .decode(digest)
            .map_err(|_| disagrees("gives a hash that is not URL-safe base64 without padding"))?;
        if hex(&digest) != sha256 {
            return Err(disagrees("gives a hash other than its bytes'"));
        }
        if !size.bytes().all(|byte| byte.is_ascii_digit())
            || size.parse::<u64>().ok() != Some(written)
        {
            return Err(disagrees("gives a size other than its bytes'"));
        }
    }
    let mut missing: Vec<&str> = unrecorded.into_keys().collect();
    missing.sort_unstable();
    match missing.first() {
        Some(name) => Err(UnpackError::Recorded {
            name: shown(name.as_bytes()),
            why: "is missing",
        }),
        None => Ok(()),
    }
}

/// The path, the hash and the size one row of `RECORD` gives, the path
/// unquoted where the row quotes it: neither a hash nor a size holds a comma,
/// so the path is all before the last two.
fn fields(row: &[u8]) -> Result<(&str, &str, &str), UnpackError> {
    let unread = UnpackError::Record {
        why: "holds a row that is not a path, a hash and a size",
    };
    let row = std::str::from_utf8(row).map_err(|_| UnpackError::Record {
        why: "holds a row that is not text",
    })?;
    let mut fields = row.rsplitn(3, ',');
    let (Some(size), Some(hash), Some(path)) = (fields.next(), fields.next(), fields.next()) else {
        return Err(unread);
    };
    let path = path
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(path);
    Ok((path, hash, size))
}

impl LaunchScript {
    /// The launcher that starts `agent` at `entry_point`, a `module:attribute`,
    /// under an interpreter given `interpreter_args`.
    ///
    /// # Errors
    ///
    /// [`UnpackError::EntryPoint`] where either side of `entry_point` is not
    /// one or more ASCII identifiers joined by single dots, so nothing but a
    /// name a Python string literal holds as it is goes into the file; else
    /// [`UnpackError::Unisolated`] where `interpreter_args` are not exactly
    /// `-I`, as [`LaunchScript::admits`] decides.
    pub fn new(
        agent: &AgentKey,
        interpreter_args: &[Box<str>],
        entry_point: &str,
    ) -> Result<Self, UnpackError> {
        let named = entry_point
            .split_once(':')
            .is_some_and(|(module, attribute)| dotted(module) && dotted(attribute));
        if !named {
            return Err(UnpackError::EntryPoint);
        }
        if !Self::admits(interpreter_args) {
            return Err(UnpackError::Unisolated);
        }
        Ok(Self {
            text: [
                LAUNCHER_HEAD,
                "AGENT = \"",
                agent.as_str(),
                "\"\nENTRY_POINT = \"",
                entry_point,
                "\"\n",
                LAUNCHER_TAIL,
            ]
            .concat(),
        })
    }

    /// Whether `interpreter_args` are the ones a launcher is written for and
    /// started under: exactly the one element `-I`, which starts the
    /// interpreter isolated and leaves the launcher the first thing it runs.
    /// Anything else is refused, since Python reads its own options only up
    /// to the first argument that is not one: a `-I` after `-c`, `-m` or a
    /// file is that program's argument, what follows `-I` runs ahead of the
    /// launcher, and flags run together are not read apart.
    #[must_use]
    pub fn admits(interpreter_args: &[Box<str>]) -> bool {
        matches!(interpreter_args, [only] if &**only == "-I")
    }

    /// What the launcher holds.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    /// The launcher as a version's file list gives it, at [`LAUNCHER`].
    #[must_use]
    pub fn file(&self) -> File {
        File {
            path: LAUNCHER.into(),
            kind: FileKind::File {
                size: u64::try_from(self.text.len()).unwrap_or(u64::MAX),
                sha256: hex(&Sha256::digest(self.text.as_bytes())).into(),
                exec: false,
            },
        }
    }

    /// Writes the launcher at [`LAUNCHER`] under `staging`, where nothing is,
    /// as an ordinary file that is not runnable, and gives it as
    /// [`LaunchScript::file`] does.
    ///
    /// # Errors
    ///
    /// [`UnpackError::Write`] where something is there already, or the file
    /// could not be written.
    pub fn write(&self, staging: &Path) -> Result<File, UnpackError> {
        let path = staging.join(LAUNCHER);
        let written = |source| UnpackError::Write {
            path: path.clone(),
            source,
        };
        let mut out = created(&path, false).map_err(written)?;
        out.write_all(self.text.as_bytes()).map_err(written)?;
        out.sync_all().map_err(written)?;
        Ok(self.file())
    }
}

/// Whether `text` is one or more ASCII Python identifiers joined by single
/// dots.
fn dotted(text: &str) -> bool {
    text.split('.').all(|part| {
        let mut bytes = part.bytes();
        bytes
            .next()
            .is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
            && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    })
}
