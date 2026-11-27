#!/usr/bin/env bash
# The performance budgets, measured. Deterministic source checks have their own
# gates; this script owns speed. `performance.yml` blocks a merge on it under
# `CI required`, and RELEASING.md blocks a tag on a quiet-machine run of it —
# see that document for why those two readings are not the same claim.
#
#     scripts/sh/bench.sh              every budget
#     scripts/sh/bench.sh startup      first frame/input, fast exits, resume preview
#     scripts/sh/bench.sh tools        deterministic filesystem and process tools
#     scripts/sh/bench.sh mem          peak and retained RSS after a long session
#     scripts/sh/bench.sh grep         search, against the rg binary
#     scripts/sh/bench.sh stream       rendered frames under a token burst
#     scripts/sh/bench.sh live         turn-band redraws while a command prints
#     scripts/sh/bench.sh drawing      a drawer laying out the worst block it takes
#     scripts/sh/bench.sh picture-wait the drawing thread while a formula's picture is made
#     scripts/sh/bench.sh tex-slab     the host's kernel memory under TeX writing files
#
# A budget that needs the host's TeX is measured only when its mode is named,
# and there an absent TeX is a failure, not a skip. `tex-slab` is that mode: no
# argument leaves it out, so a runner without TeX measures every other budget,
# and naming it also builds `crucible-sandbox-broker` in release, where the
# probe's sandbox looks for it beside the probe.
#
# The colour-on memory probe runs a copy of `crucible` built with the root
# package's `bench-math-stand-in` feature, whose engine answers every display
# formula with one kept picture without TeX or the sandbox, so the session it
# measures keeps sixteen math pictures on any runner. That copy and the probe
# are built with the feature in the shared `target/`, copied into
# `target/bench-math-stand-in/release/`, and the probe runs from there. The
# default build follows at once, before any probe runs, so every run that
# completes leaves `target/release/crucible`, which every other probe and a
# release use, as the default build, and no probe runs it as the stand-in.
# Both builds share every dependency, so the second costs one build and link
# of the root package. A run stopped between the two, killed or failed (under
# `set -e` a failed default build also leaves it), leaves the stand-in at
# `target/release/crucible` until the next run, whose default build replaces
# it.
#
# Output is split by stream, so one run serves a human and a pipeline at once:
#
#     stdout   a single JSON document — what CI stores and diffs
#     stderr   progress, and a readable summary
#
# `scripts/sh/bench.sh grep > generated/json/budgets.json` therefore leaves the
# summary on the terminal and the record in the file.
#
# Each budget is owned by one probe under `src/bin/`, which takes its own
# measurement so the number and the limit live in the same file and cannot
# drift apart. The contract a probe must honour:
#
#     print one line to stdout:  <value> <unit> <limit> [key=<number> ...]
#                                e.g. `17.4 ms 20 p95=19.1`
#     exit 0 within budget, non-zero when over
#
# Evidence fields are optional and do not change the first three fields. The
# wrapper validates and stores them as numbers under `evidence` in the JSON.
#
# The limit a probe prints is the one it was held to. `CRUCIBLE_BENCH_RUNNER=shared`,
# set by the workflows that run this on a CI runner, holds the first frame, the first
# input and a drawer's layout to the shared-runner limits written beside theirs;
# unset, every probe holds the budget below.
#
# `println!` is denied workspace-wide, so that line goes out through
# `writeln!(io::stdout(), ...)` — which also makes the probe handle the write
# error rather than panicking inside a measurement.
#
# A probe that prints anything else fails as malformed rather than being
# guessed at — a benchmark that reports the wrong number is worse than one that
# does not run.
#
# A budget with no probe is a failure, not a skip: an unmeasured number is not
# a budget, it is a wish.
set -euo pipefail

cd "$(dirname "$0")/../.."

# mode | probe under src/bin/ | the budget it owns
readonly BUDGETS=(
    "startup|bench-first-frame|first frame <= 20 ms p95"
    "startup|bench-first-frame-colour|first frame with colour, answered and silent <= 20 ms p95"
    "startup|bench-native-frame|first frame in native mode <= 20 ms p95"
    "startup|bench-native-frame-colour|first frame in native mode with colour, answered and silent <= 20 ms p95"
    "startup|bench-first-input|first input <= 60 ms p95"
    "startup|bench-first-input-colour|first input with colour, a terminal that answers <= 60 ms p95"
    "startup|bench-first-input-colour-silent|first input with colour, a candidate terminal that never answers <= 2060 ms p95"
    "startup|bench-cli-exit|help and version process exit <= 12 ms p95"
    "startup|bench-resume-preview|resume picker, deepest session previewed <= 20 ms p95"
    "startup|bench-read-back|a result read back from the deepest session's log <= 20 ms p95"
    "tools|bench-tools|deterministic read, glob, edit, write and sandbox medians <= 40 ms"
    "mem|bench-session-rss|peak and retained RSS after a long session <= 35 MB"
    "mem|bench-session-rss-colour|peak and retained RSS with colour on <= 50 MB"
    "grep|bench-grep|grep worst paired median within 1.25x the rg binary"
    "stream|bench-render-burst|rendered frames >= 30/s and sustained/opening pace >= 0.5"
    "live|bench-live-burst|turn-band redraws >= 30/s and sustained/opening pace >= 0.5"
    "drawing|bench-drawing|a drawer's text layout at its caps <= 2 ms p95 and its picture <= 30 ms p95"
    "picture-wait|bench-picture-wait|the drawing thread's longest call while a formula's picture is pending <= 20 ms"
    "tex-slab|bench-tex-slab|kernel slab under TeX writing files, net of idle: each pair <= 128 MiB, median >= 8 MiB"
)

readonly MODES=(startup tools mem grep stream live drawing picture-wait tex-slab)

# Modes measured only when named, because they need what a runner may lack.
readonly NAMED_ONLY=(tex-slab)

# The probe run from the copy built with the stand-in engine, and where that
# copy is put, beside its own `crucible`.
readonly STAND_IN_PROBE=bench-session-rss-colour
readonly STAND_IN_DIR=target/bench-math-stand-in/release

# A probe's value and limit go into the JSON unquoted, so both must be numbers.
readonly NUMERIC='^-?[0-9]+(\.[0-9]+)?$'

mode=${1:-all}
if [[ "$mode" != all ]]; then
    known=0
    for m in "${MODES[@]}"; do
        [[ "$m" == "$mode" ]] && known=1
    done
    if ((!known)); then
        printf 'unknown mode %q\nexpected one of: %s — or no argument, for all\n' \
            "$mode" "${MODES[*]}" >&2
        exit 2
    fi
fi

named_only() {
    local m
    for m in "${NAMED_ONLY[@]}"; do
        [[ "$m" == "$1" ]] && return 0
    done
    return 1
}

selected=()
for entry in "${BUDGETS[@]}"; do
    IFS='|' read -r m _ _ <<<"$entry"
    if [[ "$mode" == "$m" ]] || { [[ "$mode" == all ]] && ! named_only "$m"; }; then
        selected+=("$entry")
    fi
done

# One cargo invocation for every probe that exists, rather than one per probe:
# the second build would be a no-op that still pays for cargo's startup.
build=()
stand_in=0
broker=0
for entry in "${selected[@]}"; do
    IFS='|' read -r m probe _ <<<"$entry"
    [[ -f "src/bin/$probe.rs" ]] && build+=("--bin=$probe")
    [[ "$probe" == "$STAND_IN_PROBE" && -f "src/bin/$probe.rs" ]] && stand_in=1
    [[ "$m" == tex-slab ]] && broker=1
done

if ((stand_in)); then
    # In the shared target, so it reuses every dependency the default build
    # compiles, then copied out of the default build's way: the default build
    # below replaces what it left in `target/release/`.
    echo "==> building the colour probe on the stand-in engine (release)" >&2
    cargo build --release --quiet --features bench-math-stand-in \
        "--bin=$STAND_IN_PROBE" --bin=crucible
    # Removed first, so a copy never writes through a link into what cargo
    # keeps.
    mkdir -p "$STAND_IN_DIR"
    rm -f "$STAND_IN_DIR/$STAND_IN_PROBE" "$STAND_IN_DIR/crucible"
    cp "target/release/$STAND_IN_PROBE" target/release/crucible "$STAND_IN_DIR/"
fi

# Unconditional once the stand-in was built, and before any probe runs, so
# `target/release/crucible` is the default build again whatever is measured.
if ((stand_in || ${#build[@]})); then
    # Release profile — fat LTO, one codegen unit, stripped. Measuring a debug
    # build answers a question nobody asked.
    echo "==> building probes (release)" >&2
    cargo build --release --quiet "${build[@]}" --bin=crucible
fi

if ((broker)); then
    echo "==> building the sandbox broker (release)" >&2
    cargo build --release --quiet -p crucible-sandbox-broker
fi

failed=0
unmeasured=0
results=()

# A JSON string field, with the two characters that would break the document
# escaped. Budget text is ours and plain, but a probe's output is not.
json_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '%s' "$s"
}

for entry in "${selected[@]}"; do
    IFS='|' read -r m probe budget <<<"$entry"

    if [[ ! -f "src/bin/$probe.rs" ]]; then
        printf '    UNMEASURED %-20s %s\n' "$probe" "$budget" >&2
        results+=("$(printf '{"mode":"%s","probe":"%s","budget":"%s","status":"unmeasured"}' \
            "$m" "$probe" "$(json_escape "$budget")")")
        failed=1
        unmeasured=1
        continue
    fi

    printf '==> %-20s %s\n' "$probe" "$budget" >&2

    built=target/release
    [[ "$probe" == "$STAND_IN_PROBE" ]] && built=$STAND_IN_DIR
    status=pass
    if ! reading=$("$built/$probe"); then
        status=over
        failed=1
    fi

    fields=()
    read -r -a fields <<<"$reading" || true
    value=${fields[0]:-}
    unit=${fields[1]:-}
    limit=${fields[2]:-}
    malformed=0
    [[ "$reading" == *$'\n'* ]] && malformed=1

    # Numeric on both, because they go into the document unquoted. This also
    # catches a probe that printed a sentence instead of a measurement.
    if [[ ! "$value" =~ $NUMERIC || ! "$limit" =~ $NUMERIC || -z "$unit" ]]; then
        malformed=1
    fi

    evidence=()
    evidence_keys='|'
    if ((${#fields[@]} > 3)); then
        for field in "${fields[@]:3}"; do
            key=${field%%=*}
            datum=${field#*=}
            if [[ "$field" != *=* || ! "$key" =~ ^[a-z][a-z0-9_]*$ || ! "$datum" =~ $NUMERIC ||
                "$evidence_keys" == *"|$key|"* ]]; then
                malformed=1
                break
            fi
            evidence+=("$(printf '"%s":%s' "$key" "$datum")")
            evidence_keys+="$key|"
        done
    fi

    if ((malformed)); then
        printf '    FAIL %s: want "<value> <unit> <limit> [key=<number> ...]" on stdout, got %q\n' \
            "$probe" "$reading" >&2
        results+=("$(printf '{"mode":"%s","probe":"%s","budget":"%s","status":"malformed","output":"%s"}' \
            "$m" "$probe" "$(json_escape "$budget")" "$(json_escape "$reading")")")
        failed=1
        continue
    fi

    if [[ "$status" == over ]]; then
        printf '    FAIL %s over budget: %s %s (limit %s)\n' "$probe" "$value" "$unit" "$limit" >&2
    else
        printf '         %s %s (limit %s)\n' "$value" "$unit" "$limit" >&2
    fi

    evidence_json=
    if ((${#evidence[@]})); then
        evidence_json=$(
            IFS=,
            printf ',"evidence":{%s}' "${evidence[*]}"
        )
    fi
    results+=("$(printf '{"mode":"%s","probe":"%s","budget":"%s","value":%s,"unit":"%s","limit":%s%s,"status":"%s"}' \
        "$m" "$probe" "$(json_escape "$budget")" "$value" "$(json_escape "$unit")" "$limit" "$evidence_json" "$status")")
done

overall=pass
((failed)) && overall=fail

joined=
if ((${#results[@]})); then
    joined=$(
        IFS=,
        printf '%s' "${results[*]}"
    )
fi

printf '{"revision":"%s","mode":"%s","status":"%s","budgets":[%s]}\n' \
    "$(git rev-parse HEAD 2>/dev/null || echo unknown)" "$mode" "$overall" "$joined"

if ((unmeasured)); then
    echo >&2
    echo "A budget with no probe cannot be checked, so it cannot be released." >&2
    echo "Write the probe as src/bin/<name>.rs, or retire the budget deliberately" >&2
    echo "in AGENTS.md, CONTRIBUTING.md and README.md together." >&2
fi

if ((failed)); then
    exit 1
fi

echo >&2
echo "all budgets hold" >&2
