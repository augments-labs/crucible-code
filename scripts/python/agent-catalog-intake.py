#!/usr/bin/env -S python3 -I
"""The agent catalog's intake: registry `binary` and `uvx` entries become review data.

The intake runs in the catalog repository's CI, between a job that downloads
and starts vendor programs with no token, and a job that signs what a
maintainer merged. It holds no token and no signing key, signs nothing and
merges nothing, and it starts no program, `crucible` included: what
`crucible agents unpack-check` and `crucible agents probe` printed on each
target's runner is read from the files the untrusted job left. Everything it
reads is hostile, so it runs in isolated mode (`-I`), reads every file within a
ceiling before parsing it, refuses JSON with a repeated key or a non-finite
number or text UTF-8 cannot carry, and shows nothing of it in the human diff
but whole, inside a code span it cannot close, with every character that could
hide or reorder it written as its code point. Whatever one target or entry
breaks drops it alone, with its reason, and the run goes on with the others;
one run reads at most `RUN_BYTES` of observed data within `RUN_SECONDS`, and
what is left once either is spent waits for the next run.

`review` reads:

- `--registry`: the ACP registry index, at most 1 MiB and 512 agents;
- `--choices`: the reviewer's own choices, by registry id: the agent `key`
  crucible names it by, chosen once and never renamed, and the `reach` it may
  have once running;
- `--runners`: the runner matrix, by platform: whether its probe runs
  `confined` in a sandbox with the network closed, and the `reason` where not;
- `--observed`: what the untrusted job left, one directory per registry id and
  platform, `<id>/<platform>/`, holding `artifact` (the download), `fetched.json`
  (`{"chain": [...]}`, every address it was fetched through, the registry's
  first), `unpack-check.json` and `probe.json`; neither directory may be a
  link. A `uvx` agent's directory instead holds `resolved.json` (pip's
  `--report` for the package on that platform's pinned interpreter, of which
  only `version`, and each package's `download_info.url`, `metadata.name` and
  `metadata.version`, are read), `interpreter/` and `wheels/<n>/` (`n` the
  package's place in the report), each holding the `artifact`, `fetched.json`
  and `unpack-check.json` of one download, and `probe.json`; no directory on
  the way may be a link;
- `--interpreters`, where given: the reviewer's interpreter pins, by
  platform, each `{"url", "sha256", "python"}`: the `https` address of a
  python-build-standalone `install_only` build, its SHA-256, and the `3.M` it
  runs. A `uvx` target on a platform with no pin is dropped;
- `--deny`: `deny.toml`, whose licence list decides what may be mirrored;
- `--merged`, where given: the merged data, against which a version already
  reviewed is held. A release whose content differs from what was reviewed
  under the same version is refused, naming what differs, since a changed
  release is taken in only under a new version.

It writes the review data to `--out`, and each file-list asset (a per-file list
longer than 256 entries, kept as an asset named by its SHA-256) under
`--assets`. A target is dropped when its download is missing, larger than
1 GiB or not the bytes the registry pinned, when it was fetched through
anything but `https` to a named host, when its `cmd`, `args` or `env` is not
what a catalog may hold, `env` naming a loader variable among them, or when its
per-file list breaks a rule, a link that escapes its tree among them. An entry
is dropped when no target is left, when its reach names a literal address or
every host, or when any target left lacks complete probe output: a probe that
ran without a sandbox is accepted only on a runner the matrix lists as
unconfined, and is marked for the reviewer; a sign-in method whose `tty` is
not `true`, `false` or `unknown`, and any conformance result but `not run`, is
refused; and probes on different targets must agree on `agentInfo` and the
sign-in methods, the refusal naming the field that differs. Each drop is
recorded with its reason. An artifact whose declared licence is on `deny.toml`'s list is marked
with that licence for mirroring; nothing is mirrored until the licence gate
has passed it.

A `uvx` agent's package is pinned as `name==version`, and each platform the
runner matrix names is a target of its own: the pinned interpreter, started
with `-I`, and every package pip's report installs on it, each a wheel from
`https://files.pythonhosted.org/packages/`, at a plain address (no `%`,
query, fragment, parameter, backslash, or empty, `.` or `..` segment) that
every hop of its fetch keeps on that host, whose file name agrees with the
report and whose tags that interpreter on that platform accepts (Linux wheels
for glibc 2.34 or older, macOS ones for 11.0 on Apple silicon and 10.12 on
Intel). A target is dropped when the interpreter is not the bytes its pin
names, when a wheel holds a part of `.data` but `purelib` or `platlib`, which
the client does not install, when the agent's own wheel names no console
script by the package's name as `module:attr`, when an interpreter file lies
outside `python/` or a wheel's outside `site/`, when two of its artifacts
list one path, when a wheel lists one member twice, or when a wheel's own
record claims more members or a larger central directory than the stated
ceilings. A package pip found only as a source distribution drops its target
with a `build` list naming each one, clipped as a reason is, so it waits for
a build at review. An interpreter and its wheels are never marked for
mirroring.

`diff` writes the human diff of review data against the merged data before it,
as Markdown, showing only what changed, compared on whole values. A version
the merged data already holds is passed over only when it is the same in every
field; otherwise it is shown, marked as a same-version change.

`render` writes the catalog a client reads, from the merged data (`--merged`,
review data a reviewer completed and merged, with the `revoked` list a
reviewer adds to it, `{key, version, reason}` and `lock_revision` where only
one revision of a version is withdrawn) and, where there is one, the catalog
published before it (`--previous`). The serial is the previous one raised by
one (1 where none), `--signed-at` is the signing time in whole seconds,
`expires` is thirty days after it, and `revoked` holds every revocation either
source ever made, once each, since a revocation outlives the data that made
it. Entries pass through but for what only a reviewer reads (each target's
`probe` record, an artifact's `mirror_licence` mark); a long file list is
already an asset the entry names by hash and size. The bytes are sorted keys,
no spaces, UTF-8 and no floats, so the same data and time give the same bytes.
Nothing is written, and the run is refused, for a catalog over `INDEX_BYTES`
or holding more than `REVOKED` revocations; for a previous catalog that is not
in this schema, has a serial below 1, was signed after `--signed-at`, or lists
no `revoked` (an absent list is never read as an empty one, so a revocation
cannot be dropped unseen); for a number outside 0 to 2**53-1; and for an entry
missing a field the client's reader requires or with a key that is not an
agent key. Signing is not done here: the publishing job
signs the bytes with minisign.

`publish-check` runs in the publishing job before anything is uploaded. It
reads the merged data (`--merged`) and the directory of assets the job would
upload (`--assets`), each a regular file named by its lower-case hexadecimal
SHA-256 with no extension, and refuses the run, uploading nothing, when any
asset is not one of these: a file whose name is not a hash, whose bytes do not
hash to its name, whose hash the merged data does not hold, whose size is not
the one the data records for a file-list asset, or whose hash is an artifact's
and whose entry carries no `licence_gate` record of `{"passed": true}`. An
artifact copy is a redistribution, so a copy of any artifact, a mirror
included, waits for that record; a file-list asset is the catalog's own data
and needs none. A hash the data names both ways answers to both rules. A copy
also needs its artifact's `mirror_licence` mark. An empty directory passes,
since nothing is then uploaded. An asset is opened without following a link,
but the check cannot hold a directory still: the publishing job checks and
uploads from a directory the untrusted job cannot write once the check has
begun, or the check proves nothing about what is uploaded.
"""

from __future__ import annotations

import argparse
import configparser
import hashlib
import json
import os
import pathlib
import re
import stat
import struct
import sys
import time
import tomllib
import unicodedata
import urllib.parse
import zipfile

# The ceilings of what is read, each before it is parsed.
REGISTRY_BYTES = 1 << 20
AGENTS = 512
ARTIFACT_BYTES = 1 << 30
UNPACK_CHECK_BYTES = 16 << 20
PROBE_BYTES = 1 << 20
SMALL_BYTES = 1 << 20
DATA_BYTES = 64 << 20
# What one run of `review` may read in all, and how long it may take; what is
# left once either is spent is dropped, to be reviewed at the next run.
RUN_BYTES = 64 << 30
RUN_SECONDS = 3 * 60 * 60
# What one value may hold: arguments, variables, addresses, methods.
ITEMS = 64
TEXT_BYTES = 4096
CAPABILITY_BYTES = 4096
# The catalog's own limits.
LISTED_FILES = 256
ASSET_BYTES = 4 << 20
LISTED_ENTRIES = 1 << 20
LINK_STEPS = 8
VERSION_BYTES = 64
FORMAT = 1
# What a rendered catalog may be, as the client that reads it allows: the
# bytes of the index, and the revocations it carries; and its life.
INDEX_BYTES = 1 << 20
REVOKED = 4096
SCHEMA = 1
LIFETIME_SECONDS = 30 * 24 * 60 * 60
# The largest whole number a catalog's serial or time may be, one every JSON
# reader holds exactly.
WHOLE = (1 << 53) - 1
# How much of one hostile text a reason quotes, its head and its tail, which
# is more than any host is long; and how many changed files the diff names.
REASON_CHARS = 300
SHOWN_FILES = 50
# The most differing fields a reason names.
NAMED_FIELDS = 8
# A `uvx` target's own ceilings: the packages pip's report may install, the
# report's bytes, which carry each package's metadata, and the bytes of the
# agent's own `entry_points.txt`.
WHEELS = 256
REPORT_BYTES = 16 << 20
ENTRY_POINTS_BYTES = 64 << 10
# The bytes a wheel's central directory may take, read from its own record
# before `zipfile` holds it: room for `LISTED_ENTRIES` members of short names.
CENTRAL_DIRECTORY_BYTES = 64 << 20
# The one host a wheel is fetched from, at every hop of the way.
PYPI_HOST = "files.pythonhosted.org"

PLATFORM = re.compile(r"(linux|darwin|windows|freebsd)-(x86_64|aarch64)")
REGISTRY_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
AGENT_KEY = re.compile(r"[a-z0-9-]{1,32}")
LABEL = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?")
SHA256 = re.compile(r"[0-9a-f]{64}")
# A `uvx` package pinned as `name==version`: a PyPI project name, and a
# version of the characters a version is spelt with, so no extra, range,
# marker or address passes.
PACKAGE = re.compile(r"([A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?)==(.*)", re.DOTALL)
PINNED = re.compile(r"[A-Za-z0-9][A-Za-z0-9.!+_-]*")
# An interpreter pin's version, `3.M`.
PYTHON = re.compile(r"3\.(0|[1-9][0-9]{0,2})")
# Where every package a `uvx` target installs is taken from.
PYPI = "https://files.pythonhosted.org/packages/"
# A wheel's file name: `{name}-{version}(-{build})?-{python}-{abi}-{platform}.whl`.
WHEEL_NAME = re.compile(
    r"(?P<name>[A-Za-z0-9](?:[A-Za-z0-9._]*[A-Za-z0-9])?)-(?P<version>[A-Za-z0-9.!+_]+)"
    r"(?:-(?P<build>[0-9][A-Za-z0-9_.]*))?-(?P<python>[A-Za-z0-9_.]+)-(?P<abi>[A-Za-z0-9_.]+)"
    r"-(?P<platform>[A-Za-z0-9_.]+)\.whl"
)
# The platform tags a wheel may carry on each platform an interpreter is
# pinned for, beside `any`. Each ceiling is the oldest system crucible's own
# release runs on: glibc 2.34 on Linux, the floor the release workflow builds
# against and checks; macOS 11.0 on Apple silicon, the first macOS to run on
# it; and macOS 10.12 on Intel, the oldest Rust's x86_64-apple-darwin target
# supports. musllinux, linux_* and every tag not named here are refused.
PLATFORM_TAGS: dict[str, dict] = {
    "linux-x86_64": {
        "glibc": 34,
        "machine": "x86_64",
        "legacy": {"manylinux1_x86_64", "manylinux2010_x86_64", "manylinux2014_x86_64"},
    },
    "linux-aarch64": {"glibc": 34, "machine": "aarch64", "legacy": {"manylinux2014_aarch64"}},
    "darwin-aarch64": {"macos": (11, 0), "machines": {"arm64", "universal2"}},
    "darwin-x86_64": {"macos": (10, 12), "machines": {"x86_64", "intel", "universal2"}},
    "windows-x86_64": {"exact": {"win_amd64"}},
    "windows-aarch64": {"exact": {"win_arm64"}},
}


class Refused(Exception):
    """An input the whole run cannot go on from."""


class Dropped(Exception):
    """A target or an entry that is left out, and why."""


class Spent(Dropped):
    """The run's budget is spent, so what is left waits for the next run."""


class Build(Dropped):
    """A `uvx` target whose report installs a source distribution, so it waits
    for that to be built at review; `build` names each one."""

    def __init__(self, build: list[dict]) -> None:
        self.build = build
        names = named([f"{item['package']} {item['version']}" for item in build])
        super().__init__(
            f"{names} {'comes' if len(build) == 1 else 'come'} only as a source distribution here, "
            "so this target waits for a build at review"
        )


class Budget:
    """What one run has read, and since when."""

    def __init__(self) -> None:
        self.read = 0
        self.started = time.monotonic()

    def spend(self, count: int) -> None:
        self.read += count
        if self.read > RUN_BYTES or time.monotonic() - self.started > RUN_SECONDS:
            raise Spent(
                f"the run's budget of {RUN_BYTES} bytes and {RUN_SECONDS} seconds is spent; "
                "it is reviewed at the next run"
            )


# The budget of the `review` running, which every read of observed data spends.
BUDGET: Budget | None = None


def spend(count: int) -> None:
    if BUDGET is not None:
        BUDGET.spend(count)


def clipped(text: object) -> str:
    """Hostile `text` as a reason quotes it: whole within `REASON_CHARS`, and
    otherwise its head and its tail, so the end that names a host is kept."""
    text = str(text)
    if len(text) <= REASON_CHARS:
        return text
    half = REASON_CHARS // 2
    return text[:half] + "..." + text[-half:]


def differences(new: object, old: object, at: str = "") -> list[str]:
    """Where `new` and `old` differ, as field paths; a list of targets is
    compared by platform, and a number never equals a flag."""
    if type(new) is not type(old):
        return [at or "everything"]
    if isinstance(new, dict):
        found = []
        for name in sorted(set(new) | set(old)):
            place = f"{at}.{name}" if at else name
            if name not in new or name not in old:
                found.append(place)
            else:
                found += differences(new[name], old[name], place)
        return found
    if isinstance(new, list):
        if all(isinstance(item, dict) and "platform" in item for item in new + old):
            by_new = {str(item["platform"]): item for item in new}
            by_old = {str(item["platform"]): item for item in old}
            found = []
            for platform in sorted(set(by_new) | set(by_old)):
                place = f"{at}[{platform}]"
                if platform not in by_new or platform not in by_old:
                    found.append(place)
                else:
                    found += differences(by_new[platform], by_old[platform], place)
            return found
        found = []
        for index in range(max(len(new), len(old))):
            place = f"{at}[{index}]"
            if index >= len(new) or index >= len(old):
                found.append(place)
            else:
                found += differences(new[index], old[index], place)
        return found
    return [] if new == old else [at or "everything"]


def named(paths: list[str]) -> str:
    shown_paths = [clipped(path) for path in paths[:NAMED_FIELDS]]
    more = f" and {len(paths) - NAMED_FIELDS} more" if len(paths) > NAMED_FIELDS else ""
    return ", ".join(shown_paths) + more


def unrepeated(pairs: list[tuple[str, object]]) -> dict:
    held: dict = {}
    for name, value in pairs:
        if name in held:
            raise ValueError(f"the key {clipped(name)} is given twice")
        held[name] = value
    return held


def non_finite(name: str) -> object:
    raise ValueError(f"{name} is not a number JSON holds")


def parsed(text: bytes, what: str) -> object:
    try:
        return json.loads(text, object_pairs_hook=unrepeated, parse_constant=non_finite)
    except (ValueError, RecursionError) as refused:
        raise Refused(f"{what} is not JSON as read here: {refused}") from None


def read(path: pathlib.Path, ceiling: int, what: str) -> bytes:
    """The regular file at `path`, never a link, refused past `ceiling`."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Refused(f"{what} is missing") from None
    if not stat.S_ISREG(held.st_mode):
        raise Refused(f"{what} is not a regular file")
    with open(path, "rb") as file:
        text = file.read(ceiling + 1)
    spend(len(text))
    if len(text) > ceiling:
        raise Refused(f"{what} is larger than {ceiling} bytes")
    return text


def read_json(path: pathlib.Path, ceiling: int, what: str) -> object:
    return parsed(read(path, ceiling, what), what)


def obj(value: object, what: str) -> dict:
    if not isinstance(value, dict):
        raise Dropped(f"{what} is not an object")
    return value


def encoded(value: str, what: str) -> bytes:
    """`value` in UTF-8, refused where it holds what UTF-8 cannot, such as a
    lone surrogate JSON can spell."""
    try:
        return value.encode()
    except UnicodeEncodeError:
        raise Dropped(f"{what} holds text that is not Unicode a catalog can carry") from None


def text(value: object, what: str) -> str:
    """Non-empty text of no control character, within its ceiling."""
    if not isinstance(value, str) or not value:
        raise Dropped(f"{what} is not text")
    if len(encoded(value, what)) > TEXT_BYTES or any(ord(c) < 0x20 or ord(c) == 0x7F for c in value):
        raise Dropped(f"{what} is not text a catalog holds")
    return value


def argument(value: object, what: str) -> str:
    """An argument or a variable's value, which may be empty."""
    if not isinstance(value, str) or "\0" in value or len(encoded(value, what)) > TEXT_BYTES:
        raise Dropped(f"{what} is not an argument a catalog holds")
    return value


def arguments(value: object, what: str) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > ITEMS:
        raise Dropped(f"{what} is not a list of at most {ITEMS} arguments")
    return [argument(item, f"{what}[{index}]") for index, item in enumerate(value)]


def number(value: object, what: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise Dropped(f"{what} is not a whole number")
    return value


def digest(value: object, what: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise Dropped(f"{what} is not a SHA-256 in lowercase hexadecimal")
    return value


def inside(path: str) -> bool:
    """Whether `path` names something under the directory it is relative to."""
    return (
        bool(path)
        and not any(c in path for c in "\\:\0")
        and all(part not in ("", ".", "..") for part in path.split("/"))
    )


def relative(value: object, what: str) -> str:
    """A path relative to the version's directory; one leading `./` is dropped."""
    path = text(value, what)
    path = path[2:] if path.startswith("./") else path
    if not inside(path):
        raise Dropped(f"{what} {clipped(path)} is not a relative path that stays inside its directory")
    return path


def version(value: object, what: str) -> str:
    held = text(value, what)
    if len(held.encode()) > VERSION_BYTES or held in (".", "..") or any(c in held for c in "/\\:"):
        raise Dropped(f"{what} {clipped(held)} is not a version that can name one directory")
    return held


def loader(name: str) -> bool:
    """Whether a loader reads `name` to choose what code runs, in any case."""
    name = name.upper()
    return name.startswith(("LD_", "DYLD_")) or name in ("NODE_OPTIONS", "NODE_PATH", "PYTHONPATH", "PATH")


def variables(value: object, what: str) -> dict[str, str]:
    if value is None:
        return {}
    if not isinstance(value, dict) or len(value) > ITEMS:
        raise Dropped(f"{what} is not an object of at most {ITEMS} variables")
    held = {}
    for name, setting in value.items():
        if not name or "=" in name or "\0" in name or len(encoded(name, what)) > TEXT_BYTES:
            raise Dropped(f"{what} names a variable no environment can hold")
        if loader(name):
            raise Dropped(f"{what} sets {clipped(name)}, a variable a loader reads to choose what code runs")
        held[name] = argument(setting, f"{what}.{clipped(name)}")
    return held


def host(name: str) -> bool:
    """Whether `name` is a host's name, of two labels or more, and no address."""
    labels = name.split(".")
    return (
        len(name) <= 253
        and len(labels) >= 2
        and all(LABEL.fullmatch(label) for label in labels)
        and not labels[-1].isdigit()
    )


def reach(value: object, what: str) -> list[str]:
    """Hosts by name, each may open with `*.` over a name of two labels or more."""
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > ITEMS:
        raise Dropped(f"{what} is not a list of at most {ITEMS} hosts")
    held = []
    for item in value:
        pattern = text(item, what)
        named = pattern[2:] if pattern.startswith("*.") else pattern
        if not host(named):
            raise Dropped(f"{what} names {clipped(pattern)}, which is not a host by name or a pattern under one")
        held.append(pattern)
    return held


def address(value: object, what: str) -> tuple[str, str]:
    """An `https` address to a named host, with no user, password or port."""
    url = text(value, what)
    try:
        parts = urllib.parse.urlsplit(url)
    except ValueError:
        raise Dropped(f"{what} {clipped(url)} is not an https address on a host by name") from None
    name = parts.netloc
    if parts.scheme != "https" or not host(name):
        raise Dropped(f"{what} {clipped(url)} is not an https address on a host by name")
    return url, name


def format_of(url: str) -> str:
    path = urllib.parse.urlsplit(url).path.lower()
    if path.endswith((".tar.gz", ".tgz")):
        return "tar.gz"
    if path.endswith(".zip"):
        return "zip"
    if path.endswith((".tar.bz2", ".tar.xz", ".tar.zst", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar")):
        raise Dropped(f"{clipped(url)} is an archive of a kind this intake does not take in")
    return "bare"


def opened(path: pathlib.Path):
    """The download at `path`, opened for reading, refused where it is not a
    regular file."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Dropped("could not be hashed: nothing was downloaded") from None
    if not stat.S_ISREG(held.st_mode):
        raise Dropped("could not be hashed: the download is not a regular file")
    try:
        # Opened without following a link, so a path swapped for one after the
        # check above reads nothing.
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError:
        raise Dropped("could not be hashed: the download is not a regular file") from None
    file = os.fdopen(descriptor, "rb")
    if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
        file.close()
        raise Dropped("could not be hashed: the download is not a regular file")
    return file


def digested(file) -> tuple[str, int]:
    """The SHA-256 and size of the open download `file`, read within its ceiling."""
    sha = hashlib.sha256()
    size = 0
    while chunk := file.read(1 << 20):
        spend(len(chunk))
        size += len(chunk)
        if size > ARTIFACT_BYTES:
            raise Dropped(f"could not be hashed: the download is larger than {ARTIFACT_BYTES} bytes")
        sha.update(chunk)
    return sha.hexdigest(), size


def hashed(path: pathlib.Path) -> tuple[str, int]:
    """The SHA-256 and size of the download at `path`, read within its ceiling."""
    with opened(path) as file:
        return digested(file)


def file_list(value: object) -> tuple[list[dict], int, int]:
    """What `unpack-check` printed: its files, `unpacked` and `decompressed`."""
    printed = obj(value, "the unpack-check output")
    files = printed.get("files")
    if not isinstance(files, list) or len(files) > LISTED_ENTRIES:
        raise Dropped("the unpack-check output lists no files")
    unpacked = number(printed.get("unpacked"), "unpack-check's unpacked")
    decompressed = number(printed.get("decompressed"), "unpack-check's decompressed")
    held: list[dict] = []
    seen: dict[str, dict] = {}
    folded: set[str] = set()
    total = 0
    for index, item in enumerate(files):
        item = obj(item, f"unpack-check file {index}")
        path = text(item.get("path"), f"unpack-check file {index}'s path")
        if not inside(path):
            raise Dropped(f"the unpack-check output lists {clipped(path)}, which is not inside its tree")
        if path.casefold() in folded:
            raise Dropped(f"the unpack-check output lists {clipped(path)} twice")
        folded.add(path.casefold())
        kind = item.get("kind")
        if kind == "file":
            size = number(item.get("size"), f"{clipped(path)}'s size")
            exec_flag = item.get("exec")
            if not isinstance(exec_flag, bool):
                raise Dropped(f"{clipped(path)}'s exec flag is not true or false")
            listed = {
                "exec": exec_flag,
                "kind": "file",
                "path": path,
                "sha256": digest(item.get("sha256"), f"{clipped(path)}'s sha256"),
                "size": size,
            }
            total += size
        elif kind == "dir":
            listed = {"kind": "dir", "path": path}
        elif kind == "link":
            target = text(item.get("target"), f"{clipped(path)}'s link target")
            if target.startswith("/") or any(c in target for c in "\\:"):
                raise Dropped(f"{clipped(path)} links to {clipped(target)}, which is not a relative link")
            listed = {"kind": "link", "path": path, "target": target}
        else:
            raise Dropped(f"{clipped(path)} is not a file, a directory or a link")
        held.append(listed)
        seen[path] = listed
    if total != unpacked:
        raise Dropped(f"the unpack-check output's files sum to {total} bytes, not the {unpacked} it says")
    for listed in held:
        if listed["kind"] == "link":
            resolved(listed["path"], seen)
    return held, unpacked, decompressed


def resolved(link: str, seen: dict[str, dict]) -> str:
    """The listed file `link` reaches, through directories alone and at most
    `LINK_STEPS` links, refused where it escapes its tree or reaches nothing."""
    current = link
    for _ in range(LINK_STEPS):
        target = seen[current]["target"]
        parts: list[str] = current.split("/")[:-1]
        for part in target.split("/"):
            if part in ("", "."):
                continue
            if part == "..":
                if not parts:
                    raise Dropped(f"{clipped(link)} links to {clipped(target)}, which escapes its tree")
                parts.pop()
            else:
                parts.append(part)
        for depth in range(1, len(parts)):
            above = seen.get("/".join(parts[:depth]))
            if above is not None and above["kind"] != "dir":
                raise Dropped(f"{clipped(link)} links through {clipped('/'.join(parts[:depth]))}, which is not a directory")
        current = "/".join(parts)
        reached = seen.get(current)
        if reached is None:
            raise Dropped(f"{clipped(link)} links to {clipped(target)}, which is not in its tree")
        if reached["kind"] == "file":
            return current
        if reached["kind"] == "dir":
            raise Dropped(f"{clipped(link)} links to a directory, not a file")
    raise Dropped(f"{clipped(link)} takes more than {LINK_STEPS} links to reach a file")


def place_of(observed: pathlib.Path, *parts: str) -> pathlib.Path:
    """Where the untrusted job left what it observed of one target, or of one
    download of it, refused where a directory on the way there is a link,
    which could reach anywhere."""
    place = observed
    for part in parts:
        place = place / part
        try:
            held = os.lstat(place)
        except OSError:
            return observed.joinpath(*parts)
        if not stat.S_ISDIR(held.st_mode):
            raise Dropped(f"{clipped(part)} is not a directory of the observed tree")
    return place


def chained(place: pathlib.Path, url: str, begins: str) -> list[str]:
    """The hosts of the chain `fetched.json` under `place` gives, which must
    begin at `url`, `begins` saying whose address that is."""
    try:
        fetched = obj(read_json(place / "fetched.json", SMALL_BYTES, "fetched.json"), "fetched.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    chain = fetched.get("chain")
    if not isinstance(chain, list) or not chain or len(chain) > ITEMS:
        raise Dropped("fetched.json gives no chain of addresses")
    if chain[0] != url:
        raise Dropped(f"fetched.json does not begin at {begins}")
    hosts: list[str] = []
    for hop in chain:
        _, name = address(hop, "the redirect to")
        if name not in hosts:
            hosts.append(name)
    return hosts


def unpacked_at(place: pathlib.Path) -> tuple[list[dict], int, int]:
    """What `unpack-check` printed of the download under `place`."""
    try:
        printed = read_json(place / "unpack-check.json", UNPACK_CHECK_BYTES, "unpack-check.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    return file_list(printed)


def runnable(files: list[dict], cmd: str, platform: str) -> None:
    """Refuse `cmd` where `files` does not list it as a file it may run."""
    program = next((file for file in files if file["path"] == cmd), None)
    if program is None or program["kind"] != "file":
        raise Dropped(f"cmd {clipped(cmd)} is not a file the archive unpacks to")
    if not program["exec"] and not platform.startswith("windows-"):
        raise Dropped(f"cmd {clipped(cmd)} is not runnable")


def artifact_of(
    url: str,
    format: str,
    sha256: str,
    size: int,
    listing: tuple[list[dict], int, int],
    assets: dict[str, bytes],
) -> dict:
    """One artifact as the catalog pins it, its file list kept as a file-list
    asset where it is longer than a catalog lists inline."""
    files, unpacked, decompressed = listing
    artifact: dict = {
        "decompressed": decompressed,
        "format": format,
        "sha256": sha256,
        "size": size,
        "unpacked": unpacked,
        "url": url,
    }
    if len(files) > LISTED_FILES:
        held = compact(files)
        if len(held) > ASSET_BYTES:
            raise Dropped(f"its file list is larger than a catalog asset may be, {ASSET_BYTES} bytes")
        named = hashlib.sha256(held).hexdigest()
        assets[named] = held
        artifact["files"] = {"sha256": named, "size": len(held)}
    else:
        artifact["files"] = files
    return artifact


def compact(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    spec: object,
    runners: dict,
    mirrored: str | None,
    assets: dict[str, bytes],
) -> dict:
    """One registry target as the catalog will list it, before its probe."""
    if not PLATFORM.fullmatch(platform):
        raise Dropped("is not a platform the catalog names")
    spec = obj(spec, "the target")
    url, _ = address(spec.get("archive"), "the archive")
    cmd = relative(spec.get("cmd"), "cmd")
    args = arguments(spec.get("args"), "args")
    env = variables(spec.get("env"), "env")
    pinned = spec.get("sha256")
    if pinned is not None:
        pinned = digest(pinned.lower() if isinstance(pinned, str) else pinned, "the registry's sha256")
    format = format_of(url)
    if platform not in runners:
        raise Dropped("no runner in the matrix probes this platform")
    place = place_of(observed, agent, platform)
    sha256, size = hashed(place / "artifact")
    if pinned is not None and pinned != sha256:
        raise Dropped("the download is not the bytes the registry pinned")
    hosts = chained(place, url, "the registry's address")
    listing = unpacked_at(place)
    runnable(listing[0], cmd, platform)
    artifact = artifact_of(url, format, sha256, size, listing, assets)
    if mirrored is not None:
        artifact["mirror_licence"] = mirrored
    return {
        "args": args,
        "artifacts": [artifact],
        "cmd": cmd,
        "env": env,
        "hosts": hosts,
        "platform": platform,
    }


def interpreters_of(path: pathlib.Path | None) -> dict[str, dict]:
    """The reviewer's interpreter pins, by platform: each a python-build-standalone
    `install_only` build's address, its SHA-256 and the `3.M` it runs. With no
    pins, no platform has an interpreter."""
    if path is None:
        return {}
    value = read_json(path, SMALL_BYTES, "the interpreter pins")
    if not isinstance(value, dict):
        raise Refused("the interpreter pins are not an object by platform")
    held = {}
    for platform, pin in value.items():
        if platform not in PLATFORM_TAGS:
            raise Refused(
                f"the interpreter pins name {clipped(platform)}, which is not a platform "
                "a wheel's tags are read for"
            )
        where = f"the interpreter pin for {platform}"
        if not isinstance(pin, dict):
            raise Refused(f"{where} is not an object")
        extra = sorted(set(pin) - {"url", "sha256", "python"})
        if extra:
            raise Refused(f"{where} holds {named(extra)}, which no pin holds")
        url = pin.get("url")
        try:
            address(url, where)
        except Dropped:
            url = None
        if url is None or not urllib.parse.urlsplit(url).path.endswith("-install_only.tar.gz"):
            raise Refused(
                f"{where} names {clipped(pin.get('url'))}, which is not an https address of a "
                "python-build-standalone install_only build"
            )
        sha256 = pin.get("sha256")
        if not isinstance(sha256, str) or not SHA256.fullmatch(sha256):
            raise Refused(f"{where}: its sha256 is not a SHA-256 in lowercase hexadecimal")
        python = pin.get("python")
        if not isinstance(python, str) or not PYTHON.fullmatch(python):
            raise Refused(f"{where}: its python {clipped(python)} is not a version 3.M")
        held[platform] = {"minor": int(python[2:]), "python": python, "sha256": sha256, "url": url}
    return held


def normal(name: str) -> str:
    """A package's name as an index compares it."""
    return re.sub(r"[-_.]+", "-", name).lower()


def python_fits(python: str, abi: str, minor: int) -> bool:
    """Whether the interpreter tag `python` with the ABI tag `abi` runs on a
    CPython of 3.`minor` built without debug or free threading."""
    cpython = re.fullmatch(r"cp3(0|[1-9][0-9]*)", python)
    if cpython is not None:
        built = int(cpython[1])
        if built == minor:
            return abi in (f"cp3{minor}", "abi3", "none")
        return 2 <= built < minor and abi == "abi3"
    generic = re.fullmatch(r"py3(0|[1-9][0-9]*)?", python)
    return generic is not None and abi == "none" and (generic[1] is None or int(generic[1]) <= minor)


def platform_fits(tag: str, platform: str) -> bool:
    """Whether the platform tag `tag` runs on `platform`, as `PLATFORM_TAGS` holds it."""
    if tag == "any":
        return True
    accepted = PLATFORM_TAGS.get(platform)
    if accepted is None:
        return False
    if "glibc" in accepted:
        held = re.fullmatch(r"manylinux_2_(0|[1-9][0-9]*)_([a-z0-9_]+)", tag)
        if held is not None:
            return held[2] == accepted["machine"] and int(held[1]) <= accepted["glibc"]
        return tag in accepted["legacy"]
    if "macos" in accepted:
        held = re.fullmatch(r"macosx_(0|[1-9][0-9]*)_(0|[1-9][0-9]*)_([a-z0-9_]+)", tag)
        return (
            held is not None
            and held[3] in accepted["machines"]
            and (int(held[1]), int(held[2])) <= accepted["macos"]
        )
    return tag in accepted["exact"]


def fits(tags: str, platform: str, minor: int) -> bool:
    """Whether a wheel tagged `tags` (`{python}-{abi}-{platform}`, each part
    expanded on its dots) runs on an interpreter of 3.`minor` on `platform`."""
    parts = tags.split("-")
    if len(parts) != 3:
        return False
    pythons, abis, platforms = (part.split(".") for part in parts)
    return any(python_fits(python, abi, minor) for python in pythons for abi in abis) and any(
        platform_fits(tag, platform) for tag in platforms
    )


def dotted(value: str) -> bool:
    """Whether `value` is a dotted Python name."""
    return all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", part, re.ASCII) for part in value.split("."))


class Spending:
    """An open download that spends the run's budget on every byte read from
    it, so a wheel's members are read within the budget."""

    def __init__(self, file) -> None:
        self.file = file

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(count)
        spend(len(chunk))
        return chunk

    def seek(self, offset: int, whence: int = 0) -> int:
        return self.file.seek(offset, whence)

    def tell(self) -> int:
        return self.file.tell()

    def seekable(self) -> bool:
        return True


def data_part(name: str) -> None:
    """Refuse the wheel member `name` where the client would not install it:
    of a `.data` directory (its top directory's last dotted part being `data`
    in any case) only `purelib` and `platlib` are installed."""
    member = name[2:] if name.startswith("./") else name
    directory = member.endswith("/")
    member = member[:-1] if directory else member
    top, slash, rest = member.partition("/")
    _, dot, suffix = top.rpartition(".")
    if not dot or not suffix.isascii() or suffix.lower() != "data":
        return
    if not slash:
        if directory:
            return
        refused = top
    else:
        part, slash, path = rest.partition("/")
        if part in ("purelib", "platlib") and (path or directory):
            return
        refused = part
    raise Dropped(
        f"holds {clipped(member)} under {clipped(refused)}, which the client refuses: "
        "of a wheel's .data directory only purelib and platlib are installed"
    )


def entry_point_of(archive: zipfile.ZipFile, names: list[str], package: str) -> str:
    """The `module:attr` the agent's own wheel names as its console script `package`."""
    tops = {name.split("/", 1)[0] for name in names if name.split("/", 1)[0].endswith(".dist-info")}
    if len(tops) != 1:
        raise Dropped("holds no single .dist-info directory")
    try:
        info = archive.getinfo(f"{tops.pop()}/entry_points.txt")
    except KeyError:
        raise Dropped("has no entry_points.txt") from None
    with archive.open(info) as file:
        content = file.read(ENTRY_POINTS_BYTES + 1)
    if len(content) > ENTRY_POINTS_BYTES:
        raise Dropped(f"its entry_points.txt is larger than {ENTRY_POINTS_BYTES} bytes")
    # A default section no header can name, as a header holds no line break,
    # so a `[DEFAULT]` section is a section and lends console_scripts nothing.
    parser = configparser.ConfigParser(interpolation=None, strict=True, delimiters=("=",), default_section="\n")
    parser.optionxform = str  # a script's name is read as written
    try:
        parser.read_string(content.decode())
    except (configparser.Error, UnicodeDecodeError):
        raise Dropped("its entry_points.txt cannot be read: it is not UTF-8, or repeats a section or a name") from None
    if not parser.has_option("console_scripts", package):
        raise Dropped(f"names no console script {clipped(package)}")
    value = parser.get("console_scripts", package)
    module, colon, attr = value.partition(":")
    if not colon or not dotted(module) or not dotted(attr):
        raise Dropped(f"its console script {clipped(package)} is {clipped(value)}, not module:attr")
    return value


def central_directory(file) -> None:
    """Refuse the open wheel `file` where the record that ends it claims more
    members than `LISTED_ENTRIES` or a central directory larger than
    `CENTRAL_DIRECTORY_BYTES`, read before `zipfile` holds that directory. The
    record is found as `zipfile` finds it, the last one in the file's tail,
    and its zip64 record, where its locator stands before it, is read too."""
    end_bytes, locator_bytes = 22, 20
    size = file.seek(0, os.SEEK_END)
    start = max(0, size - (end_bytes + 0xFFFF))
    file.seek(start)
    tail = file.read(size - start)
    at = tail.rfind(b"PK\x05\x06")
    if at < 0 or len(tail) - at < end_bytes:
        raise Dropped("is not a wheel this intake can read")
    entries, directory = struct.unpack("<HI", tail[at + 10 : at + 16])
    if at >= locator_bytes and tail[at - locator_bytes : at - locator_bytes + 4] == b"PK\x06\x07":
        (offset,) = struct.unpack("<Q", tail[at - locator_bytes + 8 : at - locator_bytes + 16])
        if offset > size - 56:
            raise Dropped("is not a wheel this intake can read")
        file.seek(offset)
        record = file.read(56)
        if record[:4] != b"PK\x06\x06":
            raise Dropped("is not a wheel this intake can read")
        entries, directory = struct.unpack("<QQ", record[32:48])
    if entries > LISTED_ENTRIES:
        raise Dropped(f"its central directory lists more than {LISTED_ENTRIES} members")
    if directory > CENTRAL_DIRECTORY_BYTES:
        raise Dropped(f"its central directory is larger than {CENTRAL_DIRECTORY_BYTES} bytes")
    file.seek(0)


def wheel_read(file, package: str | None) -> str | None:
    """Refuse the open wheel `file` where a member is one the client would not
    install, or two members share a name, and give its console script
    `package` where one is asked for."""
    try:
        spending = Spending(file)
        central_directory(spending)
        with zipfile.ZipFile(spending) as archive:
            infos = archive.infolist()
            if len(infos) > LISTED_ENTRIES:
                raise Dropped(f"holds more than {LISTED_ENTRIES} members")
            names = [info.filename for info in infos]
            seen: set[str] = set()
            for name in names:
                if name in seen:
                    raise Dropped(f"lists {clipped(name)} twice")
                seen.add(name)
                data_part(name)
            return entry_point_of(archive, names, package) if package is not None else None
    except Dropped:
        raise
    except Exception:  # whatever zipfile cannot read is a wheel this intake cannot read
        raise Dropped("is not a wheel this intake can read") from None


def under(files: list[dict], top: str) -> None:
    """Refuse a listing with a path outside the directory `top`."""
    for file in files:
        path = file["path"]
        if path != top and not path.startswith(top + "/"):
            raise Dropped(f"lists {clipped(path)}, which is not under {top}/")


def staged(prefix: str, work):
    """What `work` gives, a drop it raises naming `prefix` first."""
    try:
        return work()
    except Spent:
        raise
    except Dropped as failed:
        raise Dropped(f"{prefix}: {failed}") from None


def report_of(place: pathlib.Path) -> list[tuple[str, str, str]]:
    """The address, name and version of each package pip's report under
    `place` installs, in its order; nothing else of the report is read."""
    try:
        report = obj(read_json(place / "resolved.json", REPORT_BYTES, "resolved.json"), "pip's report")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if report.get("version") != "1":
        raise Dropped(f"pip's report is in version {clipped(report.get('version'))}, not 1, the one read here")
    install = report.get("install")
    if not isinstance(install, list) or not install or len(install) > WHEELS:
        raise Dropped(f"pip's report installs nothing, or more than {WHEELS} packages")
    items = []
    for index, item in enumerate(install):
        item = obj(item, f"the report's package {index}")
        url = text(obj(item.get("download_info"), f"the report's package {index}'s download").get("url"),
                   f"the report's package {index}'s address")
        metadata = obj(item.get("metadata"), f"the report's package {index}'s metadata")
        items.append(
            (
                url,
                text(metadata.get("name"), f"the report's package {index}'s name"),
                text(metadata.get("version"), f"the report's package {index}'s version"),
            )
        )
    return items


def uvx_target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    package: str,
    released: str,
    interpreters: dict[str, dict],
    assets: dict[str, bytes],
) -> dict:
    """One platform of a `uvx` agent as the catalog will list it, before its
    probe: the interpreter pinned for it and every wheel pip's report installs
    on it, each the bytes that were downloaded and checked."""
    pin = interpreters.get(platform)
    if pin is None:
        raise Dropped("no interpreter is pinned for this platform")
    place = place_of(observed, agent, platform)
    wheels = []
    build = []
    own = None
    for index, (url, name, item_version) in enumerate(report_of(place)):
        if not url.startswith(PYPI):
            raise Dropped(f"{clipped(url)} is not on {PYPI}, the one place a wheel is taken from")
        odd = next((mark for mark in "%?#;\\" if mark in url), None)
        if odd is not None:
            raise Dropped(
                f"{clipped(url)} is not a plain address: it holds a {odd}, and a %, query, fragment, "
                "parameter or backslash would let it be read as other than it is written"
            )
        path = urllib.parse.urlsplit(url).path
        if any(segment in ("", ".", "..") for segment in path[1:].split("/")):
            raise Dropped(f"{clipped(url)} holds an empty, . or .. segment in its path")
        file = path.rsplit("/", 1)[-1]
        mine = normal(name) == normal(package) and item_version == released
        own = own or mine
        if not file.endswith(".whl"):
            # At most `WHEELS` rows, as the report lists no more, each field
            # clipped as a reason quotes it.
            build.append({"package": clipped(name), "url": clipped(url), "version": clipped(item_version)})
            continue
        held = WHEEL_NAME.fullmatch(file)
        if held is None:
            raise Dropped(f"{clipped(file)} is not a wheel's file name")
        if normal(held["name"]) != normal(name) or held["version"] != item_version:
            raise Dropped(
                f"wheel {clipped(file)} does not agree with the report, which says it is "
                f"{clipped(name)} {clipped(item_version)}"
            )
        if not fits(f"{held['python']}-{held['abi']}-{held['platform']}", platform, pin["minor"]):
            raise Dropped(
                f"wheel {clipped(file)} is built for none of the tags an interpreter of "
                f"Python {pin['python']} on {platform} accepts"
            )
        wheels.append((index, url, file, mine))
    if not own:
        raise Dropped(f"the report installs no {clipped(package)} {clipped(released)}")
    if build:
        raise Build(build)
    windows = platform.startswith("windows-")
    cmd = "python/python.exe" if windows else f"python/bin/python{pin['python']}"

    def python() -> tuple[dict, list[str], list[dict]]:
        inner = place_of(observed, agent, platform, "interpreter")
        sha256, size = hashed(inner / "artifact")
        if sha256 != pin["sha256"]:
            raise Dropped("the download is not the bytes the interpreter pin names")
        hosts = chained(inner, pin["url"], "the pinned address")
        listing = unpacked_at(inner)
        under(listing[0], "python")
        runnable(listing[0], cmd, platform)
        return artifact_of(pin["url"], "tar.gz", sha256, size, listing, assets), hosts, listing[0]

    artifact, hosts, files = staged("the interpreter", python)
    artifacts = [artifact]
    listings = [files]
    entry_point = None
    for index, url, file, mine in wheels:

        def one() -> tuple[dict, list[str], list[dict], str | None]:
            inner = place_of(observed, agent, platform, "wheels", str(index))
            with opened(inner / "artifact") as download:
                sha256, size = digested(download)
                download.seek(0)
                named_script = wheel_read(download, package if mine else None)
            reached = chained(inner, url, "the report's address")
            elsewhere = next((name for name in reached if name != PYPI_HOST), None)
            if elsewhere is not None:
                raise Dropped(f"fetched.json reaches {clipped(elsewhere)}, but a wheel is fetched from {PYPI_HOST} alone")
            listing = unpacked_at(inner)
            under(listing[0], "site")
            return artifact_of(url, "wheel", sha256, size, listing, assets), reached, listing[0], named_script

        artifact, reached, files, named_script = staged(f"wheel {clipped(file)}", one)
        artifacts.append(artifact)
        listings.append(files)
        hosts += [name for name in reached if name not in hosts]
        entry_point = entry_point or named_script
    folded: set[str] = set()
    for files in listings:
        for listed in files:
            if listed["path"].casefold() in folded:
                raise Dropped(f"{clipped(listed['path'])} is listed by two of its artifacts, which the door would refuse")
        folded.update(listed["path"].casefold() for listed in files)
    return {
        "artifacts": artifacts,
        "cmd": cmd,
        "entry_point": entry_point,
        "hosts": hosts,
        "interpreter_args": ["-I"],
        "platform": platform,
    }


def method(value: object, index: int) -> dict:
    """One sign-in method the probe recorded, with its route if it is a
    terminal one: piped unless the agent said it needs a real terminal."""
    value = obj(value, f"sign-in method {index}")
    kind = value.get("kind")
    if kind not in ("agent", "env_var", "terminal", "other"):
        raise Dropped(f"sign-in method {index} is of no kind the probe records")
    held = {
        "id": text(value.get("id"), f"sign-in method {index}'s id"),
        "kind": kind,
        "name": text(value.get("name"), f"sign-in method {index}'s name"),
    }
    if kind == "terminal":
        tty = value.get("tty")
        if not (isinstance(tty, bool) or (isinstance(tty, str) and tty == "unknown")):
            raise Dropped(
                f"sign-in method {index}'s tty is neither true, false nor unknown, "
                "so whether it needs a terminal is not known"
            )
        held.update(
            {
                "args": arguments(value.get("args"), f"sign-in method {index}'s args"),
                "command": relative(value.get("command"), f"sign-in method {index}'s command"),
                "env": variables(value.get("env"), f"sign-in method {index}'s env"),
                "route": "terminal" if tty is True else "piped",
                "tty": tty,
            }
        )
    return held


def probe(value: object, platform: str, runner: dict) -> tuple[dict, list[dict], dict]:
    """The `agentInfo`, sign-in methods and record of one target's probe."""
    value = obj(value, f"the probe output for {platform}")
    if value.get("incomplete") is True:
        raise Dropped(f"the probe output for {platform} is incomplete")
    sandbox = text(value.get("sandbox"), f"the probe's sandbox for {platform}")
    if sandbox != "closed":
        if not sandbox.startswith("unavailable"):
            raise Dropped(f"the probe for {platform} does not say whether its sandbox was closed")
        if runner["confined"]:
            raise Dropped(
                f"{platform} was probed without a sandbox on a runner the matrix lists as confined"
            )
    info = obj(value.get("agentInfo"), f"the probe's agentInfo for {platform}")
    admits = {
        "name": text(info.get("name"), f"the agentInfo name for {platform}"),
        "version": text(info.get("version"), f"the agentInfo version for {platform}"),
    }
    methods = value.get("authMethods")
    if not isinstance(methods, list) or len(methods) > ITEMS:
        raise Dropped(f"the probe for {platform} gives no list of sign-in methods")
    capabilities = value.get("capabilities")
    if len(compact(capabilities)) > CAPABILITY_BYTES:
        raise Dropped(f"the probe's capabilities for {platform} are larger than {CAPABILITY_BYTES} bytes")
    conformance = text(value.get("conformance"), f"the probe's conformance for {platform}")
    if conformance != "not run":
        raise Dropped(
            f"the probe for {platform} reports conformance {clipped(conformance)}, and only "
            "not run is taken in, since no conformance case runs at intake"
        )
    record = {
        "capabilities": capabilities,
        "conformance": conformance,
        "confined": runner["confined"],
        "sandbox": sandbox,
    }
    if "reason" in runner:
        record["reason"] = runner["reason"]
    return admits, [method(item, index) for index, item in enumerate(methods)], record


def entry(
    observed: pathlib.Path,
    agent: dict,
    agent_id: str,
    choice: dict,
    runners: dict,
    allowed: set[str],
    interpreters: dict[str, dict],
    dropped: list[dict],
) -> tuple[dict, dict[str, bytes]]:
    distribution = agent.get("distribution")
    distribution = distribution if isinstance(distribution, dict) else {}
    # A binary wins: `uvx` is read only for an agent with no binary.
    binary = distribution.get("binary")
    uvx = distribution.get("uvx") if binary is None else None
    if isinstance(binary, dict):
        kind = "binary"
        if len(binary) > ITEMS:
            raise Dropped(f"lists more than {ITEMS} binary targets")
    elif isinstance(uvx, dict):
        kind = "uvx"
        package = uvx.get("package")
        pinned = PACKAGE.fullmatch(package) if isinstance(package, str) else None
        if pinned is None or not PINNED.fullmatch(pinned[2]):
            raise Dropped(f"the uvx package {clipped(package)} is not pinned as name==version")
        name, pinned_version = pinned[1], version(pinned[2], "the uvx package's version")
        args = arguments(uvx.get("args"), "args")
        env = variables(uvx.get("env"), "env")
    else:
        raise Dropped("is distributed neither as a binary nor through uvx, the kinds this intake takes in")
    released = version(agent.get("version"), "the version")
    licence = text(agent.get("license"), "the licence")
    reached = reach(choice.get("reach"), "the chosen reach")
    mirrored = licence if licence in allowed else None
    assets: dict[str, bytes] = {}
    targets = []
    for platform, spec in sorted(binary.items()) if kind == "binary" else [(platform, None) for platform in sorted(runners)]:
        try:
            if kind == "binary":
                targets.append(target(observed, agent_id, platform, spec, runners, mirrored, assets))
            else:
                made = uvx_target(observed, agent_id, platform, name, pinned_version, interpreters, assets)
                targets.append({"args": args, "env": env, **made})
        except Exception as failed:  # whatever one target breaks, the others go on
            row = {"platform": clipped(platform), "reason": reason(failed), "registry_id": agent_id}
            if isinstance(failed, Build):
                row["build"] = failed.build
            dropped.append(row)
    if not targets:
        raise Dropped("has no target left to offer")
    probed = []
    for listed in targets:
        platform = listed["platform"]
        try:
            printed = read_json(place_of(observed, agent_id, platform) / "probe.json", PROBE_BYTES, "probe.json")
        except Refused as refused:
            raise Dropped(f"has no probe output for {platform}: {refused}") from None
        admits, methods, record = probe(printed, platform, runners[platform])
        listed["probe"] = record
        probed.append((platform, admits, methods))
    first, admits, methods = probed[0]
    for platform, other_admits, other_methods in probed[1:]:
        differing = differences(
            {"admits": other_admits, "auth_methods": other_methods},
            {"admits": admits, "auth_methods": methods},
        )
        if differing:
            raise Dropped(f"its probes on {first} and {platform} disagree about {named(differing)}")
    return (
        {
            "admits": admits,
            "auth_methods": methods,
            "key": choice["key"],
            "kind": kind,
            "license": licence,
            "reach": reached,
            "registry_id": agent_id,
            "targets": targets,
            "version": released,
        },
        assets,
    )


def reason(failed: Exception) -> str:
    """Why `failed` drops what it was raised over."""
    if isinstance(failed, (Dropped, Refused)):
        return str(failed)
    return f"could not be read here: {type(failed).__name__}: {clipped(failed)}"


def merged_of(path: pathlib.Path | None) -> dict[tuple[str, str], dict]:
    """The entries the merged data holds, by key and version."""
    if path is None:
        return {}
    data = read_json(path, DATA_BYTES, "the merged data")
    entries = data.get("entries") if isinstance(data, dict) else None
    if not isinstance(entries, list) or not all(isinstance(entry, dict) for entry in entries):
        raise Refused("the merged data is not review data as the intake writes it")
    return {(str(entry.get("key")), str(entry.get("version"))): entry for entry in entries}


def choices_of(value: object) -> dict[str, dict]:
    if not isinstance(value, dict):
        raise Refused("the choices are not an object by registry id")
    keys: set[str] = set()
    for agent_id, choice in value.items():
        if not isinstance(choice, dict) or not isinstance(choice.get("key"), str):
            raise Refused(f"the choice for {clipped(agent_id)} names no key")
        key = choice["key"]
        if not AGENT_KEY.fullmatch(key):
            raise Refused(f"{clipped(key)} is not an agent key: 1 to 32 lowercase letters, digits and hyphens")
        if key in keys:
            raise Refused(f"the key {key} is chosen for two agents")
        keys.add(key)
    return value


def runners_of(value: object) -> dict[str, dict]:
    if not isinstance(value, dict):
        raise Refused("the runner matrix is not an object by platform")
    held = {}
    for platform, runner in value.items():
        if not PLATFORM.fullmatch(platform) or not isinstance(runner, dict):
            raise Refused(f"the runner matrix names {clipped(platform)}, which is not a platform")
        confined = runner.get("confined")
        if not isinstance(confined, bool):
            raise Refused(f"the runner for {platform} does not say whether it is confined")
        held[platform] = {"confined": confined}
        if not confined:
            reason = runner.get("reason")
            if not isinstance(reason, str) or not reason:
                raise Refused(f"the runner for {platform} is unconfined and gives no reason")
            held[platform]["reason"] = reason
    return held


def allowed_licences(path: pathlib.Path) -> set[str]:
    try:
        listed = tomllib.loads(read(path, SMALL_BYTES, "deny.toml").decode())["licenses"]["allow"]
    except (tomllib.TOMLDecodeError, UnicodeDecodeError, KeyError, TypeError):
        raise Refused("deny.toml gives no licence list") from None
    if not isinstance(listed, list) or not all(isinstance(item, str) for item in listed):
        raise Refused("deny.toml's licence list is not a list of names")
    return set(listed)


def review(arguments: argparse.Namespace) -> int:
    global BUDGET
    BUDGET = None
    text_read = read(arguments.registry, REGISTRY_BYTES, "the registry")
    registry = parsed(text_read, "the registry")
    agents = registry.get("agents") if isinstance(registry, dict) else None
    if not isinstance(agents, list) or len(agents) > AGENTS:
        raise Refused(f"the registry lists no agents, or more than {AGENTS}")
    choices = choices_of(read_json(arguments.choices, SMALL_BYTES, "the choices"))
    runners = runners_of(read_json(arguments.runners, SMALL_BYTES, "the runner matrix"))
    allowed = allowed_licences(arguments.deny)
    merged = merged_of(arguments.merged)
    interpreters = interpreters_of(arguments.interpreters)
    BUDGET = Budget()
    entries = []
    dropped: list[dict] = []
    assets: dict[str, bytes] = {}
    listed: set[str] = set()
    for index, agent in enumerate(agents):
        agent_id = agent.get("id") if isinstance(agent, dict) else None
        if not isinstance(agent_id, str) or not REGISTRY_ID.fullmatch(agent_id):
            shown_id = agent_id if isinstance(agent_id, str) else f"agents[{index}]"
            dropped.append({"reason": "its registry id cannot name a directory", "registry_id": clipped(shown_id)})
            continue
        if agent_id in listed:
            dropped.append({"reason": "the registry lists it twice", "registry_id": agent_id})
            continue
        listed.add(agent_id)
        choice = choices.get(agent_id)
        if choice is None:
            dropped.append({"reason": "has no key chosen by a reviewer yet", "registry_id": agent_id})
            continue
        try:
            made, kept = entry(arguments.observed, agent, agent_id, choice, runners, allowed, interpreters, dropped)
        except Exception as failed:  # whatever one entry breaks, the others go on
            dropped.append({"reason": reason(failed), "registry_id": agent_id})
            continue
        held = merged.get((made["key"], made["version"]))
        changed = differences(made, held) if held is not None else []
        if changed:
            dropped.append(
                {
                    "reason": f"version {clipped(made['version'])} was already reviewed with other "
                    f"content, and {named(changed)} differ; a changed release is taken in only "
                    "under a new version",
                    "registry_id": agent_id,
                }
            )
            continue
        entries.append(made)
        assets.update(kept)
    BUDGET = None
    data = {
        "dropped": sorted(dropped, key=lambda item: (item["registry_id"], item.get("platform", ""))),
        "entries": sorted(entries, key=lambda item: item["key"]),
        "format": FORMAT,
        "registry": {"sha256": hashlib.sha256(text_read).hexdigest(), "size": len(text_read)},
        "runners": runners,
    }
    held_data = (json.dumps(data, indent=2, sort_keys=True) + "\n").encode()
    if len(held_data) > DATA_BYTES:
        raise Refused(f"the review data would be larger than {DATA_BYTES} bytes, which diff reads")
    arguments.assets.mkdir(parents=True, exist_ok=True)
    for name, listing in sorted(assets.items()):
        (arguments.assets / name).write_bytes(listing)
    written = arguments.out.with_name(arguments.out.name + ".partial")
    written.write_bytes(held_data)
    written.replace(arguments.out)
    return 0


def whole(value: object, what: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= WHOLE:
        raise Refused(f"{what} is not a whole number from 0 to {WHOLE}")
    return value


def revocations(value: object, what: str) -> list[dict]:
    """The revocations `value` lists, each checked as the catalog's reader will."""
    if not isinstance(value, list) or len(value) > REVOKED:
        raise Refused(f"{what} does not list its revocations, or lists more than {REVOKED}")
    held = []
    for item in value:
        item = obj(item, "a revocation")
        key = item.get("key")
        if not isinstance(key, str) or not AGENT_KEY.fullmatch(key):
            raise Refused(f"a revocation in {what} names {clipped(key)}, which is not an agent key")
        try:
            made = {
                "key": key,
                "reason": text(item.get("reason"), "a revocation's reason"),
                "version": version(item.get("version"), "a revocation's version"),
            }
        except Dropped as dropped:
            raise Refused(f"a revocation in {what}: {dropped}") from None
        if item.get("lock_revision") is not None:
            made["lock_revision"] = whole(item["lock_revision"], "a revocation's lock revision")
        held.append(made)
    return held


def whole_numbers(value: object, what: str) -> None:
    """Refuse a float anywhere in `value`: a catalog's numbers are whole."""
    if isinstance(value, float) or (isinstance(value, int) and not 0 <= value <= WHOLE):
        raise Refused(f"{what} holds a number that is not whole, or is outside 0 to {WHOLE}")
    for inner in value.values() if isinstance(value, dict) else value if isinstance(value, list) else ():
        whole_numbers(inner, what)


def present(holder: dict, name: str, kind: type, where: str) -> object:
    held = holder.get(name)
    if not isinstance(held, kind) or isinstance(held, bool):
        raise Refused(f"{where}: {name} is missing or is not a {kind.__name__}")
    return held


def checked(entry: dict) -> None:
    """Refuse an entry the client's reader would refuse for a field a renderer
    can see is missing or malformed, so no catalog is written that every client
    would turn away. The reader still decides the rest."""
    key = entry.get("key")
    if not isinstance(key, str) or not AGENT_KEY.fullmatch(key):
        raise Refused(f"an entry's key {clipped(key)} is not an agent key: 1 to 32 lowercase letters, digits and hyphens")
    where = f"entry {key}"
    try:
        version(entry.get("version"), "the version")
    except Dropped as dropped:
        raise Refused(f"{where}: {dropped}") from None
    for name in ("kind", "registry_id", "license", "review"):
        if not present(entry, name, str, where):
            raise Refused(f"{where}: {name} is empty")
    admits = present(entry, "admits", dict, where)
    for name in ("name", "version"):
        present(admits, name, str, f"{where}: admits")
    for name in ("reach", "sign_in_hosts", "state_paths", "auth_methods"):
        present(entry, name, list, where)
    targets = present(entry, "targets", list, where)
    if not targets:
        raise Refused(f"{where}: targets is empty, so nothing could be offered")
    for target in targets:
        if not isinstance(target, dict):
            raise Refused(f"{where}: a target is not an object")
        for name in ("platform", "cmd"):
            present(target, name, str, f"{where}: a target")
        present(target, "hosts", list, f"{where}: a target")
        if entry["kind"] == "uvx":
            started = present(target, "interpreter_args", list, f"{where}: a target")
            if not all(isinstance(item, str) for item in started):
                raise Refused(f"{where}: a target's interpreter_args is not a list of text")
            module, colon, attr = present(target, "entry_point", str, f"{where}: a target").partition(":")
            if not colon or not module or not attr:
                raise Refused(f"{where}: a target's entry_point is not module:attr")
        artifacts = present(target, "artifacts", list, f"{where}: a target")
        if not artifacts:
            raise Refused(f"{where}: a target's artifacts is empty, so nothing would be pinned")
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                raise Refused(f"{where}: an artifact is not an object")
            for name in ("url", "sha256", "format"):
                present(artifact, name, str, f"{where}: an artifact")
            for name in ("size", "unpacked", "decompressed"):
                present(artifact, name, int, f"{where}: an artifact")


def rendered(entries: list[dict]) -> list[dict]:
    """The entries as the catalog lists them: what only a reviewer reads is left out."""
    listed = []
    for entry in entries:
        copy = dict(entry)
        copy["targets"] = [
            {name: value for name, value in target.items() if name != "probe"}
            for target in copy.get("targets", [])
        ]
        for target in copy["targets"]:
            target["artifacts"] = [
                {name: value for name, value in artifact.items() if name != "mirror_licence"}
                for artifact in target.get("artifacts", [])
            ]
        listed.append(copy)
    return listed


def render(arguments: argparse.Namespace) -> int:
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    entries = merged.get("entries") if isinstance(merged, dict) else None
    if not isinstance(entries, list) or not all(isinstance(item, dict) for item in entries):
        raise Refused("the merged data is not review data as the intake writes it")
    try:
        entries = rendered(entries)
    except (AttributeError, TypeError):
        raise Refused("the merged data is not review data as the intake writes it") from None
    whole_numbers(entries, "the merged data")
    for item in entries:
        checked(item)
    names = [(entry.get("key"), entry.get("version")) for entry in entries]
    if len(set(map(str, names))) != len(names):
        raise Refused("the merged data holds one version of an agent twice")
    revoked = revocations(merged.get("revoked", []), "the merged data")
    serial = 0
    signed_at = whole(arguments.signed_at, "--signed-at")
    if arguments.previous is not None:
        before = obj(read_json(arguments.previous, INDEX_BYTES, "the previous catalog"), "the previous catalog")
        if before.get("schema") != SCHEMA or isinstance(before.get("schema"), bool):
            raise Refused(f"the previous catalog is not in schema {SCHEMA}, the one this renders")
        serial = whole(before.get("serial"), "the previous catalog's serial")
        if serial < 1:
            raise Refused("the previous catalog's serial is below 1, the lowest a catalog has")
        if signed_at < whole(before.get("signed_at"), "the previous catalog's signed_at"):
            raise Refused("--signed-at is earlier than the previous catalog's signed_at")
        if "revoked" not in before:
            raise Refused("the previous catalog lists no revocations, so earlier ones could be dropped unseen")
        revoked = revocations(before["revoked"], "the previous catalog") + revoked
    seen: set[tuple] = set()
    accumulated = []
    for item in revoked:
        identity = (item["key"], item["version"], item.get("lock_revision"))
        if identity not in seen:
            seen.add(identity)
            accumulated.append(item)
    if len(accumulated) > REVOKED:
        raise Refused(f"the catalog would carry {len(accumulated)} revocations, more than the {REVOKED} a client keeps")
    catalog = {
        "entries": entries,
        "expires": whole(signed_at + LIFETIME_SECONDS, "the expiry"),
        "revoked": sorted(accumulated, key=lambda item: (item["key"], item["version"], item.get("lock_revision", -1))),
        "schema": SCHEMA,
        "serial": whole(serial + 1, "the serial"),
        "signed_at": signed_at,
    }
    held = json.dumps(catalog, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
    try:
        held_bytes = held.encode()
    except UnicodeEncodeError:
        raise Refused("the catalog holds text UTF-8 cannot carry") from None
    if len(held_bytes) > INDEX_BYTES:
        raise Refused(f"the catalog would be {len(held_bytes)} bytes, more than the {INDEX_BYTES} a client reads")
    written = arguments.out.with_name(arguments.out.name + ".partial")
    try:
        written.write_bytes(held_bytes)
        written.replace(arguments.out)
    except OSError as failed:
        written.unlink(missing_ok=True)
        raise Refused(f"the catalog could not be written to {clipped(arguments.out)}: {clipped(failed.strerror)}") from None
    return 0


def published(merged: dict) -> tuple[dict[str, int], dict[str, bool]]:
    """What the merged data lets the catalog release hold, by hash: each
    file-list asset with its size, and each artifact with whether it may be
    copied (a mirror mark and a passing licence-gate record, on every entry
    that holds those bytes)."""
    listings: dict[str, int] = {}
    artifacts: dict[str, bool] = {}
    try:
        for entry in merged["entries"]:
            for target in entry.get("targets", []):
                for artifact in target.get("artifacts", []):
                    record = artifact.get("licence_gate")
                    sha = digest(artifact.get("sha256"), "an artifact's sha256")
                    passed = (
                        isinstance(record, dict)
                        and record.get("passed") is True
                        and isinstance(artifact.get("mirror_licence"), str)
                    )
                    # The same bytes under two entries pass only where both do.
                    artifacts[sha] = passed and artifacts.get(sha, True)
                    files = artifact.get("files")
                    if isinstance(files, dict):
                        listings[digest(files.get("sha256"), "a file list's sha256")] = number(
                            files.get("size"), "a file list's size"
                        )
    except (KeyError, TypeError, AttributeError, Dropped):
        raise Refused("the merged data is not review data as the intake writes it") from None
    return listings, artifacts


def publish_check(arguments: argparse.Namespace) -> int:
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    if not isinstance(merged, dict):
        raise Refused("the merged data is not review data as the intake writes it")
    listings, artifacts = published(merged)
    try:
        held = os.lstat(arguments.assets)
    except OSError:
        raise Refused("the assets directory is missing") from None
    if not stat.S_ISDIR(held.st_mode):
        raise Refused("the assets are not a directory")
    for place in sorted(arguments.assets.iterdir()):
        name = clipped(place.name)
        if not SHA256.fullmatch(place.name):
            raise Refused(f"asset {name} is not named by a lower-case SHA-256 with no extension")
        try:
            actual, size = hashed(place)
        except Dropped as failed:
            raise Refused(f"asset {name} {failed}") from None
        if actual != place.name:
            raise Refused(f"asset {name} is not the bytes its name claims")
        if place.name not in listings and place.name not in artifacts:
            raise Refused(f"asset {name} is not in the merged data")
        # Independent: a hash the data names both ways answers to both.
        if place.name in listings and size != listings[place.name]:
            raise Refused(f"asset {name} is not the size the merged data records for it")
        if place.name in artifacts and not artifacts[place.name]:
            raise Refused(
                f"asset {name} is a copy of an artifact whose entry lacks a mirror mark "
                "or a passing licence-gate record"
            )
    return 0


# Characters that draw nothing, or move or break the line, beyond what the
# Unicode categories below already name: variation selectors, the combining
# grapheme joiner, and letters that draw as blanks.
INVISIBLE = frozenset(
    [0x034F, 0x115F, 0x1160, 0x2800, 0x3164, 0xFFA0]
    + list(range(0xFE00, 0xFE10))
    + list(range(0xE0100, 0xE01F0))
)


def hidden(character: str) -> bool:
    """Whether `character` could hide or reorder what the diff shows: any
    control, format (bidi and zero-width ones among them), surrogate, private,
    unassigned, line or paragraph separator, or space other than the ASCII one."""
    category = unicodedata.category(character)
    return (
        category in ("Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp")
        or (category == "Zs" and character != " ")
        or ord(character) in INVISIBLE
    )


def shown(value: object) -> str:
    """Hostile `value`, whole, as a code span it cannot close, on one line.

    No mention, link or markup in it is read as one, and every character that
    could hide or reorder it is written as its code point, as is a backslash,
    so the escapes cannot be imitated."""
    held = "".join(
        "\\\\"
        if c == "\\"
        else (f"\\u{ord(c):04x}" if ord(c) <= 0xFFFF else f"\\U{ord(c):08x}")
        if hidden(c)
        else c
        for c in str(value)
    )
    longest = max((len(run) for run in re.findall(r"`+", held)), default=0)
    fence = "`" * (longest + 1)
    return f"{fence} {held} {fence}"


def words(values: object, empty: str = "none") -> str:
    if not isinstance(values, list) or not values:
        return empty if values in (None, [], {}) else shown(values)
    return " ".join(shown(value) for value in values)


def setting(env: object) -> str:
    if not isinstance(env, dict):
        return shown(env)
    return words([f"{name}={value}" for name, value in env.items()])


def files_of(files: object) -> str:
    if isinstance(files, dict):
        return f"a file-list asset {shown(files.get('sha256'))} of {shown(files.get('size'))} bytes"
    return f"{len(files)} listed" if isinstance(files, list) else "none"


def capabilities_of(value: object) -> str:
    return shown(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


# What `compared` is given when there is nothing before to compare with.
NEW = object()


def compared(line: str, new: object, old: object, render) -> list[str]:
    """`line` showing `new` whole, and what it was where it changed; nothing
    where it did not. The values are compared, never what is shown of them."""
    if old is NEW:
        return [f"- {line}: {render(new)}"]
    if differences(new, old) == []:
        return []
    return [f"- {line}: {render(new)} (was {render(old)})"]


def changed_files(new: object, old: object) -> list[str]:
    if not isinstance(new, list) or not isinstance(old, list):
        return []
    before = {str(file.get("path")): file for file in old if isinstance(file, dict)}
    after = {str(file.get("path")): file for file in new if isinstance(file, dict)}
    lines = [f"  - added {shown(path)}" for path in after if path not in before]
    lines += [f"  - removed {shown(path)}" for path in before if path not in after]
    lines += [f"  - changed {shown(path)}" for path in after if path in before and after[path] != before[path]]
    if len(lines) > SHOWN_FILES:
        lines = lines[:SHOWN_FILES] + [f"  - and {len(lines) - SHOWN_FILES} more"]
    return lines


def probe_line(platform: object, record: dict) -> str:
    conformance = shown(record.get("conformance"))
    if record.get("sandbox") == "closed":
        return f"- {shown(platform)} probe: sandbox closed, conformance {conformance}"
    return (
        f"- {shown(platform)} probe: **probed without a sandbox**; the runner matrix lists it "
        f"unconfined: {shown(record.get('reason'))}. The probe said {shown(record.get('sandbox'))}, "
        f"conformance {conformance}; decide whether to admit this target."
    )


def method_line(method: dict) -> str:
    line = f"- sign-in {shown(method.get('id'))}, {shown(method.get('kind'))}: {shown(method.get('name'))}"
    if method.get("kind") == "terminal":
        how = "piped" if method.get("route") == "piped" else "on a real terminal, unconfined"
        if method.get("tty") == "unknown":
            how += " (the agent did not say; piped is the default)"
        line += (
            f"; runs {shown(method.get('command'))} {words(method.get('args', []), '')} "
            f"with {setting(method.get('env', {}))}, {how}"
        )
    return line


ARTIFACT_FIELDS = ("url", "format", "size", "sha256", "unpacked", "decompressed", "mirror_licence")


def artifact_lines(artifact: dict, old: dict | None) -> list[str]:
    if old is None:
        return [
            f"- artifact {shown(artifact.get('url'))}: {shown(artifact.get('format'))}, "
            f"{shown(artifact.get('size'))} bytes, sha256 {shown(artifact.get('sha256'))}, "
            f"unpacks {shown(artifact.get('unpacked'))} bytes, files {files_of(artifact.get('files'))}"
            + (f", mirror licence {shown(artifact['mirror_licence'])}" if "mirror_licence" in artifact else "")
        ]
    lines = []
    for field in ARTIFACT_FIELDS:
        lines += compared(f"artifact {field}", artifact.get(field), old.get(field), shown)
    lines += compared("artifact files", artifact.get("files"), old.get("files"), files_of)
    return lines + changed_files(artifact.get("files"), old.get("files"))


def entry_lines(entry: dict, previous: dict | None, same: bool) -> list[str]:
    if same:
        was = " (**same version, content changed**: a release already reviewed differs as below)"
    elif previous:
        was = f" (was {shown(previous.get('version'))})"
    else:
        was = " (new agent)"
    lines = [f"## {shown(entry.get('key'))} {shown(entry.get('version'))}{was}", ""]
    before = previous or {}

    def against(field: str) -> object:
        return before.get(field) if previous is not None else NEW

    mirrored = any(
        "mirror_licence" in artifact
        for listed in entry.get("targets", [])
        for artifact in listed.get("artifacts", [])
    )
    lines += compared("registry id", entry.get("registry_id"), against("registry_id"), shown)
    lines += compared("licence", entry.get("license"), against("license"), shown)
    uvx = entry.get("kind") == "uvx"
    if uvx:
        lines.append("- not mirrored: an interpreter and its wheels are never mirrored")
    else:
        lines.append("- " + ("marked for mirroring" if mirrored else "not mirrored: its licence is not on deny.toml's list"))
    lines += compared(
        "answers initialize as",
        entry.get("admits"),
        against("admits"),
        lambda admits: f"{shown(admits.get('name'))} {shown(admits.get('version'))}" if isinstance(admits, dict) else shown(admits),
    )
    lines += compared("reach", entry.get("reach", []), against("reach"), words)
    if previous is None or differences(entry.get("auth_methods"), before.get("auth_methods")):
        lines += [method_line(method) for method in entry.get("auth_methods", [])]
        if previous is not None:
            lines.append("- the sign-in methods above changed")
    old_targets = {listed.get("platform"): listed for listed in before.get("targets", [])}
    for listed in entry.get("targets", []):
        platform = listed.get("platform")
        old = old_targets.get(platform)
        if old is not None and differences(listed, old) == []:
            continue
        fresh = previous is not None and old is None
        lines += ["", f"### {shown(platform)}" + (" (new target)" if fresh else ""), ""]
        prior = old if old is not None else {}

        def was_of(field: str) -> object:
            return prior.get(field) if old is not None else NEW

        record = listed.get("probe", {})
        if old is None or differences(record, prior.get("probe")):
            lines.append(probe_line(platform, record))
        lines += compared("capabilities", record.get("capabilities"), prior.get("probe", {}).get("capabilities") if old is not None else NEW, capabilities_of)
        lines += compared("cmd", listed.get("cmd"), was_of("cmd"), shown)
        if uvx:
            lines += compared("interpreter args", listed.get("interpreter_args", []), was_of("interpreter_args"), words)
            lines += compared("entry point", listed.get("entry_point"), was_of("entry_point"), shown)
        lines += compared("args", listed.get("args", []), was_of("args"), words)
        lines += compared("env", listed.get("env", {}), was_of("env"), setting)
        lines += compared("hosts", listed.get("hosts", []), was_of("hosts"), words)
        old_artifacts = prior.get("artifacts", [])
        for index, artifact in enumerate(listed.get("artifacts", [])):
            old_artifact = old_artifacts[index] if index < len(old_artifacts) else None
            lines += artifact_lines(artifact, old_artifact)
    lines.append("")
    return lines


def diff(arguments: argparse.Namespace) -> int:
    new = read_json(arguments.review, DATA_BYTES, "the review data")
    old = read_json(arguments.old, DATA_BYTES, "the merged data") if arguments.old else {"entries": []}
    try:
        merged = [entry for entry in old["entries"] if isinstance(entry, dict)]
        held = {(entry.get("key"), entry.get("version")): entry for entry in merged}
        lines = ["# Agent catalog review", ""]
        registry = new.get("registry", {})
        lines += [
            f"Registry read: sha256 {shown(registry.get('sha256'))}, {shown(registry.get('size'))} bytes.",
            "",
        ]
        for entry in new["entries"]:
            reviewed = held.get((entry.get("key"), entry.get("version")))
            if reviewed is not None:
                if differences(entry, reviewed) == []:
                    continue
                lines += entry_lines(entry, reviewed, same=True)
                continue
            previous = next(
                (old_entry for old_entry in reversed(merged) if old_entry.get("key") == entry.get("key")),
                None,
            )
            lines += entry_lines(entry, previous, same=False)
        lines += ["## Dropped", ""]
        for item in new["dropped"]:
            where = f" on {shown(item['platform'])}" if item.get("platform") else ""
            lines.append(f"- {shown(item.get('registry_id'))}{where}: {shown(item.get('reason'))}")
    except (KeyError, TypeError, AttributeError):
        raise Refused("the review data or the merged data is not review data as the intake writes it") from None
    sys.stdout.buffer.write(("\n".join(lines) + "\n").encode())
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    reviewing = commands.add_parser("review", help="turn registry binary and uvx entries into review data")
    for name in ("registry", "choices", "runners", "observed", "deny", "assets", "out"):
        reviewing.add_argument(f"--{name}", type=pathlib.Path, required=True)
    reviewing.add_argument(
        "--merged", type=pathlib.Path, help="the merged data, against which a version already reviewed is held"
    )
    reviewing.add_argument(
        "--interpreters", type=pathlib.Path, help="the interpreter pinned for each platform a uvx agent runs on"
    )
    differing = commands.add_parser("diff", help="the human diff of review data against merged data")
    differing.add_argument("--old", type=pathlib.Path, help="the merged data before this review")
    differing.add_argument("review", type=pathlib.Path)
    rendering = commands.add_parser("render", help="render merged data to the catalog the publishing job signs")
    rendering.add_argument("--merged", type=pathlib.Path, required=True)
    rendering.add_argument("--previous", type=pathlib.Path, help="the catalog published before this one")
    rendering.add_argument("--signed-at", type=int, required=True, help="the signing time, in whole Unix seconds")
    rendering.add_argument("--out", type=pathlib.Path, required=True)
    checking = commands.add_parser("publish-check", help="refuse an asset the merged data does not allow uploaded")
    checking.add_argument("--merged", type=pathlib.Path, required=True)
    checking.add_argument("--assets", type=pathlib.Path, required=True, help="the directory of assets to upload")
    arguments = parser.parse_args()
    commands_run = {"review": review, "diff": diff, "render": render, "publish-check": publish_check}
    try:
        return commands_run[arguments.command](arguments)
    except Refused as refused:
        print(f"agent-catalog-intake: {refused}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
