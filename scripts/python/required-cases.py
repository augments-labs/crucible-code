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
Apart from the cases the gate's profile runs alone, below, the binaries are
rerun beside one another, no more binaries at once than there are processors,
which stands in for the gate's nextest run, where the tests that keep a
processor busy run at most one per processor, since each binary runs its own
cases on threads of its own. Those holding a case of one of
`.config/nextest.toml`'s test groups take turns.

A test whose own text names `SOON_REFUSED` asserts a wall-clock bound, which a
test beside it that keeps a processor busy can make it miss, so the gate's
nextest profile runs each such test alone and picks them by name. Every binary
the selection built is listed, and the gate fails when a test that names the
bound is not run alone, or one run alone no longer names it. A required case
the profile runs alone is rerun in a run of its own, with no other rerun
going.

    scripts/python/required-cases.py <artifacts> <doc-list> <doc-ignored-list> <selection>...
    scripts/python/required-cases.py --self-test  prove the source reader reads,
                                                  what a rerun runs, which
                                                  reruns take turns, and which
                                                  tests run alone
    scripts/python/required-cases.py --gate-threads <processors>  print how many
                                                  slots the gate's nextest run
                                                  has on that many processors
"""

import contextlib
import fnmatch
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
import warnings
from concurrent.futures import ThreadPoolExecutor

MANIFEST = "scripts/required-cases.json"
NEXTEST = ".config/nextest.toml"
# The nextest profile scripts/sh/rust-checks.sh runs the suite under.
GATE_PROFILE = "gate"
# The slots an override of that profile gives a test it runs with no other.
ALONE = "num-test-threads"
# The wall-clock bound crucible-raster's refusal tests assert, which a test
# beside them that keeps a processor busy can make them miss.
TIMED = re.compile(r"\bSOON_REFUSED\b")

PLATFORM = {"linux": "linux", "darwin": "macos", "win32": "windows"}.get(
    sys.platform, sys.platform
)

DOCTEST = re.compile(r"^(?P<source>.+?) - (?P<case>\S+) \(line (?P<line>\d+)\): test$")

DEFINITION = re.compile(r"^(\s*)(?:pub\s+)?(?:async\s+)?fn\s+([^\s(<]+)\s*[(<]")

# A nextest regex matcher as nextest delimits one: inside the slashes, `\/` is
# the only escape it reads, and the first `/` not so escaped ends it. The
# repetition is possessive because nextest never goes back to read a `\` that
# began a `\/` as a character of its own.
REGEX = re.compile(r"/(?:\\/|[^/])*+/")

FILTER = re.compile(
    r"\s*(?:(?P<operator>[&|()])"
    rf"|(?P<predicate>[a-z_]+)\((?P<matcher>{REGEX.pattern}|[^()]*)\))"
)

# The predicates a test group may be written with, in the order selector() is
# given a test's fields, each with the matcher nextest uses when none is written.
PREDICATES = {"package": "#", "binary": "#", "test": "~"}


def processors():
    """The processors this process may run on.

    Where the system can say, that is the processors the process is allowed,
    which a container or `taskset` can make fewer than the machine has.
    nextest counts with Rust's `available_parallelism`, which also lowers the
    count to a cgroup's CPU quota, so in a container with a quota this can
    count more than nextest does.
    """
    if hasattr(os, "sched_getaffinity"):
        return len(os.sched_getaffinity(0))
    return os.cpu_count() or 1


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

    The tree is walked once and every case under it looks its name up here,
    rather than walking it again for each case, which grows with the cases
    times the files. A hidden directory holds no source of the workspace, but
    it can hold whole other checkouts of it — a worktree, an archived copy —
    whose cases would read as more definitions of the same name.
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
        (r"test(/\(a\/b\)/)", ("a", "a", "(a/b)"), True),
        (r"test(/a\\</)", ("a", "a", "a\\<"), True),
        (r"test(/^bash\\/?::tests::/)", ("a", "a", "bash::tests::runs_one"), True),
        (r"test(/^a\\/?\(b\)/)", ("a", "a", "a(b)"), True),
        (r"test(/^a\/b\\/c$/)", ("a", "a", "a/b/c"), True),
    ):
        try:
            picked = selector(expression)(*test)
        except ValueError as error:
            print(f"    FAIL the nextest filter reader refused {expression}, which nextest reads ({error})")
            return 1
        if picked != expected:
            print(f"    FAIL the nextest filter reader read {expression} wrongly for {test}")
            return 1
    for expression in (
        "not package(a)",
        "package(a) - package(b)",
        "kind(lib)",
        "package(a) &",
        "(package(a)",
        ")",
        r"test(a\,b)",
        "package({pa,pb})",
        "binary([^a]*)",
        "test(/(/)",
        "test(/[[:digit:]]x/)",
        r"test(/\<a/)",
        r"test(/a\>/)",
        r"test(/\b{start}a/)",
        r"test(/a\€/)",
        r"test(/a\\/)",
        "test(/a/b/)",
        "test(/a)",
        "test(/)",
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
                "[[profile.default.overrides]]\nfilter = 'package(a)'\nthreads-required = 1\ntest-group = 'confined'\n\n"
                "[[profile.ci.overrides]]\nfilter = 'binary(b)'\ntest-group = 'confined'\n\n"
                "[[profile.default.overrides]]\nfilter = 'package(z)'\nslow-timeout = '60s'\n\n"
                "[[profile.default.overrides]]\nplatform = 'cfg(windows)'\ntest-group = 'serial'\n"
            )
        groups = confining(path)
        if sorted(groups) != ["confined", "serial"]:
            print(f"    FAIL the nextest group reader found {sorted(groups)}, not confined and serial")
            return 1
        for group, test, expected in (
            ("confined", ("a", "x", "t"), True),
            ("confined", ("y", "b", "t"), True),
            ("confined", ("z", "z", "t"), False),
            ("serial", ("z", "z", "t"), True),
        ):
            if groups[group](*test) != expected:
                verb = "missed" if expected else "also picked"
                print(f"    FAIL the nextest group reader {verb} {test} for {group}")
                return 1
        if confining(os.path.join(root, "absent.toml")) != {}:
            print("    FAIL a checkout with no nextest configuration read as grouping tests")
            return 1
        for written, threads in ((path, 4), (os.path.join(root, "absent.toml"), 4)):
            if gate_threads(written, 4) != threads:
                print(f"    FAIL a gate profile that gives a test one slot was not run with {threads} slots on 4 processors")
                return 1
        with open(path, "w", encoding="utf-8") as file:
            file.write("[test-groups]\nconfined = { max-threads = 1 }\n")
        if confining(path) != {}:
            print("    FAIL a test group no override fills read as grouping tests")
            return 1
        with open(path, "w", encoding="utf-8") as file:
            file.write(
                "[test-groups]\nconfined = { max-threads = 1 }\n\n"
                f"[profile.{GATE_PROFILE}]\nthreads-required = 3\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(w)'\nthreads-required = 1\n\n"
                "[[profile.default.overrides]]\nfilter = 'package(a)'\ntest-group = 'confined'\n"
            )
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            weighted = test_groups(path)
        if (
            weighted is None
            or sorted(weighted) != ["confined"]
            or not weighted["confined"]("a", "a", "t")
            or weighted["confined"]("w", "w", "t")
        ):
            print(
                f"    FAIL the slots profile {GATE_PROFILE} gives every test, which put no test in"
                " a group of its own, were not read as the same test groups"
            )
            return 1
        if gate_threads(path, 4) != 12:
            print(f"    FAIL a profile {GATE_PROFILE} giving every test 3 slots was not run with 12 slots on 4 processors")
            return 1
        for counted, threads in (("4", "12"), ("2", "6")):
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                code = print_gate_threads(path, counted)
            if code != 0 or printed.getvalue() != f"{threads}\n":
                print(
                    f"    FAIL --gate-threads {counted} did not succeed with the {threads} slots alone on its line,"
                    " which rust-checks.sh reads"
                )
                return 1
        for counted in ("", "0", "-2", "x", "2\r", "\u00b2"):
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                code = print_gate_threads(path, counted)
            if code != 1 or "--gate-threads takes" not in printed.getvalue():
                print(f"    FAIL --gate-threads did not refuse {counted!r} as a count of processors, saying why")
                return 1
        for written, said in (
            (b"[test-groups\n", "is not TOML"),
            (b"[test-groups]\nconfined = { max-threads = 1 } # \xff\n", "is not TOML"),
            (b"[[profile.default.overrides]]\nfilter = 'kind(lib)'\ntest-group = 'confined'\n", "teach"),
            (b"[[profile.default.overrides]]\nfilter = 5\ntest-group = 'confined'\n", "teach"),
            (b"[[profile.default.overrides]]\nfilter = 'package(a)'\nthreads-required = 2\n", "teach"),
            (b"[profile.ci]\ntest-threads = 4\n", "teach"),
            (b"[profile.default]\nthreads-required = 2\n", "teach"),
            (b"[profile.ci]\nthreads-required = 3\n", "teach"),
            (f"[profile.{GATE_PROFILE}]\nthreads-required = 'num-test-threads'\n".encode(), "teach"),
            (f"[profile.{GATE_PROFILE}]\nthreads-required = true\n".encode(), "teach"),
            (f"[profile.{GATE_PROFILE}]\nthreads-required = 0\n".encode(), "teach"),
            (f"[profile.{GATE_PROFILE}]\ntest-threads = 12\n".encode(), "teach"),
            (
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(a)'\nthreads-required = 2\n".encode(),
                "teach",
            ),
            (f"[[profile.ci.overrides]]\nfilter = 'package(a)'\nthreads-required = '{ALONE}'\n".encode(), "teach"),
            (
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(a)'\nthreads-required = 1\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(b)'\nthreads-required = '{ALONE}'\n".encode(),
                "teach",
            ),
            (
                f"[[profile.{GATE_PROFILE}.overrides]]\nplatform = 'cfg(windows)'\nfilter = 'package(b)'\n"
                f"threads-required = '{ALONE}'\n".encode(),
                "teach",
            ),
            (
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'binary_id(crucible-raster)'\n"
                f"threads-required = '{ALONE}'\n".encode(),
                "teach",
            ),
        ):
            with open(path, "wb") as file:
                file.write(written)
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                refused = test_groups(path)
            if refused is not None or said not in printed.getvalue():
                print(f"    FAIL a nextest configuration it cannot read was not refused with {said!r}")
                return 1
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                code = print_gate_threads(path, "4")
            if code != 1 or said not in printed.getvalue():
                print(f"    FAIL --gate-threads did not fail, saying why, on a nextest configuration it cannot read: {written!r}")
                return 1
        with open(path, "w", encoding="utf-8") as file:
            file.write(
                f"[profile.{GATE_PROFILE}]\nthreads-required = 3\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(s)'\nslow-timeout = '60s'\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(r) & test(=t)'\n"
                f"threads-required = '{ALONE}'\npriority = 100\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(q)'\nthreads-required = '{ALONE}'\n\n"
                f"[[profile.{GATE_PROFILE}.overrides]]\nfilter = 'package(w)'\nthreads-required = 1\n"
            )
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            read, threads = test_groups(path), gate_threads(path, 4)
        if read != {} or threads != 12:
            print(
                f"    FAIL a profile {GATE_PROFILE} running tests alone before any other override sets their"
                f" slots was not read as grouping none, with 12 slots on 4 processors: {printed.getvalue()!r}"
            )
            return 1
        lone = alone(path)
        for test, expected in (
            (("r", "r", "t"), True),
            (("q", "q", "x"), True),
            (("r", "r", "u"), False),
            (("s", "s", "t"), False),
            (("w", "w", "t"), False),
        ):
            if lone(*test) != expected:
                verb = "missed" if expected else "also picked"
                print(f"    FAIL the reader of the tests profile {GATE_PROFILE} runs alone {verb} {test}")
                return 1
        if alone(os.path.join(root, "absent.toml"))("r", "r", "t"):
            print("    FAIL a checkout with no nextest configuration read as running a test alone")
            return 1

    if os.path.exists(NEXTEST):
        real = test_groups(NEXTEST)
        if real is None:
            return 1
        for test, expected in (
            (("crucible-sandbox-local", "sandbox_admission", "admits"), True),
            (("crucible-builtins", "crucible_builtins", "bash::tests::runs"), True),
            (("crucible-app", "hosted_server", "serves"), True),
            (("crucible-builtins", "crucible_builtins", "grep::tests::bash"), False),
            (("crucible-app", "crucible_app", "serves"), False),
        ):
            if any(picks(*test) for picks in real.values()) != expected:
                verb = "missed" if expected else "also picked"
                print(
                    f"    FAIL the test groups of {NEXTEST} as read here {verb} {test}; either"
                    " confining() misreads it, or it changed on purpose and these rows in"
                    " self_test() should say what it now groups"
                )
                return 1
        lone = alone(NEXTEST)
        gradient = "a_gradient_costs_its_pixels_and_its_stops_together"
        for test, expected in (
            (("crucible-raster", "crucible_raster", f"tests::{gradient}"), True),
            (("crucible-raster", "hostile", "a_file_refused_for_its_work_is_refused_before_the_work_is_done"), True),
            (("crucible-raster", "crucible_raster", f"tests::{gradient}_twice"), False),
            (("crucible-builtins", "crucible_builtins", f"tests::{gradient}"), False),
        ):
            if lone(*test) != expected:
                verb = "missed" if expected else "also picked"
                print(
                    f"    FAIL the tests {NEXTEST} runs alone as read here {verb} {test}; either alone()"
                    " misreads it, or it changed on purpose and these rows in self_test() should say"
                    " what it now runs alone"
                )
                return 1
        for count in ("2", "5"):
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                code = main(["required-cases.py", "--gate-threads", count])
            expected = gate_threads(NEXTEST, int(count))
            if code != 0 or printed.getvalue() != f"{expected}\n":
                first = printed.getvalue().partition("\n")[0]
                print(
                    f"    FAIL required-cases.py --gate-threads {count} exited {code} printing {first!r},"
                    f" not the {expected} slots {NEXTEST} gives {count} processors"
                )
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
    jobs = schedule(
        {("a", "lib", "a"): ["one"], ("b", "lib", "b"): ["one"], ("c", "lib", "c"): ["one"]},
        {
            "first": lambda package, binary, test: package in ("a", "c"),
            "second": lambda package, binary, test: package in ("a", "b"),
        },
    )
    if jobs != [[("a", "lib", "a"), ("b", "lib", "b"), ("c", "lib", "c")]]:
        print(f"    FAIL a binary in two groups let one group's reruns run beside it: {jobs}")
        return 1

    affinity, count = getattr(os, "sched_getaffinity", None), os.cpu_count
    os.sched_getaffinity, os.cpu_count = (lambda pid: {0, 1}), (lambda: 5)
    try:
        allowed = processors()
    finally:
        os.cpu_count = count
        if affinity is None:
            del os.sched_getaffinity
        else:
            os.sched_getaffinity = affinity
    if allowed != 2:
        print(f"    FAIL {allowed} processors were counted where the process may run on 2 of 5")
        return 1

    guarded = threading.Lock()
    going = set()
    asked = []
    overlaps = []
    # One processor runs no two reruns at once, so only more can show them meeting.
    meeting = threading.Barrier(2, timeout=5) if processors() > 1 else None

    def answered(binary, names):
        with guarded:
            if binary in ("a", "b") and going & {"a", "b"}:
                overlaps.append(binary)
            going.add(binary)
            asked.append(binary)
        try:
            if binary in ("c", "d") and meeting:
                meeting.wait()
            else:
                time.sleep(0.02)
        finally:
            with guarded:
                going.discard(binary)
        return len(names) - (binary in ("b", "d"))

    run = {
        (package, kind, target): [(None, "one", None), (None, "two", None)]
        for package, kind, target in (
            ("a", "lib", "a"),
            ("b", "lib", "b"),
            ("c", "lib", "c"),
            ("d", "test", "d"),
            ("e", "doc", "e"),
        )
    }
    printed = io.StringIO()
    try:
        with contextlib.redirect_stdout(printed):
            failed = rerun(
                {key: key[2] for key in run},
                run,
                {"confined": lambda package, binary, test: binary in ("a", "b")},
                lambda package, binary, test: False,
                answered,
            )
    except threading.BrokenBarrierError:
        print("    FAIL reruns in no nextest group did not run beside one another")
        return 1
    if overlaps:
        print("    FAIL two reruns of one nextest group ran beside one another")
        return 1
    if sorted(asked) != ["a", "b", "c", "d"]:
        print(f"    FAIL the binary reruns asked {sorted(asked)}, not every binary but the examples")
        return 1
    lines = printed.getvalue().splitlines()
    if failed != 1 or len(lines) != 2:
        print(f"    FAIL two reruns that did not all pass were reported as {lines}, exit {failed}")
        return 1
    if " in b lib b " not in lines[0] or " in d test d " not in lines[1]:
        print(f"    FAIL the two reruns that did not all pass were not named in one order: {lines}")
        return 1

    now = [0, 0]

    def counted(binary, names):
        with guarded:
            now[0] += 1
            now[1] = max(now)
        time.sleep(0.02)
        with guarded:
            now[0] -= 1
        return len(names)

    many = {(name, "lib", name): [(None, "one", None)] for name in "abcde"}
    with contextlib.redirect_stdout(io.StringIO()):
        rerun({key: key[2] for key in many}, many, {}, lambda package, binary, test: False, counted, width=lambda: 2)
    if now[1] > 2:
        print(f"    FAIL {now[1]} binary reruns went at once where two processors allow two")
        return 1

    # Grouped with d, a reruns after b and c, and b is handed in before a, so
    # lines printed as the reruns finished, in the order they were scheduled
    # or in the order they were handed in would name b before a.
    late = {(name, "lib", name): [(None, "one", None)] for name in "dbca"}
    printed = io.StringIO()
    with contextlib.redirect_stdout(printed):
        rerun(
            {key: key[2] for key in late},
            late,
            {"confined": lambda package, binary, test: package in ("a", "d")},
            lambda package, binary, test: False,
            lambda binary, names: len(names) - (binary in ("a", "b")),
            width=lambda: 1,
        )
    lines = printed.getvalue().splitlines()
    if len(lines) != 2 or " in a lib a " not in lines[0] or " in b lib b " not in lines[1]:
        print(f"    FAIL reruns that did not all pass were not printed in name order: {lines}")
        return 1

    # a holds two cases the gate's profile runs alone beside one it does not,
    # and d holds only such a case, so a rerun that asked d for what is left of
    # it would ask for no case, which a test binary reads as every test. b and
    # c meet, so the rest still go at once. No package is named as its target,
    # so a rerun that asked `lone` about anything but the package, the target
    # and the case would run nothing alone. Each run answers wrongly for some
    # reruns, or none; the last two put one rerun's surplus against another's
    # shortfall, which still leaves a case that did not pass.
    lonely = {
        ("pa", "lib", "a"): [(None, "timed", None), (None, "plain", None), (None, "timed2", None)],
        ("pb", "lib", "b"): [(None, "one", None)],
        ("pc", "lib", "c"): [(None, "one", None)],
        ("pd", "test", "d"): [(None, "timed", None)],
    }
    picked = {("pa", "a", "timed"), ("pa", "a", "timed2"), ("pd", "d", "timed")}
    for wrong, named in (
        ({}, []),
        ({("a", ("timed",)): -1}, ["pa lib a"]),
        ({("a", ("plain",)): -1}, ["pa lib a"]),
        ({("d", ("timed",)): -1}, ["pd test d"]),
        ({("b", ("one",)): -1}, ["pb lib b"]),
        ({("a", ("timed",)): 0, ("a", ("plain",)): 2}, ["pa lib a"]),
        ({("a", ("timed",)): 2, ("a", ("plain",)): 0}, ["pa lib a"]),
    ):
        calls = []
        crowded = []
        going = [0, False]
        meeting = threading.Barrier(2, timeout=5)

        def answered(binary, names):
            alone_here = any(name.startswith("timed") for name in names)
            with guarded:
                if going[1] or (alone_here and going[0]):
                    crowded.append((binary, names))
                going[0] += 1
                going[1] = going[1] or alone_here
                calls.append((binary, list(names)))
            try:
                if binary in ("b", "c"):
                    meeting.wait()
                else:
                    time.sleep(0.02)
            finally:
                with guarded:
                    going[0] -= 1
                    going[1] = going[1] and not alone_here
            return wrong.get((binary, tuple(names)), len(names))

        printed = io.StringIO()
        try:
            with contextlib.redirect_stdout(printed):
                failed = rerun(
                    {key: key[2] for key in lonely},
                    lonely,
                    {},
                    lambda package, binary, test: (package, binary, test) in picked,
                    answered,
                    width=lambda: 3,
                )
        except threading.BrokenBarrierError:
            print("    FAIL reruns of cases not run alone did not run beside one another")
            return 1
        expected = [
            ("a", ["plain"]),
            ("a", ["timed"]),
            ("a", ["timed2"]),
            ("b", ["one"]),
            ("c", ["one"]),
            ("d", ["timed"]),
        ]
        if sorted(calls) != expected:
            print(f"    FAIL the reruns asked {sorted(calls)}, not each case run alone by itself and the rest by binary")
            return 1
        if crowded:
            print(f"    FAIL a case run alone was rerun beside another rerun: {crowded}")
            return 1
        lines = printed.getvalue().splitlines()
        if failed != bool(named) or [line.split(" in ")[1].split(" did ")[0] for line in lines] != named:
            print(f"    FAIL with {wrong} answered, the reruns were reported as {lines}, exit {failed}")
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

    beside = threading.Barrier(2, timeout=5)

    def listing(executable):
        beside.wait()
        return {executable}, set()

    try:
        lists = inventories(
            {("a", "lib", "a"): Built("one", ".", "a", "0"), ("b", "test", "b"): Built("two", ".", "b", "0")},
            listing,
            lambda: 2,
        )
    except threading.BrokenBarrierError:
        print("    FAIL two built binaries were not listed beside one another where two processors allow it")
        return 1
    if lists != {("a", "lib", "a"): ({"one"}, set()), ("b", "test", "b"): ({"two"}, set())}:
        print(f"    FAIL two built binaries were listed as {lists}, not each as itself")
        return 1

    with tempfile.TemporaryDirectory() as root:
        with open(os.path.join(root, "tests.rs"), "w", encoding="utf-8") as file:
            file.write(
                "const SOON_REFUSED: Duration = Duration::from_millis(100);\n\n"
                "#[test]\nfn timed_and_alone() {\n    assert!(took() < SOON_REFUSED);\n}\n\n"
                "#[test]\nfn untimed_beside() {\n    assert!(took() < SOON_REFUSED_TWICE);\n}\n\n"
                "#[test]\nfn timed_beside() {\n    assert!(took() < SOON_REFUSED);\n}\n\n"
                "#[test]\nfn alone_untimed() {\n    assert!(took() < LATER);\n}\n\n"
                "#[test]\nfn twice() {\n    assert!(took() < SOON_REFUSED);\n}\n\n"
                "mod again {\n    #[test]\n    fn twice() {\n    }\n}\n\n"
                "#[test]\nfn unclosed() {\n    assert!(took() < SOON_REFUSED);\n  }\n"
            )
        with open(os.path.join(root, "plain.rs"), "w", encoding="utf-8") as file:
            file.write("#[test]\nfn unclosed_elsewhere() {\n  }\n")
        defined = {package_root("p"): definitions(root)}
        # Where a name resolves to two definitions and only one names the bound,
        # the profile running it alone is right for one and wrong for the other,
        # so it fails as ambiguous whichever way the profile runs it.
        for names, chosen, said, reason in (
            (
                ["tests::timed_and_alone", "tests::untimed_beside", "tests::from_a_macro", "unclosed_elsewhere"],
                {"tests::timed_and_alone"},
                None,
                None,
            ),
            (["tests::timed_beside"], set(), "tests::timed_beside", "runs it beside other tests"),
            (["tests::alone_untimed"], {"tests::alone_untimed"}, "tests::alone_untimed", "names no SOON_REFUSED"),
            (["tests::from_a_macro"], {"tests::from_a_macro"}, "tests::from_a_macro", "resolves to 0 definitions"),
            (["tests::twice"], set(), "tests::twice", "resolves to 2 definitions"),
            (["tests::again::twice"], {"tests::again::twice"}, "tests::again::twice", "resolves to 2 definitions"),
            (["tests::unclosed"], set(), "tests::unclosed", "has no closing brace"),
        ):
            printed = io.StringIO()
            with contextlib.redirect_stdout(printed):
                failed = timed(
                    {("p", "lib", "b"): (set(names), set())},
                    defined,
                    lambda package, binary, test, chosen=chosen: (package, binary) == ("p", "b") and test in chosen,
                )
            lines = printed.getvalue().splitlines()
            if said is None and (failed or lines):
                print(f"    FAIL tests run alone exactly when they name SOON_REFUSED were refused: {lines}")
                return 1
            if said is not None and (
                failed != 1 or len(lines) != 1 or f" {said} " not in lines[0] or reason not in lines[0]
            ):
                print(f"    FAIL {said} did not fail alone and by name with {reason!r}: {lines}")
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
    if not isinstance(expression, str):
        raise ValueError(f"the filter {expression!r} is not text")
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
    """nextest's `=` equal, `~` contains, `#` glob and `/regex/`, or the predicate's own.

    nextest reads an escape in the first three, and an alternation and a `[^`
    negated class in a glob, which fnmatch would take as they stand, so all
    three are refused. A regex is what nextest hands Rust: the text between
    the slashes as `REGEX` delimits it, with each `\\/` made `/`. Text that
    opens with a slash but is not one such regex is refused, since nextest
    refuses it, and so is a regex Python cannot compile, or one Python warns
    it may read otherwise, which is where Rust's class syntax, such as
    `[[:digit:]]` or `&&`, departs from Python's. Both read an escaped ASCII
    character that is not a letter or digit as itself, except that Rust reads
    `\\<` and `\\>` as word boundaries. An escaped letter or digit, such as
    `\\b{start}`, can mean something else in each, and Rust refuses an
    escaped character beyond ASCII, so a regex escaping any of these is
    refused too.
    """
    if text.startswith("/"):
        if not REGEX.fullmatch(text):
            raise ValueError(
                f"cannot read {text!r} as one regex: nextest reads a regex from its opening / to the first"
                " / after it not written \\/, and that / must end the matcher"
            )
        body = text[1:-1].replace("\\/", "/")
        if any(
            not (escaped.isascii() and not escaped.isalnum()) or escaped in "<>"
            for escaped in re.findall(r"\\(.)", body, re.DOTALL)
        ):
            raise ValueError(f"cannot read {text!r} as a regex, as Rust may read an escape in it otherwise")
        try:
            with warnings.catch_warnings():
                warnings.simplefilter("error", FutureWarning)
                pattern = re.compile(body)
        except (re.error, FutureWarning) as error:
            raise ValueError(f"cannot read {text!r} as a regex ({error})") from error
        return lambda value: pattern.search(value) is not None
    form, body = (text[0], text[1:]) if text[:1] in ("=", "~", "#") else (default, text)
    if "\\" in body or (form == "#" and ("{" in body or "}" in body or "[^" in body)):
        raise ValueError(f"cannot read {text!r} as it stands")
    if form == "=":
        return lambda value: value == body
    if form == "~":
        return lambda value: body in value
    return lambda value: fnmatch.fnmatchcase(value, body)


def confining(path):
    """nextest's test groups, each as a test of which cases belong to it.

    A group is read from the overrides of every profile and every platform,
    since a test that one profile or platform groups cannot be told apart here
    from one it does not, and an override naming only a platform groups every
    test. A checkout with no nextest configuration declares no groups, so
    nothing takes turns. A profile's `test-threads`, and a `threads-required`
    other than 1 that a profile sets for every test or an override for some,
    change how many tests nextest runs at once in a way no group says, so
    either is refused rather than read as nothing. One exception is a
    whole number of slots that the gate's own profile gives every test:
    `scripts/sh/rust-checks.sh` runs that profile with that many slots per
    processor, so the tests holding them run at most one per processor, the
    slots put no test in a group of its own, and only the tests an override
    sets back to one slot run more than one per processor.

    An override of the gate's profile may also give its tests every slot the
    run has, `threads-required = "num-test-threads"`, so each runs with no
    other test beside it, as `alone()` reads. nextest takes a test's slots
    from the first override that sets them, so such an override is refused
    after one that sets slots otherwise, which would decide first for the
    tests both pick, and so is one that names a platform, which would hold on
    that platform alone, or whose filter `selector()` cannot read, since
    `alone()` reads it so. That puts no test in a group of its own either:
    `rerun()` runs such a case in a run of its own, with no other rerun going.
    """
    try:
        source = open(path, "rb")
    except FileNotFoundError:
        return {}
    with source:
        config = tomllib.load(source)
    picked = {}
    for name, profile in config.get("profile", {}).items():
        if "test-threads" in profile:
            raise ValueError(f"profile {name} sets test-threads")
        slots = profile.get("threads-required", 1)
        if type(slots) is not int or slots < 1 or (slots != 1 and name != GATE_PROFILE):
            raise ValueError(f"profile {name} sets threads-required")
        decided = False
        for override in profile.get("overrides", []):
            slots = override.get("threads-required", 1)
            if slots == ALONE and name == GATE_PROFILE:
                if decided:
                    raise ValueError(
                        f"an override of profile {name} runs tests alone after one that sets their slots otherwise"
                    )
                if "platform" in override:
                    raise ValueError(f"an override of profile {name} runs tests alone on one platform")
                if "filter" in override:
                    selector(override["filter"])
            elif slots != 1:
                raise ValueError(f"an override of profile {name} sets threads-required")
            elif "threads-required" in override:
                decided = True
            if "test-group" in override:
                picks = selector(override["filter"]) if "filter" in override else lambda *test: True
                picked.setdefault(override["test-group"], []).append(picks)
    return {
        group: (lambda *test, picks=picks: any(pick(*test) for pick in picks))
        for group, picks in picked.items()
    }


def alone(path):
    """The tests the gate's profile runs with no other beside them, as a predicate on (package, binary, test name).

    Those are the tests an override of that profile gives every slot,
    `threads-required = "num-test-threads"`. `confining()` refuses such an
    override where nextest might not apply it to every test it picks, so ask
    this only of a file `test_groups()` has read. A checkout with no nextest
    configuration runs no test alone.
    """
    try:
        source = open(path, "rb")
    except FileNotFoundError:
        return lambda *test: False
    with source:
        overrides = tomllib.load(source).get("profile", {}).get(GATE_PROFILE, {}).get("overrides", [])
    picks = [
        selector(override["filter"]) if "filter" in override else (lambda *test: True)
        for override in overrides
        if override.get("threads-required") == ALONE
    ]
    return lambda *test: any(pick(*test) for pick in picks)


def package_root(package):
    """Where a package's source is, from the workspace's root: the root package's is the whole tree."""
    return "." if package == "crucible-code" else os.path.join("crates", package)


def inventories(built, listing=listed, width=processors):
    """What `listing()` says of every built binary, as many asked at once as `width()` allows."""
    keys = sorted(built)
    with ThreadPoolExecutor(max_workers=width()) as pool:
        return dict(zip(keys, pool.map(lambda key: listing(built[key].executable), keys)))


def timed(inventory, sources, picks):
    """Say which tests the gate's profile runs alone without asserting `SOON_REFUSED`, or the reverse.

    A test asserting that bound can miss it while a test that keeps a
    processor busy runs beside it, so the profile runs each one alone, and
    picks them by name. A test asserts it here when its own text names it, as
    `function_region()` reads its definition, so a helper that asserts it for
    a test is not seen. Every discovered test is looked up by the last part of
    its name in `sources`, the definitions below each package's root, which
    this fills. One whose name resolves to no definition or to several, and
    that is run alone or whose definitions name the bound, fails, as does one
    defined in a file naming the bound whose end cannot be found.
    """
    marked = {}
    failed = 0
    for (package, kind, target), (discovered, _) in sorted(inventory.items()):
        root = package_root(package)
        if root not in sources:
            sources[root] = definitions(root)
        for name in sorted(discovered):
            found = sources[root].get(name.split("::")[-1], [])
            bounded = False
            unread = None
            for path, index, indent in found:
                if path not in marked:
                    text = open(path, encoding="utf-8").read()
                    marked[path] = text.splitlines() if TIMED.search(text) else None
                if marked[path] is None:
                    continue
                region = function_region(marked[path], index, indent)
                if region is None:
                    unread = path
                elif TIMED.search(region):
                    bounded = True
            chosen = picks(package, target, name)
            where = f"{name} in {package} {kind} {target}"
            if unread is not None:
                print(
                    f"    FAIL {where} has no closing brace in its own column in {unread}, so whether it"
                    " asserts SOON_REFUSED cannot be read"
                )
            elif len(found) != 1 and (bounded or chosen):
                places = ", ".join(one[0] for one in found) or "nowhere"
                print(
                    f"    FAIL {where} resolves to {len(found)} definitions ({places}), so whether it"
                    " asserts SOON_REFUSED cannot be read; define it once, by that name, in its package's source"
                )
            elif bounded and not chosen:
                print(
                    f"    FAIL {where} asserts the SOON_REFUSED wall-clock bound, but {NEXTEST}'s"
                    f" {GATE_PROFILE} profile runs it beside other tests; name it in the override that"
                    f" sets threads-required = '{ALONE}'"
                )
            elif chosen and not bounded:
                print(
                    f"    FAIL {where} is run alone by {NEXTEST}'s {GATE_PROFILE} profile, but its text"
                    f" names no SOON_REFUSED; take it out of the override that sets threads-required = '{ALONE}'"
                )
            else:
                continue
            failed = 1
    return failed


def test_groups(path):
    """The test groups `confining()` reads there, or None once it has said why it cannot."""
    try:
        return confining(path)
    except (tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        print(f"    FAIL {path} is not TOML ({error}); nextest cannot read it either, so mend the file")
    except ValueError as error:
        print(
            f"    FAIL {path} does not read as test groups here ({error});"
            " teach confining() and selector() what it says, so reruns still take nextest's turns"
        )
    return None


def gate_threads(path, count):
    """How many slots the gate's nextest run has on `count` processors, or None once it has said why it cannot say.

    That is as many per processor as the gate's profile gives every test.
    `confining()` reads those slots as putting no test in a group of its own
    only because the run has this many, so the product is taken here, by the
    same reader.
    """
    if test_groups(path) is None:
        return None
    try:
        source = open(path, "rb")
    except FileNotFoundError:
        return count
    with source:
        profile = tomllib.load(source).get("profile", {}).get(GATE_PROFILE, {})
    return count * profile.get("threads-required", 1)


def print_gate_threads(path, count):
    """`--gate-threads <processors>`: print the slots alone on a line and return 0, or return 1.

    `count` is how many processors nextest runs with, which `scripts/sh/rust-checks.sh`
    takes from Rust's `available_parallelism` as nextest does: under a CPU quota
    `processors()` here counts more. On 1, this or `gate_threads()` has printed
    why, which `scripts/sh/rust-checks.sh` shows above its own refusal.
    """
    if not re.fullmatch(r"[1-9][0-9]*", count):
        print(
            "    FAIL --gate-threads takes how many processors nextest counts, a whole number"
            f" of 1 or more, not {count!r}"
        )
        return 1
    threads = gate_threads(path, int(count))
    if threads is None:
        return 1
    print(threads)
    return 0


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


def rerun(built, run, groups, lone, ran=ran, width=processors):
    """Rerun every binary's required cases as `schedule()` allows, and say which did not all pass.

    `lone(package, target, case)` picks, as `alone()` does, the cases the
    gate's profile runs with no other test beside them. Each is rerun in a run
    of its own, since a binary runs every case it is asked for at once, and
    with no other rerun going: a binary beside it that keeps a processor busy
    can make it miss its bound as surely as a test beside it can under
    nextest. The other cases are rerun with no more binaries at once than
    `width()` processors, which stands in for the gate's nextest run rather
    than matching it, since each binary runs its own cases on threads of its
    own. A binary's cases all pass only when each run passed exactly the cases
    it asked for. Every line is printed in one order once all have answered,
    so the output does not depend on which finished first.
    """
    reruns = {key: [name for _, name, _ in cases] for key, cases in run.items() if key[1] != "doc"}
    passed = dict.fromkeys(reruns, 0)
    rest = {}
    for key, names in sorted(reruns.items()):
        package, _, target = key
        for name in names:
            if not lone(package, target, name):
                rest.setdefault(key, []).append(name)
            elif ran(built[key], [name]) == 1:
                passed[key] += 1
    with ThreadPoolExecutor(max_workers=width()) as pool:
        for job in pool.map(
            lambda keys: [(key, ran(built[key], rest[key])) for key in keys],
            schedule(rest, groups),
        ):
            for key, answer in job:
                if answer == len(rest[key]):
                    passed[key] += answer
    failed = 0
    for key, names in sorted(reruns.items()):
        package, kind, target = key
        if passed[key] != len(names):
            print(
                f"    FAIL {len(names)} required cases in {package} {kind} {target} did not all pass;"
                " rerun that target and read the assertion"
            )
            failed = 1
    return failed


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
    groups = test_groups(NEXTEST)
    if groups is None:
        return 1
    documented = doctests(listing)
    ignored_examples = doctests(silenced)

    run = {}
    inventory = inventories(built)
    sources = {}
    failed = timed(inventory, sources, alone(NEXTEST))
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
            discovered, ignored = inventory[key]
            if name not in discovered:
                print(f"    FAIL {case['id']} is gone: {name} was not discovered in {case['package']}")
                failed = 1
                continue
            if name in ignored:
                print(f"    FAIL {case['id']} is discovered but does not run: {name} is ignored")
                failed = 1
                continue

            root = package_root(case["package"])
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

    if rerun(built, run, groups, alone(NEXTEST)):
        failed = 1
    return failed


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) == 3 and argv[1] == "--gate-threads":
        return print_gate_threads(NEXTEST, argv[2])
    if len(argv) < 5:
        print(__doc__)
        return 2
    return self_test() or check(argv[1], argv[2], argv[3], argv[4:])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
