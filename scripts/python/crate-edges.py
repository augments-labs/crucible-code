#!/usr/bin/env python3
"""Which packages of a Cargo workspace take which of its packages.

    crate-edges.py WORKSPACE MEMBER...
    crate-edges.py --self-test

Prints `package dependency` once for each package of the workspace the manifest
WORKSPACE is in and each package of that workspace it takes, in any dependency
table, and exits 0. A dependency is on a package of the workspace when its path
is that package's directory, or when it has no path and carries that package's
name. Any other exit means the workspace was not read, and nothing printed is
its graph. That is the answer when Cargo cannot describe the workspace; when a
dependency's path holds no package of it; when the MEMBER manifests are not
exactly its packages, because a crate left out of the workspace is one whose
edges Cargo never describes, and a package not handed over is one the caller
did not expect; and when something could put a package in place of a
dependency, which Cargo follows only when resolving: `[patch]` or `[replace]` in
the workspace's `Cargo.toml`, or `[patch]`, `paths` or `[source]` in its
`.cargo/config.toml` or `.cargo/config`, or an `include` there of a file that
could declare one. Other Cargo configuration, in a parent directory, a member's
directory or Cargo's home, is not read, so a substitution there goes unseen.
`--self-test` exits 0 when Cargo, asked without each of the reader's two flags,
keeps quiet and colours its warning as the self-test's settings make it, and
every one of its workspaces is read as its case says; and 1 otherwise.

Cargo is asked which dependencies each package declares, rather than the tables
being read here. One dependency can be spelled many ways — a dotted key, an
inline table or a table of its own, under a `[target]` prefix, renamed under a
key that can be anything, or renamed in the workspace table a member inherits it
from — and a reader that misses one reports no edge where there is one.
`--no-deps` keeps the question to the manifests: nothing is resolved or fetched.
Whether a file declares `[patch]`, `[replace]`, `paths`, `[source]` or `include`
is one top-level key however it is spelled, so that much is read with `tomllib`.
"""

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import tomllib


class Unanswered(Exception):
    """Why the workspace a manifest is in could not be read."""


# `[patch]` and `[replace]` put a package in place of a dependency, as `paths` and
# `[source]` do in configuration, and Cargo follows them only when resolving;
# configuration can also `include` another file that declares one. These are the
# files under a workspace's root that can declare them, each with the ones it can.
CONFIGURATION = ("[patch]", "paths", "[source]", "include")
REDIRECTIONS = [
    ("Cargo.toml", ("[patch]", "[replace]")),
    (os.path.join(".cargo", "config.toml"), CONFIGURATION),
    (os.path.join(".cargo", "config"), CONFIGURATION),
]


def redirection(root):
    """Which file under the workspace root could take a dependency from elsewhere, and by what, or None."""
    for name, keys in REDIRECTIONS:
        path = os.path.join(root, name)
        try:
            with open(path, "rb") as source:
                declared = tomllib.load(source)
        except FileNotFoundError:
            continue
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise Unanswered(f"{path} could not be read: {error}") from error
        for key in keys:
            # A table is named here in brackets, and found by the key it sits under.
            if key.strip("[]") in declared:
                return f"{path} declares {key}"
    return None


def edges(workspace, members):
    """Each package of the manifest's workspace and each package of it that it takes, sorted."""
    try:
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--no-deps",
                "--offline",
                "--format-version",
                "1",
                "--color",
                "never",
                "--config",
                "term.quiet=false",
                "--manifest-path",
                workspace,
            ],
            capture_output=True,
            check=False,
        )
    except OSError as error:
        raise Unanswered(f"cargo could not be run: {error}") from error
    # What Cargo said reaches whoever reads the gate, whatever it answered.
    sys.stderr.write(result.stderr.decode("utf-8", errors="backslashreplace"))
    if result.returncode != 0:
        raise Unanswered(f"cargo exited {result.returncode} describing {workspace}")
    try:
        described = json.loads(result.stdout)
        root = described["workspace_root"]
        packages = described["packages"]
        manifests = {os.path.realpath(package["manifest_path"]): package["name"] for package in packages}
        directories = {os.path.dirname(manifest): name for manifest, name in manifests.items()}
        found = set()
        for package in packages:
            for dependency in package["dependencies"]:
                path = dependency.get("path")
                if path is not None:
                    # Named by what is at the path, not by what the dependency calls it.
                    taken = directories.get(os.path.realpath(path))
                    if taken is None:
                        raise Unanswered(f"{package['name']} takes {path} by path, which holds no package of the workspace")
                    found.add((package["name"], taken))
                elif dependency["name"] in directories.values():
                    # A dependency without a path that carries the name of a package of
                    # this workspace is counted as that package, although Cargo would
                    # take it from a registry or a repository. Any other comes from
                    # there, and is not part of the layering.
                    found.add((package["name"], dependency["name"]))
    except (ValueError, KeyError, TypeError) as error:
        raise Unanswered(f"cargo described {workspace} in a shape this does not read: {error!r}") from error
    redirected = redirection(root)
    if redirected is not None:
        raise Unanswered(f"{redirected}, which can take a dependency from somewhere Cargo does not name without resolving")
    handed = {os.path.realpath(member) for member in members}
    unread = sorted(handed - manifests.keys())
    if unread:
        raise Unanswered(f"{', '.join(unread)}: not a package of the workspace, so its edges go unread")
    unasked = sorted(manifests.keys() - handed)
    if unasked:
        raise Unanswered(f"{', '.join(unasked)}: a package of the workspace that was not handed over")
    return sorted(found)


def package(name, body=""):
    return f'[package]\nname = "{name}"\nversion = "0.0.0"\nedition = "2024"\n{body}'


TAKES_B = 'b = { path = "b" }\n'
RENAMED = 'other = { package = "b", path = "b" }\n'
WITHHELD = "# Not handed over.\n"
# A root package `a` taking `helper` by version, and outside the workspace a
# `helper` that takes the member `b` by path, for the workspace to put in place.
HELPER = '[workspace]\nmembers = ["b"]\n' + package("a", '[dependencies]\nhelper = "0.0.0"\n')
OUTSIDE = {
    "b/Cargo.toml": package("b"),
    "../helper/Cargo.toml": package("helper", '[dependencies]\nb = { path = "../workspace/b" }\n'),
}
PATCH = '[patch.crates-io]\nhelper = { path = "../helper" }\n'
# A root that is no package and names no resolver, beside its one member: Cargo
# warns about it.
WARNS = {"Cargo.toml": '[workspace]\nmembers = ["a"]\n', "a/Cargo.toml": package("a")}


def declares(name, key):
    """The words a refusal says of `key` declared in the file `name` under the workspace root."""
    return f"{os.sep}{os.path.join(*name.split('/'))} declares {key}"


def alone(body, workspace=""):
    """A workspace whose root package `a` declares `body`, beside a package `b`."""
    return {"Cargo.toml": "[workspace]\n" + workspace + package("a", body), "b/Cargo.toml": package("b")}


# Each case is a workspace of its own, rooted at `workspace/` beside whatever
# else it names; the edges Cargo's reading of it has to give or, when it has to be
# refused, words its refusal has to say; and, for some, what Cargo has to be heard
# saying. It is asked through a link. Every manifest under the root that declares
# a package is handed over, unless it says it is not: the root one through the
# link, as the workspace is asked, and the rest directly, each relative to the
# working directory, as the gate names them. Every package is there with a target
# of its own, because Cargo refuses to load one without; a root that is no package
# names its resolver, unless the case needs Cargo to warn that it does not.
CASES = [
    (
        "a dotted key inherited from the workspace table",
        alone("[dependencies]\nb.workspace = true\n", "[workspace.dependencies]\n" + TAKES_B),
        ["a b"],
    ),
    ("an inline table", alone("[dependencies]\n" + TAKES_B), ["a b"]),
    ("a table of its own", alone('[dependencies.b]\npath = "b"\n'), ["a b"]),
    ("a dependency renamed under another key", alone("[dependencies]\n" + RENAMED), ["a b"]),
    (
        "a dependency renamed in the workspace table it is inherited from",
        alone("[dependencies]\nother.workspace = true\n", "[workspace.dependencies]\n" + RENAMED),
        ["a b"],
    ),
    ("a dependency for one target", alone("[target.'cfg(unix)'.dependencies]\n" + TAKES_B), ["a b"]),
    ("a dev-dependency", alone("[dev-dependencies]\n" + TAKES_B), ["a b"]),
    ("a build dependency", alone("[build-dependencies]\n" + TAKES_B), ["a b"]),
    (
        "an optional dependency a feature turns on",
        alone('[features]\nwith-b = ["dep:b"]\n[dependencies]\nb = { path = "b", optional = true }\n'),
        ["a b"],
    ),
    (
        "every member, under a root that is no package",
        {
            "Cargo.toml": '[workspace]\nmembers = ["a", "b"]\nresolver = "3"\n',
            "a/Cargo.toml": package("a", '[dependencies]\nb = { path = "../b" }\n'),
            "b/Cargo.toml": package("b", '[dependencies]\nc = { path = "../c" }\n'),
            "c/Cargo.toml": package("c"),
        },
        ["a b", "b c"],
    ),
    (
        "a dependency from a registry or a repository",
        alone(
            "[dependencies]\n"
            + TAKES_B
            + 'serde = "1"\nremote = { git = "https://example.invalid/remote.git" }\n'
        ),
        ["a b"],
    ),
    (
        "a dependency from a registry under the name of a package of the workspace",
        {
            "Cargo.toml": '[workspace]\nmembers = ["b"]\n' + package("a", '[dependencies]\nb = "0.0.0"\n'),
            "b/Cargo.toml": package("b"),
        },
        ["a b"],
    ),
    (
        "a dependency that names another package than the one at its path",
        {"Cargo.toml": "[workspace]\n" + package("a", '[dependencies]\nb = { path = "c" }\n'), "c/Cargo.toml": package("c")},
        ["a c"],
    ),
    ("what Cargo warns about on the way", WARNS, [], "warning:"),
    (
        "a package outside the workspace, under a member's name",
        {
            "Cargo.toml": "[workspace]\n"
            + package("a", "[dependencies]\n" + TAKES_B + 'shadow = { package = "b", path = "../outside" }\n'),
            "b/Cargo.toml": package("b"),
            "../outside/Cargo.toml": package("b"),
        },
        "by path, which holds no package of the workspace",
    ),
    (
        "a crate the workspace excludes",
        {
            "Cargo.toml": '[workspace]\nmembers = ["a"]\nexclude = ["b"]\n',
            "a/Cargo.toml": package("a"),
            "b/Cargo.toml": package("b", '[dependencies]\nc = { path = "../c" }\n'),
            "c/Cargo.toml": package("c"),
        },
        "not a package of the workspace, so its edges go unread",
        "warning:",
    ),
    (
        "a package of the workspace the caller does not hand over",
        {"Cargo.toml": "[workspace]\n" + package("a", "[dependencies]\n" + TAKES_B), "b/Cargo.toml": WITHHELD + package("b")},
        "a package of the workspace that was not handed over",
    ),
    ("a patch in the workspace manifest", {"Cargo.toml": HELPER + PATCH, **OUTSIDE}, declares("Cargo.toml", "[patch]")),
    (
        "a replacement in the workspace manifest",
        {"Cargo.toml": HELPER + '[replace]\n"helper:0.0.0" = { path = "../helper" }\n', **OUTSIDE},
        declares("Cargo.toml", "[replace]"),
    ),
    (
        "a patch in the workspace's Cargo configuration",
        {"Cargo.toml": HELPER, ".cargo/config.toml": PATCH, **OUTSIDE},
        declares(".cargo/config.toml", "[patch]"),
    ),
    (
        "a patch in Cargo configuration under its older name",
        {"Cargo.toml": HELPER, ".cargo/config": PATCH, **OUTSIDE},
        declares(".cargo/config", "[patch]"),
    ),
    (
        "a path override in the workspace's Cargo configuration",
        {"Cargo.toml": HELPER, ".cargo/config.toml": 'paths = ["../helper"]\n', **OUTSIDE},
        declares(".cargo/config.toml", "paths"),
    ),
    (
        "a source replacement in the workspace's Cargo configuration",
        {
            "Cargo.toml": HELPER,
            ".cargo/config.toml": '[source.crates-io]\nreplace-with = "vendored"\n\n[source.vendored]\ndirectory = "vendor"\n',
            **OUTSIDE,
        },
        declares(".cargo/config.toml", "[source]"),
    ),
    (
        "a path override in Cargo configuration under its older name",
        {"Cargo.toml": HELPER, ".cargo/config": 'paths = ["../helper"]\n', **OUTSIDE},
        declares(".cargo/config", "paths"),
    ),
    (
        "a source replacement in Cargo configuration under its older name",
        {
            "Cargo.toml": HELPER,
            ".cargo/config": '[source.crates-io]\nreplace-with = "vendored"\n\n[source.vendored]\ndirectory = "vendor"\n',
            **OUTSIDE,
        },
        declares(".cargo/config", "[source]"),
    ),
    (
        "a patch in a file the workspace's Cargo configuration includes",
        {
            "Cargo.toml": HELPER,
            ".cargo/config.toml": 'include = ["patch.toml"]\n',
            ".cargo/patch.toml": PATCH,
            **OUTSIDE,
        },
        declares(".cargo/config.toml", "include"),
    ),
    (
        "a file the workspace's Cargo configuration includes, patching nothing",
        {
            "Cargo.toml": HELPER,
            ".cargo/config.toml": 'include = ["term.toml"]\n',
            ".cargo/term.toml": "[term]\nverbose = false\n",
            **OUTSIDE,
        },
        declares(".cargo/config.toml", "include"),
    ),
    (
        "an include in Cargo configuration under its older name",
        {"Cargo.toml": HELPER, ".cargo/config": 'include = ["patch.toml"]\n', ".cargo/patch.toml": PATCH, **OUTSIDE},
        declares(".cargo/config", "include"),
    ),
    (
        "Cargo configuration that puts nothing in place of a dependency",
        {**alone("[dependencies]\n" + TAKES_B), ".cargo/config.toml": "[build]\njobs = 1\n"},
        ["a b"],
    ),
    ("a manifest that is not TOML", alone("[dependencies\n"), "cargo exited", "error:"),
]


def laid_out(temporary, files):
    """A case's files written under `temporary`, as the comment above `CASES` says:
    the link its workspace is asked through, and the manifests handed over."""
    root = os.path.join(temporary, "workspace")
    link = os.path.join(temporary, "link")
    os.makedirs(root)
    os.symlink(root, link)
    members = []
    for path, text in files.items():
        written = os.path.join(root, path)
        os.makedirs(os.path.dirname(written), exist_ok=True)
        with open(written, "w", encoding="utf-8") as file:
            file.write(text)
        if "[package]" in text:
            os.makedirs(os.path.join(os.path.dirname(written), "src"), exist_ok=True)
            with open(os.path.join(os.path.dirname(written), "src", "lib.rs"), "w", encoding="utf-8"):
                pass
            if WITHHELD not in text and not path.startswith(".."):
                members.append(os.path.relpath(os.path.join(link if path == "Cargo.toml" else root, path)))
    return link, members


def self_test():
    # Cargo is made quiet and colourful, as a contributor's own settings can make it,
    # so that only `--config term.quiet=false` and `--color never` keep what the cases
    # require it to say readable.
    os.environ["CARGO_TERM_QUIET"] = "true"
    os.environ["CARGO_TERM_COLOR"] = "always"
    wrong = 0
    # Without those settings in force the cases would hold neither flag, so Cargo is
    # asked about a workspace it warns about once without each: it has to keep quiet
    # when not told otherwise, and colour the warning when not told not to.
    with tempfile.TemporaryDirectory() as temporary:
        link, _ = laid_out(temporary, WARNS)
        asked = ["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1"]
        asked += ["--manifest-path", os.path.join(link, "Cargo.toml")]
        quiet = subprocess.run([*asked, "--color", "never"], capture_output=True, check=False).stderr
        coloured = subprocess.run([*asked, "--config", "term.quiet=false"], capture_output=True, check=False).stderr
    if b"resolver" in quiet:
        print("crate-edges: Cargo was not made quiet, so no case holds --config term.quiet=false", file=sys.stderr)
        wrong = 1
    if b"resolver" not in coloured or b"warning:" in coloured:
        print("crate-edges: Cargo was not made to colour what it says, so no case holds --color never", file=sys.stderr)
        wrong = 1
    for name, files, expected, *said in CASES:
        with tempfile.TemporaryDirectory() as temporary:
            link, members = laid_out(temporary, files)
            heard = io.StringIO()
            reason = ""
            with contextlib.redirect_stderr(heard):
                try:
                    got = [f"{taker} {taken}" for taker, taken in edges(os.path.join(link, "Cargo.toml"), members)]
                except Unanswered as refusal:
                    got, reason = None, str(refusal)
            cargo = heard.getvalue()
            problems = []
            if isinstance(expected, str):
                if got is not None or expected not in reason:
                    problems.append(f"gave {got}, not a refusal saying {expected!r}")
            elif got != expected:
                problems.append(f"gave {got}, not {expected}")
            if said and said[0] not in cargo:
                problems.append(f"did not pass on what Cargo said ({said[0]!r})")
            # Cargo warns on every call that does not name the format version it
            # answers in, and what it says reaches whoever reads the gate.
            if "--format-version" in cargo:
                problems.append("left Cargo to warn that no format version was named")
            for problem in problems:
                print(f"crate-edges: {name} {problem}", file=sys.stderr)
            if problems:
                if reason:
                    print(f"    refused: {reason}", file=sys.stderr)
                for line in cargo.splitlines():
                    print(f"    cargo: {line}", file=sys.stderr)
                wrong = 1
    return wrong


def main(argv):
    if argv[1:] == ["--self-test"]:
        return self_test()
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        found = edges(argv[1], argv[2:])
    except Unanswered as reason:
        print(f"crate-edges: {reason}", file=sys.stderr)
        return 2
    for taker, taken in found:
        print(taker, taken)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
