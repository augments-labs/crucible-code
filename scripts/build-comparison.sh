#!/usr/bin/env bash
# Compare build behavior for two revisions in the same checkout and runner.
#
# This is observational: compiler speed, cache state and peak memory belong to
# the machine taking the reading, so no absolute number here blocks a pull
# request. The workflow runs base and candidate back to back with independent
# target directories, records Cargo timing reports, and publishes one JSON
# comparison. Run it manually with:
#
#     scripts/build-comparison.sh BASE CANDIDATE [OUTPUT_DIRECTORY]
#
# The checkout must be clean because each revision is checked out in turn. Its
# original revision is restored even when a build fails.
set -euo pipefail

cd "$(dirname "$0")/.."

base=${1:?usage: scripts/build-comparison.sh BASE CANDIDATE [OUTPUT_DIRECTORY]}
candidate=${2:?usage: scripts/build-comparison.sh BASE CANDIDATE [OUTPUT_DIRECTORY]}
output=${3:-build-comparison}

[[ -x /usr/bin/time ]] || {
    echo 'build comparison requires GNU /usr/bin/time' >&2
    exit 2
}
[[ -z $(git status --porcelain) ]] || {
    echo 'build comparison requires a clean checkout' >&2
    exit 2
}
command -v python3 >/dev/null || {
    echo 'build comparison requires python3 to assemble its JSON artifact' >&2
    exit 2
}

original=$(git rev-parse --verify HEAD)
base=$(git rev-parse --verify "$base^{commit}")
candidate=$(git rev-parse --verify "$candidate^{commit}")
mkdir -p "$output"
output=$(cd "$output" && pwd -P)
work=$(mktemp -d)
trap 'git checkout --detach --quiet "$original"; rm -rf -- "$work"' EXIT

time_command() {
    local revision=$1 scenario=$2 target=$3 log=$4
    shift 4

    mkdir -p "$target"
    local timing=$target/cargo-timings
    rm -rf -- "$timing"

    local status=0
    /usr/bin/time --format='%e %M' --output="$work/resource" \
        env CARGO_TARGET_DIR="$target" \
        cargo "$@" --workspace --all-targets --locked --timings >"$work/stdout" 2>"$work/stderr" || status=$?

    local elapsed peak
    read -r elapsed peak < "$work/resource"
    mkdir -p "$output/timings/$log"
    if [[ -d $timing ]]; then
        cp -a "$timing/." "$output/timings/$log/"
    fi
    python3 - "$output/$log.json" "$revision" "$scenario" "$status" "$elapsed" "$peak" <<'PY'
import json
import sys
path, revision, scenario, status, elapsed, peak = sys.argv[1:]
with open(path, "w", encoding="utf-8") as stream:
    json.dump({
        "revision": revision,
        "scenario": scenario,
        "status": int(status),
        "elapsed_seconds": float(elapsed),
        "peak_rss_kib": int(peak or 0),
    }, stream, separators=(",", ":"))
    stream.write("\n")
PY
    if ((status != 0)); then
        cat "$work/stderr" >&2
        return "$status"
    fi
}

measure_revision() {
    local name=$1 revision=$2 target=$output/target-$1
    rm -rf -- "$target"
    git checkout --detach --quiet "$revision"

    time_command "$revision" clean-check "$target" "$name-clean" check
    time_command "$revision" no-op-check "$target" "$name-noop" check

    # A leaf crate with no internal dependencies: touches invalidate the leaf
    # and the composition root that takes it, while preserving every dependency
    # artifact in this target directory.
    local leaf=crates/crucible-tui/src/lib.rs
    local stamp
    stamp=$(stat -c '%y' "$leaf")
    touch "$leaf"
    time_command "$revision" leaf-touch-check "$target" "$name-leaf" check
    touch -d "$stamp" "$leaf"

    # The composition root is the opposite edge of the graph: editing its entry
    # invalidates the application and no lower crate. The source bytes stay
    # untouched in both cases; only Cargo's dependency freshness is exercised.
    stamp=$(stat -c '%y' src/main.rs)
    touch src/main.rs
    time_command "$revision" root-touch-check "$target" "$name-root" check
    touch -d "$stamp" src/main.rs
}

measure_revision base "$base"
measure_revision candidate "$candidate"
git checkout --detach --quiet "$original"

python3 - "$output" "$base" "$candidate" <<'PY'
import json
import pathlib
import sys
root = pathlib.Path(sys.argv[1])
base, candidate = sys.argv[2:]
scenarios = ["clean", "noop", "leaf", "root"]
records = {}
for side in ("base", "candidate"):
    records[side] = {}
    for scenario in scenarios:
        records[side][scenario] = json.loads((root / f"{side}-{scenario}.json").read_text())
summary = {
    "base": base,
    "candidate": candidate,
    "observational": True,
    "measurements": records,
}
(root / "comparison.json").write_text(
    json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY

rm -rf -- "$output/target-base" "$output/target-candidate"
printf 'build comparison written to %s/comparison.json\n' "$output"
