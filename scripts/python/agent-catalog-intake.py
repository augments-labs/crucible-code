#!/usr/bin/env -S python3 -I
"""The agent catalog's intake: registry `binary` entries become review data.

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
  link;
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
import hashlib
import json
import os
import pathlib
import re
import stat
import sys
import time
import tomllib
import unicodedata
import urllib.parse

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

PLATFORM = re.compile(r"(linux|darwin|windows|freebsd)-(x86_64|aarch64)")
REGISTRY_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
AGENT_KEY = re.compile(r"[a-z0-9-]{1,32}")
LABEL = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?")
SHA256 = re.compile(r"[0-9a-f]{64}")


class Refused(Exception):
    """An input the whole run cannot go on from."""


class Dropped(Exception):
    """A target or an entry that is left out, and why."""


class Spent(Dropped):
    """The run's budget is spent, so what is left waits for the next run."""


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


def hashed(path: pathlib.Path) -> tuple[str, int]:
    """The SHA-256 and size of the download at `path`, read within its ceiling."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Dropped("could not be hashed: nothing was downloaded") from None
    if not stat.S_ISREG(held.st_mode):
        raise Dropped("could not be hashed: the download is not a regular file")
    sha = hashlib.sha256()
    size = 0
    try:
        # Opened without following a link, so a path swapped for one after the
        # check above reads nothing.
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError:
        raise Dropped("could not be hashed: the download is not a regular file") from None
    with os.fdopen(descriptor, "rb") as file:
        if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
            raise Dropped("could not be hashed: the download is not a regular file")
        while chunk := file.read(1 << 20):
            spend(len(chunk))
            size += len(chunk)
            if size > ARTIFACT_BYTES:
                raise Dropped(f"could not be hashed: the download is larger than {ARTIFACT_BYTES} bytes")
            sha.update(chunk)
    return sha.hexdigest(), size


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


def place_of(observed: pathlib.Path, agent: str, platform: str) -> pathlib.Path:
    """Where the untrusted job left what it observed of one target, refused
    where a directory on the way there is a link, which could reach anywhere."""
    place = observed
    for part in (agent, platform):
        place = place / part
        try:
            held = os.lstat(place)
        except OSError:
            return observed / agent / platform
        if not stat.S_ISDIR(held.st_mode):
            raise Dropped(f"{clipped(part)} is not a directory of the observed tree")
    return place


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
    try:
        fetched = obj(read_json(place / "fetched.json", SMALL_BYTES, "fetched.json"), "fetched.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    chain = fetched.get("chain")
    if not isinstance(chain, list) or not chain or len(chain) > ITEMS:
        raise Dropped("fetched.json gives no chain of addresses")
    if chain[0] != url:
        raise Dropped("fetched.json does not begin at the registry's address")
    hosts: list[str] = []
    for hop in chain:
        _, name = address(hop, "the redirect to")
        if name not in hosts:
            hosts.append(name)
    try:
        printed = read_json(place / "unpack-check.json", UNPACK_CHECK_BYTES, "unpack-check.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    files, unpacked, decompressed = file_list(printed)
    program = next((file for file in files if file["path"] == cmd), None)
    if program is None or program["kind"] != "file":
        raise Dropped(f"cmd {clipped(cmd)} is not a file the archive unpacks to")
    if not program["exec"] and not platform.startswith("windows-"):
        raise Dropped(f"cmd {clipped(cmd)} is not runnable")
    artifact: dict = {
        "decompressed": decompressed,
        "format": format,
        "sha256": sha256,
        "size": size,
        "unpacked": unpacked,
        "url": url,
    }
    if len(files) > LISTED_FILES:
        listing = compact(files)
        if len(listing) > ASSET_BYTES:
            raise Dropped(f"its file list is larger than a catalog asset may be, {ASSET_BYTES} bytes")
        named = hashlib.sha256(listing).hexdigest()
        assets[named] = listing
        artifact["files"] = {"sha256": named, "size": len(listing)}
    else:
        artifact["files"] = files
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
    dropped: list[dict],
) -> tuple[dict, dict[str, bytes]]:
    distribution = agent.get("distribution")
    binary = distribution.get("binary") if isinstance(distribution, dict) else None
    if not isinstance(binary, dict):
        raise Dropped("is not distributed as a binary, the one kind this intake takes in")
    if len(binary) > ITEMS:
        raise Dropped(f"lists more than {ITEMS} binary targets")
    released = version(agent.get("version"), "the version")
    licence = text(agent.get("license"), "the licence")
    reached = reach(choice.get("reach"), "the chosen reach")
    mirrored = licence if licence in allowed else None
    assets: dict[str, bytes] = {}
    targets = []
    for platform, spec in sorted(binary.items()):
        try:
            targets.append(target(observed, agent_id, platform, spec, runners, mirrored, assets))
        except Exception as failed:  # whatever one target breaks, the others go on
            dropped.append({"platform": clipped(platform), "reason": reason(failed), "registry_id": agent_id})
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
            "kind": "binary",
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
            made, kept = entry(arguments.observed, agent, agent_id, choice, runners, allowed, dropped)
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
    reviewing = commands.add_parser("review", help="turn registry binary entries into review data")
    for name in ("registry", "choices", "runners", "observed", "deny", "assets", "out"):
        reviewing.add_argument(f"--{name}", type=pathlib.Path, required=True)
    reviewing.add_argument(
        "--merged", type=pathlib.Path, help="the merged data, against which a version already reviewed is held"
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
