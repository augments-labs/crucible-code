#!/usr/bin/env python3
"""Whether a crate's manifest ships another crate, not only tests with it.

    shipped-edge.py MANIFEST CRATE
    shipped-edge.py --self-test

Exits 0 when the package that MANIFEST declares takes CRATE as a normal or a
build dependency, 3 when it takes it only for tests or not at all, and 2 when
that could not be answered. The third answer is separate because a check that
cannot tell "no edge" from "never looked" gives the reassuring one, and "no
edge" is 3 because 1 is what Python exits with when it crashes. `--self-test`
exits 0 when every manifest and description whose answer is known gives it,
and 1 otherwise.

Cargo is asked rather than the TOML read here. One dependency can be spelled
many ways — quoted or bare, dotted or a table of its own, under a `[target]`
prefix, renamed under a key that can be anything, or renamed in the workspace
table a member inherits it from — and Cargo is the one reader that resolves
every spelling to the package it names. `--no-deps` keeps the question to the
manifests: nothing is resolved or fetched, though every path dependency still
has to be a package Cargo can load.
"""

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile

SHIPPED_EDGE, UNANSWERED, NO_SHIPPED_EDGE = 0, 2, 3

# Whether each dependency kind Cargo writes is in what ships; a normal
# dependency is written with a null kind.
SHIPPED = {None: True, "build": True, "dev": False}


def metadata(manifest):
    """What Cargo says about the workspace the manifest is in, or None."""
    try:
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
                manifest,
            ],
            capture_output=True,
            encoding="utf-8",
            check=False,
        )
    except OSError as error:
        print(f"cargo could not be run: {error}", file=sys.stderr)
        return None
    if result.returncode != 0:
        sys.stderr.write(result.stderr)
        return None
    return result.stdout


def edge(described, manifest, crate):
    """The answer that description gives for the package this manifest declares."""
    # The metadata describes the whole workspace the manifest belongs to, so an
    # edge another member takes is not this one's.
    wanted = os.path.realpath(manifest)
    try:
        declared = next(
            (
                package
                for package in json.loads(described)["packages"]
                if os.path.realpath(package["manifest_path"]) == wanted
            ),
            None,
        )
        if declared is None:
            print(f"{manifest} declares no package of its own", file=sys.stderr)
            return UNANSWERED
        kinds = [dependency["kind"] for dependency in declared["dependencies"] if dependency["name"] == crate]
        return SHIPPED_EDGE if any(SHIPPED[kind] for kind in kinds) else NO_SHIPPED_EDGE
    except (ValueError, KeyError, TypeError) as error:
        print(f"cargo described {manifest} in a shape this does not read: {error!r}", file=sys.stderr)
        return UNANSWERED


def answer(manifest, crate):
    described = metadata(manifest)
    return UNANSWERED if described is None else edge(described, manifest, crate)


def package(name, body=""):
    return f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2024"\n{body}'


EDGE = 'crucible-sandbox-local = { path = "dep" }\n'
RENAMED = 'backend = { package = "crucible-sandbox-local", path = "dep" }\n'
DEP = package("crucible-sandbox-local")


def alone(body):
    return {"Cargo.toml": "[workspace]\n" + package("a", body), "dep/Cargo.toml": DEP}


# Each case is a workspace of its own, with the manifest asked about under
# `ask` when it is not the root one, and the answer Cargo's reading of it has
# to produce. Every path dependency is there with a target of its own, because
# Cargo refuses to load one without.
CASES = [
    ("a normal dependency", alone("[dependencies]\n" + EDGE), SHIPPED_EDGE),
    ("a build dependency", alone("[build-dependencies]\n" + EDGE), SHIPPED_EDGE),
    ("a dependency for one target", alone("[target.'cfg(unix)'.dependencies]\n" + EDGE), SHIPPED_EDGE),
    ("a dependency renamed under another key", alone("[dependencies]\n" + RENAMED), SHIPPED_EDGE),
    (
        "a dependency renamed in the workspace table it is inherited from",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n[workspace.dependencies]\n' + RENAMED,
            "dep/Cargo.toml": DEP,
            "member/Cargo.toml": package("member", "[dependencies]\nbackend.workspace = true\n"),
            "ask": "member/Cargo.toml",
        },
        SHIPPED_EDGE,
    ),
    (
        "a dependency taken for tests and shipped as well",
        alone("[dependencies]\n" + EDGE + "[dev-dependencies]\n" + EDGE),
        SHIPPED_EDGE,
    ),
    ("a dev-dependency", alone("[dev-dependencies]\n" + EDGE), NO_SHIPPED_EDGE),
    ("a renamed dev-dependency", alone("[dev-dependencies]\n" + RENAMED), NO_SHIPPED_EDGE),
    (
        "a key spelling the crate over another package",
        {
            "Cargo.toml": "[workspace]\n"
            + package("a", '[dependencies]\ncrucible-sandbox-local = { package = "other", path = "other" }\n'),
            "other/Cargo.toml": package("other"),
        },
        NO_SHIPPED_EDGE,
    ),
    (
        "another member of the workspace taking it",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n' + package("a"),
            "member/Cargo.toml": package("member", "[dependencies]\n" + EDGE),
            "member/dep/Cargo.toml": DEP,
        },
        NO_SHIPPED_EDGE,
    ),
    ("no dependencies at all", alone(""), NO_SHIPPED_EDGE),
    ("a manifest that is not there", {"ask": "absent/Cargo.toml"}, UNANSWERED),
    ("a manifest that is not TOML", alone("[dependencies\n"), UNANSWERED),
    (
        "a workspace manifest that declares no package",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n',
            "member/Cargo.toml": package("member", "[dependencies]\n" + EDGE),
            "member/dep/Cargo.toml": DEP,
        },
        UNANSWERED,
    ),
]


def described(kind="dev", name="crucible-sandbox-local"):
    dependency = {"name": name, "kind": kind}
    return json.dumps({"packages": [{"manifest_path": "/fixture/Cargo.toml", "dependencies": [dependency]}]})


# A description no manifest makes Cargo write, and the answer it has to produce
# all the same: an unread shape is never a clean manifest.
SHAPES = [
    ("a description that is not JSON", "not JSON", UNANSWERED),
    ("a dependency of a kind this does not know", described(kind="optional"), UNANSWERED),
    ("a dependency with no name", described().replace('"name"', '"called"'), UNANSWERED),
]


def known(name, expected, question):
    """Whether a question gets the answer known for it, saying what it said if not."""
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        got = question()
    if got != expected:
        print(f"shipped-edge: {name} answered {got}, not {expected}")
        print(said.getvalue(), end="")
    return got == expected


def self_test():
    wrong = 0
    for name, files, expected in CASES:
        with tempfile.TemporaryDirectory() as temporary:
            root = os.path.join(temporary, "workspace")
            for path, text in files.items():
                if path == "ask":
                    continue
                folder = os.path.join(root, os.path.dirname(path))
                os.makedirs(os.path.join(folder, "src"), exist_ok=True)
                with open(os.path.join(folder, "src", "lib.rs"), "w", encoding="utf-8"):
                    pass
                with open(os.path.join(root, path), "w", encoding="utf-8") as manifest:
                    manifest.write(text)
            # Asked relative, as the gate asks, and through a link, as a checkout
            # can be reached: Cargo answers with an absolute path it does not
            # resolve, so neither side of the comparison is canonical until made so.
            os.makedirs(root, exist_ok=True)
            os.symlink(root, os.path.join(temporary, "link"))
            asked = os.path.relpath(os.path.join(temporary, "link", files.get("ask", "Cargo.toml")))
            if not known(name, expected, lambda: answer(asked, "crucible-sandbox-local")):
                wrong = 1
    for name, text, expected in SHAPES:
        if not known(name, expected, lambda: edge(text, "/fixture/Cargo.toml", "crucible-sandbox-local")):
            wrong = 1
    return wrong


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return UNANSWERED
    return answer(argv[1], argv[2])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
