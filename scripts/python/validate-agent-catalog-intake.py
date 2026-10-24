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

import argparse
import copy
import hashlib
import importlib.util
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
    dropped(data, "npx-agent", None, "binary")
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


def main() -> int:
    failures = []
    with tempfile.TemporaryDirectory(prefix="intake-") as held:
        scratch = pathlib.Path(held)
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
        ]
        for name, check in checks:
            try:
                check()
            except Exception as failed:  # every check reports, not just the first
                failures.append(f"{name}: {type(failed).__name__}: {failed}")
    for failure in failures:
        print(f"    {failure}"[:2000], file=sys.stderr)
    if failures:
        return 1
    print("agent catalog intake fixtures passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
