#!/usr/bin/env -S python3 -I
"""Tests for the agent catalog's intake, run over committed fixtures.

The registry, the reviewer's choices and the runner matrix are committed under
`tests/fixtures/agent-catalog/intake/`. What the untrusted job would leave for
the intake (each download, the addresses it was redirected through, and the
`unpack-check` and `probe` output) is laid out here in a scratch directory,
from the `unpack-check` and `probe` fixtures the Rust tests hold to the real
output, so no network is reached and `crucible` is never run.

A `uvx` agent's interpreter archive and wheels are built here, in the scratch
directory, with `tarfile` and `zipfile`, so no binary is committed; the
`unpack-check` listing written beside each is in the shape
`unpack-check/binary.json` pins to the real output, with each wheel's paths
under `site/` and the interpreter's under `python/`.
"""

from __future__ import annotations

import argparse
import base64
import bz2
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import struct
import subprocess
import sys
import tarfile
import tempfile
import tracemalloc
import warnings
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
INTAKE = ROOT / "scripts/python/agent-catalog-intake.py"
FIXTURES = ROOT / "tests/fixtures/agent-catalog"
REGISTRY = FIXTURES / "intake/registry.json"
CHOICES = FIXTURES / "intake/choices.json"
RUNNERS = FIXTURES / "intake/runners.json"
INTERPRETERS = FIXTURES / "intake/interpreters.json"
UNPACKED = json.loads((FIXTURES / "unpack-check/binary.json").read_text())
PROBED = json.loads((FIXTURES / "probe/fake-peer.json").read_text())
UNCONFINED = json.loads((FIXTURES / "probe/unconfined.json").read_text())
RENDER = FIXTURES / "render"
SIGNED_AT = 1_790_000_000


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


def probed(**changed: object) -> dict:
    """The probe fixture, with its terminal method changed as `changed` says,
    or the whole output where a key is a top-level one."""
    out = copy.deepcopy(PROBED)
    for name, value in changed.items():
        if name in out:
            out[name] = value
        else:
            out["authMethods"][1][name] = value
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
    ("split-agent", "linux-x86_64"): (
        True,
        ["https://downloads.split.example/split-agent-linux-x86_64.tar.gz"],
        UNPACKED,
        PROBED,
    ),
    # The same sign-in method, spelled for another platform.
    ("split-agent", "darwin-aarch64"): (
        True,
        ["https://downloads.split.example/split-agent-darwin-aarch64.tar.gz"],
        UNPACKED,
        probed(command="agent.exe"),
    ),
    # A number where a flag belongs: equal to true, and not true.
    ("tty-agent", "linux-x86_64"): (
        True,
        ["https://downloads.tty.example/tty-agent-linux-x86_64.tar.gz"],
        UNPACKED,
        probed(tty=1),
    ),
    ("failed-agent", "linux-x86_64"): (
        True,
        ["https://downloads.failed.example/failed-agent-linux-x86_64.tar.gz"],
        UNPACKED,
        probed(conformance="FAILED 14 of 20"),
    ),
    # Laid out complete, but its directory is a link to one elsewhere.
    ("linked-agent", "linux-x86_64"): (
        True,
        ["https://downloads.linked.example/linked-agent-linux-x86_64.tar.gz"],
        UNPACKED,
        PROBED,
    ),
}

LINKED = "linked-agent"


def lay_out(observed: pathlib.Path) -> None:
    for (agent, platform), (downloaded, chain, unpacked, probed) in OBSERVED.items():
        if agent == LINKED:
            elsewhere = observed.parent / "elsewhere" / agent
            if not (observed / agent).is_symlink():
                elsewhere.mkdir(parents=True, exist_ok=True)
                (observed / agent).parent.mkdir(parents=True, exist_ok=True)
                (observed / agent).symlink_to(elsewhere, target_is_directory=True)
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


def review(
    scratch: pathlib.Path,
    *extra: str | pathlib.Path,
    name: str = "review.json",
    registry: pathlib.Path = REGISTRY,
    choices: pathlib.Path = CHOICES,
) -> tuple[dict, pathlib.Path]:
    observed = scratch / "observed"
    if not observed.exists():
        lay_out(observed)
    assets = scratch / "assets"
    out = scratch / name
    ran = intake(
        "review",
        "--registry", registry,
        "--choices", choices,
        "--runners", RUNNERS,
        "--observed", observed,
        "--deny", ROOT / "deny.toml",
        "--assets", assets,
        "--out", out,
        *extra,
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
    # An agent distributed in none of the kinds the intake takes in.
    dropped(data, "docker-agent", None, "binary", "uvx", "npx")
    # With no Node pinned, an npm agent has no target.
    dropped(data, "npx-agent", "linux-x86_64", "no Node is pinned")
    dropped(data, "npx-agent", None, "no target")
    dropped(data, "unkeyed-agent", None, "no key")
    dropped(data, "x`` <b>@here", None, "registry id")
    dropped(data, "split-agent", None, "auth_methods[1].command", "darwin-aarch64")
    dropped(data, "tty-agent", None, "tty")
    dropped(data, "failed-agent", None, "conformance", "not run")
    dropped(data, "linked-agent", "linux-x86_64", "not a directory")


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
    assert '"loadSession":true' in shown, "the probe's capabilities are shown"

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


def fake_target(data: dict, platform: str = "linux-x86_64") -> dict:
    [entry] = [entry for entry in data["entries"] if entry["key"] == "fake"]
    [target] = [target for target in entry["targets"] if target["platform"] == platform]
    return target


def written(scratch: pathlib.Path, name: str, data: dict) -> pathlib.Path:
    path = scratch / name
    path.write_text(json.dumps(data))
    return path


def check_same_version(scratch: pathlib.Path, data: dict) -> None:
    """A version already merged whose content changed is refused by `review`
    and shown as a change by `diff`, never passed over as already seen."""
    merged = copy.deepcopy(data)
    target = fake_target(merged)
    target["args"] = ["acp"]
    target["artifacts"][0]["sha256"] = "d" * 64
    held = written(scratch, "merged-same.json", merged)
    again, _ = review(scratch, "--merged", held, name="again.json")
    assert "fake" not in {entry["key"] for entry in again["entries"]}, "a changed same version is refused"
    dropped(again, "fake-agent", None, "1.0.0", "already reviewed", "args", "sha256")
    ran = intake("diff", "--old", held, scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    lines = ran.stdout.splitlines()
    assert any("same version" in line and "fake" in line for line in lines), ran.stdout
    assert any("args" in line and "--no-color" in line and "was" in line for line in lines), ran.stdout
    assert any("sha256" in line and "d" * 64 in line for line in lines), ran.stdout


def check_hostile_entries(scratch: pathlib.Path) -> None:
    """Each entry that cannot be read is refused alone; the run goes on."""
    registry = json.loads(REGISTRY.read_text())
    choices = json.loads(CHOICES.read_text())

    def agent(agent_id: str, key: str, **target: object) -> None:
        spec = {"archive": f"https://downloads.{key}.example/{key}.tar.gz", "cmd": "./agent"}
        spec.update(target)
        registry["agents"].append(
            {"id": agent_id, "version": target.pop("version", "1.0.0"), "license": "MIT",
             "distribution": {"binary": {"linux-x86_64": spec}}}
        )
        choices[agent_id] = {"key": key}

    agent("v6-agent", "v6", archive="https://[abc/a.tar.gz")
    agent("nfkc-agent", "nfkc", archive="https://exa\u2100.com/a.tar.gz")
    agent("surrogate-arg", "surrogate-arg", args=["\ud800"])
    agent("hop-agent", "hop")
    registry["agents"][-1]["distribution"]["binary"]["linux-x86_64"]["archive"] = (
        "https://downloads.hop.example/hop.tar.gz"
    )
    registry["agents"].append(
        {"id": "surrogate-version", "version": "\ud800", "license": "MIT",
         "distribution": {"binary": {"linux-x86_64": {"archive": "https://a.example/a.tar.gz", "cmd": "./agent"}}}}
    )
    choices["surrogate-version"] = {"key": "surrogate-version"}
    registry["agents"].append({"id": "\ud800-agent", "version": "1.0.0"})
    place = scratch / "observed" / "hop-agent" / "linux-x86_64"
    place.mkdir(parents=True, exist_ok=True)
    (place / "artifact").write_bytes(b"hop\n")
    (place / "fetched.json").write_text(
        json.dumps({"chain": ["https://downloads.hop.example/hop.tar.gz", "https://[abc/x"]})
    )
    hostile, _ = review(
        scratch,
        name="hostile.json",
        registry=written(scratch, "hostile-registry.json", registry),
        choices=written(scratch, "hostile-choices.json", choices),
    )
    assert {"big", "fake"} <= {entry["key"] for entry in hostile["entries"]}, "the others are still reviewed"
    dropped(hostile, "v6-agent", "linux-x86_64", "https address")
    dropped(hostile, "nfkc-agent", "linux-x86_64", "https address")
    dropped(hostile, "surrogate-arg", "linux-x86_64", "args")
    dropped(hostile, "hop-agent", "linux-x86_64", "redirect")
    dropped(hostile, "surrogate-version", None, "version")
    ran = intake("diff", scratch / "hostile.json")
    assert ran.returncode == 0, ran.stderr
    assert "\\ud800" in ran.stdout, "a lone surrogate is shown escaped"


def check_full_values(scratch: pathlib.Path, data: dict) -> None:
    """A change past any length is shown, and a host is shown whole."""
    long_host = "github.com." + ".".join(["a" * 50] * 4) + ".attacker.example"
    new = copy.deepcopy(data)
    target = fake_target(new)
    target["args"] = ["acp", "A" * 210 + "--evil"]
    target["hosts"] = [long_host]
    old = copy.deepcopy(data)
    for entry in old["entries"]:
        if entry["key"] == "fake":
            entry["version"] = "0.9.0"
    fake_target(old)["args"] = ["acp", "A" * 210 + "safe"]
    ran = intake("diff", "--old", written(scratch, "old-long.json", old), written(scratch, "new-long.json", new))
    assert ran.returncode == 0, ran.stderr
    args = [line for line in ran.stdout.splitlines() if line.startswith("- args")]
    assert any("--evil" in line and "was" in line and "safe" in line for line in args), args
    assert long_host in ran.stdout, "a host is never cut"


def check_escapes(scratch: pathlib.Path, data: dict) -> None:
    """Every invisible, format and line-breaking character is shown escaped."""
    hidden = ["\u200e", "\u200f", "\u061c", "\u200b", "\u2060", "\ufeff", "\u00ad",
              "\U000e0041", "\u0085", "\u00a0", "\u2028", "\u2029", "\u202e", "\ufe0f"]
    new = copy.deepcopy(data)
    fake_target(new)["args"] = [f"x{character}y" for character in hidden]
    ran = intake("diff", written(scratch, "hidden.json", new))
    assert ran.returncode == 0, ran.stderr
    for character in hidden:
        assert character not in ran.stdout, f"U+{ord(character):04X} is shown raw"
    assert "x\\u200ey" in ran.stdout and "x\\U000e0041y" in ran.stdout, "each is shown by its code point"


def render(
    scratch: pathlib.Path,
    name: str,
    *,
    merged: pathlib.Path = RENDER / "merged.json",
    previous: pathlib.Path | None = RENDER / "previous.json",
    signed_at: int = SIGNED_AT,
) -> tuple[subprocess.CompletedProcess[str], pathlib.Path]:
    out = scratch / name
    extra = ["--previous", previous] if previous is not None else []
    ran = intake("render", "--merged", merged, *extra, "--signed-at", str(signed_at), "--out", out)
    return ran, out


def rendered(scratch: pathlib.Path, name: str, **kept: object) -> tuple[bytes, dict]:
    ran, out = render(scratch, name, **kept)
    assert ran.returncode == 0, f"render failed:\n{ran.stdout}{ran.stderr}"
    held = out.read_bytes()
    return held, json.loads(held)


def check_render(scratch: pathlib.Path) -> None:
    """The catalog is rendered from merged data with a higher serial, thirty
    days of life, every revocation ever made, and the same bytes each time."""
    first, catalog = rendered(scratch, "catalog-1.json")
    again, _ = rendered(scratch, "catalog-2.json")
    assert first == again, "rendering the same data twice gave different bytes"
    expected = json.dumps(catalog, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    assert first == expected, "the bytes are not sorted, compact UTF-8"
    assert (RENDER / "catalog.json").read_bytes() == first, "the committed rendered catalog is not what render makes"

    merged = json.loads((RENDER / "merged.json").read_text())
    assert sorted(catalog) == ["entries", "expires", "revoked", "schema", "serial", "signed_at"], sorted(catalog)
    assert catalog["schema"] == 1 and catalog["serial"] == 8, "the serial is the previous one raised by one"
    assert catalog["signed_at"] == SIGNED_AT and catalog["expires"] == SIGNED_AT + 30 * 24 * 60 * 60, catalog
    assert [entry["key"] for entry in catalog["entries"]] == [entry["key"] for entry in merged["entries"]]
    assert [(r["key"], r["version"]) for r in catalog["revoked"]] == [
        ("antigravity", "1.2.0"),
        ("fake", "0.9.0"),
    ], "revocations from the previous catalog and the merged data are kept once each"
    for entry in catalog["entries"]:
        for target in entry["targets"]:
            assert "probe" not in target, "the reviewer's probe record is not part of the catalog"
            for artifact in target["artifacts"]:
                assert "mirror_licence" not in artifact, artifact
    big = next(entry for entry in catalog["entries"] if entry["key"] == "big")
    listing = big["targets"][0]["artifacts"][0]["files"]
    assert sorted(listing) == ["sha256", "size"], "a long file list is carried as a hash and a size"

    ran, out = render(scratch, "first.json", previous=None)
    assert ran.returncode == 0, ran.stderr
    assert json.loads(out.read_text())["serial"] == 1, "with no catalog before it the serial is 1"
    _, next_one = rendered(scratch, "catalog-3.json", previous=scratch / "catalog-1.json", signed_at=SIGNED_AT + 1)
    assert next_one["serial"] == catalog["serial"] + 1, "the serial strictly increases"
    bare = scratch / "bare.json"
    bare.write_text(json.dumps({"entries": merged["entries"], "format": 1}))
    _, kept = rendered(scratch, "catalog-4.json", merged=bare, previous=scratch / "catalog-1.json")
    assert len(kept["revoked"]) == 2, "a revocation outlives the data that made it"


def check_render_refuses(scratch: pathlib.Path) -> None:
    """A catalog a client would refuse is refused before it is written."""
    merged = json.loads((RENDER / "merged.json").read_text())
    many_revoked = scratch / "many-revoked.json"
    merged["revoked"] = [
        {"key": "fake", "reason": "withdrawn", "version": f"1.{index}.0"} for index in range(4097)
    ]
    many_revoked.write_text(json.dumps(merged))
    ran, out = render(scratch, "too-many.json", merged=many_revoked)
    assert ran.returncode == 1 and "4096" in ran.stderr and not out.exists(), (ran.returncode, ran.stderr)
    merged["revoked"] = merged["revoked"][:4096]
    full = scratch / "full-revoked.json"
    full.write_text(json.dumps(merged))
    ran, _ = render(scratch, "exactly-4096.json", merged=full, previous=None)
    assert ran.returncode == 0, ran.stderr

    merged["revoked"] = []
    merged["entries"][0]["caution"] = "x" * (1 << 20)
    large = scratch / "large.json"
    large.write_text(json.dumps(merged))
    ran, out = render(scratch, "too-large.json", merged=large)
    assert ran.returncode == 1 and "1048576" in ran.stderr and not out.exists(), (ran.returncode, ran.stderr)

    merged["entries"][0]["caution"] = "fine"
    floaty = scratch / "floaty.json"
    floaty.write_text(json.dumps({**merged, "entries": [{**merged["entries"][0], "extra": 1.5}]}))
    ran, out = render(scratch, "floaty-out.json", merged=floaty)
    assert ran.returncode == 1 and "whole" in ran.stderr and not out.exists(), (ran.returncode, ran.stderr)
    odd = scratch / "odd.json"
    odd.write_text(json.dumps({**merged, "revoked": [{"key": "fake", "reason": "r", "version": "1/2"}]}))
    ran, out = render(scratch, "odd-out.json", merged=odd)
    assert ran.returncode == 1 and "Traceback" not in ran.stderr and not out.exists(), (ran.returncode, ran.stderr)

    wrong = scratch / "wrong.json"
    wrong.write_text(json.dumps({"entries": "none", "format": 1}))
    ran, out = render(scratch, "wrong-out.json", merged=wrong)
    assert ran.returncode == 1 and not out.exists(), (ran.returncode, ran.stderr)
    held = scratch / "held-back.json"
    held.write_text(json.dumps({"serial": 2**64, "revoked": []}))
    ran, out = render(scratch, "held-out.json", previous=held)
    assert ran.returncode == 1 and not out.exists(), (ran.returncode, ran.stderr)


def refused(scratch: pathlib.Path, name: str, words: str, **kept: object) -> None:
    """A render that fails with a message naming `words`, writes nothing and
    shows no traceback."""
    ran, out = render(scratch, name, **kept)
    assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr!r} lacks {words!r}"
    assert "Traceback" not in ran.stderr and not out.exists(), f"{name}: {ran.stderr!r}"


def check_render_previous_and_entries(scratch: pathlib.Path) -> None:
    """A previous catalog that is not one this renderer could have written, and
    entries every client would refuse, are refused before anything is written."""
    def made(name: str, data: object) -> pathlib.Path:
        path = scratch / name
        path.write_text(json.dumps(data))
        return path

    previous = json.loads((RENDER / "previous.json").read_text())
    without = {key: value for key, value in previous.items() if key != "revoked"}
    refused(scratch, "no-revoked.json", "revocations", previous=made("p-no-revoked.json", without))
    refused(scratch, "bad-revoked.json", "revocations", previous=made("p-bad-revoked.json", {**previous, "revoked": {}}))
    refused(scratch, "schema.json", "schema", previous=made("p-schema.json", {**previous, "schema": 2}))
    refused(scratch, "no-schema.json", "schema", previous=made("p-no-schema.json", {k: v for k, v in previous.items() if k != "schema"}))
    refused(scratch, "floor.json", "serial", previous=made("p-floor.json", {**previous, "serial": 0}))
    refused(scratch, "earlier.json", "earlier", previous=made("p-earlier.json", {**previous, "signed_at": SIGNED_AT + 1}))

    merged = json.loads((RENDER / "merged.json").read_text())

    def with_entry(name: str, change) -> pathlib.Path:
        data = copy.deepcopy(merged)
        change(data["entries"][0])
        return made(name, data)

    def drop(field: str):
        return lambda entry: entry.pop(field)

    for field in ("review", "sign_in_hosts", "state_paths", "reach", "auth_methods", "registry_id", "license", "kind", "admits"):
        refused(scratch, f"no-{field}.json", field, merged=with_entry(f"m-no-{field}.json", drop(field)))
    refused(scratch, "key.json", "key", merged=with_entry("m-key.json", lambda e: e.update(key="Bad_Key")))
    refused(scratch, "no-targets.json", "targets", merged=with_entry("m-no-targets.json", lambda e: e.update(targets=[])))
    refused(
        scratch, "no-platform.json", "platform",
        merged=with_entry("m-no-platform.json", lambda e: e["targets"][0].pop("platform")),
    )
    refused(
        scratch, "no-artifacts.json", "artifacts",
        merged=with_entry("m-no-artifacts.json", lambda e: e["targets"][0].update(artifacts=[])),
    )
    refused(
        scratch, "huge.json", "whole",
        merged=with_entry("m-huge.json", lambda e: e["targets"][0]["artifacts"][0].update(size=2**64)),
    )
    refused(
        scratch, "negative.json", "whole",
        merged=with_entry("m-negative.json", lambda e: e["targets"][0]["artifacts"][0].update(size=-1)),
    )
    directory = scratch / "a-directory"
    directory.mkdir()
    ran = intake("render", "--merged", RENDER / "merged.json", "--signed-at", str(SIGNED_AT), "--out", directory)
    assert ran.returncode == 1 and "Traceback" not in ran.stderr and "agent-catalog-intake" in ran.stderr, ran.stderr
    ran = intake(
        "render", "--merged", RENDER / "merged.json", "--signed-at", str(SIGNED_AT),
        "--out", scratch / "no-such-directory" / "out.json",
    )
    assert ran.returncode == 1 and "Traceback" not in ran.stderr and "agent-catalog-intake" in ran.stderr, ran.stderr


def load_intake():
    spec = importlib.util.spec_from_file_location("agent_catalog_intake", INTAKE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_budget(scratch: pathlib.Path) -> None:
    """One run reads within a named budget, and writes no more review data
    than `diff` reads back."""
    module = load_intake()
    namespace = argparse.Namespace(
        registry=REGISTRY, choices=CHOICES, runners=RUNNERS, observed=scratch / "observed",
        deny=ROOT / "deny.toml", assets=scratch / "budget-assets", out=scratch / "budget.json", merged=None,
        interpreters=None, node=None, node_keys=None, dispositions=None, sboms=None, texts=None,
    )
    module.RUN_BYTES = 3000
    assert module.review(namespace) == 0
    spent = json.loads((scratch / "budget.json").read_text())
    assert any("budget" in item["reason"] for item in spent["dropped"]), spent["dropped"]
    module.RUN_BYTES = 1 << 40
    module.DATA_BYTES = 1000
    namespace.out = scratch / "too-much.json"
    try:
        module.review(namespace)
    except module.Refused as refused:
        assert "larger than" in str(refused), refused
    else:
        raise AssertionError("review data larger than diff reads was written")
    assert not namespace.out.exists()


def gate_record(asset: bytes, notice: bytes = b"a NOTICE", licence: bytes = b"a licence text") -> dict:
    """A passing licence-gate record, in the shape the gate writes, for `asset`."""
    return {
        "asset": {"sha256": hashlib.sha256(asset).hexdigest(), "size": len(asset)},
        "components": 1,
        "licences": [{"id": "MIT", "sha256": hashlib.sha256(licence).hexdigest(), "size": len(licence)}],
        "notice": {"sha256": hashlib.sha256(notice).hexdigest(), "size": len(notice)},
        "passed": True,
    }


def publish_checked(scratch: pathlib.Path, name: str, data: dict, *assets: tuple[str, bytes]) -> subprocess.CompletedProcess[str]:
    held = scratch / f"publish-{name}"
    (held / "assets").mkdir(parents=True)
    for asset, content in assets:
        (held / "assets" / asset).write_bytes(content)
    (held / "merged.json").write_text(json.dumps(data))
    return intake("publish-check", "--merged", held / "merged.json", "--assets", held / "assets")


def publish_refuses(ran: subprocess.CompletedProcess[str], words: str) -> None:
    assert ran.returncode == 1 and words in ran.stderr, f"{ran.returncode} {ran.stderr!r} lacks {words!r}"
    assert "Traceback" not in ran.stderr, ran.stderr


def check_publish(scratch: pathlib.Path) -> None:
    """The publishing job uploads only assets whose hash the merged data holds,
    and an artifact copy only where its entry carries a passing licence-gate
    record for those very bytes; an asset that is in the data passes."""
    merged = json.loads((RENDER / "merged.json").read_text())
    big = next(entry for entry in merged["entries"] if entry["key"] == "big")
    artifact = big["targets"][0]["artifacts"][0]
    copied = b"the bytes of a mirror copy"
    copy_hash = hashlib.sha256(copied).hexdigest()
    artifact["sha256"] = copy_hash
    artifact["mirror_licence"] = "MIT"

    def checked(name: str, data: dict, *assets: tuple[str, bytes]) -> subprocess.CompletedProcess[str]:
        return publish_checked(scratch, name, data, *assets)

    refuses = publish_refuses

    def at(data: dict) -> dict:
        return data["entries"][merged["entries"].index(big)]["targets"][0]["artifacts"][0]

    # A file-list asset the data names by hash, with the bytes that hash.
    bytes_of_listing = b"[]"
    artifact["files"] = {"sha256": hashlib.sha256(bytes_of_listing).hexdigest(), "size": len(bytes_of_listing)}
    listed = artifact["files"]["sha256"]
    ran = checked("listing", merged, (listed, bytes_of_listing))
    assert ran.returncode == 0, f"a listed asset was refused:\n{ran.stdout}{ran.stderr}"
    ran = checked("none", merged)
    assert ran.returncode == 0, f"nothing to upload was refused:\n{ran.stdout}{ran.stderr}"

    # A hash the data does not hold, however well the file is named.
    stranger = b"not in the data"
    refuses(checked("stranger", merged, (hashlib.sha256(stranger).hexdigest(), stranger)), "not in the merged data")
    # Bytes that are not the hash their name claims.
    refuses(checked("forged", merged, (listed, b"other bytes")), "not the bytes")
    # A name that is not a hash.
    refuses(checked("named", merged, ("index.json", bytes_of_listing)), "not named by")

    # An artifact copy: refused with no licence-gate record, or one that did
    # not pass, or one that passed for other bytes or in another shape.
    refuses(checked("ungated", merged, (copy_hash, copied)), "licence-gate")
    elsewhere = gate_record(b"some other bytes")
    unbound = dict(gate_record(copied), asset={"sha256": copy_hash})
    for name, record in (
        ("failed", {"passed": False, "reason": "it did not pass"}),
        ("loose", dict(gate_record(copied), passed="yes")),
        ("empty", {}),
        ("bare", {"passed": True}),
        ("elsewhere", elsewhere),
        ("unbound", unbound),
    ):
        gated = copy.deepcopy(merged)
        at(gated)["licence_gate"] = record
        refuses(checked(name, gated, (copy_hash, copied)), "licence-gate")
    gated = copy.deepcopy(merged)
    at(gated)["licence_gate"] = gate_record(copied)
    # The NOTICE and licence text a passing record gives are published by
    # their hash and size, beside the copy, and the copy never ships without
    # them.
    notice, licence = b"a NOTICE", b"a licence text"
    shipped = ((hashlib.sha256(notice).hexdigest(), notice), (hashlib.sha256(licence).hexdigest(), licence))
    ran = checked("gated", gated, (copy_hash, copied), *shipped)
    assert ran.returncode == 0, f"a gated copy was refused:\n{ran.stdout}{ran.stderr}"
    refuses(checked("copy-only", gated, (copy_hash, copied)), "NOTICE")
    refuses(checked("copy-noticed", gated, (copy_hash, copied), shipped[0]), "licence text")
    refuses(checked("copy-licensed", gated, (copy_hash, copied), shipped[1]), "NOTICE")
    # A NOTICE and its texts may ship alone: they redistribute nothing.
    ran = checked("gated-notice", gated, *shipped)
    assert ran.returncode == 0, f"a passing record's NOTICE was refused:\n{ran.stdout}{ran.stderr}"
    # The record vouches for the bytes the data pins, so they must hash to it.
    refuses(checked("gated-forged", gated, (copy_hash, b"other bytes"), *shipped), "not the bytes")
    misstated = copy.deepcopy(gated)
    at(misstated)["licence_gate"]["notice"]["size"] = len(notice) + 1
    refuses(checked("notice-size", misstated, shipped[0]), "size")
    # A failing record publishes no NOTICE.
    failing = copy.deepcopy(merged)
    at(failing)["licence_gate"] = dict(gate_record(copied), passed=False)
    refuses(checked("notice-failed", failing, shipped[0]), "not in the merged data")
    # Two entries holding the same bytes: a copy passes only where both pass.
    doubled = copy.deepcopy(gated)
    twin = copy.deepcopy(doubled["entries"][merged["entries"].index(big)])
    twin["key"] = "twin"
    twin["targets"][0]["artifacts"][0]["licence_gate"] = {"passed": False, "reason": "no lockfile or SBOM was given"}
    doubled["entries"].append(twin)
    refuses(checked("twin", doubled, (copy_hash, copied)), "licence-gate")
    # An artifact copy needs the mirror mark as well as the record.
    unmarked = copy.deepcopy(gated)
    del at(unmarked)["mirror_licence"]
    refuses(checked("unmarked", unmarked, (copy_hash, copied)), "mirror")
    # One hash that is both a file list and an artifact is held to both: the
    # file-list rule must not let it past the gate.
    dual = copy.deepcopy(merged)
    held = at(dual)
    held["files"] = {"sha256": copy_hash, "size": len(copied)}
    refuses(checked("dual", dual, (copy_hash, copied)), "licence-gate")
    held["licence_gate"] = gate_record(copied)
    ran = checked("dual-gated", dual, (copy_hash, copied), *shipped)
    assert ran.returncode == 0, f"a gated dual hash was refused:\n{ran.stdout}{ran.stderr}"
    # An asset that is a link is refused, not followed.
    linked = scratch / "publish-linked"
    (linked / "assets").mkdir(parents=True)
    (linked / "elsewhere").write_bytes(bytes_of_listing)
    (linked / "assets" / listed).symlink_to(linked / "elsewhere")
    (linked / "merged.json").write_text(json.dumps(merged))
    refuses(intake("publish-check", "--merged", linked / "merged.json", "--assets", linked / "assets"), "regular file")
    # Merged data that is not review data is refused, not read as holding nothing.
    refuses(checked("shapeless", {"entries": "none"}), "not review data")


def check_publish_replaced(scratch: pathlib.Path) -> None:
    """A gated output installed in an artifact's place is published by the
    record that names it, and the artifact it replaces is then not copied."""
    module = load_intake()
    merged = json.loads((RENDER / "merged.json").read_text())
    index = next(index for index, entry in enumerate(merged["entries"]) if entry["key"] == "fake")
    artifact = merged["entries"][index]["targets"][0]["artifacts"][0]
    downloaded = b"the bytes the vendor published"
    original = hashlib.sha256(downloaded).hexdigest()
    artifact["sha256"] = original
    assert artifact.get("mirror_licence") == "MIT", "the fixture's artifact is marked for mirroring"
    built = b"the bytes of an output built at review"
    built_hash = hashlib.sha256(built).hexdigest()
    module.replaced(artifact, gate_record(built))
    assert artifact["replaced_by"] == {"sha256": built_hash, "size": len(built)}, artifact.get("replaced_by")
    assert artifact["licence_gate"] == gate_record(built), "the record is kept beside what it replaces"
    beside = [(hashlib.sha256(held).hexdigest(), held) for held in (b"a NOTICE", b"a licence text")]
    ran = publish_checked(scratch, "replaced", merged, (built_hash, built), *beside)
    assert ran.returncode == 0, f"a gated replacement was refused:\n{ran.stdout}{ran.stderr}"
    publish_refuses(publish_checked(scratch, "replaced-alone", merged, (built_hash, built)), "NOTICE")
    # The record names the replacement, so it vouches for no copy of the original.
    publish_refuses(publish_checked(scratch, "replaced-original", merged, (original, downloaded)), "licence-gate")
    # A replacement whose record names other bytes is refused.
    other = copy.deepcopy(merged)
    other["entries"][index]["targets"][0]["artifacts"][0]["licence_gate"] = gate_record(b"other bytes")
    publish_refuses(publish_checked(scratch, "replaced-elsewhere", other, (built_hash, built), *beside), "licence-gate")
    # A failing record is recorded, and names no replacement.
    failing = copy.deepcopy(merged)
    held = failing["entries"][index]["targets"][0]["artifacts"][0]
    module.replaced(held, {"passed": False, "reason": "off the list"})
    assert held["licence_gate"] == {"passed": False, "reason": "off the list"}, "a failing record is still recorded"
    assert "replaced_by" not in held, "a failing record names no replacement"
    publish_refuses(publish_checked(scratch, "replaced-failed", failing, (built_hash, built)), "not in the merged data")
    # The catalog a client reads keeps the replacement and leaves the record out.
    held_merged = written(scratch, "merged-replaced.json", merged)
    ran, out = render(scratch, "catalog-replaced.json", merged=held_merged)
    assert ran.returncode == 0, f"render failed:\n{ran.stdout}{ran.stderr}"
    listed = json.loads(out.read_text())["entries"]
    shown_artifact = next(entry for entry in listed if entry["key"] == "fake")["targets"][0]["artifacts"][0]
    assert shown_artifact["replaced_by"] == {"sha256": built_hash, "size": len(built)}, "render keeps replaced_by"
    assert "licence_gate" not in shown_artifact, "render leaves the gate's record out"


GATE = FIXTURES / "licence-gate"
GATE_ASSET = b"the bytes the gate publishes"


def gate(
    scratch: pathlib.Path,
    name: str,
    source: pathlib.Path,
    *,
    texts: pathlib.Path = GATE / "texts",
) -> tuple[subprocess.CompletedProcess[str], pathlib.Path, dict | None]:
    asset = scratch / "gate-asset"
    if not asset.exists():
        asset.write_bytes(GATE_ASSET)
    out = scratch / f"gate-{name}"
    ran = intake(
        "licence-gate", "--input", source, "--deny", ROOT / "deny.toml", "--texts", texts, "--asset", asset,
        "--out", out,
    )
    try:
        record = json.loads(ran.stdout)
    except ValueError:
        record = None
    return ran, out, record


def gate_refuses(scratch: pathlib.Path, name: str, source: pathlib.Path, *words: str, **kept: object) -> dict:
    ran, out, record = gate(scratch, name, source, **kept)
    assert ran.returncode == 1 and "Traceback" not in ran.stderr, f"{name}: {ran.returncode} {ran.stderr!r}"
    assert isinstance(record, dict) and record.get("passed") is False, f"{name}: no failing record printed"
    assert sorted(record) == ["passed", "reason"], f"{name}: {sorted(record)}"
    for word in words:
        assert word in record["reason"] and word in ran.stderr, f"{name}: {record['reason']!r} lacks {word!r}"
    assert not out.exists(), f"{name}: a refused input wrote its NOTICE"
    return record


def sbom_of(scratch: pathlib.Path, name: str, *components: dict) -> pathlib.Path:
    return written(scratch, f"sbom-{name}.json", {"bomFormat": "CycloneDX", "specVersion": "1.6", "components": list(components)})


def check_licence_gate_refuses(scratch: pathlib.Path) -> None:
    """One component off deny.toml's list refuses the whole input, and so
    does every licence the gate cannot hold to that list."""
    lock = json.loads((GATE / "lock.json").read_text())
    lock["packages"][2]["license"] = "Unlicense"
    gate_refuses(scratch, "lock-off", written(scratch, "lock-off.json", lock), "either", "3.2.1", "Unlicense")
    sbom = json.loads((GATE / "sbom.json").read_text())
    sbom["components"][3]["licenses"] = [{"expression": "Unlicense"}]
    gate_refuses(scratch, "sbom-off", written(scratch, "sbom-off.json", sbom), "hasher", "Unlicense")

    for name, licences, words in (
        ("unlicensed", [], "no licence"),
        ("noassertion", [{"expression": "NOASSERTION"}], "NOASSERTION"),
        ("none", [{"license": {"id": "NONE"}}], "NONE"),
        ("reference", [{"expression": "MIT AND LicenseRef-Own"}], "reference"),
        ("document", [{"expression": "DocumentRef-spdx:LicenseRef-Own"}], "reference"),
        ("later", [{"expression": "Apache-2.0+"}], "later version"),
        ("unparsed", [{"expression": "MIT AND (ISC"}], "does not parse"),
        ("dangling", [{"expression": "MIT OR"}], "does not parse"),
        ("bare-with", [{"expression": "(MIT) WITH LLVM-exception"}], "does not parse"),
        ("long", [{"expression": "MIT" + " AND MIT" * 40}], "256"),
        ("exception", [{"expression": "MIT WITH LLVM-exception"}], "not on deny.toml's list"),
        ("neither", [{"expression": "Unlicense OR Zero"}], "Unlicense"),
        ("both", [{"expression": "MIT"}, {"expression": "Unlicense"}], "Unlicense"),
        ("named", [{"license": {"name": "A licence of its own"}}], "SPDX id"),
        ("encoded", [{"license": {"id": "MIT", "text": {"content": "Q29weQ==", "encoding": "base64"}}}], "encoding"),
    ):
        source = sbom_of(scratch, name, {"name": "lonely", "version": "1.0.0", "licenses": licences})
        gate_refuses(scratch, name, source, "lonely", words)
    # Nested components are part of the input, and are not read past.
    nested = sbom_of(scratch, "nested", {
        "name": "outer", "version": "1.0.0", "licenses": [{"expression": "MIT"}],
        "components": [{"name": "inner", "version": "1.0.0", "licenses": [{"expression": "Unlicense"}]}],
    })
    gate_refuses(scratch, "nested", nested, "outer", "nests")
    # Neither format, or no component at all.
    gate_refuses(scratch, "shapeless", written(scratch, "shapeless.json", {"packages": "none"}), "neither")
    gate_refuses(scratch, "empty", sbom_of(scratch, "empty"), "no component")
    gate_refuses(scratch, "not-json", GATE / "texts" / "MIT.txt", "not JSON")
    # A hostile name is shown as the intake shows hostile text.
    hostile = sbom_of(scratch, "hostile", {"name": "evil‮txt", "version": "1", "licenses": [{"expression": "Unlicense"}]})
    record = gate_refuses(scratch, "hostile", hostile, "\\u202e")
    assert "‮" not in record["reason"], "a reordering character is written as its code point"
    # Text that could hide or reorder what the NOTICE shows refuses its
    # component even under a listed licence; a copyright text keeps its
    # line breaks and tabs.
    for name, component, words in (
        ("bidi-name", {"name": "evil‮gnp", "version": "1", "licenses": [{"expression": "MIT"}]}, "\\u202e"),
        ("c1-version", {"name": "csi", "version": "1\u009b", "licenses": [{"expression": "MIT"}]}, "\\u009b"),
        ("line-name", {"name": "two\u2028lines", "version": "1", "licenses": [{"expression": "MIT"}]}, "\\u2028"),
        ("c1-text", {
            "name": "csi-text", "version": "1",
            "licenses": [{"license": {"id": "MIT", "text": {"content": "Copyright\n\tThe \u009b[31m Authors"}}}],
        }, "csi-text"),
        ("bidi-text", {
            "name": "bidi-text", "version": "1",
            "licenses": [{"license": {"id": "MIT", "text": {"content": "Copyright \u202e The Authors"}}}],
        }, "bidi-text"),
    ):
        refused = gate_refuses(scratch, name, sbom_of(scratch, name, component), words)
        assert "hide or reorder" in refused["reason"], f"{name}: the reason does not say why"
    hostile_lock = json.loads((GATE / "lock.json").read_text())
    hostile_lock["packages"][0]["name"] = "npx‮agent"
    gate_refuses(scratch, "bidi-lock", written(scratch, "bidi-lock.json", hostile_lock), "hide or reorder")
    tabbed = sbom_of(scratch, "tabbed", {
        "name": "tabbed", "version": "1",
        "licenses": [{"license": {"id": "MIT", "text": {"content": "Copyright\n\tThe Authors"}}}],
    })
    ran, out, _ = gate(scratch, "tabbed", tabbed)
    assert ran.returncode == 0 and "\tThe Authors" in (out / "NOTICE").read_text(), ran.stderr
    # The SBOM's own subject is part of its input: a licence it gives is
    # gated like any component's, and one with none gives the gate nothing.
    subject = json.loads((GATE / "sbom.json").read_text())
    subject["metadata"] = {"component": {"name": "the-subject", "version": "9.9.9", "licenses": [{"expression": "Unlicense"}]}}
    gate_refuses(scratch, "subject-off", written(scratch, "subject-off.json", subject), "the-subject", "Unlicense")
    subject["metadata"]["component"]["licenses"] = [{"expression": "MIT"}]
    ran, out, record = gate(scratch, "subject-on", written(scratch, "subject-on.json", subject))
    assert ran.returncode == 0 and record["components"] == 5, ran.stderr
    assert "the-subject 9.9.9 MIT" in (out / "NOTICE").read_text().splitlines(), "the subject is not in the NOTICE"
    del subject["metadata"]["component"]["licenses"]
    ran, _, record = gate(scratch, "subject-bare", written(scratch, "subject-bare.json", subject))
    assert ran.returncode == 0 and record["components"] == 4, ran.stderr
    subject["metadata"]["component"]["licenses"] = []
    ran, _, record = gate(scratch, "subject-empty", written(scratch, "subject-empty.json", subject))
    assert ran.returncode == 0 and record["components"] == 4, ran.stderr
    # A subject in no shape the gate reads is refused, never passed over.
    off = {"name": "the-subject", "version": "9.9.9", "licenses": [{"expression": "Unlicense"}]}
    for name, metadata in (
        ("subject-list", {"component": [off]}),
        ("subject-string", {"component": "the-subject"}),
        ("subject-zero", {"component": {**off, "licenses": 0}}),
        ("subject-blank", {"component": {**off, "licenses": ""}}),
        ("metadata-list", [{"component": off}]),
    ):
        malformed = {**json.loads((GATE / "sbom.json").read_text()), "metadata": metadata}
        gate_refuses(scratch, name, written(scratch, f"{name}.json", malformed), "metadata")
    # A licence on the list with no text to ship beside it.
    texts = scratch / "gate-texts-without-isc"
    texts.mkdir()
    for held in (GATE / "texts").iterdir():
        if held.name != "ISC.txt":
            (texts / held.name).write_bytes(held.read_bytes())
    gate_refuses(scratch, "textless", GATE / "lock.json", "ISC", texts=texts)
    # The asset gated must be there to be bound.
    ran = intake(
        "licence-gate", "--input", GATE / "lock.json", "--deny", ROOT / "deny.toml", "--texts", GATE / "texts",
        "--asset", scratch / "no-such-asset", "--out", scratch / "gate-assetless",
    )
    assert ran.returncode == 1 and json.loads(ran.stdout)["passed"] is False, ran.stderr
    # A directory that is already there is not written into.
    (scratch / "gate-taken").mkdir()
    ran, _, _ = gate(scratch, "taken", GATE / "lock.json")
    assert ran.returncode == 1 and "already" in ran.stderr and "Traceback" not in ran.stderr, ran.stderr
    # A write that fails partway leaves no directory behind to refuse a rerun.
    import argparse
    module = load_intake()
    arguments = argparse.Namespace(
        input=GATE / "sbom.json", deny=ROOT / "deny.toml", texts=GATE / "texts", asset=scratch / "gate-asset",
        out=scratch / "gate-interrupted",
    )
    writes = []
    write_bytes = pathlib.Path.write_bytes

    def failing(path: pathlib.Path, content: bytes) -> int:
        writes.append(path)
        if len(writes) == 2:
            raise OSError(28, "No space left on device")
        return write_bytes(path, content)

    pathlib.Path.write_bytes = failing
    try:
        module.licence_gate_command(arguments)
    except module.Refused as refused:
        assert "NOTICE" in str(refused), refused
    else:
        raise AssertionError("a failed write was not refused")
    finally:
        pathlib.Path.write_bytes = write_bytes
    assert len(writes) == 2 and not arguments.out.exists(), "a failed write left its directory behind"
    ran, out, _ = gate(scratch, "interrupted", GATE / "sbom.json")
    assert ran.returncode == 0 and (out / "NOTICE").exists(), ran.stderr

    # The ceilings: the component count and the text a component carries.
    module = load_intake()
    allowed = module.allowed_licences(ROOT / "deny.toml")
    module.GATE_COMPONENTS = 2
    record, files = module.licence_gate(GATE / "lock.json", allowed, GATE / "texts", scratch / "gate-asset")
    assert record["passed"] is False and "more than 2" in record["reason"] and not files, record
    module.GATE_COMPONENTS = 1 << 16
    module.COMPONENT_TEXT_BYTES = 8
    record, _ = module.licence_gate(GATE / "sbom.json", allowed, GATE / "texts", scratch / "gate-asset")
    assert record["passed"] is False and "fake-agent" in record["reason"], record
    module.COMPONENT_TEXT_BYTES = 1 << 20
    module.COMPONENT_TEXTS_BYTES = 8
    record, _ = module.licence_gate(GATE / "sbom.json", allowed, GATE / "texts", scratch / "gate-asset")
    assert record["passed"] is False and "together" in record["reason"], record
    module.COMPONENT_TEXTS_BYTES = 1 << 20
    # What `review` holds of every NOTICE and text until it writes them is
    # bounded for the run: the artifact past the ceiling fails closed.
    sboms = scratch / "gate-run-sboms"
    sboms.mkdir()
    (sboms / f"{'a' * 64}.json").write_bytes((GATE / "sbom.json").read_bytes())
    (sboms / f"{'b' * 64}.json").write_bytes((GATE / "lock.json").read_bytes())
    held: dict[str, bytes] = {}
    run = module.Gate(allowed, sboms, GATE / "texts")
    first = run.of("a" * 64, scratch / "gate-asset", held)
    assert first["passed"] is True, first
    module.GATE_HELD_BYTES = sum(len(content) for content in held.values()) + 1
    second = run.of("b" * 64, scratch / "gate-asset", held)
    assert second["passed"] is False and "review holds" in second["reason"], second
    assert sum(len(content) for content in held.values()) < module.GATE_HELD_BYTES, "the ceiling was passed"
    again = run.of("a" * 64, scratch / "gate-asset", held)
    assert again["passed"] is True, "bytes already held are not counted twice"


def check_licence_gate_notice(scratch: pathlib.Path) -> None:
    """A passing input ships one NOTICE assembled from it and every licence
    text it uses, and the record binds the asset, the NOTICE and each text."""
    ran, out, record = gate(scratch, "sbom", GATE / "sbom.json")
    assert ran.returncode == 0, f"a listed input was refused:\n{ran.stdout}{ran.stderr}"
    notice = (out / "NOTICE").read_bytes()
    assert notice == (GATE / "sbom.NOTICE").read_bytes(), "the NOTICE is not the one assembled from the fixture"
    used = ["Apache-2.0 WITH LLVM-exception", "ISC", "MIT"]
    assert sorted(path.name for path in (out / "LICENSES").iterdir()) == [f"{licence}.txt" for licence in used]
    for licence in used:
        assert (out / "LICENSES" / f"{licence}.txt").read_bytes() == (GATE / "texts" / f"{licence}.txt").read_bytes()
    assert sorted(path.name for path in out.iterdir()) == ["LICENSES", "NOTICE"], "the gate writes nothing else"
    assert record == {
        "asset": {"sha256": hashlib.sha256(GATE_ASSET).hexdigest(), "size": len(GATE_ASSET)},
        "components": 4,
        "licences": [
            {
                "id": licence,
                "sha256": hashlib.sha256((GATE / "texts" / f"{licence}.txt").read_bytes()).hexdigest(),
                "size": len((GATE / "texts" / f"{licence}.txt").read_bytes()),
            }
            for licence in used
        ],
        "notice": {"sha256": hashlib.sha256(notice).hexdigest(), "size": len(notice)},
        "passed": True,
    }, "the record does not bind the bytes the gate wrote"
    again, out_again, record_again = gate(scratch, "sbom-again", GATE / "sbom.json")
    assert again.returncode == 0 and record_again == record, "the same input gave another record"
    assert (out_again / "NOTICE").read_bytes() == notice, "the same input gave another NOTICE"

    # The intake's own npm lock: a bundled package is a component, and an
    # alternative is taken in the order it is written.
    ran, out, record = gate(scratch, "lock", GATE / "lock.json")
    assert ran.returncode == 0, f"a listed lock was refused:\n{ran.stdout}{ran.stderr}"
    lines = (out / "NOTICE").read_text().splitlines()
    for line in ("bundled-dep 1.0.0 ISC", "either 3.2.1 Apache-2.0", "npx-agent 2.1.0 MIT"):
        assert line in lines, f"the lock's NOTICE lacks {line!r}"
    assert record["components"] == 3 and [item["id"] for item in record["licences"]] == ["Apache-2.0", "ISC", "MIT"]


def check_licence_gate_review(scratch: pathlib.Path, data: dict) -> None:
    """`review` gates every mirror-marked artifact: with its input given, the
    record binds the artifact and the NOTICE becomes an asset; with none, it
    fails closed."""
    plain = {target["platform"]: target for target in next(e for e in data["entries"] if e["key"] == "fake")["targets"]}
    for target in plain.values():
        assert target["artifacts"][0]["licence_gate"] == {
            "passed": False, "reason": "no lockfile or SBOM was given",
        }, "a mirror-marked artifact with no input fails the gate"
    big = next(entry for entry in data["entries"] if entry["key"] == "big")
    assert "licence_gate" not in big["targets"][0]["artifacts"][0], "an artifact not marked for mirroring is not gated"

    linux = hashlib.sha256(archive("fake-agent", "linux-x86_64")).hexdigest()
    sboms = scratch / "sboms"
    sboms.mkdir()
    (sboms / f"{linux}.json").write_bytes((GATE / "sbom.json").read_bytes())
    ran = intake(
        "review", "--registry", REGISTRY, "--choices", CHOICES, "--runners", RUNNERS,
        "--observed", scratch / "observed", "--deny", ROOT / "deny.toml", "--assets", scratch / "textless-assets",
        "--out", scratch / "textless.json", "--sboms", sboms,
    )
    assert ran.returncode == 1 and "--texts" in ran.stderr and "Traceback" not in ran.stderr, ran.stderr
    gated, assets = review(scratch, "--sboms", sboms, "--texts", GATE / "texts", name="gated.json")
    targets = {target["platform"]: target for target in next(e for e in gated["entries"] if e["key"] == "fake")["targets"]}
    record = targets["linux-x86_64"]["artifacts"][0]["licence_gate"]
    assert record["passed"] is True and record["components"] == 4, record
    assert record["asset"] == {"sha256": linux, "size": len(archive("fake-agent", "linux-x86_64"))}, record["asset"]
    notice = (assets / record["notice"]["sha256"]).read_bytes()
    assert notice == (GATE / "sbom.NOTICE").read_bytes(), "the NOTICE asset is the one assembled from the input"
    for licence in record["licences"]:
        held = (assets / licence["sha256"]).read_bytes()
        assert held == (GATE / "texts" / f"{licence['id']}.txt").read_bytes(), "a licence text is an asset by its hash"
    assert targets["darwin-aarch64"]["artifacts"][0]["licence_gate"]["passed"] is False, "an input is per artifact"

    # The publishing job takes the gated copy and what ships beside it, and
    # still refuses the copy with no input.
    shipped = [(linux, archive("fake-agent", "linux-x86_64")), (record["notice"]["sha256"], notice)]
    shipped += [(licence["sha256"], (assets / licence["sha256"]).read_bytes()) for licence in record["licences"]]
    ran = publish_checked(scratch, "reviewed", gated, *shipped)
    assert ran.returncode == 0, f"a reviewed and gated copy was refused:\n{ran.stdout}{ran.stderr}"
    publish_refuses(publish_checked(scratch, "reviewed-alone", gated, shipped[0]), "NOTICE")
    darwin = hashlib.sha256(archive("fake-agent", "darwin-aarch64")).hexdigest()
    publish_refuses(
        publish_checked(scratch, "reviewed-ungated", gated, (darwin, archive("fake-agent", "darwin-aarch64"))),
        "licence-gate",
    )


def check_licence_gate_rereview(scratch: pathlib.Path, data: dict) -> None:
    """The gate's record is this review's, not the release's content: a
    version already merged with another record, or none, is kept with the
    record reviewed now, and `diff` shows the record that changed."""
    linux = hashlib.sha256(archive("fake-agent", "linux-x86_64")).hexdigest()
    sboms = scratch / "rereview-sboms"
    sboms.mkdir()
    (sboms / f"{linux}.json").write_bytes((GATE / "sbom.json").read_bytes())
    merged = copy.deepcopy(data)
    for target in next(entry for entry in merged["entries"] if entry["key"] == "fake")["targets"]:
        for artifact in target["artifacts"]:
            artifact.pop("licence_gate", None)
    held = written(scratch, "merged-ungated.json", merged)
    again, _ = review(scratch, "--merged", held, "--sboms", sboms, "--texts", GATE / "texts", name="rereview.json")
    entries = {entry["key"]: entry for entry in again["entries"]}
    assert "fake" in entries, "a version already reviewed was dropped for its licence-gate record"
    record = fake_target(again)["artifacts"][0]["licence_gate"]
    assert record["passed"] is True, "the record reviewed now does not stand"
    # Any other change to that version is still refused.
    changed = copy.deepcopy(merged)
    fake_target(changed)["args"] = ["acp"]
    refused, _ = review(scratch, "--merged", written(scratch, "merged-changed.json", changed), name="rereview-changed.json")
    assert "fake" not in {entry["key"] for entry in refused["entries"]}, "a changed same version was kept"

    ran = intake("diff", "--old", held, scratch / "rereview.json")
    assert ran.returncode == 0, ran.stderr
    lines = ran.stdout.splitlines()
    gated = [line for line in lines if "licence gate" in line]
    assert any("passed" in line and "was none" in line for line in gated), ran.stdout
    assert any("failed" in line and "was none" in line for line in gated), "the failing darwin record is not shown"
    assert not any("content changed" in line for line in lines), "a record change is called a content change"
    assert any("licence-gate record changed" in line and "fake" in line for line in lines), ran.stdout
    ran = intake("diff", "--old", scratch / "review.json", scratch / "rereview.json")
    assert ran.returncode == 0, ran.stderr
    assert any("licence gate" in line and "passed" in line and "was" in line and "failed" in line
               for line in ran.stdout.splitlines()), "a failing record turned passing is not shown"
    ran = intake("diff", "--old", scratch / "rereview.json", scratch / "rereview.json")
    assert "licence gate" not in ran.stdout, "an unchanged record is shown"
    ran = intake("diff", scratch / "rereview.json")
    assert any("licence gate" in line and "passed" in line for line in ran.stdout.splitlines()), ran.stdout
    # Two records that differ never read the same.
    module = load_intake()
    for field in ("licence", "licence-size", "notice-sha256", "notice-size", "extra", "nested", "failing-extra"):
        other = copy.deepcopy(record)
        if field == "licence":
            other["licences"][0]["sha256"] = "e" * 64
        elif field == "licence-size":
            other["licences"][0]["size"] += 1
        elif field == "notice-sha256":
            other["notice"]["sha256"] = "e" * 64
        elif field == "notice-size":
            other["notice"]["size"] += 1
        elif field == "extra":
            other["note"] = "x"
        elif field == "nested":
            other["licences"][0]["note"] = "x"
        else:
            failing = {"passed": False, "reason": "r"}
            assert module.gate_of({**failing, "note": "x"}) != module.gate_of(failing), "a failing record's extra reads the same"
            continue
        assert module.gate_of(other) != module.gate_of(record), f"a record changed in its {field} reads the same"


PYPI = "https://files.pythonhosted.org/packages/"
PYTHON = "3.12"
OWN = "pyagent-1.4.0-py3-none-any.whl"
CONSOLE = "[console_scripts]\npyagent = pyagent.acp:main\n"


def sha(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def listed(members: dict[str, tuple[bytes, bool]], dirs: tuple = (), links: tuple = ()) -> dict:
    """An unpack-check output for the files `members` gives by path, with
    their bytes and exec flags, the directories `dirs` and the links `links`."""
    files: list[dict] = [{"kind": "dir", "path": path} for path in dirs]
    files += [
        {"exec": runnable, "kind": "file", "path": path, "sha256": sha(content), "size": len(content)}
        for path, (content, runnable) in members.items()
    ]
    files += [{"kind": "link", "path": path, "target": target} for path, target in links]
    unpacked = sum(len(content) for content, _ in members.values())
    return {"decompressed": unpacked, "files": files, "unpacked": unpacked}


def interpreter(platform: str, *, program: bool = True, runnable: bool = True, stray: bool = False) -> tuple[bytes, dict]:
    """A python-build-standalone `install_only` archive for `platform`, with
    more files than a catalog lists inline, and its unpack-check output."""
    windows = platform.startswith("windows-")
    members: dict[str, tuple[bytes, bool]] = {}
    if program:
        name = "python/python.exe" if windows else f"python/bin/python{PYTHON}"
        members[name] = (f"interpreter for {platform}\n".encode(), runnable and not windows)
    for index in range(300):
        members[f"python/lib/python{PYTHON}/m{index:03}.py"] = (f"# {index}\n".encode(), False)
    if stray:
        members["lib/stray.py"] = (b"# outside\n", False)
    links = () if windows or not program else (("python/bin/python3", f"python{PYTHON}"),)
    dirs = ("python", "python/lib", f"python/lib/python{PYTHON}") + (() if windows else ("python/bin",))
    held = io.BytesIO()
    with gzip.GzipFile(fileobj=held, mode="wb", mtime=0) as zipped, tarfile.open(fileobj=zipped, mode="w") as tar:
        for path in dirs:
            info = tarfile.TarInfo(path)
            info.type, info.mode = tarfile.DIRTYPE, 0o755
            tar.addfile(info)
        for path, (content, executable) in members.items():
            info = tarfile.TarInfo(path)
            info.size, info.mode = len(content), 0o755 if executable else 0o644
            tar.addfile(info, io.BytesIO(content))
        for path, target in links:
            info = tarfile.TarInfo(path)
            info.type, info.linkname = tarfile.SYMTYPE, target
            tar.addfile(info)
    return held.getvalue(), listed(members, dirs, links)


def installed(member: str) -> str:
    """Where a wheel's member is installed, as `unpack-check` lists it."""
    top, _, rest = member.partition("/")
    if top.endswith(".data"):
        return "site/" + rest.partition("/")[2]
    return "site/" + member


def wheel(
    name: str,
    version: str,
    *,
    console: str | None = None,
    extra: dict[str, bytes] | None = None,
) -> tuple[bytes, dict]:
    """A wheel of `name` at `version`, its entry points `console` where given,
    and its unpack-check output."""
    dist = f"{name}-{version}.dist-info"
    members = {
        f"{name}/__init__.py": f"# {name}\n".encode(),
        f"{dist}/METADATA": f"Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n".encode(),
        f"{dist}/WHEEL": b"Wheel-Version: 1.0\nRoot-Is-Purelib: true\n",
    }
    if console is not None:
        members[f"{dist}/entry_points.txt"] = console.encode()
    members.update(extra or {})
    members[f"{dist}/RECORD"] = b""
    held = io.BytesIO()
    with zipfile.ZipFile(held, "w", zipfile.ZIP_DEFLATED) as archive_file:
        for member, content in members.items():
            archive_file.writestr(zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0)), content)
    return held.getvalue(), listed({installed(member): (content, False) for member, content in members.items()})


def claimed(built: tuple[bytes, dict], *, entries: int | None = None, size: int | None = None) -> tuple[bytes, dict]:
    """The wheel `built` with its end-of-central-directory record moved into a
    zip64 one that claims `entries` members or a central directory of `size`
    bytes, without holding either."""
    content, listing = built
    end = content.rindex(b"PK\x05\x06")
    count, held_size, offset = struct.unpack("<HII", content[end + 10 : end + 20])
    record = struct.pack(
        "<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0,
        entries if entries is not None else count, entries if entries is not None else count,
        size if size is not None else held_size, offset,
    )
    locator = struct.pack("<IIQI", 0x07064B50, 0, end, 1)
    final = struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0)
    return content[:end] + record + locator + final, listing


def decoyed(built: tuple[bytes, dict], entries: int) -> tuple[bytes, dict]:
    """The zip64 wheel `built` (as `claimed` writes it) with a second zip64
    record, claiming `entries` members, put just before its locator, where
    `zipfile` reads one, while the locator still points at the first."""
    content, listing = built
    locator = content.rindex(b"PK\x06\x07")
    first = locator - 56
    record = bytearray(content[first:locator])
    record[24:40] = struct.pack("<QQ", entries, entries)
    return content[:locator] + bytes(record) + content[locator:], listing


def commented(built: tuple[bytes, dict]) -> tuple[bytes, dict]:
    """The wheel `built` with a zip comment after its end record."""
    held = io.BytesIO(built[0])
    with zipfile.ZipFile(held, "a") as archive_file:
        archive_file.comment = b"a comment"
    return held.getvalue(), built[1]


def bomb(inflated: int) -> bytes:
    """The agent's own wheel with an `entry_points.txt` that deflates to a
    little and inflates to `inflated` bytes."""
    held = io.BytesIO()
    dist = "pyagent-1.4.0.dist-info"
    with zipfile.ZipFile(held, "w", zipfile.ZIP_DEFLATED) as archive_file:
        archive_file.writestr(f"{dist}/METADATA", "Metadata-Version: 2.1\nName: pyagent\nVersion: 1.4.0\n")
        info = zipfile.ZipInfo(f"{dist}/entry_points.txt", date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        with archive_file.open(info, "w") as member:
            member.write(CONSOLE.encode())
            chunk = b"#" * (1 << 20)
            for _ in range(inflated >> 20):
                member.write(chunk)
        archive_file.writestr(f"{dist}/RECORD", "")
    return held.getvalue()


def twice(built: tuple[bytes, dict], member: str, content: bytes) -> tuple[bytes, dict]:
    """The wheel `built` with a second member named `member`."""
    held = io.BytesIO(built[0])
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        with zipfile.ZipFile(held, "a") as archive_file:
            archive_file.writestr(zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0)), content)
    return held.getvalue(), built[1]


def own(**changed: object) -> dict:
    """The agent's own wheel as a report item, from PyPI."""
    built = wheel("pyagent", "1.4.0", console=CONSOLE, extra={
        "pyagent-1.4.0.data/purelib/pyagent_extra.py": b"# a module under purelib is installed\n",
    })
    item = {"url": PYPI + "aa/bb/" + OWN, "name": "pyagent", "version": "1.4.0", "wheel": built}
    item.update(changed)
    return item


def dependency(file: str, *, name: str = "rpds-py", version: str = "0.20.0", **changed: object) -> dict:
    item = {"url": PYPI + "cc/dd/" + file, "name": name, "version": version, "wheel": wheel("rpds_py", "0.20.0")}
    item.update(changed)
    return item


def sdist(file: str, name: str, version: str) -> dict:
    return {"url": PYPI + "ee/ff/" + file, "name": name, "version": version, "wheel": None}


def lay_out_uvx(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    items: list[dict],
    *,
    built: tuple[bytes, dict] | None = None,
    interpreter_chain: list[str] | None = None,
    report_version: object = "1",
) -> None:
    """What the untrusted job leaves for one `uvx` target; `built` stands for
    the interpreter's bytes and listing, the pinned ones by default."""
    pins = json.loads(INTERPRETERS.read_text())
    place = observed / agent / platform
    (place / "interpreter").mkdir(parents=True)
    content, listing = built or interpreter(platform)
    (place / "interpreter/artifact").write_bytes(content)
    chain = interpreter_chain or [pins[platform]["url"]]
    (place / "interpreter/fetched.json").write_text(json.dumps({"chain": chain}))
    (place / "interpreter/unpack-check.json").write_text(json.dumps(listing))
    report = {
        "environment": {"python_version": PYTHON},
        "install": [
            {
                "download_info": {"archive_info": {"hashes": {"sha256": "0" * 64}}, "url": item["url"]},
                "is_direct": False,
                "metadata": {"metadata_version": "2.1", "name": item["name"], "version": item["version"]},
                "requested": index == 0,
            }
            for index, item in enumerate(items)
        ],
        "pip_version": "24.2",
        "version": report_version,
    }
    (place / "resolved.json").write_text(json.dumps(report))
    for index, item in enumerate(items):
        if item["wheel"] is None:
            continue
        held = place / "wheels" / str(index)
        held.mkdir(parents=True)
        content, listing = item["wheel"]
        (held / "artifact").write_bytes(content)
        (held / "fetched.json").write_text(json.dumps({"chain": item.get("chain", [item["url"]])}))
        (held / "unpack-check.json").write_text(json.dumps(item.get("listing", listing)))
    (place / "probe.json").write_text(json.dumps(PROBED))


def pinned_interpreters(scratch: pathlib.Path) -> pathlib.Path:
    """The committed pins, each holding the hash of the archive built here."""
    pins = json.loads(INTERPRETERS.read_text())
    for platform, pin in pins.items():
        pin["sha256"] = sha(interpreter(platform)[0])
    return written(scratch, "interpreters.json", pins)


def uvx_registry(scratch: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    """A registry of `uvx` agents and what the untrusted job left for them;
    every target not laid out here is left with nothing observed."""
    observed = scratch / "observed"
    agents: list[dict] = []
    choices: dict[str, dict] = {}

    def agent(agent_id: str, distribution: dict | None = None, **uvx: object) -> None:
        spec: dict = {"package": "pyagent==1.4.0", "args": ["acp"]}
        spec.update(uvx)
        agents.append({
            "id": agent_id, "name": agent_id, "version": "1.4.0", "license": "MIT",
            "distribution": distribution if distribution is not None else {"uvx": spec},
        })
        choices[agent_id] = {"key": agent_id.removesuffix("-agent")}

    def target(agent_id: str, items: list[dict], platform: str = "linux-x86_64", **kept: object) -> None:
        lay_out_uvx(observed, agent_id, platform, items, **kept)

    rpds = "rpds_py-0.20.0-cp312-cp312-"
    agent("py-agent", env={"PYAGENT_QUIET": "1"})
    choices["py-agent"]["key"] = "pyagent"
    target(
        "py-agent",
        [own(), dependency(rpds + "manylinux_2_17_x86_64.manylinux2014_x86_64.whl", name="Rpds.Py")],
        interpreter_chain=[json.loads(INTERPRETERS.read_text())["linux-x86_64"]["url"], "https://objects.python.example/blob/1"],
    )
    target("py-agent", [own(), dependency(rpds + "macosx_11_0_arm64.whl")], "darwin-aarch64")
    target("py-agent", [own(), dependency(rpds + "win_amd64.whl")], "windows-x86_64")
    # A dependency built for another machine: this target alone is dropped.
    target("py-agent", [own(), dependency(rpds + "manylinux_2_17_x86_64.whl")], "linux-aarch64")

    # Each part of `.data` but purelib and platlib, one on each target.
    agent("data-agent")
    for platform, member in (
        ("linux-x86_64", "pyagent-1.4.0.data/scripts/pyagent"),
        ("darwin-aarch64", "pyagent-1.4.0.data/headers/pyagent.h"),
        ("windows-x86_64", "pyagent-1.4.0.data/data/share/pyagent.txt"),
    ):
        data_wheel = wheel("pyagent", "1.4.0", console=CONSOLE, extra={member: b"# not a module\n"})
        target("data-agent", [own(wheel=data_wheel)], platform)

    agent("sdist-agent")
    target("sdist-agent", [sdist("pyagent-1.4.0.tar.gz", "pyagent", "1.4.0")])
    target("sdist-agent", [own(), sdist("oldlib-0.1.tar.gz", "oldlib", "0.1"), sdist("other-2.0.zip", "Other", "2.0")], "darwin-aarch64")

    dep = rpds + "manylinux_2_17_x86_64.whl"
    cases: list[tuple[str, list[dict], dict]] = [
        ("host-agent", [own(), dependency(dep, url="https://pypi.example/packages/cc/dd/" + dep)], {}),
        ("http-agent", [own(), dependency(dep, url="http://files.pythonhosted.org/packages/cc/dd/" + dep)], {}),
        ("percent-agent", [own(), dependency("rpds%5Fpy-0.20.0-cp312-cp312-manylinux_2_17_x86_64.whl")], {}),
        ("grammar-agent", [own(), dependency("rpds_py-0.20.0-manylinux_2_17_x86_64.whl")], {}),
        ("named-agent", [own(), dependency(dep, name="rpds-pi")], {}),
        ("versioned-agent", [own(), dependency(dep, version="0.20.1")], {}),
        ("glibc-agent", [own(), dependency(rpds + "manylinux_2_35_x86_64.whl")], {}),
        ("musl-agent", [own(), dependency(rpds + "musllinux_1_2_x86_64.whl")], {}),
        ("chain-agent", [own(), dependency(dep, chain=[PYPI + "cc/dd/other.whl"])], {}),
        ("hop-agent", [own(), dependency(dep, chain=[PYPI + "cc/dd/" + dep, "http://192.0.2.9/x.whl"])], {}),
        ("pin-agent", [own()], {"built": (b"not the pinned interpreter\n", interpreter("linux-x86_64")[1])}),
        ("fetch-agent", [own()], {"interpreter_chain": ["https://python.example/elsewhere-install_only.tar.gz"]}),
        ("report-agent", [own()], {"report_version": "2"}),
        ("missing-agent", [dependency(dep)], {}),
        ("unlisted-agent", [own(wheel=wheel("pyagent", "1.4.0"))], {}),
        ("extras-ep-agent", [own(wheel=wheel("pyagent", "1.4.0", console="[console_scripts]\npyagent = pyagent.acp:main [cli]\n"))], {}),
        ("spaced-ep-agent", [own(wheel=wheel("pyagent", "1.4.0", console="[console_scripts]\npyagent = pyagent acp:main\n"))], {}),
        ("repeated-ep-agent", [own(wheel=wheel("pyagent", "1.4.0", console=CONSOLE + "pyagent = other:main\n"))], {}),
        ("unnamed-ep-agent", [own(wheel=wheel("pyagent", "1.4.0", console="[console_scripts]\npyagent-cli = pyagent.acp:main\n"))], {}),
        ("dotted-ep-agent", [own(wheel=wheel("pyagent", "1.4.0", console="[console_scripts]\npyagent = pyagent..acp:main\n"))], {}),
        ("overlap-agent", [own(), dependency(dep, wheel=(wheel("rpds_py", "0.20.0")[0], listed({"site/pyagent/__init__.py": (b"x", False)})))], {}),
        ("outside-agent", [own(), dependency(dep, wheel=(wheel("rpds_py", "0.20.0")[0], listed({"lib/rpds.py": (b"x", False)})))], {}),
        ("stray-agent", [own()], {"built": (interpreter("linux-x86_64")[0], interpreter("linux-x86_64", stray=True)[1])}),
        ("noexec-agent", [own()], {"built": (interpreter("linux-x86_64")[0], interpreter("linux-x86_64", runnable=False)[1])}),
        ("noprogram-agent", [own()], {"built": (interpreter("linux-x86_64")[0], interpreter("linux-x86_64", program=False)[1])}),
        ("datadir-agent", [own(wheel=wheel("pyagent", "1.4.0", console=CONSOLE, extra={"PyAgent-1.4.0.DATA/scripts/x": b"x"}))], {}),
        # The central directory's own record is held to the ceilings before it is read.
        ("entries-agent", [own(wheel=claimed(own()["wheel"], entries=(1 << 20) + 1))], {}),
        ("directory-agent", [own(wheel=claimed(own()["wheel"], size=46 * (1 << 20) + 1))], {}),
        # The zip64 record checked is the one zipfile reads, just before the locator.
        ("decoy-agent", [own(wheel=decoyed(claimed(own()["wheel"]), (1 << 20) + 1))], {}),
        ("comment-agent", [own(wheel=commented(own()["wheel"]))], {}),
        ("zip64ok-agent", [own(wheel=claimed(own()["wheel"]))], {}),
        # Two members the client takes for one: a case or a leading ./ apart.
        ("case-agent", [own(wheel=twice(own()["wheel"], "PyAgent/__init__.py", b"# again\n"))], {}),
        ("dotslash-agent", [own(wheel=twice(own()["wheel"], "./pyagent/__init__.py", b"# again\n"))], {}),
        # A report that installs more packages than a target may.
        ("many-agent", [own()] + [sdist(f"dep{index}-1.0.tar.gz", f"dep{index}", "1.0") for index in range(256)], {}),
        # A `[DEFAULT]` section is a section, not defaults for console_scripts.
        ("default-agent", [own(wheel=wheel("pyagent", "1.4.0", console="[DEFAULT]\npyagent = pyagent.acp:main\n[console_scripts]\n"))], {}),
        ("twice-agent", [own(wheel=twice(own()["wheel"], "pyagent-1.4.0.dist-info/entry_points.txt", b"[console_scripts]\npyagent = other:main\n"))], {}),
        ("bigep-agent", [own(wheel=wheel("pyagent", "1.4.0", console=CONSOLE + "#" * (64 << 10) + "\n"))], {}),
        # A wheel's address is plain: no query, fragment, params, backslash or odd segment.
        ("query-agent", [own(), dependency(dep, url=PYPI + "cc/dd/" + dep + "?x=1")], {}),
        ("fragment-agent", [own(), dependency(dep, url=PYPI + "cc/dd/" + dep + "#sha256=" + "0" * 64)], {}),
        ("params-agent", [own(), dependency(dep, url=PYPI + "cc;p/dd/" + dep)], {}),
        ("backslash-agent", [own(), dependency(dep, url=PYPI + "cc\\dd/" + dep)], {}),
        ("empty-agent", [own(), dependency(dep, url=PYPI + "cc//dd/" + dep)], {}),
        ("dot-agent", [own(), dependency(dep, url=PYPI + "cc/./dd/" + dep)], {}),
        ("dotdot-agent", [own(), dependency(dep, url=PYPI + "cc/../dd/" + dep)], {}),
        ("percentdir-agent", [own(), dependency(dep, url=PYPI + "c%63/dd/" + dep)], {}),
        # A wheel is fetched from PyPI alone, every hop of the way.
        ("wheelhop-agent", [own(), dependency(dep, chain=[PYPI + "cc/dd/" + dep, "https://objects.example/rpds.whl"])], {}),
        # A build row holds what a drop reason may quote, no more.
        ("longbuild-agent", [own(), sdist("a" * 4000 + "-0.1.tar.gz", "a" * 4000, "0.1")], {}),
    ]
    for agent_id, items, kept in cases:
        agent(agent_id)
        target(agent_id, items, **kept)
    # A pin that is not one package at one version is refused before anything is read.
    for agent_id, package in (
        ("range-agent", "pyagent>=1.4.0"),
        ("extra-agent", "pyagent[cli]==1.4.0"),
        ("url-agent", "pyagent @ https://example.com/pyagent.whl"),
        ("marker-agent", "pyagent==1.4.0; python_version > '3'"),
        ("bare-agent", "pyagent"),
        ("edge-agent", "-pyagent==1.4.0"),
    ):
        agent(agent_id, package=package)
    agent("loader-agent", env={"LD_PRELOAD": "/tmp/hook.so"})
    agent("listed-agent", args="acp")
    # A binary agent that also names uvx is taken in as a binary agent.
    agent("both-agent", {
        "binary": {"linux-x86_64": {"archive": "https://downloads.both.example/both.tar.gz", "cmd": "./agent"}},
        "uvx": {"package": "pyagent==1.4.0", "args": ["acp"]},
    })
    place = observed / "both-agent" / "linux-x86_64"
    place.mkdir(parents=True)
    (place / "artifact").write_bytes(archive("both-agent", "linux-x86_64"))
    (place / "fetched.json").write_text(json.dumps({"chain": ["https://downloads.both.example/both.tar.gz"]}))
    (place / "unpack-check.json").write_text(json.dumps(UNPACKED))
    (place / "probe.json").write_text(json.dumps(PROBED))
    registry = written(scratch, "uvx-registry.json", {"version": "1.0.0", "agents": agents})
    return registry, written(scratch, "uvx-choices.json", choices)


def check_uvx(scratch: pathlib.Path) -> None:
    """A `uvx` agent is pinned per target as an interpreter and the wheels pip
    resolved for it; each target that cannot be is dropped alone, naming why."""
    scratch = scratch / "uvx"
    registry, choices = uvx_registry(scratch)
    pins = pinned_interpreters(scratch)
    data, assets = review(scratch, "--interpreters", pins, registry=registry, choices=choices)
    entries = {entry["key"]: entry for entry in data["entries"]}
    assert sorted(entries) == ["both", "pyagent", "zip64ok"], sorted(entries)
    assert entries["both"]["kind"] == "binary", "an agent that lists a binary is a binary agent"

    entry = entries["pyagent"]
    assert entry["kind"] == "uvx" and entry["registry_id"] == "py-agent", entry["kind"]
    assert entry["admits"] == PROBED["agentInfo"], entry["admits"]
    targets = {target["platform"]: target for target in entry["targets"]}
    assert sorted(targets) == ["darwin-aarch64", "linux-x86_64", "windows-x86_64"], sorted(targets)
    pinned = json.loads(INTERPRETERS.read_text())
    for platform, target in targets.items():
        windows = platform.startswith("windows-")
        assert target["cmd"] == ("python/python.exe" if windows else "python/bin/python3.12"), target["cmd"]
        assert target["interpreter_args"] == ["-I"], target["interpreter_args"]
        assert target["args"] == ["acp"] and target["env"] == {"PYAGENT_QUIET": "1"}, target
        assert target["entry_point"] == "pyagent.acp:main", target["entry_point"]
        assert target["probe"]["sandbox"] == "closed", target["probe"]
        python, *wheels = target["artifacts"]
        assert python["url"] == pinned[platform]["url"] and python["format"] == "tar.gz", python
        assert python["sha256"] == sha(interpreter(platform)[0]), "the interpreter is the bytes its pin names"
        listing = python["files"]
        assert sorted(listing) == ["sha256", "size"], "an interpreter's long file list is a file-list asset"
        kept = (assets / listing["sha256"]).read_bytes()
        assert sha(kept) == listing["sha256"] and len(kept) == listing["size"], listing
        assert len(json.loads(kept)) == len(interpreter(platform)[1]["files"]), "the asset holds every file"
        assert [artifact["format"] for artifact in wheels] == ["wheel", "wheel"], wheels
        assert wheels[0]["url"] == PYPI + "aa/bb/" + OWN, "the wheels follow in the report's order"
        assert wheels[0]["files"] == own()["wheel"][1]["files"], "a wheel's short file list is listed inline"
        assert "site/pyagent_extra.py" in {file["path"] for file in wheels[0]["files"]}, "purelib is installed"
        for artifact in target["artifacts"]:
            assert "mirror_licence" not in artifact, "an interpreter and its wheels are never marked for mirroring"
    assert targets["linux-x86_64"]["hosts"] == [
        "python.example", "objects.python.example", "files.pythonhosted.org",
    ], targets["linux-x86_64"]["hosts"]

    # The evaluator's first case: a wheel for another machine drops that target only.
    dropped(data, "py-agent", "linux-aarch64", "rpds_py-0.20.0-cp312-cp312-manylinux_2_17_x86_64.whl", "tags")
    dropped(data, "py-agent", "windows-aarch64", "no interpreter is pinned")
    dropped(data, "py-agent", "freebsd-x86_64", "no interpreter is pinned")

    # The evaluator's second case: a part of `.data` the client does not install.
    dropped(data, "data-agent", "linux-x86_64", OWN, "pyagent-1.4.0.data/scripts/pyagent", "under scripts")
    dropped(data, "data-agent", "darwin-aarch64", "under headers")
    dropped(data, "data-agent", "windows-x86_64", "under data,")
    dropped(data, "data-agent", None, "no target")
    dropped(data, "datadir-agent", "linux-x86_64", "under scripts")

    # The evaluator's third case: a source distribution marks its target for a build.
    builds = {
        item["platform"]: item.get("build")
        for item in data["dropped"]
        if item["registry_id"] == "sdist-agent" and "platform" in item
    }
    assert builds["linux-x86_64"] == [
        {"package": "pyagent", "url": PYPI + "ee/ff/pyagent-1.4.0.tar.gz", "version": "1.4.0"},
    ], builds
    assert builds["darwin-aarch64"] == [
        {"package": "oldlib", "url": PYPI + "ee/ff/oldlib-0.1.tar.gz", "version": "0.1"},
        {"package": "Other", "url": PYPI + "ee/ff/other-2.0.zip", "version": "2.0"},
    ], builds
    assert builds["windows-x86_64"] is None, "a target with no report is not marked for a build"
    dropped(data, "sdist-agent", "linux-x86_64", "waits for a build at review")
    dropped(data, "sdist-agent", "darwin-aarch64", "waits for a build at review")
    assert not [item for item in data["dropped"] if item["registry_id"] == "py-agent" and "build" in item]

    dropped(data, "host-agent", "linux-x86_64", "pypi.example", "files.pythonhosted.org/packages/")
    dropped(data, "http-agent", "linux-x86_64", "http://files.pythonhosted.org", "files.pythonhosted.org/packages/")
    dropped(data, "percent-agent", "linux-x86_64", "%")
    dropped(data, "grammar-agent", "linux-x86_64", "not a wheel's file name")
    dropped(data, "named-agent", "linux-x86_64", "rpds-pi", "agree")
    dropped(data, "versioned-agent", "linux-x86_64", "0.20.1", "agree")
    dropped(data, "glibc-agent", "linux-x86_64", "manylinux_2_35", "tags")
    dropped(data, "musl-agent", "linux-x86_64", "musllinux", "tags")
    dropped(data, "chain-agent", "linux-x86_64", "does not begin at the report's address")
    dropped(data, "hop-agent", "linux-x86_64", "http://192.0.2.9")
    dropped(data, "pin-agent", "linux-x86_64", "the interpreter", "not the bytes")
    dropped(data, "fetch-agent", "linux-x86_64", "the interpreter", "does not begin at the pinned address")
    dropped(data, "report-agent", "linux-x86_64", "report", "not 1")
    dropped(data, "missing-agent", "linux-x86_64", "installs no pyagent 1.4.0")
    dropped(data, "unlisted-agent", "linux-x86_64", "entry_points.txt")
    dropped(data, "extras-ep-agent", "linux-x86_64", "console script", "module:attr")
    dropped(data, "spaced-ep-agent", "linux-x86_64", "console script", "module:attr")
    dropped(data, "repeated-ep-agent", "linux-x86_64", "entry_points.txt")
    dropped(data, "unnamed-ep-agent", "linux-x86_64", "no console script pyagent")
    dropped(data, "dotted-ep-agent", "linux-x86_64", "console script", "module:attr")
    dropped(data, "overlap-agent", "linux-x86_64", "site/pyagent/__init__.py", "two of its artifacts")
    dropped(data, "outside-agent", "linux-x86_64", "lib/rpds.py", "site/")
    dropped(data, "stray-agent", "linux-x86_64", "lib/stray.py", "python/")
    dropped(data, "noexec-agent", "linux-x86_64", "python/bin/python3.12", "not runnable")
    dropped(data, "noprogram-agent", "linux-x86_64", "python/bin/python3.12", "not a file")
    dropped(data, "entries-agent", "linux-x86_64", OWN, "central directory", str((1 << 20)))
    dropped(data, "directory-agent", "linux-x86_64", OWN, "central directory", str(46 * (1 << 20)))
    dropped(data, "decoy-agent", "linux-x86_64", OWN, "central directory", "zip64")
    dropped(data, "comment-agent", "linux-x86_64", OWN, "comment")
    assert [target["platform"] for target in entries["zip64ok"]["targets"]] == ["linux-x86_64"], "a zip64 wheel within the ceilings reads"
    dropped(data, "case-agent", "linux-x86_64", OWN, "PyAgent/__init__.py", "twice")
    dropped(data, "dotslash-agent", "linux-x86_64", OWN, "pyagent/__init__.py", "twice")
    dropped(data, "many-agent", "linux-x86_64", "more than 256 packages")
    dropped(data, "default-agent", "linux-x86_64", "no console script pyagent")
    dropped(data, "twice-agent", "linux-x86_64", "entry_points.txt", "twice")
    dropped(data, "bigep-agent", "linux-x86_64", "entry_points.txt", str(64 << 10))
    for agent_id in ("query", "fragment", "params", "backslash", "percentdir"):
        dropped(data, f"{agent_id}-agent", "linux-x86_64", "not a plain address")
    for agent_id in ("empty", "dot", "dotdot"):
        dropped(data, f"{agent_id}-agent", "linux-x86_64", "segment")
    dropped(data, "wheelhop-agent", "linux-x86_64", "objects.example", "files.pythonhosted.org")
    long_rows = [item for item in data["dropped"] if item["registry_id"] == "longbuild-agent" and item.get("platform") == "linux-x86_64"]
    assert len(long_rows) == 1 and len(long_rows[0]["build"]) == 1, long_rows
    for field in ("package", "url", "version"):
        held_field = long_rows[0]["build"][0][field]
        assert len(held_field) <= 303, f"a build row's {field} is {len(held_field)} characters, past what a reason quotes"
    for agent_id in ("range-agent", "extra-agent", "url-agent", "marker-agent", "bare-agent", "edge-agent"):
        dropped(data, agent_id, None, "name==version")
    dropped(data, "loader-agent", None, "LD_PRELOAD")
    dropped(data, "listed-agent", None, "args")

    # With no pins at all, no platform has an interpreter.
    unpinned, _ = review(scratch, name="unpinned.json", registry=registry, choices=choices)
    assert "pyagent" not in {entry["key"] for entry in unpinned["entries"]}, "no pin, no uvx target"
    dropped(unpinned, "py-agent", "linux-x86_64", "no interpreter is pinned")

    # The diff shows the whole start beside cmd, and says nothing is mirrored.
    ran = intake("diff", scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    lines = ran.stdout.splitlines()
    assert "- interpreter args: ` -I `" in lines, ran.stdout
    assert "- entry point: ` pyagent.acp:main `" in lines, ran.stdout
    assert any("never mirrored" in line for line in lines), ran.stdout
    assert not any("installs as its bundle" in line for line in lines), ran.stdout
    old = copy.deepcopy(data)
    for listed_entry in old["entries"]:
        if listed_entry["key"] == "pyagent":
            listed_entry["version"] = "1.3.0"
            for listed_target in listed_entry["targets"]:
                listed_target["entry_point"] = "pyagent.old:main"
    ran = intake("diff", "--old", written(scratch, "uvx-old.json", old), scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    assert any("entry point" in line and "pyagent.old:main" in line and "was" in line for line in ran.stdout.splitlines())


def check_uvx_interpreters(scratch: pathlib.Path) -> None:
    """The reviewer's pins are refused whole where any one breaks a rule."""
    pins = json.loads(INTERPRETERS.read_text())
    url = pins["linux-x86_64"]["url"]
    for name, changed, words in (
        ("http", {"url": url.replace("https://", "http://")}, "install_only"),
        ("suffix", {"url": url.replace("-install_only.tar.gz", "-full.tar.zst")}, "install_only"),
        ("digest", {"sha256": "A" * 64}, "sha256"),
        ("python", {"python": "3"}, "3.M"),
        ("pythonx", {"python": "3.x"}, "3.M"),
        ("extra", {"mirror": True}, "mirror"),
    ):
        held = copy.deepcopy(pins)
        held["linux-x86_64"].update(changed)
        ran = intake(
            "review", "--registry", REGISTRY, "--choices", CHOICES, "--runners", RUNNERS,
            "--observed", scratch / "observed", "--deny", ROOT / "deny.toml",
            "--assets", scratch / "none", "--out", scratch / f"pins-{name}.json",
            "--interpreters", written(scratch, f"pins-{name}-in.json", held),
        )
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not (scratch / f"pins-{name}.json").exists(), ran.stderr
    for name, text, words in (
        ("twice", '{"linux-x86_64": {}, "linux-x86_64": {}}', "twice"),
        ("freebsd", json.dumps({"freebsd-x86_64": pins["linux-x86_64"]}), "freebsd-x86_64"),
        ("list", "[]", "platform"),
    ):
        path = scratch / f"pins-{name}-in.json"
        path.write_text(text)
        ran = intake(
            "review", "--registry", REGISTRY, "--choices", CHOICES, "--runners", RUNNERS,
            "--observed", scratch / "observed", "--deny", ROOT / "deny.toml",
            "--assets", scratch / "none", "--out", scratch / f"pins-{name}.json", "--interpreters", path,
        )
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr, ran.stderr


def check_uvx_bomb() -> None:
    """An `entry_points.txt` that inflates past its ceiling is refused having
    inflated no more than the ceiling: the budget counts the stored bytes, so
    only the bounded read keeps a deflate bomb out of memory."""
    intake_module = load_intake()
    inflated = 256 << 20
    content = bomb(inflated)
    assert len(content) < (1 << 20), len(content)
    tracemalloc.start()
    try:
        intake_module.wheel_read(io.BytesIO(content), "pyagent")
    except intake_module.Dropped as failed:
        refused = str(failed)
    else:
        refused = ""
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    assert "entry_points.txt" in refused and str(intake_module.ENTRY_POINTS_BYTES) in refused, refused
    assert peak < 16 << 20, f"reading the bomb held {peak} bytes"


def check_uvx_tags() -> None:
    """Each wheel tag an interpreter of 3.12 accepts on a platform, beside the
    nearest one it refuses."""
    fits = load_intake().fits
    for tags, platform, held in (
        ("py3-none-any", "linux-x86_64", True),
        ("py2.py3-none-any", "linux-x86_64", True),
        ("py2-none-any", "linux-x86_64", False),
        ("py312-none-any", "linux-x86_64", True),
        ("py313-none-any", "linux-x86_64", False),
        ("cp312-cp312-manylinux_2_34_x86_64", "linux-x86_64", True),
        ("cp312-cp312-manylinux_2_35_x86_64", "linux-x86_64", False),
        ("cp312-cp312-manylinux2014_x86_64", "linux-x86_64", True),
        ("cp312-cp312-manylinux1_x86_64", "linux-x86_64", True),
        ("cp312-cp312-manylinux2014_aarch64", "linux-x86_64", False),
        ("cp312-cp312-manylinux_2_17_aarch64.manylinux2014_aarch64", "linux-aarch64", True),
        ("cp312-cp312-manylinux2010_aarch64", "linux-aarch64", False),
        ("cp312-cp312-musllinux_1_2_x86_64", "linux-x86_64", False),
        ("cp312-cp312-linux_x86_64", "linux-x86_64", False),
        ("cp313-cp313-manylinux_2_17_x86_64", "linux-x86_64", False),
        ("cp312-cp312d-manylinux_2_17_x86_64", "linux-x86_64", False),
        ("cp313-cp313t-manylinux_2_17_x86_64", "linux-x86_64", False),
        ("cp312-none-any", "linux-x86_64", True),
        ("cp312-abi3-win_amd64", "windows-x86_64", True),
        ("cp32-abi3-win_amd64", "windows-x86_64", True),
        ("cp31-abi3-win_amd64", "windows-x86_64", False),
        ("cp311-cp311-win_amd64", "windows-x86_64", False),
        ("cp311-abi3-win_arm64", "windows-aarch64", True),
        ("cp312-cp312-win32", "windows-x86_64", False),
        ("cp312-cp312-win_amd64", "windows-aarch64", False),
        ("cp312-cp312-macosx_11_0_arm64", "darwin-aarch64", True),
        ("cp312-cp312-macosx_12_0_arm64", "darwin-aarch64", False),
        ("cp312-cp312-macosx_10_9_universal2", "darwin-aarch64", True),
        ("cp312-cp312-macosx_10_12_x86_64", "darwin-x86_64", True),
        ("cp312-cp312-macosx_10_13_x86_64", "darwin-x86_64", False),
        ("cp312-cp312-macosx_10_9_intel", "darwin-x86_64", True),
        ("cp312-cp312-macosx_10_9_arm64", "darwin-x86_64", False),
        ("py3-none-any", "freebsd-x86_64", True),
        ("cp312-cp312-freebsd_14_0_amd64", "freebsd-x86_64", False),
    ):
        assert fits(tags, platform, 12) is held, f"{tags} on {platform}: expected {held}"


def check_uvx_render(scratch: pathlib.Path) -> None:
    """The committed catalog carries a `uvx` entry, and a `uvx` entry missing
    what the client reads to start it is refused before anything is written."""
    catalog = json.loads((RENDER / "catalog.json").read_text())
    entry = next(entry for entry in catalog["entries"] if entry["kind"] == "uvx")
    target = entry["targets"][0]
    assert target["interpreter_args"] == ["-I"] and ":" in target["entry_point"], target
    merged = json.loads((RENDER / "merged.json").read_text())
    index = next(index for index, entry in enumerate(merged["entries"]) if entry["kind"] == "uvx")
    for name, change, words in (
        ("no-args", lambda t: t.pop("interpreter_args"), "interpreter_args"),
        ("text-args", lambda t: t.update(interpreter_args="-I"), "interpreter_args"),
        ("number-args", lambda t: t.update(interpreter_args=[1]), "interpreter_args"),
        ("no-entry", lambda t: t.pop("entry_point"), "entry_point"),
        ("bare-entry", lambda t: t.update(entry_point="pyagent"), "entry_point"),
        ("empty-module", lambda t: t.update(entry_point=":main"), "entry_point"),
        ("empty-attr", lambda t: t.update(entry_point="pyagent:"), "entry_point"),
    ):
        data = copy.deepcopy(merged)
        change(data["entries"][index]["targets"][0])
        refused(scratch, f"uvx-{name}.json", words, merged=written(scratch, f"m-uvx-{name}.json", data))


NODE_VERSION = "22.15.0"
NODE_DIST = f"https://nodejs.org/dist/v{NODE_VERSION}/"
NODE_NAMES = {
    "linux-x86_64": "linux-x64",
    "linux-aarch64": "linux-arm64",
    "darwin-aarch64": "darwin-arm64",
    "windows-x86_64": "win-x64",
    "windows-aarch64": "win-arm64",
}
# The platforms each npm agent below is laid out on; the others are not.
NODE_BUILT = ("linux-x86_64", "darwin-aarch64", "windows-x86_64")
NPM = "https://registry.npmjs.org/"
TOP = "node_modules/npx-agent"
BUNDLED = "node_modules/npx-agent/node_modules/bundled-dep"
OPTIONAL = "node_modules/@scope/opt-linux"
ALIAS = "node_modules/strip"
GYP = "node_modules/withgyp"
DEV = "node_modules/devonly"
LIBC_GNU = "node_modules/@scope/native-linux-gnu"
LIBC_MUSL = "node_modules/@scope/native-linux-musl"


def gpg(home: pathlib.Path, *args: str, faked: str | None = None) -> bytes:
    """gpg on the throwaway keyring at `home`, at the faked time `faked`."""
    extra = ["--faked-system-time", faked] if faked else []
    return subprocess.run(
        ["gpg", "--homedir", str(home), "--batch", "--no-tty", "--passphrase", "", "--pinentry-mode", "loopback",
         *extra, *args],
        capture_output=True, check=True, timeout=60,
    ).stdout


def signing_key(home: pathlib.Path, name: str, expiry: str = "never") -> str:
    """A throwaway Ed25519 signing key made at the start of 2020, and its fingerprint."""
    home.mkdir(mode=0o700, parents=True)
    gpg(home, "--quick-gen-key", f"{name} <{name}@example.invalid>", "ed25519", "sign", expiry, faked="20200101T000000")
    listing = gpg(home, "--with-colons", "--list-keys").decode()
    return next(line.split(":")[9] for line in listing.splitlines() if line.startswith("fpr:"))


def clearsigned(
    home: pathlib.Path, fpr: str, body: bytes, faked: str | None = None, digest: str | None = None,
) -> bytes:
    (home / "body").write_bytes(body)
    out = home / "body.asc"
    out.unlink(missing_ok=True)
    chosen = ["--digest-algo", digest] if digest else []
    gpg(home, "--local-user", fpr, *chosen, "--clearsign", "--output", str(out), str(home / "body"), faked=faked)
    return out.read_bytes()


class Signers:
    """Four throwaway release keys: one listed, one listed that expired on
    2021-02-01, one listed whose primary expired on 2021-02-01 while its
    signing subkey never does, and one whose public key is vendored but whose
    fingerprint is not listed; and the keys directory the intake is pointed
    at."""

    def __init__(self, scratch: pathlib.Path) -> None:
        self.homes = scratch / "gpg"
        self.good = signing_key(self.homes / "good", "good")
        self.unlisted = signing_key(self.homes / "unlisted", "unlisted")
        expired = self.homes / "expired"
        self.expired = signing_key(expired, "expired", "2022-01-01")
        body = shasums()
        # Signed before the key expired, and after; its expiry is moved
        # earlier only once both are made.
        self.before = clearsigned(expired, self.expired, body, faked="20210115T000000")
        self.after = clearsigned(expired, self.expired, body, faked="20210301T000000")
        gpg(expired, "--quick-set-expire", self.expired, "2021-02-01", faked="20210102T000000")
        # Signed by a subkey after its primary expired.
        parent = self.homes / "parent"
        parent.mkdir(mode=0o700, parents=True)
        gpg(parent, "--quick-gen-key", "parent <parent@example.invalid>", "ed25519", "cert", "never",
            faked="20200101T000000")
        listing = gpg(parent, "--with-colons", "--list-keys").decode()
        self.parent = next(line.split(":")[9] for line in listing.splitlines() if line.startswith("fpr:"))
        gpg(parent, "--quick-add-key", self.parent, "ed25519", "sign", "never", faked="20200101T000000")
        self.orphaned = clearsigned(parent, self.parent, body, faked="20210301T000000")
        gpg(parent, "--quick-set-expire", self.parent, "2021-02-01", faked="20210102T000000")
        # A digest Node's keys must not be trusted under.
        self.sha1 = clearsigned(self.homes / "good", self.good, body, digest="SHA1")
        self.keys = scratch / "node-keys"
        self.keys.mkdir()
        named_keys = (("good", self.good), ("unlisted", self.unlisted), ("expired", self.expired), ("parent", self.parent))
        for name, fpr in named_keys:
            (self.keys / f"{fpr}.asc").write_bytes(gpg(self.homes / name, "--armor", "--export", fpr))
        (self.keys / "fingerprints").write_text(f"# test keys\n{self.good}\n{self.expired}\n{self.parent}\n")

    def signed(self, body: bytes | None = None, by: str = "good") -> bytes:
        fpr = self.good if by == "good" else self.unlisted
        return clearsigned(self.homes / by, fpr, shasums() if body is None else body)

    def close(self) -> None:
        for name in ("good", "unlisted", "expired", "parent"):
            subprocess.run(["gpgconf", "--homedir", str(self.homes / name), "--kill", "gpg-agent"],
                           capture_output=True, check=False, timeout=60)


def tgz(members: dict[str, tuple[bytes, bool]], *, dirs: tuple = (), links: tuple = ()) -> bytes:
    held = io.BytesIO()
    with gzip.GzipFile(fileobj=held, mode="wb", mtime=0) as zipped, tarfile.open(fileobj=zipped, mode="w") as tar:
        for path in dirs:
            info = tarfile.TarInfo(path)
            info.type, info.mode = tarfile.DIRTYPE, 0o755
            tar.addfile(info)
        for path, (content, executable) in members.items():
            info = tarfile.TarInfo(path)
            info.size, info.mode = len(content), 0o755 if executable else 0o644
            tar.addfile(info, io.BytesIO(content))
        for path, target in links:
            info = tarfile.TarInfo(path)
            info.type, info.linkname = tarfile.SYMTYPE, target
            tar.addfile(info)
    return held.getvalue()


def node_archive(platform: str) -> tuple[bytes, dict, str, str]:
    """Node's release archive for `platform`, the unpack-check output of the
    members kept from it, its file name and its binary."""
    top = f"node-v{NODE_VERSION}-{NODE_NAMES[platform]}"
    windows = platform.startswith("windows-")
    program = f"{top}/node.exe" if windows else f"{top}/bin/node"
    kept = {program: (f"node for {platform}\n".encode(), not windows), f"{top}/LICENSE": (b"Node.js is MIT\n", False)}
    members = {**kept, f"{top}/include/node/node.h": (b"// a header the client does not keep\n", False)}
    if windows:
        held = io.BytesIO()
        with zipfile.ZipFile(held, "w") as archive_file:
            for member, (content, _) in members.items():
                archive_file.writestr(zipfile.ZipInfo(member, date_time=(1980, 1, 1, 0, 0, 0)), content)
        return held.getvalue(), listed(kept), f"{top}.zip", program
    return tgz(members, dirs=(top, f"{top}/bin")), listed(kept), f"{top}.tar.gz", program


def shasums() -> bytes:
    lines = [f"{sha(node_archive(platform)[0])}  {node_archive(platform)[2]}" for platform in NODE_BUILT]
    lines.append(f"{'0' * 64}  node-v{NODE_VERSION}.tar.gz")
    return ("\n".join(lines) + "\n").encode()


def lay_out_node(observed: pathlib.Path, signed: bytes) -> None:
    """What the job leaves of Node once per run: the signed checksums and
    each platform's download, with the unpack-check output of what is kept."""
    (observed / "node").mkdir(parents=True)
    (observed / "node/SHASUMS256.txt.asc").write_bytes(signed)
    (observed / "node/fetched.json").write_text(json.dumps({"chain": [NODE_DIST + "SHASUMS256.txt.asc"]}))
    for platform in NODE_BUILT:
        content, listing, file, _ = node_archive(platform)
        place = observed / "node" / platform
        place.mkdir()
        (place / "artifact").write_bytes(content)
        (place / "fetched.json").write_text(json.dumps({"chain": [NODE_DIST + file]}))
        (place / "unpack-check.json").write_text(json.dumps(listing))


def manifest(**fields: object) -> tuple[bytes, bool]:
    return json.dumps(fields).encode(), False


def npm_tree(
    top: dict | None = None, changes: dict | None = None, top_files: dict | None = None, extra: dict | None = None,
) -> dict:
    """An agent's npm tree: npm's lock, each package's tarball by its place in
    the lock, and the files each package writes. `top` replaces the agent's
    own package.json, `top_files` adds to its files, `changes` to a
    location's lock fields (a field changed to None is left out), and `extra`
    to its packages."""
    top = top if top is not None else top_manifest()
    own = {
        "package.json": (json.dumps(top).encode(), False),
        "bin/agent.js": (b"#!/usr/bin/env node\nrequire('../lib/run.js');\n", False),
        "lib/run.js": (b"module.exports = 1;\n", False),
        ".gitignore": (b"node_modules\n", False),
        "node_modules/bundled-dep/package.json": manifest(
            name="bundled-dep", version="1.0.0", license="ISC", bin={"bundled": "./index.js"}),
        "node_modules/bundled-dep/index.js": (b"// bundled\n", False),
        **(top_files or {}),
    }
    # location: (npm's lock fields, the files its tarball writes, or None)
    packages: dict[str, tuple[dict, dict | None]] = {
        OPTIONAL: (
            {"version": "1.0.0", "resolved": NPM + "@scope/opt-linux/-/opt-linux-1.0.0.tgz", "optional": True, "os": ["linux"]},
            {"package.json": manifest(name="@scope/opt-linux", version="1.0.0"), "opt.node": (b"\x7fELF native\n", True)},
        ),
        DEV: ({"version": "0.1.0", "resolved": NPM + "devonly/-/devonly-0.1.0.tgz", "dev": True}, None),
        TOP: (
            {"version": "3.0.0", "resolved": NPM + "npx-agent/-/npx-agent-3.0.0.tgz", "hasInstallScript": True,
             "bin": {"npx-agent": "bin/agent.js"}, "license": "MIT", "dependencies": {"strip": "npm:strip-ansi@7.1.0"}},
            own,
        ),
        BUNDLED: ({"version": "1.0.0", "inBundle": True}, None),
        ALIAS: (
            {"name": "strip-ansi", "version": "7.1.0", "resolved": NPM + "strip-ansi/-/strip-ansi-7.1.0.tgz"},
            {"package.json": manifest(name="strip-ansi", version="7.1.0", license="MIT"),
             "index.js": (b"module.exports = s => s;\n", False)},
        ),
        GYP: (
            {"version": "2.0.0", "resolved": NPM + "withgyp/-/withgyp-2.0.0.tgz", "hasInstallScript": True},
            {"package.json": manifest(name="withgyp", version="2.0.0", scripts={"test": "x"},
                                      license={"type": "ISC"}),
             "binding.gyp": (b"{\"targets\": []}\n", False), "src/addon.c": (b"int addon;\n", False)},
        ),
        **(extra or {}),
    }
    lock_packages: dict = {"": {"name": "root", "version": "1.0.0", "dependencies": {"npx-agent": "3.0.0"}}}
    tarballs: dict[int, bytes] = {}
    for index, (location, (fields, files)) in enumerate(sorted(packages.items()), start=1):
        fields = dict(fields)
        if files is not None:
            tarball = tgz({f"package/{path}": member for path, member in files.items()},
                          dirs=("package", "package/lib"), links=(("package/lib/link.js", "run.js"),))
            tarballs[index] = tarball
            if location == ALIAS:
                fields["integrity"] = "sha1-" + base64.b64encode(hashlib.sha1(tarball).digest()).decode()
            else:
                fields["integrity"] = "sha512-" + base64.b64encode(hashlib.sha512(tarball).digest()).decode()
        fields.update((changes or {}).get(location, {}))
        lock_packages[location] = {field: value for field, value in fields.items() if value is not None}
    return {
        "lock": {"name": "root", "version": "1.0.0", "lockfileVersion": 3, "requires": True, "packages": lock_packages},
        "tarballs": tarballs,
        "files": {location: files for location, (_, files) in packages.items() if files is not None},
    }


def installed_on(tree: dict, platform: str) -> list[str]:
    """The locations `npm ci` installs on `platform`, in the lock's order: in
    these trees an `os` or `libc` lists only what is admitted, and a runner
    on Linux is glibc."""
    linux = platform.startswith("linux-")
    return [
        location for location, fields in tree["lock"]["packages"].items()
        if location and not fields.get("dev")
        and ("os" not in fields or platform.partition("-")[0] in fields["os"])
        and ("libc" not in fields or (linux and "glibc" in fields["libc"]))
    ]


def npm_listing(tree: dict, platform: str) -> dict:
    """What `tree` lists of `npm ci`'s output on `platform`: each package's
    files, what its package.json's bin names made runnable and `.gitignore`
    its_files as `.npmignore`, beside npm's own files and the project's."""
    windows = platform.startswith("windows-")
    files = [
        {"exec": False, "path": "package.json", "sha256": sha(b"{}"), "size": 2},
        {"exec": False, "path": "package-lock.json", "sha256": sha(b"{}"), "size": 2},
        {"exec": False, "path": "node_modules/.package-lock.json", "sha256": sha(b"{}"), "size": 2},
        {"exec": not windows, "path": "node_modules/.bin/npx-agent", "sha256": sha(b"#!"), "size": 2},
    ]
    for location in installed_on(tree, platform):
        its_files = tree["files"].get(location, {})
        declared = json.loads(its_files["package.json"][0]) if "package.json" in its_files else {}
        given = declared.get("bin")
        under = declared.get("directories", {}).get("bin") if given is None else None
        bins = {given} if isinstance(given, str) else set((given or {}).values())
        bins |= {path for path in its_files if under is not None and path.startswith(under + "/")}
        for path, (content, executable) in its_files.items():
            name = path[: -len(".gitignore")] + ".npmignore" if path.rpartition("/")[2] == ".gitignore" else path
            runnable = (executable or path in bins) and not windows
            files.append({"exec": runnable, "path": f"{location}/{name}", "sha256": sha(content), "size": len(content)})
    return {"files": files}


def built_listing(tree: dict, platform: str) -> dict:
    """What `crucible agents unpack-check` prints for the lock on `platform`."""
    return {"files": [
        {"exec": executable, "kind": "file", "path": f"{location}/{path}", "sha256": sha(content), "size": len(content)}
        for location in installed_on(tree, platform)
        for path, (content, executable) in tree["files"].get(location, {}).items()
    ]}


def expected_lock(tree: dict, platform: str) -> dict:
    """The lock asset for `platform`, worked out from the files each tarball
    was made of rather than read back from it."""
    places = {location: index for index, location in enumerate(tree["lock"]["packages"])}
    packages = []
    for location in installed_on(tree, platform):
        fields = tree["lock"]["packages"][location]
        name = fields.get("name", location.removeprefix("node_modules/").rpartition("node_modules/")[2])
        package: dict = {"location": location, "name": name, "version": fields["version"]}
        package.update(stated(tree, location, name))
        if fields.get("inBundle"):
            package["in_bundle"] = True
        else:
            tarball = tree["tarballs"][places[location]]
            files = tree["files"][location]
            package.update(
                url=fields["resolved"], size=len(tarball), sha512=hashlib.sha512(tarball).hexdigest(),
                files=len(files), unpacked=sum(len(content) for content, _ in files.values()),
            )
        packages.append(package)
    return {"packages": packages, "unpack_rules": 1}


def stated(tree: dict, location: str, name: str) -> dict:
    """The `license` and `bin` the lock gives the package at `location`, from
    the package.json among the files it was made of, its holder's for a
    bundled package: a licence only where it is text, and the programs a
    `bin` string, a `bin` object or the files under `directories.bin` give,
    each script without its leading `./`; neither where it gives none."""
    holder = location if location in tree["files"] else next(
        held for held in tree["files"] if location.startswith(held + "/"))
    under = location[len(holder) + 1:]
    files = {path[len(under) + 1:] if under else path: member for path, member in tree["files"][holder].items()
             if not under or path.startswith(under + "/")}
    declared = json.loads(files["package.json"][0])
    held: dict = {}
    if isinstance(declared.get("license"), str):
        held["license"] = declared["license"]
    given = declared.get("bin")
    if isinstance(given, str):
        given = {name.rpartition("/")[2]: given}
    elif given is None and "bin" in declared.get("directories", {}):
        directory = declared["directories"]["bin"]
        given = {path.rpartition("/")[2]: path for path in files if path.startswith(directory + "/")}
    if given:
        held["bin"] = {program: script.removeprefix("./") for program, script in given.items()}
    return held


def lock_bytes(lock: dict) -> bytes:
    return json.dumps(lock, sort_keys=True, separators=(",", ":")).encode()


def lay_out_npm(
    observed: pathlib.Path,
    agent: str,
    tree: dict,
    *,
    chains: dict | None = None,
    npm_side=None,
    built_side=None,
    platforms: tuple = NODE_BUILT,
) -> None:
    """What the job leaves for one npm agent: npm's lock, each tarball, and on
    each of `platforms` the two listings of its tree, crucible's lock and the
    probe. `npm_side` and `built_side` change a platform's listing."""
    place = observed / agent
    (place / "resolve").mkdir(parents=True)
    (place / "resolve/package-lock.json").write_text(json.dumps(tree["lock"]))
    locations = list(tree["lock"]["packages"])
    for index, tarball in tree["tarballs"].items():
        held = place / "tarballs" / str(index)
        held.mkdir(parents=True)
        (held / "artifact").write_bytes(tarball)
        url = tree["lock"]["packages"][locations[index]].get("resolved")
        chain = (chains or {}).get(locations[index], [url])
        (held / "fetched.json").write_text(json.dumps({"chain": chain}))
    for platform in platforms:
        made = place / platform / "tree"
        made.mkdir(parents=True)
        npm = npm_listing(tree, platform)
        built = built_listing(tree, platform)
        if npm_side is not None:
            npm_side(platform, npm)
        if built_side is not None:
            built_side(platform, built)
        (made / "npm.json").write_text(json.dumps(npm))
        (made / "unpack-check.json").write_text(json.dumps(built))
        (made / "lock.json").write_bytes(lock_bytes(expected_lock(tree, platform)))
        (place / platform / "probe.json").write_text(json.dumps(PROBED))


def alias_with(**changed: object) -> tuple[dict, dict]:
    """The aliased package of the default tree, its package.json given
    `changed` too."""
    return (
        {"name": "strip-ansi", "version": "7.1.0", "resolved": NPM + "strip-ansi/-/strip-ansi-7.1.0.tgz"},
        {"package.json": manifest(name="strip-ansi", version="7.1.0", license="MIT", **changed),
         "index.js": (b"module.exports = s => s;\n", False)},
    )


def top_manifest(**changed: object) -> dict:
    """The agent's own package.json, with `changed` in place of its fields; a
    field changed to None is left out."""
    held = {
        "name": "npx-agent", "version": "3.0.0", "bin": {"npx-agent": "bin/agent.js"}, "license": "Apache-2.0",
        "scripts": {"postinstall": "node setup.js", "test": "node test.js"},
    }
    held.update(changed)
    return {field: value for field, value in held.items() if value is not None}


def npm_case(
    observed: pathlib.Path,
    agents: list[dict],
    choices: dict,
    agent_id: str,
    tree: dict | None = None,
    package: str = "npx-agent@3.0.0",
    **kept: object,
) -> dict | None:
    """One npm agent in the registry, and what the job left for it."""
    spec: dict = {"package": package, "args": ["--acp"]}
    if agent_id == "npx-agent":
        spec["env"] = {"NPX_QUIET": "1"}
    agents.append({"id": agent_id, "name": agent_id, "version": "3.0.0", "license": "MIT", "distribution": {"npx": spec}})
    choices[agent_id] = {"key": "nodeagent" if agent_id == "npx-agent" else agent_id.removesuffix("-npm")}
    if tree is not None:
        lay_out_npm(observed, agent_id, tree, **kept)
    return tree


def on(platforms: tuple, change):
    """A listing change made on `platforms` alone."""
    def side(platform: str, listing: dict) -> None:
        if platform in platforms:
            change(listing["files"])
    return side


def file_of(files: list[dict], path: str) -> dict:
    return next(item for item in files if item["path"] == path)


def npm_registry(scratch: pathlib.Path, signers: Signers) -> tuple[pathlib.Path, pathlib.Path, dict]:
    """A registry of npm agents, the main one and each case, with what the job
    left for them; returns the registry, the choices and the main tree."""
    observed = scratch / "observed"
    lay_out_node(observed, signers.signed())
    agents: list[dict] = []
    choices: dict = {}
    main = npm_case(observed, agents, choices, "npx-agent", npm_tree())
    for agent_id, package in (
        ("range-npm", "npx-agent@^3.0.0"),
        ("tag-npm", "npx-agent@latest"),
        ("alias-npm", "npx-agent@npm:other@3.0.0"),
        ("upper-npm", "Npx-Agent@3.0.0"),
        ("bare-npm", "npx-agent"),
    ):
        npm_case(observed, agents, choices, agent_id, package=package)
    npm_case(observed, agents, choices, "notop-npm", npm_tree(), package="npx-agent@3.0.1")
    wrong_sha1 = "sha1-" + base64.b64encode(b"\0" * 20).decode()
    wrong_sha512 = "sha512-" + base64.b64encode(b"\0" * 64).decode()
    npm_case(observed, agents, choices, "sha1bad-npm", npm_tree(changes={ALIAS: {"integrity": wrong_sha1}}))
    npm_case(observed, agents, choices, "sha512bad-npm", npm_tree(changes={TOP: {"integrity": wrong_sha512}}))
    # A SHA-512 that agrees is held; a SHA-1 beside it is not.
    tree = npm_tree()
    held = tree["lock"]["packages"][TOP]["integrity"]
    npm_case(observed, agents, choices, "both-npm", npm_tree(changes={TOP: {"integrity": f"{held} {wrong_sha1}"}}))
    npm_case(observed, agents, choices, "offreg-npm", npm_tree(changes={ALIAS: {"resolved": "https://registry.example/strip-ansi/-/strip-ansi-7.1.0.tgz"}}))
    npm_case(observed, agents, choices, "query-npm", npm_tree(changes={ALIAS: {"resolved": NPM + "strip-ansi/-/strip-ansi-7.1.0.tgz?x=1"}}))
    # An address with a space, or a letter outside ASCII, which a client
    # would have to encode to fetch.
    for agent_id, name in (("spaced-npm", "strip ansi"), ("accent-npm", "strip-ans\u00ed")):
        npm_case(observed, agents, choices, agent_id, npm_tree(changes={ALIAS: {"resolved": NPM + f"strip-ansi/-/{name}-7.1.0.tgz"}}))
    # A lock that pins no tarball, or names a package other than its tarball.
    npm_case(observed, agents, choices, "unpinned-npm", npm_tree(changes={ALIAS: {"integrity": None}}))
    npm_case(observed, agents, choices, "otherversion-npm", npm_tree(changes={ALIAS: {"version": "7.1.1"}}))
    npm_case(observed, agents, choices, "othername-npm", npm_tree(changes={ALIAS: {"name": "other-ansi"}}))
    npm_case(observed, agents, choices, "otherbundled-npm", npm_tree(changes={BUNDLED: {"version": "1.0.1"}}))
    # The registry's address in capitals: its host is the registry's, its
    # prefix is not.
    npm_case(observed, agents, choices, "capital-npm", npm_tree(changes={ALIAS: {"resolved": "HTTPS://registry.npmjs.org/strip-ansi/-/strip-ansi-7.1.0.tgz"}}))
    npm_case(
        observed, agents, choices, "hop-npm", npm_tree(),
        chains={ALIAS: [NPM + "strip-ansi/-/strip-ansi-7.1.0.tgz", "https://cdn.example/strip-ansi.tgz"]},
    )
    npm_case(observed, agents, choices, "scriptless-npm", npm_tree(changes={GYP: {"hasInstallScript": False}}))
    npm_case(observed, agents, choices, "scripted-npm", npm_tree(changes={ALIAS: {"hasInstallScript": True}}))
    npm_case(observed, agents, choices, "place-npm", npm_tree(changes={ALIAS: {"inBundle": True}}))
    # A glibc and a musl build of one optional package: Linux takes the first.
    npm_case(observed, agents, choices, "libc-npm", npm_tree(extra={
        location: (
            {"version": "1.0.0", "resolved": NPM + f"@scope/{name}/-/{name}-1.0.0.tgz", "optional": True,
             "os": ["linux"], "libc": [libc]},
            {"package.json": manifest(name=f"@scope/{name}", version="1.0.0"), "native.node": (libc.encode(), True)},
        )
        for location, name, libc in ((LIBC_GNU, "native-linux-gnu", "glibc"), (LIBC_MUSL, "native-linux-musl", "musl"))
    }))
    # The tree npm built differs from crucible's, on Linux alone.
    linux = ("linux-x86_64",)
    run_js = TOP + "/lib/run.js"
    npm_case(observed, agents, choices, "missing-npm", npm_tree(),
             npm_side=on(linux, lambda files: files.remove(file_of(files, ALIAS + "/index.js"))))
    npm_case(observed, agents, choices, "hash-npm", npm_tree(),
             npm_side=on(linux, lambda files: file_of(files, run_js).update(sha256="e" * 64)))
    # On Windows nothing is runnable, so an exec flag is not compared there.
    npm_case(observed, agents, choices, "exec-npm", npm_tree(),
             npm_side=on(("linux-x86_64", "windows-x86_64"), lambda files: file_of(files, run_js).update(exec=True)))
    npm_case(observed, agents, choices, "extra-npm", npm_tree(),
             npm_side=on(linux, lambda files: files.append(
                 {"exec": False, "path": "node_modules/stray/index.js", "sha256": "e" * 64, "size": 1})))
    npm_case(observed, agents, choices, "npmignore-npm", npm_tree(),
             npm_side=on(linux, lambda files: file_of(files, TOP + "/.npmignore").update(sha256="e" * 64)))
    npm_case(observed, agents, choices, "lockdiff-npm", npm_tree())
    # One file listed twice, exactly or once case is folded.
    npm_case(observed, agents, choices, "twice-npm", npm_tree(),
             npm_side=on(linux, lambda files: files.append(dict(file_of(files, run_js)))))
    folded_js = {"exec": False, "path": TOP + "/LIB/run.js", "sha256": sha(b"module.exports = 1;\n"), "size": 20}
    npm_case(observed, agents, choices, "folded-npm", npm_tree(),
             npm_side=on(linux, lambda files: files.append(dict(folded_js))),
             built_side=on(linux, lambda files: files.append(dict(folded_js, kind="file"))))
    # npm makes runnable only what a package.json's bin names, and never
    # takes a mode away.
    agent_js = TOP + "/bin/agent.js"
    npm_case(observed, agents, choices, "unrun-npm", npm_tree(),
             npm_side=on(linux, lambda files: file_of(files, agent_js).update(exec=False)),
             built_side=on(linux, lambda files: file_of(files, agent_js).update(exec=True)))
    # A bin a package.json names in another package's files, here the one it
    # bundles, which npm makes runnable and the client does not link.
    npm_case(observed, agents, choices, "ownerbin-npm",
             npm_tree(top=top_manifest(bin={"npx-agent": "bin/agent.js", "other": "node_modules/bundled-dep/index.js"})))
    # A bin the lock names and the package.json does not.
    npm_case(observed, agents, choices, "lockbin-npm", npm_tree(changes={TOP: {"bin": {"npx-agent": "bin/agent.js", "run": "lib/run.js"}}}),
             npm_side=on(("linux-x86_64", "darwin-aarch64"), lambda files: file_of(files, run_js).update(exec=True)))
    (observed / "lockdiff-npm/linux-x86_64/tree/lock.json").write_bytes(b'{"packages":[],"unpack_rules":1}')
    # The bin npx would run.
    for agent_id, top in (
        ("binstr-npm", top_manifest(bin="bin/agent.js")),
        ("binmany-npm", top_manifest(bin={"other": "lib/run.js", "npx-agent": "bin/agent.js"})),
        ("binnone-npm", top_manifest(bin={"a": "lib/run.js", "b": "bin/agent.js"})),
        ("bindir-npm", top_manifest(bin=None, directories={"bin": "bin"})),
        ("binmissing-npm", top_manifest(bin={"npx-agent": "bin/missing.js"})),
    ):
        npm_case(observed, agents, choices, agent_id, npm_tree(top=top))
    # A licence the client would refuse, and programs it would: two whose
    # names differ only in case, one name at the top of the tree twice, one
    # declared by two packages below the top and none at it, and one whose
    # name is no file name. A licence that is not text is left out instead.
    npm_case(observed, agents, choices, "licence-npm", npm_tree(top=top_manifest(license="MIT\u00e9")))
    npm_case(observed, agents, choices, "longlicence-npm", npm_tree(top=top_manifest(license="M" * 257)))
    npm_case(observed, agents, choices, "binfold-npm", npm_tree(
        top=top_manifest(bin={"npx-agent": "bin/agent.js", "Strip": "lib/run.js"}),
        extra={ALIAS: alias_with(bin={"strip": "index.js"})}))
    npm_case(observed, agents, choices, "bintop-npm", npm_tree(extra={ALIAS: alias_with(bin={"npx-agent": "index.js"})}))
    inner = ALIAS + "/node_modules/inner"
    npm_case(observed, agents, choices, "bincontested-npm", npm_tree(extra={inner: (
        {"version": "1.0.0", "resolved": NPM + "inner/-/inner-1.0.0.tgz"},
        {"package.json": manifest(name="inner", version="1.0.0", bin={"bundled": "i.js"}), "i.js": (b"1\n", False)},
    )}))
    npm_case(observed, agents, choices, "binname-npm", npm_tree(top=top_manifest(bin={"npx-agent": "bin/agent.js", "a:b": "lib/run.js"})))
    npm_case(observed, agents, choices, "binnode-npm", npm_tree(top=top_manifest(bin={"npx-agent": "bin/agent.js", "NODE": "bin/agent.js"})))
    # The program below the top of the tree that one at its top settles.
    npm_case(observed, agents, choices, "binsettled-npm", npm_tree(extra={inner: (
        {"version": "1.0.0", "resolved": NPM + "inner/-/inner-1.0.0.tgz"},
        {"package.json": manifest(name="inner", version="1.0.0", bin={"strip": "i.js"}), "i.js": (b"1\n", False)},
    ), ALIAS: alias_with(bin={"strip": "index.js"})}))
    # Registry ids the job's own directories would collide with, as a file
    # system that folds case names them: Node's, and each other's.
    for agent_id, key in (("node", "node"), ("Node", "upper-node"), ("fold-npm", "fold"), ("FOLD-npm", "fold-two")):
        npm_case(observed, agents, choices, agent_id)
        choices[agent_id] = {"key": key}
    registry = written(scratch, "npm-registry.json", {"version": "1.0.0", "agents": agents})
    assert main is not None
    return registry, written(scratch, "npm-choices.json", choices), main


def npm_review(
    scratch: pathlib.Path,
    signers: Signers,
    registry: pathlib.Path,
    choices: pathlib.Path,
    *extra: str | pathlib.Path,
    name: str = "review.json",
    node: str = "22.15.0",
) -> tuple[dict, pathlib.Path]:
    pin = written(scratch, f"node-{name}", {"version": node})
    return review(scratch, "--node", pin, "--node-keys", signers.keys, *extra, name=name, registry=registry, choices=choices)


def decided(scratch: pathlib.Path, value: object, name: str = "dispositions.json") -> pathlib.Path:
    return written(scratch, name, value)


def check_npm(scratch: pathlib.Path, signers: Signers) -> None:
    """An npm agent is pinned per target as Node and the lock of the tree npm
    installs there, once that tree is the one crucible builds from the lock;
    whatever cannot be is dropped, naming why."""
    scratch = scratch / "npm"
    scratch.mkdir()
    registry, choices, tree = npm_registry(scratch, signers)
    dispositions = decided(scratch, {"npx-agent": {f"{TOP} postinstall": "inert"}})
    data, assets = npm_review(scratch, signers, registry, choices, "--dispositions", dispositions)
    entries = {entry["key"]: entry for entry in data["entries"]}
    assert sorted(entries) == [
        "bindir", "binmany", "binsettled", "binstr", "both", "exec", "extra", "folded", "hash", "libc", "lockbin",
        "lockdiff", "missing", "nodeagent", "npmignore", "twice", "unrun",
    ], sorted(entries)

    entry = entries["nodeagent"]
    assert entry["kind"] == "npm" and entry["registry_id"] == "npx-agent", entry["kind"]
    assert entry["lock_revision"] == 1, entry.get("lock_revision")
    targets = {target["platform"]: target for target in entry["targets"]}
    assert sorted(targets) == sorted(NODE_BUILT), sorted(targets)
    for platform, target in targets.items():
        content, listing, file, program = node_archive(platform)
        assert target["cmd"] == program, target["cmd"]
        assert target["node_args"] == [] and target["args"] == ["--acp"], target
        assert target["env"] == {"NPX_QUIET": "1"}, target["env"]
        assert target["bin"] == TOP + "/bin/agent.js", target["bin"]
        [node] = target["artifacts"]
        assert node["url"] == NODE_DIST + file and node["sha256"] == sha(content), node["url"]
        assert node["format"] == ("zip" if platform.startswith("windows-") else "tar.gz"), node["format"]
        assert node["kept"] == [program, program.split("/")[0] + "/LICENSE"], node["kept"]
        assert node["files"] == listing["files"], "Node's kept members are listed"
        assert "mirror_licence" not in node, "Node is never mirrored"
        assert target["hosts"] == ["nodejs.org", "registry.npmjs.org"], target["hosts"]
        lock = lock_bytes(expected_lock(tree, platform))
        assert target["lock"] == {"sha256": sha(lock), "size": len(lock), "unpack_rules": 1}, "the lock is pinned by hash"
        assert (assets / sha(lock)).read_bytes() == lock, "the lock asset is the lock the tree was built from"
        assert target["probe"]["sandbox"] == "closed", target["probe"]
    # An alias is locked under its own name where it is installed, a bundled
    # package as part of the tarball that holds it, an optional one only where
    # it fits, and a dev one nowhere: worked out here, not read back.
    linux = json.loads((assets / targets["linux-x86_64"]["lock"]["sha256"]).read_bytes())
    alias_tarball = tree["tarballs"][5]
    assert {"location": ALIAS, "name": "strip-ansi", "version": "7.1.0", "url": NPM + "strip-ansi/-/strip-ansi-7.1.0.tgz",
            "size": len(alias_tarball), "sha512": hashlib.sha512(alias_tarball).hexdigest(), "license": "MIT",
            "files": 2, "unpacked": len(manifest(name="strip-ansi", version="7.1.0", license="MIT")[0])
            + len(b"module.exports = s => s;\n")
            } in linux["packages"], linux["packages"]
    assert {"location": BUNDLED, "name": "bundled-dep", "version": "1.0.0", "in_bundle": True, "license": "ISC",
            "bin": {"bundled": "index.js"}} in linux["packages"], linux
    # Each package's licence as its package.json states it, where it is text,
    # and the programs it declares.
    held = {package["location"]: package for package in linux["packages"]}
    assert held[TOP]["license"] == "Apache-2.0" and held[TOP]["bin"] == {"npx-agent": "bin/agent.js"}, held[TOP]
    assert held[ALIAS]["license"] == "MIT" and "bin" not in held[ALIAS], held[ALIAS]
    assert "license" not in held[GYP] and "license" not in held[OPTIONAL], "a licence that is not text is not written"
    own = next(package for package in linux["packages"] if package["location"] == TOP)
    assert own["files"] == 6, "a bundled package's files are counted in the tarball that holds it"
    assert [package["location"] for package in linux["packages"]] == [OPTIONAL, TOP, BUNDLED, ALIAS, GYP], linux
    assert targets["darwin-aarch64"]["lock"] == targets["windows-x86_64"]["lock"], "one lock is shared by hash"
    assert targets["darwin-aarch64"]["lock"] != targets["linux-x86_64"]["lock"], "an optional package that fits differs"
    # Each script --ignore-scripts skipped carries its disposition, or none.
    assert targets["linux-x86_64"]["scripts"] == [
        {"disposition": "inert", "event": "postinstall", "package": TOP},
        {"disposition": None, "event": "install", "package": GYP},
    ], targets["linux-x86_64"]["scripts"]
    assert {"event": "install", "package": GYP, "registry_id": "npx-agent"} in data["undecided"], data["undecided"]
    assert {"event": "postinstall", "package": TOP, "registry_id": "npx-agent"} not in data["undecided"], data["undecided"]

    dropped(data, "npx-agent", "freebsd-x86_64", "Node publishes no build")
    dropped(data, "npx-agent", "linux-aarch64", "Node 22.15.0", "nothing was downloaded")
    for agent_id in ("range-npm", "tag-npm", "alias-npm", "upper-npm", "bare-npm"):
        dropped(data, agent_id, None, "exact version")
    dropped(data, "notop-npm", None, "installs no npx-agent 3.0.1")
    dropped(data, "sha1bad-npm", None, ALIAS, "integrity")
    dropped(data, "sha512bad-npm", None, TOP, "integrity")
    dropped(data, "offreg-npm", None, "registry.example", "https://registry.npmjs.org/")
    dropped(data, "query-npm", None, "not a plain address")
    for agent_id in ("spaced-npm", "accent-npm"):
        dropped(data, agent_id, None, "printable ASCII")
    dropped(data, "capital-npm", None, "https://registry.npmjs.org/, the one place")
    dropped(data, "unpinned-npm", None, ALIAS, "no SHA-512 or SHA-1 integrity")
    for agent_id, location in (("otherversion-npm", ALIAS), ("othername-npm", ALIAS), ("otherbundled-npm", BUNDLED)):
        dropped(data, agent_id, None, location, "its package.json names")
    dropped(data, "hop-npm", None, "cdn.example", "registry.npmjs.org")
    dropped(data, "scriptless-npm", None, GYP, "no install script", "install")
    dropped(data, "scripted-npm", None, ALIAS, "has an install script")
    dropped(data, "place-npm", None, ALIAS, "bundled in no package")
    libc_targets = {target["platform"]: target for target in entries["libc"]["targets"]}
    assert sorted(libc_targets) == sorted(NODE_BUILT), [d for d in data["dropped"] if d["registry_id"] == "libc-npm"]
    libc_linux = json.loads((assets / libc_targets["linux-x86_64"]["lock"]["sha256"]).read_bytes())["packages"]
    libc_held = {package["location"] for package in libc_linux}
    assert LIBC_GNU in libc_held and LIBC_MUSL not in libc_held, "Linux installs the glibc build alone"
    for platform in ("darwin-aarch64", "windows-x86_64"):
        held = json.loads((assets / libc_targets[platform]["lock"]["sha256"]).read_bytes())["packages"]
        assert not {LIBC_GNU, LIBC_MUSL} & {package["location"] for package in held}, platform
    dropped(data, "missing-npm", "linux-x86_64", "npm's tree lacks", ALIAS + "/index.js")
    dropped(data, "hash-npm", "linux-x86_64", run_js := TOP + "/lib/run.js", "differs")
    dropped(data, "exec-npm", "linux-x86_64", run_js, "runnable")
    assert sorted(target["platform"] for target in entries["exec"]["targets"]) == ["darwin-aarch64", "windows-x86_64"]
    dropped(data, "extra-npm", "linux-x86_64", "node_modules/stray/index.js", "under no package")
    dropped(data, "npmignore-npm", "linux-x86_64", TOP + "/.gitignore", "differs")
    dropped(data, "lockdiff-npm", "linux-x86_64", "tree/lock.json")
    dropped(data, "twice-npm", "linux-x86_64", run_js, "listed twice")
    dropped(data, "folded-npm", "linux-x86_64", TOP + "/LIB/run.js", "listed twice")
    dropped(data, "unrun-npm", "linux-x86_64", TOP + "/bin/agent.js", "runnable")
    for platform in ("linux-x86_64", "darwin-aarch64"):
        dropped(data, "lockbin-npm", platform, run_js, "runnable")
    # A program whose script lies in the package it bundles is one the
    # client refuses, wherever it is installed.
    for platform in NODE_BUILT:
        dropped(data, "ownerbin-npm", platform, TOP, "other", "another package's location")
    assert [target["platform"] for target in entries["lockbin"]["targets"]] == ["windows-x86_64"], "lockbin"
    for key in ("missing", "hash", "extra", "npmignore", "lockdiff", "twice", "folded", "unrun"):
        assert sorted(t["platform"] for t in entries[key]["targets"]) == ["darwin-aarch64", "windows-x86_64"], key
    for key in ("binstr", "binmany", "bindir"):
        assert {t["bin"] for t in entries[key]["targets"]} == {TOP + "/bin/agent.js"}, (key, entries[key]["targets"])
    # A bin under directories.bin is each file there, named as the file is.
    bindir = json.loads((assets / entries["bindir"]["targets"][0]["lock"]["sha256"]).read_bytes())["packages"]
    assert next(p for p in bindir if p["location"] == TOP)["bin"] == {"agent.js": "bin/agent.js"}, bindir
    settled = json.loads((assets / entries["binsettled"]["targets"][0]["lock"]["sha256"]).read_bytes())["packages"]
    assert {p["location"]: p.get("bin") for p in settled}[ALIAS + "/node_modules/inner"] == {"strip": "i.js"}, settled
    for platform in NODE_BUILT:
        dropped(data, "licence-npm", platform, TOP, "licence", "printable ASCII")
        dropped(data, "longlicence-npm", platform, TOP, "licence", "256")
        dropped(data, "binfold-npm", platform, ALIAS, TOP, "differs only in case")
        dropped(data, "bintop-npm", platform, "npx-agent", "top of the tree")
        dropped(data, "bincontested-npm", platform, "bundled", "none at the top")
        dropped(data, "binname-npm", platform, "a:b", "not one file name")
        dropped(data, "binnode-npm", platform, "NODE", "pinned Node")
    dropped(data, "binnone-npm", "linux-x86_64", "2 bins", "none npx-agent")
    for platform in NODE_BUILT:
        dropped(data, "binmissing-npm", platform, TOP + "/bin/missing.js", "not a file in the tree")
    dropped(data, "node", None, "node")
    dropped(data, "Node", None, "observed directory")
    for agent_id in ("fold-npm", "FOLD-npm"):
        dropped(data, agent_id, None, "observed directory")

    # The diff marks a script with no disposition, and the whole start.
    ran = intake("diff", scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    lines = ran.stdout.splitlines()
    assert any("**undecided**" in line and GYP in line and "install" in line for line in lines), ran.stdout
    assert any("bin" in line and TOP + "/bin/agent.js" in line for line in lines), ran.stdout
    assert any("lock revision" in line and "1" in line for line in lines), ran.stdout

    check_npm_revision(scratch, signers, registry, choices, dispositions, data)
    check_npm_recheck(scratch, data, assets)
    check_npm_publish(scratch, data, assets)
    check_npm_lock_and_tree(scratch, tree)


def check_npm_revision(
    scratch: pathlib.Path, signers: Signers, registry: pathlib.Path, choices: pathlib.Path,
    dispositions: pathlib.Path, data: dict,
) -> None:
    """A version reviewed before keeps its lock revision while every lock is
    the same, and takes the next one when any lock changed."""
    held = written(scratch, "npm-merged.json", data)
    same, _ = npm_review(scratch, signers, registry, choices, "--dispositions", dispositions, "--merged", held, name="same.json")
    [entry] = [entry for entry in same["entries"] if entry["key"] == "nodeagent"]
    assert entry["lock_revision"] == 1, entry["lock_revision"]
    old = copy.deepcopy(data)
    [listed] = [entry for entry in old["entries"] if entry["key"] == "nodeagent"]
    listed["lock_revision"] = 4
    listed["targets"][0]["lock"]["sha256"] = "d" * 64
    changed = written(scratch, "npm-merged-changed.json", old)
    bumped, _ = npm_review(scratch, signers, registry, choices, "--dispositions", dispositions, "--merged", changed, name="bumped.json")
    [entry] = [entry for entry in bumped["entries"] if entry["key"] == "nodeagent"]
    assert entry["lock_revision"] == 5, entry["lock_revision"]
    ran = intake("diff", "--old", changed, scratch / "bumped.json")
    assert ran.returncode == 0, ran.stderr
    assert any("new lock revision" in line and "5" in line for line in ran.stdout.splitlines()), ran.stdout
    # Under the same locks, any other difference is a changed release, which
    # is taken in only under a new version; a lock that changed beside it
    # takes in nothing more.
    for name, change in (
        ("args", lambda entry: entry["targets"][0].update(args=["--other"])),
        ("lock-and-env", lambda entry: (entry["targets"][0]["lock"].update(sha256="d" * 64),
                                        entry["targets"][0].update(env={}))),
        ("target-gone", lambda entry: entry["targets"].pop()),
    ):
        old = copy.deepcopy(data)
        [listed] = [entry for entry in old["entries"] if entry["key"] == "nodeagent"]
        change(listed)
        held = written(scratch, f"npm-merged-{name}.json", old)
        moved, _ = npm_review(scratch, signers, registry, choices, "--dispositions", dispositions, "--merged", held,
                              name=f"moved-{name}.json")
        assert "nodeagent" not in [entry["key"] for entry in moved["entries"]], f"{name} was taken in"
        dropped(moved, "npx-agent", None, "already reviewed with other content", "a changed release")


def check_npm_recheck(scratch: pathlib.Path, data: dict, assets: pathlib.Path) -> None:
    """The daily re-check lists each tarball once by hash, and proposes a
    revocation only for a tarball the registry no longer serves."""
    merged = written(scratch, "recheck-merged.json", data)
    out = scratch / "recheck.json"
    ran = intake("recheck", "--merged", merged, "--assets", assets, "--out", out)
    assert ran.returncode == 0, ran.stderr
    listed = json.loads(out.read_text())["tarballs"]
    hashes = [item["sha512"] for item in listed]
    assert len(hashes) == len(set(hashes)), "a tarball is listed once"
    alias = next(item for item in listed if item["url"].endswith("strip-ansi-7.1.0.tgz"))
    # Every case kept shares the main agent's tarballs but those whose own
    # package differs, and adds those it adds: four of the main tree, one of
    # each kept bin case's own package, one of the glibc build, and the
    # settled program's case's aliased package and the one it nests.
    assert len(listed) == 10, (len(listed), sorted(item["url"].rpartition("/")[2] for item in listed))
    for status, proposed in ((404, True), (410, True), (503, False)):
        results = written(scratch, f"results-{status}.json", {alias["sha512"]: status})
        out = scratch / f"recheck-{status}.json"
        ran = intake("recheck", "--merged", merged, "--assets", assets, "--results", results, "--out", out)
        assert ran.returncode == 0, ran.stderr
        revoked = json.loads(out.read_text())
        assert isinstance(revoked, list), "recheck writes the list the merged data's revoked takes"
        if proposed:
            keys = sorted(item["key"] for item in revoked)
            assert "nodeagent" in keys and all(item["version"] == "3.0.0" and item["lock_revision"] == 1 for item in revoked), revoked
            assert all(alias["url"] in item["reason"] and str(status) in item["reason"] for item in revoked), revoked
            # Its entries are appended to the merged data's revoked as they are.
            data_rendered = json.loads((RENDER / "merged.json").read_text())
            data_rendered["revoked"] = data_rendered.get("revoked", []) + revoked
            held = written(scratch, f"recheck-render-{status}.json", data_rendered)
            _, catalog = rendered(scratch, f"recheck-catalog-{status}.json", merged=held)
            assert {(item["key"], item["version"], item["lock_revision"]) for item in revoked} <= {
                (item["key"], item["version"], item.get("lock_revision")) for item in catalog["revoked"]
            }, catalog["revoked"]
        else:
            assert revoked == [], f"{status} proposes nothing: {revoked}"
    # Two tarballs of one lock gone propose one revocation of it.
    gone = next(item for item in listed if item["url"].endswith("withgyp-2.0.0.tgz"))
    results = written(scratch, "results-two.json", {alias["sha512"]: 404, gone["sha512"]: 410})
    out = scratch / "recheck-two.json"
    ran = intake("recheck", "--merged", merged, "--assets", assets, "--results", results, "--out", out)
    assert ran.returncode == 0, ran.stderr
    revoked = json.loads(out.read_text())
    identities = [(item["key"], item["version"], item["lock_revision"]) for item in revoked]
    assert len(identities) == len(set(identities)), f"one lock revision is revoked once: {revoked}"
    # An address is quoted with every character that could hide it escaped.
    hostile = scratch / "recheck-hostile"
    (hostile / "assets").mkdir(parents=True)
    url = NPM + "x/-/x\u202e.tgz"
    lock = lock_bytes({"packages": [{"location": "node_modules/x", "name": "x", "version": "1.0.0", "url": url,
                                     "sha512": "a" * 128, "size": 1, "files": 1, "unpacked": 1}], "unpack_rules": 1})
    (hostile / "assets" / sha(lock)).write_bytes(lock)
    merged = written(hostile, "merged.json", {"entries": [
        {"key": "hostile", "kind": "npm", "lock_revision": 1, "targets": [{"lock": {"sha256": sha(lock)}}], "version": "1.0.0"},
    ]})
    out = hostile / "recheck.json"
    ran = intake("recheck", "--merged", merged, "--assets", hostile / "assets",
                 "--results", written(hostile, "results.json", {"a" * 128: 404}), "--out", out)
    assert ran.returncode == 0, ran.stderr
    [item] = json.loads(out.read_text())
    assert "\u202e" not in item["reason"] and "\\u202e" in item["reason"], "the address is shown escaped"


def check_npm_publish(scratch: pathlib.Path, data: dict, assets: pathlib.Path) -> None:
    """A lock asset the merged data names is published."""
    held = scratch / "publish-npm"
    (held / "assets").mkdir(parents=True)
    [entry] = [entry for entry in data["entries"] if entry["key"] == "nodeagent"]
    named = entry["targets"][0]["lock"]["sha256"]
    (held / "assets" / named).write_bytes((assets / named).read_bytes())
    (held / "merged.json").write_text(json.dumps(data))
    ran = intake("publish-check", "--merged", held / "merged.json", "--assets", held / "assets")
    assert ran.returncode == 0, f"a lock asset was refused:\n{ran.stdout}{ran.stderr}"


def check_npm_lock_and_tree(scratch: pathlib.Path, tree: dict) -> None:
    """`lock` writes the lock `review` holds the tree to, and `tree` lists a
    directory's regular files as npm's side of the comparison."""
    out = scratch / "lock.json"
    ran = intake("lock", "--resolved", scratch / "observed/npx-agent", "--package", "npx-agent@3.0.0",
                 "--platform", "darwin-aarch64", "--out", out)
    assert ran.returncode == 0, ran.stderr
    assert out.read_bytes() == lock_bytes(expected_lock(tree, "darwin-aarch64")), out.read_text()
    made = scratch / "made"
    (made / "node_modules/a/bin").mkdir(parents=True)
    (made / "node_modules/a/index.js").write_bytes(b"1\n")
    (made / "node_modules/a/bin/run").write_bytes(b"#!\n")
    os.chmod(made / "node_modules/a/bin/run", 0o744)
    (made / "node_modules/.bin").mkdir()
    (made / "node_modules/.bin/run").symlink_to("../a/bin/run")
    out = scratch / "tree.json"
    ran = intake("tree", "--root", made, "--out", out)
    assert ran.returncode == 0, ran.stderr
    assert json.loads(out.read_text()) == {"files": [
        {"exec": True, "path": "node_modules/a/bin/run", "sha256": sha(b"#!\n"), "size": 3},
        {"exec": False, "path": "node_modules/a/index.js", "sha256": sha(b"1\n"), "size": 2},
    ]}, out.read_text()


def packed(entries: list[tuple[str, bytes, bytes]]) -> bytes:
    """A tarball of `entries`, each a name, its tar type and its bytes, in
    the GNU format, which can carry a sparse member's type."""
    held = io.BytesIO()
    with gzip.GzipFile(fileobj=held, mode="wb", mtime=0) as zipped, \
            tarfile.open(fileobj=zipped, mode="w", format=tarfile.GNU_FORMAT) as tar:
        for name, kind, content in entries:
            info = tarfile.TarInfo(name)
            info.type, info.mode = kind, 0o755 if kind == tarfile.DIRTYPE else 0o644
            info.size = len(content) if kind != tarfile.DIRTYPE else 0
            tar.addfile(info, io.BytesIO(content) if info.size else None)
    return held.getvalue()


def tar_member(name: bytes, kind: bytes, content: bytes = b"", size: int | None = None) -> bytes:
    """One tar member written as its bytes: a ustar header naming `name`, of
    type `kind`, giving `size` (by default the length of `content`), then
    `content` padded to whole blocks."""
    block = bytearray(512)
    block[0:len(name)] = name
    block[100:108] = b"0000644\0"
    block[108:124] = b"0000000\0" * 2
    block[124:136] = b"%011o\0" % (len(content) if size is None else size)
    block[136:148] = b"00000000000\0"
    block[148:156] = b" " * 8
    block[156:157] = kind
    block[257:265] = b"ustar\x0000"
    block[148:156] = b"%06o\0 " % sum(block)
    return bytes(block) + content + bytes(-len(content) % 512)


def pax_records(**records: bytes) -> bytes:
    """An extended header's records, each `length key=value` and a newline."""
    held = b""
    for key, value in records.items():
        line = b" %s=%s\n" % (key.encode(), value)
        length = len(line) + 1
        while len(b"%d" % length) + len(line) != length:
            length += 1
        held += b"%d" % length + line
    return held


def tar_written(*members: bytes) -> bytes:
    """A tarball of `members`, already written, and the two empty blocks that
    end an archive."""
    return gzip.compress(b"".join(members) + bytes(1024), mtime=0)


def with_tarball(tree: dict, location: str, tarball: bytes) -> dict:
    """`tree` with `tarball` in place of the one at `location`, and the lock's
    integrity for it made the new one's."""
    held = {**tree, "lock": json.loads(json.dumps(tree["lock"])), "tarballs": dict(tree["tarballs"])}
    held["tarballs"][list(tree["lock"]["packages"]).index(location)] = tarball
    integrity = "sha512-" + base64.b64encode(hashlib.sha512(tarball).digest()).decode()
    held["lock"]["packages"][location]["integrity"] = integrity
    return held


def check_npm_tarballs(scratch: pathlib.Path) -> None:
    """A tarball is read as the client reads it: one gzip stream, ordinary
    files alone written, every name one the client writes, none twice, and
    no more members than the client takes; and a lock's packages write no
    more files together than the client unpacks."""
    scratch = scratch / "npm-tarballs"
    tree = npm_tree()
    base = [(f"package/{path}", tarfile.REGTYPE, content) for path, (content, _) in tree["files"][TOP].items()]
    deep = "/".join(["d" * 200] * 4)
    many = [(f"package/d{index}", tarfile.DIRTYPE, b"") for index in range(65_537)]
    # As many files as the tree may hold, each under 58 directories of its
    # own, so the names it implies are many times its files.
    under = "/".join(["abcdefghi"] * 58)
    room = 65_536 - sum(len(files) for files in tree["files"].values())
    nested = [(f"package/u{index:05}/{under}", tarfile.REGTYPE, b"") for index in range(room)]
    # Fewer names than a tree may hold, each long enough that together they
    # hold more bytes than a tarball's names may.
    wide = [(f"package/w/{index:06}{'x' * 244}", tarfile.REGTYPE, b"") for index in range(33_600)]
    # The tarball as written header by header, for what a tar writer will
    # not write: each case's members follow it.
    raw = b"".join(tar_member(name.encode(), b"0", content) for name, _, content in base)
    added = tar_member(b"package/lib/more.js", b"0", b"2\n")
    # A global header as the registry's tarballs open with one, holding a
    # comment alone, which names no member.
    global_comment = tar_member(b"pax_global_header", b"g", pax_records(comment=b"0123456789abcdef\x1b[2J"))
    # A global header declaring one byte more than an extended header may
    # hold, and no body: it is refused before any of it is read.
    global_big = tar_member(b"pax_global_header", b"g", size=load_intake().EXTENDED_BYTES + 1)
    # A comment, a blank line and a `path` after it, which the client's reader
    # stops before and npm's does not.
    hidden = pax_records(comment=b"0123456789abcdef") + b"\n" + pax_records(path=b"package/lib/hidden.js")
    global_hidden = tar_member(b"pax_global_header", b"g", hidden)
    for name, tarball, words in (
        ("slashed", tar_written(raw, tar_member(b"package/lib/more/", b"\0", b"2\n")), "empty"),
        ("paxslash", tar_written(raw, tar_member(b"package/x", b"x", pax_records(path=b"package/lib/more.js/")),
                                 added), "empty"),
        ("global", tar_written(raw, tar_member(b"package/g", b"g", pax_records(path=b"package/lib/global.js")),
                               added), "says more than a comment"),
        ("globaltwice", tar_written(global_comment, global_comment, raw, added), "after another"),
        ("globalbig", tar_written(global_big), "global extended header of more than"),
        ("globalhidden", tar_written(global_hidden, raw, added), "says more than a comment"),
        ("globalahead", tar_written(raw, tar_member(b"././@LongLink", b"L", b"package/lib/one.js\0"),
                                    global_comment, added), "between a member's own headers"),
        ("spelled", tar_written(raw, tar_member(b"package/A/x", b"0", b"2\n"), tar_member(b"package/a/y", b"0", b"2\n")),
         "spelled in another case"),
        ("paxsize", tar_written(raw, tar_member(b"package/x", b"x", pax_records(size=b"3")), added), "size"),
        ("longtwice", tar_written(raw, tar_member(b"././@LongLink", b"L", b"package/lib/one.js\0"),
                                  tar_member(b"././@LongLink", b"L", b"package/lib/two.js\0"), added),
         "describes one member twice"),
        ("solaris", tar_written(raw, tar_member(b"package/x", b"X", pax_records(path=b"package/lib/x.js")),
                                added), "of a kind"),
        ("modules", tar_written(raw, tar_member(b"package/node_modules/other/index.js", b"0", b"2\n")),
         "node_modules directory"),
        ("Modules", tar_written(raw, tar_member(b"package/Node_Modules/other/index.js", b"0", b"2\n")),
         "node_modules directory"),
    ):
        observed = scratch / name / "observed"
        lay_out_npm(observed, "npx-agent", with_tarball(tree, TOP, tarball))
        out = scratch / name / "lock.json"
        ran = intake("lock", "--resolved", observed / "npx-agent", "--package", "npx-agent@3.0.0",
                     "--platform", "darwin-aarch64", "--out", out)
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-400:]!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not out.exists(), name
    for name, tarball, words in (
        ("members", packed(base) + gzip.compress(b"more", mtime=0), "gzip stream"),
        ("sparse", packed([*base, ("package/hole.bin", tarfile.GNUTYPE_SPARSE, b"")]), "of a kind"),
        ("contiguous", packed([*base, ("package/next.bin", tarfile.CONTTYPE, b"x")]), "of a kind"),
        ("twice", packed([*base, ("package/lib/run.js", tarfile.REGTYPE, b"2\n")]), "twice"),
        ("folded", packed([*base, ("package/LIB/run.js", tarfile.REGTYPE, b"2\n")]), "twice"),
        ("under", packed([*base, ("package/lib/run.js/inner.js", tarfile.REGTYPE, b"2\n")]), "twice"),
        ("device", packed([*base, ("package/lib/aux.txt", tarfile.REGTYPE, b"2\n")]), "device"),
        ("dot", packed([*base, ("package/lib/run.", tarfile.REGTYPE, b"2\n")]), "dot or a space"),
        ("long", packed([*base, (f"package/{deep}", tarfile.REGTYPE, b"2\n")]), "767"),
        ("headers", packed(base + many), "65536"),
        ("nested", packed(base + nested), "131072 names"),
        ("wide", packed(base + wide), "8388608 bytes"),
    ):
        observed = scratch / name / "observed"
        lay_out_npm(observed, "npx-agent", with_tarball(tree, TOP, tarball))
        out = scratch / name / "lock.json"
        ran = intake("lock", "--resolved", observed / "npx-agent", "--package", "npx-agent@3.0.0",
                     "--platform", "darwin-aarch64", "--out", out)
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-400:]!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not out.exists(), name
    # Two tarballs that write more files together than a tree may hold.
    half = [(f"package/f{index}", tarfile.REGTYPE, b"") for index in range(32_769)]
    held = with_tarball(tree, TOP, packed(base + half))
    alias = [(f"package/{path}", tarfile.REGTYPE, content) for path, (content, _) in tree["files"][ALIAS].items()]
    held = with_tarball(held, ALIAS, packed(alias + half))
    observed = scratch / "files" / "observed"
    lay_out_npm(observed, "npx-agent", held)
    out = scratch / "files" / "lock.json"
    ran = intake("lock", "--resolved", observed / "npx-agent", "--package", "npx-agent@3.0.0",
                 "--platform", "darwin-aarch64", "--out", out)
    assert ran.returncode == 1 and "65536 files" in ran.stderr, f"files: {ran.returncode} {ran.stderr[-400:]!r}"
    # Two tarballs that write, or hold, as many bytes together as a tree may,
    # and one byte more; no tarball that large can be laid out here, so the
    # lock is made from what the intake read of them.
    module = load_intake()
    ceiling = module.TREE_BYTES
    for field, words in (("unpacked", f"write more than {ceiling} bytes"), ("size", f"hold more than {ceiling} bytes")):
        for extra in (0, 1):
            installed = [
                {"bundled": False, "location": f"node_modules/p{index}", "name": f"p{index}", "version": "1.0.0",
                 "url": f"{NPM}p{index}/-/p{index}-1.0.0.tgz", "manifest": {},
                 "tarball": {"files": 1, "sha512": bytes(64), "size": 1, "unpacked": 1,
                             field: ceiling // 2 + extra * index}}
                for index in (0, 1)
            ]
            try:
                module.npm_lock(installed)
            except module.Dropped as refused:
                assert extra and words in str(refused), f"{field} {extra}: {refused}"
            else:
                assert not extra, f"{field}: a lock past {ceiling} bytes was made"
    # The tree as built, which every case changes, is laid out and read, and
    # so is one whose added file is named by an extended header, and sized by
    # it as its own header sizes it.
    named = tar_member(b"package/x", b"x", pax_records(path=b"package/lib/more.js", size=b"2"))
    for name, tarball in (
        ("whole", packed(base)),
        ("extended", tar_written(raw, named, tar_member(b"package/short", b"0", b"2\n"))),
        ("commented", tar_written(global_comment, raw, added)),
    ):
        lay_out_npm(scratch / name / "observed", "npx-agent", with_tarball(tree, TOP, tarball))
        out = scratch / name / "lock.json"
        ran = intake("lock", "--resolved", scratch / name / "observed/npx-agent", "--package", "npx-agent@3.0.0",
                     "--platform", "darwin-aarch64", "--out", out)
        assert ran.returncode == 0, f"{name}: {ran.stderr[-400:]!r}"


def check_npm_node(scratch: pathlib.Path, signers: Signers) -> None:
    """Node's checksums are taken only under one good signature by a listed
    key made before that key expired, and Node's download only as they list it;
    a pin or dispositions that break a rule refuse the run."""
    scratch = scratch / "npm-node"
    for name, signed, change, words in (
        ("unlisted", signers.signed(by="unlisted"), None, "fingerprint"),
        ("expired", signers.after, None, "expired"),
        ("orphaned", signers.orphaned, None, "expired"),
        ("sha1", signers.sha1, None, "digest"),
        ("garbled", b"not signed at all\n", None, "one good signature"),
        ("repeated", signers.signed(shasums() + shasums()), None, "twice"),
        ("tampered", signers.signed(), "node/linux-x86_64/artifact", "not the bytes SHASUMS256.txt lists"),
        ("before", signers.before, None, None),
    ):
        held = scratch / name
        observed = held / "observed"
        lay_out_node(observed, signed)
        agents: list[dict] = []
        choices: dict = {}
        npm_case(observed, agents, choices, "npx-agent", npm_tree())
        if change is not None:
            (observed / change).write_bytes(b"other bytes")
        registry = written(held, "registry.json", {"version": "1.0.0", "agents": agents})
        data, _ = npm_review(held, signers, registry, written(held, "choices.json", choices))
        if words is None:
            assert [entry["key"] for entry in data["entries"]] == ["nodeagent"], f"{name}: {data['dropped']}"
        elif change is not None:
            dropped(data, "npx-agent", "linux-x86_64", words)
            assert [entry["key"] for entry in data["entries"]] == ["nodeagent"], name
        else:
            assert not data["entries"], f"{name}: {data['entries']}"
            for platform in NODE_BUILT:
                dropped(data, "npx-agent", platform, "not verified", words)
    held = scratch / "before"

    def refuses(name: str, words: str, *extra: str | pathlib.Path, env: dict | None = None) -> None:
        ran = subprocess.run(
            [sys.executable, "-I", str(INTAKE), "review", "--registry", REGISTRY, "--choices", CHOICES,
             "--runners", RUNNERS, "--observed", held / "observed", "--deny", ROOT / "deny.toml",
             "--assets", scratch / "none", "--out", scratch / f"{name}.json", *map(str, extra)],
            capture_output=True, text=True, check=False, timeout=60, env=env,
        )
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not (scratch / f"{name}.json").exists(), f"{name}: {ran.stderr!r}"

    for name, pin, words in (
        ("odd", '{"version": "23.1.0"}', "long-term-support"),
        ("early", '{"version": "22.14.0"}', "22.15.0"),
        ("old", '{"version": "20.19.0"}', "long-term-support"),
        ("extra", '{"version": "22.15.0", "mirror": true}', "mirror"),
        ("repeated", '{"version": "22.15.0", "version": "24.0.0"}', "twice"),
        ("loose", '{"version": "v22.15.0"}', "X.Y.Z"),
    ):
        path = scratch / f"node-{name}.json"
        path.write_text(pin)
        refuses(f"pin-{name}", words, "--node", path, "--node-keys", signers.keys)
    pin = written(scratch, "pin.json", {"version": "22.15.0"})
    empty = scratch / "no-gpg"
    empty.mkdir()
    refuses("no-gpg", "gpg", "--node", pin, "--node-keys", signers.keys, env={"PATH": str(empty)})
    for name, value, words in (
        ("built", {"npx-agent": {f"{TOP} postinstall": "built"}}, "build route"),
        ("local", {"npx-agent": {f"{TOP} postinstall": "local"}}, "build route"),
        ("unknown", {"npx-agent": {f"{TOP} postinstall": "skipped"}}, "inert"),
        ("shapeless", {"npx-agent": "inert"}, "dispositions"),
    ):
        refuses(f"dispositions-{name}", words, "--dispositions", decided(scratch, value, f"d-{name}.json"))
    repeated = scratch / "d-repeated.json"
    repeated.write_text('{"npx-agent": {"a postinstall": "inert", "a postinstall": "lost"}}')
    refuses("dispositions-repeated", "twice", "--dispositions", repeated)


def check_npm_render(scratch: pathlib.Path) -> None:
    """The committed catalog carries an npm entry, and one missing what the
    client reads to start it, or with a script nobody decided, is refused."""
    catalog = json.loads((RENDER / "catalog.json").read_text())
    entry = next(entry for entry in catalog["entries"] if entry["kind"] == "npm")
    target = entry["targets"][0]
    assert target["lock"]["unpack_rules"] == 1 and target["node_args"] == [], target
    merged = json.loads((RENDER / "merged.json").read_text())
    index = next(index for index, entry in enumerate(merged["entries"]) if entry["kind"] == "npm")
    for name, change, words in (
        ("undecided", lambda t: t["scripts"][0].update(disposition=None), "disposition"),
        ("unknown", lambda t: t["scripts"][0].update(disposition="compiled"), "disposition"),
        ("local-bare", lambda t: t["scripts"][0].update(disposition="local"), "command"),
        ("local-host", lambda t: t["scripts"][0].update(
            disposition="local", command="node x.js", hosts=["no host"], result={"sha256": "0" * 64, "size": 2}
        ), "hosts"),
        ("no-bin", lambda t: t.pop("bin"), "bin"),
        ("rules", lambda t: t["lock"].update(unpack_rules=2), "unpack_rules"),
        ("no-lock", lambda t: t.pop("lock"), "lock"),
        ("node-args", lambda t: t.update(node_args="--x"), "node_args"),
    ):
        data = copy.deepcopy(merged)
        change(data["entries"][index]["targets"][0])
        refused(scratch, f"npm-{name}.json", words, merged=written(scratch, f"m-npm-{name}.json", data))
    data = copy.deepcopy(merged)
    data["entries"][index].pop("lock_revision")
    refused(scratch, "npm-revision.json", "lock_revision", merged=written(scratch, "m-npm-revision.json", data))


# The members of the `.tar.bz2` fixture, in archive order: name, kind, mode,
# and content or link target. A name longer than 100 bytes takes a GNU long
# name header, and the program's set-user-id bit is not a permission.
BZ2_MEMBERS = [
    ("goose", "dir", 0o755, None),
    ("goose/bin", "dir", 0o750, None),
    ("goose/bin/goose", "file", 0o4755, b"\x7fELF the goose program\n"),
    ("goose/README.md", "file", 0o644, b"# goose\n"),
    ("goose/share", "dir", 0o755, None),
    ("goose/share/" + "a-long-name-" * 10 + ".txt", "file", 0o600, b"a long name\n"),
    ("goose/bin/g", "link", 0o777, "goose"),
]
TAR_KINDS = {
    "dir": tarfile.DIRTYPE, "link": tarfile.SYMTYPE, "hard": tarfile.LNKTYPE, "fifo": tarfile.FIFOTYPE,
    "char": tarfile.CHRTYPE, "block": tarfile.BLKTYPE, "sparse": tarfile.GNUTYPE_SPARSE,
}


def bz2_tar(
    members: list[tuple],
    *,
    format: int = tarfile.GNU_FORMAT,
    owner: tuple[int, str] = (1000, "builder"),
    mtime: int = 1_700_000_000,
    pax: dict | None = None,
) -> bytes:
    """A `.tar.bz2` of `members`, each owned by `owner` and stamped `mtime`."""
    held = io.BytesIO()
    with tarfile.open(fileobj=held, mode="w", format=format, pax_headers=pax) as tar:
        for name, kind, mode, value in members:
            info = tarfile.TarInfo(name)
            info.mode, info.mtime = mode, mtime
            info.uid = info.gid = owner[0]
            info.uname = info.gname = owner[1]
            if kind == "file":
                info.size = len(value)
                tar.addfile(info, io.BytesIO(value))
                continue
            info.type = TAR_KINDS[kind]
            if kind in ("link", "hard"):
                info.linkname = value
            if isinstance(value, dict):
                info.pax_headers = value
            tar.addfile(info)
    return bz2.compress(held.getvalue())


def repack_of(scratch: pathlib.Path, name: str, content: bytes) -> tuple[subprocess.CompletedProcess[str], pathlib.Path]:
    held = scratch / "repack" / name
    held.mkdir(parents=True)
    (held / "in.tar.bz2").write_bytes(content)
    return intake("repack", "--input", held / "in.tar.bz2", "--out", held / "out.tar.gz"), held


def repacked_by(scratch: pathlib.Path, name: str, content: bytes) -> bytes:
    ran, held = repack_of(scratch, name, content)
    assert ran.returncode == 0, f"{name}: the repack was refused: {ran.stderr[-600:]}"
    return (held / "out.tar.gz").read_bytes()


def bz2_listing(members: list[tuple]) -> dict:
    """The client's listing of the repack of `members`."""
    return listed(
        {name: (value, bool(mode & 0o111)) for name, kind, mode, value in members if kind == "file"},
        tuple(name for name, kind, _, _ in members if kind == "dir"),
        tuple((name, value) for name, kind, _, value in members if kind == "link"),
    )


def check_repack(scratch: pathlib.Path) -> None:
    """A `.tar.bz2` is repacked to a `.tar.gz` whose files are the same names,
    modes and bytes, with nothing of who made it or when, the same bytes every
    time; a member the client would refuse refuses the repack, leaving nothing."""
    content = bz2_tar(BZ2_MEMBERS)
    out = repacked_by(scratch, "fixture", content)
    assert out[:3] == b"\x1f\x8b\x08" and out[3] == 0, "the gzip header names no file and sets no flag"
    assert out[4:8] == b"\0\0\0\0", "the gzip header carries mtime 0"
    raw = gzip.decompress(out)
    assert b"ustar  \0" in raw and b"ustar\x0000" not in raw, "the repack is written as a GNU tar"
    with tarfile.open(fileobj=io.BytesIO(out), mode="r:gz") as tar:
        members = tar.getmembers()
        assert [member.name for member in members] == [name for name, _, _, _ in BZ2_MEMBERS], "names and order kept"
        for member, (name, kind, mode, value) in zip(members, BZ2_MEMBERS):
            assert member.type == ({"file": tarfile.REGTYPE} | TAR_KINDS)[kind], f"{name}: kind {member.type!r}"
            assert member.mode == mode & 0o777, f"{name}: mode {oct(member.mode)}"
            assert (member.uid, member.gid, member.uname, member.gname, member.mtime) == (0, 0, "", "", 0), name
            if kind == "file":
                extracted = tar.extractfile(member)
                assert extracted is not None and sha(extracted.read()) == sha(value), f"{name}: content differs"
            if kind == "link":
                assert member.linkname == value, f"{name}: links to {member.linkname!r}"
    assert repacked_by(scratch, "again", content) == out, "two repacks of one archive are the same bytes"
    other = bz2_tar(BZ2_MEMBERS, format=tarfile.PAX_FORMAT, owner=(0, "root"), mtime=5)
    assert other != content and repacked_by(scratch, "other", other) == out, "who made it and when are not kept"

    def refused(name: str, held: bytes, *words: str) -> None:
        ran, place = repack_of(scratch, name, held)
        assert ran.returncode == 1 and "Traceback" not in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-600:]!r}"
        for word in words:
            assert word in ran.stderr, f"{name}: {ran.stderr[-600:]!r} lacks {word!r}"
        assert sorted(path.name for path in place.iterdir()) == ["in.tar.bz2"], f"{name}: a refused repack left a file"

    for name, extra, words in (
        ("hard", [("goose/bin/h", "hard", 0o755, "goose/bin/goose")], "hard link"),
        ("fifo", [("goose/pipe", "fifo", 0o644, None)], "FIFO"),
        ("char", [("goose/tty", "char", 0o644, None)], "device"),
        ("block", [("goose/disk", "block", 0o644, None)], "device"),
        ("sparse", [("goose/holes", "sparse", 0o644, None)], "sparse"),
        ("twice", [("goose/README.md", "file", 0o644, b"again\n")], "twice"),
        ("folded", [("goose/readme.MD", "file", 0o644, b"again\n")], "twice"),
        ("dotdot", [("goose/../../evil", "file", 0o644, b"x")], ". or .."),
        ("absolute", [("/etc/evil", "file", 0o644, b"x")], "absolute"),
        ("colon", [("goose/a:b", "file", 0o644, b"x")], "Windows refuses"),
        ("link-out", [("goose/l", "link", 0o777, "/etc/passwd")], "relative"),
        ("link-back", [("goose/l", "link", 0o777, "..\\..\\evil")], "relative"),
    ):
        refused(name, bz2_tar(BZ2_MEMBERS + extra), *words)
    deep = "/".join(["d" * 70] * 63)
    refused("long", bz2_tar([(deep, "file", 0o644, b"x")], format=tarfile.PAX_FORMAT), "4096")
    refused("long-name", bz2_tar([("n/" * 2500 + "x", "file", 0o644, b"x")]), "long name")
    refused("pax-sparse", bz2_tar([("goose/holes", "dir", 0o755, {"GNU.sparse.map": "0,1", "GNU.sparse.size": "1"})],
                                  format=tarfile.PAX_FORMAT), "sparse")
    refused("global", bz2_tar(BZ2_MEMBERS, format=tarfile.PAX_FORMAT, pax={"comment": "made by"}), "global")
    refused("big-pax", bz2_tar([("goose/x", "dir", 0o755, {"comment": "x" * (70 << 10)})], format=tarfile.PAX_FORMAT),
            "extended header")
    refused("trailing", content + b"more", "one bzip2 stream")
    refused("two-streams", content + bz2.compress(b"more"), "one bzip2 stream")
    refused("truncated", content[:-40], "ends before")
    refused("garbage", b"not a bzip2 stream at all", "not a bzip2-compressed tar")
    refused("plain", gzip.decompress(out), "not a bzip2-compressed tar")
    refused("not-tar", bz2.compress(b"x" * 2000), "not a bzip2-compressed tar")
    # An output already there is replaced only once the whole repack is made.
    ran, held = repack_of(scratch, "missing", b"")
    (held / "in.tar.bz2").unlink()
    ran = intake("repack", "--input", held / "in.tar.bz2", "--out", held / "out.tar.gz")
    assert ran.returncode == 1 and "Traceback" not in ran.stderr and not list(held.iterdir()), ran.stderr

    module = load_intake()
    for words, ceilings in (
        ("ceiling", {"REPACK_DECOMPRESSED": 4096}),
        (f"more than {3} members", {"TARBALL_MEMBERS": 3}),
        ("archive is larger than", {"ARTIFACT_BYTES": 100}),
        ("repack would be larger than", {"REPACK_BYTES": 100}),
    ):
        saved = {name: getattr(module, name) for name in ceilings}
        try:
            for name, value in ceilings.items():
                setattr(module, name, value)
            try:
                module.repacked(io.BytesIO(content))
            except module.Dropped as failed:
                assert words in str(failed), f"{ceilings}: {failed} lacks {words!r}"
            else:
                raise AssertionError(f"{ceilings}: the repack was not refused")
        finally:
            for name, value in saved.items():
                setattr(module, name, value)
    assert module.repacked(io.BytesIO(content)) == out, "the job's repack is the review's"


BZ_URL = "https://downloads.bz.example/goose-1.0-{}.tar.bz2"


def check_bz2_review(scratch: pathlib.Path) -> None:
    """A `.tar.bz2` marked for mirroring is repacked by `review` from the bytes
    it hashed, kept only where the job's repack is those very bytes and the
    licence gate passes over it, and installs as that repack; one that is not
    marked is dropped, and so is every other archive kind."""
    scratch = scratch / "bz2"
    observed = scratch / "observed"
    sboms = scratch / "sboms"
    sboms.mkdir(parents=True)
    contents = {
        "linux-x86_64": bz2_tar(BZ2_MEMBERS),
        "darwin-aarch64": bz2_tar(BZ2_MEMBERS),
        "linux-aarch64": bz2_tar(BZ2_MEMBERS, mtime=7),
        "windows-x86_64": bz2_tar(BZ2_MEMBERS, mtime=8),
    }
    out = repacked_by(scratch, "fixture", contents["linux-x86_64"])
    agents = []

    def lay_out_bz2(agent: str, platform: str, content: bytes, url: str, job_repack: bytes | None) -> None:
        place = observed / agent / platform
        place.mkdir(parents=True)
        (place / "artifact").write_bytes(content)
        (place / "fetched.json").write_text(json.dumps({"chain": [url]}))
        if job_repack is not None:
            (place / "repacked").write_bytes(job_repack)
        (place / "unpack-check.json").write_text(json.dumps(bz2_listing(BZ2_MEMBERS)))
        (place / "probe.json").write_text(json.dumps(PROBED))

    binary = {}
    for platform, content in contents.items():
        url = BZ_URL.format(platform)
        binary[platform] = {"archive": url, "cmd": "goose/bin/goose", "sha256": sha(content)}
        job_repack = b"other bytes" if platform == "darwin-aarch64" else repacked_by(scratch, platform, content)
        lay_out_bz2("bz-agent", platform, content, url, job_repack)
    agents.append({"id": "bz-agent", "name": "bz", "version": "1.0.0", "license": "MIT", "distribution": {"binary": binary}})
    (sboms / f"{sha(contents['linux-x86_64'])}.json").write_bytes((GATE / "sbom.json").read_bytes())
    failing = json.loads((GATE / "sbom.json").read_text())
    failing["components"][0]["licenses"] = [{"expression": "Unlicense"}]
    (sboms / f"{sha(contents['windows-x86_64'])}.json").write_text(json.dumps(failing))
    for agent, licence, url in (
        ("bzprop-agent", "LicenseRef-Proprietary", BZ_URL.format("linux-x86_64")),
        ("xz-agent", "MIT", "https://downloads.bz.example/goose-1.0-linux-x86_64.tar.xz"),
    ):
        lay_out_bz2(agent, "linux-x86_64", contents["linux-x86_64"], url, out)
        target = {"archive": url, "cmd": "goose/bin/goose"}
        agents.append({"id": agent, "name": agent, "version": "1.0.0", "license": licence,
                       "distribution": {"binary": {"linux-x86_64": target}}})
    registry = written(scratch, "registry.json", {"version": "1.0.0", "agents": agents})
    choices = written(scratch, "choices.json", {agent["id"]: {"key": agent["id"].removesuffix("-agent")} for agent in agents})
    data, assets = review(scratch, "--sboms", sboms, "--texts", GATE / "texts", registry=registry, choices=choices)

    [entry] = data["entries"]
    assert entry["key"] == "bz" and [target["platform"] for target in entry["targets"]] == ["linux-x86_64"], entry
    [artifact] = entry["targets"][0]["artifacts"]
    original = contents["linux-x86_64"]
    assert artifact["url"] == BZ_URL.format("linux-x86_64"), "the artifact keeps the vendor's address"
    assert (artifact["sha256"], artifact["size"]) == (sha(original), len(original)), "and the download's hash and size"
    assert artifact["format"] == "tar.gz", "it installs as its repack, a tar.gz"
    assert artifact["mirror_licence"] == "MIT", artifact.get("mirror_licence")
    assert artifact["files"] == bz2_listing(BZ2_MEMBERS)["files"], "its files are the listing of the repack"
    assert artifact["replaced_by"] == {"sha256": sha(out), "size": len(out)}, artifact.get("replaced_by")
    record = artifact["licence_gate"]
    assert record["passed"] is True and record["asset"] == {"sha256": sha(out), "size": len(out)}, record
    assert (assets / sha(out)).read_bytes() == out, "the repack is kept as an asset by its hash"
    assert not (assets / sha(original)).exists(), "the vendor's bz2 is not kept"
    dropped(data, "bz-agent", "darwin-aarch64", "repack", "not the bytes")
    dropped(data, "bz-agent", "linux-aarch64", "no lockfile or SBOM was given")
    dropped(data, "bz-agent", "windows-x86_64", "Unlicense")
    dropped(data, "bzprop-agent", "linux-x86_64", "not on deny.toml's list", "repacked")
    dropped(data, "xz-agent", "linux-x86_64", "of a kind this intake does not take in")

    # The publishing job takes the repack under its passing record, beside its
    # NOTICE and licence texts, and never the vendor's bz2.
    beside = [(record["notice"]["sha256"], (assets / record["notice"]["sha256"]).read_bytes())]
    beside += [(licence["sha256"], (assets / licence["sha256"]).read_bytes()) for licence in record["licences"]]
    ran = publish_checked(scratch, "bz2", data, (sha(out), out), *beside)
    assert ran.returncode == 0, f"a gated repack was refused:\n{ran.stdout}{ran.stderr}"
    publish_refuses(publish_checked(scratch, "bz2-alone", data, (sha(out), out)), "NOTICE")
    publish_refuses(publish_checked(scratch, "bz2-original", data, (sha(original), original), *beside), "licence-gate")
    ungated = copy.deepcopy(data)
    ungated["entries"][0]["targets"][0]["artifacts"][0].pop("licence_gate")
    # Without the record its NOTICE and texts are in no data, so the repack goes alone.
    publish_refuses(publish_checked(scratch, "bz2-ungated", ungated, (sha(out), out)), "licence-gate")

    # `diff` shows what installs in the artifact's place; that `render` keeps
    # it is `check_publish_replaced`'s.
    ran = intake("diff", scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    assert f"replaced by `` {sha(out)} ``" in ran.stdout or f"replaced by {sha(out)}" in ran.stdout or (
        "replaced by" in ran.stdout and sha(out) in ran.stdout
    ), "diff shows the repack that installs in the artifact's place"
    changed = copy.deepcopy(data)
    changed["entries"][0]["targets"][0]["artifacts"][0]["replaced_by"] = {"sha256": "c" * 64, "size": 1}
    ran = intake("diff", "--old", written(scratch, "old.json", changed), scratch / "review.json")
    assert ran.returncode == 0 and "artifact replaced by" in ran.stdout, ran.stdout[-2000:]

    # With no input for the gate, a bz2 has no route to install, so it is dropped.
    plain, _ = review(scratch, name="ungated.json", registry=registry, choices=choices)
    dropped(plain, "bz-agent", "linux-x86_64", "no lockfile or SBOM was given")
    assert not plain["entries"], plain["entries"]

    # What one run holds of repacks is bounded; past it the repack is refused.
    module = load_intake()
    gate = module.Gate(module.allowed_licences(ROOT / "deny.toml"), sboms, GATE / "texts")
    module.REPACK_HELD_BYTES = len(out) - 1
    held_assets: dict = {}
    try:
        gate.replacing({}, sboms / f"{sha(original)}.json", out, held_assets)
    except module.Dropped as failed:
        assert str(module.REPACK_HELD_BYTES) in str(failed), str(failed)
    else:
        raise AssertionError("a repack past what a run holds was kept")
    assert not held_assets, "nothing of a refused repack is kept"
    module.REPACK_HELD_BYTES = 1 << 30

    # The job's repack is compared to this review's without being held whole,
    # and refused on its size before any of it is read.
    module.BUDGET = module.Budget()
    try:
        place = scratch / "compare"
        place.mkdir()
        for name, held_bytes, words in (
            ("longer", out + b"x", "not the bytes"),
            ("flipped", out[:-1] + bytes([out[-1] ^ 1]), "not the bytes"),
        ):
            (place / name).write_bytes(held_bytes)
            before = module.BUDGET.read
            try:
                module.same_bytes(place / name, out, "the job's repack")
            except module.Dropped as failed:
                assert words in str(failed), f"{name}: {failed}"
            else:
                raise AssertionError(f"{name}: a repack that differs was taken")
            if name == "longer":
                assert module.BUDGET.read == before, "a repack of another size is refused before it is read"
        (place / "same").write_bytes(out)
        module.same_bytes(place / "same", out, "the job's repack")
    finally:
        module.BUDGET = None

    # What one run decompresses of every bz2 is bounded; past it the next
    # target waits for the next run.
    unpacked_size = len(bz2.decompress(original))
    module.REPACK_RUN_DECOMPRESSED = unpacked_size * 3 // 2
    namespace = argparse.Namespace(
        registry=registry, choices=choices, runners=RUNNERS, observed=observed, deny=ROOT / "deny.toml",
        assets=scratch / "run-assets", out=scratch / "run-budget.json", merged=None, interpreters=None, node=None,
        node_keys=None, dispositions=None, sboms=sboms, texts=GATE / "texts",
    )
    assert module.review(namespace) == 0
    spent = json.loads((scratch / "run-budget.json").read_text())
    dropped(spent, "bz-agent", "darwin-aarch64", "not the bytes")
    for platform in ("linux-aarch64", "linux-x86_64", "windows-x86_64"):
        dropped(spent, "bz-agent", platform, str(module.REPACK_RUN_DECOMPRESSED), "next run")


BSD_ABI = "FreeBSD:14:amd64"
LDD_ADDRESS = "(0x82a3c1000)"


def staged_runtime(
    held: pathlib.Path,
    *,
    program: str = "node",
    package: str = "node22",
    licences: list[str] | None = None,
    logic: str = "single",
    python: bool = False,
) -> tuple[pathlib.Path, dict, dict]:
    """What the FreeBSD step stages for one runtime: the program, one library
    it needs (through its versioned link), each package's licence, and for
    CPython its standard library; with the package list `pkg query` gives.
    Returns the staged directory, the ldd sections by staged path, and the
    packages."""
    staged = held / "staged"
    for directory in ("bin", "lib", f"licenses/{package}", "licenses/libfoo"):
        (staged / directory).mkdir(parents=True)
    (staged / "bin" / program).write_bytes(f"\x7fELF {program}\n".encode())
    (staged / "bin" / program).chmod(0o755)
    (staged / "lib/libfoo.so.1.2").write_bytes(b"\x7fELF libfoo\n")
    (staged / "lib/libfoo.so.1").symlink_to("libfoo.so.1.2")
    (staged / f"licenses/{package}/LICENSE").write_bytes(f"{package} licence\n".encode())
    (staged / "licenses/libfoo/COPYING").write_bytes(b"libfoo licence\n")
    root = os.path.realpath(staged)
    sections = {
        f"bin/{program}": [
            f"libfoo.so.1 => {root}/bin/../lib/libfoo.so.1 {LDD_ADDRESS}",
            f"libc.so.7 => /lib/libc.so.7 {LDD_ADDRESS}",
            f"libc++.so.1 => /usr/lib/libc++.so.1 {LDD_ADDRESS}",
            f"[vdso] {LDD_ADDRESS}",
        ],
        "lib/libfoo.so.1.2": [f"libc.so.7 => /lib/libc.so.7 {LDD_ADDRESS}"],
    }
    files = [f"/usr/local/bin/{program}"]
    if python:
        (staged / f"lib/python{PYTHON}/lib-dynload").mkdir(parents=True)
        (staged / f"lib/python{PYTHON}/os.py").write_bytes(b"# os\n")
        (staged / f"lib/python{PYTHON}/lib-dynload/_ssl.so").write_bytes(b"\x7fELF _ssl\n")
        sections[f"lib/python{PYTHON}/lib-dynload/_ssl.so"] = [f"libc.so.7 => /lib/libc.so.7 {LDD_ADDRESS}"]
        files += [f"/usr/local/lib/python{PYTHON}/os.py", f"/usr/local/lib/python{PYTHON}/lib-dynload/_ssl.so"]
    packages = {
        "packages": [
            {"name": package, "version": "22.15.1", "licenses": licences or ["MIT"], "logic": logic, "files": files},
            {"name": "libfoo", "version": "1.2_1", "licenses": ["ISCL", "MIT"], "logic": "and",
             "files": ["/usr/local/lib/libfoo.so.1.2", "/usr/local/lib/libfoo.so.1"]},
        ]
    }
    return staged, sections, packages


def ldd_text(staged: pathlib.Path, sections: dict) -> str:
    root = os.path.realpath(staged)
    return "".join(f"{root}/{path}:\n" + "".join(f"\t{line}\n" for line in lines) for path, lines in sections.items())


def bundle_of(
    held: pathlib.Path, staged: pathlib.Path, ldd: str, packages: dict, abi: str = BSD_ABI
) -> tuple[subprocess.CompletedProcess[str], pathlib.Path, pathlib.Path]:
    (held / "ldd.txt").write_text(ldd)
    written(held, "packages.json", packages)
    out, sbom = held / "out" / "bundle.tar.gz", held / "out" / "sbom.json"
    out.parent.mkdir(exist_ok=True)
    ran = intake(
        "bundle", "--staged", staged, "--ldd", held / "ldd.txt", "--packages", held / "packages.json",
        "--abi", abi, "--out", out, "--sbom", sbom,
    )
    return ran, out, sbom


def bundled(held: pathlib.Path, **kept: object) -> tuple[bytes, dict, dict]:
    """A bundle the FreeBSD step would make, its SBOM, and the client's listing of it."""
    held.mkdir(parents=True)
    staged, sections, packages = staged_runtime(held, **kept)
    ran, out, sbom = bundle_of(held, staged, ldd_text(staged, sections), packages)
    assert ran.returncode == 0, f"the bundle was refused: {ran.stderr[-600:]}"
    files: dict[str, tuple[bytes, bool]] = {}
    dirs, links = [], []
    with tarfile.open(fileobj=io.BytesIO(out.read_bytes()), mode="r:gz") as tar:
        for member in tar.getmembers():
            if member.isdir():
                dirs.append(member.name)
            elif member.issym():
                links.append((member.name, member.linkname))
            else:
                extracted = tar.extractfile(member)
                assert extracted is not None
                files[member.name] = (extracted.read(), bool(member.mode & 0o111))
    return out.read_bytes(), json.loads(sbom.read_text()), listed(files, tuple(dirs), tuple(links))


def check_bundle(scratch: pathlib.Path) -> None:
    """The FreeBSD step's staged runtime is bundled with the libraries ldd
    resolves into it, each installed by a listed package, as a deterministic
    tar.gz and an SBOM the licence gate reads; a library that resolves
    anywhere but the bundle or the base system cannot be bundled."""
    scratch = scratch / "bundle"
    held = scratch / "made"
    held.mkdir(parents=True)
    staged, sections, packages = staged_runtime(held)
    ran, out, sbom = bundle_of(held, staged, ldd_text(staged, sections), packages)
    assert ran.returncode == 0, f"the bundle was refused: {ran.stderr[-600:]}"
    made, sbom_made = out.read_bytes(), sbom.read_bytes()
    assert made[3] == 0 and made[4:8] == b"\0\0\0\0", "the gzip header names no file and carries mtime 0"
    with tarfile.open(fileobj=io.BytesIO(made), mode="r:gz") as tar:
        members = tar.getmembers()
        assert [member.name for member in members] == [
            "bin", "bin/node", "lib", "lib/libfoo.so.1", "lib/libfoo.so.1.2", "licenses", "licenses/libfoo",
            "licenses/libfoo/COPYING", "licenses/node22", "licenses/node22/LICENSE",
        ], [member.name for member in members]
        for member in members:
            assert (member.uid, member.gid, member.uname, member.gname, member.mtime) == (0, 0, "", "", 0), member.name
            on_disk = os.lstat(staged / member.name)
            if not member.issym():
                assert member.mode == on_disk.st_mode & 0o777, f"{member.name}: mode {oct(member.mode)}"
            if member.isfile():
                extracted = tar.extractfile(member)
                assert extracted is not None and extracted.read() == (staged / member.name).read_bytes(), member.name
        link = next(member for member in members if member.name == "lib/libfoo.so.1")
        assert link.issym() and link.linkname == "libfoo.so.1.2", "a library's link is kept as a link"
    again = scratch / "again"
    again.mkdir()
    ran, out, sbom = bundle_of(again, staged, ldd_text(staged, sections), packages)
    assert ran.returncode == 0 and out.read_bytes() == made and sbom.read_bytes() == sbom_made, "a bundle is the same bytes"
    bom = json.loads(sbom_made)
    assert bom["bomFormat"] == "CycloneDX" and bom["specVersion"] == "1.6", bom
    assert {"name": "freebsd:abi", "value": BSD_ABI} in bom["metadata"]["properties"], bom["metadata"]
    assert [(c["name"], c["version"], c["licenses"]) for c in bom["components"]] == [
        ("libfoo", "1.2_1", [{"expression": "ISC AND MIT"}]),
        ("node22", "22.15.1", [{"expression": "MIT"}]),
    ], bom["components"]
    (scratch / "bundle-asset").write_bytes(made)
    ran = intake(
        "licence-gate", "--input", held / "out" / "sbom.json", "--deny", ROOT / "deny.toml", "--texts", GATE / "texts",
        "--asset", held / "out" / "bundle.tar.gz", "--out", scratch / "gated",
    )
    assert ran.returncode == 0 and json.loads(ran.stdout)["asset"]["sha256"] == sha(made), ran.stderr
    # A ports licence the table does not hold asserts none, so the gate fails
    # closed; under `or` another licence may still carry the package.
    for name, licences, logic, expression in (
        ("unknown", ["GPLv2"], "single", "NOASSERTION"),
        ("either", ["MIT", "GPLv2"], "or", "MIT OR NOASSERTION"),
        ("empty", [], "single", "NOASSERTION"),
    ):
        place = scratch / name
        place.mkdir()
        staged_here, sections_here, packages_here = staged_runtime(place, licences=licences, logic=logic)
        if not licences:
            packages_here["packages"][0]["licenses"] = []
        ran, out, sbom = bundle_of(place, staged_here, ldd_text(staged_here, sections_here), packages_here)
        assert ran.returncode == 0, f"{name}: {ran.stderr[-600:]}"
        component = next(c for c in json.loads(sbom.read_text())["components"] if c["name"] == "node22")
        assert component["licenses"] == [{"expression": expression}], f"{name}: {component}"
    ran = intake(
        "licence-gate", "--input", scratch / "unknown/out/sbom.json", "--deny", ROOT / "deny.toml",
        "--texts", GATE / "texts", "--asset", scratch / "unknown/out/bundle.tar.gz", "--out", scratch / "ungated",
    )
    assert ran.returncode == 1 and "NOASSERTION" in ran.stderr, ran.stderr

    def refused(name: str, words: tuple[str, ...], change=None, ldd=None, abi: str = BSD_ABI) -> None:
        place = scratch / f"refused-{name}"
        place.mkdir()
        staged_here, sections_here, packages_here = staged_runtime(place)
        if change is not None:
            change(staged_here, sections_here, packages_here)
        text_here = ldd(staged_here, sections_here) if ldd is not None else ldd_text(staged_here, sections_here)
        ran, out, _ = bundle_of(place, staged_here, text_here, packages_here, abi)
        assert ran.returncode == 1 and "Traceback" not in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-600:]!r}"
        for word in words:
            assert word in ran.stderr, f"{name}: {ran.stderr[-600:]!r} lacks {word!r}"
        assert not list(out.parent.iterdir()), f"{name}: a refused bundle left a file"
        return ran

    def needs(line: str):
        return lambda staged_here, sections_here, packages_here: sections_here["bin/node"].append(line)

    root_of = os.path.realpath
    # The evaluator's case: a library outside the bundle and the base system.
    refused("local", ("libbar.so.2", "cannot be bundled"), needs(f"libbar.so.2 => /usr/local/lib/libbar.so.2 {LDD_ADDRESS}"))
    refused("missing", ("libbaz.so.1", "cannot be bundled"), needs("libbaz.so.1 => not found (0)"))
    refused("elsewhere", ("libq.so.1", "cannot be bundled"), needs(f"libq.so.1 => /opt/lib/libq.so.1 {LDD_ADDRESS}"))
    refused("unstaged", ("libz.so.6", "cannot be bundled"), lambda s, sections_here, p: sections_here["bin/node"].append(
        f"libz.so.6 => {root_of(s)}/lib/libz.so.6 {LDD_ADDRESS}"))
    refused("escaping", ("libz.so.6", "cannot be bundled"), lambda s, sections_here, p: sections_here["bin/node"].append(
        f"libz.so.6 => {root_of(s)}/lib/../../elsewhere/libz.so.6 {LDD_ADDRESS}"))
    refused("unneeded", ("lib/libextra.so.1", "no ldd line needs"),
            lambda s, se, p: (s / "lib/libextra.so.1").write_bytes(b"not loaded\n"))
    refused("unpackaged", ("lib/libfoo.so.1.2", "no listed package installs"),
            lambda s, se, p: p["packages"][1]["files"].remove("/usr/local/lib/libfoo.so.1.2"))
    refused("unparsed", ("does not parse",), needs("libfoo.so.1 -> somewhere"))
    # What ldd printed is the job's, so a refusal shows it escaped.
    ran = refused("escape", ("libq.so\\u001b[31m", "cannot be bundled"), needs("libq.so\x1b[31m => not found (0)"))
    assert "\x1b" not in ran.stderr, ran.stderr[-600:]
    ran = refused("return", ("does not parse", "\\u000d"), needs(f"libq.so.1 => /opt/lib/libq.so.1 {LDD_ADDRESS}\r"))
    assert "\r" not in ran.stderr, ran.stderr[-600:]
    # A listed path installs a staged file only at its place under /usr/local/.
    refused("elsewhere-installed", ("bin/node", "no listed package installs"),
            lambda s, se, p: p["packages"][0]["files"].__setitem__(0, "/home/other/bin/node"))
    refused("unsectioned", ("lib/libfoo.so.1.2", "ldd printed nothing"), lambda s, sections_here, p: sections_here.pop(
        "lib/libfoo.so.1.2"))
    refused("foreign", ("not a staged",), ldd=lambda s, se: ldd_text(s, se) + "/usr/local/bin/other:\n")
    refused("again", ("twice",), ldd=lambda s, se: ldd_text(s, se) + ldd_text(s, {"bin/node": se["bin/node"]}))
    refused("loose", ("does not parse",), ldd=lambda s, se: "ldd says hello\n" + ldd_text(s, se))
    refused("abi", ("ABI",), abi="FreeBSD:14:i386")
    refused("field", ("holds", "origin"), lambda s, se, p: p["packages"][0].update(origin="www/node22"))
    refused("single", ("single",), lambda s, se, p: p["packages"][0].update(licenses=["MIT", "ISCL"]))
    refused("logic", ("logic",), lambda s, se, p: p["packages"][0].update(logic="dual"))
    refused("relative", ("absolute",), lambda s, se, p: p["packages"][0]["files"].append("usr/local/bin/x"))
    refused("licence-dir", ("licenses/stray", "no listed package"),
            lambda s, se, p: (s / "licenses/stray").mkdir())
    refused("top", ("share",), lambda s, se, p: (s / "share").mkdir())
    refused("programs", ("one program",), lambda s, se, p: (s / "bin/second").write_bytes(b"#!/bin/sh\n"))
    refused("fifo", ("fifo",), lambda s, se, p: os.mkfifo(s / "lib/pipe"))
    # A staged name is the job's, so a refusal shows it escaped.
    ran = refused("fifo-escape", ("pipe\\u001b[31m", "fifo"), lambda s, se, p: os.mkfifo(s / "lib/pipe\x1b[31m"))
    assert "\x1b" not in ran.stderr, ran.stderr[-600:]
    refused("twice-named", ("twice",), lambda s, se, p: p["packages"].append(dict(p["packages"][1])))


PKG_URL = "https://pkg.freebsd.org/" + BSD_ABI + "/latest/All/{}.pkg"


def lay_out_runtime(place: pathlib.Path, package: bytes, url: str, made: tuple[bytes, dict, dict]) -> None:
    """What the FreeBSD step leaves for one runtime: the pinned package, the
    address it came from, the bundle, the client's listing of it and its SBOM."""
    place.mkdir(parents=True, exist_ok=True)
    bundle, sbom, listing = made
    (place / "artifact").write_bytes(package)
    (place / "fetched.json").write_text(json.dumps({"chain": [url]}))
    (place / "repacked").write_bytes(bundle)
    (place / "unpack-check.json").write_text(json.dumps(listing))
    (place / "sbom.json").write_text(json.dumps(sbom))


def check_freebsd_python(scratch: pathlib.Path) -> None:
    """A `uvx` agent runs on FreeBSD on the CPython package the reviewer pins,
    bundled on FreeBSD, installed as that bundle under a passing licence gate;
    a runtime that is not the pinned package or fails the gate drops only
    the FreeBSD target."""
    scratch = scratch / "freebsd-python"
    observed = scratch / "observed"
    package = b"the python312 package\n"
    url = PKG_URL.format("python312-3.12.7")
    made = bundled(scratch / "made", program=f"python{PYTHON}", package="python312", python=True)
    failing = bundled(scratch / "psf", program=f"python{PYTHON}", package="python312", python=True, licences=["PSFL"])
    agents, choices = [], {}
    for agent, runtime, chain in (
        ("bsd-agent", made, url),
        ("psf-agent", failing, url),
        ("other-agent", made, url),
        ("moved-agent", made, "https://pkg.freebsd.org/elsewhere.pkg"),
    ):
        agents.append({"id": agent, "name": agent, "version": "1.4.0", "license": "MIT",
                       "distribution": {"uvx": {"package": "pyagent==1.4.0", "args": ["acp"]}}})
        choices[agent] = {"key": agent.removesuffix("-agent")}
        lay_out_uvx(observed, agent, "freebsd-x86_64", [own()], built=(package, runtime[2]), interpreter_chain=[chain])
        lay_out_runtime(observed / agent / "freebsd-x86_64/interpreter",
                        b"other bytes" if agent == "other-agent" else package, chain, runtime)
    pins = json.loads(INTERPRETERS.read_text())
    pins["freebsd-x86_64"] = {"url": url, "sha256": sha(package), "python": PYTHON, "abi": BSD_ABI}
    registry = written(scratch, "registry.json", {"version": "1.0.0", "agents": agents})
    data, assets = review(
        scratch, "--interpreters", written(scratch, "pins.json", pins), "--texts", GATE / "texts",
        registry=registry, choices=written(scratch, "choices.json", choices),
    )
    [entry] = data["entries"]
    [target] = entry["targets"]
    assert entry["key"] == "bsd" and target["platform"] == "freebsd-x86_64", entry
    assert target["cmd"] == f"bin/python{PYTHON}" and target["interpreter_args"] == ["-I"], target["cmd"]
    runtime = target["artifacts"][0]
    bundle, _, listing = made
    assert (runtime["url"], runtime["sha256"], runtime["size"]) == (url, sha(package), len(package)), runtime["url"]
    assert runtime["format"] == "tar.gz" and runtime["files"] == listing["files"], runtime["format"]
    assert "kept" not in runtime and "mirror_licence" not in runtime, "every file of the bundle is kept"
    assert runtime["replaced_by"] == {"sha256": sha(bundle), "size": len(bundle)}, runtime.get("replaced_by")
    assert runtime["licence_gate"]["passed"] is True, runtime["licence_gate"]
    assert (assets / sha(bundle)).read_bytes() == bundle, "the bundle is kept as an asset by its hash"
    assert target["artifacts"][1]["format"] == "wheel", "the agent's wheels install beside it"
    dropped(data, "psf-agent", "freebsd-x86_64", "the interpreter", "PSF-2.0", "not on deny.toml's list")
    dropped(data, "other-agent", "freebsd-x86_64", "the interpreter", "not the bytes")
    dropped(data, "moved-agent", "freebsd-x86_64", "the interpreter", "pinned address")
    # The diff no longer says an interpreter is never mirrored where one ships.
    ran = intake("diff", scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    [said] = [line for line in ran.stdout.splitlines() if line.startswith("- not mirrored")]
    assert "except" in said and "` freebsd-x86_64 `" in said and "installs as its bundle" in said, said
    beside = [(record["sha256"], (assets / record["sha256"]).read_bytes())
              for record in [runtime["licence_gate"]["notice"], *runtime["licence_gate"]["licences"]]]
    ran = publish_checked(scratch, "freebsd-python", data, (sha(bundle), bundle), *beside)
    assert ran.returncode == 0, f"a gated bundle was refused:\n{ran.stdout}{ran.stderr}"
    publish_refuses(publish_checked(scratch, "freebsd-package", data, (sha(package), package), *beside), "licence-gate")
    # With no licence texts nothing could ship beside a bundle, so it has none.
    textless, _ = review(scratch, "--interpreters", scratch / "pins.json", name="textless.json",
                         registry=registry, choices=scratch / "choices.json")
    dropped(textless, "bsd-agent", "freebsd-x86_64", "no licence texts were given")

    for name, changed, words in (
        ("host", {"url": url.replace("pkg.freebsd.org", "pkg.example.org")}, "pkg.freebsd.org"),
        ("suffix", {"url": url.replace(".pkg", ".txz")}, ".pkg"),
        ("abi-path", {"url": url.replace(BSD_ABI, "FreeBSD:13:amd64")}, BSD_ABI),
        ("query", {"url": url + "?x=1"}, "plain"),
        ("abi", {"abi": "FreeBSD:14:aarch64"}, "ABI"),
        ("no-abi", {"abi": None}, "ABI"),
        ("extra", {"mirror": True}, "mirror"),
    ):
        held = copy.deepcopy(pins)
        held["freebsd-x86_64"].update(changed)
        held["freebsd-x86_64"] = {key: value for key, value in held["freebsd-x86_64"].items() if value is not None}
        ran = intake(
            "review", "--registry", registry, "--choices", scratch / "choices.json", "--runners", RUNNERS,
            "--observed", observed, "--deny", ROOT / "deny.toml", "--assets", scratch / "none",
            "--out", scratch / f"pins-{name}.json", "--interpreters", written(scratch, f"pins-{name}-in.json", held),
        )
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-600:]!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not (scratch / f"pins-{name}.json").exists(), ran.stderr
    linux = copy.deepcopy(pins)
    linux["linux-x86_64"]["abi"] = BSD_ABI
    ran = intake(
        "review", "--registry", registry, "--choices", scratch / "choices.json", "--runners", RUNNERS,
        "--observed", observed, "--deny", ROOT / "deny.toml", "--assets", scratch / "none",
        "--out", scratch / "pins-linux.json", "--interpreters", written(scratch, "pins-linux-in.json", linux),
    )
    assert ran.returncode == 1 and "abi" in ran.stderr and "Traceback" not in ran.stderr, ran.stderr


def check_freebsd_node(scratch: pathlib.Path, signers: Signers) -> None:
    """An npm agent runs on FreeBSD on the Node package the reviewer pins,
    in the major line of the run's Node, bundled on FreeBSD and installed as
    that bundle under a passing licence gate; one failing the gate drops only
    the FreeBSD target."""
    scratch = scratch / "freebsd-node"
    observed = scratch / "observed"
    lay_out_node(observed, signers.signed())
    agents: list[dict] = []
    choices: dict = {}
    platforms = (*NODE_BUILT, "freebsd-x86_64")
    npm_case(observed, agents, choices, "npx-agent", npm_tree(), platforms=platforms)
    package = b"the node22 package\n"
    url = PKG_URL.format("node22-22.15.1")
    made = bundled(scratch / "made")
    lay_out_runtime(observed / "node/freebsd-x86_64", package, url, made)
    registry = written(scratch, "registry.json", {"version": "1.0.0", "agents": agents})
    choices_path = written(scratch, "choices.json", choices)
    freebsd = {"url": url, "sha256": sha(package), "abi": BSD_ABI, "version": "22.15.1"}
    pin = written(scratch, "node.json", {"version": NODE_VERSION, "freebsd-x86_64": freebsd})
    data, assets = review(scratch, "--node", pin, "--node-keys", signers.keys, "--texts", GATE / "texts",
                          registry=registry, choices=choices_path)
    [entry] = data["entries"]
    targets = {target["platform"]: target for target in entry["targets"]}
    assert sorted(targets) == sorted(platforms), sorted(targets)
    target = targets["freebsd-x86_64"]
    assert target["cmd"] == "bin/node", target["cmd"]
    [runtime] = target["artifacts"]
    bundle, _, listing = made
    assert (runtime["url"], runtime["sha256"], runtime["size"]) == (url, sha(package), len(package)), runtime["url"]
    assert runtime["format"] == "tar.gz" and runtime["files"] == listing["files"], runtime["format"]
    assert "kept" not in runtime, "every file of the bundle is kept"
    assert runtime["replaced_by"] == {"sha256": sha(bundle), "size": len(bundle)}, runtime.get("replaced_by")
    assert (assets / sha(bundle)).read_bytes() == bundle, "the bundle is kept as an asset by its hash"
    assert "kept" in targets["linux-x86_64"]["artifacts"][0], "Node's own builds are kept as before"
    # The diff no longer says Node is never mirrored where its bundle ships.
    ran = intake("diff", scratch / "review.json")
    assert ran.returncode == 0, ran.stderr
    [said] = [line for line in ran.stdout.splitlines() if line.startswith("- not mirrored")]
    assert "except" in said and "` freebsd-x86_64 `" in said and "installs as its bundle" in said, said

    failing = bundled(scratch / "failing", licences=["GPLv2"])
    (observed / "node/freebsd-x86_64/sbom.json").write_text(json.dumps(failing[1]))
    held, _ = review(scratch, "--node", pin, "--node-keys", signers.keys, "--texts", GATE / "texts",
                     name="failing.json", registry=registry, choices=choices_path)
    dropped(held, "npx-agent", "freebsd-x86_64", "NOASSERTION")
    assert sorted(t["platform"] for t in held["entries"][0]["targets"]) == sorted(NODE_BUILT), "only FreeBSD drops"
    bare = written(scratch, "bare.json", {"version": NODE_VERSION})
    held, _ = review(scratch, "--node", bare, "--node-keys", signers.keys, name="bare.json",
                     registry=registry, choices=choices_path)
    dropped(held, "npx-agent", "freebsd-x86_64", "Node publishes no build", "no FreeBSD package")

    for name, changed, words in (
        ("major", {"version": "24.1.0"}, "major"),
        ("floor", {"version": "22.14.0"}, "22.15.0"),
        ("loose", {"version": "v22.15.1"}, "X.Y.Z"),
        ("host", {"url": url.replace("pkg.freebsd.org", "pkg.example.org")}, "pkg.freebsd.org"),
        ("suffix", {"url": url.replace(".pkg", ".txz")}, ".pkg"),
        ("abi", {"abi": "FreeBSD:14"}, "ABI"),
        ("digest", {"sha256": "A" * 64}, "sha256"),
        ("extra", {"mirror": True}, "mirror"),
    ):
        pinned = {**freebsd, **changed}
        path = written(scratch, f"node-{name}.json", {"version": NODE_VERSION, "freebsd-x86_64": pinned})
        ran = intake(
            "review", "--registry", registry, "--choices", choices_path, "--runners", RUNNERS,
            "--observed", observed, "--deny", ROOT / "deny.toml", "--assets", scratch / "none",
            "--out", scratch / f"pin-{name}.json", "--node", path, "--node-keys", signers.keys,
        )
        assert ran.returncode == 1 and words in ran.stderr, f"{name}: {ran.returncode} {ran.stderr[-600:]!r} lacks {words!r}"
        assert "Traceback" not in ran.stderr and not (scratch / f"pin-{name}.json").exists(), ran.stderr


# What the job leaves for a skipped script of an npm agent's own package.
TOP_SCRIPT = (TOP, "postinstall")
BUILT_HOSTS = ["registry.npmjs.org", "github.com"]


def script_place(observed: pathlib.Path, agent: str, platform: str, location: str, event: str) -> pathlib.Path:
    """Where the job leaves what it saw of one script: under the target's
    `scripts/`, by the SHA-256 of `<location> <event>`."""
    return observed / agent / platform / "scripts" / sha(f"{location} {event}".encode())


def package_files(tree: dict, **added: bytes) -> dict[str, tuple[bytes, bool]]:
    """The agent's own package directory after a run of its script: the files
    its tarball wrote, and `added`, by path."""
    files = dict(tree["files"][TOP])
    files.update({path.replace("__", "/"): (content, False) for path, content in added.items()})
    return files


def tree_of(files: dict[str, tuple[bytes, bool]]) -> bytes:
    """The listing `tree` prints of a directory holding `files`."""
    listed_files = sorted(
        ({"exec": runs, "path": path, "sha256": sha(content), "size": len(content)}
         for path, (content, runs) in files.items()),
        key=lambda item: item["path"],
    )
    return json.dumps({"files": listed_files}, sort_keys=True, separators=(",", ":")).encode()


def packed_as_packing(files: dict[str, tuple[bytes, bool]], links: dict[str, str] | None = None) -> bytes:
    """`files` as a deterministic tar.gz, written as `Packing` writes one: a
    GNU tar of each directory above a file and each file, in order, owner 0
    and mtime 0, in one gzip stream at level 9 naming no file; with `links`,
    each a symbolic link to its target, which no build `pack` writes holds."""
    names: dict[str, tuple[bytes, bool] | str | None] = {}
    for path in sorted([*files, *(links or {})]):
        parts = path.split("/")
        for depth in range(1, len(parts)):
            names.setdefault("/".join(parts[:depth]), None)
        names[path] = files[path] if path in files else (links or {})[path]
    sink = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=sink, compresslevel=9, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w", format=tarfile.GNU_FORMAT, encoding="utf-8") as tar:
            for name, member in sorted(names.items(), key=lambda item: tuple(item[0].split("/"))):
                info = tarfile.TarInfo(name)
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = 0
                if member is None:
                    info.type, info.mode = tarfile.DIRTYPE, 0o755
                    tar.addfile(info)
                elif isinstance(member, str):
                    info.type, info.mode, info.linkname = tarfile.SYMTYPE, 0o777, member
                    tar.addfile(info)
                else:
                    content, runs = member
                    info.mode, info.size = (0o755 if runs else 0o644), len(content)
                    tar.addfile(info, io.BytesIO(content))
    return sink.getvalue()


def lay_out_script(
    place: pathlib.Path,
    first: bytes | None,
    second: bytes | None,
    *,
    hosts: object = BUILT_HOSTS,
    built: dict[str, tuple[bytes, bool]] | None = None,
    sboms: pathlib.Path | None = None,
    sbom: bytes | None = None,
    packed=packed_as_packing,
    listed_as=None,
) -> bytes | None:
    """What the job leaves for one script: its two runs' listings, the hosts
    it reached, and, where it built, the deterministic tar.gz of its package
    and that tree's listing, with the build's SBOM by the archive's hash;
    `packed` writes the archive of the built files, and `listed_as`, where
    given, the files `built.json` lists instead of them. Returns the archive."""
    place.mkdir(parents=True)
    for name, listing in (("run-1.json", first), ("run-2.json", second)):
        if listing is not None:
            (place / name).write_bytes(listing)
    (place / "hosts.json").write_text(json.dumps({"hosts": hosts}))
    if built is None:
        return None
    archive_bytes = packed(built)
    (place / "built.tar.gz").write_bytes(archive_bytes)
    (place / "built.json").write_bytes(tree_of(built if listed_as is None else listed_as(built)))
    if sboms is not None and sbom is not None:
        (sboms / f"{sha(archive_bytes)}.json").write_bytes(sbom)
    return archive_bytes


BUILDS: dict = {}


def builds_review(scratch: pathlib.Path, signers: Signers) -> tuple[dict, pathlib.Path, dict]:
    """One review of npm agents whose own package's postinstall script the
    job ran twice, or built, on some targets; returns the data, the assets
    and the archives the job built, by agent and platform. Reviewed once and
    kept, as every check of it reads the same run."""
    held = BUILDS
    if "data" in held:
        return held["data"], held["assets"], held["archives"]
    scratch = scratch / "npm-builds"
    observed = scratch / "observed"
    sboms = scratch / "sboms"
    sboms.mkdir(parents=True)
    lay_out_node(observed, signers.signed())
    agents: list[dict] = []
    choices: dict = {}
    archives: dict[tuple[str, str], bytes | None] = {}
    passing_sbom = (GATE / "sbom.json").read_bytes()
    failing = json.loads(passing_sbom)
    failing["components"][0]["licenses"] = [{"expression": "Unlicense"}]
    failing_sbom = json.dumps(failing).encode()

    def agent(agent_id: str, command: str, laid: dict[str, dict]) -> dict:
        tree = npm_tree(top=top_manifest(scripts={"postinstall": command}))
        npm_case(observed, agents, choices, agent_id, tree)
        same = tree_of(package_files(tree, **{"build__done": b"1\n"}))
        for platform, kept in laid.items():
            first = kept.pop("first", same)
            second = kept.pop("second", same)
            built = kept.pop("built", None)
            if built is not None:
                built = package_files(tree, **{"build__Release__addon.node": f"\x7fELF {agent_id} {platform}\n".encode()})
            archives[(agent_id, platform)] = lay_out_script(
                script_place(observed, agent_id, platform, *TOP_SCRIPT), first, second,
                built=built, sboms=sboms, **kept,
            )
        return tree

    differs = b'{"files":[]}'
    # A compiler's script is a build by rule, whatever its runs leave.
    agent("gyp-npm", "node-gyp rebuild", {
        "linux-x86_64": {"built": True, "sbom": passing_sbom},
        "darwin-aarch64": {},
    })
    # Runs that leave other listings are a build; ones that agree are run
    # locally, except where every script is built.
    agent("differ-npm", "node setup.js", {
        "linux-x86_64": {"second": differs},
        "darwin-aarch64": {},
        "windows-x86_64": {},
    })
    # A build the gate refuses, or has no input for, drops its target.
    agent("ungated-npm", "node setup.js", {
        "linux-x86_64": {"second": differs, "built": True},
        "darwin-aarch64": {"second": differs, "built": True, "sbom": failing_sbom},
        "windows-x86_64": {"built": True, "sbom": passing_sbom},
    })
    # Commands that run a build tool however it is reached: through a
    # package manager, an expansion, a glob, a versioned or scripted name;
    # ones that cannot be split, or hold what could hide what they run; and
    # one that names a file like one and runs none.
    for agent_id, command in (
        ("npxgyp-npm", "npx node-gyp@10 rebuild"),
        ("prebuild-npm", "prebuild-install || node build.js"),
        ("path-npm", "./node_modules/.bin/node-gyp rebuild"),
        ("subst-npm", "node -e \"$(cmake --version)\""),
        ("quote-npm", "node \"setup.js"),
        ("braced-npm", "ma${u}ke"),
        ("glob-npm", "mak?"),
        ("versioned-npm", "gcc-12 x.c"),
        ("gypjs-npm", "node node_modules/node-gyp/bin/node-gyp.js rebuild"),
        ("run-npm", "npm run build"),
        ("triplet-npm", "x86_64-linux-gnu-gcc x.c"),
        ("crossversioned-npm", "aarch64-linux-gnu-g++-12 x.cc"),
        ("dotversioned-npm", "gcc-12.2.0 x.c"),
        ("pnpx-npm", "pnpx tsc"),
        ("bunx-npm", "bunx tsc"),
        ("bidi-npm", "node x.js \u202e;rm"),
        ("nel-npm", "node x.js \x85rm"),
        ("near-npm", "node remake.js"),
    ):
        agent(agent_id, command, {"linux-x86_64": {}})
    # Builds review does not take as a build `pack` wrote: one compressed
    # otherwise, one with bytes after its gzip stream, one holding a link,
    # and one whose listing leaves a file out.
    def recompressed(files: dict) -> bytes:
        return gzip.compress(gzip.decompress(packed_as_packing(files)), compresslevel=1, mtime=0)

    for agent_id, kept in (
        ("recompressed-npm", {"packed": recompressed}),
        ("trailing-npm", {"packed": lambda files: packed_as_packing(files) + b"\0"}),
        ("linked-npm", {"packed": lambda files: packed_as_packing(files, {"build/link": "done"})}),
        ("omitted-npm", {"listed_as": lambda files: {
            path: held for path, held in files.items() if not path.endswith("addon.node")
        }}),
    ):
        agent(agent_id, "node-gyp rebuild", {"linux-x86_64": {"built": True, "sbom": passing_sbom, **kept}})
    # A run the job left no listing of, and hosts that are not hosts.
    agent("norun-npm", "node setup.js", {"linux-x86_64": {"second": None}})
    agent("badhost-npm", "node setup.js", {"linux-x86_64": {"hosts": ["10.0.0.1"]}})
    registry = written(scratch, "registry.json", {"version": "1.0.0", "agents": agents})
    choices_path = written(scratch, "choices.json", choices)
    data, assets = npm_review(scratch, signers, registry, choices_path, "--sboms", sboms, "--texts", GATE / "texts")
    held.update(data=data, assets=assets, archives=archives)
    return data, assets, archives


def built_targets(data: dict, key: str) -> dict[str, dict]:
    entry = next((entry for entry in data["entries"] if entry["key"] == key), {"targets": []})
    return {target["platform"]: target for target in entry["targets"]}


def top_script(target: dict) -> dict:
    return next(script for script in target["scripts"] if (script["package"], script["event"]) == TOP_SCRIPT)


def check_npm_build_by_rule(scratch: pathlib.Path, signers: Signers) -> None:
    """A script that runs `node-gyp` is a build even where its two runs
    agree: built where the job built it and the gate passes, and its target
    dropped where the job left no build."""
    data, assets, archives = builds_review(scratch, signers)
    dropped(data, "gyp-npm", "darwin-aarch64", "postinstall", TOP, "is a build", "node-gyp", "left no build")
    targets = built_targets(data, "gyp")
    assert sorted(targets) == ["linux-x86_64", "windows-x86_64"], sorted(targets)
    built = archives[("gyp-npm", "linux-x86_64")]
    script = top_script(targets["linux-x86_64"])
    assert script["disposition"] == "built", script
    assert script["replaced_by"] == {"sha256": sha(built), "size": len(built)}, script
    assert script["licence_gate"]["passed"] is True, script["licence_gate"]
    assert script["licence_gate"]["asset"] == script["replaced_by"], script["licence_gate"]
    assert (assets / sha(built)).read_bytes() == built, "the build is kept as an asset by its hash"
    assert "command" not in script and "result" not in script, script
    # Where the job saw nothing of the script, it stays for the reviewer.
    assert top_script(targets["windows-x86_64"])["disposition"] is None, targets["windows-x86_64"]["scripts"]
    for agent_id in (
        "npxgyp-npm", "prebuild-npm", "path-npm", "subst-npm", "quote-npm", "braced-npm", "glob-npm",
        "versioned-npm", "gypjs-npm", "run-npm", "triplet-npm", "crossversioned-npm", "dotversioned-npm", "pnpx-npm", "bunx-npm", "bidi-npm", "nel-npm",
    ):
        dropped(data, agent_id, "linux-x86_64", "is a build", "left no build")
    dropped(data, "quote-npm", "linux-x86_64", "cannot be split")
    for agent_id in ("braced-npm", "glob-npm"):
        dropped(data, agent_id, "linux-x86_64", "a shell expands")
    for agent_id in ("bidi-npm", "nel-npm"):
        dropped(data, agent_id, "linux-x86_64", "could hide what it runs")
    near = top_script(built_targets(data, "near")["linux-x86_64"])
    assert near["disposition"] == "local" and near["command"] == "node remake.js", near


def check_npm_build_by_difference(scratch: pathlib.Path, signers: Signers) -> None:
    """A script whose two runs leave other listings of its package, or whose
    run left none, is a build; one whose runs agree is run locally, with the
    command it runs, the hosts it reached and the listing both runs left."""
    data, assets, _ = builds_review(scratch, signers)
    dropped(data, "differ-npm", "linux-x86_64", "postinstall", TOP, "is a build", "differ", "left no build")
    dropped(data, "norun-npm", "linux-x86_64", "is a build", "run-2.json", "left no build")
    dropped(data, "badhost-npm", "linux-x86_64", "10.0.0.1", "host")
    tree = npm_tree(top=top_manifest(scripts={"postinstall": "node setup.js"}))
    agreed = tree_of(package_files(tree, **{"build__done": b"1\n"}))
    script = top_script(built_targets(data, "differ")["darwin-aarch64"])
    assert script == {
        "command": "node setup.js",
        "disposition": "local",
        "event": "postinstall",
        "hosts": sorted(BUILT_HOSTS),
        "package": TOP,
        "result": {"sha256": sha(agreed), "size": len(agreed)},
    }, script
    assert (assets / sha(agreed)).read_bytes() == agreed, "the agreed listing is kept as an asset by its hash"
    # What one run holds of the listings its local scripts agreed on is
    # bounded; past it the target is dropped and nothing of it is kept.
    module = load_intake()
    gate = module.Gate(set(), None, None)
    observed = scratch / "npm-builds" / "observed"
    item = {"manifest": {"scripts": {"postinstall": "node setup.js"}}}
    undecided = {"package": TOP, "event": "postinstall", "disposition": None}
    module.GATE_HELD_BYTES = len(agreed) - 1
    held_assets: dict = {}
    try:
        module.classed(observed, "differ-npm", "darwin-aarch64", item, undecided, gate, held_assets)
    except module.Dropped as failed:
        assert str(module.GATE_HELD_BYTES) in str(failed) and "holds" in str(failed), str(failed)
    else:
        raise AssertionError("a local script's listing past what a run holds was kept")
    assert not held_assets and gate.held_bytes == 0, "nothing of a refused listing is kept"
    module.GATE_HELD_BYTES = len(agreed)
    local = module.classed(observed, "differ-npm", "darwin-aarch64", item, undecided, gate, held_assets)
    assert local["disposition"] == "local" and held_assets == {sha(agreed): agreed}, local
    assert gate.held_bytes == len(agreed), gate.held_bytes
    ran = intake("diff", scratch / "npm-builds" / "review.json")
    assert ran.returncode == 0, ran.stderr
    [line] = [line for line in ran.stdout.splitlines() if "**undecided**" not in line and "node setup.js" in line
              and "local" in line]
    for said in ("github.com", "registry.npmjs.org", sha(agreed)):
        assert said in line, line


def check_npm_build_on_target(scratch: pathlib.Path, signers: Signers) -> None:
    """On Windows a script is never run locally, even where its two runs
    agree: it is a build, and its target is dropped where the job left none."""
    data, _, _ = builds_review(scratch, signers)
    dropped(data, "differ-npm", "windows-x86_64", "postinstall", "is a build", "windows-x86_64", "left no build")
    assert "windows-x86_64" not in built_targets(data, "differ"), "no Windows target is listed with a local script"


def check_npm_build_gated(scratch: pathlib.Path, signers: Signers) -> None:
    """A build is held to the licence gate: with no SBOM, or one the gate
    refuses, its target is dropped with the gate's reason, and one that
    passes lists its script as built; the publish job takes that build."""
    data, assets, archives = builds_review(scratch, signers)
    dropped(data, "ungated-npm", "linux-x86_64", "no lockfile or SBOM was given")
    dropped(data, "ungated-npm", "darwin-aarch64", "Unlicense")
    script = top_script(built_targets(data, "ungated")["windows-x86_64"])
    built = archives[("ungated-npm", "windows-x86_64")]
    assert script["disposition"] == "built" and script["replaced_by"]["sha256"] == sha(built), script
    held = scratch / "npm-builds" / "publish"
    (held / "assets").mkdir(parents=True)
    record = script["licence_gate"]
    # The listing each local script's runs agreed on is published as the
    # catalog's own data, as a lock is.
    results = [
        script["result"]["sha256"]
        for entry in data["entries"]
        for target in entry["targets"]
        for script in target.get("scripts", [])
        if script.get("disposition") == "local"
    ]
    assert results, "the review lists a local script"
    for named in [sha(built), record["notice"]["sha256"], *(licence["sha256"] for licence in record["licences"]),
                  *results]:
        (held / "assets" / named).write_bytes((assets / named).read_bytes())
    (held / "merged.json").write_text(json.dumps(data))
    ran = intake("publish-check", "--merged", held / "merged.json", "--assets", held / "assets")
    assert ran.returncode == 0, f"a gated build or a local script's listing was refused:\n{ran.stdout}{ran.stderr}"
    # A local script whose command holds what could hide what it runs is
    # not rendered.
    module = load_intake()
    entry, target = next(
        (entry, target) for entry in data["entries"] for target in entry["targets"]
        if any(script.get("disposition") == "local" for script in target.get("scripts", []))
    )
    # The scripts the reviewer left undecided are decided, so only the
    # local one is in question.
    target = copy.deepcopy(target)
    for script in target["scripts"]:
        if script.get("disposition") is None:
            script["disposition"] = "inert"
    module.npm_checked(entry, target, "the entry")
    for hiding in ("\u202e", "\x85"):
        hidden_target = copy.deepcopy(target)
        next(script for script in hidden_target["scripts"] if script.get("disposition") == "local")["command"] = (
            f"node x.js {hiding};rm"
        )
        try:
            module.npm_checked(entry, hidden_target, "the entry")
        except module.Refused as refused:
            assert "could hide what it runs" in str(refused), str(refused)
        else:
            raise AssertionError(f"a local script's command holding {hiding!r} was taken")
    # A build whose record gates other bytes than it is replaced by may not
    # be published.
    mismatched = copy.deepcopy(data)
    target = built_targets(mismatched, "ungated")["windows-x86_64"]
    top_script(target)["licence_gate"]["asset"]["sha256"] = "0" * 64
    (held / "mismatched.json").write_text(json.dumps(mismatched))
    ran = intake("publish-check", "--merged", held / "mismatched.json", "--assets", held / "assets")
    assert ran.returncode == 1 and sha(built) in ran.stderr and "no passing licence-gate record" in ran.stderr, (
        ran.stderr
    )


def check_npm_build_refused(scratch: pathlib.Path, signers: Signers) -> None:
    """A build is taken only as `pack` writes one, of files and directories
    alone, holding the files its listing gives: any other drops its target,
    the gate never reached."""
    data, _, _ = builds_review(scratch, signers)
    dropped(data, "recompressed-npm", "linux-x86_64", "built.tar.gz", "not written as `pack` writes")
    dropped(data, "trailing-npm", "linux-x86_64", "built.tar.gz", "not written as `pack` writes")
    dropped(data, "linked-npm", "linux-x86_64", "build/link", "neither a file nor a directory")
    dropped(data, "omitted-npm", "linux-x86_64", "built.json lists")
    for key in ("recompressed", "trailing", "linked", "omitted"):
        assert "linux-x86_64" not in built_targets(data, key), f"{key} is listed with a build review refuses"


def check_pack(scratch: pathlib.Path) -> None:
    """`pack` writes a package directory as the deterministic tar.gz review
    takes a build as, the same bytes each time."""
    root = scratch / "pack" / "package"
    files = {"package.json": (b"{}", False), "build/Release/addon.node": (b"\x7fELF\n", True)}
    for path, (content, runs) in files.items():
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_bytes(content)
        (root / path).chmod(0o755 if runs else 0o644)
    for name in ("one.tar.gz", "two.tar.gz"):
        ran = intake("pack", "--root", root, "--out", scratch / "pack" / name)
        assert ran.returncode == 0, ran.stderr
    made = (scratch / "pack" / "one.tar.gz").read_bytes()
    assert made == (scratch / "pack" / "two.tar.gz").read_bytes(), "a pack is the same bytes each time"
    assert made == packed_as_packing(files), "a pack is the archive review takes a build as"
    (root / "link").symlink_to("package.json")
    ran = intake("pack", "--root", root, "--out", scratch / "pack" / "linked.tar.gz")
    assert ran.returncode == 1 and "link" in ran.stderr and "Traceback" not in ran.stderr, ran.stderr


def check_uvx_sdist_built(scratch: pathlib.Path) -> None:
    """A source distribution the job built a wheel from is listed by its own
    address, hash and size and installs as that wheel, held to every check a
    downloaded one is and to the licence gate; one whose wheel fails the gate
    drops its target, and one with no wheel still waits for a build."""
    scratch = scratch / "uvx-sdist"
    observed = scratch / "observed"
    sboms = scratch / "sboms"
    sboms.mkdir(parents=True)
    source = b"the pyagent 1.4.0 source distribution\n"
    url = PYPI + "ee/ff/pyagent-1.4.0.tar.gz"
    def built_on(platform: str) -> tuple[bytes, dict]:
        # One wheel per platform, told apart by its hash.
        return wheel("pyagent", "1.4.0", console=CONSOLE, extra={"pyagent/platform.txt": platform.encode()})

    failing = json.loads((GATE / "sbom.json").read_text())
    failing["components"][0]["licenses"] = [{"expression": "Unlicense"}]
    for platform, sbom in (
        ("linux-x86_64", (GATE / "sbom.json").read_bytes()),
        ("darwin-aarch64", json.dumps(failing).encode()),
        ("windows-x86_64", None),
    ):
        lay_out_uvx(observed, "sbuilt-agent", platform, [sdist("pyagent-1.4.0.tar.gz", "pyagent", "1.4.0")])
        if sbom is None:
            continue
        held = observed / "sbuilt-agent" / platform / "wheels/0"
        (held / "built").mkdir(parents=True)
        (held / "artifact").write_bytes(source)
        (held / "fetched.json").write_text(json.dumps({"chain": [url]}))
        content, listing = built_on(platform)
        (held / "built/name.json").write_text(json.dumps({"file": OWN}))
        (held / "built/artifact").write_bytes(content)
        (held / "built/unpack-check.json").write_text(json.dumps(listing))
        (sboms / f"{sha(content)}.json").write_bytes(sbom)
    registry = written(scratch, "registry.json", {"version": "1.4.0", "agents": [{
        "id": "sbuilt-agent", "name": "sbuilt", "version": "1.4.0", "license": "MIT",
        "distribution": {"uvx": {"package": "pyagent==1.4.0", "args": ["acp"]}},
    }]})
    choices = written(scratch, "choices.json", {"sbuilt-agent": {"key": "sbuilt"}})
    data, assets = review(scratch, "--interpreters", pinned_interpreters(scratch), "--sboms", sboms,
                          "--texts", GATE / "texts", registry=registry, choices=choices)
    [entry] = data["entries"]
    targets = {target["platform"]: target for target in entry["targets"]}
    assert sorted(targets) == ["linux-x86_64"], sorted(targets)
    target = targets["linux-x86_64"]
    assert target["entry_point"] == "pyagent.acp:main", target["entry_point"]
    [_, artifact] = target["artifacts"]
    content, listing = built_on("linux-x86_64")
    assert (artifact["url"], artifact["sha256"], artifact["size"]) == (url, sha(source), len(source)), artifact
    assert artifact["format"] == "wheel" and artifact["files"] == listing["files"], artifact
    assert artifact["replaced_by"] == {"sha256": sha(content), "size": len(content)}, artifact.get("replaced_by")
    assert artifact["licence_gate"]["passed"] is True, artifact["licence_gate"]
    assert (assets / sha(content)).read_bytes() == content, "the built wheel is kept as an asset by its hash"
    dropped(data, "sbuilt-agent", "darwin-aarch64", "Unlicense")
    dropped(data, "sbuilt-agent", "windows-x86_64", "waits for a build at review")
    [windows] = [item for item in data["dropped"] if item.get("platform") == "windows-x86_64"]
    assert windows["build"] == [{"package": "pyagent", "url": url, "version": "1.4.0"}], windows


def main() -> int:
    failures = []
    with tempfile.TemporaryDirectory(prefix="intake-") as held:
        scratch = pathlib.Path(held)
        signers = Signers(scratch)
        data, assets = review(scratch)
        checks = [
            ("review", lambda: check_review(data, assets)),
            ("diff", lambda: check_diff(scratch, data)),
            ("hostile input", lambda: check_refuses_hostile_input(scratch)),
            ("same version", lambda: check_same_version(scratch, data)),
            ("hostile entries", lambda: check_hostile_entries(scratch)),
            ("full values", lambda: check_full_values(scratch, data)),
            ("escapes", lambda: check_escapes(scratch, data)),
            ("budget", lambda: check_budget(scratch)),
            ("render", lambda: check_render(scratch)),
            ("render refuses", lambda: check_render_refuses(scratch)),
            ("render previous and entries", lambda: check_render_previous_and_entries(scratch)),
            ("publish", lambda: check_publish(scratch)),
            ("publish replaced", lambda: check_publish_replaced(scratch)),
            ("licence gate refuses", lambda: check_licence_gate_refuses(scratch)),
            ("licence gate notice", lambda: check_licence_gate_notice(scratch)),
            ("licence gate review", lambda: check_licence_gate_review(scratch, data)),
            ("licence gate rereview", lambda: check_licence_gate_rereview(scratch, data)),
            ("uvx", lambda: check_uvx(scratch)),
            ("uvx interpreters", lambda: check_uvx_interpreters(scratch)),
            ("uvx tags", check_uvx_tags),
            ("uvx bomb", check_uvx_bomb),
            ("uvx render", lambda: check_uvx_render(scratch)),
            ("npm", lambda: check_npm(scratch, signers)),
            ("npm node", lambda: check_npm_node(scratch, signers)),
            ("npm tarballs", lambda: check_npm_tarballs(scratch)),
            ("npm render", lambda: check_npm_render(scratch)),
            ("repack", lambda: check_repack(scratch)),
            ("bz2 review", lambda: check_bz2_review(scratch)),
            ("bundle", lambda: check_bundle(scratch)),
            ("freebsd python", lambda: check_freebsd_python(scratch)),
            ("freebsd node", lambda: check_freebsd_node(scratch, signers)),
            ("npm build by rule", lambda: check_npm_build_by_rule(scratch, signers)),
            ("npm build by difference", lambda: check_npm_build_by_difference(scratch, signers)),
            ("npm build on target", lambda: check_npm_build_on_target(scratch, signers)),
            ("npm build refused", lambda: check_npm_build_refused(scratch, signers)),
            ("npm build gated", lambda: check_npm_build_gated(scratch, signers)),
            ("pack", lambda: check_pack(scratch)),
            ("uvx sdist built", lambda: check_uvx_sdist_built(scratch)),
        ]
        try:
            for name, check in checks:
                try:
                    check()
                except Exception as failed:  # every check reports, not just the first
                    failures.append(f"{name}: {type(failed).__name__}: {failed}")
        finally:
            signers.close()
    for failure in failures:
        print(f"    {failure}"[:2000], file=sys.stderr)
    if failures:
        return 1
    print("agent catalog intake fixtures passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
