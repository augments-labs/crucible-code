//! Which repository the answer's bare numbers are counted against.
//!
//! A model writing about work in progress writes `#487`, because that is what
//! everybody working on the repository calls it. Turning those four characters
//! into somewhere a reader can go needs one fact the text does not carry: the
//! repository. It is written down in `.git/config`, under the remote the
//! checkout was cloned from.
//!
//! Read out of the file directly rather than by running `git remote get-url`,
//! for the reason [`super::branching`] reads `HEAD` itself: this is on the
//! startup path, a child process there is an exec nobody asked for, and a
//! checkout without git installed still has the file.
//!
//! Everything short of a repository is `None` — no remote, a remote spelled a
//! way this does not read, a directory that is not a checkout. A number with
//! no repository behind it is drawn as the four characters it has always been,
//! which is the answer this is allowed to be wrong towards.

use std::fs;
use std::path::{Path, PathBuf};

use crucible_tui::Forge;

/// The section a cloned checkout's own remote is written under.
const ORIGIN: &str = "[remote \"origin\"]";

/// What a forge puts between the repository and the number.
///
/// One spelling for the forge that files issues and pull requests in one
/// series, and one for the forge that does not. Both are the issue page: a
/// number in prose is an issue or a pull request and no amount of reading the
/// text will say which, but a forge counting both in one series answers the
/// issue address for either.
const FILED: &str = "/issues/";

/// GitLab's, which keeps its own pages under a prefix.
const NESTED: &str = "/-/issues/";

/// The name a host has to carry to be read as the one that nests.
///
/// A guess, and the only one here. Self-hosted forges are named whatever their
/// owner named them, so a GitLab at `git.example.com` is a GitLab this cannot
/// see — its numbers point at the right repository through a path it does not
/// use, and a reader who follows one arrives at a page that says so. The
/// alternative is asking the network at startup, which is not a trade this
/// makes for a decoration on a line of prose.
const NESTS: &str = "gitlab";

/// How a remote is spelled when it is not a URL.
///
/// `git@host:owner/repo.git`, which is scp syntax rather than a URL and is what
/// a clone over ssh writes by default.
const AT: char = '@';

/// The repository checked out at `root`, or `None` where there is none to name.
pub(crate) fn forge(root: &Path) -> Option<Forge> {
    let config = fs::read_to_string(config(root)?).ok()?;
    read(&remote(&config)?)
}

/// Where the checkout at `root` keeps its configuration.
///
/// In a linked worktree `.git` is a file naming that worktree's own directory,
/// and the configuration is not in there: it belongs to the repository, which
/// `commondir` names from the worktree's side.
fn config(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if !git.is_file() {
        return Some(git.join("config"));
    }

    let pointed = fs::read_to_string(&git).ok()?;
    let worktree = Path::new(pointed.strip_prefix("gitdir:")?.trim());
    let common = fs::read_to_string(worktree.join("commondir")).ok()?;
    Some(worktree.join(common.trim()).join("config"))
}

/// The address `origin` was cloned from, as `config` has it written.
///
/// Enough of the format to find one value under one section, which is what is
/// wanted here. Git's configuration has more in it than this reads — included
/// files, values continued over a line break, a comment after a value — and
/// every one of them ends in the same place a spelling this does not know
/// ends: no forge, and a number drawn as itself.
fn remote(config: &str) -> Option<String> {
    let mut inside = false;

    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == ORIGIN;
            continue;
        }

        if !inside {
            continue;
        }

        let (name, url) = line.split_once('=')?;
        if name.trim() == "url" {
            return Some(url.trim().to_owned());
        }
    }

    None
}

/// The forge `url` names, or `None` where it names something this does not read.
fn read(url: &str) -> Option<Forge> {
    // The scp spelling has no scheme and a colon where a URL has a slash. Told
    // apart by the colon coming before any slash, so `ssh://git@host/o/r` is
    // read as the URL it is rather than as a host called `ssh`.
    let scp = url
        .split_once(':')
        .filter(|(before, after)| !before.contains('/') && !after.starts_with("//"));

    let (host, path) = if let Some((before, after)) = scp {
        (before, after)
    } else {
        let (_, rest) = url.split_once("://")?;
        rest.split_once('/')?
    };

    // Whoever the clone authenticated as is not part of where the page is.
    let host = host.rsplit_once(AT).map_or(host, |(_, host)| host);
    let slug = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or_else(|| path.trim_matches('/'))
        .trim_end_matches('/');

    if host.is_empty() || !slug.contains('/') {
        return None;
    }

    let path = if host.contains(NESTS) { NESTED } else { FILED };
    Some(Forge::new(format!("https://{host}"), slug, path))
}

#[cfg(test)]
mod tests;
