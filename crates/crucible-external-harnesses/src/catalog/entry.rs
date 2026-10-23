//! What the catalog says about one reviewed release of an agent, owned and
//! typed.
//!
//! An entry is one version of one agent. Everything here was recorded by the
//! intake and passed by a reviewer, so it is what is offered, shown in a
//! confirmation and checked against once a download arrives; nothing in it is
//! looked up again from the registry.
//!
//! What this reader checks is the shape of each value, once:
//!
//! - every path it gives is relative, with no root, drive, colon, backslash or
//!   NUL and no empty, `.` or `..` component, so it names something under the
//!   directory it is relative to;
//! - a link's target is relative, with no backslash, colon or NUL, and may
//!   climb with `..`; whether it resolves inside the version is left to
//!   whatever creates the link to check;
//! - a version is one directory name, at most [`VERSION_BYTES`];
//! - every address begins `https://`, and whether its host is one the target
//!   lists is left to whoever downloads it to check;
//! - every digest is a SHA-256 in lowercase hexadecimal, and every artifact
//!   lists the files it unpacks to;
//! - no variable it sets is one a loader reads to choose what code runs.
//!
//! Nothing more is checked here. Control characters, names a platform
//! reserves and names that differ only in case are the unpacker's to refuse,
//! since it writes the files, and the sizes are the unpacker's and the store's
//! to reconcile, with checked arithmetic.

use crucible_types::AgentKey;
use serde_json::{Map, Value};

use super::CatalogError;

/// The largest catalog asset a file list, a package tree or a script's result
/// is kept in, in bytes.
pub const ASSET_BYTES: u64 = 4 << 20;

/// The most files an artifact lists in the catalog itself; a longer list is a
/// catalog asset.
pub const LISTED_FILES: usize = 256;

/// The one rule set for unpacking a package tree this reader knows.
pub const UNPACK_RULES: u64 = 1;

/// The longest version, in bytes; a version names one directory in the store.
pub const VERSION_BYTES: usize = 64;

/// One reviewed version of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The crucible name of the agent.
    pub key: AgentKey,
    /// The id the registry lists the agent under, which may change while the
    /// key does not.
    pub registry_id: Box<str>,
    /// The version reviewed.
    pub version: Box<str>,
    /// Which revision of the resolved package tree this is, for an agent
    /// whose tree is resolved again under the same version.
    pub lock_revision: Option<u64>,
    /// How the agent is distributed, which decides how it is unpacked and
    /// started.
    pub kind: Kind,
    /// The licence the agent is published under, as its SPDX expression.
    pub license: Box<str>,
    /// Where the terms the user accepts by installing it are published.
    pub terms: Option<Box<str>>,
    /// What the reviewer wants a user to know before installing it.
    pub caution: Option<Box<str>>,
    /// The review pull request that admitted this version.
    pub review: Box<str>,
    /// What the vendor says it does with what it is sent.
    pub content_use: Option<ContentUse>,
    /// The `agentInfo` the agent answers `initialize` with.
    pub admits: Admits,
    /// The hosts the running agent reaches.
    pub reach: Vec<Box<str>>,
    /// The hosts a sign-in link may point at.
    pub sign_in_hosts: Vec<Box<str>>,
    /// The variable that turns the agent's own updater off.
    pub updater_off: Option<Variable>,
    /// The variable the agent reads its home from, which a profile directory
    /// is applied through.
    pub profile_env: Option<Box<str>>,
    /// Where under the user's home the agent keeps its sign-in.
    pub state_paths: Vec<Box<str>>,
    /// The sign-in methods the agent offered when the intake started it.
    pub auth_methods: Vec<AuthMethod>,
    /// The platforms it is published for.
    pub targets: Vec<Target>,
}

/// How an agent is distributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A program of its own.
    Binary,
    /// A Python package, started by a pinned interpreter.
    Uvx,
    /// An npm package, started by a pinned Node.
    Npm,
}

/// What a vendor says it does with what it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentUse {
    /// The statement, as the user is shown it.
    pub text: Box<str>,
    /// Where the vendor publishes it.
    pub source: Box<str>,
    /// When it was read there.
    pub date: Box<str>,
    /// The origins it covers.
    pub origins: Vec<Box<str>>,
}

/// The `agentInfo` a started agent must answer with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admits {
    /// Its name.
    pub name: Box<str>,
    /// Its version.
    pub version: Box<str>,
}

/// One environment variable and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    /// Its name.
    pub name: Box<str>,
    /// Its value.
    pub value: Box<str>,
}

/// One sign-in method an agent offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    /// The id the agent names it by.
    pub id: Box<str>,
    /// Its name, as the agent gives it.
    pub name: Box<str>,
    /// What the agent says about it.
    pub description: Option<Box<str>>,
    /// How it is carried out.
    pub method: Method,
}

/// How a sign-in method is carried out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// The agent signs in itself, asked through ACP `authenticate`.
    Agent,
    /// A secret the user types is given to the agent in a variable.
    EnvVar {
        /// The variable.
        var: Box<str>,
    },
    /// A command of the agent's own, run with the user at a terminal.
    Terminal {
        /// The program, relative to the version's directory.
        command: Box<str>,
        /// Its arguments.
        args: Vec<Box<str>>,
        /// Its environment.
        env: Vec<Variable>,
        /// Whether it works over pipes or needs a real terminal.
        route: Route,
    },
}

/// How a terminal sign-in reaches the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Over pipes, in the sandbox, drawn by crucible.
    Piped,
    /// On the real terminal, which crucible hands over.
    Terminal,
}

/// The agent as published for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The platform, as `<os>-<arch>`.
    pub platform: Box<str>,
    /// The program started, relative to the version's directory.
    pub cmd: Box<str>,
    /// The agent's own arguments.
    pub args: Vec<Box<str>>,
    /// The agent's environment.
    pub env: Vec<Variable>,
    /// The hosts its downloads may be redirected through.
    pub hosts: Vec<Box<str>>,
    /// The archives it is installed from, each pinned.
    pub artifacts: Vec<Artifact>,
    /// What starting it takes beyond `cmd` and `args`.
    pub launch: Launch,
}

/// What starting an agent takes, by how it is distributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// `cmd` is the agent.
    Binary,
    /// `cmd` is the pinned interpreter, which runs a launcher that calls the
    /// entry point.
    Uvx {
        /// The interpreter's own arguments, ahead of the launcher.
        interpreter_args: Vec<Box<str>>,
        /// The entry point the launcher calls, as `module:attr`.
        entry_point: Box<str>,
    },
    /// `cmd` is the pinned Node, which runs the package's script.
    Npm {
        /// Node's own arguments, ahead of the script.
        node_args: Vec<Box<str>>,
        /// The script, relative to the version's directory.
        bin: Box<str>,
        /// The resolved package tree.
        lock: Lock,
        /// Every install script in the tree, and what the reviewer decided.
        scripts: Vec<Script>,
    },
}

/// A resolved npm package tree, kept as a catalog asset of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// The asset.
    pub asset: AssetRef,
    /// Which rules its packages are unpacked by.
    pub unpack_rules: u64,
}

/// One install script in a package tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The package it belongs to, as its place in the tree.
    pub package: Box<str>,
    /// The npm event it runs on: `preinstall`, `install` or `postinstall`.
    pub event: Box<str>,
    /// What the reviewer decided about it.
    pub disposition: Disposition,
}

/// What a reviewer decided about an install script that is not run as npm
/// would run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// It does nothing the agent needs.
    Inert,
    /// A platform package does its work.
    Replaced,
    /// What it would add is lost, and the agent works without it.
    Lost,
    /// Its result was built at review and is installed as an asset.
    Built,
    /// It is run at install, in the sandbox, and must then match.
    Local {
        /// The host patterns it may reach.
        hosts: Vec<Box<str>>,
        /// The files its package directory must hold afterwards.
        result: AssetRef,
    },
}

/// One pinned download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// Where the vendor publishes it.
    pub url: Box<str>,
    /// A copy of the same bytes the catalog keeps, under the same pin.
    pub mirror: Option<Box<str>>,
    /// Its SHA-256.
    pub sha256: Box<str>,
    /// Its size in bytes.
    pub size: u64,
    /// Its format.
    pub format: Format,
    /// The total size of what is unpacked from it.
    pub unpacked: u64,
    /// The most it may decompress to, the members it discards included.
    pub decompressed: u64,
    /// The files it unpacks to.
    pub files: Files,
    /// The members kept from it, when not every member is: the rest are read
    /// and discarded.
    pub kept: Vec<Box<str>>,
    /// The catalog asset installed in its place, built at review from it;
    /// `format`, `files` and the sizes then describe that asset.
    pub replaced_by: Option<AssetRef>,
}

/// The format of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A zip archive.
    Zip,
    /// A gzip-compressed tar.
    TarGz,
    /// The program itself.
    Bare,
    /// A Python wheel.
    Wheel,
}

/// The files an artifact unpacks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Files {
    /// Listed in the catalog itself.
    Listed(Vec<File>),
    /// Too many to list there, so kept as a catalog asset.
    Asset(AssetRef),
}

/// One file an artifact unpacks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Its path, relative to the version's directory.
    pub path: Box<str>,
    /// What it is.
    pub kind: FileKind,
}

/// What an unpacked file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// An ordinary file.
    File {
        /// Its size in bytes.
        size: u64,
        /// Its SHA-256.
        sha256: Box<str>,
        /// Whether it can be run.
        exec: bool,
    },
    /// A directory.
    Dir,
    /// A symbolic link, to a path relative to the link.
    Link {
        /// Where it points.
        target: Box<str>,
    },
}

/// A catalog release asset, named by its SHA-256 and fetched within its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    /// Its SHA-256.
    pub sha256: Box<str>,
    /// Its size in bytes.
    pub size: u64,
}

/// A release the catalog withdraws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    /// The agent.
    pub key: AgentKey,
    /// The version.
    pub version: Box<str>,
    /// The revision of the version's package tree, when only that one is
    /// withdrawn.
    pub lock_revision: Option<u64>,
    /// Why, as the user is told.
    pub reason: Box<str>,
}

impl Entry {
    /// The entry `fields` holds.
    pub(super) fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        let kind = match &*fields.text("kind")? {
            "binary" => Kind::Binary,
            "uvx" => Kind::Uvx,
            "npm" => Kind::Npm,
            _ => return Err(fields.wrong("kind", "is not binary, uvx or npm")),
        };
        let updater_off = match fields.optional_object("updater_off")? {
            Some(variable) => Some(Variable {
                name: variable.variable_name("name")?,
                value: variable.value("value")?,
            }),
            None => None,
        };
        let profile_env = match fields.optional_text("profile_env")? {
            Some(_) => Some(fields.variable_name("profile_env")?),
            None => None,
        };
        let content_use = match fields.optional_object("content_use")? {
            Some(statement) => Some(ContentUse {
                text: statement.text("text")?,
                source: statement.https("source")?,
                date: statement.text("date")?,
                origins: statement.texts("origins")?,
            }),
            None => None,
        };
        let admits = fields.object("admits")?;
        Ok(Self {
            key: fields.key("key")?,
            registry_id: fields.text("registry_id")?,
            version: fields.version("version")?,
            lock_revision: fields.optional_number("lock_revision")?,
            kind,
            license: fields.text("license")?,
            terms: fields.optional_https("terms")?,
            caution: fields.optional_text("caution")?,
            review: fields.https("review")?,
            content_use,
            admits: Admits {
                name: admits.text("name")?,
                version: admits.text("version")?,
            },
            reach: fields.texts("reach")?,
            sign_in_hosts: fields.texts("sign_in_hosts")?,
            updater_off,
            profile_env,
            state_paths: fields.relatives("state_paths")?,
            auth_methods: fields
                .list("auth_methods")?
                .iter()
                .map(AuthMethod::read)
                .collect::<Result<_, _>>()?,
            targets: fields
                .list("targets")?
                .iter()
                .map(|target| Target::read(target, kind))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl AuthMethod {
    fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        let method = match &*fields.text("kind")? {
            "agent" => Method::Agent,
            "env_var" => Method::EnvVar {
                var: fields.variable_name("var")?,
            },
            "terminal" => Method::Terminal {
                command: fields.relative("command")?,
                args: fields.arguments("args")?,
                env: fields.variables("env")?,
                route: match &*fields.text("route")? {
                    "piped" => Route::Piped,
                    "terminal" => Route::Terminal,
                    _ => return Err(fields.wrong("route", "is not piped or terminal")),
                },
            },
            _ => return Err(fields.wrong("kind", "is not agent, env_var or terminal")),
        };
        Ok(Self {
            id: fields.text("id")?,
            name: fields.text("name")?,
            description: fields.optional_text("description")?,
            method,
        })
    }
}

impl Target {
    fn read(fields: &Fields<'_>, kind: Kind) -> Result<Self, CatalogError> {
        let launch = match kind {
            Kind::Binary => Launch::Binary,
            Kind::Uvx => {
                let entry_point = fields.text("entry_point")?;
                if !entry_point
                    .split_once(':')
                    .is_some_and(|(module, attr)| !module.is_empty() && !attr.is_empty())
                {
                    return Err(fields.wrong("entry_point", "is not module:attr"));
                }
                Launch::Uvx {
                    interpreter_args: fields.arguments("interpreter_args")?,
                    entry_point,
                }
            }
            Kind::Npm => {
                let lock = fields.object("lock")?;
                if lock.number("unpack_rules")? != UNPACK_RULES {
                    return Err(lock.wrong(
                        "unpack_rules",
                        "is a rule set for unpacking that this crucible does not know",
                    ));
                }
                Launch::Npm {
                    node_args: fields.arguments("node_args")?,
                    bin: fields.relative("bin")?,
                    lock: Lock {
                        asset: lock.asset(Some(ASSET_BYTES))?,
                        unpack_rules: UNPACK_RULES,
                    },
                    scripts: fields
                        .list("scripts")?
                        .iter()
                        .map(Script::read)
                        .collect::<Result<_, _>>()?,
                }
            }
        };
        let artifacts: Vec<Artifact> = fields
            .list("artifacts")?
            .iter()
            .map(Artifact::read)
            .collect::<Result<_, _>>()?;
        if artifacts.is_empty() {
            return Err(fields.wrong("artifacts", "is empty, so nothing would be pinned"));
        }
        Ok(Self {
            platform: fields.text("platform")?,
            cmd: fields.relative("cmd")?,
            args: fields.arguments("args")?,
            env: fields.variables("env")?,
            hosts: fields.texts("hosts")?,
            artifacts,
            launch,
        })
    }
}

impl Script {
    fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        let disposition = match &*fields.text("disposition")? {
            "inert" => Disposition::Inert,
            "replaced" => Disposition::Replaced,
            "lost" => Disposition::Lost,
            "built" => Disposition::Built,
            "local" => Disposition::Local {
                hosts: fields.texts("hosts")?,
                result: fields.object("result")?.asset(Some(ASSET_BYTES))?,
            },
            _ => {
                return Err(fields.wrong(
                    "disposition",
                    "is not inert, replaced, lost, built or local",
                ));
            }
        };
        Ok(Self {
            package: fields.relative("package")?,
            event: fields.text("event")?,
            disposition,
        })
    }
}

impl Artifact {
    fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        let format = match &*fields.text("format")? {
            "zip" => Format::Zip,
            "tar.gz" => Format::TarGz,
            "bare" => Format::Bare,
            "wheel" => Format::Wheel,
            _ => return Err(fields.wrong("format", "is not zip, tar.gz, bare or wheel")),
        };
        let files = match fields.field("files") {
            None => return Err(fields.wrong("files", "is missing, so nothing would be pinned")),
            Some(Value::Object(_)) => {
                Files::Asset(fields.object("files")?.asset(Some(ASSET_BYTES))?)
            }
            Some(_) => {
                let listed = fields.list("files")?;
                if listed.len() > LISTED_FILES {
                    return Err(fields.wrong(
                        "files",
                        "lists more than 256 files, and a longer list is a catalog asset",
                    ));
                }
                Files::Listed(listed.iter().map(File::read).collect::<Result<_, _>>()?)
            }
        };
        Ok(Self {
            url: fields.https("url")?,
            mirror: fields.optional_https("mirror")?,
            sha256: fields.sha256("sha256")?,
            size: fields.number("size")?,
            format,
            unpacked: fields.number("unpacked")?,
            decompressed: fields.number("decompressed")?,
            files,
            kept: fields.relatives("kept")?,
            replaced_by: match fields.optional_object("replaced_by")? {
                Some(asset) => Some(asset.asset(None)?),
                None => None,
            },
        })
    }
}

impl File {
    fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        let kind = match fields.optional_text("kind")?.as_deref() {
            None | Some("file") => FileKind::File {
                size: fields.number("size")?,
                sha256: fields.sha256("sha256")?,
                exec: fields.flag("exec")?,
            },
            Some("dir") => FileKind::Dir,
            Some("link") => {
                let target = fields.text("target")?;
                if target.starts_with('/') || target.contains(['\\', ':', '\0']) {
                    return Err(fields.wrong("target", "is not a relative link"));
                }
                FileKind::Link { target }
            }
            Some(_) => return Err(fields.wrong("kind", "is not file, dir or link")),
        };
        Ok(Self {
            path: fields.relative("path")?,
            kind,
        })
    }
}

impl Revoked {
    /// The withdrawn release `fields` holds.
    pub(super) fn read(fields: &Fields<'_>) -> Result<Self, CatalogError> {
        Ok(Self {
            key: fields.key("key")?,
            version: fields.version("version")?,
            lock_revision: fields.optional_number("lock_revision")?,
            reason: fields.text("reason")?,
        })
    }
}

/// Whether `name` is a variable a loader reads to choose what code runs.
/// Windows reads variable names in any case, so the comparison does too.
fn loader(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("LD_")
        || name.starts_with("DYLD_")
        || matches!(
            name.as_str(),
            "NODE_OPTIONS" | "NODE_PATH" | "PYTHONPATH" | "PATH"
        )
}

/// Whether `path` names something inside the directory it is relative to: no
/// root, drive or backslash, and no empty, `.` or `..` component.
fn inside(path: &str) -> bool {
    !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// One JSON object of the catalog, and where in the catalog it is, which every
/// refusal about one of its fields names.
pub(super) struct Fields<'a> {
    object: &'a Map<String, Value>,
    at: String,
}

impl<'a> Fields<'a> {
    /// The object `value` is, found at `at`.
    pub(super) fn new(value: &'a Value, at: String) -> Result<Self, CatalogError> {
        match value {
            Value::Object(object) => Ok(Self { object, at }),
            _ => Err(CatalogError::Field {
                field: if at.is_empty() {
                    "the catalog".into()
                } else {
                    at.into()
                },
                why: "is not an object",
            }),
        }
    }

    /// Where field `name` of this object is.
    fn place(&self, name: &str) -> String {
        if self.at.is_empty() {
            name.to_owned()
        } else {
            format!("{}.{name}", self.at)
        }
    }

    /// The refusal of field `name` for `why`.
    pub(super) fn wrong(&self, name: &str, why: &'static str) -> CatalogError {
        CatalogError::Field {
            field: self.place(name).into(),
            why,
        }
    }

    /// Field `name`, where it is given and not `null`.
    pub(super) fn field(&self, name: &str) -> Option<&'a Value> {
        self.object.get(name).filter(|value| !value.is_null())
    }

    pub(super) fn optional_text(&self, name: &str) -> Result<Option<Box<str>>, CatalogError> {
        match self.field(name) {
            None => Ok(None),
            Some(Value::String(text)) if text.is_empty() => Err(self.wrong(name, "is empty")),
            Some(Value::String(text)) => Ok(Some(text.as_str().into())),
            Some(_) => Err(self.wrong(name, "is not text")),
        }
    }

    pub(super) fn text(&self, name: &str) -> Result<Box<str>, CatalogError> {
        self.optional_text(name)?
            .ok_or_else(|| self.wrong(name, "is missing"))
    }

    /// A variable's value, which may be empty.
    fn value(&self, name: &str) -> Result<Box<str>, CatalogError> {
        match self.field(name) {
            Some(Value::String(text)) => Ok(text.as_str().into()),
            Some(_) => Err(self.wrong(name, "is not text")),
            None => Err(self.wrong(name, "is missing")),
        }
    }

    pub(super) fn optional_number(&self, name: &str) -> Result<Option<u64>, CatalogError> {
        match self.field(name) {
            None => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or_else(|| self.wrong(name, "is not a whole number")),
        }
    }

    pub(super) fn number(&self, name: &str) -> Result<u64, CatalogError> {
        self.optional_number(name)?
            .ok_or_else(|| self.wrong(name, "is missing"))
    }

    /// A flag, false where it is not given.
    fn flag(&self, name: &str) -> Result<bool, CatalogError> {
        match self.field(name) {
            None => Ok(false),
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(_) => Err(self.wrong(name, "is not true or false")),
        }
    }

    /// A version, which names one directory in the store.
    fn version(&self, name: &str) -> Result<Box<str>, CatalogError> {
        let version = self.text(name)?;
        let one = version.len() <= VERSION_BYTES
            && !matches!(&*version, "." | "..")
            && !version
                .chars()
                .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control());
        if one {
            Ok(version)
        } else {
            Err(self.wrong(name, "is not a version that can name one directory"))
        }
    }

    fn key(&self, name: &str) -> Result<AgentKey, CatalogError> {
        AgentKey::new(&self.text(name)?).map_err(|_| self.wrong(name, "is not an agent key"))
    }

    fn optional_https(&self, name: &str) -> Result<Option<Box<str>>, CatalogError> {
        match self.optional_text(name)? {
            Some(url) if url.len() > "https://".len() && url.starts_with("https://") => {
                Ok(Some(url))
            }
            Some(_) => Err(self.wrong(name, "is not an https address")),
            None => Ok(None),
        }
    }

    fn https(&self, name: &str) -> Result<Box<str>, CatalogError> {
        self.optional_https(name)?
            .ok_or_else(|| self.wrong(name, "is missing"))
    }

    fn sha256(&self, name: &str) -> Result<Box<str>, CatalogError> {
        let digest = self.text(name)?;
        if digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(digest)
        } else {
            Err(self.wrong(name, "is not a SHA-256 in lowercase hexadecimal"))
        }
    }

    fn relative(&self, name: &str) -> Result<Box<str>, CatalogError> {
        let path = self.text(name)?;
        if inside(&path) {
            Ok(path)
        } else {
            Err(self.wrong(
                name,
                "is not a relative path that stays inside its directory",
            ))
        }
    }

    /// The asset this object names, at most `cap` bytes where there is one.
    fn asset(&self, cap: Option<u64>) -> Result<AssetRef, CatalogError> {
        let size = self.number("size")?;
        if cap.is_some_and(|cap| size > cap) {
            return Err(self.wrong("size", "is larger than a catalog asset may be"));
        }
        Ok(AssetRef {
            sha256: self.sha256("sha256")?,
            size,
        })
    }

    /// A variable's name, refused where a loader reads it.
    fn variable_name(&self, name: &str) -> Result<Box<str>, CatalogError> {
        let variable = self.text(name)?;
        checked(&variable, || self.place(name))?;
        Ok(variable)
    }

    pub(super) fn optional_object(&self, name: &str) -> Result<Option<Fields<'a>>, CatalogError> {
        self.field(name)
            .map(|value| Fields::new(value, self.place(name)))
            .transpose()
    }

    pub(super) fn object(&self, name: &str) -> Result<Fields<'a>, CatalogError> {
        self.optional_object(name)?
            .ok_or_else(|| self.wrong(name, "is missing"))
    }

    /// The array field `name` is, empty where it is not given.
    fn array(&self, name: &str) -> Result<&'a [Value], CatalogError> {
        match self.field(name) {
            None => Ok(&[]),
            Some(Value::Array(values)) => Ok(values),
            Some(_) => Err(self.wrong(name, "is not a list")),
        }
    }

    /// The objects the array field `name` holds.
    pub(super) fn list(&self, name: &str) -> Result<Vec<Fields<'a>>, CatalogError> {
        self.array(name)?
            .iter()
            .enumerate()
            .map(|(index, value)| Fields::new(value, format!("{}[{index}]", self.place(name))))
            .collect()
    }

    /// The texts the array field `name` holds, each one `kept` by.
    fn each(
        &self,
        name: &str,
        kept: impl Fn(&str) -> bool,
        why: &'static str,
    ) -> Result<Vec<Box<str>>, CatalogError> {
        self.array(name)?
            .iter()
            .enumerate()
            .map(|(index, value)| match value.as_str() {
                Some(text) if kept(text) => Ok(text.into()),
                Some(_) => Err(self.wrong(&format!("{name}[{index}]"), why)),
                None => Err(self.wrong(&format!("{name}[{index}]"), "is not text")),
            })
            .collect()
    }

    fn texts(&self, name: &str) -> Result<Vec<Box<str>>, CatalogError> {
        self.each(name, |text| !text.is_empty(), "is empty")
    }

    /// Arguments, any of which may be empty.
    fn arguments(&self, name: &str) -> Result<Vec<Box<str>>, CatalogError> {
        self.each(name, |_| true, "is not text")
    }

    fn relatives(&self, name: &str) -> Result<Vec<Box<str>>, CatalogError> {
        self.each(
            name,
            inside,
            "is not a relative path that stays inside its directory",
        )
    }

    /// The variables the object field `name` sets, none where it is not given.
    fn variables(&self, name: &str) -> Result<Vec<Variable>, CatalogError> {
        let set = match self.field(name) {
            None => return Ok(Vec::new()),
            Some(Value::Object(set)) => set,
            Some(_) => return Err(self.wrong(name, "is not an object")),
        };
        set.iter()
            .map(|(variable, value)| {
                checked(variable, || self.place(name))?;
                match value {
                    Value::String(value) => Ok(Variable {
                        name: variable.as_str().into(),
                        value: value.as_str().into(),
                    }),
                    _ => Err(self.wrong(&format!("{name}.{variable}"), "is not text")),
                }
            })
            .collect()
    }
}

/// `variable`, refused as a name no environment holds or one a loader reads,
/// with the field it was set in.
fn checked(variable: &str, field: impl Fn() -> String) -> Result<(), CatalogError> {
    if variable.is_empty() || variable.contains(['=', '\0']) {
        return Err(CatalogError::Field {
            field: field().into(),
            why: "names a variable no environment can hold",
        });
    }
    if loader(variable) {
        return Err(CatalogError::Loader {
            field: field().into(),
            name: variable.into(),
        });
    }
    Ok(())
}
