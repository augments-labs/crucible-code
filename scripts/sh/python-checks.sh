#!/usr/bin/env bash
# Deterministic checks owned by the maintained Python harnesses.
set -uo pipefail

cd "$(dirname "$0")/../.."

failed=0

# Every file in the directory, not a list beside it: a harness added without
# being named here would otherwise never be compiled by this gate.
if ! python3 - <<'PYTHON'
import ast
import pathlib
found = sorted(pathlib.Path("scripts/python").glob("*.py"))
if not found:
    raise SystemExit("scripts/python holds no harness to compile")
for path in found:
    ast.parse(path.read_bytes(), filename=str(path))
PYTHON
then
    printf '    FAIL portable harness Python does not compile\n'
    failed=1
fi
if ! PYTHONDONTWRITEBYTECODE=1 scripts/python/validate-provider-canary.py; then
    printf '    FAIL provider canary fixture did not enforce tool and usage journal facts\n'
    failed=1
fi
if ! PYTHONDONTWRITEBYTECODE=1 scripts/python/validate-task-campaign.py; then
    printf '    FAIL task campaign reports did not preserve usage, cost and comparison facts\n'
    failed=1
fi
if ! scripts/python/task-campaign.py validate --suite benchmarks/coding-tasks/suite.json; then
    printf '    FAIL the coding-task campaign suite is not self-contained and valid\n'
    failed=1
fi
if ! PYTHONDONTWRITEBYTECODE=1 scripts/python/validate-release-notes.py; then
    printf '    FAIL release notes are not the summary and comparison link of their section\n'
    failed=1
fi
# The release workflow writes the body after the tag is pushed, where a refusal
# leaves a tag to repair: the change that bumps the version is where a section
# without a summary or comparison link has to stop.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if ! PYTHONDONTWRITEBYTECODE=1 scripts/python/release-notes.py "$version" >/dev/null; then
    printf '    FAIL CHANGELOG.md cannot give %s its release notes\n' "$version"
    failed=1
fi

if ((failed)); then
    exit 1
fi

echo 'all Python checks passed'
