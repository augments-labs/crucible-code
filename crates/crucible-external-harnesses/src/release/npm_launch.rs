//! What crucible writes into a version of a release that Node runs, beside
//! the package tree: the preload that confines the agent to that tree, and a
//! stub for each program the tree's packages declare.
//!
//! [`Preload`] is [`PRELOAD`] at the top of the version, a fixed `CommonJS` file
//! the pinned Node is given with `--require` ahead of the agent's script. It
//! registers synchronous module hooks, which `require` and `import` both go
//! through, and refuses a package name (a specifier that is not relative,
//! absolute, a `file:` or `node:` URL or a built-in module) whose resolution
//! lands outside its own directory once links are followed, so a module
//! planted in a `node_modules` above the store, in `HOME/.node_modules` or in
//! `HOME/.node_libraries` is never loaded. The refusal is the error a missing
//! module raises, `MODULE_NOT_FOUND` for `require` and `ERR_MODULE_NOT_FOUND`
//! for `import`, so an optional dependency taken in a `try` degrades as it
//! would were it absent, and it names the specifier alone, never a path above
//! the tree. A Node without `module.registerHooks` stops at the preload rather
//! than start the agent unconfined. A worker, or a child `node` started by
//! `fork`, takes the parent's `process.execArgv`, which holds the `--require`,
//! so it is confined the same way. The preload is started only with no other
//! Node option, as [`Preload::admits`] decides: a flag the catalog gave could
//! load code from anywhere.
//!
//! [`Stub`] is one program of `.bin`, the directory at [`BIN`] that is put
//! last on the agent's `PATH`, so a dependency's program started by its bare
//! name runs the pinned Node on its real script, under the preload. A program's
//! name comes from a package the tree holds, so it is held to the rules a
//! name the release unpacker writes is, and must be one component. On Unix a
//! stub is an `sh` script with each path single-quoted, a `'` in one written
//! `'\''`. On Windows it is a `.cmd` file with each path double-quoted, in
//! which `cmd` takes `&` and `^` literally, and with `%` written `%%`, since a
//! batch file expands `%` whatever the quoting: that quoting together with the
//! doubled `%` is the escaping an install path needs there. A path holding a
//! `"`, a line end or a NUL is refused, since no quoting holds one; Windows
//! allows none of them in a name.

use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::unpack::{named, shown};
use super::{UnpackError, hex};
use crate::catalog::{File, FileKind};

/// The preload's name, at the top of the version's directory.
pub const PRELOAD: &str = "preload.cjs";

/// The directory of stubs, under the version's directory.
pub const BIN: &str = ".bin";

// The preload's text. The Python gate reads it from between the two marker
// lines and runs it on a real Node, so keep it one raw string between them.
// node-preload: begin
const PRELOAD_TEXT: &str = r"// Written by crucible: Node is given this file with --require before the
// agent's script, and passes it on to every worker and forked child through
// process.execArgv. A package name is resolved as Node resolves it, and one
// that lands outside this file's own directory is refused as missing, so
// nothing above the installed tree is ever loaded.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const url = require('node:url');
const Module = require('node:module');

if (typeof Module.registerHooks !== 'function') {
  throw new Error('this Node has no module.registerHooks, so the agent is not started');
}

const tree = fs.realpathSync(__dirname);
const builtins = new Set(Module.builtinModules);

function bare(specifier) {
  return !(
    /^\.\.?([\\/]|$)/.test(specifier) ||
    path.isAbsolute(specifier) ||
    specifier.startsWith('file:') ||
    specifier.startsWith('node:') ||
    builtins.has(specifier)
  );
}

function inside(resolved) {
  if (resolved.startsWith('node:')) {
    return true;
  }
  if (!resolved.startsWith('file:')) {
    return false;
  }
  let real;
  try {
    real = fs.realpathSync(url.fileURLToPath(resolved));
  } catch {
    return false;
  }
  const within = path.relative(tree, real);
  return (
    within !== '' &&
    within !== '..' &&
    !within.startsWith('..' + path.sep) &&
    !path.isAbsolute(within)
  );
}

Module.registerHooks({
  resolve(specifier, context, nextResolve) {
    const resolved = nextResolve(specifier, context);
    if (!bare(specifier) || inside(resolved.url)) {
      return resolved;
    }
    const required = (context.conditions || []).includes('require');
    const refusal = new Error(
      (required ? 'Cannot find module ' : 'Cannot find package ') + JSON.stringify(specifier),
    );
    refusal.code = required ? 'MODULE_NOT_FOUND' : 'ERR_MODULE_NOT_FOUND';
    throw refusal;
  },
});
";
// node-preload: end

/// The preload of a version of a release that Node runs.
#[derive(Debug, Clone, Copy)]
pub struct Preload;

/// One program of a version's `.bin`, written for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stub {
    /// Its path, relative to the version's directory.
    path: String,
    /// What it holds.
    text: String,
    /// Whether it is made runnable.
    exec: bool,
}

/// Why a stub was not written.
#[derive(Debug, thiserror::Error)]
pub enum StubError {
    /// The program's name is not one file name a stub may be written under.
    #[error("a package names a program that is not one file name a stub may be written under")]
    Name(#[source] UnpackError),
    /// A path the stub would hold is not text.
    #[error("the path of the {0} a stub starts is not text")]
    Untext(&'static str),
    /// A path the stub would hold has a character no quoting holds.
    #[error("the path of the {0} a stub starts holds a double quote, a line end or a NUL")]
    Unquotable(&'static str),
}

impl Preload {
    /// What the preload holds.
    #[must_use]
    pub fn bytes() -> &'static [u8] {
        PRELOAD_TEXT.as_bytes()
    }

    /// The preload as a version's file list gives it, at [`PRELOAD`].
    #[must_use]
    pub fn file() -> File {
        recorded(PRELOAD, PRELOAD_TEXT.as_bytes(), false)
    }

    /// Whether `node_args` are the ones the preload is started under: none.
    /// A Node option the catalog gave could load code from anywhere,
    /// `--require`, `--import`, `--inspect` and `--experimental-loader` among
    /// them, so anything at all is refused.
    #[must_use]
    pub fn admits(node_args: &[Box<str>]) -> bool {
        node_args.is_empty()
    }
}

impl Stub {
    /// The `sh` stub at `.bin/<name>` that runs `node` on `script` under
    /// `preload`, each an absolute path.
    ///
    /// # Errors
    ///
    /// [`StubError::Name`] where `name` is not one file name the release
    /// unpacker would write, else [`StubError::Untext`] or
    /// [`StubError::Unquotable`] for the first path that is not text or holds
    /// a NUL.
    pub fn unix(name: &str, node: &Path, preload: &Path, script: &Path) -> Result<Self, StubError> {
        let name = program(name)?;
        let [node, preload, script] = paths([node, preload, script], &['\0'])?
            .map(|path| format!("'{}'", path.replace('\'', "'\\''")));
        Ok(Self {
            path: format!("{BIN}/{name}"),
            text: format!("#!/bin/sh\nexec {node} --require {preload} {script} \"$@\"\n"),
            exec: true,
        })
    }

    /// The `.cmd` stub at `.bin/<name>.cmd` that runs `node` on `script` under
    /// `preload`, each an absolute path.
    ///
    /// # Errors
    ///
    /// [`StubError::Name`] where `name` is not one file name the release
    /// unpacker would write, else [`StubError::Untext`] or
    /// [`StubError::Unquotable`] for the first path that is not text or holds
    /// a `"`, a CR, an LF or a NUL.
    pub fn windows(
        name: &str,
        node: &Path,
        preload: &Path,
        script: &Path,
    ) -> Result<Self, StubError> {
        let name = program(name)?;
        let [node, preload, script] = paths([node, preload, script], &['"', '\r', '\n', '\0'])?
            .map(|path| format!("\"{}\"", path.replace('%', "%%")));
        Ok(Self {
            path: format!("{BIN}/{name}.cmd"),
            text: format!("@{node} --require {preload} {script} %*\r\n"),
            exec: false,
        })
    }

    /// Its path, relative to the version's directory.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What it holds.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }

    /// The stub as a version's file list gives it.
    #[must_use]
    pub fn file(&self) -> File {
        recorded(&self.path, self.text.as_bytes(), self.exec)
    }
}

/// The file list's record of `bytes` at `path`.
fn recorded(path: &str, bytes: &[u8], exec: bool) -> File {
    File {
        path: path.into(),
        kind: FileKind::File {
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            sha256: hex(&Sha256::digest(bytes)).into(),
            exec,
        },
    }
}

/// `name` held to the rules a name the release unpacker writes is, and to
/// one component.
fn program(name: &str) -> Result<String, StubError> {
    let name = named(name.as_bytes(), false).map_err(StubError::Name)?;
    if name.contains('/') {
        return Err(StubError::Name(UnpackError::Name {
            name: shown(name.as_bytes()),
            why: "is more than one component",
        }));
    }
    Ok(name)
}

/// The text of the paths of the Node, the preload and the script, in that
/// order, each refused where it is not text or holds one of `unquotable`.
fn paths<'a>(given: [&'a Path; 3], unquotable: &[char]) -> Result<[&'a str; 3], StubError> {
    let mut texts = [""; 3];
    for ((text, path), what) in texts.iter_mut().zip(given).zip(["Node", "preload", "script"]) {
        *text = path.to_str().ok_or(StubError::Untext(what))?;
        if text.contains(unquotable) {
            return Err(StubError::Unquotable(what));
        }
    }
    Ok(texts)
}
