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
number, and shows nothing of it in the human diff but inside a code span it
cannot close.

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
  first), `unpack-check.json` and `probe.json`;
- `--deny`: `deny.toml`, whose licence list decides what may be mirrored.

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
unconfined, and is marked for the reviewer. Each drop is recorded with its
reason. An artifact whose declared licence is on `deny.toml`'s list is marked
with that licence for mirroring; nothing is mirrored until the licence gate
has passed it.

`diff` writes the human diff of review data against the merged data before it,
as Markdown, showing only what changed.
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
import tomllib
import urllib.parse

# The ceilings of what is read, each before it is parsed.
REGISTRY_BYTES = 1 << 20
AGENTS = 512
ARTIFACT_BYTES = 1 << 30
UNPACK_CHECK_BYTES = 16 << 20
PROBE_BYTES = 1 << 20
SMALL_BYTES = 1 << 20
DATA_BYTES = 64 << 20
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
# How much of one hostile text, and of one file list, the diff shows.
SHOWN_CHARS = 200
SHOWN_FILES = 50

PLATFORM = re.compile(r"(linux|darwin|windows|freebsd)-(x86_64|aarch64)")
REGISTRY_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
AGENT_KEY = re.compile(r"[a-z0-9-]{1,32}")
LABEL = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?")
SHA256 = re.compile(r"[0-9a-f]{64}")


class Refused(Exception):
    """An input the whole run cannot go on from."""


class Dropped(Exception):
    """A target or an entry that is left out, and why."""


def clipped(text: object) -> str:
    """Hostile `text`, cut to what a reason quotes."""
    text = str(text)
    return text if len(text) <= SHOWN_CHARS else text[:SHOWN_CHARS] + "..."


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
    if len(text) > ceiling:
        raise Refused(f"{what} is larger than {ceiling} bytes")
    return text


def read_json(path: pathlib.Path, ceiling: int, what: str) -> object:
    return parsed(read(path, ceiling, what), what)


def obj(value: object, what: str) -> dict:
    if not isinstance(value, dict):
        raise Dropped(f"{what} is not an object")
    return value


def text(value: object, what: str) -> str:
    """Non-empty text of no control character, within its ceiling."""
    if not isinstance(value, str) or not value:
        raise Dropped(f"{what} is not text")
    if len(value.encode()) > TEXT_BYTES or any(ord(c) < 0x20 or ord(c) == 0x7F for c in value):
        raise Dropped(f"{what} is not text a catalog holds")
    return value


def argument(value: object, what: str) -> str:
    """An argument or a variable's value, which may be empty."""
    if not isinstance(value, str) or "\0" in value or len(value.encode()) > TEXT_BYTES:
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
        if not name or "=" in name or "\0" in name or len(name.encode()) > TEXT_BYTES:
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
    parts = urllib.parse.urlsplit(url)
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
    with open(path, "rb") as file:
        while chunk := file.read(1 << 20):
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
    place = observed / agent / platform
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
        if tty not in (True, False, "unknown"):
            raise Dropped(f"sign-in method {index} does not say whether it needs a terminal")
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
    record = {
        "capabilities": capabilities,
        "conformance": text(value.get("conformance"), f"the probe's conformance for {platform}"),
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
        except Dropped as why:
            dropped.append({"platform": clipped(platform), "reason": str(why), "registry_id": agent_id})
    if not targets:
        raise Dropped("has no target left to offer")
    probed = []
    for listed in targets:
        platform = listed["platform"]
        try:
            printed = read_json(observed / agent_id / platform / "probe.json", PROBE_BYTES, "probe.json")
        except Refused as refused:
            raise Dropped(f"has no probe output for {platform}: {refused}") from None
        admits, methods, record = probe(printed, platform, runners[platform])
        listed["probe"] = record
        probed.append((platform, admits, methods))
    first, admits, methods = probed[0]
    for platform, other_admits, other_methods in probed[1:]:
        if (other_admits, other_methods) != (admits, methods):
            raise Dropped(f"its probes on {first} and {platform} disagree about agentInfo or sign-in methods")
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
    text_read = read(arguments.registry, REGISTRY_BYTES, "the registry")
    registry = parsed(text_read, "the registry")
    agents = registry.get("agents") if isinstance(registry, dict) else None
    if not isinstance(agents, list) or len(agents) > AGENTS:
        raise Refused(f"the registry lists no agents, or more than {AGENTS}")
    choices = choices_of(read_json(arguments.choices, SMALL_BYTES, "the choices"))
    runners = runners_of(read_json(arguments.runners, SMALL_BYTES, "the runner matrix"))
    allowed = allowed_licences(arguments.deny)
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
        except Dropped as why:
            dropped.append({"reason": str(why), "registry_id": agent_id})
            continue
        entries.append(made)
        assets.update(kept)
    data = {
        "dropped": sorted(dropped, key=lambda item: (item["registry_id"], item.get("platform", ""))),
        "entries": sorted(entries, key=lambda item: item["key"]),
        "format": FORMAT,
        "registry": {"sha256": hashlib.sha256(text_read).hexdigest(), "size": len(text_read)},
        "runners": runners,
    }
    arguments.assets.mkdir(parents=True, exist_ok=True)
    for named, listing in sorted(assets.items()):
        (arguments.assets / named).write_bytes(listing)
    written = arguments.out.with_name(arguments.out.name + ".partial")
    written.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
    written.replace(arguments.out)
    return 0


def shown(value: object) -> str:
    """Hostile `value` as a code span it cannot close, on one line: no mention,
    link or markup in it is read as one."""
    held = clipped(value)
    held = "".join(
        f"\\u{ord(c):04x}"
        if ord(c) < 0x20 or ord(c) == 0x7F or 0x2028 <= ord(c) <= 0x202E or 0x2066 <= ord(c) <= 0x2069
        else c
        for c in held
    )
    longest = max((len(run) for run in re.findall(r"`+", held)), default=0)
    fence = "`" * (longest + 1)
    return f"{fence} {held} {fence}"


def words(values: list, empty: str = "none") -> str:
    return " ".join(shown(value) for value in values) if values else empty


def setting(env: dict) -> str:
    return words([f"{name}={value}" for name, value in env.items()])


def files_of(artifact: dict) -> str:
    files = artifact.get("files")
    if isinstance(files, dict):
        return f"a file-list asset {shown(files.get('sha256'))} of {shown(files.get('size'))} bytes"
    return f"{len(files)} listed" if isinstance(files, list) else "none"


def compared(line: str, new: str, old: str | None) -> list[str]:
    """`line` showing `new`, with what it was where it changed, and nothing
    where it did not."""
    if old is None:
        return [f"- {line}: {new}"]
    if old == new:
        return []
    return [f"- {line}: {new} (was {old})"]


def changed_files(new: object, old: object) -> list[str]:
    if not isinstance(new, list) or not isinstance(old, list):
        return []
    before = {file.get("path"): file for file in old if isinstance(file, dict)}
    after = {file.get("path"): file for file in new if isinstance(file, dict)}
    lines = [f"  - added {shown(path)}" for path in after if path not in before]
    lines += [f"  - removed {shown(path)}" for path in before if path not in after]
    lines += [f"  - changed {shown(path)}" for path in after if path in before and after[path] != before[path]]
    if len(lines) > SHOWN_FILES:
        lines = lines[:SHOWN_FILES] + [f"  - and {len(lines) - SHOWN_FILES} more"]
    return lines


def probe_line(platform: str, record: dict) -> str:
    if record.get("sandbox") == "closed":
        return f"- {shown(platform)} probe: sandbox closed, conformance {shown(record.get('conformance'))}"
    return (
        f"- {shown(platform)} probe: **probed without a sandbox**; the runner matrix lists it "
        f"unconfined: {shown(record.get('reason'))}. The probe said {shown(record.get('sandbox'))}; "
        f"decide whether to admit this target."
    )


def method_line(method: dict) -> str:
    line = f"- sign-in {shown(method.get('id'))}, {shown(method.get('kind'))}: {shown(method.get('name'))}"
    if method.get("kind") == "terminal":
        route = method.get("route")
        how = "piped" if route == "piped" else "on a real terminal, unconfined"
        if method.get("tty") == "unknown":
            how += " (the agent did not say; piped is the default)"
        line += (
            f"; runs {shown(method.get('command'))} {words(method.get('args', []), '')} "
            f"with {setting(method.get('env', {}))}, {how}"
        )
    return line


def entry_lines(entry: dict, previous: dict | None) -> list[str]:
    was = f" (was {shown(previous.get('version'))})" if previous else " (new agent)"
    lines = [f"## {shown(entry.get('key'))} {shown(entry.get('version'))}{was}", ""]
    before = previous or {}
    licence = entry.get("license")
    mirrored = any(
        "mirror_licence" in artifact
        for listed in entry.get("targets", [])
        for artifact in listed.get("artifacts", [])
    )
    lines.append(f"- registry id: {shown(entry.get('registry_id'))}")
    lines.append(
        f"- licence: {shown(licence)}, "
        + ("marked for mirroring" if mirrored else "not mirrored: not on deny.toml's list")
    )
    admits = entry.get("admits", {})
    lines += compared(
        "answers initialize as",
        f"{shown(admits.get('name'))} {shown(admits.get('version'))}",
        None if previous is None else f"{shown(before.get('admits', {}).get('name'))} {shown(before.get('admits', {}).get('version'))}",
    )
    lines += compared(
        "reach",
        words(entry.get("reach", [])),
        None if previous is None else words(before.get("reach", [])),
    )
    old_methods = before.get("auth_methods")
    if previous is None or old_methods != entry.get("auth_methods"):
        lines += [method_line(method) for method in entry.get("auth_methods", [])]
        if previous is not None:
            lines.append("- the sign-in methods above changed")
    old_targets = {listed.get("platform"): listed for listed in before.get("targets", [])}
    for listed in entry.get("targets", []):
        platform = listed.get("platform")
        old = old_targets.get(platform) if previous is not None else None
        fresh = previous is not None and old is None
        lines += ["", f"### {shown(platform)}" + (" (new target)" if fresh else ""), ""]
        lines.append(probe_line(platform, listed.get("probe", {})))
        prior = old or {}
        unseen = old is None
        lines += compared("cmd", shown(listed.get("cmd")), None if unseen else shown(prior.get("cmd")))
        lines += compared("args", words(listed.get("args", [])), None if unseen else words(prior.get("args", [])))
        lines += compared("env", setting(listed.get("env", {})), None if unseen else setting(prior.get("env", {})))
        lines += compared("hosts", words(listed.get("hosts", [])), None if unseen else words(prior.get("hosts", [])))
        old_artifacts = prior.get("artifacts", [])
        for index, artifact in enumerate(listed.get("artifacts", [])):
            old_artifact = old_artifacts[index] if index < len(old_artifacts) else None
            lines.append(
                f"- artifact {shown(artifact.get('url'))}: {shown(artifact.get('format'))}, "
                f"{shown(artifact.get('size'))} bytes, sha256 {shown(artifact.get('sha256'))}, "
                f"unpacks {shown(artifact.get('unpacked'))} bytes, files {files_of(artifact)}"
            )
            if old_artifact is not None:
                lines += changed_files(artifact.get("files"), old_artifact.get("files"))
    lines.append("")
    return lines


def diff(arguments: argparse.Namespace) -> int:
    new = read_json(arguments.review, DATA_BYTES, "the review data")
    old = read_json(arguments.old, DATA_BYTES, "the merged data") if arguments.old else {"entries": []}
    try:
        merged = [entry for entry in old["entries"] if isinstance(entry, dict)]
        held = {(entry.get("key"), entry.get("version")) for entry in merged}
        lines = ["# Agent catalog review", ""]
        registry = new.get("registry", {})
        lines += [
            f"Registry read: sha256 {shown(registry.get('sha256'))}, {shown(registry.get('size'))} bytes.",
            "",
        ]
        for entry in new["entries"]:
            if (entry.get("key"), entry.get("version")) in held:
                continue
            previous = next(
                (old_entry for old_entry in reversed(merged) if old_entry.get("key") == entry.get("key")),
                None,
            )
            lines += entry_lines(entry, previous)
        lines += ["## Dropped", ""]
        for item in new["dropped"]:
            where = f" on {shown(item['platform'])}" if item.get("platform") else ""
            lines.append(f"- {shown(item.get('registry_id'))}{where}: {shown(item.get('reason'))}")
    except (KeyError, TypeError, AttributeError):
        raise Refused("the review data or the merged data is not review data as the intake writes it") from None
    sys.stdout.write("\n".join(lines) + "\n")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    reviewing = commands.add_parser("review", help="turn registry binary entries into review data")
    for name in ("registry", "choices", "runners", "observed", "deny", "assets", "out"):
        reviewing.add_argument(f"--{name}", type=pathlib.Path, required=True)
    differing = commands.add_parser("diff", help="the human diff of review data against merged data")
    differing.add_argument("--old", type=pathlib.Path, help="the merged data before this review")
    differing.add_argument("review", type=pathlib.Path)
    arguments = parser.parse_args()
    try:
        return review(arguments) if arguments.command == "review" else diff(arguments)
    except Refused as refused:
        print(f"agent-catalog-intake: {refused}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
