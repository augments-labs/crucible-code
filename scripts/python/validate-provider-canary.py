#!/usr/bin/env python3
"""Unit tests for stable provider-canary session validation."""

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
from types import SimpleNamespace

ROOT = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("provider_canary", ROOT / "provider-canary.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def encoded(body: dict) -> str:
    return json.dumps({"run_item": {"version": 1, "body": body}})


def fixture(root: pathlib.Path, *, usage: bool = True, succeeded: bool = True) -> SimpleNamespace:
    home = root / "home"
    workspace = root / "work"
    (home / "sessions").mkdir(parents=True)
    workspace.mkdir()
    (workspace / "canary.txt").write_text(MODULE.FILE_TEXT, encoding="utf-8")
    invocation = {
        "kind": "invocation",
        "tool": "write",
        "arguments": {"path": "canary.txt"},
        "invocation_state": {
            "state": "finished",
            "outcome": "succeeded" if succeeded else "failed",
        },
    }
    read_invocation = {
        "kind": "invocation",
        "tool": "read",
        "arguments": {"path": "canary.txt"},
        "invocation_state": {"state": "finished", "outcome": "succeeded"},
    }
    usage_body = {
        "kind": "provider_attempt",
        "event": "usage",
        "outcome": "read",
        "usage": {
            "input": {
                "total": 12,
                "uncached": 4,
                "cache_read": 8,
                "cache_write_or_creation": 0,
            },
            "output": 3,
            "reasoning": None,
            "total": 15,
            "storage_token_hours": None,
        },
        "cost": {"total": None},
    }
    lines = [encoded(invocation), encoded(read_invocation)]
    if usage:
        lines.extend([encoded(usage_body), encoded(usage_body)])
    (home / "sessions" / "one.jsonl").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return SimpleNamespace(
        home=str(home),
        workspace=str(workspace),
        model="provider/model",
    )


def expect_failure(args: SimpleNamespace, contains: str) -> None:
    try:
        MODULE.validate(
            args,
            0,
            f"{MODULE.MARKER}\nprovider tool round trip-verified\n",
            1.0,
        )
    except RuntimeError as problem:
        assert contains in str(problem), problem
    else:
        raise AssertionError(f"validation unexpectedly accepted missing {contains}")


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="crucible-provider-validator-") as directory:
        accepted = fixture(pathlib.Path(directory) / "accepted")
        report = MODULE.validate(
            accepted,
            0,
            f"{MODULE.MARKER}\nprovider tool round trip-verified\n",
            1.25,
        )
        assert report["status"] == "pass"
        assert len(report["turn_usage"]) == 2
        assert report["successful_read_invocations"] == 1
        assert report["cache_activity"] == ["read", "read"]

        missing_usage = fixture(pathlib.Path(directory) / "missing-usage", usage=False)
        expect_failure(missing_usage, "usage facts")

        failed_tool = fixture(pathlib.Path(directory) / "failed-tool", succeeded=False)
        expect_failure(failed_tool, "successful typed write invocation")

    print("provider canary validator passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
