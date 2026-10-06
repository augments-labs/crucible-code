//! Where a file the answer linked to is.
//!
//! A model writing about the checkout links to `src/main.rs:12`, because that is
//! how a path is written in the repository it is working in. A terminal opens
//! an address it is handed, and an address with no scheme is one no terminal
//! knows how to open: VS Code refuses it outright and JetBrains reads it as a
//! word. What turns the path into somewhere a click can go is the one fact the
//! text does not carry — the directory it is relative to — and the session has
//! that from the moment it opens.
//!
//! The address is a `file://` URI, the one form every terminal that opens links
//! hands to an editor. Where the line goes is the one thing they disagree on.
//! VS Code and its forks look a file up before opening it, so a line written
//! into the path is a file that is not there; they read the line from a
//! fragment, `#12`, as kitty does, and a desktop opener drops the fragment and
//! opens the file. JetBrains keeps the fragment as part of the file name and
//! reads the line from `:12` after the path instead. So the line is spelled the
//! way the terminal in hand reads it, which is [`Line`], and the composition
//! layer, which can see the environment, is what says which.
//!
//! An address that already names a scheme is not a path, and is left exactly as
//! it was written.

use std::path::Path;

/// How the terminal in hand reads the line of a file it is asked to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Line {
    /// `file:///a/b.rs#12`: VS Code and its forks, kitty, and every terminal
    /// that hands the address to a desktop opener, which drops the fragment.
    #[default]
    Fragment,
    /// `file:///a/b.rs:12:5`: JetBrains, which reads a fragment as part of the
    /// file's name.
    Colon,
}

/// The directory a relative path in the answer is read against, and how the
/// terminal reads a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    /// The root as the start of a `file:` URI, escaped, with no trailing
    /// slash: `file:///home/me/repo`, and `file://` for `/` itself.
    root: Box<str>,
    /// How the line is written after the path.
    line: Line,
}

impl Files {
    /// Paths read against `root`, an absolute directory, with the line written
    /// the way `line` says.
    #[must_use]
    pub fn new(root: &Path, line: Line) -> Self {
        let root = root.to_string_lossy();
        let root = root.trim_end_matches(['/', '\\']);
        // `/` itself trims to nothing, and every path joined to it brings the
        // slash back.
        let root = if root.is_empty() {
            "file://".to_owned()
        } else {
            uri(root)
        };
        Self {
            root: root.into_boxed_str(),
            line,
        }
    }

    /// Where `target` points, or `None` where it is not a path: an address
    /// that names its own scheme, an anchor in the page, or nothing at all.
    ///
    /// A backslash is read as a separator, since a model writing about a
    /// Windows checkout writes one, and `./` in front says nothing a joined
    /// path does not.
    #[must_use]
    pub fn address(&self, target: &str) -> Option<String> {
        if names_scheme(target) {
            return None;
        }
        let (path, place) = place(target);
        let path = path.replace('\\', "/");
        let path = path.strip_prefix("./").unwrap_or(&path);
        if path.is_empty() {
            return None;
        }

        let mut address = if absolute(path) {
            uri(path)
        } else {
            format!("{}/{}", self.root, escaped(path))
        };
        if let Some((row, column)) = place {
            match self.line {
                Line::Fragment => {
                    address.push('#');
                    address.push_str(row);
                }
                Line::Colon => {
                    for number in std::iter::once(row).chain(column) {
                        address.push(':');
                        address.push_str(number);
                    }
                }
            }
        }
        Some(address)
    }
}

/// Whether `target` opens with a scheme, `https:` or `mailto:`.
///
/// A scheme is a letter and then letters, digits, `+` and `-`, two characters
/// at least, so a drive letter is not one. RFC 3986 allows a dot as well, but
/// no scheme a terminal opens has one, and `main.rs:12` is a file and a line.
fn names_scheme(target: &str) -> bool {
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && scheme.len() > 1
        && characters.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-')
}

/// `target` apart from the line it names, as a row and maybe a column.
///
/// A line arrives as a compiler writes it, `:12` or `:12:5` after the path,
/// or as a forge's anchor, `#L12` or `#L12-L20`, whose first row is the one
/// opened. Any other anchor is a heading in a rendered page, which an editor
/// has no use for, and goes.
fn place(target: &str) -> (&str, Option<(&str, Option<&str>)>) {
    if let Some((path, anchor)) = target.split_once('#') {
        let row = anchor
            .strip_prefix('L')
            .map(digits)
            .filter(|row| !row.is_empty());
        return (path, row.map(|row| (row, None)));
    }
    match number_after(target) {
        Some((rest, last)) => match number_after(rest) {
            Some((path, row)) => (path, Some((row, Some(last)))),
            None => (rest, Some((last, None))),
        },
        None => (target, None),
    }
}

/// `text` without the `:` and digits it ends with, and those digits.
fn number_after(text: &str) -> Option<(&str, &str)> {
    let (rest, number) = text.rsplit_once(':')?;
    (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .then_some((rest, number))
}

/// The digits `text` starts with.
fn digits(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    text.get(..end).unwrap_or_default()
}

/// Whether `path`, with `/` for every separator, starts at a root: `/`, a
/// drive, or a share.
fn absolute(path: &str) -> bool {
    let mut bytes = path.bytes();
    match (bytes.next(), bytes.next(), bytes.next()) {
        (Some(b'/'), _, _) => true,
        (Some(drive), Some(b':'), Some(b'/')) => drive.is_ascii_alphabetic(),
        _ => false,
    }
}

/// `path`, absolute, as the start of a `file:` URI, without any line.
///
/// A Windows root the system canonicalized arrives as `\\?\C:\repo` or
/// `\\?\UNC\server\share`; the prefix is how the system is told to take the
/// path as written, and is not part of where the file is.
fn uri(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = match path.strip_prefix("//?/") {
        Some(rest) => rest
            .strip_prefix("UNC/")
            .map_or_else(|| rest.to_owned(), |share| format!("//{share}")),
        None => path,
    };
    if path.starts_with("//") {
        // `file://server/share/a`: the server is the URI's host.
        format!("file:{}", escaped(&path))
    } else if path.starts_with('/') {
        format!("file://{}", escaped(&path))
    } else {
        format!("file:///{}", escaped(&path))
    }
}

/// `path` with every byte a URI path may not hold as it is written as a
/// percent escape, a `%` among them: the path is a name, not an address
/// someone escaped already.
fn escaped(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:!$&'()*+,;=@".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            for nibble in [byte >> 4, byte & 0x0f] {
                escaped.extend(
                    char::from_digit(u32::from(nibble), 16).map(|digit| digit.to_ascii_uppercase()),
                );
            }
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(line: Line) -> Files {
        Files::new(Path::new("/home/me/my repo"), line)
    }

    #[test]
    fn a_relative_path_is_read_against_the_root() {
        let files = files(Line::Fragment);

        assert_eq!(
            files.address("src/main.rs").as_deref(),
            Some("file:///home/me/my%20repo/src/main.rs")
        );
        assert_eq!(
            files.address("./src/main.rs").as_deref(),
            Some("file:///home/me/my%20repo/src/main.rs")
        );
    }

    #[test]
    fn a_checkout_at_the_root_of_the_disk_is_one_slash_deep() {
        assert_eq!(
            Files::new(Path::new("/"), Line::Fragment)
                .address("a.rs")
                .as_deref(),
            Some("file:///a.rs")
        );
    }

    #[test]
    fn an_absolute_path_is_its_own_address() {
        assert_eq!(
            files(Line::Fragment).address("/etc/hosts").as_deref(),
            Some("file:///etc/hosts")
        );
    }

    #[test]
    fn a_line_is_written_the_way_the_terminal_reads_one() {
        // The answer writes a line either way: after a colon, as a compiler
        // does, or as a forge's anchor. Each terminal is handed the one form
        // it reads, and a column only where that form has room for one.
        let fragment = files(Line::Fragment);
        let colon = files(Line::Colon);
        let at = "file:///home/me/my%20repo/src/main.rs";

        for (target, by_fragment, by_colon) in [
            ("src/main.rs:12", "#12", ":12"),
            ("src/main.rs:12:5", "#12", ":12:5"),
            ("src/main.rs#L12", "#12", ":12"),
            ("src/main.rs#L12-L20", "#12", ":12"),
        ] {
            assert_eq!(
                fragment.address(target),
                Some(format!("{at}{by_fragment}")),
                "{target}"
            );
            assert_eq!(
                colon.address(target),
                Some(format!("{at}{by_colon}")),
                "{target}"
            );
        }
    }

    #[test]
    fn an_anchor_that_is_not_a_line_is_left_behind() {
        // `#usage` is a heading in a rendered page. In an editor it is
        // nothing, and kept as part of the path it names a file that is not
        // there.
        assert_eq!(
            files(Line::Colon).address("docs/guide.md#usage").as_deref(),
            Some("file:///home/me/my%20repo/docs/guide.md")
        );
    }

    #[test]
    fn an_address_with_a_scheme_is_not_a_path() {
        let files = files(Line::Fragment);

        for target in [
            "https://example.test/a",
            "mailto:me@example.test",
            "file:///etc/hosts",
            "vscode://file/a",
            "#usage",
            "",
        ] {
            assert_eq!(files.address(target), None, "{target}");
        }
    }

    #[test]
    fn a_name_with_a_dot_before_its_colon_is_a_file_and_a_line() {
        // `main.rs:12` starts the way a scheme does, but no scheme anybody
        // opens has a dot in it, and this is how a compiler names a place.
        assert_eq!(
            files(Line::Fragment).address("main.rs:12").as_deref(),
            Some("file:///home/me/my%20repo/main.rs#12")
        );
    }

    #[test]
    fn a_character_a_path_may_hold_and_an_address_may_not_is_escaped() {
        assert_eq!(
            files(Line::Fragment).address("notes/50% é?.md").as_deref(),
            Some("file:///home/me/my%20repo/notes/50%25%20%C3%A9%3F.md")
        );
    }

    #[test]
    fn a_windows_root_and_path_are_written_as_a_uri() {
        // A drive letter is a path, not a scheme, and a canonical Windows
        // root arrives with the verbatim prefix in front of it.
        let files = Files::new(Path::new(r"\\?\C:\work\repo"), Line::Fragment);

        assert_eq!(
            files.address(r"src\main.rs:3").as_deref(),
            Some("file:///C:/work/repo/src/main.rs#3")
        );
        assert_eq!(
            files.address(r"D:\other\a.rs").as_deref(),
            Some("file:///D:/other/a.rs")
        );

        let shared = Files::new(Path::new(r"\\?\UNC\server\share\repo"), Line::Fragment);
        assert_eq!(
            shared.address("a.rs").as_deref(),
            Some("file://server/share/repo/a.rs")
        );
    }
}
