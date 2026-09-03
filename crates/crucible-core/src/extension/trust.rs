//! Whether one exact extension may run.
//!
//! Reading a manifest is not deciding about it. A sweep of the extensions
//! directory finds whatever is installed, including whatever was installed by
//! something other than the person now running crucible, and nothing found that
//! way has been agreed to. This is where the agreement is looked up.
//!
//! Two things make one decision, because an answer to only the first is a hole.
//! Whether it may run is a verdict about an extension; which bytes that verdict
//! was reached about is what stops the verdict outliving them. An extension that
//! updates itself, or that somebody else's write turned into a different
//! program, keeps its identifier and loses its digest — so a decision recorded
//! against the digest stops applying at exactly the moment the thing it was
//! about stopped existing.
//!
//! Nothing here reads configuration or a file. What was decided arrives as
//! [`ExtensionDecision`] from whoever owns the document it was written in, and
//! this crate says only what follows from it.

use std::fmt;

use super::ExtensionManifest;

/// What has been decided about one extension.
///
/// Both halves come from the person running crucible. Neither may be answered
/// by a file in a checkout: an extension is somebody else's program, and a
/// repository that could turn one on would be granting authority on behalf of
/// whoever cloned it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionDecision<'a> {
    /// Whether it was said this extension may run.
    pub enabled: bool,
    /// The manifest digest that was said about, where one was recorded.
    pub digest: Option<&'a str>,
}

/// Permission to run one exact extension.
///
/// There is no way to build one but to ask, which is the whole point of the
/// type: a host that takes this rather than a manifest and a flag cannot be
/// handed something nobody agreed to, and cannot be written in a way that
/// forgets to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionTrusted {
    id: Box<str>,
    digest: Box<str>,
}

impl ExtensionTrusted {
    /// Which extension this permits.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The bytes it permits, which are the ones that were decided about.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

/// Why an extension may not run.
///
/// Three answers rather than one, because each asks something different of
/// whoever is told. Nothing was said yet, something was said but it names no
/// manifest, and something was said about a manifest that is not this one are
/// not one refusal seen from three angles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionUntrusted {
    /// Nobody has said this extension may run.
    Undecided,
    /// It may run, but the decision names no manifest.
    ///
    /// A verdict with nothing to hold it to is one that would follow the
    /// identifier onto whatever is installed under it next, which is the thing
    /// the digest exists to prevent. Refused rather than allowed, because an
    /// incomplete record of an agreement is not the agreement.
    Unpinned,
    /// It may run, but what was decided about is not what is installed.
    Changed {
        /// The digest the decision was recorded against.
        decided: Box<str>,
    },
}

impl fmt::Display for ExtensionUntrusted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undecided => f.write_str("nobody has said this extension may run"),
            Self::Unpinned => f.write_str("no digest says which program was agreed to"),
            Self::Changed { decided } => write!(
                f,
                "the manifest has changed since it was agreed to at {decided}",
            ),
        }
    }
}

impl std::error::Error for ExtensionUntrusted {}

impl ExtensionManifest {
    /// Whether this exact manifest may run.
    ///
    /// # Errors
    ///
    /// [`ExtensionUntrusted`] where nothing was decided, where what was decided
    /// names no manifest, or where it names another one. Asked in that order:
    /// an extension nobody turned on is not owed a complaint about its digest,
    /// which would be crucible reporting a discrepancy in an agreement that was
    /// never reached.
    pub fn trusted(
        &self,
        decided: ExtensionDecision<'_>,
    ) -> Result<ExtensionTrusted, ExtensionUntrusted> {
        if !decided.enabled {
            return Err(ExtensionUntrusted::Undecided);
        }
        let Some(digest) = decided.digest else {
            return Err(ExtensionUntrusted::Unpinned);
        };
        if digest != &*self.identity().digest {
            return Err(ExtensionUntrusted::Changed {
                decided: digest.into(),
            });
        }
        Ok(ExtensionTrusted {
            id: self.identity().id.clone(),
            digest: self.identity().digest.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
