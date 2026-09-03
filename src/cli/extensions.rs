//! What `--extensions` prints.
//!
//! A listing of what is installed and what could not be read, written from a
//! sweep that has already happened. Nothing here reads a file or starts
//! anything: the flag exists so that somebody can see what crucible found
//! *before* deciding whether any of it should ever run, and a listing that had
//! to run an extension to describe it would be the opposite of that.
//!
//! The whole answer is built as one string and written once, because it goes to
//! standard output for a person or a pipe rather than to the renderer — by the
//! time this runs there is no session, no screen and nothing to protect.

use std::fmt::Write as _;
use std::path::Path;

use crucible_config::{Extensions, Installed, Settings};

/// The listing, as one block of text ending in a newline.
///
/// Empty of extensions is a sentence rather than an empty answer: somebody who
/// just installed one and sees nothing needs to be told which directory was
/// looked in, because the usual mistake is having put it somewhere else.
pub(crate) fn listing(found: &Extensions, settings: &Settings) -> String {
    let mut said = String::new();
    let at = shown(found.at());

    match found.found().len() {
        0 => {
            let _ = writeln!(said, "no extensions in {at}");
        }
        1 => {
            let _ = writeln!(said, "1 extension in {at}");
        }
        many => {
            let _ = writeln!(said, "{many} extensions in {at}");
        }
    }

    // Said on the first line rather than at the end, where a long listing would
    // scroll it away: an incomplete answer that reads like a complete one is
    // how an extension ends up installed, missing from the list, and
    // impossible to explain.
    if found.stopped() {
        let _ = writeln!(
            said,
            "more are installed than crucible looks at, so this list is short",
        );
    }

    let mut any_off = false;
    for one in found.found() {
        let enabled = settings.extension_enabled(&one.manifest().identity().id);
        any_off |= !enabled;

        said.push('\n');
        describe(&mut said, one, enabled);
    }

    // Once, at the end, rather than beside every entry that says no. An
    // installed extension is off until somebody turns it on, so on most
    // machines every entry says no and the remedy repeated seven times is
    // noise; said once it is the next thing to do.
    if any_off {
        let _ = writeln!(
            said,
            "\nnothing runs until its enabled key is true in your home configuration file",
        );
    }

    let refused = found.refused();
    if !refused.is_empty() {
        let _ = writeln!(
            said,
            "\n{} could not be read:",
            match refused.len() {
                1 => String::from("1 directory"),
                many => format!("{many} directories"),
            }
        );
        for problem in refused {
            let _ = writeln!(said, "  {problem}");
        }
    }

    said
}

/// One extension, in the words its own manifest used.
///
/// `enabled` is the only line here that the manifest does not get a say in, and
/// it is written in the same column as the rest because it is the same kind of
/// fact: what somebody would need to know before deciding this one is the
/// suspect.
fn describe(said: &mut String, one: &Installed, enabled: bool) {
    let manifest = one.manifest();
    let identity = manifest.identity();
    let requests = manifest.requests();

    let _ = writeln!(said, "{} {}", identity.id, identity.version);
    let _ = writeln!(said, "  from      {}", one.file());
    let _ = writeln!(
        said,
        "  protocol  {}.{}, needs crucible {}",
        requests.protocol.major(),
        requests.protocol.minor(),
        requests.minimum,
    );
    // "nothing" rather than an empty line, both times. An extension that asks
    // for no capability is a real and unremarkable thing, and a blank space
    // beside the label reads like the listing failed to work it out.
    let _ = writeln!(
        said,
        "  asks for  {}",
        joined(
            requests
                .capabilities
                .iter()
                .map(|capability| capability.as_str())
        )
    );
    let _ = writeln!(
        said,
        "  gives     {}",
        joined(
            requests
                .contributions
                .iter()
                .map(|contribution| contribution.as_str())
        )
    );
    // The manifest asks; this is the answer. Spelled as the key's own word so
    // that the listing and the file a reader goes on to open say one thing.
    let _ = writeln!(said, "  enabled   {}", if enabled { "yes" } else { "no" });
    // Taken over the manifest's own bytes by the parser, never read out of it:
    // a file stating its own digest would be a file asserting it had not
    // changed since somebody trusted it.
    let _ = writeln!(said, "  digest    {}", identity.digest);
}

/// A list of spellings, or the word for an empty one.
fn joined<'a>(spellings: impl Iterator<Item = &'a str>) -> String {
    let listed: Vec<&str> = spellings.collect();
    if listed.is_empty() {
        return String::from("nothing");
    }

    listed.join(", ")
}

/// A path, as the user would name it.
fn shown(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests;
