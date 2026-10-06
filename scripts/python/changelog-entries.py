#!/usr/bin/env python3
"""Changelog entries kept one file per change, and the section built from them.

A pull request writes its entry to `changelog.d/<name>.md` instead of editing
`CHANGELOG.md`, so two pull requests open at once never edit the same lines,
and only a release commit writes the changelog. An entry file is one heading
from the changelog's own list, an empty line, and one or more list items, the
first opening with a bold lead; it holds no heading of its own and is at most
4096 bytes. `changelog.d/README.md` says so to whoever opens the directory and
is the one file there that is not an entry.

`check` holds every entry file to that shape and `## [Unreleased]` to staying
empty, since an entry written there by hand is the conflict the directory
exists to prevent. `assemble` writes a version section from the entries,
directly under the empty `## [Unreleased]`, headings in the changelog's order
and entries under one heading in byte order of their names, then deletes the
entry files. It writes the lists alone: the summary above them and the
comparison link are written by the person cutting the release, and
`release-notes.py` refuses the section until they are.
"""

from __future__ import annotations

import argparse
import datetime
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]

HEADINGS = ("Added", "Changed", "Fixed", "Removed", "Security", "Documentation", "Internal")
ENTRIES = "changelog.d"
README = "README.md"
NAME = re.compile(r"[a-z0-9][a-z0-9-]*\.md")
MAX_BYTES = 4096
VERSION = re.compile(r"[0-9A-Za-z.+-]+")
DATE = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")
UNRELEASED = "## [Unreleased]"


class Refused(Exception):
    """The entries or the changelog cannot be assembled as they stand."""


def entry_problems(label: str, data: bytes) -> list[str]:
    """What keeps one entry file from rule 1, each naming the file."""
    if len(data) > MAX_BYTES:
        return [f"{label}: is {len(data)} bytes; an entry is at most {MAX_BYTES} bytes"]
    try:
        lines = data.decode("utf-8").split("\n")
    except UnicodeDecodeError:
        return [f"{label}: is not UTF-8"]
    problems = []
    if lines[0] not in {f"### {heading}" for heading in HEADINGS}:
        allowed = ", ".join(f"`### {heading}`" for heading in HEADINGS)
        problems.append(f"{label}: line 1 must be exactly one of {allowed}")
    if len(lines) < 2 or lines[1] != "":
        problems.append(f"{label}: line 2 must be empty")
    if len(lines) < 3 or not lines[2].startswith("- **"):
        problems.append(f"{label}: line 3 must open a list item with a bold lead, `- **`")
    for number, line in enumerate(lines[3:], start=4):
        if line.startswith("#"):
            problems.append(f"{label}: line {number} is a heading; an entry holds only list items")
        elif line.strip() and not line.startswith(("- ", " ", "\t")):
            problems.append(f"{label}: line {number} is neither a list item nor its continuation")
    return problems


def unreleased_problems(changelog: str) -> list[str]:
    """`## [Unreleased]` holds nothing: entries live in `changelog.d/`."""
    lines = changelog.splitlines()
    if UNRELEASED not in lines:
        return []
    for line in lines[lines.index(UNRELEASED) + 1 :]:
        if line.startswith("## [") or re.match(r"\[[^\]]+\]: ", line):
            break
        if line.strip():
            return [
                f"CHANGELOG.md: `{UNRELEASED}` must stay empty; "
                f"write the entry to {ENTRIES}/<name>.md instead"
            ]
    return []


def entries(root: pathlib.Path) -> tuple[list[tuple[str, bytes]], list[str]]:
    """The entry files by name in byte order, and what is wrong with any of them."""
    found, problems = [], []
    folder = root / ENTRIES
    if not folder.is_dir():
        return found, problems
    for path in sorted(folder.iterdir(), key=lambda path: path.name.encode()):
        if path.name == README:
            continue
        label = f"{ENTRIES}/{path.name}"
        if not path.is_file() or path.is_symlink():
            problems.append(f"{label}: is not a regular file; {ENTRIES} holds only entry files")
            continue
        if not NAME.fullmatch(path.name):
            problems.append(f"{label}: the name must match `[a-z0-9][a-z0-9-]*` followed by `.md`")
        data = path.read_bytes()
        problems.extend(entry_problems(label, data))
        found.append((path.name, data))
    return found, problems


def check(root: pathlib.Path) -> list[str]:
    """Every problem with the entry files and with `## [Unreleased]`."""
    _, problems = entries(root)
    changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    return problems + unreleased_problems(changelog)


def section(found: list[tuple[str, bytes]], version: str, date: str) -> list[str]:
    """The version section's lines: its heading, then each heading's items."""
    grouped: dict[str, list[str]] = {heading: [] for heading in HEADINGS}
    for _, data in found:
        lines = data.decode("utf-8").split("\n")
        items = lines[2:]
        while items and not items[-1].strip():
            items.pop()
        grouped[lines[0].removeprefix("### ")].extend(items)
    out = [f"## [{version}] - {date}", ""]
    for heading, items in grouped.items():
        if items:
            out += [f"### {heading}", "", *items, ""]
    return out


def assemble(root: pathlib.Path, version: str, date: str) -> None:
    """Write the version section and delete the entries, or change nothing."""
    if not VERSION.fullmatch(version):
        raise Refused(f"the version {version!r} is not a version as Cargo.toml writes one")
    if not DATE.fullmatch(date):
        raise Refused(f"the date {date!r} is not YYYY-MM-DD")
    try:
        datetime.date.fromisoformat(date)
    except ValueError:
        raise Refused(f"the date {date!r} is not a day on the calendar, as YYYY-MM-DD") from None

    path = root / "CHANGELOG.md"
    changelog = path.read_text(encoding="utf-8")
    problems = check(root)
    if problems:
        raise Refused("\n".join(problems))
    lines = changelog.split("\n")
    if UNRELEASED not in lines:
        raise Refused(f"CHANGELOG.md has no {UNRELEASED} to write the section under")
    heading = re.compile(r"## \[" + re.escape(version) + r"\]")
    if any(heading.match(line) for line in lines):
        raise Refused(f"CHANGELOG.md already has a section for {version}")
    found, _ = entries(root)
    if not found:
        raise Refused(f"there is no entry file in {ENTRIES} to assemble")

    # Unreleased is empty, so everything up to the next heading or link is blank.
    start = lines.index(UNRELEASED) + 1
    end = start
    while end < len(lines) and not lines[end].strip():
        end += 1
    rebuilt = lines[:start] + [""] + section(found, version, date) + lines[end:]
    path.write_text("\n".join(rebuilt), encoding="utf-8")
    for name, _ in found:
        (root / ENTRIES / name).unlink()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    # The tree the commands act on; the validator points it at a copy.
    shared = argparse.ArgumentParser(add_help=False)
    shared.add_argument("--root", type=pathlib.Path, default=ROOT, help=argparse.SUPPRESS)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check", parents=[shared], help="hold the entry files and Unreleased to their rules")
    build = commands.add_parser("assemble", parents=[shared], help="write a version section from the entry files")
    build.add_argument("--version", required=True, help="the version without its v, as in Cargo.toml")
    build.add_argument("--date", required=True, help="the release day, YYYY-MM-DD")
    arguments = parser.parse_args()

    if arguments.command == "check":
        problems = check(arguments.root)
        for problem in problems:
            print(problem)
        return 1 if problems else 0
    try:
        assemble(arguments.root, arguments.version, arguments.date)
    except Refused as refusal:
        print(refusal, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
