#!/usr/bin/env python3
"""Hermetic checks for coding-task campaign usage and comparison reports."""

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
from types import SimpleNamespace

ROOT = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("task_campaign", ROOT / "task-campaign.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def usage(home: pathlib.Path) -> None:
    sessions = home / "sessions"
    sessions.mkdir(parents=True)
    body = {
        "kind": "provider_attempt",
        "event": "usage",
        "usage": {"input": {"total": 14}, "output": 3},
        "cost": {"total": {"femtocurrency": "1200", "currency": "USD"}},
    }
    (sessions / "one.jsonl").write_text(
        json.dumps({"run_item": {"body": body}}) + "\n", encoding="utf-8"
    )


def campaign(path: pathlib.Path, label: str, passed: int, duration: float, cost: int) -> None:
    value = {
        "version": 1,
        "label": label,
        "tasks": [{"name": "fixture"}],
        "summary": {
            "passed": passed,
            "total": 1,
            "duration_seconds": duration,
            "input_tokens": 14,
            "output_tokens": 3,
            "cost_femtocurrency": {"USD": cost},
        },
    }
    path.write_text(json.dumps(value), encoding="utf-8")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="crucible-task-validator-") as directory:
        root = pathlib.Path(directory)
        home = root / "home"
        usage(home)
        assert MODULE.metrics(home) == (14, 3, {"USD": 1200})
        assert MODULE.cost_summary(
            [
                {"cost_femtocurrency": {"USD": 1}},
                {"cost_femtocurrency": {"EUR": 2, "USD": 3}},
            ]
        ) == {"EUR": 2, "USD": 4}

        baseline = root / "baseline.json"
        candidate = root / "candidate.json"
        compared = root / "comparison.json"
        campaign(baseline, "baseline", 0, 4.0, 1200)
        campaign(candidate, "candidate", 1, 3.5, 1000)
        args = SimpleNamespace(
            baseline=str(baseline), candidate=str(candidate), output=str(compared)
        )
        assert MODULE.compare(args) == 0
        delta = json.loads(compared.read_text(encoding="utf-8"))["delta"]
        assert delta == {
            "passed": 1,
            "duration_seconds": -0.5,
            "input_tokens": 0,
            "output_tokens": 0,
            "cost_femtocurrency": {"USD": -200},
        }

    print("task campaign validator passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
