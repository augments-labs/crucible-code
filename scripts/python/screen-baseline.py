#!/usr/bin/env python3
"""The terminal screens a change is not supposed to redraw.

The whole-screen suite compares each capture against an accepted picture, so it
catches a screen that moved. It cannot catch a screen that moved together with
the picture of it: a layout constant changes, the suite is asked to accept the
new drawing, and the accepted file now agrees with the code that broke it.
Nothing in the suite can tell that apart from a screen that never changed.

This gate names each accepted picture in `scripts/screen-baseline.json` with the
things a reader would need to know it is still the same capture:

    present     every accepted picture is on disk and every one on disk is named
    unmoved     the picture still hashes to the recorded value
    keyed       the case that draws it still opens the same window and is still
                answered by the same scripted vendor
    read        the harness that turns a terminal into that text is unchanged

The key is read out of the case rather than written beside it: the
`Watched::` call says which case name, how many columns and rows, and which
constructor — and so which theme and which permissions — the window was opened
with, and the `Vendor::` call says what it was answered with. A capture taken
at a different width, on a different theme or against a different script is a
different observation wearing the same name, and the recorded call text is what
notices.

Accepting a redrawn screen is an edit here as well as to the picture, and the
reviewer is agreeing that the screen should look different — not that the suite
went green again.

    scripts/python/screen-baseline.py              check the tree against the manifest
    scripts/python/screen-baseline.py --record     rewrite the manifest from the tree
    scripts/python/screen-baseline.py --self-test  prove the source reader reads
"""

import hashlib
import json
import os
import re
import sys

MANIFEST = "scripts/screen-baseline.json"
SUITE = "tests/whole_screen"
CAPTURES = os.path.join(SUITE, "snapshots")
CASES = os.path.join(SUITE, "main.rs")

# The harness, and not the cases. `main.rs` holds every case body, so hashing it
# would put a new case in conflict with every accepted picture for no reason a
# reader could act on; what a case asserts is already bound one case at a time
# in `scripts/required-cases.json`. These three are the interpreter: they turn a
# pseudo-terminal into the text below, and a change in them can make an
# unchanged picture mean something else.
HARNESS = [
    os.path.join(SUITE, "screen.rs"),
    os.path.join(SUITE, "vendor.rs"),
    os.path.join(SUITE, "watched.rs"),
]

OPENING = re.compile(r"^(\s*)(?:pub\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)\s*[(<]")
KEYED = re.compile(r"\b(?:Watched|Vendor)::")


def slashed(path):
    """One spelling of a path, so a manifest reads the same on every platform."""
    return path.replace(os.sep, "/")


def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def functions(lines):
    """Each function in `lines`, as a name and the lines its body spans."""
    found = {}
    for index, line in enumerate(lines):
        match = OPENING.match(line)
        if not match:
            continue
        indent, name = match.group(1), match.group(2)
        if line.rstrip().endswith("}"):
            # A body written on the signature's own line, which has no closing
            # line of its own to look for.
            found.setdefault(name, []).append([line])
            continue
        closing = indent + "}"
        for end in range(index + 1, len(lines)):
            if lines[end] == closing:
                found.setdefault(name, []).append(lines[index : end + 1])
                break
    return found


def constructions(body):
    """Every `Watched::`/`Vendor::` call in `body`, on one line each.

    A call is taken from its type name to the parenthesis that closes it, so a
    width rustfmt happened to wrap onto its own line still reads as part of the
    call it belongs to.
    """
    text = "\n".join(body)
    found = []
    for match in KEYED.finditer(text):
        start = match.start()
        depth = 0
        for index in range(start, len(text)):
            if text[index] == "(":
                depth += 1
            elif text[index] == ")":
                depth -= 1
                if depth == 0:
                    found.append(" ".join(text[start : index + 1].split()))
                    break
    return found


def owner(name, cases):
    """The case that draws the capture called `name`.

    Most captures carry their case's own name. The few that are asked for by
    hand, or built from a width, are found by the text the case names them with.
    """
    if name in cases and len(cases[name]) == 1:
        return name, cases[name][0]
    stem = name
    matched = []
    for case, bodies in cases.items():
        if len(bodies) != 1:
            continue
        text = "\n".join(bodies[0])
        for quoted in re.findall(r'assert_snapshot!\(\s*(?:format!\()?"([^"]*)"', text):
            # A name built from a width is written as a format string, so only
            # the part before the first hole is text the file name will carry.
            literal = quoted.split("{")[0]
            if stem == quoted or (literal and stem.startswith(literal)):
                matched.append((case, bodies[0]))
                break
    if len(matched) == 1:
        return matched[0]
    return None, None


def observed():
    """What the tree says, in the shape the manifest records."""
    lines = open(CASES, encoding="utf-8").read().splitlines()
    cases = functions(lines)
    captures = []
    unowned = []
    for file in sorted(os.listdir(CAPTURES)):
        if not file.endswith(".snap"):
            continue
        name = file[: -len(".snap")]
        name = name.split("__", 1)[1] if "__" in name else name
        case, body = owner(name, cases)
        if case is None:
            unowned.append(file)
            continue
        path = os.path.join(CAPTURES, file)
        captures.append(
            {
                "capture": slashed(path),
                "case": case,
                "opened": constructions(body),
                "sha256": digest(open(path, encoding="utf-8").read()),
            }
        )
    harness = {
        slashed(path): digest(open(path, encoding="utf-8").read()) for path in HARNESS
    }
    return harness, captures, unowned


def check():
    if not os.path.isdir(CAPTURES):
        print(f"    FAIL {CAPTURES} is not there; no accepted screen was read")
        return 1
    if not os.path.exists(MANIFEST):
        print(f"    FAIL {MANIFEST} is not there; record it before accepting a screen")
        return 1
    manifest = json.load(open(MANIFEST, encoding="utf-8"))
    if not manifest["captures"]:
        print(f"    FAIL {MANIFEST} names no capture; it would pass over any screen")
        return 1

    harness, captures, unowned = observed()
    failed = 0
    for file in unowned:
        print(f"    FAIL {file} belongs to no case in {CASES}; name its case")
        failed = 1

    for path, recorded in manifest["harness"].items():
        if path not in harness:
            print(f"    FAIL {path} is gone; the accepted screens were read by it")
            failed = 1
        elif harness[path] != recorded:
            print(f"    FAIL {path} changed how a screen is read")
            print(f"         re-read the accepted screens, then record {harness[path]}")
            failed = 1
    for path in harness:
        if path not in manifest["harness"]:
            print(f"    FAIL {path} reads the screens and is not recorded in {MANIFEST}")
            failed = 1

    accepted = {one["capture"]: one for one in manifest["captures"]}
    seen = {one["capture"]: one for one in captures}
    for path in sorted(set(accepted) - set(seen)):
        print(f"    FAIL {path} is accepted in {MANIFEST} and not on disk")
        failed = 1
    for path in sorted(set(seen) - set(accepted)):
        print(f"    FAIL {path} is on disk and accepted nowhere in {MANIFEST}")
        failed = 1

    for path in sorted(set(accepted) & set(seen)):
        was, now = accepted[path], seen[path]
        if was["case"] != now["case"]:
            print(f"    FAIL {path} is now drawn by {now['case']}, not {was['case']}")
            failed = 1
            continue
        if was["opened"] != now["opened"]:
            print(f"    FAIL {now['case']} no longer opens the window it was captured in")
            for line in was["opened"]:
                print(f"         was  {line}")
            for line in now["opened"]:
                print(f"         now  {line}")
            failed = 1
        if was["sha256"] != now["sha256"]:
            print(f"    FAIL {path} is not the screen that was accepted")
            print(f"         read it as a terminal, then record {now['sha256']}")
            failed = 1
    return failed


def record():
    harness, captures, unowned = observed()
    for file in unowned:
        print(f"    FAIL {file} belongs to no case in {CASES}; name its case")
    if unowned:
        return 1
    with open(MANIFEST, "w", encoding="utf-8") as out:
        json.dump({"harness": harness, "captures": captures}, out, indent=2)
        out.write("\n")
    print(f"    recorded {len(captures)} screens in {MANIFEST}")
    return 0


def self_test():
    """A reader that reads nothing keys every screen the same way."""
    source = [
        "#[test]",
        "fn a_drawn_case() {",
        '    let vendor = Vendor::answering("hello");',
        "    let mut window = Watched::open(",
        '        "a-case",',
        "        80,",
        "        24,",
        "    );",
        "    insta::assert_snapshot!(window.picture());",
        "}",
        "",
        "fn beside_it() {}",
    ]
    cases = functions(source)
    if sorted(cases) != ["a_drawn_case", "beside_it"]:
        print(f"    FAIL the reader found {sorted(cases)} rather than both functions")
        return 1
    found = constructions(cases["a_drawn_case"][0])
    expected = ['Vendor::answering("hello")', 'Watched::open( "a-case", 80, 24, )']
    if found != expected:
        print(f"    FAIL the reader keyed the case as {found}")
        return 1
    return 0


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) == 2 and argv[1] == "--record":
        return self_test() or record()
    if len(argv) != 1:
        print(__doc__)
        return 2
    return self_test() or check()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
