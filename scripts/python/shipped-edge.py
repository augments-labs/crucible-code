#!/usr/bin/env python3
"""Whether a crate's manifest ships another crate, not only tests with it.

    shipped-edge.py MANIFEST CRATE
    shipped-edge.py --self-test

Exits 0 when the package MANIFEST declares takes CRATE as a normal or a build
dependency, 1 when it takes it only for tests or not at all, and 2 when that
could not be answered. The third answer is separate because a check that cannot
tell "no edge" from "never looked" gives the reassuring one.

Cargo is asked rather than the TOML read here. One dependency can be spelled
many ways — quoted or bare, dotted or a table of its own, under a `[target]`
prefix, renamed under a key that can be anything, or renamed in the workspace
table a member inherits it from — and Cargo is the one reader that resolves
every spelling to the package it names. `--no-deps` keeps the question to the
manifests: nothing is resolved or fetched, though a path dependency still has to
be there to be read.
"""

import json
import os
import subprocess
import sys
import tempfile

# Whether each dependency kind Cargo writes is in what ships; a normal
# dependency is written with no kind at all.
SHIPPED = {None: True, "build": True, "dev": False}


def metadata(manifest, quiet):
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
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL if quiet else None,
            text=True,
            check=False,
        )
    except OSError as error:
        if not quiet:
            print(f"cargo could not be run: {error}", file=sys.stderr)
        return None
    return result.stdout if result.returncode == 0 else None


def edge(described, manifest, crate, quiet=False):
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
            if not quiet:
                print(f"{manifest} declares no package of its own", file=sys.stderr)
            return 2
        kinds = [dependency["kind"] for dependency in declared["dependencies"] if dependency["name"] == crate]
        return 0 if any(SHIPPED[kind] for kind in kinds) else 1
    except (ValueError, KeyError, TypeError) as error:
        if not quiet:
            print(f"cargo described {manifest} in a shape this does not read: {error!r}", file=sys.stderr)
        return 2


def answer(manifest, crate, quiet=False):
    described = metadata(manifest, quiet)
    return 2 if described is None else edge(described, manifest, crate, quiet)


def package(name, body=""):
    return f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2024"\n{body}'


EDGE = 'crucible-sandbox-local = { path = "dep" }\n'
RENAMED = 'backend = { package = "crucible-sandbox-local", path = "dep" }\n'
DEP = package("crucible-sandbox-local")


def alone(body):
    return {"Cargo.toml": "[workspace]\n" + package("a", body), "dep/Cargo.toml": DEP}


# Each case is a workspace of its own, with the manifest asked about under
# `ask` when it is not the root one, and the answer Cargo's reading of it has
# to produce. Every path dependency is there, because Cargo reads it.
CASES = [
    ("a normal dependency", alone("[dependencies]\n" + EDGE), 0),
    ("a build dependency", alone("[build-dependencies]\n" + EDGE), 0),
    ("a dependency for one target", alone("[target.'cfg(unix)'.dependencies]\n" + EDGE), 0),
    ("a dependency renamed under another key", alone("[dependencies]\n" + RENAMED), 0),
    (
        "a dependency renamed in the workspace table it is inherited from",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n[workspace.dependencies]\n' + RENAMED,
            "dep/Cargo.toml": DEP,
            "member/Cargo.toml": package("member", "[dependencies]\nbackend.workspace = true\n"),
            "ask": "member/Cargo.toml",
        },
        0,
    ),
    (
        "a dependency taken for tests and shipped as well",
        alone("[dependencies]\n" + EDGE + "[dev-dependencies]\n" + EDGE),
        0,
    ),
    ("a dev-dependency", alone("[dev-dependencies]\n" + EDGE), 1),
    ("a renamed dev-dependency", alone("[dev-dependencies]\n" + RENAMED), 1),
    (
        "a key spelling the crate over another package",
        {
            "Cargo.toml": "[workspace]\n"
            + package("a", '[dependencies]\ncrucible-sandbox-local = { package = "other", path = "other" }\n'),
            "other/Cargo.toml": package("other"),
        },
        1,
    ),
    (
        "another member of the workspace taking it",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n' + package("a"),
            "member/Cargo.toml": package("member", "[dependencies]\n" + EDGE),
            "member/dep/Cargo.toml": DEP,
        },
        1,
    ),
    ("no dependencies at all", alone(""), 1),
    ("a manifest that is not there", {"ask": "absent/Cargo.toml"}, 2),
    ("a manifest that is not TOML", alone("[dependencies\n"), 2),
    (
        "a workspace manifest that declares no package",
        {
            "Cargo.toml": '[workspace]\nmembers = ["member"]\n',
            "member/Cargo.toml": package("member", "[dependencies]\n" + EDGE),
            "member/dep/Cargo.toml": DEP,
        },
        2,
    ),
]


def described(kind="dev", name="crucible-sandbox-local"):
    dependency = {"name": name, "kind": kind}
    return json.dumps({"packages": [{"manifest_path": "/fixture/Cargo.toml", "dependencies": [dependency]}]})


# A description no manifest makes Cargo write, and the answer it has to produce
# all the same: an unread shape is never a clean manifest.
SHAPES = [
    ("a description that is not JSON", "not JSON", 2),
    ("a dependency of a kind this does not know", described(kind="optional"), 2),
    ("a dependency with no name", described().replace('"name"', '"called"'), 2),
]


def self_test():
    wrong = 0
    for name, files, expected in CASES:
        with tempfile.TemporaryDirectory() as root:
            for path, text in files.items():
                if path == "ask":
                    continue
                folder = os.path.join(root, os.path.dirname(path))
                os.makedirs(os.path.join(folder, "src"), exist_ok=True)
                with open(os.path.join(folder, "src", "lib.rs"), "w", encoding="utf-8"):
                    pass
                with open(os.path.join(root, path), "w", encoding="utf-8") as manifest:
                    manifest.write(text)
            # Relative, as the gate asks, while Cargo answers with an absolute path.
            asked = os.path.relpath(os.path.join(root, files.get("ask", "Cargo.toml")))
            got = answer(asked, "crucible-sandbox-local", quiet=True)
        if got != expected:
            print(f"shipped-edge: {name} answered {got}, not {expected}")
            wrong = 1
    for name, text, expected in SHAPES:
        got = edge(text, "/fixture/Cargo.toml", "crucible-sandbox-local", quiet=True)
        if got != expected:
            print(f"shipped-edge: {name} answered {got}, not {expected}")
            wrong = 1
    return wrong


def main(argv):
    if len(argv) == 2 and argv[1] == "--self-test":
        return self_test()
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    return answer(argv[1], argv[2])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
