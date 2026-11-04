#!/usr/bin/env -S python3 -I
"""The preload crucible starts an npm agent under, run on a real Node.

The preload is read from the Rust source that ships it, from between its two
marker lines, and written at the top of a version laid out as crucible's store
lays one out, in a scratch directory that is also the agent's `HOME`. The
agent's script requires an optional dependency inside a `try`, and requires
and imports a module named `planted`, which is installed in a `node_modules`
above the store and in `HOME/.node_modules`, where Node's walk up would find
it. It requires and imports `linkout` too, a link in the tree into a version
beside it whose name extends the tree's own, requires `.\\planted`, which on
POSIX names a file planted above the store, requires `file:planted`,
`file:/planted`, `file:///planted` and `file://planted`, which there name ones
planted too, imports a `data:` URL and the `file:` URL of a module above the
store, and imports the `file:` URL of a module inside the tree, which still
loads. Each
planted copy writes a marker file when it runs. The script does the same again
in a worker and in a child started by `fork`.

It passes when every one of them refuses each, `require` with
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

# What the probe imports as a `data:` URL, which is refused as a package.
DATA = "data:text/javascript,export default 1"

PROBE = r"""
'use strict';
const DATA = __DATA__;
const OUTSIDE = __OUTSIDE__;
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
    backslashed: caught(() => require('.\\planted')),
    filed: caught(() => require('file:planted')),
    filed_path: caught(() => require('file:/planted')),
    filed_root: caught(() => require('file:///planted')),
    filed_host: caught(() => require('file://planted')),
    linked: caught(() => require('linkout')),
    inside: caught(() => require('inside')),
  };
  try {
    await import('planted');
    seen.imported = { code: 'loaded' };
  } catch (error) {
    seen.imported = { code: error.code, message: error.message };
  }
  try {
    await import('linkout');
    seen.imported_linked = { code: 'loaded' };
  } catch (error) {
    seen.imported_linked = { code: error.code, message: error.message };
  }
  try {
    await import(DATA);
    seen.imported_data = { code: 'loaded' };
  } catch (error) {
    seen.imported_data = { code: error.code, message: error.message };
  }
  try {
    await import(OUTSIDE);
    seen.imported_outside_url = { code: 'loaded' };
  } catch (error) {
    seen.imported_outside_url = { code: error.code, message: error.message };
  }
  try {
    await import(require('node:url').pathToFileURL(require.resolve('inside')).href);
    seen.imported_inside_url = { code: 'loaded' };
  } catch (error) {
    seen.imported_inside_url = { code: error.code, message: error.message };
  }
  try {
    await import('inside');
    seen.imported_inside = { code: 'loaded' };
  } catch (error) {
    seen.imported_inside = { code: error.code, message: error.message };
  }
  return seen;
};
""".replace("__DATA__", json.dumps(DATA))

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


def outside(root: pathlib.Path) -> pathlib.Path:
    """A module above the store, which the probe imports by its `file:` URL."""
    return root / "outside.mjs"


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
        ("probe.cjs", PROBE.replace("__OUTSIDE__", json.dumps(outside(root).as_uri()))),
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
    # A version beside this one whose name extends its own, which a link in
    # the tree leads into: outside the tree, though its path begins with the
    # tree's.
    sibling = version.with_name(version.name + "x")
    planted.append((sibling / "node_modules" / "planted", "sibling"))
    for directory, marker in planted:
        module(directory, PLANTED % (json.dumps(str(markers)), json.dumps(marker)))
    try:
        os.symlink(sibling / "node_modules" / "planted", version / "node_modules" / "linkout", target_is_directory=True)
    except OSError as error:
        raise SystemExit(f"node preload: a link could not be made in the tree: {error}")
    if os.name != "nt":
        # A file whose name begins `.\`, which on POSIX Node resolves
        # `require('.\\planted')` to as a package, not as a relative path.
        (root / "node_modules" / ".\\planted.js").write_text(
            PLANTED % (json.dumps(str(markers)), json.dumps("above-store-backslash")),
            encoding="utf-8",
        )
        # A file in a directory named `file:`, which `require('file:/planted')`
        # and its spellings with more slashes walk up to as a package too.
        (root / "node_modules" / "file:").mkdir()
        (root / "node_modules" / "file:" / "planted.js").write_text(
            PLANTED % (json.dumps(str(markers)), json.dumps("above-store-file-path")),
            encoding="utf-8",
        )
        # A file whose name begins `file:`, which `require('file:planted')`
        # walks up to as a package, as the preload confines every `file:`
        # specifier.
        (root / "node_modules" / "file:planted.js").write_text(
            PLANTED % (json.dumps(str(markers)), json.dumps("above-store-file")),
            encoding="utf-8",
        )
    outside(root).write_text(
        "import { writeFileSync } from 'node:fs';\n"
        f"writeFileSync({json.dumps(str(markers / 'outside-url'))}, 'ran');\n",
        encoding="utf-8",
    )
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
    rest = message.replace(json.dumps(specifier), "")
    if json.dumps(specifier) not in message or "/" in rest or "\\" in rest:
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
        problems += refused(got, "linked", "MODULE_NOT_FOUND", "linkout", where)
        problems += refused(got, "imported_linked", "ERR_MODULE_NOT_FOUND", "linkout", where)
        problems += refused(got, "imported_data", "ERR_MODULE_NOT_FOUND", DATA, where)
        problems += refused(got, "imported_outside_url", "ERR_MODULE_NOT_FOUND", outside(root).as_uri(), where)
        if os.name != "nt":
            problems += refused(got, "backslashed", "MODULE_NOT_FOUND", ".\\planted", where)
            problems += refused(got, "filed", "MODULE_NOT_FOUND", "file:planted", where)
            # Whichever of Node or the preload refuses these, none loads.
            for field in ["filed_path", "filed_root", "filed_host"]:
                if (got.get(field) or {}).get("code") != "MODULE_NOT_FOUND":
                    problems.append(f"{where}: {field} gave {(got.get(field) or {}).get('code')!r}, not MODULE_NOT_FOUND")
        for field in ["inside", "imported_inside", "imported_inside_url"]:
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
