#!/usr/bin/env python3
"""Unit tests for the release body written from a changelog section."""

from __future__ import annotations

import importlib.util
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("release_notes", ROOT / "release-notes.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

COMPARE = "https://github.com/example/project/compare"

CHANGELOG = f"""# Changelog

## [Unreleased]

## [1.2.0-rc.1] - 2026-01-04

**A candidate whose heading starts with the next version's.**

## [1.1.0] - 2026-01-03

**Sessions come back faster.** Picking one up reads only the end of its file.

### Fixed

- **A fix the body leaves to the changelog.** It is listed there in full.

## [1.0.1] - 2026-01-02

### Fixed

- **A fix with no summary above it.**

## [1.0.0] - 2026-01-01

**The first release.** Nothing came before it to list.

[Unreleased]: {COMPARE}/v1.2.0-rc.1...HEAD
[1.2.0-rc.1]: {COMPARE}/v1.1.0...v1.2.0-rc.1
[1.1.0]: {COMPARE}/v1.0.1...v1.1.0
[1.0.1]: {COMPARE}/v1.0.0...v1.0.1
[1.0.0]: {COMPARE}/v0.9.0...v1.0.0
"""


def refused(changelog: str, version: str, reason: str) -> None:
    try:
        body = MODULE.notes(changelog, version)
    except MODULE.Refused as refusal:
        assert reason in str(refusal), refusal
    else:
        raise AssertionError(f"{version} was given a body despite {reason}:\n{body}")


def main() -> int:
    body = MODULE.notes(CHANGELOG, "1.1.0")
    assert body == (
        "**Sessions come back faster.** Picking one up reads only the end of its file.\n"
        "\n"
        f"**Full changelog**: [v1.0.1...v1.1.0]({COMPARE}/v1.0.1...v1.1.0)\n"
        "\n"
        "Every change in this release is listed in "
        "[CHANGELOG.md](https://github.com/example/project/blob/v1.1.0/CHANGELOG.md).\n"
    ), body

    # The oldest section has no list to stop at, only the link definitions.
    oldest = MODULE.notes(CHANGELOG, "1.0.0")
    assert oldest.startswith(
        "**The first release.** Nothing came before it to list.\n\n**Full changelog**"
    ), oldest

    # A `+` in build metadata is matched as itself.
    built = MODULE.notes(CHANGELOG.replace("1.1.0", "1.1.0+1"), "1.1.0+1")
    assert built == body.replace("1.1.0", "1.1.0+1"), built

    refused(CHANGELOG, "1.0.1", "no summary")
    refused(CHANGELOG, "1.2.0", "no section")
    refused(
        CHANGELOG.replace(f"[1.1.0]: {COMPARE}/v1.0.1...v1.1.0\n", ""),
        "1.1.0",
        "no comparison link",
    )
    refused(
        CHANGELOG.replace("v1.0.1...v1.1.0", "v1.0.1...v1.1.1"),
        "1.1.0",
        "ends at v1.1.1",
    )

    print("release notes validator passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
