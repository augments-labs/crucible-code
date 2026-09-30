#!/usr/bin/env python3
"""Whether the release notes module names anything that reaches outside the binary.

    notes-reach.py FILE
    notes-reach.py --self-test

Reads the Rust source FILE, prints one line for each thing it names that it may
not, and exits 0 when there is none and 1 when there is. Any other exit means
the file was not read, and nothing printed is its answer.

`/release-notes` reads the changelog the binary was built with, and so opens no
socket and reads no file. The module is held to what it takes values from,
written down whole below: a path may start only at one of `ROOTS`, whether it
is written with a leading `::` or without; the only `crate` path is the error
the command returns; `std` gives only `cmp` and `fmt`; the one `include*!` is
the changelog; and the only module it declares is its tests. So `super`, `self`,
`tokio`, `core`, a braced `std` import and any other crate are refused, as is a
`mod` other than its tests in a test build, or a `path` in any attribute, that
would bring in a file this never reads. A macro is refused, since what it
expands to is read by nothing here.

Words inside a string, a character or a comment are no path, and are taken out
first by reading the file as Rust is read: a string runs to its closing quote
whatever it holds, across lines, and a raw string to its closing quote and
hashes; a quote between two apostrophes is a character, and an apostrophe
before a word with none after it is a lifetime; comments nest. A string or a
comment that never closes is refused, since what follows it cannot be read.
`--self-test` exits 0 when every shape this has been shown to miss before is
refused and every allowed one is read clean, and 1 otherwise.
"""

import re
import sys

# The crates and words a path may start at.
ROOTS = {
    "crucible_client_api",
    "crucible_tui",
    "crate",
    "std",
    "u8",
    "u16",
    "u32",
    "u64",
    "usize",
    "char",
    "str",
    "bool",
}

# The whole of what a `crate` or `std` path may be, to its second segment.
PATHS = {"crate::cli::Fatal", "std::cmp", "std::fmt"}

# The one file the module builds in.
INCLUDED = 'include_str!("../../../../CHANGELOG.md")'

# The one module it declares.
MODULES = {"tests"}


class Unreadable(Exception):
    """A literal or a comment that never closes."""


def code(source):
    """`source` with every string, character and comment emptied, lines kept."""
    out = []
    at = 0
    end = len(source)

    def word_before(index):
        return index > 0 and (source[index - 1].isalnum() or source[index - 1] == "_")

    while at < end:
        here = source[at]
        if source.startswith("//", at):
            stop = source.find("\n", at)
            at = end if stop < 0 else stop
            continue
        if source.startswith("/*", at):
            depth, at = 1, at + 2
            while depth:
                if at >= end:
                    raise Unreadable("a comment never closes")
                if source.startswith("/*", at):
                    depth, at = depth + 1, at + 2
                elif source.startswith("*/", at):
                    depth, at = depth - 1, at + 2
                else:
                    out.append("\n" if source[at] == "\n" else "")
                    at += 1
            out.append(" ")
            continue
        raw = re.compile(r"(?:b|c)?r(#*)\"").match(source, at)
        if raw and not word_before(at):
            closing = '"' + raw.group(1)
            stop = source.find(closing, raw.end())
            if stop < 0:
                raise Unreadable("a raw string never closes")
            out.append('""' + "\n" * source.count("\n", at, stop))
            at = stop + len(closing)
            continue
        if here == '"':
            at += 1
            lines = 0
            while True:
                if at >= end:
                    raise Unreadable("a string never closes")
                if source[at] == "\\":
                    # A line break escaped to join two lines is still a line.
                    lines += source[at + 1 : at + 2] == "\n"
                    at += 2
                    continue
                if source[at] == '"':
                    at += 1
                    break
                lines += source[at] == "\n"
                at += 1
            out.append('""' + "\n" * lines)
            continue
        if here == "'":
            literal = re.compile(r"'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|.)|[^\\'\n])'").match(source, at)
            if literal:
                out.append("''")
                at = literal.end()
                continue
        out.append(here)
        at += 1
    return "".join(out)


def reached(source):
    """Each thing `source` names that it may not, in the order found."""
    try:
        text = code(source)
    except Unreadable as refusal:
        return [f"{refusal}; what follows it cannot be read"]

    said = []
    roots = set(re.findall(r"(?<![A-Za-z0-9_:.>])(?:::)?([a-z_][A-Za-z0-9_]*)::", text))
    roots |= set(re.findall(r"\buse\s+(?:::)?([a-z_][A-Za-z0-9_]*)", text))
    roots |= set(re.findall(r"\bextern\s+crate\s+([a-z_][A-Za-z0-9_]*)", text))
    for root in sorted(roots - ROOTS):
        said.append(f"names {root}")
    if re.search(r"\buse\s+(?:::)?\{", text):
        said.append("names a braced import at the root")
    for path in sorted(set(re.findall(r"(?<![A-Za-z0-9_:>])(?:::)?((?:crate|std)::(?:\{|[A-Za-z_]\w*)(?:::[A-Za-z_]\w*)*)", text))):
        kept = path if path.startswith("crate::") else "::".join(path.split("::")[:2])
        if kept not in PATHS:
            said.append(f"names {kept}")
    # The code keeps the source's lines, or nothing below can be matched to them.
    if text.count("\n") != source.count("\n"):
        return ["could not keep the source's lines, so it was not read"]
    if re.search(r"\bmacro_rules\s*!", text):
        said.append("writes a macro, whose expansion this never reads")
    # The one include is judged in the code, where a comment cannot vouch for it.
    written = source.split("\n")
    for number, line in enumerate(text.split("\n"), 1):
        if re.search(r"\binclude\w*\s*!", line):
            own = written[number - 1] if number <= len(written) else ""
            if own.strip() != f"pub(super) const CHANGELOG: &str = {INCLUDED};":
                said.append(f"line {number} builds in something other than the changelog")
    for module in sorted(set(re.findall(r"\bmod\s+(?:r#)?([A-Za-z_]\w*)\s*;", text)) - MODULES):
        said.append(f"declares the module {module}, which this never reads")
    # Its tests are the one module, and only a test build has it.
    if len(re.findall(r"\bmod\s+(?:r#)?tests\s*;", text)) != len(
        re.findall(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*mod\s+tests\s*;", text)
    ):
        said.append("declares its tests outside a test build")
    if re.search(r"#\s*!?\s*\[[^\]]*\bpath\s*=", text):
        said.append("points a module at a file of its own choosing")
    if "crucible_client_api" not in roots:
        said.append("names no crate this check knows it takes, so it measured nothing")
    return said


# A module that names only what is allowed.
BASE = """use crucible_client_api::{Name, Text};
use crucible_tui::Row;

use crate::cli::Fatal;

pub(super) const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

fn order() -> std::cmp::Ordering {
    std::cmp::Ordering::Less
}

#[cfg(test)]
mod tests;
"""

# Lines that must be refused, each after `BASE`: every shape a reader of this
# kind has been shown to miss.
REFUSED = [
    "use tokio::fs as _;",
    "use tokio as _;",
    "use std::{fs as _};",
    "use super::super::login as _;",
    "use self::inner as _;",
    "use crucible_http as _;",
    "use core::ptr as _;",
    "fn d() { let _ = std::fs::read_to_string(\"x\"); }",
    "fn a() { let _ = ::std::fs::read_to_string(\"x\"); }",
    "fn b() { let _ = ::tokio::net::TcpStream::connect(\"x\"); }",
    "use ::std::net::TcpStream;",
    "fn c() { let _ = (\"https://x\", std::fs::read(\"y\")); }",
    "fn e() { let _ = (\"error\", std::fs::read(\"y\")); }",
    "fn f() { let _ = (\"header\", ::tokio::net::TcpStream::connect(\"a:1\")); }",
    "fn g() { let q = '\"'; let _ = std::fs::read(\"x\"); }",
    "fn j() { let _ = [b'\"', 0]; let _ = std::fs::read(\"x\"); }",
    "fn h() { let _ = (r#\"a\"b\"#, std::fs::read(\"y\")); }",
    "fn k() { let _ = \"one\nend\", std::fs::read(\"x\"); }",
    "fn l() { let _ = std::env::var(\"HOME\"); }",
    "fn m() { let _ = std::process::Command::new(\"sh\"); }",
    "include!(\"other.rs\");",
    "const B: &[u8] = include_bytes!(\"secret\");",
    "mod other;",
    "#[path = \"x.rs\"]\nmod tests;",
    "extern crate tokio;",
    "use {std::fs};",
    "fn n() { let _ = \"never closes; }",
    "/* never closes",
    "fn x() { let _ = (\"\\\"\", std::fs::read(\"x\"), \"\\\"\"); }",
    "fn y() { let _ = (r#\"a\"b\"#, std::fs::read(\"y\"), r#\"c\"d\"#); }",
    "fn z() { let _ = (br#\"a\"b\"#, std::fs::read(\"y\"), br#\"c\"d\"#); }",
    "/* a /* \" */ \" */ fn w() { let _ = std::fs::read(\"x\"); } const Q: &str = \"\";",
    "mod r#reach;",
    "#[cfg_attr(all(), path = \"other.rs\")]\nmod tests;",
    "#[cfg_attr(all(), path = \"other.rs\")]\n#[cfg(test)]\nmod tests;",
    "mod tests;",
    "const B: &[u8] = include_bytes!(\"/etc/hostname\"); // include_str!(\"../../../../CHANGELOG.md\")",
    "const A: &str = \"a\\\n    b\\\n    c\";\n/*\npub(super) const CHANGELOG: &str = include_str!(\"../../../../CHANGELOG.md\");\n*/\nconst B: &[u8] = include_bytes!(\"embedded.txt\");",
    "// a\u2028pub(super) const CHANGELOG: &str = include_str!(\"../../../../CHANGELOG.md\");\nconst B: &[u8] = include_bytes!(\"embedded.txt\");",
    "mod r#tests;",
    "#[allow(dead_code)]\nmod tests;",
    "#[cfg(not(test))]\nmod tests;",
    "macro_rules! m { ($n:ident) => { mod $n; }; }\nm!(reach);",
    "macro_rules! m { ($i:ident) => { $i!(\"x\") }; }\nconst B: &[u8] = m!(include_bytes);",
]

# Lines that must be read clean after `BASE`: what the module does write.
ALLOWED = [
    "fn o() { let _ = (\"error\", Name::new(\"x\")); }",
    "fn p() -> char { '\"' }",
    "fn q<'a>(x: &'a str) -> &'a str { x }",
    "fn r() -> &'static str { r#\"a \"quoted\" std::fs\"# }",
    "fn s() -> &'static str { \"two\nlines with tokio::net in them\" }",
    "// std::fs in a comment\nfn t() -> u64 { u64::MAX }",
    "/* nested /* tokio::net */ still a comment */",
    "/* a /* b */ std::fs */",
    "fn u() -> usize { [1, 2].iter().copied().collect::<Vec<usize>>().len() }",
    "fn v() -> Result<(), Fatal> { Ok(()) }",
    "fn w() -> std::fmt::Result { Ok(()) }",
    "const A: &str = \"a\\\n    b\";\nfn t2() -> u8 { 1 }",
]


def self_test():
    wrong = []
    if reached(BASE):
        wrong.append(f"the allowed module was refused: {reached(BASE)}")
    for line in REFUSED:
        if not reached(BASE + line + "\n"):
            wrong.append(f"not refused: {line!r}")
    for line in ALLOWED:
        said = reached(BASE + line + "\n")
        if said:
            wrong.append(f"refused {line!r}: {said}")
    for line in wrong:
        print(line)
    return 1 if wrong else 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) != 1:
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    try:
        with open(argv[0], encoding="utf-8") as file:
            source = file.read()
    except OSError as problem:
        print(f"{argv[0]} could not be read: {problem}", file=sys.stderr)
        return 2
    said = reached(source)
    for line in said:
        print(line)
    return 1 if said else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
