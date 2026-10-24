#!/usr/bin/env -S python3 -I
"""Tests for the agent catalog's intake, run over committed fixtures.

The registry, the reviewer's choices and the runner matrix are committed under
`tests/fixtures/agent-catalog/intake/`. What the untrusted job would leave for
the intake (each download, the addresses it was redirected through, and the
`unpack-check` and `probe` output) is laid out here in a scratch directory,
from the `unpack-check` and `probe` fixtures the Rust tests hold to the real
output, so no network is reached and `crucible` is never run.
"""

from __future__ import annotations

import copy
import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
INTAKE = ROOT / "scripts/python/agent-catalog-intake.py"
FIXTURES = ROOT / "tests/fixtures/agent-catalog"
REGISTRY = FIXTURES / "intake/registry.json"
CHOICES = FIXTURES / "intake/choices.json"
RUNNERS = FIXTURES / "intake/runners.json"
UNPACKED = json.loads((FIXTURES / "unpack-check/binary.json").read_text())
PROBED = json.loads((FIXTURES / "probe/fake-peer.json").read_text())
UNCONFINED = json.loads((FIXTURES / "probe/unconfined.json").read_text())


def archive(agent: str, platform: str) -> bytes:
    return f"fixture archive of {agent} for {platform}\n".encode()


def many(count: int) -> dict:
    """An unpack-check output listing `count` files, the program among them."""
    files = [{"exec": True, "kind": "file", "path": "agent", "sha256": "a" * 64, "size": 1}]
    files += [
        {"exec": False, "kind": "file", "path": f"share/{index:03}", "sha256": "b" * 64, "size": 2}
        for index in range(count - 2)
    ]
    files.insert(1, {"kind": "dir", "path": "share"})
    unpacked = sum(file.get("size", 0) for file in files)
    return {"decompressed": unpacked * 4, "files": files, "unpacked": unpacked}


def escaping() -> dict:
    out = copy.deepcopy(UNPACKED)
    for file in out["files"]:
        if file["kind"] == "link":
            file["target"] = "../../etc/passwd"
    return out


FAKE = "https://downloads.fake.example/fake-1.0.0"

# (registry id, platform): (download, redirect chain, unpack-check, probe).
OBSERVED = {
    ("fake-agent", "linux-x86_64"): (
        True,
        [f"{FAKE}-linux-x86_64.tar.gz", "https://objects.cdn.example/blob/7?sig=1"],
        UNPACKED,
        PROBED,
    ),
    ("fake-agent", "darwin-aarch64"): (True, [f"{FAKE}-darwin-aarch64.zip"], UNPACKED, PROBED),
    ("fake-agent", "freebsd-x86_64"): (
        True,
        [f"{FAKE}-freebsd-x86_64.tar.gz"],
        UNPACKED,
        UNCONFINED,
    ),
    # Never downloaded, so there is nothing to hash.
    ("fake-agent", "windows-x86_64"): (False, [f"{FAKE}-windows-x86_64.zip"], UNPACKED, PROBED),
    ("fake-agent", "linux-aarch64"): (True, [f"{FAKE}-linux-aarch64.tar.gz"], UNPACKED, PROBED),
    ("fake-agent", "windows-aarch64"): (
        True,
        [f"{FAKE}-windows-aarch64.zip", "http://192.0.2.7/fake.zip"],
        UNPACKED,
        PROBED,
    ),
    ("big-agent", "linux-x86_64"): (
        True,
        ["https://downloads.big.example/big-2.1.0-linux-x86_64.tar.gz"],
        many(300),
        PROBED,
    ),
    ("big-agent", "darwin-aarch64"): (
        True,
        ["https://downloads.big.example/big-2.1.0-darwin-aarch64.tar.gz"],
        UNPACKED,
        PROBED,
    ),
    ("linky-agent", "linux-x86_64"): (
        True,
        ["https://downloads.linky.example/linky-0.3.0-linux-x86_64.tar.gz"],
        escaping(),
        PROBED,
    ),
    ("unprobed-agent", "linux-x86_64"): (
        True,
        ["https://downloads.unprobed.example/unprobed-linux-x86_64.tar.gz"],
        UNPACKED,
        PROBED,
    ),
    ("unprobed-agent", "darwin-aarch64"): (
        True,
        ["https://downloads.unprobed.example/unprobed-darwin-aarch64.tar.gz"],
        UNPACKED,
        None,
    ),
    # Probed without a sandbox on a runner the matrix says is confined.
    ("leaky-agent", "linux-x86_64"): (
        True,
        ["https://downloads.leaky.example/leaky-linux-x86_64.tar.gz"],
        UNPACKED,
        UNCONFINED,
    ),
    ("reachy-agent", "linux-x86_64"): (
        True,
        ["https://downloads.reachy.example/reachy-linux-x86_64.tar.gz"],
        UNPACKED,
        PROBED,
    ),
}


def lay_out(observed: pathlib.Path) -> None:
    for (agent, platform), (downloaded, chain, unpacked, probed) in OBSERVED.items():
        place = observed / agent / platform
        place.mkdir(parents=True)
        if downloaded:
            (place / "artifact").write_bytes(archive(agent, platform))
        (place / "fetched.json").write_text(json.dumps({"chain": chain}))
        (place / "unpack-check.json").write_text(json.dumps(unpacked))
        if probed is not None:
            (place / "probe.json").write_text(json.dumps(probed))


def intake(*args: str | pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-I", str(INTAKE), *map(str, args)],
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )


def review(scratch: pathlib.Path) -> tuple[dict, pathlib.Path]:
    observed = scratch / "observed"
    lay_out(observed)
    assets = scratch / "assets"
    out = scratch / "review.json"
    ran = intake(
        "review",
        "--registry", REGISTRY,
        "--choices", CHOICES,
        "--runners", RUNNERS,
        "--observed", observed,
        "--deny", ROOT / "deny.toml",
        "--assets", assets,
        "--out", out,
    )
    assert ran.returncode == 0, f"review failed:\n{ran.stdout}{ran.stderr}"
    return json.loads(out.read_text()), assets


def dropped(data: dict, agent: str, platform: str | None, *reasons: str) -> None:
    for item in data["dropped"]:
        if item["registry_id"] == agent and item.get("platform") == platform:
            for reason in reasons:
                assert reason in item["reason"], f"{agent} {platform}: {item['reason']!r} lacks {reason!r}"
            return
    raise AssertionError(f"{agent} {platform} was not dropped: {json.dumps(data['dropped'], indent=1)}")


def check_review(data: dict, assets: pathlib.Path) -> None:
    entries = {entry["key"]: entry for entry in data["entries"]}
    assert sorted(entries) == ["big", "fake"], sorted(entries)
    assert data["runners"] == json.loads(RUNNERS.read_text()), "the runner matrix is copied"

    fake = entries["fake"]
    assert fake["registry_id"] == "fake-agent" and fake["version"] == "1.0.0", fake
    assert fake["kind"] == "binary" and fake["license"] == "MIT", fake
    assert fake["admits"] == PROBED["agentInfo"], fake["admits"]
    assert fake["reach"] == ["api.fake.example", "*.fake.example"], fake["reach"]
    targets = {target["platform"]: target for target in fake["targets"]}
    assert sorted(targets) == ["darwin-aarch64", "freebsd-x86_64", "linux-x86_64"], sorted(targets)

    linux = targets["linux-x86_64"]
    assert linux["cmd"] == "agent" and linux["args"] == ["acp", "--no-color"], linux
    assert linux["env"] == {"FAKE_NO_UPDATE": "1"}, linux["env"]
    assert linux["hosts"] == ["downloads.fake.example", "objects.cdn.example"], linux["hosts"]
    [artifact] = linux["artifacts"]
    held = archive("fake-agent", "linux-x86_64")
    assert artifact["sha256"] == hashlib.sha256(held).hexdigest(), artifact
    assert artifact["size"] == len(held) and artifact["format"] == "tar.gz", artifact
    assert artifact["url"] == f"{FAKE}-linux-x86_64.tar.gz", artifact
    assert artifact["files"] == UNPACKED["files"], "the per-file list is recorded as printed"
    assert (artifact["unpacked"], artifact["decompressed"]) == (51, 5120), artifact
    assert artifact["mirror_licence"] == "MIT", "an MIT artifact is marked for mirroring"
    assert linux["probe"]["sandbox"] == "closed" and linux["probe"]["confined"] is True, linux["probe"]
    assert linux["probe"]["conformance"] == "not run", linux["probe"]

    # The registry gave no SHA-256 here: the intake supplies it.
    darwin = targets["darwin-aarch64"]["artifacts"][0]
    assert darwin["sha256"] == hashlib.sha256(archive("fake-agent", "darwin-aarch64")).hexdigest()
    assert darwin["format"] == "zip", darwin

    freebsd = targets["freebsd-x86_64"]["probe"]
    assert freebsd["sandbox"].startswith("unavailable"), freebsd
    assert freebsd["confined"] is False and freebsd["reason"] == "FreeBSD has no sandbox backend"

    methods = {method["id"]: method for method in fake["auth_methods"]}
    assert methods["browser"] == {"id": "browser", "kind": "agent", "name": "Sign in with a browser"}
    terminal = methods["terminal"]
    assert terminal["route"] == "piped" and terminal["tty"] == "unknown", terminal
    assert terminal["command"] == "agent" and terminal["args"] == ["login"], terminal
    assert terminal["env"] == {"LOGIN_MODE": "1"}, terminal

    big = entries["big"]
    [target] = big["targets"]
    [artifact] = target["artifacts"]
    assert "mirror_licence" not in artifact, "a proprietary artifact is not marked for mirroring"
    asset = artifact["files"]
    assert set(asset) == {"sha256", "size"}, "a 300-file list moves to a file-list asset"
    kept = (assets / asset["sha256"]).read_bytes()
    assert hashlib.sha256(kept).hexdigest() == asset["sha256"] and len(kept) == asset["size"]
    assert len(json.loads(kept)) == 300, "the asset holds every file"

    dropped(data, "fake-agent", "windows-x86_64", "could not be hashed")
    dropped(data, "fake-agent", "linux-aarch64", "LD_PRELOAD", "loader")
    dropped(data, "fake-agent", "windows-aarch64", "http://192.0.2.7")
    dropped(data, "big-agent", "darwin-aarch64", "not the bytes the registry pinned")
    dropped(data, "linky-agent", "linux-x86_64", "lib/current", "escapes")
    dropped(data, "linky-agent", None, "no target")
    dropped(data, "unprobed-agent", None, "no probe output", "darwin-aarch64")
    dropped(data, "leaky-agent", None, "linux-x86_64", "without a sandbox", "confined")
    dropped(data, "reachy-agent", None, "10.0.0.1")
    dropped(data, "npx-agent", None, "binary")
    dropped(data, "unkeyed-agent", None, "no key")
    dropped(data, "x`` <b>@here", None, "registry id")


def check_diff(scratch: pathlib.Path, data: dict) -> None:
    new = scratch / "review.json"
    ran = intake("diff", new)
    assert ran.returncode == 0, ran.stderr
    shown = ran.stdout
    unconfined = [line for line in shown.splitlines() if "freebsd-x86_64" in line and "sandbox" in line]
    assert unconfined, "the unconfined target is shown"
    assert any("without a sandbox" in line and "FreeBSD has no sandbox backend" in line for line in unconfined), unconfined
    assert "``` x`` <b>@here ```" in shown, "hostile text is shown inside a code span it cannot close"
    assert "\n<b>" not in shown and " <b>@" not in shown.replace("``` x`` <b>@here ```", ""), shown
    assert "could not be hashed" in shown and "file-list asset" in shown, shown

    old = copy.deepcopy(data)
    for entry in old["entries"]:
        if entry["key"] == "fake":
            entry["version"] = "0.9.0"
            for target in entry["targets"]:
                if target["platform"] == "linux-x86_64":
                    target["args"] = ["acp"]
    previous = scratch / "merged.json"
    previous.write_text(json.dumps(old))
    ran = intake("diff", "--old", previous, new)
    assert ran.returncode == 0, ran.stderr
    changed = ran.stdout
    assert "0.9.0" in changed, changed
    args = [line for line in changed.splitlines() if "args" in line]
    assert any("--no-color" in line and "was" in line for line in args), args
    # What did not change is not repeated: big is the same version as before.
    assert "big" not in changed.split("## Dropped")[0], changed


def check_refuses_hostile_input(scratch: pathlib.Path) -> None:
    registry = scratch / "duplicate.json"
    registry.write_text('{"agents": [], "agents": []}')
    ran = intake(
        "review",
        "--registry", registry,
        "--choices", CHOICES,
        "--runners", RUNNERS,
        "--observed", scratch,
        "--deny", ROOT / "deny.toml",
        "--assets", scratch / "none",
        "--out", scratch / "none.json",
    )
    assert ran.returncode != 0 and "agents" in ran.stderr, ran.stderr
    assert not (scratch / "none.json").exists(), "a refused registry writes no review data"


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="intake-") as held:
        scratch = pathlib.Path(held)
        data, assets = review(scratch)
        check_review(data, assets)
        check_diff(scratch, data)
        check_refuses_hostile_input(scratch)
    print("agent catalog intake fixtures passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
