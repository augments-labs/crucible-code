#!/usr/bin/env python3
"""Run or compare a manual Crucible coding-task campaign."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time


def parser() -> argparse.ArgumentParser:
    top = argparse.ArgumentParser()
    commands = top.add_subparsers(dest="command", required=True)
    run = commands.add_parser("run")
    run.add_argument("--binary", required=True)
    run.add_argument("--model", required=True)
    run.add_argument("--suite", required=True)
    run.add_argument("--output", required=True)
    run.add_argument("--label", required=True)
    compare = commands.add_parser("compare")
    compare.add_argument("--baseline", required=True)
    compare.add_argument("--candidate", required=True)
    compare.add_argument("--output", required=True)
    validate = commands.add_parser("validate")
    validate.add_argument("--suite", required=True)
    return top


def load_suite(path: str) -> list[dict]:
    suite = pathlib.Path(path).resolve()
    document = json.loads(suite.read_text(encoding="utf-8"))
    repository = suite.parents[2]
    if document.get("version") != 1 or not isinstance(document.get("tasks"), list):
        raise ValueError("suite must have version 1 and a tasks array")
    names: set[str] = set()
    for task in document["tasks"]:
        name = task.get("name")
        if not isinstance(name, str) or not name or name in names:
            raise ValueError("every task needs a unique non-empty name")
        names.add(name)
        for field in ("fixture", "prompt", "verify"):
            if not isinstance(task.get(field), str) or not task[field]:
                raise ValueError(f"task {name!r} needs a non-empty {field}")
        fixture = pathlib.Path(task["fixture"])
        if not fixture.is_absolute():
            fixture = repository / fixture
        if not fixture.is_dir():
            raise ValueError(f"task {name!r} fixture does not exist: {fixture}")
        task["fixture"] = str(fixture)
        if pathlib.Path(task["verify"]).is_absolute():
            raise ValueError(f"task {name!r} verify command must be relative to its fixture")
    return document["tasks"]


def metrics(home: pathlib.Path) -> tuple[int | None, int | None, dict[str, int]]:
    input_tokens = 0
    output_tokens = 0
    costs: dict[str, int] = {}
    found = False
    for session in (home / "sessions").glob("*.jsonl"):
        for encoded in session.read_text(encoding="utf-8").splitlines():
            body = json.loads(encoded).get("run_item", {}).get("body", {})
            if body.get("kind") != "provider_attempt" or body.get("event") != "usage":
                continue
            usage = body.get("usage", {})
            input_total = usage.get("input", {}).get("total")
            output = usage.get("output")
            if isinstance(input_total, int):
                input_tokens += input_total
                found = True
            if isinstance(output, int):
                output_tokens += output
                found = True
            total = body.get("cost", {}).get("total")
            if isinstance(total, dict):
                currency = total.get("currency")
                amount = total.get("femtocurrency")
                if isinstance(currency, str) and isinstance(amount, str):
                    costs[currency] = costs.get(currency, 0) + int(amount)
    if not found:
        return None, None, costs
    return input_tokens, output_tokens, costs


def cost_summary(results: list[dict]) -> dict[str, int]:
    currencies = {currency for task in results for currency in task["cost_femtocurrency"]}
    return {
        currency: sum(task["cost_femtocurrency"].get(currency, 0) for task in results)
        for currency in sorted(currencies)
    }


def one(binary: str, model: str, task: dict) -> dict:
    with tempfile.TemporaryDirectory(prefix="crucible-task-") as directory:
        root = pathlib.Path(directory)
        workspace = root / "work"
        home = root / "home"
        subprocess.run(["cp", "-a", f"{task['fixture']}/.", workspace], check=True)
        home.mkdir()
        (home / "config.json").write_text(
            '{"updates":{"check":"never"},"permissions":{"mode":"fullAccess"},'
            '"sandbox":{"enabled":false},"output":{"color":"never"}}\n',
            encoding="utf-8",
        )
        env = os.environ.copy()
        env.update(HOME=str(root), CRUCIBLE_CODE_HOME=str(home), NO_COLOR="1")
        started = time.monotonic()
        completed = subprocess.run(
            [binary, "--model", model],
            cwd=workspace,
            env=env,
            input=task["prompt"] + "\n",
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=task.get("timeout_seconds", 600),
            check=False,
        )
        duration = time.monotonic() - started
        verification = subprocess.run(
            ["bash", "-lc", task["verify"]],
            cwd=workspace,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=task.get("verify_timeout_seconds", 120),
            check=False,
        )
        input_tokens, output_tokens, costs = metrics(home)
        return {
            "name": task["name"],
            "passed": completed.returncode == 0 and verification.returncode == 0,
            "agent_status": completed.returncode,
            "verification_status": verification.returncode,
            "duration_seconds": round(duration, 3),
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cost_femtocurrency": costs,
            "verification_output": verification.stdout[-4000:],
            "agent_output": completed.stdout[-4000:],
        }


def run(args: argparse.Namespace) -> int:
    tasks = load_suite(args.suite)
    binary = str(pathlib.Path(args.binary).resolve())
    results = [one(binary, args.model, task) for task in tasks]
    report = {
        "version": 1,
        "label": args.label,
        "model": args.model,
        "binary": binary,
        "tasks": results,
        "summary": {
            "passed": sum(task["passed"] for task in results),
            "total": len(results),
            "duration_seconds": round(sum(task["duration_seconds"] for task in results), 3),
            "input_tokens": sum(task["input_tokens"] or 0 for task in results),
            "output_tokens": sum(task["output_tokens"] or 0 for task in results),
            "cost_femtocurrency": cost_summary(results),
        },
    }
    pathlib.Path(args.output).write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


def report(path: str) -> dict:
    value = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    if value.get("version") != 1 or not isinstance(value.get("tasks"), list):
        raise ValueError(f"invalid campaign report: {path}")
    return value


def compare(args: argparse.Namespace) -> int:
    baseline = report(args.baseline)
    candidate = report(args.candidate)
    baseline_names = [task.get("name") for task in baseline["tasks"]]
    candidate_names = [task.get("name") for task in candidate["tasks"]]
    if baseline_names != candidate_names:
        raise ValueError("baseline and candidate did not run the same ordered task set")
    left = baseline["summary"]
    right = candidate["summary"]
    currencies = set(left.get("cost_femtocurrency", {})) | set(
        right.get("cost_femtocurrency", {})
    )
    result = {
        "version": 1,
        "baseline": baseline.get("label"),
        "candidate": candidate.get("label"),
        "tasks": candidate_names,
        "delta": {
            "passed": right["passed"] - left["passed"],
            "duration_seconds": round(right["duration_seconds"] - left["duration_seconds"], 3),
            "input_tokens": right["input_tokens"] - left["input_tokens"],
            "output_tokens": right["output_tokens"] - left["output_tokens"],
            "cost_femtocurrency": {
                currency: right.get("cost_femtocurrency", {}).get(currency, 0)
                - left.get("cost_femtocurrency", {}).get(currency, 0)
                for currency in sorted(currencies)
            },
        },
        "summaries": {"baseline": left, "candidate": right},
    }
    pathlib.Path(args.output).write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "run":
            return run(args)
        if args.command == "compare":
            return compare(args)
        load_suite(args.suite)
        print(f"valid task campaign suite: {args.suite}")
        return 0
    except (OSError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as problem:
        print(f"task campaign failed: {problem}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
