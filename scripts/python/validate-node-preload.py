#!/usr/bin/env -S python3 -I
"""The preload crucible starts an npm agent under, run on a real Node.

The preload is read from the Rust source that ships it, from between its two
marker lines, and written at the top of a version laid out as crucible's store
lays one out, in a scratch directory that is also the agent's `HOME`. The
agent's script requires an optional dependency inside a `try`, and requires
and imports a module named `planted`, which is installed in a `node_modules`
above the store and in `HOME/.node_modules`, where Node's walk up would find
it. Each planted copy writes a marker file when it runs. The script does the
same again in a worker and in a child started by `fork`.

It passes when every one of them refuses both, `require` with
`MODULE_NOT_FOUND` and `import` with `ERR_MODULE_NOT_FOUND`, naming nothing
but the specifier, while a module inside the tree still loads, no marker is
written, and a Node without `module.registerHooks` stops at the preload rather
than run the agent.

Without a `node` on `PATH` that has `module.registerHooks`, it says it skipped
and why, and passes. A path given as the one argument is read in place of the
shipped source, so a preload with its hook removed can be seen to fail.
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

SOURCE = pathlib.Path("crates/crucible-external-harnesses/src/release/npm_launch.rs")
BEGIN = "// node-preload: begin"
END = "// node-preload: end"
# Each Node run is bounded: one that hangs fails rather than stalls the gate.
TIMEOUT = 60

# The agent's own script, in the tree. It runs the probe here, in a worker and
# in a forked child, and prints what each saw as one JSON object.
AGENT = r"""
'use strict';
const path = require('node:path');
const { Worker } = require('node:worker_threads');
const { fork } = require('node:child_process');
const probe = require('./probe.cjs');

async function main() {
  const seen = { main: await probe() };
  seen.worker = await new Promise((resolve, reject) => {
    const worker = new Worker(path.join(__dirname, 'worker.cjs'));
    worker.once('message', resolve);
    worker.once('error', reject);
  });
  seen.fork = await new Promise((resolve, reject) => {
    const child = fork(path.join(__dirname, 'child.cjs'), [], { stdio: 'inherit' });
    child.once('message', resolve);
    child.once('error', reject);
  });
  process.stdout.write(JSON.stringify(seen));
}
main().catch((error) => {
  process.stderr.write(String(error && error.stack) + '\n');
  process.exit(2);
});
"""

PROBE = r"""
'use strict';
function caught(load) {
  try {
    load();
    return { code: 'loaded' };
  } catch (error) {
    return { code: error.code, message: error.message };
  }
}
module.exports = async function probe() {
  const seen = {
    optional: caught(() => require('optional-dep')),
    planted: caught(() => require('planted')),
    inside: caught(() => require('inside')),
  };
  try {
    await import('planted');
    seen.imported = { code: 'loaded' };
  } catch (error) {
    seen.imported = { code: error.code, message: error.message };
  }
  try {
    await import('inside');
    seen.imported_inside = { code: 'loaded' };
  } catch (error) {
    seen.imported_inside = { code: error.code, message: error.message };
  }
  return seen;
};
"""

WORKER = r"""
'use strict';
const { parentPort } = require('node:worker_threads');
require('./probe.cjs')().then((seen) => parentPort.postMessage(seen));
"""

CHILD = r"""
'use strict';
require('./probe.cjs')().then((seen) => process.send(seen, () => process.exit(0)));
"""

# A module that must never run: it leaves a marker where it was found.
PLANTED = r"""
require('node:fs').writeFileSync(require('node:path').join(%s, %s), 'ran');
module.exports = 'planted';
"""

INSIDE = "module.exports = 'inside';\n"


def preload_text(source: pathlib.Path) -> str:
    """The preload, read from between the markers in `source`."""
    text = source.read_text(encoding="utf-8")
    try:
        _, rest = text.split(BEGIN + "\n", 1)
        block, _ = rest.split("\n" + END, 1)
    except ValueError:
        raise SystemExit(f"{source} does not hold the preload between its markers")
    found = re.fullmatch(r'const PRELOAD_TEXT: &str = r(#*)"(.*)"\1;', block, re.DOTALL)
    if found is None:
        raise SystemExit(f"{source} does not hold the preload as one raw string")
    return found.group(2)


def preload_name(source: pathlib.Path) -> str:
    """The preload's file name, as `PRELOAD` gives it in `source`."""
    found = re.search(r'pub const PRELOAD: &str = "([^"]+)";', source.read_text(encoding="utf-8"))
    if found is None:
        raise SystemExit(f"{source} does not name the preload")
    return found.group(1)


def hooked_node() -> tuple[str | None, str]:
    """The `node` on `PATH`, where it has `module.registerHooks`, and else why not."""
    node = shutil.which("node")
    if node is None:
        return None, "no node on PATH"
    try:
        ran = subprocess.run(
            [node, "-p", "process.version + ' ' + typeof require('node:module').registerHooks"],
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return None, f"{node} could not be run: {error}"
    version, _, kind = ran.stdout.strip().partition(" ")
    if ran.returncode != 0 or kind != "function":
        return None, f"{node} ({version or 'unknown version'}) has no module.registerHooks"
    return node, version


def module(directory: pathlib.Path, body: str) -> None:
    """A CommonJS package at `directory` whose main file holds `body`."""
    directory.mkdir(parents=True)
    (directory / "package.json").write_text(json.dumps({"main": "index.js"}), encoding="utf-8")
    (directory / "index.js").write_text(body, encoding="utf-8")


def layout(root: pathlib.Path, preload: str, name: str) -> tuple[pathlib.Path, pathlib.Path, pathlib.Path]:
    """A store under `root` with one version, and modules planted around it:
    the preload's path, the agent's script and the directory markers go to."""
    markers = root / "markers"
    markers.mkdir()
    version = root / "programs" / "agent" / "1.0.0"
    agent = version / "node_modules" / "agent"
    agent.mkdir(parents=True)
    (version / name).write_text(preload, encoding="utf-8")
    for file, body in [
        ("cli.js", AGENT),
        ("probe.cjs", PROBE),
        ("worker.cjs", WORKER),
        ("child.cjs", CHILD),
    ]:
        (agent / file).write_text(body, encoding="utf-8")
    module(version / "node_modules" / "inside", INSIDE)
    planted = [
        (root / "node_modules" / "planted", "above-store"),
        (root / ".node_modules" / "planted", "home"),
        (root / ".node_modules" / "optional-dep", "home-optional"),
        (root / ".node_libraries" / "planted", "home-libraries"),
    ]
    for directory, marker in planted:
        module(directory, PLANTED % (json.dumps(str(markers)), json.dumps(marker)))
    return version / name, agent / "cli.js", markers


def environment(root: pathlib.Path) -> dict[str, str]:
    """The agent's environment: the scratch directory as its home, nothing that
    chooses what Node loads."""
    return {"HOME": str(root), "USERPROFILE": str(root), "PATH": os.environ.get("PATH", "")}


def refused(seen: dict, field: str, code: str, specifier: str, where: str) -> list[str]:
    """What is wrong with how `seen[field]` refused `specifier`."""
    got = seen.get(field) or {}
    problems = []
    if got.get("code") != code:
        problems.append(f"{where}: {field} gave {got.get('code')!r}, not {code}")
    message = got.get("message") or ""
    if json.dumps(specifier) not in message or "/" in message or "\\" in message:
        problems.append(f"{where}: {field}'s refusal does not name the specifier alone: {message!r}")
    return problems


def confined(node: str, preload: pathlib.Path, script: pathlib.Path, root: pathlib.Path) -> list[str]:
    """What is wrong with how the agent ran under the preload."""
    try:
        ran = subprocess.run(
            [node, "--require", str(preload), str(script)],
            cwd=root,
            env=environment(root),
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return [f"the agent did not finish within {TIMEOUT} seconds"]
    if ran.returncode != 0:
        return [f"the agent exited {ran.returncode}: {ran.stderr.strip()}"]
    try:
        seen = json.loads(ran.stdout)
    except json.JSONDecodeError:
        return [f"the agent printed no JSON: {ran.stdout!r}"]
    problems = []
    for where in ["main", "worker", "fork"]:
        got = seen.get(where) or {}
        problems += refused(got, "optional", "MODULE_NOT_FOUND", "optional-dep", where)
        problems += refused(got, "planted", "MODULE_NOT_FOUND", "planted", where)
        problems += refused(got, "imported", "ERR_MODULE_NOT_FOUND", "planted", where)
        for field in ["inside", "imported_inside"]:
            if (got.get(field) or {}).get("code") != "loaded":
                problems.append(f"{where}: the module inside the tree did not load: {got.get(field)}")
    return problems


def unhooked(node: str, preload: pathlib.Path, script: pathlib.Path, root: pathlib.Path) -> list[str]:
    """What is wrong with how the preload stops a Node without
    `module.registerHooks`, which a first preload takes away."""
    hide = root / "hide.cjs"
    hide.write_text("delete require('node:module').registerHooks;\n", encoding="utf-8")
    ran_marker = root / "markers" / "agent-ran"
    probe = root / "agent-ran.cjs"
    probe.write_text(
        f"require('node:fs').writeFileSync({json.dumps(str(ran_marker))}, 'ran');\n",
        encoding="utf-8",
    )
    try:
        ran = subprocess.run(
            [node, "--require", str(hide), "--require", str(preload), str(probe)],
            cwd=root,
            env=environment(root),
            capture_output=True,
            text=True,
            timeout=TIMEOUT,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return [f"Node without registerHooks did not finish within {TIMEOUT} seconds"]
    problems = []
    if ran.returncode == 0:
        problems.append("Node without module.registerHooks ran the agent")
    if ran_marker.exists():
        ran_marker.unlink()
        problems.append("Node without module.registerHooks ran the agent's script")
    return problems


def main() -> int:
    source = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else SOURCE
    preload = preload_text(source)
    name = preload_name(SOURCE)
    node, version = hooked_node()
    if node is None:
        print(f"node preload: skipped, {version}")
        return 0
    with tempfile.TemporaryDirectory(prefix="node-preload-") as scratch:
        root = pathlib.Path(scratch).resolve()
        preload_path, script, markers = layout(root, preload, name)
        problems = confined(node, preload_path, script, root)
        problems += unhooked(node, preload_path, script, root)
        problems += [f"a planted module ran: {marker.name}" for marker in sorted(markers.iterdir())]
    for problem in problems:
        print(f"    FAIL node preload: {problem}")
    if problems:
        return 1
    print(f"node preload: confined on {node} {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
