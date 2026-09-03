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
use crucible_core::{ExtensionManifest, ExtensionProtocol, ExtensionUnhosted};

/// The listing, as one block of text ending in a newline.
///
/// Empty of extensions is a sentence rather than an empty answer: somebody who
/// just installed one and sees nothing needs to be told which directory was
/// looked in, because the usual mistake is having put it somewhere else.
///
/// `running` is the crucible this is, which decides whether an extension
/// naming an older one could be hosted at all.
pub(crate) fn listing(found: &Extensions, settings: &Settings, running: &str) -> String {
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
        let id = &one.manifest().identity().id;
        let decided = Decided {
            enabled: settings.extension_enabled(id),
            written: settings.extension_settings(id),
        };
        any_off |= !decided.enabled;

        said.push('\n');
        describe(&mut said, one, decided, running);
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

/// What the person running crucible said about one extension.
///
/// The two travel together because they come from the same block of the same
/// file and are printed one after the other. Held as one thing rather than as
/// two more parameters, which is also what keeps [`describe`] inside the
/// argument count this workspace allows.
struct Decided<'a> {
    /// Whether they said it may run.
    enabled: bool,
    /// The names they wrote under its `config`, never what they set them to.
    written: Vec<&'a str>,
}

/// One extension, in the words its own manifest used.
///
/// Two of these lines the manifest gets no say in, and they are written in the
/// same column as the rest because they are the same kind of fact: what
/// somebody would need to know before deciding this one is the suspect. They
/// come after what was claimed and in the order they are decided — whether
/// this build could run it at all, and then whether anybody said it may.
fn describe(said: &mut String, one: &Installed, decided: Decided<'_>, running: &str) {
    let Decided { enabled, written } = decided;
    let manifest = one.manifest();
    let identity = manifest.identity();
    let requests = manifest.requests();

    let _ = writeln!(said, "{} {}", identity.id, identity.version);
    let _ = writeln!(said, "  from      {}", one.file());
    let _ = writeln!(
        said,
        "  protocol  {}, needs crucible {}",
        requests.protocol, requests.minimum,
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
    // The two lines above are what it asked for; this is whether this build
    // could answer. Said even for an extension nobody has turned on, because
    // the two are different refusals and turning it on would not cure this one.
    let _ = writeln!(said, "  hosted    {}", hosting(manifest, running));
    // The manifest asks; this is the answer. Spelled as the key's own word so
    // that the listing and the file a reader goes on to open say one thing.
    let _ = writeln!(said, "  enabled   {}", if enabled { "yes" } else { "no" });
    // The names only. Crucible has never read this extension's documentation,
    // so it cannot tell which of these holds a key somebody pasted — and a
    // listing is a thing people paste into an issue. Named at all because
    // whoever wrote them needs to see the block was read as the one they meant.
    let _ = writeln!(said, "  config    {}", joined(written.iter().copied()));
    // Taken over the manifest's own bytes by the parser, never read out of it:
    // a file stating its own digest would be a file asserting it had not
    // changed since somebody trusted it.
    let _ = writeln!(said, "  digest    {}", identity.digest);
}

/// Whether this build could run it, and what stands in the way where it could not.
///
/// The reason names the fact the manifest's own line above does not carry: what
/// this crucible speaks, or which crucible this is. Neither is in the manifest,
/// so neither can be read off the two lines already printed.
fn hosting(manifest: &ExtensionManifest, running: &str) -> String {
    let asked = manifest.requests().protocol;

    match manifest.hosted(running) {
        Ok(agreed) if agreed == asked => String::from("yes"),
        // Not a refusal: the pair settle on the smaller vocabulary they both
        // know. It is said anyway, because the extension was written against
        // reach it is not going to get and its author is the one who can tell
        // whether that matters.
        Ok(agreed) => format!("yes, speaking {agreed}"),
        Err(ExtensionUnhosted::Protocol) => format!(
            "no; this crucible speaks protocol {}",
            ExtensionProtocol::HOST
        ),
        Err(ExtensionUnhosted::Newer) => format!("no; this crucible is {running}"),
    }
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
