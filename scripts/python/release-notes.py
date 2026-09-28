#!/usr/bin/env python3
"""The body a GitHub release is published with, written from CHANGELOG.md.

A version section opens with a summary: a bold lead and at most three sentences
for someone deciding whether to upgrade, above the section's first `###` list. The
body is that summary, the section's comparison link from the foot of the file,
and a link to the changelog as it stands at the tag, where every change is
listed. The lists stay out of the body: a release page that repeats every entry
is too long to read, and the changelog already keeps them under review.

The release workflow writes the body after the tag is pushed, where a refusal
leaves a tag to repair, so a section this cannot write from has to be caught
before then; the Python gate runs it for the version in `Cargo.toml` on every
change.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]


class Refused(Exception):
    """The changelog cannot give this version a release body."""


def notes(changelog: str, version: str) -> str:
    lines = changelog.splitlines()
    heading = re.compile(r"## \[" + re.escape(version) + r"\]")
    start = next((index for index, line in enumerate(lines) if heading.match(line)), None)
    if start is None:
        raise Refused(f"CHANGELOG.md has no section for {version}")

    summary = []
    for line in lines[start + 1 :]:
        if line.startswith(("### ", "## [")) or re.match(r"\[[^\]]+\]: ", line):
            break
        summary.append(line)
    lead = "\n".join(summary).strip()
    if not lead:
        raise Refused(f"the {version} section has no summary above its first ### list")

    link = re.compile(r"\[" + re.escape(version) + r"\]: (\S+)/compare/(\S+)\.\.\.(\S+)$")
    found = next((found for line in lines if (found := link.match(line))), None)
    if found is None:
        raise Refused(f"CHANGELOG.md has no comparison link for {version}")
    repository, before, after = found.groups()
    if after != f"v{version}":
        raise Refused(f"the comparison link for {version} ends at {after}")

    return (
        f"{lead}\n"
        "\n"
        f"**Full changelog**: [{before}...{after}]({repository}/compare/{before}...{after})\n"
        "\n"
        "Every change in this release is listed in "
        f"[CHANGELOG.md]({repository}/blob/{after}/CHANGELOG.md).\n"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("version", help="the version without its v, as in Cargo.toml")
    parser.add_argument("--changelog", type=pathlib.Path, default=ROOT / "CHANGELOG.md")
    arguments = parser.parse_args()
    try:
        body = notes(arguments.changelog.read_text(encoding="utf-8"), arguments.version)
    except Refused as refusal:
        print(refusal, file=sys.stderr)
        return 1
    sys.stdout.write(body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
