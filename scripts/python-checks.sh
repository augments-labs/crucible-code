#!/usr/bin/env bash
# Deterministic checks owned by the maintained Python harnesses.
set -uo pipefail

cd "$(dirname "$0")/.."

failed=0

if ! python3 - <<'PYTHON'
import ast
import pathlib
for name in (
    "scripts/provider-canary.py",
    "scripts/task-campaign.py",
    "scripts/validate-provider-canary.py",
    "scripts/validate-task-campaign.py",
):
    ast.parse(pathlib.Path(name).read_bytes(), filename=name)
PYTHON
then
    printf '    FAIL portable harness Python does not compile\n'
    failed=1
fi
if ! PYTHONDONTWRITEBYTECODE=1 scripts/validate-provider-canary.py; then
    printf '    FAIL provider canary fixture did not enforce tool and usage journal facts\n'
    failed=1
fi
if ! PYTHONDONTWRITEBYTECODE=1 scripts/validate-task-campaign.py; then
    printf '    FAIL task campaign reports did not preserve usage, cost and comparison facts\n'
    failed=1
fi
if ! scripts/task-campaign.py validate --suite benchmarks/coding-tasks/suite.json; then
    printf '    FAIL the coding-task campaign suite is not self-contained and valid\n'
    failed=1
fi

if ((failed)); then
    exit 1
fi

echo 'all Python checks passed'
