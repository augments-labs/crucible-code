//! Writing one rule into the file a project keeps out of git.
//!
//! The crate below decides what the file may say and what one more rule leaves
//! it looking like. This opens it, and puts the answer back.

use std::fs;
use std::io;
use std::path::Path;

use crucible_config::ConfigError;
use crucible_core::Minted;

/// What can stop an answer of `always` from lasting.
///
/// None of these ends the turn. The call the user allowed still runs and the
/// engine still remembers it for the session; what is lost is the part that
/// outlives the process, and the report says which rule that was so it can be
/// written by hand.
#[derive(Debug, thiserror::Error)]
pub(crate) enum RememberError {
    #[error("{file} could not be written: {source}")]
    Unwritable { file: Box<str>, source: io::Error },

    #[error(transparent)]
    Unusable(#[from] ConfigError),
}

/// Adds `rule` to the `allow` list in `file`.
///
/// Everything already in the file stays where it was, byte for byte. A file
/// that is not there yet becomes one holding the rule and nothing else.
pub(crate) fn allowing(file: &Path, rule: &Minted) -> Result<(), RememberError> {
    // Named the way the user would name it, because that is what a refusal
    // from below tells them to open.
    let named = file.display().to_string();
    let unwritable = |source| RememberError::Unwritable {
        file: named.clone().into(),
        source,
    };

    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        // Nothing there yet, which is what most projects look like. The empty
        // text is what the crate below reads as "write a whole file".
        Err(source) if source.kind() == io::ErrorKind::NotFound => String::new(),
        Err(source) => return Err(unwritable(source)),
    };

    let written = crucible_config::allowing(&text, &named, rule)?;

    put(file, &written).map_err(unwritable)
}

/// Replaces the file, or leaves whatever is there untouched.
fn put(file: &Path, text: &str) -> io::Result<()> {
    let directory = file.parent().unwrap_or_else(|| Path::new(""));

    // Made rather than required: a project nobody has configured has no
    // `.crucible` at all, and this is the first thing to go in it.
    if !directory.as_os_str().is_empty() {
        fs::create_dir_all(directory)?;
    }

    // Written beside the file and renamed over it. A write that stops part-way
    // through leaves half a document, and half a document is a file crucible
    // refuses to start from — so the failure would cost the user their whole
    // configuration rather than one rule. The process id is what keeps two
    // crucibles in one checkout off each other's half-written file.
    let beside = directory.join(format!(".writing.{}", std::process::id()));
    fs::write(&beside, text)?;
    fs::rename(&beside, file)
}

#[cfg(test)]
mod tests;
