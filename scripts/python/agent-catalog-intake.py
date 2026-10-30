#!/usr/bin/env -S python3 -I
"""The agent catalog's intake: registry `binary`, `uvx` and `npx` entries become review data.

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
  the way may be a link. An `npx` agent's directory holds `resolve/`
  (`package-lock.json`, npm's lock for the package), `tarballs/<n>/` (`n` the
  package's place in the lock's `packages`, the root counted as 0), each the
  `artifact` and `fetched.json` of one tarball, and per platform `tree/`
  (`lock.json`, `npm.json` and `unpack-check.json`, below) and `probe.json`.
  Node's own downloads are under `node/`: `SHASUMS256.txt.asc` and its
  `fetched.json`, and `<platform>/` holding the `artifact` and `fetched.json`
  of each build, so an agent whose registry id is `node`, or is another's,
  once case is folded, is dropped;
- `--interpreters`, where given: the reviewer's interpreter pins, by
  platform, each `{"url", "sha256", "python"}`: the `https` address of a
  python-build-standalone `install_only` build, its SHA-256, and the `3.M` it
  runs. A `uvx` target on a platform with no pin is dropped;
- `--node`, where given: the reviewer's Node pin, `{"version": "X.Y.Z"}`,
  an even major line from 22, and 22.15.0 or later on line 22. With no pin,
  every `npx` target is dropped. Giving it needs `gpg` on `PATH`;
- `--node-keys`, where given: the directory of the keys a Node release may be
  signed under, each `<fingerprint>.asc` and a `fingerprints` list of the
  primary fingerprints accepted; the keys vendored beside this script where
  not given;
- `--dispositions`, where given: the reviewer's decision on each install
  script `--ignore-scripts` skipped, by registry id and then
  `"<location> <event>"`, one of `inert`, `replaced` or `lost`; `built` and
  `local` belong to a build route this review does not take, and refuse the
  run;
- `--deny`: `deny.toml`, whose licence list decides what may be mirrored;
- `--merged`, where given: the merged data, against which a version already
  reviewed is held. A release whose content differs from what was reviewed
  under the same version is refused, naming what differs, since a changed
  release is taken in only under a new version. An `npx` agent's targets'
  locks are the one exception: the tree npm resolves for one version can
  change, so the entry keeps the merged `lock_revision` while every target's
  lock is the merged one, and takes the next where any differs; any other
  difference is refused as for every agent.

It writes the review data to `--out`, and each file-list asset (a per-file list
longer than 256 entries, kept as an asset named by its SHA-256) and each npm
lock under `--assets`. A target is dropped when its download is missing, larger than
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
list one path, when a wheel lists one member twice (a leading `./` removed
and case folded, as the client compares names), when a wheel does not end as
a wheel ends (its end record last, with no comment, and any zip64 record just
before its locator, which points at it), or when that record claims more
members or a larger central directory than the stated ceilings. A package pip found only as a source distribution drops its target
with a `build` list naming each one, clipped as a reason is, so it waits for
a build at review. An interpreter and its wheels are never marked for
mirroring.

An `npx` agent's package is pinned as `name@version`: a lowercase npm name,
scoped or not, and one exact version, so a range, a tag or an `npm:` alias
drops it. It runs on Node, which is checked before any agent: the signed
`SHASUMS256.txt.asc` for the pin, fetched from
`https://nodejs.org/dist/v<version>/`, is verified with `gpg` in a home of
its own and taken only under one good signature over a SHA-2 digest, made
before its key and that key's primary expired, by a key whose primary
fingerprint is listed, and only the
`<sha256>  <file>` lines it signed are read, none twice. Each platform's
build must be the bytes that list gives it, fetched through `nodejs.org`
alone; its `bin/node` (`node.exe` on Windows) and `LICENSE` are all a client
keeps of it, and Node is never mirrored. Node publishes no FreeBSD build, so
that target is dropped. The tree is npm's lock, in lockfile version 3, of
which only each package's location, name, version, `resolved`, `integrity`,
`inBundle`, `hasInstallScript`, `dev`, `optional`, `os`, `cpu` and `libc`
are read; a `dev` package is never installed. Every location is
`node_modules/<name>`, nested only under `node_modules/`, unique once case is
folded, and no longer than 767 bytes. Every tarball is fetched from
`https://registry.npmjs.org/` at a plain address of printable ASCII with
no space, and every hop of its fetch
stays on that host; its SHA-512 and size are computed here, a SHA-512
`integrity` must agree with it, a SHA-1 one is held only where it is the only
one, and a package with neither is refused. A tarball is read as the client
reads it, each header as it is written and none global: one gzip stream and
nothing after it, at most 65,536 members, an ordinary file, a directory or a
link alone, every name one the client writes, inside its package and the
ones it bundles and not straight under a `node_modules` directory, none
written twice once case is folded, and at most 131,072 names, directories
included, holding at most 8 MiB; and a lock's tarballs write at
most 65,536 files and 4 GiB together. Each package's own package.json,
a bundled one's included, must give the name and version the lock gives it. A
bundled package has no tarball of its own and is counted in the one that
holds it. A target's lock, the asset a client unpacks its tree from, lists
every package installed on it, in the lock's order, an optional one whose
`os`, `cpu` or `libc` refuses the platform left out (Linux runners are
glibc, and a `libc` refuses any other system), decided from the lock as npm
decides before npm runs; it is named by its SHA-256, so
targets with the same tree share one. The target is dropped unless the job's
`tree/lock.json` is that lock byte for byte and the tree `npm ci` built
(`tree/npm.json`) and the one `unpack-check` built from the lock
(`tree/unpack-check.json`) agree on every file under an installed location,
npm's own `.bin` links and `.package-lock.json` aside, but for an execute bit
on a file a package names as a `bin`, a `.gitignore` npm renamed from an
`.npmignore` of the same bytes, and any execute bit on Windows, where nothing
is marked runnable. The agent starts as `npx` would start it: Node with the
one `bin` the agent's own package names, or the one named for the package.
Each `preinstall`, `install` and `postinstall` script skipped, an `install`
implied by a `binding.gyp` among them, is listed on the target with the
reviewer's disposition, where npm's `hasInstallScript` must agree with the
scripts the package's own `package.json` gives; one with no disposition yet
is listed as `undecided` in the review data, marked in the diff, and refused
by `render`.

The job before the intake runs, for each `npx` agent, `npm install
--package-lock-only --ignore-scripts --omit=dev
--registry=https://registry.npmjs.org/ --before=<seven days ago>` to write
`resolve/package-lock.json`, and fetches each tarball it lists; then, on each
platform's runner, `lock` to write `tree/lock.json`, `npm ci --ignore-scripts
--omit=dev` with the pinned Node, `tree` over what it installed to write
`tree/npm.json`, and `crucible agents unpack-check` on the lock to write
`tree/unpack-check.json`.

`lock` writes the lock `review` computes for one platform (`--platform`)
from an agent's observed directory (`--resolved`) and its package
(`--package`, `name@version`), so the trees the job compares are of the lock
the catalog will list. `tree` lists the regular files under `--root`, as
`{"files": [{path, sha256, size, exec}]}` sorted by path, `exec` set where
any execute bit is; a link is neither followed nor listed, and the listing
stops at the client's ceilings for a tree. `recheck` runs daily: from the
merged data and the directory of lock assets it lists each tarball the
merged npm locks name, once, by its SHA-512 and address; given what the
registry answered for each (`--results`, the status by SHA-512), it proposes
a revocation, by key, version and lock revision, of every lock holding a
tarball answered 404 or 410, which a fresh install could no longer fetch,
and none for any other answer, which may pass. The proposals are written as
a bare list in the shape of the merged data's `revoked`, `{key, version,
lock_revision, reason}`, so a reviewer who agrees appends them as they are.

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
the one the data records for a file-list or lock asset, or whose hash is an
artifact's and whose entry carries no `licence_gate` record of
`{"passed": true}`. An artifact copy is a redistribution, so a copy of any
artifact, a mirror included, waits for that record; a file-list or lock asset
is the catalog's own data and needs none. A hash the data names both ways
answers to both rules. A copy
also needs its artifact's `mirror_licence` mark. An empty directory passes,
since nothing is then uploaded. An asset is opened without following a link,
but the check cannot hold a directory still: the publishing job checks and
uploads from a directory the untrusted job cannot write once the check has
begun, or the check proves nothing about what is uploaded.
"""

from __future__ import annotations

import argparse
import base64
import configparser
import hashlib
import json
import os
import pathlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import time
import tomllib
import unicodedata
import urllib.parse
import zipfile
import zlib

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
# before `zipfile` holds it: 46 bytes, the smallest member header, for each of
# `LISTED_ENTRIES` members, so no directory within it holds more headers than
# the entry ceiling allows.
CENTRAL_DIRECTORY_BYTES = 46 * LISTED_ENTRIES
# The one host a wheel is fetched from, at every hop of the way.
PYPI_HOST = "files.pythonhosted.org"
# An npm target's own ceilings: npm's lock and the packages it lists; one
# tarball, the bytes it unpacks to and the bytes of a package.json in it; the
# two listings of the tree that are compared; Node's signed checksums, and
# one vendored key; and the reviewer's dispositions.
LOCK_BYTES = 32 << 20
LOCK_PACKAGES = 8192
TARBALL_BYTES = 256 << 20
TARBALL_DECOMPRESSED = 1 << 30
PACKAGE_JSON_BYTES = 1 << 20
TREE_LISTING_BYTES = 64 << 20
TREE_ENTRIES = LISTED_ENTRIES
SHASUMS_BYTES = 1 << 20
KEY_BYTES = 64 << 10
DISPOSITIONS_BYTES = SMALL_BYTES
# The files and bytes one lock's tree may write, and how long a location and
# a version may be, as the client that unpacks it allows.
TREE_FILES = 1 << 16
TREE_BYTES = 4 << 30
NPM_PATH_BYTES = 767
# The most members one tarball may give, and the most bytes and components a
# name written from it may have, in any one component and in all.
TARBALL_MEMBERS = 1 << 16
# A tar block, and the most bytes a GNU long name or link, a name an extended
# header gives, and an extended header itself may hold, as the client reads
# them.
TAR_BLOCK = 512
LONG_NAME_BYTES = 4097
EXTENDED_BYTES = 64 << 10
# The most names one tarball's files and the directories above them make,
# each held once, as the client allows a tree; and the most bytes those
# names, each component held once, may hold, 64 a name on average.
TREE_NAMES = 1 << 17
TARBALL_NAME_BYTES = 64 * TREE_NAMES
NAME_COMPONENT_BYTES = 255
NAME_COMPONENTS = 64
# The devices Windows takes a name for whatever follows it after a dot,
# besides `com0` to `com9` and `lpt0` to `lpt9`, and the characters it
# refuses in a name besides the backslash and the colon.
WINDOWS_DEVICES = {"con", "prn", "aux", "nul", "conin$", "conout$"}
WINDOWS_REFUSES = '<>"|?*'
SEMVER_BYTES = 256
# How long one gpg call may take.
GPG_SECONDS = 60
# The oldest Node a pin may name: an even, long-term-support, major line, and
# on line 22 the first release with `module.registerHooks`, which the client
# starts an agent's loader with.
NODE_FLOOR = (22, 15, 0)
NODE_VERSION = re.compile(r"(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})")
NODE_DIST = "https://nodejs.org/dist/"
# Node's build for each platform, by the name its release gives it, and the
# archive it comes as. Node publishes no FreeBSD build.
NODE_BUILDS = {
    "linux-x86_64": ("linux-x64", "tar.gz"),
    "linux-aarch64": ("linux-arm64", "tar.gz"),
    "darwin-x86_64": ("darwin-x64", "tar.gz"),
    "darwin-aarch64": ("darwin-arm64", "tar.gz"),
    "windows-x86_64": ("win-x64", "zip"),
    "windows-aarch64": ("win-arm64", "zip"),
}
# What npm calls each platform's system and machine in a package's `os` and `cpu`.
NPM_OS = {"linux": "linux", "darwin": "darwin", "windows": "win32", "freebsd": "freebsd"}
NPM_CPU = {"x86_64": "x64", "aarch64": "arm64"}
# The C library npm finds on a Linux runner.
NPM_LIBC = "glibc"
# Where every package an npm target installs is taken from, at every hop.
NPM_REGISTRY = "https://registry.npmjs.org/"
NPM_HOST = "registry.npmjs.org"
# An `npx` package pinned as `name@version`: a lowercase npm name, scoped or
# not, and one exact semantic version, so no range, tag or `npm:` alias passes.
NPX = re.compile(
    r"((?:@[a-z0-9][a-z0-9._~-]{0,212}/)?[a-z0-9][a-z0-9._~-]{0,213})"
    r"@((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)"
)
# The printable characters a component of an npm location may not hold, as
# the client refuses them in a name it unpacks.
NPM_REFUSED = '\\:<>"|?*'
# The install-time scripts `--ignore-scripts` skips, in the order npm runs them.
INSTALL_EVENTS = ("preinstall", "install", "postinstall")
# What a reviewer may decide a skipped script was: nothing the agent needs,
# done instead by crucible, or lost with nothing in its place. `built` and
# `local` belong to the build route, which this review does not take.
DISPOSITIONS = ("inert", "replaced", "lost")
# How many rules a client unpacks a lock's tree by: the first.
UNPACK_RULES = 1

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
    """Refuse the open wheel `file` where its central directory, as the record
    that ends it states, holds more than `LISTED_ENTRIES` members or more than
    `CENTRAL_DIRECTORY_BYTES`, read before `zipfile` holds that directory.

    A wheel is held to the layout a wheel has, so that the record read here is
    the one `zipfile` obeys: the end record is the file's last 22 bytes, with no
    comment after it; and where the 20 bytes before it are a zip64 locator, or
    any of its fields stands at its all-ones value and so defers to zip64, the
    zip64 record is the 56 bytes just before that locator, where `zipfile`
    reads it, and the locator must point at that very place."""
    end_bytes, locator_bytes, record_bytes = 22, 20, 56
    size = file.seek(0, os.SEEK_END)
    if size < end_bytes:
        raise Dropped("is not a wheel this intake can read")
    file.seek(size - end_bytes)
    end = file.read(end_bytes)
    if end[:4] != b"PK\x05\x06" or end[20:22] != b"\x00\x00":
        raise Dropped(
            "does not end in the record that closes a central directory, with no zip comment after it, "
            "as a wheel ends"
        )
    on_disk, entries, directory, offset = struct.unpack("<HHII", end[8:20])
    deferred = 0xFFFF in (on_disk, entries) or 0xFFFFFFFF in (directory, offset)
    locator_at = size - end_bytes - locator_bytes
    locator = b""
    if locator_at >= 0:
        file.seek(locator_at)
        locator = file.read(locator_bytes)
    if locator[:4] == b"PK\x06\x07" or deferred:
        record_at = locator_at - record_bytes
        if locator[:4] != b"PK\x06\x07" or record_at < 0:
            raise Dropped("its central directory defers to a zip64 record it does not hold")
        (stated,) = struct.unpack("<Q", locator[8:16])
        file.seek(record_at)
        record = file.read(record_bytes)
        if stated != record_at or record[:4] != b"PK\x06\x06":
            raise Dropped(
                "its central directory's zip64 locator does not point at the zip64 record just before it, "
                "the one zipfile reads"
            )
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
            # Names compared as the client compares them: a leading `./`
            # removed, and case folded.
            seen: set[str] = set()
            for name in names:
                compared = name.removeprefix("./").casefold()
                if compared in seen:
                    raise Dropped(f"lists {clipped(name)} twice, as the client compares names")
                seen.add(compared)
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


def plain(url: str) -> str:
    """The path of `url`, refused where it is not a plain address: a %, query,
    fragment, parameter or backslash would let it be read as other than it is
    written, and so would an empty, `.` or `..` segment."""
    odd = next((mark for mark in "%?#;\\" if mark in url), None)
    if odd is not None:
        raise Dropped(
            f"{clipped(url)} is not a plain address: it holds a {odd}, and a %, query, fragment, "
            "parameter or backslash would let it be read as other than it is written"
        )
    path = urllib.parse.urlsplit(url).path
    if any(segment in ("", ".", "..") for segment in path[1:].split("/")):
        raise Dropped(f"{clipped(url)} holds an empty, . or .. segment in its path")
    return path


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
        file = plain(url).rsplit("/", 1)[-1]
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


def node_of(path: pathlib.Path | None) -> str | None:
    """The reviewer's Node pin, `{"version": "X.Y.Z"}`, held to `NODE_FLOOR`.
    With no pin, no npm target has a Node to run on."""
    if path is None:
        return None
    value = read_json(path, SMALL_BYTES, "the Node pin")
    if not isinstance(value, dict):
        raise Refused("the Node pin is not an object")
    extra = sorted(set(value) - {"version"})
    if extra:
        raise Refused(f"the Node pin holds {named(extra)}, which no pin holds")
    pinned = value.get("version")
    held = NODE_VERSION.fullmatch(pinned) if isinstance(pinned, str) else None
    if held is None:
        raise Refused(f"the Node pin's version {clipped(pinned)} is not a version X.Y.Z")
    parts = tuple(int(part) for part in held.groups())
    if parts[0] % 2 or parts < NODE_FLOOR:
        raise Refused(
            f"Node {pinned} is not a long-term-support release with module.registerHooks: "
            "the pin is an even major line from 22, and 22.15.0 or later on line 22"
        )
    return pinned


def release_keys(path: pathlib.Path) -> tuple[set[str], list[bytes]]:
    """The primary fingerprints a Node release may be signed under, from
    `fingerprints`, and every armoured key beside it."""
    try:
        listing = read(path / "fingerprints", SMALL_BYTES, "the Node release keys' fingerprints").decode()
    except UnicodeDecodeError:
        raise Refused("the Node release keys' fingerprints are not UTF-8") from None
    listed: set[str] = set()
    for line in listing.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if not re.fullmatch(r"[0-9A-F]{40}", line):
            raise Refused(f"the Node release keys' fingerprints list {clipped(line)}, which is not a fingerprint")
        listed.add(line)
    try:
        names = sorted(name for name in os.listdir(path) if name.endswith(".asc"))
    except OSError:
        raise Refused("the Node release keys are not a directory that can be read") from None
    if not listed or not names or len(names) > ITEMS:
        raise Refused(f"the Node release keys list no fingerprint, or hold no key or more than {ITEMS}")
    return listed, [read(path / name, KEY_BYTES, f"the Node release key {clipped(name)}") for name in names]


def gpg_run(command: list[str]) -> bytes:
    """What `command` printed, or the drop that says why it could not run."""
    try:
        ran = subprocess.run(command, capture_output=True, timeout=GPG_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        raise Dropped(f"gpg took longer than {GPG_SECONDS} seconds") from None
    except OSError as failed:
        raise Dropped(f"gpg could not be run: {clipped(failed.strerror)}") from None
    if ran.returncode != 0 and "--verify" not in command:
        raise Dropped(f"gpg refused the vendored Node release keys: {clipped(ran.stderr.decode(errors='replace'))}")
    return ran.stdout


def node_sums(gpg: str, keys: tuple[set[str], list[bytes]], signed: bytes) -> dict[str, str]:
    """The SHA-256 of each file `signed`, a Node release's
    `SHASUMS256.txt.asc`, lists, once its signature is held to be one of the
    vendored release keys': one signature, good, by a key whose primary
    fingerprint is listed, over a SHA-2 digest, made before any expiry of the
    key that made it and of its primary.
    Only the body gpg verified is read, in a keyring of its own."""
    listed, armoured = keys
    with tempfile.TemporaryDirectory(prefix="node-keys-") as held:
        home = pathlib.Path(held)
        command = [
            gpg, "--homedir", str(home), "--batch", "--no-tty", "--no-autostart",
            "--no-default-keyring", "--keyring", str(home / "keys.gpg"),
        ]
        files = []
        for index, key in enumerate(armoured):
            files.append(home / f"key-{index}.asc")
            files[-1].write_bytes(key)
        gpg_run(command + ["--import", *map(str, files)])
        (home / "SHASUMS256.txt.asc").write_bytes(signed)
        status = gpg_run(command + [
            "--trust-model", "always", "--no-auto-key-retrieve", "--status-fd", "1",
            "--output", str(home / "body"), "--verify", str(home / "SHASUMS256.txt.asc"),
        ]).decode(errors="replace")
        keyring = gpg_run(command + ["--with-colons", "--fixed-list-mode", "--list-keys"]).decode(errors="replace")
        lines = [line.split() for line in status.splitlines() if line.startswith("[GNUPG:] ")]
        said = [line[1] for line in lines if len(line) > 1]
        refused = sorted(set(said) & {"BADSIG", "ERRSIG", "EXPSIG", "REVKEYSIG", "NO_PUBKEY", "NODATA"})
        if refused or said.count("NEWSIG") != 1 or not {"GOODSIG", "EXPKEYSIG"} & set(said):
            raise Dropped(
                "SHASUMS256.txt.asc does not carry one good signature by a vendored Node release key"
                + (f": gpg said {named(refused)}" if refused else "")
            )
        valid = next((line for line in lines if line[1] == "VALIDSIG" and len(line) >= 12), None)
        if valid is None or valid[11] not in listed:
            raise Dropped("SHASUMS256.txt.asc is signed by a key whose fingerprint the vendored list does not hold")
        # OpenPGP's SHA-256, SHA-384, SHA-512 and SHA-224; a collision in
        # anything weaker could put other sums under the same signature.
        if valid[9] not in ("8", "9", "10", "11"):
            raise Dropped(f"SHASUMS256.txt.asc is signed over digest {clipped(valid[9])}, not one of the SHA-2 family")
        # The expiry of every key and subkey, by fingerprint, as the colon
        # listing gives it: the expiry field of a `pub` or `sub` line, then
        # its `fpr` line.
        expiries: dict[str, str] = {}
        expiry = ""
        for line in keyring.splitlines():
            fields = line.split(":")
            if fields[0] in ("pub", "sub") and len(fields) > 6:
                expiry = fields[6]
            elif fields[0] == "fpr" and len(fields) > 9:
                expiries[fields[9]] = expiry
        # Before the expiry of the key that made it and of its primary, which
        # gpg's listing may not carry down to a subkey.
        signed_at = valid[4]
        for ends in {expiries.get(valid[2], ""), expiries.get(valid[11], "")}:
            if not signed_at.isdigit() or (ends and (not ends.isdigit() or int(signed_at) >= int(ends))):
                raise Dropped("SHASUMS256.txt.asc was signed after the key that signed it expired")
        body = read(home / "body", SHASUMS_BYTES, "SHASUMS256.txt")
    sums: dict[str, str] = {}
    for line in body.decode(errors="replace").splitlines():
        held_line = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if held_line is None:
            raise Dropped(f"SHASUMS256.txt holds {clipped(line)}, which is not a SHA-256 and a file name")
        if held_line[2] in sums:
            raise Dropped(f"SHASUMS256.txt lists {clipped(held_line[2])} twice")
        sums[held_line[2]] = held_line[1]
    return sums


class Node:
    """The Node release a run pins, its signed checksums, or why there are
    none, and each platform's download of it as an artifact, made once and
    shared by every npm agent."""

    def __init__(self, pinned: str | None, sums: dict[str, str] | None, failure: str | None) -> None:
        self.pinned = pinned
        self.sums = sums
        self.failure = failure
        self.made: dict[str, tuple] = {}

    def artifact(self, observed: pathlib.Path, platform: str, assets: dict[str, bytes]) -> tuple[dict, list[str], str]:
        """Node's artifact for `platform`, the hosts it came through, and the
        binary a target runs."""
        if self.pinned is None:
            raise Dropped("no Node is pinned, so an npm agent has nothing to run on")
        if self.failure is not None:
            raise Dropped(f"Node {self.pinned} is not verified: {self.failure}")
        if platform not in self.made:
            try:
                self.made[platform] = (self.build(observed, platform), None)
            except Spent:
                raise
            except Dropped as failed:
                self.made[platform] = (None, str(failed))
        made, failure = self.made[platform]
        if failure is not None:
            raise Dropped(failure)
        artifact, hosts, cmd, kept = made
        assets.update(kept)
        return copy_of(artifact), list(hosts), cmd

    def build(self, observed: pathlib.Path, platform: str) -> tuple[dict, list[str], str, dict[str, bytes]]:
        build = NODE_BUILDS.get(platform)
        if build is None:
            raise Dropped(f"Node publishes no build for {platform}")
        name, format = build
        top = f"node-v{self.pinned}-{name}"
        file = f"{top}.{format}"
        url = f"{NODE_DIST}v{self.pinned}/{file}"
        cmd = f"{top}/node.exe" if platform.startswith("windows-") else f"{top}/bin/node"
        kept = [cmd, f"{top}/LICENSE"]
        place = place_of(observed, "node", platform)

        def made() -> tuple[dict, list[str], str, dict[str, bytes]]:
            sha256, size = hashed(place / "artifact")
            assert self.sums is not None
            if self.sums.get(file) != sha256:
                raise Dropped(f"the download is not the bytes SHASUMS256.txt lists for {file}")
            hosts = chained(place, url, "Node's address")
            listing = unpacked_at(place)
            for listed in listing[0]:
                if listed["kind"] == "link" or (listed["kind"] == "file" and listed["path"] not in kept):
                    raise Dropped(f"unpack-check lists {clipped(listed['path'])}, which is not kept")
            runnable(listing[0], cmd, platform)
            held: dict[str, bytes] = {}
            artifact = artifact_of(url, format, sha256, size, listing, held)
            artifact["kept"] = kept
            return artifact, hosts, cmd, held

        return staged(f"Node {self.pinned}", made)


def copy_of(value: object) -> object:
    """A deep copy of JSON data, so no two entries share one value."""
    return json.loads(json.dumps(value))


def node_release(
    observed: pathlib.Path, pinned: str | None, gpg: str | None, keys: tuple[set[str], list[bytes]] | None
) -> Node:
    """The run's Node: its checksums once the signed list the job left is
    verified and was fetched from Node's own address, or why not."""
    if pinned is None or gpg is None or keys is None:
        return Node(None, None, None)
    try:
        place = place_of(observed, "node")
        chained(place, f"{NODE_DIST}v{pinned}/SHASUMS256.txt.asc", "Node's address for SHASUMS256.txt.asc")
        try:
            signed = read(place / "SHASUMS256.txt.asc", SHASUMS_BYTES, "SHASUMS256.txt.asc")
        except Refused as refused:
            raise Dropped(str(refused)) from None
        return Node(pinned, node_sums(gpg, keys, signed), None)
    except Spent:
        raise
    except Dropped as failed:
        return Node(pinned, None, str(failed))


def npx_package(value: object) -> tuple[str, str]:
    """An `npx` package as `name@version`: a lowercase npm name, scoped or
    not, at one exact version, so no range, tag or `npm:` alias passes."""
    held = NPX.fullmatch(value) if isinstance(value, str) else None
    if held is None:
        raise Dropped(f"the npx package {clipped(value)} is not pinned as a lowercase name@ an exact version")
    return held[1], held[2]


def npm_part(part: str) -> bool:
    """Whether `part` is one component a location may hold."""
    return (
        bool(part)
        and part not in (".", "..")
        and not part.endswith((".", " "))
        and all("!" <= c <= "~" and c not in NPM_REFUSED for c in part)
    )


def npm_name(name: str) -> bool:
    """Whether `name` is a package's name as a location holds it: one
    component, or a scope beginning `@` and one component under it."""
    parts = name.split("/")
    if len(parts) == 2 and parts[0].startswith("@") and len(parts[0]) > 1:
        return npm_part(parts[0]) and npm_part(parts[1]) and parts[1] != "node_modules"
    return len(parts) == 1 and npm_part(name) and not name.startswith("@") and name != "node_modules"


def npm_location(value: object) -> str:
    """Where npm installs a package: `node_modules/` and a name, then any
    number of `/node_modules/` and another, within `NPM_PATH_BYTES`."""
    if not isinstance(value, str) or len(value.encode(errors="replace")) > NPM_PATH_BYTES:
        raise Dropped(f"the lock installs a package at {clipped(value)}, which is not where npm installs one")
    rest = value
    while True:
        head, slash, rest = rest.partition("/")
        if head != "node_modules" or not slash:
            break
        first, slash, rest = rest.partition("/")
        name = first
        if first.startswith("@"):
            second, slash, rest = rest.partition("/")
            name = f"{first}/{second}"
        if not npm_name(name):
            break
        if not slash:
            return value
    raise Dropped(f"the lock installs a package at {clipped(value)}, which is not where npm installs one")


def fitting(values: object, current: str, what: str) -> bool:
    """Whether npm's `os` or `cpu` list `values` admits `current`: a `!` names
    one refused, and any other entry the only ones admitted."""
    if values is None:
        return True
    if not isinstance(values, list) or len(values) > ITEMS or not all(isinstance(item, str) for item in values):
        raise Dropped(f"the lock's {what} is not a list of names")
    if f"!{current}" in values:
        return False
    admitted = [item for item in values if not item.startswith("!")]
    return not admitted or current in admitted


class Bounded:
    """A stream read no further than `ceiling` bytes, refused past it."""

    def __init__(self, file, ceiling: int, what: str) -> None:
        self.file = file
        self.left = ceiling
        self.what = what

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(self.left + 1 if count < 0 else min(count, self.left + 1))
        self.left -= len(chunk)
        if self.left < 0:
            raise Dropped(f"{self.what} unpacks to more than its ceiling of bytes")
        return chunk


class OneGzip:
    """The one gzip stream `file` holds, decompressed as it is read; whatever
    follows that stream is left for `ended` to refuse."""

    def __init__(self, file) -> None:
        self.file = file
        self.inflate = zlib.decompressobj(wbits=31)
        self.held = b""

    def read(self, count: int = -1) -> bytes:
        count = 1 << 20 if count < 0 else count
        while len(self.held) < count and not self.inflate.eof:
            chunk = self.inflate.unconsumed_tail or self.file.read(1 << 16)
            if not chunk:
                raise Dropped("the tarball's gzip stream ends before it is whole")
            self.held += self.inflate.decompress(chunk, 1 << 20)
        chunk, self.held = self.held[:count], self.held[count:]
        return chunk

    def ended(self) -> bool:
        """Whether the stream is whole and nothing follows it."""
        return self.inflate.eof and not self.inflate.unused_data and not self.file.read(1)


def npm_written(name: str) -> str:
    """`name` as the client writes a file under it, one leading `./` dropped,
    refused where the client would refuse it: printable ASCII alone,
    relative, with no backslash, colon or character Windows refuses, at most
    255 bytes a component and 64 components, and no empty, `.` or `..`
    component, none that ends in a dot or a space, and none that names a
    device Windows reserves."""
    name = name[2:] if name.startswith("./") else name
    shown_name = clipped(name)
    parts = name.split("/")
    if len(parts) > NAME_COMPONENTS or any(len(part) > NAME_COMPONENT_BYTES for part in parts):
        raise Dropped(f"the tarball writes {shown(shown_name)}, deeper than {NAME_COMPONENTS} components "
                      f"or with one longer than {NAME_COMPONENT_BYTES} bytes")
    if any(not " " <= character <= "~" for character in name):
        raise Dropped(f"the tarball writes {shown(shown_name)}, which is not printable ASCII")
    if name.startswith("/") or any(mark in name for mark in "\\:" + WINDOWS_REFUSES):
        raise Dropped(f"the tarball writes {shown(shown_name)}, which is absolute or holds a character "
                      "Windows refuses")
    for part in parts:
        stem = part.split(".")[0].rstrip(" ").lower()
        if part in ("", ".", ".."):
            raise Dropped(f"the tarball writes {shown(shown_name)}, with an empty, . or .. component")
        if part.endswith((".", " ")):
            raise Dropped(f"the tarball writes {shown(shown_name)}, with a component ending in a dot or a space")
        if stem in WINDOWS_DEVICES or (len(stem) == 4 and stem[:3] in ("com", "lpt") and stem[3].isdigit()):
            raise Dropped(f"the tarball writes {shown(shown_name)}, which names a device Windows reserves")
    return name


def npm_after_name(text: str) -> str | None:
    """What follows the package's name `text` begins with, as the client
    finds one: one component, or a scope of `@` and more and one component
    under it; none where it begins with no name."""
    first, _, rest = text.partition("/")
    if not first or first == "node_modules":
        return None
    if not first.startswith("@"):
        return text[len(first):]
    second = rest.split("/")[0]
    if len(first) == 1 or not second:
        return None
    return text[len(first) + 1 + len(second):]


def npm_writes(locations: set[str], held: set[str], location: str, path: str) -> bool:
    """Whether the tarball of the package at `location` may write `path`, as
    the client decides: the deepest of the lock's `locations` above it,
    reached from `location` by `/node_modules/` and a package's name, is one
    `held` in that tarball, and what is below it does not begin with a
    `node_modules` component, whatever its case."""
    folded = path.lower()
    end = deepest = len(location)
    while folded.startswith("/node_modules/", end):
        rest = npm_after_name(folded[end + len("/node_modules/"):])
        if rest is None or not rest.startswith("/"):
            break
        end = len(folded) - len(rest)
        if path[:end] in locations:
            deepest = end
    return path[:deepest] in held and folded[deepest + 1:].split("/")[0] != "node_modules"


def tar_number(field: bytes, binary: bool = False) -> int:
    """The number a tar header's `field` gives, as the client's tar reader
    reads it: where `binary` allows it and the high bit of its first byte is
    set, its last eight bytes in base 256; otherwise octal, up to its first
    NUL, within ASCII whitespace and after at most one `+`."""
    if binary and field[0] & 0x80:
        return int.from_bytes(field[-8:], "big")
    text = field.split(b"\0", 1)[0].strip(b" \t\n\v\f\r")
    digits = text[1:] if text.startswith(b"+") else text
    if not digits or digits.strip(b"01234567") or int(digits, 8) >> 64:
        raise Dropped("the tarball holds a header with a number that is not one")
    return int(digits, 8)


def tar_name(header: bytes) -> bytes:
    """The name a tar `header` gives, as the client's tar reader reads it:
    its name field up to its first NUL, after the prefix field and a `/`
    where the header is ustar and the prefix is not empty or the name field
    holds a backslash."""
    name = header[:100].split(b"\0", 1)[0]
    if header[257:263] == b"ustar\0" and header[263:265] == b"00" and (header[345] or b"\\" in header[:100]):
        prefix = header[345:500].split(b"\0", 1)[0]
        name = prefix + b"/" + name if prefix else name
    return name


def tar_extended(data: bytes) -> list[tuple[bytes, bytes]]:
    """The records of an extended header's `data`, as the client's tar
    reader reads them: a line each up to the first empty one, each `length
    key=value` with `length` counting the line and its newline."""
    records = []
    for line in data.split(b"\n"):
        if not line:
            break
        length, space, record = line.partition(b" ")
        key, equals, value = record.partition(b"=")
        digits = length[1:] if length.startswith(b"+") else length
        if not space or not equals or not digits.isdigit() or int(digits) != len(line) + 1:
            raise Dropped("the tarball holds an extended header the client cannot read")
        records.append((key, value))
    return records


def tar_read(stream, count: int) -> bytes:
    """The next `count` bytes of `stream`, refused where it ends first."""
    held = bytearray()
    while len(held) < count:
        chunk = stream.read(min(count - len(held), 1 << 20))
        if not chunk:
            raise Dropped("the tarball ends inside a header or a member")
        held += chunk
    return bytes(held)


def tar_skip(stream, count: int) -> None:
    """Reads past the next `count` bytes of `stream`, refused where it ends
    first."""
    while count:
        chunk = stream.read(min(count, 1 << 20))
        if not chunk:
            raise Dropped("the tarball ends inside a member")
        count -= len(chunk)


def tarball_of(place: pathlib.Path, wanted: set[str], location: str, locations: set[str]) -> dict:
    """The tarball under `place`, of the package at `location`, as npm
    unpacks it and the client reads it: its SHA-512, SHA-1 and size; the
    files and bytes it writes, its first component stripped and a member
    with nothing after it skipped, directories and links writing nothing;
    and the `package.json` and whether a `binding.gyp` lies at each package
    root in `wanted` ("" its own, or a bundled package's below it).

    Its headers are read as the client reads them, each as it is written: a
    GNU long name or link, or an extended header, describes the one member
    after it, and its own name is its header's. A global extended header is
    refused: npm applies one to every member after it and the client does
    not, so the two would write such a member under different names.

    It is refused where the client would refuse it: more than one gzip
    stream, a header whose checksum or numbers are wrong, one member
    described twice by the same kind of header or a description with no
    member after it, an extended header the client cannot read or whose
    `size` is not its member's, a member of a kind other than a file, a
    directory or a link, more than `TARBALL_MEMBERS` members, a name the
    client does not write or that is longer than `NPM_PATH_BYTES` under
    `location`, a file written outside its package and the ones it bundles,
    of the lock's `locations`, or straight under a `node_modules` directory,
    a name written twice, or as a file and a directory, once case is folded,
    and names that with the directories above them are more than
    `TREE_NAMES`, or hold more than `TARBALL_NAME_BYTES`. A sparse member,
    by its header or by its extended one, is refused, though the client
    writes the second as it is stored."""
    held_locations = {location} | {f"{location}/{root}" for root in wanted if root}
    with opened(place / "artifact") as file:
        sha512 = hashlib.sha512()
        sha1 = hashlib.sha1()
        size = 0
        while chunk := file.read(1 << 20):
            spend(len(chunk))
            size += len(chunk)
            if size > TARBALL_BYTES:
                raise Dropped(f"the tarball is larger than {TARBALL_BYTES} bytes")
            sha512.update(chunk)
            sha1.update(chunk)
        file.seek(0)
        files = unpacked = members = 0
        manifests: dict[str, bytes] = {}
        gyps: set[str] = set()
        # The names written and the directories above them, once folded, as
        # a tree of components each held once, so what is kept grows with
        # the names and not with their depth: each node's child by its
        # component, and whether each node is a file.
        children: dict[tuple[int, str], int] = {}
        files_at: list[bool] = [False]
        held_bytes = 0
        # What the headers read so far say of the member after them, by type.
        ahead: dict[bytes, bytes] = {}
        try:
            stream = OneGzip(Spending(file))
            unzipped = Bounded(stream, TARBALL_DECOMPRESSED, "the tarball")
            # One header at a time, and nothing kept of a member once read,
            # so a tarball of many members is not held whole.
            while header := unzipped.read(TAR_BLOCK):
                header += tar_read(unzipped, TAR_BLOCK - len(header))
                if not any(header):
                    break
                if tar_number(header[148:156]) != sum(header[:148]) + sum(header[156:]) + 8 * 32:
                    raise Dropped("the tarball holds a header whose checksum is wrong")
                kind = header[156:157]
                size_of = tar_number(header[124:136], binary=True)
                padding = -size_of % TAR_BLOCK
                if kind in (b"L", b"K", b"x"):
                    ceiling = EXTENDED_BYTES if kind == b"x" else LONG_NAME_BYTES
                    if size_of > ceiling:
                        raise Dropped(f"the tarball holds a long name or extended header of more than {ceiling} bytes")
                    if kind in ahead:
                        raise Dropped("the tarball describes one member twice")
                    ahead[kind] = tar_read(unzipped, size_of)
                    tar_skip(unzipped, padding)
                    continue
                if kind == b"g":
                    raise Dropped("the tarball holds a global extended header, which npm applies to the "
                                  "members after it and the client does not")
                members += 1
                if members > TARBALL_MEMBERS:
                    raise Dropped(f"the tarball holds more than {TARBALL_MEMBERS} members")
                raw = tar_name(header)
                for key, value in tar_extended(ahead.pop(b"x", b"")):
                    if key in (b"path", b"linkpath") and len(value) > LONG_NAME_BYTES:
                        raise Dropped(f"the tarball holds a name longer than {LONG_NAME_BYTES} bytes")
                    if key == b"path":
                        raw = value
                    digits = value[1:] if value.startswith(b"+") else value
                    if key == b"size" and not (digits.isdigit() and int(digits) == size_of):
                        raise Dropped("the tarball's extended header gives a member another size than its header")
                    if key.startswith(b"GNU.sparse."):
                        raise Dropped("the tarball holds a sparse member, of a kind npm's rules refuse")
                long = ahead.pop(b"L", None)
                if long is not None:
                    raw = long[:-1] if long.endswith(b"\0") else long
                ahead.pop(b"K", None)
                # Every byte stands for itself, so a name outside printable
                # ASCII is refused below as it is written.
                name = raw.decode("latin-1")
                if kind in (b"5", b"2", b"1"):
                    tar_skip(unzipped, size_of + padding)
                    continue
                if kind not in (b"\0", b"0"):
                    raise Dropped(f"the tarball holds {shown(clipped(name))}, of a kind npm's rules refuse")
                _, slash, rest = name.partition("/")
                if not slash or not rest:
                    tar_skip(unzipped, size_of + padding)
                    continue
                rest = npm_written(rest)
                path = f"{location}/{rest}"
                if len(path) > NPM_PATH_BYTES:
                    raise Dropped(f"the tarball writes {shown(clipped(path))}, longer than {NPM_PATH_BYTES} bytes")
                npm_written(path)
                if not npm_writes(locations, held_locations, location, path):
                    raise Dropped(f"the tarball writes {shown(clipped(path))}, outside its package and the ones "
                                  "it bundles, or straight under a node_modules directory")
                tar_number(header[100:108])
                node = 0
                parts = path.casefold().split("/")
                for depth, part in enumerate(parts, 1):
                    child = children.get((node, part))
                    if child is None:
                        # The first node is the tree's root, which is no name.
                        if len(files_at) > TREE_NAMES:
                            raise Dropped(f"the tarball writes more than {TREE_NAMES} names, each "
                                          "directory above a file counted")
                        held_bytes += len(part)
                        if held_bytes > TARBALL_NAME_BYTES:
                            raise Dropped(f"the tarball's names hold more than {TARBALL_NAME_BYTES} bytes")
                        child = children[(node, part)] = len(files_at)
                        files_at.append(depth == len(parts))
                    elif depth == len(parts) or files_at[child]:
                        raise Dropped(f"the tarball writes {shown(clipped(path))} twice, or as a file and "
                                      "a directory, once case is folded")
                    node = child
                files += 1
                unpacked += size_of
                root, _, leaf = rest.rpartition("/")
                if root in wanted and leaf == "binding.gyp":
                    gyps.add(root)
                if root in wanted and leaf == "package.json":
                    if size_of > PACKAGE_JSON_BYTES:
                        raise Dropped(f"{clipped(rest)} is larger than {PACKAGE_JSON_BYTES} bytes")
                    manifests[root] = tar_read(unzipped, size_of)
                    tar_skip(unzipped, padding)
                else:
                    tar_skip(unzipped, size_of + padding)
            if ahead:
                raise Dropped("the tarball describes a member it does not hold")
            while unzipped.read(1 << 20):
                pass
            if not stream.ended():
                raise Dropped("the tarball goes on past its one gzip stream")
        except Dropped:
            raise
        except Exception:  # whatever gzip cannot read is a tarball npm would not unpack
            raise Dropped("is not a gzip-compressed tar this intake can read") from None
    return {
        "files": files,
        "gyps": gyps,
        "manifests": manifests,
        "sha1": sha1.digest(),
        "sha512": sha512.digest(),
        "size": size,
        "unpacked": unpacked,
    }


def integrity_agrees(value: object, tarball: dict) -> None:
    """Refuse a tarball the lock's `integrity` does not describe: every
    SHA-512 it gives must agree, and a SHA-1 is held only where no SHA-512 is
    given; a lock that gives neither pins nothing npm checks, so it is
    refused."""
    if value is None:
        raise Dropped("the lock gives no SHA-512 or SHA-1 integrity for its tarball")
    given: dict[str, list[bytes]] = {}
    for token in text(value, "the lock's integrity").split():
        algorithm, dash, encoded_digest = token.partition("-")
        encoded_digest = encoded_digest.split("?", 1)[0]
        try:
            given.setdefault(algorithm, []).append(base64.b64decode(encoded_digest, validate=True))
        except ValueError:
            raise Dropped(f"the lock's integrity {clipped(token)} is not base64") from None
    held = given.get("sha512") or given.get("sha1")
    if held is None:
        raise Dropped("the lock gives no SHA-512 or SHA-1 integrity for its tarball")
    expected = tarball["sha512"] if "sha512" in given else tarball["sha1"]
    if any(item != expected for item in held):
        raise Dropped("the tarball is not the bytes the lock's integrity describes")


def install_events(manifest: dict, gyp: bool) -> list[str]:
    """The install-time scripts npm would run for a package, which
    `--ignore-scripts` skips: its own, and the `install` npm implies for a
    `binding.gyp` at its root when it gives neither `install` nor `preinstall`."""
    scripts = manifest.get("scripts")
    scripts = scripts if isinstance(scripts, dict) else {}
    events = [event for event in INSTALL_EVENTS if isinstance(scripts.get(event), str)]
    if gyp and "install" not in events and "preinstall" not in events:
        events.append("install")
    return [event for event in INSTALL_EVENTS if event in events]


def npm_resolved(observed: pathlib.Path, agent: str, package: str, released: str) -> dict:
    """The tree npm's lock under `<agent>/resolve/` describes, with each
    tarball the job left for it checked and counted: the packages it would
    install on some platform, in the lock's order, and the agent's own
    package.json."""
    place = place_of(observed, agent, "resolve")
    try:
        lock = obj(read_json(place / "package-lock.json", LOCK_BYTES, "package-lock.json"), "package-lock.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if lock.get("lockfileVersion") != 3 or isinstance(lock.get("lockfileVersion"), bool):
        raise Dropped("package-lock.json is not in lockfile version 3, the one read here")
    listed = obj(lock.get("packages"), "package-lock.json's packages")
    if len(listed) > LOCK_PACKAGES + 1:
        raise Dropped(f"package-lock.json lists more than {LOCK_PACKAGES} packages")
    order = list(listed.items())
    if not order or order[0][0] != "":
        raise Dropped("package-lock.json's packages do not begin with the project's root")
    packages: list[dict] = []
    folded: set[str] = set()
    for index, (location, fields) in enumerate(order[1:], start=1):
        fields = obj(fields, f"the lock's package {index}")
        if fields.get("dev") is True:
            continue  # never installed, so nothing of it is read
        location = npm_location(location)
        if location.casefold() in folded:
            raise Dropped(f"the lock installs two packages at {clipped(location)}, as a folding file system names them")
        folded.add(location.casefold())
        name = fields.get("name", location.rpartition("node_modules/")[2])
        if not isinstance(name, str) or not npm_name(name):
            raise Dropped(f"the lock names the package at {clipped(location)} {clipped(name)}, which is not a package's name")
        held_version = fields.get("version")
        if (
            not isinstance(held_version, str)
            or not held_version
            or len(held_version) > SEMVER_BYTES
            or not all("!" <= c <= "~" for c in held_version)
        ):
            raise Dropped(f"the lock's version for {clipped(location)} is not a version")
        packages.append({
            "bundled": fields.get("inBundle") is True,
            "cpu": fields.get("cpu"),
            "index": index,
            "libc": fields.get("libc"),
            "location": location,
            "name": name,
            "optional": fields.get("optional") is True,
            "os": fields.get("os"),
            "scripted": fields.get("hasInstallScript") is True,
            "version": held_version,
            "fields": fields,
        })
    top = f"node_modules/{package}"
    own = next((item for item in packages if item["location"] == top), None)
    if own is None or own["bundled"] or own["name"] != package or own["version"] != released:
        raise Dropped(f"the lock installs no {clipped(package)} {clipped(released)} at {clipped(top)}")
    unpacked = {item["location"] for item in packages if not item["bundled"]}
    for item in packages:
        if item["bundled"]:
            above = item["location"]
            holder = None
            while "/node_modules/" in above:
                above = above.rpartition("/node_modules/")[0]
                if above in unpacked:
                    holder = above
                    break
            if holder is None:
                raise Dropped(f"{clipped(item['location'])} is bundled in no package the lock installs above it")
            item["holder"] = holder
    by_location = {item["location"]: item for item in packages}
    locations = set(by_location)
    for item in packages:
        if item["bundled"]:
            continue
        wanted = {""} | {
            other["location"][len(item["location"]) + 1:]
            for other in packages
            if other.get("holder") == item["location"]
        }

        def fetched(item: dict = item, wanted: set[str] = wanted) -> dict:
            url = text(item["fields"].get("resolved"), "its resolved address")
            # The client fetches the address as it is written, so it holds
            # nothing that would have to be encoded first.
            if any(not "!" <= character <= "~" for character in url):
                raise Dropped(
                    f"{shown(clipped(url))} holds a space or a character outside printable ASCII, "
                    "so it is not fetched as it is written"
                )
            if not url.startswith(NPM_REGISTRY):
                raise Dropped(f"{clipped(url)} is not on {NPM_REGISTRY}, the one place a package is taken from")
            plain(url)
            inner = place_of(observed, agent, "tarballs", str(item["index"]))
            tarball = tarball_of(inner, wanted, item["location"], locations)
            integrity_agrees(item["fields"].get("integrity"), tarball)
            reached = chained(inner, url, "the lock's address")
            elsewhere = next((name for name in reached if name != NPM_HOST), None)
            if elsewhere is not None:
                raise Dropped(f"fetched.json reaches {clipped(elsewhere)}, but a tarball is fetched from {NPM_HOST} alone")
            return {"tarball": tarball, "url": url}

        item.update(staged(f"the tarball of {clipped(item['location'])}", fetched))
    for item in packages:
        holder = by_location[item.get("holder", item["location"])]
        root = item["location"][len(holder["location"]) + 1:] if item["bundled"] else ""
        manifest_bytes = holder["tarball"]["manifests"].get(root)
        if manifest_bytes is None:
            raise Dropped(f"the tarball for {clipped(item['location'])} holds no package.json for it")
        try:
            manifest = obj(parsed(manifest_bytes, "its package.json"), "its package.json")
        except Refused as refused:
            raise Dropped(f"{clipped(item['location'])}: {refused}") from None
        if manifest.get("name") != item["name"] or manifest.get("version") != item["version"]:
            raise Dropped(
                f"{clipped(item['location'])}: its package.json names another package or version "
                "than the lock gives it"
            )
        item["manifest"] = manifest
        item["events"] = install_events(manifest, root in holder["tarball"]["gyps"])
        if bool(item["events"]) != item["scripted"]:
            raise Dropped(
                f"the lock says {clipped(item['location'])} "
                + ("has an install script, and its package.json gives none" if item["scripted"]
                   else "has no install script, and its package.json gives "
                   + ", ".join(item["events"]))
            )
    return {"own": own, "packages": packages}


def npm_installed(resolution: dict, platform: str) -> list[dict]:
    """The packages `npm ci --omit=dev` installs on `platform`, in the lock's
    order, decided from the lock as npm decides: an optional one whose `os`,
    `cpu` or `libc` refuses the platform is left out with everything under
    it, and any other such one refuses the target. A `libc` is held only on
    Linux, whose runners are glibc; anywhere else, any `libc` refuses."""
    system, machine = platform.split("-")
    current_os, current_cpu = NPM_OS[system], NPM_CPU[machine]
    linux = system == "linux"
    left_out: list[str] = []
    installed = []
    for item in resolution["packages"]:
        if any(item["location"].startswith(above + "/") for above in left_out):
            continue
        fits_libc = item["libc"] is None or (linux and fitting(item["libc"], NPM_LIBC, "libc"))
        if fitting(item["os"], current_os, "os") and fitting(item["cpu"], current_cpu, "cpu") and fits_libc:
            installed.append(item)
        elif item["optional"]:
            left_out.append(item["location"])
        else:
            raise Dropped(f"{clipped(item['location'])} is not optional, and its os, cpu or libc refuses {platform}")
    if any(item.get("holder") is not None and item["holder"] in left_out for item in installed):
        raise Dropped("a bundled package's holder is not installed here")
    return installed


def npm_lock(installed: list[dict]) -> bytes:
    """The lock asset a client unpacks the tree from: every package installed,
    in the lock's order, a bundled one marked as such and every other with the
    tarball the intake checked, compact with sorted keys; refused where its
    tarballs write more files or bytes together, or hold more bytes, than the
    client unpacks for one tree."""
    held_tarballs = [item["tarball"] for item in installed if not item["bundled"]]
    for field, ceiling, said in (
        ("files", TREE_FILES, "write more than {} files"),
        ("unpacked", TREE_BYTES, "write more than {} bytes"),
        ("size", TREE_BYTES, "hold more than {} bytes"),
    ):
        if sum(tarball[field] for tarball in held_tarballs) > ceiling:
            raise Dropped(f"its tarballs {said.format(ceiling)} together, more than a client unpacks for one tree")
    packages = []
    for item in installed:
        package: dict = {"location": item["location"], "name": item["name"], "version": item["version"]}
        if item["bundled"]:
            package["in_bundle"] = True
        else:
            tarball = item["tarball"]
            package.update(
                files=tarball["files"],
                sha512=tarball["sha512"].hex(),
                size=tarball["size"],
                unpacked=tarball["unpacked"],
                url=item["url"],
            )
        packages.append(package)
    held = compact({"packages": packages, "unpack_rules": UNPACK_RULES})
    if len(held) > ASSET_BYTES:
        raise Dropped(f"its lock is larger than a catalog asset may be, {ASSET_BYTES} bytes")
    return held


def tree_listing(place: pathlib.Path, name: str, built: bool) -> dict[str, dict]:
    """A tree's files by path: npm's, as `tree` lists `npm ci`'s output, or
    crucible's, as `unpack-check` prints the tree a lock unpacks to."""
    try:
        printed = obj(read_json(place / name, TREE_LISTING_BYTES, f"tree/{name}"), f"tree/{name}")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    files = printed.get("files")
    if not isinstance(files, list) or len(files) > TREE_ENTRIES:
        raise Dropped(f"tree/{name} lists no files, or more than {TREE_ENTRIES}")
    held: dict[str, dict] = {}
    # Two paths a folding file system takes for one are one file listed twice.
    folded: set[str] = set()
    for index, item in enumerate(files):
        item = obj(item, f"tree/{name} file {index}")
        path = text(item.get("path"), f"tree/{name} file {index}'s path")
        if not inside(path) or path.casefold() in folded:
            raise Dropped(f"tree/{name} lists {clipped(path)}, which is not inside its tree or is listed twice")
        if built and item.get("kind") != "file":
            raise Dropped(f"tree/{name} lists {clipped(path)} as other than a file")
        runs = item.get("exec")
        if not isinstance(runs, bool):
            raise Dropped(f"tree/{name}: {clipped(path)}'s exec flag is not true or false")
        held[path] = {
            "exec": runs,
            "sha256": digest(item.get("sha256"), f"tree/{name}: {clipped(path)}'s sha256"),
            "size": number(item.get("size"), f"tree/{name}: {clipped(path)}'s size"),
        }
        folded.add(path.casefold())
    return held


def npm_own(path: str, locations: set[str]) -> bool:
    """Whether npm wrote `path` for itself rather than from a tarball: the
    project's `package.json` and `package-lock.json`, and under the root's or
    a package's `node_modules`, its `.bin` links and `.package-lock.json`."""
    if path in ("package.json", "package-lock.json"):
        return True
    parts = path.split("/")
    for at, part in enumerate(parts[:-1]):
        tail = parts[at + 1:]
        if part == "node_modules" and (tail[0] == ".bin" and len(tail) > 1 or tail == [".package-lock.json"]):
            above = "/".join(parts[:at])
            return above == "" or above in locations
    return False


def owner_of(path: str, locations: set[str]) -> str | None:
    """The deepest installed location `path` lies under."""
    parts = path.split("/")
    for depth in range(len(parts) - 1, 0, -1):
        above = "/".join(parts[:depth])
        if above in locations:
            return above
    return None


def trees_agree(place: pathlib.Path, installed: list[dict], platform: str) -> set[str]:
    """Refuse a target whose tree as npm built it differs from crucible's,
    compared by path, SHA-256 and exec flag under every installed location,
    and give the files crucible's holds. npm makes runnable a file the `bin`
    of the package.json of the package holding it names, and never takes a
    mode away, and writes a `.gitignore` as `.npmignore`; neither is a
    difference. On Windows no file is runnable, so the exec flag is not
    compared."""
    locations = {item["location"] for item in installed}
    npm = tree_listing(place, "npm.json", built=False)
    built = tree_listing(place, "unpack-check.json", built=True)
    every = set(npm) | set(built)
    bins: set[str] = set()
    for item in installed:
        for path in (manifest_bins(item["manifest"], item["name"], item["location"], every) or {}).values():
            try:
                held = f"{item['location']}/{relative(path, 'its bin')}"
            except Dropped:
                continue  # npm links no bin it cannot place, so it marks nothing
            if owner_of(held, locations) == item["location"]:
                bins.add(held)
    ours: dict[str, dict] = {}
    for path, item in npm.items():
        if npm_own(path, locations):
            continue
        if owner_of(path, locations) is None:
            raise Dropped(f"npm installed {clipped(path)}, which lies under no package the lock installs here")
        ours[path] = item
    for path in built:
        directory, slash, leaf = path.rpartition("/")
        renamed = f"{directory}{slash}.npmignore"
        if leaf == ".gitignore" and path not in ours and renamed in ours and renamed not in built:
            ours[path] = ours.pop(renamed)
    windows = platform.startswith("windows-")
    for path in sorted(set(ours) | set(built)):
        if path not in ours:
            raise Dropped(f"npm's tree lacks {clipped(path)}, which crucible's holds")
        if path not in built:
            raise Dropped(f"crucible's tree lacks {clipped(path)}, which npm's holds")
        if ours[path]["sha256"] != built[path]["sha256"] or ours[path]["size"] != built[path]["size"]:
            raise Dropped(f"{clipped(path)} differs between npm's tree and crucible's")
        made_runnable = ours[path]["exec"] and not built[path]["exec"] and path in bins
        if not windows and ours[path]["exec"] != built[path]["exec"] and not made_runnable:
            raise Dropped(f"{clipped(path)} is runnable in one of npm's tree and crucible's and not the other")
    return set(built)


def manifest_bins(manifest: dict, name: str, location: str, files: set[str]) -> dict[str, str] | None:
    """The bins a package's package.json gives, by name, as paths in the
    package: a `bin` given as a path is named as the package is without its
    scope, and with no `bin`, each file of `files` under `directories.bin` is
    one, named as the file is; None where it gives none it can be read for."""
    unscoped = name.rpartition("/")[2]
    given = manifest.get("bin")
    directories = manifest.get("directories")
    if isinstance(given, str):
        return {unscoped: given}
    if isinstance(given, dict) and all(isinstance(path, str) for path in given.values()):
        return dict(given)
    if given is None and isinstance(directories, dict) and isinstance(directories.get("bin"), str):
        try:
            under = f"{location}/{relative(directories['bin'], 'its directories.bin')}/"
        except Dropped:
            return None
        return {path.rpartition("/")[2]: path[len(location) + 1:] for path in sorted(files) if path.startswith(under)}
    return None


def chosen_bin(own: dict, files: set[str]) -> str:
    """The bin `npx` would run for the agent's own package: the only one its
    package.json names, else the one named as the package is without its
    scope."""
    location = own["location"]
    unscoped = own["name"].rpartition("/")[2]
    bins = manifest_bins(own["manifest"], own["name"], location, files)
    if bins is None:
        raise Dropped("its package.json names no bin")
    if len(bins) == 1:
        [path] = bins.values()
    elif unscoped in bins:
        path = bins[unscoped]
    else:
        raise Dropped(f"its package.json names {len(bins)} bins and none {clipped(unscoped)}, so npx would run none")
    held = f"{location}/{relative(path, 'its bin')}"
    if held not in files:
        raise Dropped(f"its bin {clipped(held)} is not a file in the tree")
    return held


def npm_target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    resolution: dict,
    node: Node,
    decided: dict[str, str],
    assets: dict[str, bytes],
) -> dict:
    """One platform of an npm agent as the catalog will list it, before its
    probe: Node for it, and the lock of what `npm ci` installs on it, once the
    tree npm built there is the tree crucible builds from that lock."""
    artifact, hosts, cmd = node.artifact(observed, platform, assets)
    installed = npm_installed(resolution, platform)
    lock = npm_lock(installed)
    place = place_of(observed, agent, platform, "tree")
    try:
        made = read(place / "lock.json", ASSET_BYTES, "tree/lock.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if made != lock:
        raise Dropped("tree/lock.json is not the lock the intake computes for this platform, so the trees compared are of another")
    files = trees_agree(place, installed, platform)
    bin = chosen_bin(resolution["own"], files)
    named_lock = hashlib.sha256(lock).hexdigest()
    assets[named_lock] = lock
    scripts = [
        {"disposition": decided.get(f"{item['location']} {event}"), "event": event, "package": item["location"]}
        for item in installed
        for event in item["events"]
    ]
    return {
        "artifacts": [artifact],
        "bin": bin,
        "cmd": cmd,
        "hosts": hosts + ([NPM_HOST] if NPM_HOST not in hosts else []),
        "lock": {"sha256": named_lock, "size": len(lock), "unpack_rules": UNPACK_RULES},
        "node_args": [],
        "platform": platform,
        "scripts": scripts,
    }


def dispositions_of(path: pathlib.Path | None) -> dict[str, dict[str, str]]:
    """The reviewer's dispositions for the scripts `--ignore-scripts` skips, by
    registry id and then `<location> <event>`."""
    if path is None:
        return {}
    value = read_json(path, DISPOSITIONS_BYTES, "the dispositions")
    if not isinstance(value, dict) or not all(isinstance(item, dict) for item in value.values()):
        raise Refused("the dispositions are not an object by registry id of objects by script")
    for agent_id, scripts in value.items():
        for script, disposition in scripts.items():
            where = f"the disposition of {clipped(agent_id)} {clipped(script)}"
            if disposition in ("built", "local"):
                raise Refused(f"{where} is {disposition}, which belongs to the build route, not to this review")
            if disposition not in DISPOSITIONS:
                raise Refused(f"{where} is {clipped(disposition)}, not one of {', '.join(DISPOSITIONS)}")
    return value


def lock_revision(made: dict, held: dict | None) -> int:
    """The lock revision of an npm entry: 1 for a version not reviewed
    before; the merged one where every target's lock is the one merged; and
    one more than the merged one where any lock changed."""
    if held is None:
        return 1
    before = held.get("lock_revision")
    before = before if isinstance(before, int) and not isinstance(before, bool) and before > 0 else 1
    locks = {listed["platform"]: listed["lock"] for listed in made["targets"]}
    merged_locks = {
        str(listed.get("platform")): listed.get("lock")
        for listed in held.get("targets", [])
        if isinstance(listed, dict)
    }
    return before if locks == merged_locks else before + 1


def unlocked(entry: dict) -> dict:
    """An npm entry without its lock revision and each target's lock, the
    one part a version reviewed before may change in."""
    held = {field: value for field, value in entry.items() if field != "lock_revision"}
    targets = held.get("targets")
    if isinstance(targets, list):
        held["targets"] = [
            {field: value for field, value in listed.items() if field != "lock"} if isinstance(listed, dict) else listed
            for listed in targets
        ]
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
    node: Node,
    decided: dict[str, str],
    dropped: list[dict],
) -> tuple[dict, dict[str, bytes]]:
    distribution = agent.get("distribution")
    distribution = distribution if isinstance(distribution, dict) else {}
    # A binary wins, then `uvx`: `npx` is read only for an agent with neither.
    binary = distribution.get("binary")
    uvx = distribution.get("uvx") if binary is None else None
    npx = distribution.get("npx") if binary is None and uvx is None else None
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
    elif isinstance(npx, dict):
        kind = "npm"
        name, pinned_version = npx_package(npx.get("package"))
        args = arguments(npx.get("args"), "args")
        env = variables(npx.get("env"), "env")
    else:
        raise Dropped("is distributed neither as a binary nor through uvx or npx, the kinds this intake takes in")
    released = version(agent.get("version"), "the version")
    licence = text(agent.get("license"), "the licence")
    reached = reach(choice.get("reach"), "the chosen reach")
    mirrored = licence if licence in allowed else None
    assets: dict[str, bytes] = {}
    targets = []
    # npm's lock is read once for every target, where Node can run any.
    resolution = (
        npm_resolved(observed, agent_id, name, pinned_version)
        if kind == "npm" and node.pinned is not None and node.failure is None
        else None
    )
    for platform, spec in sorted(binary.items()) if kind == "binary" else [(platform, None) for platform in sorted(runners)]:
        try:
            if kind == "binary":
                targets.append(target(observed, agent_id, platform, spec, runners, mirrored, assets))
            elif kind == "npm":
                made = npm_target(observed, agent_id, platform, resolution, node, decided, assets)
                targets.append({"args": args, "env": env, **made})
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
    pinned_node = node_of(arguments.node)
    gpg = keys = None
    if pinned_node is not None:
        gpg = shutil.which("gpg")
        if gpg is None:
            raise Refused("gpg is not on PATH, so Node's signed checksums cannot be verified")
        keys = release_keys(arguments.node_keys or pathlib.Path(__file__).resolve().parent / "node-release-keys")
    dispositions = dispositions_of(arguments.dispositions)
    BUDGET = Budget()
    node = node_release(arguments.observed, pinned_node, gpg, keys)
    entries = []
    dropped: list[dict] = []
    assets: dict[str, bytes] = {}
    listed: set[str] = set()
    # Each top-level name of the observed tree, as a file system that folds
    # case names it: Node's own directory, and one directory per registry id.
    folded: dict[str, int] = {"node": 1}
    for agent in {agent.get("id") for agent in agents if isinstance(agent, dict) and isinstance(agent.get("id"), str)}:
        folded[agent.casefold()] = folded.get(agent.casefold(), 0) + 1
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
        if folded[agent_id.casefold()] > 1:
            # The job leaves Node's own downloads under `node/` beside each
            # agent's directory, and a folding file system takes two names
            # that differ in case alone for one.
            dropped.append({
                "reason": "its registry id names the same observed directory as node, where Node's downloads "
                "are left, or another agent's, once case is folded",
                "registry_id": agent_id,
            })
            continue
        choice = choices.get(agent_id)
        if choice is None:
            dropped.append({"reason": "has no key chosen by a reviewer yet", "registry_id": agent_id})
            continue
        try:
            made, kept = entry(
                arguments.observed, agent, agent_id, choice, runners, allowed, interpreters, node,
                dispositions.get(agent_id, {}), dropped,
            )
        except Exception as failed:  # whatever one entry breaks, the others go on
            dropped.append({"reason": reason(failed), "registry_id": agent_id})
            continue
        held = merged.get((made["key"], made["version"]))
        new, old = made, held
        if made["kind"] == "npm":
            # A version reviewed before may differ from the merged one in its
            # locks alone, which give its lock revision; any other difference
            # meets the refusal below.
            made["lock_revision"] = lock_revision(made, held)
            new, old = unlocked(made), None if held is None else unlocked(held)
        changed = differences(new, old) if old is not None else []
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
    # Each script a kept npm entry skipped that no reviewer has decided yet.
    undecided = sorted(
        {
            (made["registry_id"], script["package"], script["event"])
            for made in entries
            for listed_target in made["targets"]
            for script in listed_target.get("scripts", [])
            if script["disposition"] is None
        }
    )
    data = {
        "dropped": sorted(dropped, key=lambda item: (item["registry_id"], item.get("platform", ""))),
        "entries": sorted(entries, key=lambda item: item["key"]),
        "format": FORMAT,
        "registry": {"sha256": hashlib.sha256(text_read).hexdigest(), "size": len(text_read)},
        "runners": runners,
        "undecided": [
            {"event": event, "package": package, "registry_id": agent_id} for agent_id, package, event in undecided
        ],
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
        if entry["kind"] == "npm":
            npm_checked(entry, target, where)
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


def npm_checked(entry: dict, target: dict, where: str) -> None:
    """Refuse an npm target the client could not start, or one carrying a
    script no reviewer decided."""
    present(entry, "lock_revision", int, where)
    started = present(target, "node_args", list, f"{where}: a target")
    if not all(isinstance(item, str) for item in started):
        raise Refused(f"{where}: a target's node_args is not a list of text")
    try:
        relative(present(target, "bin", str, f"{where}: a target"), "bin")
    except Dropped as dropped:
        raise Refused(f"{where}: a target's {dropped}") from None
    lock = present(target, "lock", dict, f"{where}: a target")
    if lock.get("unpack_rules") != UNPACK_RULES or isinstance(lock.get("unpack_rules"), bool):
        raise Refused(f"{where}: a target's lock is not unpacked by unpack_rules {UNPACK_RULES}, the rules a client has")
    present(lock, "sha256", str, f"{where}: a target's lock")
    present(lock, "size", int, f"{where}: a target's lock")
    for script in present(target, "scripts", list, f"{where}: a target"):
        if not isinstance(script, dict) or script.get("disposition") not in DISPOSITIONS:
            shown_script = script.get("disposition") if isinstance(script, dict) else script
            raise Refused(
                f"{where}: a script's disposition is {clipped(shown_script)}, and a script is "
                f"published only once a reviewer decides it is one of {', '.join(DISPOSITIONS)}"
            )
        for name in ("package", "event"):
            present(script, name, str, f"{where}: a script")


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
    file-list and lock asset with its size, and each artifact with whether it may be
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
                lock = target.get("lock")
                if lock is not None:
                    # An npm target's lock is an asset as a file list is.
                    listings[digest(lock.get("sha256"), "a lock's sha256")] = number(lock.get("size"), "a lock's size")
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


def lock_of(lock: object) -> str:
    if not isinstance(lock, dict):
        return shown(lock)
    return (
        f"a lock asset {shown(lock.get('sha256'))} of {shown(lock.get('size'))} bytes, "
        f"unpack_rules {shown(lock.get('unpack_rules'))}"
    )


def scripts_of(scripts: object) -> list[str]:
    """Each script `--ignore-scripts` skipped, with its disposition, or marked
    for the reviewer where it has none."""
    if not isinstance(scripts, list):
        return [f"- scripts: {shown(scripts)}"]
    lines = []
    for script in scripts:
        script = script if isinstance(script, dict) else {}
        disposition = script.get("disposition")
        decided = "**undecided**" if disposition is None else shown(disposition)
        lines.append(f"- script {shown(script.get('package'))} {shown(script.get('event'))}: {decided}")
    return lines


def entry_lines(entry: dict, previous: dict | None, same: bool) -> list[str]:
    npm = entry.get("kind") == "npm"
    revised = same and npm and previous is not None and entry.get("lock_revision") != previous.get("lock_revision")
    if revised:
        was = " (**new lock revision** of a version already reviewed: its locks differ as below)"
    elif same:
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
    elif npm:
        lines.append("- not mirrored: Node and the npm tree are never mirrored")
        revision = entry.get("lock_revision")
        lines.append(
            f"- lock revision {shown(revision)}"
            + (f" (**new lock revision**, was {shown(previous.get('lock_revision'))})" if revised else "")
        )
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
        if npm:
            lines += compared("node args", listed.get("node_args", []), was_of("node_args"), words)
            lines += compared("bin", listed.get("bin"), was_of("bin"), shown)
            lines += compared("lock", listed.get("lock"), was_of("lock"), lock_of)
            # A script is shown whole every time, so an undecided one is never passed over.
            lines += scripts_of(listed.get("scripts", []))
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


def lock(arguments: argparse.Namespace) -> int:
    """Write the lock `review` computes for one platform, so the job builds
    and compares the tree a client would unpack."""
    try:
        name, released = npx_package(arguments.package)
        system, _, machine = arguments.platform.partition("-")
        if system not in NPM_OS or machine not in NPM_CPU:
            raise Dropped(f"{clipped(arguments.platform)} is not a platform the catalog lists")
        resolved = arguments.resolved
        resolution = npm_resolved(resolved.parent, resolved.name, name, released)
        held = npm_lock(npm_installed(resolution, arguments.platform))
    except Dropped as dropped:
        raise Refused(str(dropped)) from None
    arguments.out.write_bytes(held)
    return 0


def tree(arguments: argparse.Namespace) -> int:
    """List the regular files under `--root`, as npm's side of the tree
    comparison: by path, each file's SHA-256, size and whether any execute bit
    is set. A link is never followed and never listed; a directory is walked."""
    root = arguments.root
    try:
        held = os.lstat(root)
    except OSError:
        raise Refused("the tree's root is missing") from None
    if not stat.S_ISDIR(held.st_mode):
        raise Refused("the tree's root is not a directory")
    files = []
    total = 0
    waiting = [(root, "")]
    while waiting:
        place, prefix = waiting.pop()
        with os.scandir(place) as listing:
            for found in listing:
                path = prefix + found.name
                if len(files) + len(waiting) > TREE_ENTRIES:
                    raise Refused(f"the tree holds more than {TREE_ENTRIES} entries")
                if found.is_dir(follow_symlinks=False):
                    waiting.append((pathlib.Path(found.path), path + "/"))
                    continue
                if not found.is_file(follow_symlinks=False):
                    continue
                descriptor = os.open(found.path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                with os.fdopen(descriptor, "rb") as file:
                    mode = os.fstat(file.fileno()).st_mode
                    if not stat.S_ISREG(mode):
                        continue
                    sha = hashlib.sha256()
                    size = 0
                    while chunk := file.read(1 << 20):
                        size += len(chunk)
                        total += len(chunk)
                        if total > TREE_BYTES:
                            raise Refused(f"the tree holds more than {TREE_BYTES} bytes")
                        sha.update(chunk)
                files.append({"exec": bool(mode & 0o111), "path": path, "sha256": sha.hexdigest(), "size": size})
    written = compact({"files": sorted(files, key=lambda item: item["path"])})
    if len(written) > TREE_LISTING_BYTES:
        raise Refused(f"the tree's listing is larger than {TREE_LISTING_BYTES} bytes")
    arguments.out.write_bytes(written)
    return 0


def recheck(arguments: argparse.Namespace) -> int:
    """List each tarball the merged npm locks name, once, for the daily job
    to ask the registry for; or, given what it answered (`--results`, the
    status by SHA-512), propose a revocation of each lock revision holding a
    tarball the registry answered 404 or 410 for, which a fresh install could
    no longer fetch. Any other answer proposes nothing, since it may pass.
    The proposals are written as entries of the merged data's `revoked`."""
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    entries = merged.get("entries") if isinstance(merged, dict) else None
    if not isinstance(entries, list):
        raise Refused("the merged data is not review data as the intake writes it")
    tarballs: dict[str, str] = {}
    holders: dict[str, set[tuple[str, str, int]]] = {}
    for listed in entries:
        if not isinstance(listed, dict) or listed.get("kind") != "npm":
            continue
        held = (str(listed.get("key")), str(listed.get("version")), whole(listed.get("lock_revision"), "a lock revision"))
        for listed_target in listed.get("targets", []):
            named = digest(listed_target.get("lock", {}).get("sha256"), "a lock's sha256")
            raw = read(arguments.assets / named, ASSET_BYTES, f"lock asset {named}")
            if hashlib.sha256(raw).hexdigest() != named:
                raise Refused(f"lock asset {named} is not the bytes its name claims")
            packages = parsed(raw, f"lock asset {named}")
            packages = packages.get("packages") if isinstance(packages, dict) else None
            if not isinstance(packages, list):
                raise Refused(f"lock asset {named} lists no packages")
            for package in packages:
                if not isinstance(package, dict) or package.get("in_bundle") is True:
                    continue
                sha512 = package.get("sha512")
                url = package.get("url")
                if not isinstance(sha512, str) or not re.fullmatch(r"[0-9a-f]{128}", sha512) or not isinstance(url, str):
                    raise Refused(f"lock asset {named} lists a tarball with no SHA-512 or address")
                tarballs.setdefault(sha512, url)
                holders.setdefault(sha512, set()).add(held)
    if arguments.results is None:
        listing = [{"sha512": sha512, "url": url} for sha512, url in sorted(tarballs.items(), key=lambda item: item[1])]
        arguments.out.write_bytes(compact({"tarballs": listing}))
        return 0
    results = read_json(arguments.results, SMALL_BYTES, "the results")
    if not isinstance(results, dict):
        raise Refused("the results are not an object of statuses by SHA-512")
    # Every tarball the registry no longer serves, by the lock revision
    # that holds it, so each revision is proposed once however many are gone.
    lost: dict[tuple[str, str, int], list[tuple[str, int]]] = {}
    for sha512, status in results.items():
        if sha512 not in tarballs or isinstance(status, bool) or not isinstance(status, int):
            raise Refused(f"the results give {clipped(sha512)}, which is not a listed tarball with a whole status")
        if status in (404, 410):
            for holder in holders[sha512]:
                lost.setdefault(holder, []).append((tarballs[sha512], status))
    revoked = []
    for (key, released, revision), gone in sorted(lost.items()):
        url, status = min(gone)
        more = f", and {len(gone) - 1} more of its tarballs are gone" if len(gone) > 1 else ""
        # The address is the lock's, which no reviewer wrote, so it is
        # quoted as the diff quotes hostile text.
        reason = f"its tarball {shown(clipped(url))} is answered {status} by the registry{more}"
        revoked.append({"key": key, "lock_revision": revision, "reason": reason, "version": released})
    arguments.out.write_bytes(compact(revoked))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    reviewing = commands.add_parser("review", help="turn registry binary, uvx and npx entries into review data")
    for name in ("registry", "choices", "runners", "observed", "deny", "assets", "out"):
        reviewing.add_argument(f"--{name}", type=pathlib.Path, required=True)
    reviewing.add_argument(
        "--merged", type=pathlib.Path, help="the merged data, against which a version already reviewed is held"
    )
    reviewing.add_argument(
        "--interpreters", type=pathlib.Path, help="the interpreter pinned for each platform a uvx agent runs on"
    )
    reviewing.add_argument("--node", type=pathlib.Path, help="the Node release npm agents run on")
    reviewing.add_argument("--node-keys", type=pathlib.Path, help="the keys a Node release may be signed under")
    reviewing.add_argument("--dispositions", type=pathlib.Path, help="the reviewer's decision on each skipped script")
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
    locking = commands.add_parser("lock", help="write the lock an npm agent's tree is unpacked from on one platform")
    locking.add_argument("--resolved", type=pathlib.Path, required=True, help="the agent's observed directory")
    locking.add_argument("--package", required=True, help="the agent's package, as name@version")
    locking.add_argument("--platform", required=True)
    locking.add_argument("--out", type=pathlib.Path, required=True)
    listing = commands.add_parser("tree", help="list the files of the tree npm installed")
    listing.add_argument("--root", type=pathlib.Path, required=True)
    listing.add_argument("--out", type=pathlib.Path, required=True)
    rechecking = commands.add_parser("recheck", help="list merged npm tarballs, or propose revocations for gone ones")
    rechecking.add_argument("--merged", type=pathlib.Path, required=True)
    rechecking.add_argument("--assets", type=pathlib.Path, required=True, help="the directory holding the lock assets")
    rechecking.add_argument("--results", type=pathlib.Path, help="the registry's status for each tarball, by SHA-512")
    rechecking.add_argument("--out", type=pathlib.Path, required=True)
    arguments = parser.parse_args()
    commands_run = {
        "review": review, "diff": diff, "render": render, "publish-check": publish_check,
        "lock": lock, "tree": tree, "recheck": recheck,
    }
    try:
        return commands_run[arguments.command](arguments)
    except Refused as refused:
        print(f"agent-catalog-intake: {refused}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
