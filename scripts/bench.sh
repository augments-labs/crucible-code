#!/usr/bin/env bash
# The performance budgets, measured. `scripts/check.sh` proves the code is
# correct; this proves it is still fast, and RELEASING.md blocks a tag on it.
#
#     scripts/bench.sh              every budget
#     scripts/bench.sh startup      first frame, first input
#     scripts/bench.sh mem          peak RSS after a long session
#     scripts/bench.sh grep         search, against the rg binary
#     scripts/bench.sh stream       rendered frames under a token burst
#     scripts/bench.sh live         turn-band redraws while a command prints
#
# Output is split by stream, so one run serves a human and a pipeline at once:
#
#     stdout   a single JSON document — what CI stores and diffs
#     stderr   progress, and a readable summary
#
# `scripts/bench.sh grep > budgets.json` therefore leaves the summary on the
# terminal and the record in the file.
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

cd "$(dirname "$0")/.."

# mode | probe under src/bin/ | the budget it owns
readonly BUDGETS=(
    "startup|bench-first-frame|first frame <= 20 ms p95"
    "startup|bench-first-input|first input <= 60 ms p95"
    "mem|bench-session-rss|peak RSS after a 20-turn session <= 35 MB"
    "grep|bench-grep|grep worst paired median within 1.25x the rg binary"
    "stream|bench-render-burst|rendered frames >= 30/s and sustained/opening >= 0.5"
    "live|bench-live-burst|turn-band redraws >= 30/s and sustained/opening >= 0.5"
)

readonly MODES=(startup mem grep stream live)

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

selected=()
for entry in "${BUDGETS[@]}"; do
    IFS='|' read -r m _ _ <<<"$entry"
    if [[ "$mode" == all || "$mode" == "$m" ]]; then
        selected+=("$entry")
    fi
done

# One cargo invocation for every probe that exists, rather than one per probe:
# the second build would be a no-op that still pays for cargo's startup.
build=()
for entry in "${selected[@]}"; do
    IFS='|' read -r _ probe _ <<<"$entry"
    [[ -f "src/bin/$probe.rs" ]] && build+=("--bin=$probe")
done

if ((${#build[@]})); then
    # Release profile — fat LTO, one codegen unit, stripped. Measuring a debug
    # build answers a question nobody asked.
    echo "==> building probes (release)" >&2
    cargo build --release --quiet "${build[@]}" --bin=crucible
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

    status=pass
    if ! reading=$("target/release/$probe"); then
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
    echo "in CLAUDE.md, CONTRIBUTING.md and README.md together." >&2
fi

if ((failed)); then
    exit 1
fi

echo >&2
echo "all budgets hold" >&2
