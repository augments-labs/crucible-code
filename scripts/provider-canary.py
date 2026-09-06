#!/usr/bin/env python3
"""Drive and validate one small live provider session."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import selectors
import signal
import subprocess
import sys
import time

TIMEOUT = 120.0
MARKER = "crucible-canary-complete"
FILE_TEXT = "provider tool round trip\n"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--home", required=True)
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--report", required=True)
    return parser.parse_args()


def session_files(home: pathlib.Path) -> list[pathlib.Path]:
    sessions = home / "sessions"
    return sorted(sessions.glob("*.jsonl")) if sessions.is_dir() else []


def run(args: argparse.Namespace) -> tuple[int, str, float]:
    env = os.environ.copy()
    env.update(
        HOME=str(pathlib.Path(args.home).parent),
        CRUCIBLE_CODE_HOME=args.home,
        NO_COLOR="1",
    )
    started = time.monotonic()
    child = subprocess.Popen(
        [args.binary, "--model", args.model],
        cwd=args.workspace,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        start_new_session=True,
    )
    assert child.stdin is not None
    assert child.stdout is not None
    child.stdin.write(
        "Use write to create canary.txt with exactly provider tool round trip followed by a newline. "
        f"After the tool succeeds, reply with only {MARKER}.\n"
    )
    child.stdin.flush()

    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    output: list[str] = []
    deadline = started + TIMEOUT
    sent_second = False
    try:
        while time.monotonic() < deadline:
            if child.poll() is not None:
                output.append(child.stdout.read())
                break
            for key, _ in selector.select(timeout=0.25):
                piece = key.fileobj.readline()
                if piece:
                    output.append(piece)
            joined = "".join(output)
            if not sent_second and MARKER in joined:
                child.stdin.write(
                    "Use read on canary.txt, then reply with only its words, followed by a hyphen and the word verified.\n"
                )
                child.stdin.close()
                sent_second = True
        else:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            raise RuntimeError(f"provider session exceeded {TIMEOUT:.0f} seconds")
        return child.wait(), "".join(output), time.monotonic() - started
    finally:
        selector.close()
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()


def usage_facts(lines: list[dict]) -> list[dict]:
    facts: list[dict] = []
    for line in lines:
        body = line.get("run_item", {}).get("body", {})
        if body.get("kind") == "provider_attempt" and body.get("event") == "usage":
            facts.append(body)
    return facts


def succeeded_tools(lines: list[dict]) -> dict[str, list[dict]]:
    invocations: dict[str, list[dict]] = {"write": [], "read": []}
    for line in lines:
        body = line.get("run_item", {}).get("body", {})
        state = body.get("invocation_state", {})
        tool = body.get("tool")
        arguments = body.get("arguments", {})
        if (
            body.get("kind") == "invocation"
            and tool in invocations
            and arguments.get("path") == "canary.txt"
            and state.get("state") == "finished"
            and state.get("outcome") == "succeeded"
        ):
            invocations[tool].append(body)
    return invocations


def normalized_usage(facts: list[dict]) -> list[dict]:
    normalized = []
    for fact in facts:
        values = fact.get("usage", {})
        input_values = values.get("input", {})
        if not isinstance(input_values.get("total"), (int, type(None))):
            raise RuntimeError("usage.input.total is not normalized numeric data")
        if not isinstance(values.get("output"), (int, type(None))):
            raise RuntimeError("usage.output is not normalized numeric data")
        normalized.append(
            {
                "outcome": fact.get("outcome"),
                "input": input_values,
                "output": values.get("output"),
                "reasoning": values.get("reasoning"),
                "total": values.get("total"),
                "storage_token_hours": values.get("storage_token_hours"),
                "cost": fact.get("cost"),
            }
        )
    return normalized


def validate(args: argparse.Namespace, status: int, output: str, elapsed: float) -> dict:
    home = pathlib.Path(args.home)
    workspace = pathlib.Path(args.workspace)
    logs = session_files(home)
    if status != 0:
        raise RuntimeError(f"{args.model} exited {status}:\n{output}")
    if MARKER not in output or "provider tool round trip-verified" not in output.lower():
        raise RuntimeError(f"{args.model} missed a canary marker:\n{output}")
    if (workspace / "canary.txt").read_text(encoding="utf-8") != FILE_TEXT:
        raise RuntimeError("the live tool call did not create the exact harmless fixture")
    if len(logs) != 1:
        raise RuntimeError(f"expected one session log, found {len(logs)}")

    parsed = [json.loads(line) for line in logs[0].read_text(encoding="utf-8").splitlines()]
    invocations = succeeded_tools(parsed)
    for tool in ("write", "read"):
        if not invocations[tool]:
            raise RuntimeError(f"the session journal has no successful typed {tool} invocation")

    usage = usage_facts(parsed)
    if len(usage) < 2:
        raise RuntimeError(f"expected usage facts for a multi-turn exchange, found {len(usage)}")
    normalized = normalized_usage(usage)

    return {
        "model": args.model,
        "status": "pass",
        "duration_seconds": round(elapsed, 3),
        "turn_usage": normalized,
        "cache_observational": True,
        "cache_activity": [fact["outcome"] for fact in normalized],
        "successful_write_invocations": len(invocations["write"]),
        "successful_read_invocations": len(invocations["read"]),
    }


def main() -> int:
    args = arguments()
    report = pathlib.Path(args.report)
    try:
        status, output, elapsed = run(args)
        result = validate(args, status, output, elapsed)
    except (OSError, ValueError, json.JSONDecodeError, RuntimeError, subprocess.SubprocessError) as problem:
        print(f"provider canary failed: {problem}", file=sys.stderr)
        return 1
    report.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"{args.model} completed a multi-turn typed-tool canary")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
