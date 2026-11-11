#!/usr/bin/env python3
"""The named obligations that must keep running, whatever the tests are called.

`cargo test` reports a total. A total cannot tell a reader that the one case
proving an approval binds a single call still exists, still runs, and still
asserts what it did — a deleted case, an `#[ignore]`, a hollowed body and a
narrowed invocation all leave a green total behind. This gate names those
obligations in `scripts/required-cases.json` and checks four separate things
about each:

    discovered   the case is in the list the shared test selection builds
    executed     it is not ignored, and it passes when run by exact name
    intact       its source text still hashes to the recorded value
    reachable    its target was built at all by that same selection

The manifest records the case with its package, target, source file and the
SHA-256 of its exact source text — attributes and body, as rustfmt writes them.
Moving a case is a manifest edit; changing what it asserts is a manifest edit a
reviewer has to agree with, which is the point. The hash covers the function
text alone, so a renamed module or a relocated file costs only the `source`
field.

A `doc` case is a documentation example rather than a function, and its hash
covers the fenced block from the opening fence down. The fence is the assertion
there: `compile_fail` is what makes the example a proof, and rewriting it as
`ignore`, `no_run` or `text` leaves the example listed and green while it proves
nothing. rustdoc does not check the error code written beside `compile_fail`, so
that code records which failure the example is about; the hash is what keeps it,
and the attribute rustdoc does honour, from changing unreviewed.

Platform-specific cases are pending on the platforms that cannot run them, not
skipped: each supported platform's own run enforces its own rows.

A case is rerun by exact name in the test binary the selection built, from its
package's directory as cargo would run it, so the rerun is the code the suite
ran and costs no build. Asking cargo for one package instead would resolve that
package's features alone and build it again, different from what was tested.
The binaries are rerun at once, as nextest runs them, except that those holding
a case of one of `.config/nextest.toml`'s test groups take turns.

    scripts/python/required-cases.py <artifacts> <doc-list> <doc-ignored-list> <selection>...
    scripts/python/required-cases.py --self-test  prove the source reader reads,
                                                  what a rerun runs, and which
                                                  reruns take turns
"""

import fnmatch
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from concurrent.futures import ThreadPoolExecutor

MANIFEST = "scripts/required-cases.json"
NEXTEST = ".config/nextest.toml"

PLATFORM = {"linux": "linux", "darwin": "macos", "win32": "windows"}.get(
    sys.platform, sys.platform
)

DOCTEST = re.compile(r"^(?P<source>.+?) - (?P<case>\S+) \(line (?P<line>\d+)\): test$")

DEFINITION = re.compile(r"^(\s*)(?:pub\s+)?(?:async\s+)?fn\s+([^\s(<]+)\s*[(<]")

FILTER = re.compile(
    r"\s*(?:(?P<operator>[&|()])"
    r"|(?P<predicate>[a-z_]+)\((?P<matcher>/(?:\\.|[^/\\])*/|[^()]*)\))"
)

# The predicates a test group may be written with, in the order selector() is
# given a test's fields, each with the matcher nextest uses when none is written.
PREDICATES = {"package": "#", "binary": "#", "test": "~"}


def slashed(path):
    """One spelling of a path, so a manifest reads the same on every platform."""
    return os.path.normpath(path).replace(os.sep, "/")


def function_region(lines, index, indent):
    """The exact source of the function at `index`, with what is attached above.

    Walking up stops at the first blank line or at the end of the item before,
    so `#[test]`, `#[should_panic(expected = ...)]` and the prose above them are
    inside the hash. Walking down stops at the closing brace in the function's
    own column, which `cargo fmt --all --check` is what makes reliable.
    """
    start = index
    while start > 0:
        above = lines[start - 1].strip()
        if not above or above.endswith("}") or above.endswith(";"):
            break
        if not (above.startswith("#") or above.startswith("//")):
            break
        start -= 1
    closing = indent + "}"
    for end in range(index, len(lines)):
        if lines[end] == closing:
            return "\n".join(lines[start : end + 1]) + "\n"
    return None


def fenced_region(lines, index):
    """The documentation example whose opening fence is at `index`.

    Ends at the first line that is nothing but a closing fence once its comment
    marker is off, so the attributes on the opening fence — the error code that
    says which failure the example is about — are inside the hash with the code.
    """
    for end in range(index + 1, len(lines)):
        rest = lines[end].strip()
        for marker in ("///", "//!"):
            if rest.startswith(marker):
                rest = rest[len(marker) :].strip()
                break
        if rest == "```":
            return "\n".join(lines[index : end + 1]) + "\n"
    return None


def definitions(root):
    """Every function defined below `root`, by name, so an ambiguity can be told.

    The tree is read once for every case under it rather than once per case,
    which grows with the cases times the files. A hidden directory holds no
    source of the workspace, but it can hold whole other checkouts of it — a
    worktree, an archived copy — whose cases would read as more definitions of
    the same name.
    """
    found = {}
    for base, directories, files in os.walk(root):
        directories[:] = [one for one in directories if one != "target" and not one.startswith(".")]
        for file in files:
            if not file.endswith(".rs"):
                continue
            path = os.path.join(base, file)
            for index, line in enumerate(open(path, encoding="utf-8").read().splitlines()):
                match = DEFINITION.match(line)
                if match:
                    found.setdefault(match.group(2), []).append(
                        (slashed(path), index, match.group(1))
                    )
    return found


def self_test():
    """A parser that reads nothing passes every hash it is asked about."""
    source = [
        "mod tests {",
        "    use super::*;",
        "",
        "    /// Prose that belongs to the case.",
        "    #[test]",
        "    fn a_named_case() {",
        '        assert_eq!(one(), "two");',
        "    }",
        "",
        "    fn beside_it() {}",
        "}",
    ]
    region = function_region(source, 5, "    ")
    expected = (
        "    /// Prose that belongs to the case.\n"
        "    #[test]\n"
        "    fn a_named_case() {\n"
        '        assert_eq!(one(), "two");\n'
        "    }\n"
    )
    if region != expected:
        print("    FAIL the required-case source reader did not read a case whole")
        return 1
    hollowed = list(source)
    del hollowed[6]
    if function_region(hollowed, 5, "    ") == region:
        print("    FAIL the required-case source reader did not notice a lost assertion")
        return 1

    documented = [
        "/// What went wrong, in the far end's own words.",
        "///",
        "/// ```compile_fail,E0277",
        '/// let trouble = Trouble::new("could not reach the index").unwrap();',
        "/// ```",
        "pub struct Trouble {",
    ]
    fenced = fenced_region(documented, 2)
    example = (
        "/// ```compile_fail,E0277\n"
        '/// let trouble = Trouble::new("could not reach the index").unwrap();\n'
        "/// ```\n"
    )
    if fenced != example:
        print("    FAIL the required-case source reader did not read an example whole")
        return 1
    weakened = list(documented)
    weakened[2] = "/// ```compile_fail"
    if fenced_region(weakened, 2) == fenced:
        print("    FAIL the required-case source reader did not notice a weakened fence")
        return 1

    printed = (
        "test src/lib.rs - Trouble::oneness (line 40) ... ok\n"
        "test src/lib.rs - Trouble::one (line 12) - compile fail ... ok\n"
    )
    if not reported(printed, "src/lib.rs", "Trouble::one", 12):
        print("    FAIL the required-case reader did not see its own example pass")
        return 1
    if reported(printed, "src/lib.rs", "Trouble::one", 40):
        print("    FAIL the required-case reader let a neighbour's example answer for it")
        return 1

    built = Built(
        "target/debug/deps/linux_network-0a1b",
        "crates/crucible-sandbox-local",
        "crucible-sandbox-local",
        "0.1.0",
    )
    command, directory, environment = invocation(built, ["one", "two"])
    if command != ["target/debug/deps/linux_network-0a1b", "--exact", "one", "two"]:
        print("    FAIL a rerun would run something other than the binary the selection built")
        return 1
    if directory != "crates/crucible-sandbox-local":
        print("    FAIL a rerun would start outside the package directory cargo runs its tests from")
        return 1
    if environment.get("CARGO_PKG_NAME") != "crucible-sandbox-local":
        print("    FAIL a rerun would not be told the package cargo tells its tests about")
        return 1
    examples = documented_invocation(["--workspace", "--locked"], ["Trouble::one"])
    if examples != ["cargo", "test", "--workspace", "--locked", "--doc", "--", "Trouble::one"]:
        print("    FAIL a rerun of the examples would not ask for the selection the suite ran")
        return 1

    picks = selector(
        "package(crucible-sandbox-local)\n"
        "| (package(crucible-builtins) & test(/^bash::tests::/))\n"
        "| (package(crucible-app) & binary(hosted_server))\n"
    )
    for test, expected in (
        (("crucible-sandbox-local", "linux_network", "admits_one"), True),
        (("crucible-sandbox-local-extra", "linux_network", "admits_one"), False),
        (("crucible-builtins", "crucible_builtins", "bash::tests::runs_one"), True),
        (("crucible-builtins", "crucible_builtins", "grep::tests::bash::tests::one"), False),
        (("crucible-app", "hosted_server", "serves"), True),
        (("crucible-app", "crucible_app", "serves"), False),
    ):
        if picks(*test) != expected:
            verb = "missed" if expected else "also picked"
            print(f"    FAIL the nextest filter reader {verb} {test}")
            return 1
    for expression, test, expected in (
        ("test(=one)", ("a", "a", "one"), True),
        ("test(=one)", ("a", "a", "one_more"), False),
        ("test(~ne_m)", ("a", "a", "one_more"), True),
        ("test(ne_m)", ("a", "a", "one_more"), True),
        ("binary(#host*)", ("a", "hosted_server", "t"), True),
        ("binary(host*)", ("a", "a_hosted_server", "t"), False),
        ("package(=a) & (test(x) | test(y))", ("a", "a", "y"), True),
        ("package(=a) & (test(x) | test(y))", ("b", "b", "y"), False),
    ):
        if selector(expression)(*test) != expected:
            print(f"    FAIL the nextest filter reader read {expression} wrongly for {test}")
            return 1
    for expression in (
        "not package(a)",
        "package(a) - package(b)",
        "kind(lib)",
        "package(a) &",
        "(package(a)",
    ):
        try:
            selector(expression)
        except ValueError:
            continue
        print(f"    FAIL the nextest filter reader guessed at {expression} instead of refusing it")
        return 1

    with tempfile.TemporaryDirectory() as root:
        path = os.path.join(root, "nextest.toml")
        with open(path, "w", encoding="utf-8") as file:
            file.write(
                "[test-groups]\nconfined = { max-threads = 1 }\n\n"
                "[[profile.default.overrides]]\nfilter = 'package(a)'\ntest-group = 'confined'\n\n"
                "[[profile.ci.overrides]]\nfilter = 'binary(b)'\ntest-group = 'confined'\n\n"
                "[[profile.default.overrides]]\nfilter = 'package(z)'\nslow-timeout = '60s'\n"
            )
        groups = confining(path)
        if sorted(groups) != ["confined"]:
            print(f"    FAIL the nextest group reader found {sorted(groups)}, not confined alone")
            return 1
        for test, expected in (
            (("a", "x", "t"), True),
            (("y", "b", "t"), True),
            (("z", "z", "t"), False),
        ):
            if groups["confined"](*test) != expected:
                verb = "missed" if expected else "also picked"
                print(f"    FAIL the nextest group reader {verb} {test}")
                return 1
        if confining(os.path.join(root, "absent.toml")) != {}:
            print("    FAIL a checkout with no nextest configuration read as grouping tests")
            return 1

    jobs = schedule(
        {
            ("a", "lib", "a"): ["one"],
            ("b", "lib", "b"): ["one"],
            ("c", "lib", "c"): ["one"],
            ("d", "lib", "d"): ["one"],
            ("e", "lib", "e"): ["one"],
            ("f", "lib", "f"): ["one"],
            ("g", "lib", "g"): ["plain", "bash::tests::one"],
            ("h", "lib", "h"): ["plain"],
        },
        {
            "first": lambda package, binary, test: (
                package in ("a", "d", "e") or test.startswith("bash::")
            ),
            "second": lambda package, binary, test: package in ("b", "c", "e"),
        },
    )
    if sorted(sorted(job) for job in jobs) != [
        [("a", "lib", "a"), ("b", "lib", "b"), ("c", "lib", "c")]
        + [("d", "lib", "d"), ("e", "lib", "e"), ("g", "lib", "g")],
        [("f", "lib", "f")],
        [("h", "lib", "h")],
    ]:
        print(f"    FAIL reruns of one nextest group would run beside one another: {jobs}")
        return 1

    with tempfile.TemporaryDirectory() as root:
        for where in ("tests", os.path.join(".worktrees", "other", "tests")):
            os.makedirs(os.path.join(root, where))
            with open(os.path.join(root, where, "main.rs"), "w", encoding="utf-8") as file:
                file.write("fn a_named_case() {\n}\n\nfn twice() {\n}\n")
        with open(os.path.join(root, "tests", "near.rs"), "w", encoding="utf-8") as file:
            file.write("fn a_named_case_too() {\n}\n\nfn twice() {\n}\n")
        defined = definitions(root)
        found = [slashed(os.path.relpath(one[0], root)) for one in defined.get("a_named_case", [])]
        if "tests/near.rs" in found:
            print("    FAIL the required-case finder let a longer name answer for a case")
            return 1
        if found != ["tests/main.rs"]:
            print("    FAIL the required-case finder read a checkout nested under a hidden directory")
            return 1
        found = sorted(slashed(os.path.relpath(one[0], root)) for one in defined.get("twice", []))
        if found != ["tests/main.rs", "tests/near.rs"]:
            print(
                "    FAIL the required-case finder kept one definition of a name defined twice,"
                " so an ambiguous case would read as unique"
            )
            return 1
    return 0


class Built:
    """One test binary the shared selection built, and the package it tests."""

    def __init__(self, executable, directory, package, version):
        self.executable = executable
        self.directory = directory
        self.package = package
        self.version = version

    def environment(self):
        """What cargo tells a test binary about its package when it starts one.

        A case may read these: one proves that none of cargo's own variables
        reach a command, and so needs one to be set.
        """
        return {
            "CARGO_MANIFEST_DIR": self.directory,
            "CARGO_PKG_NAME": self.package,
            "CARGO_PKG_VERSION": self.version,
        }


def executables(stream):
    """Test binaries the shared selection built, by package and target."""
    built = {}
    for line in open(stream, encoding="utf-8"):
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact" or not message.get("executable"):
            continue
        target = message["target"]
        # A bin is built twice under `cargo test`: once as the program, once as
        # the harness that holds its cases. Only the second one can be listed,
        # and only the profile tells them apart.
        if not message.get("profile", {}).get("test"):
            continue
        directory = os.path.dirname(os.path.relpath(message["manifest_path"], os.getcwd()))
        package = "crucible-code" if not directory else os.path.basename(directory)
        # `path+file:///…/crates/name#0.1.0`, or `…#name@0.1.0` when the
        # directory is not called after the package.
        version = message["package_id"].rsplit("#", 1)[-1].rsplit("@", 1)[-1]
        built[(package, target["kind"][0], target["name"])] = Built(
            message["executable"], os.path.dirname(message["manifest_path"]), package, version
        )
    return built


def doctests(listing):
    """Documentation examples the shared selection discovered, and where they are.

    `--no-run --message-format=json` never mentions them: cargo builds no
    binary for a documentation example, so their inventory comes from asking
    rustdoc's own harness. The listed line is the opening fence, which is what
    makes the example's text findable without a line number in the manifest.
    """
    found = {}
    for line in open(listing, encoding="utf-8"):
        match = DOCTEST.match(line.rstrip("\n"))
        if not match:
            continue
        source = slashed(match.group("source"))
        parts = source.split("/")
        package = parts[1] if parts[0] == "crates" and len(parts) > 1 else "crucible-code"
        found.setdefault((package, match.group("case")), []).append(
            (source, int(match.group("line")))
        )
    return found


def listed(executable):
    """What the built binary says it has, and which of those will not run."""

    def names(*extra):
        output = subprocess.run(
            [executable, "--list", *extra], capture_output=True, text=True, check=False
        ).stdout
        return {line[: -len(": test")] for line in output.splitlines() if line.endswith(": test")}

    return names(), names("--ignored")


def invocation(built, names):
    """The command that runs exactly these cases, where, and what it is told.

    cargo starts a test binary in its package's directory, and a case that
    reads a fixture by a relative path depends on that.
    """
    return [built.executable, "--exact", *names], built.directory, built.environment()


def documented_invocation(selection, names):
    """The command that runs the examples with these names under the selection.

    rustdoc builds each example against its crate's library, so asking for the
    selection the suite ran is what keeps that library the one already built.
    """
    return ["cargo", "test", *selection, "--doc", "--", *names]


def attempted(command, directory=None, environment=None):
    """What the command said, and how it exited."""
    result = subprocess.run(
        command,
        cwd=directory,
        env={**os.environ, **(environment or {})},
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode, result.stdout


def ran(built, names):
    """How many cases passed when the built binary was asked for exactly these."""
    code, printed = attempted(*invocation(built, names))
    if code != 0:
        return -1
    return sum(
        int(line.split()[3]) for line in printed.splitlines() if line.startswith("test result:")
    )


def selector(expression):
    """The tests a nextest filter picks, as a predicate on (package, binary, test name).

    Only the forms a test group here is written in are read: `package()`,
    `binary()` and `test()`, joined by `&` and `|` and grouped in parentheses,
    each with nextest's own matchers. Any other form is refused rather than
    guessed at, since a guess could let two binaries of one group run at once.
    """
    text = expression.strip()
    tokens = []
    at = 0
    while at < len(text):
        token = FILTER.match(text, at)
        if not token:
            raise ValueError(f"cannot read {text[at:].strip()!r}")
        tokens.append(token)
        at = token.end()
    position = 0

    def operator():
        return tokens[position].group("operator") if position < len(tokens) else None

    def either():
        nonlocal position
        alternatives = [both()]
        while operator() == "|":
            position += 1
            alternatives.append(both())
        return lambda *test: any(alternative(*test) for alternative in alternatives)

    def both():
        nonlocal position
        parts = [one()]
        while operator() == "&":
            position += 1
            parts.append(one())
        return lambda *test: all(part(*test) for part in parts)

    def one():
        nonlocal position
        if position == len(tokens):
            raise ValueError(f"{text!r} ends where a test was expected")
        token = tokens[position]
        position += 1
        if token.group("operator") == "(":
            inner = either()
            if operator() != ")":
                raise ValueError(f"{text!r} leaves a parenthesis open")
            position += 1
            return inner
        if token.group("operator") or token.group("predicate") not in PREDICATES:
            raise ValueError(f"cannot read {token.group(0).strip()!r}")
        field = list(PREDICATES).index(token.group("predicate"))
        matches = name_matcher(token.group("matcher").strip(), PREDICATES[token.group("predicate")])
        return lambda *test: matches(test[field])

    picks = either()
    if position != len(tokens):
        raise ValueError(f"cannot read {tokens[position].group(0).strip()!r}")
    return picks


def name_matcher(text, default):
    """nextest's `=` equal, `~` contains, `#` glob and `/regex/`, or the predicate's own."""
    if len(text) > 1 and text.startswith("/") and text.endswith("/"):
        pattern = re.compile(text[1:-1])
        return lambda value: pattern.search(value) is not None
    form, body = (text[0], text[1:]) if text[:1] in ("=", "~", "#") else (default, text)
    if form == "=":
        return lambda value: value == body
    if form == "~":
        return lambda value: body in value
    return lambda value: fnmatch.fnmatchcase(value, body)


def confining(path):
    """nextest's test groups, each as a test of which cases belong to it.

    A group is read from the overrides of every profile, since a test that one
    profile groups cannot be told apart here from one it does not. A checkout
    with no nextest configuration groups nothing, as it does for nextest.
    """
    try:
        source = open(path, "rb")
    except FileNotFoundError:
        return {}
    with source:
        config = tomllib.load(source)
    picked = {}
    for profile in config.get("profile", {}).values():
        for override in profile.get("overrides", []):
            if "test-group" in override and "filter" in override:
                picked.setdefault(override["test-group"], []).append(selector(override["filter"]))
    return {
        group: (lambda *test, picks=picks: any(pick(*test) for pick in picks))
        for group, picks in picked.items()
    }


def schedule(reruns, groups):
    """The binary reruns that may go at once, each a list taken in turn.

    nextest runs no more of a group's tests at once than the group allows,
    while a binary here runs every case it is asked for at once. So a binary
    holding a case of a group takes its turn with every other binary that does,
    as strictly as a group of one, and two groups that share a binary share
    their turns. Every other binary goes beside them, as nextest runs it.
    """
    jobs = []
    for key, names in sorted(reruns.items()):
        package, _, binary = key
        shared = {
            group
            for group, picks in groups.items()
            if any(picks(package, binary, name) for name in names)
        }
        joined = [job for job in jobs if job[0] & shared]
        for job in joined:
            jobs.remove(job)
            shared |= job[0]
        jobs.append((shared, sorted([key, *(one for job in joined for one in job[1])])))
    return [keys for _, keys in jobs]


def reported(printed, source, name, line):
    """Whether that output says this one example ran and passed.

    A filter matches anywhere in a test's name, so asking for `Item::one` also
    runs the examples on `Item::oneness`, and counting passes would answer a
    question nobody asked. rustdoc prints a line per example naming the file and
    the line its opening fence is on, which no neighbour can answer to.
    """
    head = f"test {source} - {name} (line {line})"
    for one in printed.splitlines():
        one = one.replace(os.sep, "/")
        if one.startswith(head) and one.endswith("... ok"):
            return True
    return False


def check(stream, listing, silenced, selection):
    manifest = json.load(open(MANIFEST, encoding="utf-8"))["cases"]
    if not manifest:
        print(f"    FAIL {MANIFEST} names no required case; this check measured nothing")
        return 1
    built = executables(stream)
    if not built:
        print("    FAIL the test selection built no test binary; this check measured nothing")
        return 1
    try:
        groups = confining(NEXTEST)
    except ValueError as error:
        print(
            f"    FAIL {NEXTEST} does not read as test groups here ({error});"
            " teach selector() the form, so a group's reruns still take turns"
        )
        return 1
    documented = doctests(listing)
    ignored_examples = doctests(silenced)

    failed = 0
    run = {}
    inventory = {}
    sources = {}
    for case in manifest:
        kind = case["target"]["kind"]
        key = (case["package"], kind, case["target"]["name"])
        name = case["case"]
        line = None
        if PLATFORM not in case["platforms"]:
            print(f"    pending on {PLATFORM}: {case['id']} — {case['obligation']}")
            continue

        if kind == "doc":
            # A documentation example is built with its crate's library and run
            # by `cargo test --doc`, so the library the selection built is what
            # says whether the example was in reach at all.
            if (case["package"], "lib", case["target"]["name"]) not in built:
                print(
                    f"    FAIL {case['id']} is out of reach: the test selection did not build"
                    f" {case['package']} lib {case['target']['name']}"
                )
                failed = 1
                continue
            if (case["package"], name) in ignored_examples:
                print(f"    FAIL {case['id']} is discovered but does not run: {name} is ignored")
                failed = 1
                continue
            where = documented.get((case["package"], name), [])
            if not where:
                print(f"    FAIL {case['id']} is gone: {name} was not discovered in {case['package']}")
                failed = 1
                continue
            if len(where) != 1:
                places = ", ".join(f"{one} line {two}" for one, two in where)
                print(
                    f"    FAIL {case['id']} resolves to {len(where)} examples ({places});"
                    f" an example has no name of its own, so leave the required one the"
                    f" only example on {name}"
                )
                failed = 1
                continue
            path, line = where[0]
            if path != case["source"]:
                print(f"    FAIL {case['id']} moved to {path}; update its source in {MANIFEST}")
                failed = 1
                continue
            region = fenced_region(open(path, encoding="utf-8").read().splitlines(), line - 1)
            if region is None:
                print(f"    FAIL {case['id']} has no closing fence in {path}")
                failed = 1
                continue
        else:
            if key not in built:
                print(
                    f"    FAIL {case['id']} is out of reach: the test selection did not build"
                    f" {case['package']} {kind} {case['target']['name']}"
                )
                failed = 1
                continue
            if key not in inventory:
                inventory[key] = listed(built[key].executable)
            discovered, ignored = inventory[key]
            if name not in discovered:
                print(f"    FAIL {case['id']} is gone: {name} was not discovered in {case['package']}")
                failed = 1
                continue
            if name in ignored:
                print(f"    FAIL {case['id']} is discovered but does not run: {name} is ignored")
                failed = 1
                continue

            root = "." if case["package"] == "crucible-code" else os.path.join("crates", case["package"])
            if root not in sources:
                sources[root] = definitions(root)
            found = sources[root].get(name.split("::")[-1], [])
            if len(found) != 1:
                where = ", ".join(one[0] for one in found) or "nowhere"
                print(f"    FAIL {case['id']} resolves to {len(found)} definitions ({where}); name it uniquely")
                failed = 1
                continue
            path, index, indent = found[0]
            if path != case["source"]:
                print(f"    FAIL {case['id']} moved to {path}; update its source in {MANIFEST}")
                failed = 1
                continue
            region = function_region(open(path, encoding="utf-8").read().splitlines(), index, indent)
            if region is None:
                print(f"    FAIL {case['id']} has no closing brace in its own column in {path}")
                failed = 1
                continue

        digest = hashlib.sha256(region.encode("utf-8")).hexdigest()
        if digest != case["body_sha256"]:
            print(f"    FAIL {case['id']} no longer asserts what it did: {name}")
            print(f"         {case['obligation']}")
            print(f"         review the diff, then record {digest} in {MANIFEST}")
            failed = 1
            continue
        run.setdefault(key, []).append((case, name, line))

    # rustdoc names an example after the item it documents, and the listed name
    # is the only handle a filter has. A filter matches anywhere, so what says
    # an obligation ran is the line rustdoc printed for its exact example, not
    # how many passed beside it, and that line also says which one stopped
    # holding. Every example is asked for in one run.
    examples = [one for key, cases in sorted(run.items()) if key[1] == "doc" for one in cases]
    if examples:
        names = sorted({name for _, name, _ in examples})
        code, printed = attempted(documented_invocation(selection, names))
        held = True
        for case, name, line in examples:
            if not reported(printed, case["source"], name, line):
                print(f"    FAIL {case['id']} no longer holds: {name} did not run and pass")
                print(f"         {case['obligation']}")
                held = False
                failed = 1
        if code != 0 and held:
            print("    FAIL the run of the required examples failed; rerun it and read the first failure")
            failed = 1

    reruns = {key: [name for _, name, _ in cases] for key, cases in run.items() if key[1] != "doc"}
    with ThreadPoolExecutor() as pool:
        passed = dict(
            pair
            for job in pool.map(
                lambda keys: [(key, ran(built[key], reruns[key])) for key in keys],
                schedule(reruns, groups),
            )
            for pair in job
        )
    for key, names in sorted(reruns.items()):
        package, kind, target = key
        if passed[key] != len(names):
            print(
                f"    FAIL {len(names)} required cases in {package} {kind} {target} did not all pass;"
                " rerun that target and read the assertion"
            )
            failed = 1

    return failed


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) < 5:
        print(__doc__)
        return 2
    return self_test() or check(argv[1], argv[2], argv[3], argv[4:])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
