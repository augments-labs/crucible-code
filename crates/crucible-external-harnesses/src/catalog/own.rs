//! What a registry or catalog row says about a program the user installed
//! themselves, read into the three things crucible takes from it.
//!
//! Read from a registry or catalog row for a program the user installed;
//! nothing in it is pinned or reviewed. Of a signed catalog entry's binary
//! target, or of a registry binary distribution for this platform, only the
//! name the program is guessed to go by on `PATH` (the file name of `cmd`),
//! its arguments and its environment are kept. Whether the row carries a
//! SHA-256, or an archive this crucible can unpack, does not matter here: such
//! a row is still read, and stays one crucible never installs. An npm or uvx
//! catalog entry starts a pinned interpreter rather than a program of its own,
//! so it gives no guess.
//!
//! A registry entry with no binary for this platform is read from the package
//! it publishes for `npx`, else for `uvx`: the guess is the npm package's last
//! path segment without a trailing `@<version>`, or the Python package without
//! a trailing `==<version>`, and the package's arguments and environment are
//! kept. Any other version specifier, extras, an address or a path gives no
//! guess, nor does a name a package runner goes by ([`RUNNERS`]): the guess is
//! a program crucible may start, and crucible never starts a package runner.
//!
//! The row is read through the catalog's own field reader, so `cmd` is
//! refused unless it is relative and stays inside its directory, and every
//! variable a loader reads to choose what code runs is refused as it is for an
//! entry. `BASH_ENV` and `ENV` are refused as well, in any case, since a shell
//! the program starts reads either to choose a file of code to run first.

use serde_json::{Map, Value};

use super::registry::{RegistryEntry, Runner};
use super::{CatalogError, Entry, Fields, Kind, Variable};

/// The program a row proposes the user may have on `PATH`, and what it is
/// started with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnEntry {
    guess: Box<str>,
    args: Vec<Box<str>>,
    env: Vec<Variable>,
}

impl OwnEntry {
    /// What the signed catalog's `entry` proposes for the platform this build
    /// runs on, or nothing where it is an npm or uvx entry or has no target
    /// here.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] where the target sets `BASH_ENV` or `ENV`.
    pub fn catalog(entry: &Entry) -> Result<Option<Self>, CatalogError> {
        if entry.kind != Kind::Binary {
            return Ok(None);
        }
        let Some(target) = entry.here() else {
            return Ok(None);
        };
        let env = target
            .env
            .iter()
            .map(|variable| (variable.name.as_ref(), variable.value.as_ref()));
        Self::read(&target.cmd, &target.args, env).map(Some)
    }

    /// What the registry's `entry` proposes for `platform`: from its binary
    /// there where it publishes one, else from the package it publishes, or
    /// nothing where it publishes neither or its package's name gives no
    /// guess. A target with no SHA-256, or with an archive this crucible
    /// cannot unpack, is read all the same.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] where its `cmd` is not a relative path that stays in
    /// its directory, or it sets a variable a loader or shell reads to choose
    /// what code runs.
    pub fn registry(entry: &RegistryEntry, platform: &str) -> Result<Option<Self>, CatalogError> {
        if let Some(target) = entry.target(platform) {
            let env = target
                .env
                .iter()
                .map(|(name, value)| (name.as_ref(), value.as_ref()));
            return Self::read(&target.program(), &target.args, env).map(Some);
        }
        let Some(package) = entry.package() else {
            return Ok(None);
        };
        let Some(guess) = guessed(package.runner, &package.package) else {
            return Ok(None);
        };
        let env = package
            .env
            .iter()
            .map(|(name, value)| (name.as_ref(), value.as_ref()));
        Self::read(guess, &package.args, env).map(Some)
    }

    /// The name the program is guessed to go by on `PATH`.
    #[must_use]
    pub fn guess(&self) -> &str {
        &self.guess
    }

    /// The arguments it is started with.
    #[must_use]
    pub fn args(&self) -> &[Box<str>] {
        &self.args
    }

    /// The environment it is started with.
    #[must_use]
    pub fn env(&self) -> &[Variable] {
        &self.env
    }

    /// The row `cmd`, `args` and `env` are, read back through the catalog's
    /// field reader.
    fn read<'a>(
        cmd: &str,
        args: &[Box<str>],
        env: impl Iterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, CatalogError> {
        let mut row = Map::new();
        row.insert("cmd".into(), Value::from(cmd));
        row.insert(
            "args".into(),
            args.iter()
                .map(|arg| Value::from(&**arg))
                .collect::<Value>(),
        );
        row.insert(
            "env".into(),
            Value::Object(
                env.map(|(name, value)| (name.to_owned(), Value::from(value)))
                    .collect(),
            ),
        );
        let row = Value::Object(row);
        let fields = Fields::new(&row, String::new())?;
        let cmd = fields.relative("cmd")?;
        let env = fields.variables("env")?;
        if let Some(variable) = env.iter().find(|variable| shell(&variable.name)) {
            return Err(CatalogError::Loader {
                field: format!("env.{}", variable.name).into(),
                name: variable.name.clone(),
            });
        }
        Ok(Self {
            guess: cmd.rsplit('/').next().unwrap_or(&cmd).into(),
            args: fields.arguments("args")?,
            env,
        })
    }
}

/// The programs a package runner is started as, which no package's name is
/// guessed to be: a guess crucible looks for on `PATH` is a program it may
/// start, and crucible never starts a package runner.
pub(super) const RUNNERS: &[&str] = &[
    "npx", "uvx", "npm", "pnpm", "pnpx", "yarn", "bun", "bunx", "uv", "pipx",
];

/// The name the program `package`, published for `runner`, is guessed to go
/// by on `PATH`: an npm package's last path segment without a trailing
/// `@<version>`, a Python package without a trailing `==<version>`. Any other
/// version specifier, extras, an address, a path, or a name a package runner
/// goes by gives none.
fn guessed(runner: Runner, package: &str) -> Option<&str> {
    let (name, version) = match runner {
        Runner::Npx => {
            let unscoped = match package.strip_prefix('@') {
                Some(scoped) => {
                    let (scope, rest) = scoped.split_once('/')?;
                    named(scope).then_some(rest)?
                }
                None => package,
            };
            match unscoped.split_once('@') {
                Some((name, version)) => (name, Some(version)),
                None => (unscoped, None),
            }
        }
        Runner::Uvx => match package.split_once("==") {
            Some((name, version)) => (name, Some(version)),
            None => (package, None),
        },
    };
    let versioned = version.is_none_or(|version| {
        !version.is_empty()
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '!'))
    });
    let runs = RUNNERS.iter().any(|runner| {
        name.split('.')
            .next()
            .is_some_and(|stem| stem.eq_ignore_ascii_case(runner))
    });
    (versioned && named(name) && !runs).then_some(name)
}

/// Whether `name` is a package's own name: ASCII letters, digits, `.`, `-` and
/// `_`, starting with a letter or digit.
fn named(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Whether `name` is a variable a shell reads to choose a file of code to run
/// first, in any case, as Windows reads variable names.
fn shell(name: &str) -> bool {
    name.eq_ignore_ascii_case("BASH_ENV") || name.eq_ignore_ascii_case("ENV")
}
