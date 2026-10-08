//! The receipt's format, version 1, and the values it holds once read.
//!
//! The format is line text: a header naming the format version, then one
//! `key=value` line for each key, in this order and each exactly once:
//!
//! ```text
//! crucible-installer-receipt 1
//! manager=crucible-installer
//! installation=<32 lowercase hex digits, fixed for the install's life>
//! target=<linux-x86_64|linux-aarch64|macos-x86_64|macos-aarch64|freebsd-x86_64>
//! layout=versioned
//! prefix=<the absolute, canonical prefix>
//! version=<major>.<minor>.<patch>
//! sha256.crucible=<64 lowercase hex digits>
//! sha256.crucible-sandbox-broker=<64 lowercase hex digits>
//! ```
//!
//! Only the broker line may be left out, when the release carries no broker.
//! Each line ends in a newline. The installer writes it in shell, so the
//! format is one a POSIX shell can read and write without a parser of its own,
//! and the shell reader in `tests/fixtures/installer/receipt.sh` is held to the
//! same answer as this one on every receipt beside it. A unit crucible stages
//! itself gets its receipt from this module, written byte for byte as the
//! installer writes one, so every accepted receipt beside it is written back
//! unchanged.
//!
//! Anything the grammar does not name is refused rather than skipped: a reader
//! that ignored an unknown key would let a later installer's receipt mean less
//! to it than it says. A first line naming a later format version is refused
//! as a receipt from a newer installer, so that this release says why rather
//! than calling it damaged. A prefix is bytes, as a Unix path is, and is the
//! one value that may hold bytes above 127; no value may hold a control
//! character.

use std::ffi::OsStr;
use std::fmt;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

/// The most a receipt may occupy, its last newline included.
pub(crate) const MAX_BYTES: usize = 8192;

/// The header every version 1 receipt opens with, before its format number.
const HEADER: &[u8] = b"crucible-installer-receipt ";

/// What `installation` holds.
const HEX_32: &str = "32 lowercase hex digits";

/// What each `sha256.*` key holds.
const HEX_64: &str = "64 lowercase hex digits";

/// What `target` holds.
const TARGET: &str = "a platform the installer supports";

/// What `prefix` holds.
const CANONICAL: &str = "an absolute, canonical path";

/// What `version` holds.
const RELEASE: &str = "a release number";

// Each key, in the order a receipt holds them.
const MANAGER: &str = "manager";
const INSTALLATION: &str = "installation";
const TARGET_KEY: &str = "target";
const LAYOUT: &str = "layout";
const PREFIX: &str = "prefix";
const VERSION: &str = "version";
const CRUCIBLE: &str = "sha256.crucible";
const BROKER: &str = "sha256.crucible-sandbox-broker";

/// A receipt read and found to follow the format.
///
/// Reading it proves only what it says, not that it is true: whether the
/// layout it sits in agrees with it is [`ReceiptLayout`](super::ReceiptLayout)'s
/// question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Which installation this is, kept across its releases.
    installation: Installation,
    /// The platform the release was built for.
    target: Target,
    /// The directory the release units are kept under.
    prefix: PathBuf,
    /// The release the unit holds.
    version: Version,
    /// The SHA-256 of the unit's `crucible`.
    crucible: Digest,
    /// The SHA-256 of the unit's broker, when the release carries one.
    broker: Option<Digest>,
}

/// Why a receipt was refused.
///
/// No variant carries a value from the receipt: a prefix is a path in
/// somebody's home, and the line and key say where to look.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// More bytes were offered than a receipt may hold.
    #[error("the receipt is larger than {MAX_BYTES} bytes")]
    TooLarge,
    /// A byte below 32 other than a newline, or 127, appears.
    #[error("line {line} of the receipt holds a control character")]
    Control {
        /// The line it is on, counting from 1.
        line: usize,
    },
    /// The last byte is not a newline.
    #[error("the last line of the receipt does not end")]
    Unterminated,
    /// The first line is not a receipt header.
    #[error("this is not an installer receipt")]
    NotAReceipt,
    /// The first line names a format version later than 1.
    #[error("the receipt was written by a newer installer")]
    Newer,
    /// A line names another key than the one its place holds.
    #[error("line {line} of the receipt is not {key}")]
    Expected {
        /// The line, counting from 1.
        line: usize,
        /// The key the line should have named.
        key: &'static str,
    },
    /// The receipt stops before a key every receipt has.
    #[error("the receipt ends before {key}")]
    Ended {
        /// The first key missing.
        key: &'static str,
    },
    /// A line follows the broker's, which is the last a receipt may hold.
    #[error("nothing may follow the broker's line, but line {line} does")]
    Trailing {
        /// The line, counting from 1.
        line: usize,
    },
    /// A value does not follow its key's grammar.
    #[error("{key} on line {line} of the receipt is not {grammar}")]
    Invalid {
        /// The line, counting from 1.
        line: usize,
        /// The key whose value it is.
        key: &'static str,
        /// What the value has to be.
        grammar: &'static str,
    },
}

/// The installation a receipt belongs to: 32 lowercase hex digits the
/// installer picks once and keeps across every release it installs there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation(Box<str>);

/// A platform the installer supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Linux on `x86_64`.
    LinuxX86_64,
    /// Linux on `aarch64`.
    LinuxAarch64,
    /// macOS on `x86_64`.
    MacosX86_64,
    /// macOS on `aarch64`.
    MacosAarch64,
    /// FreeBSD on `x86_64`.
    FreebsdX86_64,
}

/// A release number, `major.minor.patch`, as the receipt and the layout's
/// directory names spell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(Box<str>);

/// A SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest([u8; 32]);

impl Receipt {
    /// Reads a receipt from its bytes.
    ///
    /// # Errors
    ///
    /// [`ReceiptError`] naming the first thing that does not follow the
    /// format.
    pub fn parse(bytes: &[u8]) -> Result<Self, ReceiptError> {
        if bytes.len() > MAX_BYTES {
            return Err(ReceiptError::TooLarge);
        }
        if let Some(at) = bytes.iter().position(|byte| control(*byte)) {
            let before = bytes.get(..at).unwrap_or_default();
            let line = before.split(|byte| *byte == b'\n').count();
            return Err(ReceiptError::Control { line });
        }
        let Some(body) = bytes.strip_suffix(b"\n") else {
            return Err(if bytes.is_empty() {
                ReceiptError::NotAReceipt
            } else {
                ReceiptError::Unterminated
            });
        };
        let mut lines = Lines {
            rest: body.split(|byte| *byte == b'\n').peekable(),
            line: 0,
        };
        header(lines.rest.next().unwrap_or_default())?;
        lines.line = 1;

        if lines.value(MANAGER)? != b"crucible-installer" {
            return Err(lines.invalid(MANAGER, "crucible-installer"));
        }
        let installation = lines.value(INSTALLATION)?;
        let installation = hex(installation, 32)
            .then(|| text(installation))
            .flatten()
            .map(Installation)
            .ok_or_else(|| lines.invalid(INSTALLATION, HEX_32))?;
        let target = lines.value(TARGET_KEY)?;
        let target = Target::ALL
            .into_iter()
            .find(|known| known.as_str().as_bytes() == target)
            .ok_or_else(|| lines.invalid(TARGET_KEY, TARGET))?;
        if lines.value(LAYOUT)? != b"versioned" {
            return Err(lines.invalid(LAYOUT, "versioned"));
        }
        let prefix = lines.value(PREFIX)?;
        if !canonical(prefix) {
            return Err(lines.invalid(PREFIX, CANONICAL));
        }
        let prefix = PathBuf::from(OsStr::from_bytes(prefix));
        let version =
            Version::parse(lines.value(VERSION)?).ok_or_else(|| lines.invalid(VERSION, RELEASE))?;
        let crucible =
            Digest::parse(lines.value(CRUCIBLE)?).ok_or_else(|| lines.invalid(CRUCIBLE, HEX_64))?;
        let broker = if lines.rest.peek().is_some() {
            let broker = Digest::parse(lines.value(BROKER)?);
            Some(broker.ok_or_else(|| lines.invalid(BROKER, HEX_64))?)
        } else {
            None
        };
        if lines.rest.next().is_some() {
            return Err(ReceiptError::Trailing {
                line: lines.line + 1,
            });
        }

        Ok(Self {
            installation,
            target,
            prefix,
            version,
            crucible,
            broker,
        })
    }

    /// Which installation this is.
    pub fn installation(&self) -> &Installation {
        &self.installation
    }

    /// The platform the release was built for.
    pub fn target(&self) -> Target {
        self.target
    }

    /// The directory the release units are kept under.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// The release the unit holds.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The SHA-256 of the unit's `crucible`.
    pub fn crucible(&self) -> &Digest {
        &self.crucible
    }

    /// The SHA-256 of the unit's broker, when the release carries one.
    pub fn broker(&self) -> Option<&Digest> {
        self.broker.as_ref()
    }

    /// The receipt of another release of the same installation, kept under
    /// the same prefix on the same platform, holding `crucible` and, when the
    /// release carries one, `broker`.
    pub(crate) fn for_release(
        &self,
        version: Version,
        crucible: Digest,
        broker: Option<Digest>,
    ) -> Self {
        Self {
            installation: self.installation.clone(),
            target: self.target,
            prefix: self.prefix.clone(),
            version,
            crucible,
            broker,
        }
    }

    /// The receipt in the format, as the installer writes it byte for byte.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(b"1\n");
        let crucible = self.crucible.to_string();
        let broker = self.broker.map(|broker| broker.to_string());
        let lines = [
            (MANAGER, b"crucible-installer".as_slice()),
            (INSTALLATION, self.installation.as_str().as_bytes()),
            (TARGET_KEY, self.target.as_str().as_bytes()),
            (LAYOUT, b"versioned"),
            (PREFIX, self.prefix.as_os_str().as_bytes()),
            (VERSION, self.version.as_str().as_bytes()),
            (CRUCIBLE, crucible.as_bytes()),
        ];
        let broker = broker.as_ref().map(|broker| (BROKER, broker.as_bytes()));
        for (key, value) in lines.into_iter().chain(broker) {
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(b'=');
            bytes.extend_from_slice(value);
            bytes.push(b'\n');
        }
        bytes
    }
}

impl Installation {
    /// The identifier as the receipt spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Installation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Target {
    /// Every target.
    const ALL: [Self; 5] = [
        Self::LinuxX86_64,
        Self::LinuxAarch64,
        Self::MacosX86_64,
        Self::MacosAarch64,
        Self::FreebsdX86_64,
    ];

    /// The target this build runs on, or `None` on a platform the installer
    /// does not support.
    pub fn running() -> Option<Self> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxX86_64)
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Some(Self::LinuxAarch64)
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some(Self::MacosX86_64)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::MacosAarch64)
        } else if cfg!(all(target_os = "freebsd", target_arch = "x86_64")) {
            Some(Self::FreebsdX86_64)
        } else {
            None
        }
    }

    /// The name the receipt and the release archives use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "linux-x86_64",
            Self::LinuxAarch64 => "linux-aarch64",
            Self::MacosX86_64 => "macos-x86_64",
            Self::MacosAarch64 => "macos-aarch64",
            Self::FreebsdX86_64 => "freebsd-x86_64",
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Version {
    /// Reads `major.minor.patch`, each a decimal number without a leading
    /// zero; the receipt and the active-release link are held to this one
    /// grammar.
    pub(crate) fn parse(text: &[u8]) -> Option<Self> {
        let mut parts = text.split(|byte| *byte == b'.');
        let parts = [parts.next(), parts.next(), parts.next(), parts.next()];
        let [Some(major), Some(minor), Some(patch), None] = parts else {
            return None;
        };
        if ![major, minor, patch].into_iter().all(number) {
            return None;
        }
        self::text(text).map(Self)
    }

    /// The number as the receipt spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Digest {
    /// Wraps the 32 bytes a hasher produced.
    pub(crate) fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Reads 64 lowercase hex digits.
    pub(super) fn parse(value: &[u8]) -> Option<Self> {
        if !hex(value, 64) {
            return None;
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(value.chunks_exact(2)) {
            let [high, low] = pair else {
                return None;
            };
            *byte = nibble(*high) << 4 | nibble(*low);
        }
        Some(Self(bytes))
    }
}

/// Lowercase hex, as the receipt spells it.
impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// The receipt's lines after the header, each taken by the key its place
/// holds.
struct Lines<'a, I: Iterator<Item = &'a [u8]>> {
    /// The lines not yet taken.
    rest: std::iter::Peekable<I>,
    /// The line last taken, counting from 1.
    line: usize,
}

impl<'a, I: Iterator<Item = &'a [u8]>> Lines<'a, I> {
    /// The value of the next line, which must name `key`.
    fn value(&mut self, key: &'static str) -> Result<&'a [u8], ReceiptError> {
        let text = self.rest.next().ok_or(ReceiptError::Ended { key })?;
        self.line += 1;
        text.strip_prefix(key.as_bytes())
            .and_then(|rest| rest.strip_prefix(b"="))
            .ok_or(ReceiptError::Expected {
                line: self.line,
                key,
            })
    }

    /// The refusal of the line last taken, whose value is not `grammar`.
    fn invalid(&self, key: &'static str, grammar: &'static str) -> ReceiptError {
        ReceiptError::Invalid {
            line: self.line,
            key,
            grammar,
        }
    }
}

/// Reads the first line, which names the format version.
fn header(line: &[u8]) -> Result<(), ReceiptError> {
    match line.strip_prefix(HEADER) {
        Some(b"1") => Ok(()),
        Some([b'1'..=b'9', rest @ ..]) if rest.iter().all(u8::is_ascii_digit) => {
            Err(ReceiptError::Newer)
        }
        _ => Err(ReceiptError::NotAReceipt),
    }
}

/// A byte below 32 other than a newline, or 127.
fn control(byte: u8) -> bool {
    (byte < 0x20 && byte != b'\n') || byte == 0x7f
}

/// Whether `value` is exactly `digits` lowercase hex digits.
fn hex(value: &[u8], digits: usize) -> bool {
    value.len() == digits
        && value
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The value of one lowercase hex digit.
fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        _ => digit - b'a' + 10,
    }
}

/// Whether `part` is a decimal number written without a leading zero.
fn number(part: &[u8]) -> bool {
    match part {
        b"0" => true,
        [b'1'..=b'9', rest @ ..] => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// Whether `value` is absolute and names no `.`, `..` or empty component.
fn canonical(value: &[u8]) -> bool {
    value.strip_prefix(b"/").is_some_and(|rest| {
        rest.split(|byte| *byte == b'/')
            .all(|part| !matches!(part, b"" | b"." | b".."))
    })
}

/// A value the grammar has already held to ASCII, as text.
fn text(value: &[u8]) -> Option<Box<str>> {
    std::str::from_utf8(value).ok().map(Into::into)
}

#[cfg(test)]
mod tests;
