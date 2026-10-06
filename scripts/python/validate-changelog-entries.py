#!/usr/bin/env python3
"""Unit tests for the changelog entry files and the section built from them."""

from __future__ import annotations

import importlib.util
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent
SCRIPT = ROOT / "changelog-entries.py"
SPEC = importlib.util.spec_from_file_location("changelog_entries", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

CHANGELOG = """# Changelog

Notable changes.

## [Unreleased]

## [1.0.0] - 2026-01-01

**The first release.** Nothing came before it to list.

[Unreleased]: https://github.com/example/project/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/example/project/compare/v0.9.0...v1.0.0
"""

README = "# Changelog entries\n\nOne file per change.\n"

FIXED = "### Fixed\n\n- **A fix.** It is listed in full.\n"

# Every tree is made under one directory, removed when the run ends.
WORK = tempfile.TemporaryDirectory(prefix="changelog-entries-")


def tree(entries: dict[str, str | bytes], changelog: str = CHANGELOG) -> pathlib.Path:
    root = pathlib.Path(tempfile.mkdtemp(dir=WORK.name))
    (root / "CHANGELOG.md").write_text(changelog, encoding="utf-8")
    folder = root / "changelog.d"
    folder.mkdir()
    (folder / "README.md").write_text(README, encoding="utf-8")
    for name, body in entries.items():
        path = folder / name
        if isinstance(body, bytes):
            path.write_bytes(body)
        else:
            path.write_text(body, encoding="utf-8")
    return root


def snapshot(root: pathlib.Path) -> dict[str, bytes]:
    return {
        str(path.relative_to(root)): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def broken(entries: dict[str, str | bytes], file: str, rule: str, changelog: str = CHANGELOG) -> None:
    problems = MODULE.check(tree(entries, changelog))
    named = [problem for problem in problems if problem.startswith(f"{file}: ") and rule in problem]
    assert named, f"{file} was not refused for {rule!r}: {problems}"


def run(root: pathlib.Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *arguments, "--root", str(root)],
        capture_output=True,
        text=True,
        check=False,
    )


def check_rules() -> None:
    # Every heading rule 1 allows, one item, continuation lines and a second item.
    good = {
        f"{heading.lower()}-entry.md": f"### {heading}\n\n- **A change.**\n  Continued.\n- **Another.**\n"
        for heading in MODULE.HEADINGS
    }
    assert MODULE.check(tree(good)) == [], MODULE.check(tree(good))
    assert MODULE.check(tree({})) == []

    # The name.
    broken({"Upper.md": FIXED}, "changelog.d/Upper.md", "name")
    broken({"-lead.md": FIXED}, "changelog.d/-lead.md", "name")
    broken({"under_score.md": FIXED}, "changelog.d/under_score.md", "name")
    broken({"entry.txt": FIXED}, "changelog.d/entry.txt", "name")
    broken({".hidden.md": FIXED}, "changelog.d/.hidden.md", "name")
    folder = tree({})
    (folder / "changelog.d" / "nested").mkdir()
    assert any(p.startswith("changelog.d/nested: ") for p in MODULE.check(folder)), MODULE.check(folder)

    # Line 1, line 2 and line 3.
    broken({"other.md": "### Other\n\n- **A change.**\n"}, "changelog.d/other.md", "line 1")
    broken({"spaced.md": "### Fixed \n\n- **A change.**\n"}, "changelog.d/spaced.md", "line 1")
    broken({"tight.md": "### Fixed\n- **A change.**\n"}, "changelog.d/tight.md", "line 2")
    broken({"plain.md": "### Fixed\n\n- A change with no bold lead.\n"}, "changelog.d/plain.md", "line 3")
    broken({"empty.md": "### Fixed\n\n"}, "changelog.d/empty.md", "line 3")
    broken({"nothing.md": ""}, "changelog.d/nothing.md", "line 1")

    # After line 3: list items and their continuation only, and no heading.
    broken(
        {"heading.md": "### Fixed\n\n- **A change.**\n\n### Added\n\n- **More.**\n"},
        "changelog.d/heading.md",
        "line 5 is a heading",
    )
    broken(
        {"prose.md": "### Fixed\n\n- **A change.**\n\nA paragraph outside the list.\n"},
        "changelog.d/prose.md",
        "line 5 is neither",
    )

    # The size and the encoding.
    item = "- **A change.**\n"
    large = "### Fixed\n\n" + item * (4096 // len(item))
    assert len(large.encode()) > 4096
    broken({"large.md": large}, "changelog.d/large.md", "4096 bytes")
    fits = "### Fixed\n\n" + item * ((4096 - 11) // len(item))
    assert len(fits.encode()) <= 4096
    assert MODULE.check(tree({"fits.md": fits})) == []
    broken({"binary.md": b"### Fixed\n\n- **\xff**\n"}, "changelog.d/binary.md", "UTF-8")

    # Rule 2: nothing is written under `## [Unreleased]` by hand.
    for written in (
        CHANGELOG.replace("## [Unreleased]\n", "## [Unreleased]\n\n- **By hand.**\n"),
        CHANGELOG.replace("## [Unreleased]\n", "## [Unreleased]\n\n### Fixed\n"),
    ):
        broken({}, "CHANGELOG.md", "Unreleased", written)
    last = "# Changelog\n\n## [Unreleased]\n\n- **By hand.**\n\n[Unreleased]: https://example.com\n"
    broken({}, "CHANGELOG.md", "Unreleased", last)
    blank = "# Changelog\n\n## [Unreleased]\n\n   \n\n[Unreleased]: https://example.com\n"
    assert MODULE.check(tree({}, blank)) == []


def check_command() -> None:
    root = tree({"fine.md": FIXED})
    passed = run(root, "check")
    assert passed.returncode == 0, passed

    root = tree({"other.md": "### Other\n\n- **A change.**\n"})
    failed = run(root, "check")
    assert failed.returncode == 1, failed
    assert "changelog.d/other.md" in failed.stdout, failed

    root = tree({}, CHANGELOG.replace("## [Unreleased]\n", "## [Unreleased]\n\n- **By hand.**\n"))
    failed = run(root, "check")
    assert failed.returncode == 1, failed
    assert "CHANGELOG.md" in failed.stdout, failed


def assemble_rules() -> None:
    root = tree(
        {
            "b-second.md": "### Fixed\n\n- **The second fix.**\n  Continued.\n\n",
            "a-first.md": "### Fixed\n\n- **The first fix.**\n- **Its sibling.**\n",
            "z-added.md": "### Added\n\n- **A feature.**\n",
            "internal.md": "### Internal\n\n- **A gate.**\n",
            "security.md": "### Security\n\n- **A hole closed.**\n",
        }
    )
    done = run(root, "assemble", "--version", "1.1.0", "--date", "2026-02-03")
    assert done.returncode == 0, done
    expected = CHANGELOG.replace(
        "## [Unreleased]\n\n",
        "## [Unreleased]\n"
        "\n"
        "## [1.1.0] - 2026-02-03\n"
        "\n"
        "### Added\n"
        "\n"
        "- **A feature.**\n"
        "\n"
        "### Fixed\n"
        "\n"
        "- **The first fix.**\n"
        "- **Its sibling.**\n"
        "- **The second fix.**\n"
        "  Continued.\n"
        "\n"
        "### Security\n"
        "\n"
        "- **A hole closed.**\n"
        "\n"
        "### Internal\n"
        "\n"
        "- **A gate.**\n"
        "\n",
    )
    written = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    assert written == expected, written
    left = sorted(path.name for path in (root / "changelog.d").iterdir())
    assert left == ["README.md"], left
    assert MODULE.check(root) == [], MODULE.check(root)

    # Names are ordered by their bytes, so a digit comes before a letter.
    root = tree({"b.md": "### Fixed\n\n- **B.**\n", "9.md": "### Fixed\n\n- **Nine.**\n"})
    assert run(root, "assemble", "--version", "1.1.0", "--date", "2026-02-03").returncode == 0
    written = (root / "CHANGELOG.md").read_text(encoding="utf-8")
    assert written.index("**Nine.**") < written.index("**B.**"), written


def refused(root: pathlib.Path, reason: str, *arguments: str) -> None:
    before = snapshot(root)
    result = run(root, "assemble", *arguments)
    assert result.returncode == 1, result
    assert reason in result.stderr, result
    assert snapshot(root) == before, f"a refusal for {reason!r} changed the tree"


def assemble_refusals() -> None:
    version = ("--version", "1.1.0", "--date", "2026-02-03")
    refused(tree({}), "no entry file", *version)
    refused(tree({"fine.md": FIXED}), "already has a section for 1.0.0", "--version", "1.0.0", "--date", "2026-02-03")
    refused(tree({"other.md": "### Other\n\n- **A change.**\n"}), "changelog.d/other.md", *version)
    refused(
        tree({"fine.md": FIXED}, CHANGELOG.replace("## [Unreleased]\n", "## [Unreleased]\n\n- **By hand.**\n")),
        "Unreleased",
        *version,
    )
    refused(tree({"fine.md": FIXED}, CHANGELOG.replace("## [Unreleased]\n\n", "")), "no ## [Unreleased]", *version)
    refused(tree({"fine.md": FIXED}), "YYYY-MM-DD", "--version", "1.1.0", "--date", "2026-2-3")
    refused(tree({"fine.md": FIXED}), "YYYY-MM-DD", "--version", "1.1.0", "--date", "2026-02-30")
    refused(tree({"fine.md": FIXED}), "version", "--version", "1.1.0]", "--date", "2026-02-03")


def main() -> int:
    with WORK:
        check_rules()
        check_command()
        assemble_rules()
        assemble_refusals()
    print("changelog entries validator passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
