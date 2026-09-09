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
there: `compile_fail,E0277` names the error the example is about, while a bare
`compile_fail` passes for any compile error at all, so dropping the code is a
weakening that leaves the example listed and green.

Platform-specific cases are pending on the platforms that cannot run them, not
skipped: each supported platform's own run enforces its own rows.

    scripts/python/required-cases.py <artifacts> <doc-list> <doc-ignored-list>
    scripts/python/required-cases.py --self-test       prove the source reader reads
"""

import hashlib
import json
import os
import re
import subprocess
import sys

MANIFEST = "scripts/required-cases.json"

PLATFORM = {"linux": "linux", "darwin": "macos", "win32": "windows"}.get(
    sys.platform, sys.platform
)

DOCTEST = re.compile(r"^(?P<source>.+?) - (?P<case>\S+) \(line (?P<line>\d+)\): test$")


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


def find_function(root, name):
    """Every place `name` is defined below `root`, so an ambiguity can be told."""
    opening = re.compile(
        r"^(\s*)(?:pub\s+)?(?:async\s+)?fn\s+" + re.escape(name) + r"\s*[(<]"
    )
    found = []
    for base, directories, files in os.walk(root):
        directories[:] = [one for one in directories if one not in ("target", ".git")]
        for file in files:
            if not file.endswith(".rs"):
                continue
            path = os.path.join(base, file)
            lines = open(path, encoding="utf-8").read().splitlines()
            for index, line in enumerate(lines):
                match = opening.match(line)
                if match:
                    found.append((slashed(path), index, match.group(1), lines))
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
    return 0


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
        built[(package, target["kind"][0], target["name"])] = message["executable"]
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


def selector(target):
    return {
        "lib": ["--lib"],
        "bin": ["--bin", target["name"]],
        "test": ["--test", target["name"]],
        "doc": ["--doc"],
    }[target["kind"]]


def ran(package, arguments, filters):
    """How many cases passed when cargo was asked for exactly these."""
    result = subprocess.run(
        ["cargo", "test", "--locked", "-p", package, *arguments, "--", *filters],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return -1
    return sum(
        int(line.split()[3]) for line in result.stdout.splitlines() if line.startswith("test result:")
    )


def check(stream, listing, silenced):
    manifest = json.load(open(MANIFEST, encoding="utf-8"))["cases"]
    if not manifest:
        print(f"    FAIL {MANIFEST} names no required case; this check measured nothing")
        return 1
    built = executables(stream)
    if not built:
        print("    FAIL the test selection built no test binary; this check measured nothing")
        return 1
    documented = doctests(listing)
    ignored_examples = doctests(silenced)

    failed = 0
    run = {}
    inventory = {}
    for case in manifest:
        kind = case["target"]["kind"]
        key = (case["package"], kind, case["target"]["name"])
        name = case["case"]
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
                print(f"    FAIL {case['id']} resolves to {len(where)} examples ({places}); name it uniquely")
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
                inventory[key] = listed(built[key])
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
            found = find_function(root, name.split("::")[-1])
            if len(found) != 1:
                where = ", ".join(one[0] for one in found) or "nowhere"
                print(f"    FAIL {case['id']} resolves to {len(found)} definitions ({where}); name it uniquely")
                failed = 1
                continue
            path, index, indent, lines = found[0]
            if path != case["source"]:
                print(f"    FAIL {case['id']} moved to {path}; update its source in {MANIFEST}")
                failed = 1
                continue
            region = function_region(lines, index, indent)
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
        run.setdefault(key, []).append((case, name))

    for key, cases in sorted(run.items()):
        package, kind, target = key
        arguments = selector({"kind": kind, "name": target})
        if kind == "doc":
            # rustdoc gives an example a name ending in what the example is for,
            # which `--list` does not print, so each runs under its listed name
            # as a filter and has to be the one thing that matched. One at a
            # time, so a failure can say which obligation stopped holding.
            for case, name in cases:
                if ran(package, arguments, [name]) != 1:
                    print(f"    FAIL {case['id']} no longer holds: {name} did not run and pass on its own")
                    print(f"         {case['obligation']}")
                    failed = 1
            continue
        names = [name for _, name in cases]
        if ran(package, arguments, ["--exact", *names]) != len(names):
            print(
                f"    FAIL {len(names)} required cases in {package} {kind} {target} did not all pass;"
                " rerun that target and read the assertion"
            )
            failed = 1

    return failed


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) != 4:
        print(__doc__)
        return 2
    return self_test() or check(argv[1], argv[2], argv[3])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
