#!/usr/bin/env python3
"""Whether the ledger of bridges and the code that crosses them agree.

    bridge-ledger.py LEDGER MANIFEST...
    bridge-ledger.py --self-test

LEDGER is the Rust file that declares `pub enum Bridge`, one variant for each
synchronous caller that crosses into an asynchronous contract, and the MANIFEST
arguments are the manifests of the workspace's packages. Exits 0 when the two
agree, 1 when they do not, with each disagreement said on stderr, and 2 when
the ledger or a manifest could not be read, or when what was handed over is not
a ledger and the packages holding it, so that a ledger nobody could parse never
passes as one nothing disagrees with. `--self-test` exits 0 when every case
gives the answer written beside it, and 1 otherwise.

They agree when every variant's documentation says whether it polls once or
waits, in a `- Crossing:` item reading `polls once` or `waits`, says what bounds
it, and, where it waits, says what bounds its wait in a `- Wait bounded by:`
item, names as its owner one package of the workspace and says in words what
retires it; when no source outside the ledger's package crosses a variant by
the other kind than the one its entry says, which is seen only where the
variant's path is followed, blanks and line breaks aside, by `.wait` or by
`.cross`; when a variant is named, outside the package that declares it, only
in its owner;
when every variant is named in its owner's shipped source, because a bridge
nothing crosses is deleted rather than kept; when no shipped source but the
ledger builds, in a spelling this check knows, the waker or the context that a
future polled by hand needs, which is one way a crossing nobody wrote down can
look; and when no source writes a borrow of a future, in a spelling this check
knows, as a crossing's argument, because a crossing drops what it is handed,
and a dropped borrow leaves the future to be crossed again until it answers,
which is a wait written as a loop.

A variant is found by its path, `Bridge::Name`, which is how every crossing
spells it, so a spelling this check knows that would hide one from that search
— an alias of the enum, by `use` or by `type`, a glob or braced import of its
variants, or a bridge taken off a refusal and crossed again — is a disagreement
in itself. `Bridge as` is refused wherever it is written, a qualified trait
path such as `<Bridge as fmt::Debug>::fmt` included, although that aliases
nothing. `Self::` inside an impl for `Bridge` is not seen, and neither is
`<Bridge>::Name`. In the ledger's own package no `Bridge::Name` counts as a
crossing, and the ledger's own file is not searched, so a variant whose owner
is that package is always reported crossed nowhere: the check fails on it,
loudly, rather than passing it. What retires a bridge counts as said in words
when nothing in it is shaped like a plan's identifier, an approximation whose
edges are given beside `IDENTIFIER`.

A hand poll is seen only where it spells `Waker::noop`, `noop_waker`,
`Waker::from`, `RawWaker` or `Context::from_waker`, which build the waker and
the context a poll needs; a waker made another way, by `.into()` or by
`noop_waker_ref()`, is seen only through `Context::from_waker` building a
context on it. `Context` reached through an alias, a context handed in by an
enclosing `poll`, and a library call that polls or waits for its caller, such
as tokio's `block_on`, build nothing this check can see, and pass.

A lent future and a bridge crossed again are found by how they are written,
which is not every way to write them. A borrow is seen only in a crossing
written as a method call, `.cross(`, whose argument begins with `&mut`,
`Pin::new(&mut …)` or `Box::pin(&mut …)`, or ends with an `.as_mut()` after an
argument that holds no `;`, `{` or `}` and parentheses at most two deep.
`Pin::as_mut(&mut …)`, a path-qualified `std::pin::Pin::new(…)` and a crossing
written as a path call, `Bridge::cross(Bridge::Name, &mut …)`, pass, and so does
any borrow handed to a wait, `.wait(`. What comes
before that `.as_mut()` is read as written, string and character literals
included: a `;`, `{` or `}` anywhere in it ends the match, so a lend whose
argument holds a format string such as `format!("{name}")`, or a struct
literal, is missed; and a literal holding an unpaired parenthesis can make the
check report a crossing that lends nothing, or miss one that lends. A bridge
crossed again is seen only where `.bridge()` comes directly before `.cross` or
`.wait`.
Either one bound to a name first is outside what the check can see, and so is
a `Bridge` passed across crates as a value, as a parameter or a field.

Only a line whose first characters, after blanks, are `//` is left out, doc
comments among them, so that a sentence about a bridge there is not a crossing
of it. A trailing `//` comment, the lines of a block comment, and string and
character literals are read as code by every pattern, which fails both ways: a
waker spelling, a lend or a hiding spelling written in one is reported as
though it were code, and `Bridge::Name` written in one in its owner's shipped
source counts as a crossing, so the check that a bridge is crossed somewhere
can pass for a bridge nothing crosses. The package a file belongs to is the one
whose directory is the deepest to contain it. What ships is judged by path
alone: what is under that package's `src/`, outside any directory named `tests`
and in no file named `tests.rs` or ending in `_tests.rs`, so an inline
`#[cfg(test)]` module in a shipped file ships with it.
"""

import os
import re
import sys
import tomllib

AGREE, DISAGREE, UNREAD = 0, 1, 2

# How each line `say` writes begins, so a reader of the gate can tell whose
# sentence it is: every disagreement, every refusal to read, and every
# self-test case that gave another answer. The usage text, printed when fewer
# than a ledger and one manifest are given, and a traceback if the check
# itself crashes, do not begin with it.
REASON = "bridge-ledger: "

ENUM = re.compile(r"^pub enum Bridge \{$", re.MULTILINE)
CLOSE = re.compile(r"^\}$", re.MULTILINE)
VARIANT = re.compile(r"^\s*([A-Z][A-Za-z0-9]*),$")
DOC = re.compile(r"^\s*/// ?(.*)$")
OWNER = re.compile(r"`([a-z0-9][a-z0-9-]*)`")
NAMED = re.compile(r"\bBridge::([A-Z][A-Za-z0-9]*)")
HIDDEN = re.compile(r"\bBridge\s+as\b|\bBridge::\s*[{*]|\btype\s+\w+\s*=\s*(?:[\w:]*::)?Bridge\s*;")
# A future polled by hand, seen by what the poll builds. On stable Rust a poll
# made outside an async context needs a `Context`, and `Context::from_waker` is
# the only stable way to build one, so that spelling is seen, and so are the
# waker constructions `Waker::noop`, `noop_waker` as a whole word,
# `Waker::from` and `RawWaker`. A waker made another way, by `.into()` or by
# `noop_waker_ref()`, is seen only through the context built on it. What
# passes: `Context` reached through an alias; a context handed in by an
# enclosing `poll`, which the poll uses rather than builds; and a library call
# that polls or waits for its caller, such as tokio's `block_on`, which builds
# nothing in the source.
BY_HAND = re.compile(r"\bWaker::noop\b|\bnoop_waker\b|\bWaker::from\b|\bRawWaker\b|\bContext::from_waker\b")
# A crossing handed a borrow of a future rather than the future. The crossing
# drops what it is handed, so a refusal then drops only the borrow, and the
# future the caller kept can be crossed again until it answers. Seen only in a
# crossing written as a method call, `.cross(`, whose own argument spells the
# borrow: it begins with `&mut`, `Pin::new(&mut` or `Box::pin(&mut`, or it ends
# with an `.as_mut()` nothing is called on after, following an argument that
# holds no `;`, `{` or `}` and parentheses at most two deep. A borrow bound to
# a name first is not seen, and neither is one spelled another way:
# `Pin::as_mut(&mut …)`, a path-qualified `std::pin::Pin::new(…)`, and any
# lend in a crossing written as a path call,
# `Bridge::cross(Bridge::Name, &mut …)`, pass.
#
# What an argument holds before that `.as_mut()`: no `;`, `{` or `}` anywhere,
# and parentheses only in pairs, two deep at most. All of them are read as
# written, those inside a string or character literal among them. A `;`, `{` or
# `}` ends the match wherever it is, so a lend whose argument holds a format
# string such as `format!("{name}")`, or a struct literal, is missed. A literal
# holding an unpaired parenthesis can carry a match past the parenthesis that
# closes the crossing, or end it before the `.as_mut()` that ends the argument.
# The check can then report a crossing that lends nothing, or miss one that
# lends.
ARGUMENT = r"(?:[^;{}()]|\((?:[^;{}()]|\([^;{}()]*\))*\))*"
LENT = re.compile(
    rf"\.cross\(\s*(?:&mut\b|Pin::new\(\s*&mut\b|Box::pin\(\s*&mut\b)"
    rf"|\.cross\({ARGUMENT}\.as_mut\(\)\s*,?\s*\)"
)
# A bridge taken off a refusal and crossed again: the crossing no longer spells
# which variant it is, so neither who owns it nor that it retries can be seen.
# Seen only where `.bridge()` comes directly before `.cross` or `.wait`; a
# bridge bound to a name first and crossed by it is not.
RECROSSED = re.compile(r"\.bridge\(\)\s*\.(?:cross|wait)\b")
# How an entry says it crosses: once, by one poll, or by waiting. Read from its
# `- Crossing:` item, a full stop after it allowed.
ONCE, WAITS = "polls once", "waits"
# A variant crossed by the method that says how, with only blanks, line breaks
# included, between the path and the method. A variant bound to a name first,
# or reached through a crossing written as a path call, `Bridge::wait(…)`, is
# not seen by either.
CROSSED_ONCE = re.compile(r"\bBridge::([A-Z][A-Za-z0-9]*)\s*\.cross\b")
WAITED = re.compile(r"\bBridge::([A-Z][A-Za-z0-9]*)\s*\.wait\b")
# Roughly what a plan's identifier looks like: one to three capitals, perhaps a
# hyphen, then digits, as a whole word. What retires a bridge is said in words
# a reader of the code can check, never by a name only a plan resolves. This is
# an approximation, wrong at both edges. It matches words that are no plan's
# identifier, such as `UTF-16` or `MD5`, so a retirement that names one fails
# the check loudly. And it misses some that are: a lowercase one such as `zq7`,
# one with a letter after its number such as `Q7b`, and one with more than
# three letters before its number such as `WXYZ-7`.
IDENTIFIER = re.compile(r"\b[A-Z]{1,3}-?[0-9]+\b")


class Unread(Exception):
    """Nothing was compared: the ledger or a manifest could not be read, or
    what was handed over is not a ledger and the packages holding it.
    """


def say(reason):
    print(f"{REASON}{reason}", file=sys.stderr)


def entries(ledger):
    """Each variant the ledger declares, with its documentation's lines."""
    opened = ENUM.search(ledger)
    if opened is None:
        raise Unread("the ledger declares no `pub enum Bridge`")
    body = ledger[opened.end() :]
    closed = CLOSE.search(body)
    if closed is None:
        raise Unread("the ledger's `enum Bridge` never closes")
    found, docs = [], []
    for line in body[: closed.start()].splitlines():
        doc, variant = DOC.match(line), VARIANT.match(line)
        if doc:
            docs.append(doc.group(1))
        elif variant:
            found.append((variant.group(1), docs))
            docs = []
        elif line.strip():
            raise Unread(f"the ledger's `enum Bridge` holds a line this check cannot place: {line.strip()!r}")
    if not found:
        raise Unread("the ledger's `enum Bridge` has no variants")
    return found


def item(docs, name):
    """The text of a variant's `- Name:` item, its continuation lines joined."""
    for index, line in enumerate(docs):
        if line.startswith(f"- {name}:"):
            text = [line.removeprefix(f"- {name}:").strip()]
            for more in docs[index + 1 :]:
                if not more.startswith("  ") or not more.strip():
                    break
                text.append(more.strip())
            return " ".join(text).strip()
    return ""


def package_of(path, packages):
    """The package whose directory is the deepest to contain `path`."""
    best, depth = None, -1
    for name, directory in packages.items():
        inside = directory == "." or path == directory or path.startswith(directory + os.sep)
        here = 0 if directory == "." else directory.count(os.sep) + 1
        if inside and here > depth:
            best, depth = name, here
    return best


def shipped(path, package, packages):
    """Whether `path` is source its package ships rather than a test of it."""
    directory = packages[package]
    inner = path if directory == "." else path.removeprefix(directory + os.sep)
    parts = inner.split(os.sep)
    stem = parts[-1].removesuffix(".rs")
    return parts[0] == "src" and "tests" not in parts[:-1] and stem != "tests" and not stem.endswith("_tests")


def uncommented(text):
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("//"))


def disagreements(ledger_path, ledger, packages, sources):
    """Every way the ledger and the sources disagree, one sentence each.

    `packages` maps each package's name to its directory and `sources` each
    Rust file's path to its text, all relative to the same root.
    """
    wrong, owners, kinds = [], {}, {}
    declared = entries(ledger)
    for name, docs in declared:
        kind = item(docs, "Crossing").removesuffix(".")
        if kind not in (ONCE, WAITS):
            wrong.append(f"`Bridge::{name}` does not say whether it polls once or waits")
        else:
            kinds[name] = kind
        if kind == WAITS and not item(docs, "Wait bounded by"):
            wrong.append(f"`Bridge::{name}` waits and does not say what bounds its wait")
        if not item(docs, "Bound"):
            wrong.append(f"`Bridge::{name}` does not say what bounds it")
        retired = item(docs, "Retired")
        if not retired:
            wrong.append(f"`Bridge::{name}` does not say what retires it")
        elif IDENTIFIER.search(retired):
            wrong.append(f"`Bridge::{name}` says what retires it by an identifier, not in words: {retired!r}")
        owner = OWNER.fullmatch(item(docs, "Owner"))
        if owner is None:
            wrong.append(f"`Bridge::{name}` does not name one crate, in backquotes, as its owner")
        elif owner.group(1) not in packages:
            wrong.append(f"`Bridge::{name}` is owned by `{owner.group(1)}`, which is no package of this workspace")
        else:
            owners[name] = owner.group(1)

    home = package_of(ledger_path, packages)
    crossed = set()
    for path, text in sorted(sources.items()):
        if path == ledger_path:
            continue
        package = package_of(path, packages)
        ships = package is not None and shipped(path, package, packages)
        code = uncommented(text)
        if HIDDEN.search(code):
            wrong.append(f"{path} spells `Bridge` in a way that hides which of its variants it names")
        if LENT.search(code):
            wrong.append(f"{path} lends a crossing its future, which then drops only the borrow; hand it the future")
        if RECROSSED.search(code):
            wrong.append(f"{path} crosses a bridge taken off a refusal, which hides which of its variants it crosses")
        if ships and BY_HAND.search(code):
            wrong.append(f"{path} polls a future by hand; a synchronous caller crosses through a `Bridge` instead")
        if package == home:
            continue
        for found in WAITED.finditer(code):
            if kinds.get(found.group(1)) == ONCE:
                wrong.append(f"{path} waits on `Bridge::{found.group(1)}`, which the ledger says polls once")
        for found in CROSSED_ONCE.finditer(code):
            if kinds.get(found.group(1)) == WAITS:
                wrong.append(f"{path} polls `Bridge::{found.group(1)}` once, which the ledger says waits")
        for found in NAMED.finditer(code):
            name = found.group(1)
            if name not in dict(declared):
                wrong.append(f"{path} names `Bridge::{name}`, which the ledger does not hold")
            elif name in owners and owners[name] != package:
                wrong.append(f"{path} is in `{package}` and crosses `Bridge::{name}`, which `{owners[name]}` owns")
            elif ships:
                crossed.add(name)
    for name, _ in declared:
        if name in owners and name not in crossed:
            wrong.append(
                f"`Bridge::{name}` is crossed nowhere in what `{owners[name]}` ships; "
                "a bridge nothing crosses is deleted rather than kept"
            )
    return wrong


def packages_of(manifests):
    packages = {}
    for manifest in manifests:
        try:
            with open(manifest, "rb") as handle:
                name = tomllib.load(handle)["package"]["name"]
        except (OSError, tomllib.TOMLDecodeError, KeyError, TypeError) as error:
            raise Unread(f"{manifest} does not name its package: {error}") from error
        if not isinstance(name, str):
            raise Unread(f"{manifest} does not name its package with a string")
        packages[name] = os.path.normpath(os.path.dirname(manifest))
    return packages


def gathered(packages):
    """Every Rust file under each package's source, test and bench roots."""
    sources = {}
    for directory in packages.values():
        for top in ("src", "tests", "benches", "examples"):
            for root, _, files in os.walk(os.path.join(directory, top)):
                for file in files:
                    if file.endswith(".rs"):
                        path = os.path.normpath(os.path.join(root, file))
                        with open(path, "rb") as handle:
                            sources[path] = handle.read().decode("utf-8", "replace")
        build = os.path.normpath(os.path.join(directory, "build.rs"))
        if os.path.isfile(build):
            with open(build, "rb") as handle:
                sources[build] = handle.read().decode("utf-8", "replace")
    return sources


LEDGER = """\
pub enum Bridge {
    /// Asking the model.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each delta
    ///   read.
    /// - Owner: `runner`
    /// - Retired: when the turn loop is asynchronous.
    Turn,
    /// Starting a server.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each start.
    /// - Owner: `code`
    /// - Retired: when the application runs on one runtime.
    Hosting,
}
"""

# A ledger that also holds an entry that waits, and what bounds its wait.
WAITING_ENTRY = """\
    /// Taking a login step.
    ///
    /// - Crossing: waits.
    /// - Bound: one wait for each step.
    /// - Wait bounded by: the turn's cancel, and the step's own
    ///   deadline.
    /// - Owner: `code`
    /// - Retired: when a login is asynchronous.
    Login,
"""
WAITING = LEDGER.removesuffix("}\n") + WAITING_ENTRY + "}\n"
UNBOUNDED = WAITING.replace("    /// - Wait bounded by: the turn's cancel, and the step's own\n    ///   deadline.\n", "")

PACKAGES = {"code": ".", "runner": os.path.join("crates", "runner"), "runtime": os.path.join("crates", "runtime")}
AT = os.path.join("crates", "runtime", "src", "bridge.rs")
# What the ledger's own file does besides declaring it: the one hand-made poll.
CROSSING = "fn cross() { Waker::noop(); Self::Turn; }\n"
RUNNER = os.path.join("crates", "runner", "src", "turn.rs")
RUNNER_MODULE_TEST = os.path.join("crates", "runner", "src", "turn", "tests.rs")
RUNNER_TEST = os.path.join("crates", "runner", "tests", "turn.rs")
CODE = os.path.join("src", "serve.rs")
LOGIN = os.path.join("src", "login.rs")
LOGGING_IN = "fn login() { Bridge::Login.wait(Some(&handle), &cancel, step()); }\n"
AGREEING = {
    AT: LEDGER + CROSSING,
    RUNNER: "fn turn() { Bridge::Turn.cross(ask()); }\n",
    CODE: "fn serve() { Bridge::Hosting.cross(start()); }\n",
    os.path.join("src", "notes.rs"): "// Bridge::Turn is the runner's, and this is a sentence about it.\n",
    RUNNER_TEST: "fn fake() { Waker::noop(); }\n",
}


def changed(ledger=LEDGER, files=None):
    """The agreeing sources, with the ledger swapped and some files replaced.

    A file replaced with None is removed.
    """
    sources = dict(AGREEING)
    sources[AT] = ledger + CROSSING
    for path, text in (files or {}).items():
        if text is None:
            sources.pop(path, None)
        else:
            sources[path] = text
    return sources


def without(line):
    return LEDGER.replace(line, "")


# Each case: a name, the ledger, the sources, and either the fragments of the
# disagreements it must report, one per disagreement in order, or the fragment
# of the refusal it must give instead.
CASES = [
    ("the ledger and the code agree", LEDGER, AGREEING, []),
    (
        "a variant with no bound",
        without("    /// - Bound: one poll for each start.\n"),
        changed(without("    /// - Bound: one poll for each start.\n")),
        ["`Bridge::Hosting` does not say what bounds it"],
    ),
    (
        "a variant with no retirement",
        without("    /// - Retired: when the application runs on one runtime.\n"),
        changed(without("    /// - Retired: when the application runs on one runtime.\n")),
        ["`Bridge::Hosting` does not say what retires it"],
    ),
    (
        "a variant that does not say how it crosses",
        LEDGER.replace("    /// - Crossing: polls once.\n", "", 1),
        changed(LEDGER.replace("    /// - Crossing: polls once.\n", "", 1)),
        ["`Bridge::Turn` does not say whether it polls once or waits"],
    ),
    (
        "a variant that crosses some third way",
        LEDGER.replace("polls once.", "streams.", 1),
        changed(LEDGER.replace("polls once.", "streams.", 1)),
        ["`Bridge::Turn` does not say whether it polls once or waits"],
    ),
    (
        "a waiting variant that says what bounds its wait, waited on by its owner",
        WAITING,
        changed(WAITING, {LOGIN: LOGGING_IN}),
        [],
    ),
    (
        "a waiting variant that does not say what bounds its wait",
        UNBOUNDED,
        changed(UNBOUNDED, {LOGIN: LOGGING_IN}),
        ["`Bridge::Login` waits and does not say what bounds its wait"],
    ),
    (
        "a variant that polls once, waited on",
        LEDGER,
        changed(files={RUNNER: "fn turn() {\n    Bridge::Turn\n        .wait(None, &cancel, ask());\n}\n"}),
        [f"{RUNNER} waits on `Bridge::Turn`, which the ledger says polls once"],
    ),
    (
        "a waiting variant, polled once",
        WAITING,
        changed(WAITING, {LOGIN: "fn login() { Bridge::Login.cross(step()); }\n"}),
        [f"{LOGIN} polls `Bridge::Login` once, which the ledger says waits"],
    ),
    (
        "a bridge taken off a refusal and waited on again",
        WAITING,
        changed(
            WAITING,
            {
                LOGIN: "fn login() { if let Err(no) = Bridge::Login.wait(None, &cancel, step()) "
                "{ no.bridge().wait(None, &cancel, step()); } }\n"
            },
        ),
        [f"{LOGIN} crosses a bridge taken off a refusal"],
    ),
    (
        "a variant retired by an identifier",
        LEDGER.replace("when the turn loop is asynchronous.", "by T12."),
        changed(LEDGER.replace("when the turn loop is asynchronous.", "by T12.")),
        ["`Bridge::Turn` says what retires it by an identifier"],
    ),
    (
        "a variant owned by no package",
        LEDGER.replace("`code`", "`gone`"),
        changed(LEDGER.replace("`code`", "`gone`")),
        ["owned by `gone`, which is no package"],
    ),
    (
        "a variant whose owner is not one crate",
        LEDGER.replace("`runner`", "`runner` and `code`"),
        changed(LEDGER.replace("`runner`", "`runner` and `code`")),
        ["`Bridge::Turn` does not name one crate"],
    ),
    (
        "a variant crossed by a package that does not own it",
        LEDGER,
        changed(files={CODE: "fn serve() { Bridge::Hosting.cross(start()); Bridge::Turn.cross(ask()); }\n"}),
        [f"{CODE} is in `code` and crosses `Bridge::Turn`, which `runner` owns"],
    ),
    (
        "a variant the ledger does not hold",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(ask()); Bridge::Gone.cross(ask()); }\n"}),
        ["names `Bridge::Gone`, which the ledger does not hold"],
    ),
    (
        "a variant crossed only in its owner's tests",
        LEDGER,
        changed(
            files={
                RUNNER: None,
                RUNNER_MODULE_TEST: "fn turn() { Bridge::Turn.cross(ask()); }\n",
                RUNNER_TEST: "fn turn() { Bridge::Turn.cross(ask()); }\n",
            }
        ),
        ["`Bridge::Turn` is crossed nowhere in what `runner` ships"],
    ),
    (
        "a variant named only in a comment",
        LEDGER,
        changed(files={RUNNER: "// Bridge::Turn.cross(ask()) was here.\n"}),
        ["`Bridge::Turn` is crossed nowhere in what `runner` ships"],
    ),
    (
        "a future polled by hand in shipped source",
        LEDGER,
        changed(files={CODE: "fn serve() { Bridge::Hosting.cross(start()); Waker::noop(); }\n"}),
        [f"{CODE} polls a future by hand"],
    ),
    (
        "a waker built from a value in shipped source",
        LEDGER,
        changed(files={CODE: "fn serve() { Bridge::Hosting.cross(start()); Waker::from(Arc::new(Noop)); }\n"}),
        [f"{CODE} polls a future by hand"],
    ),
    (
        "a waker built from its parts in shipped source",
        LEDGER,
        changed(files={CODE: "fn serve() { Bridge::Hosting.cross(start()); RawWaker::new(ptr::null(), &TABLE); }\n"}),
        [f"{CODE} polls a future by hand"],
    ),
    (
        "a context built on a waker made by into() in shipped source",
        LEDGER,
        changed(
            files={
                CODE: "fn serve() { Bridge::Hosting.cross(start()); "
                "let waker: Waker = Arc::new(Noop).into(); Context::from_waker(&waker); }\n"
            }
        ),
        [f"{CODE} polls a future by hand"],
    ),
    (
        "a context built on a waker made by into() in a test file",
        LEDGER,
        changed(
            files={
                os.path.join("tests", "serve.rs"): "fn serve() { Bridge::Hosting.cross(start()); "
                "let waker: Waker = Arc::new(Noop).into(); Context::from_waker(&waker); }\n"
            }
        ),
        [],
    ),
    (
        "a future polled by hand in a test module",
        LEDGER,
        changed(files={RUNNER_MODULE_TEST: "fn poll() { Waker::noop(); }\n"}),
        [],
    ),
    (
        "an alias hiding the variants",
        LEDGER,
        changed(files={os.path.join("src", "alias.rs"): "use crucible_runtime::Bridge as Crossing;\n"}),
        ["spells `Bridge` in a way that hides"],
    ),
    (
        "a type alias hiding the variants",
        LEDGER,
        changed(files={os.path.join("src", "alias.rs"): "type Crossing = crucible_runtime::Bridge;\n"}),
        ["spells `Bridge` in a way that hides"],
    ),
    (
        "a glob import hiding the variants",
        LEDGER,
        changed(files={os.path.join("src", "glob.rs"): "use crucible_runtime::Bridge::*;\n"}),
        ["spells `Bridge` in a way that hides"],
    ),
    (
        "a braced import hiding the variants",
        LEDGER,
        changed(files={os.path.join("src", "braced.rs"): "use crucible_runtime::Bridge::{Hosting};\n"}),
        ["spells `Bridge` in a way that hides"],
    ),
    (
        "a future lent to a crossing by a mutable borrow",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(&mut step); }\n"}),
        [f"{RUNNER} lends a crossing its future"],
    ),
    (
        "a future lent to a crossing pinned where it stands",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(Pin::new(&mut step)); }\n"}),
        [f"{RUNNER} lends a crossing its future"],
    ),
    (
        "a future owned in a box and pinned, crossed as its own",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(Pin::new(Box::new(step))); }\n"}),
        [],
    ),
    (
        "a future lent to a crossing in a box of its borrow",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(Box::pin(&mut step)); }\n"}),
        [f"{RUNNER} lends a crossing its future"],
    ),
    (
        "a pinned future lent to a crossing",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(step.as_mut()); }\n"}),
        [f"{RUNNER} lends a crossing its future"],
    ),
    (
        "a pinned future lent to a crossing laid out over lines",
        LEDGER,
        changed(
            files={RUNNER: "fn turn() {\n    Bridge::Turn.cross(\n        pinned(&mut step).as_mut(),\n    );\n}\n"}
        ),
        [f"{RUNNER} lends a crossing its future"],
    ),
    (
        "a future a pinned borrow makes, crossed as its own",
        LEDGER,
        changed(files={RUNNER: "fn turn() { Bridge::Turn.cross(left.process.as_mut().stop()); }\n"}),
        [],
    ),
    (
        "a pinned borrow in a match on what a crossing answered",
        LEDGER,
        changed(
            files={
                RUNNER: "fn turn() {\n    match Bridge::Turn.cross(ask()) {\n        Ok(answer) => keep(answer),\n"
                "        Err(_) => retry(step.as_mut()),\n    }\n}\n"
            }
        ),
        [],
    ),
    (
        "a pinned borrow later in the chain a crossing starts",
        LEDGER,
        changed(
            files={
                RUNNER: "fn turn() {\n    Bridge::Turn\n        .cross(ask())\n"
                "        .map(|answer| keep(answer.as_mut()))\n}\n"
            }
        ),
        [],
    ),
    (
        "a bridge taken off a refusal and crossed again",
        LEDGER,
        changed(
            files={
                RUNNER: "fn turn() { if let Err(no) = Bridge::Turn.cross(ask()) { no.bridge().cross(ask()); } }\n"
            }
        ),
        [f"{RUNNER} crosses a bridge taken off a refusal"],
    ),
    ("a file with no ledger", "pub struct Bridge;\n", changed("pub struct Bridge;\n"), "declares no `pub enum Bridge`"),
    ("a ledger that never closes", LEDGER.removesuffix("}\n"), changed(LEDGER.removesuffix("}\n")), "never closes"),
    ("a ledger with no variants", "pub enum Bridge {\n}\n", changed("pub enum Bridge {\n}\n"), "has no variants"),
    (
        "a ledger with a line this check cannot place",
        LEDGER.replace("    Hosting,", "    Hosting = 3,"),
        changed(LEDGER.replace("    Hosting,", "    Hosting = 3,")),
        "cannot place",
    ),
]


def self_test():
    wrong = 0
    for name, ledger, sources, expected in CASES:
        try:
            got, refusal = disagreements(AT, ledger, PACKAGES, sources), None
        except Unread as error:
            got, refusal = None, str(error)
        if isinstance(expected, str):
            if refusal is None or expected not in refusal:
                say(f"{name}: gave {got}, not a refusal saying {expected!r}")
                wrong = 1
            continue
        if refusal is not None:
            say(f"{name}: refused ({refusal}) rather than comparing")
            wrong = 1
            continue
        if len(got) != len(expected) or not all(
            fragment in line for fragment, line in zip(expected, got, strict=True)
        ):
            say(f"{name}: gave {got}, not one disagreement for each of {expected}")
            wrong = 1
    return wrong


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return UNREAD
    ledger_path = os.path.normpath(argv[1])
    try:
        with open(ledger_path, encoding="utf-8") as handle:
            ledger = handle.read()
        packages = packages_of(argv[2:])
        if package_of(ledger_path, packages) is None:
            raise Unread(f"{ledger_path} is in none of the packages handed over")
        wrong = disagreements(ledger_path, ledger, packages, gathered(packages))
    except (Unread, OSError, UnicodeDecodeError) as refusal:
        say(str(refusal))
        return UNREAD
    for line in wrong:
        say(line)
    return DISAGREE if wrong else AGREE


if __name__ == "__main__":
    sys.exit(main(sys.argv))
