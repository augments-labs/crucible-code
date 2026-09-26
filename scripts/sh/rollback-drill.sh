#!/usr/bin/env bash
# Headless prior-binary rollback drill.
#
# Runs the previous released binary (v0.42.0) over a copy of synthetic session
# fixtures the candidate has read and recovered, headless with redirected
# input, and compares replay, pending-action recovery and command behaviour to
# the preservation contract:
#
#   replay:            the prior binary resumes a conversation the candidate
#                      resumed, replays the recorded answer, and leaves the log
#                      byte-identical;
#   pending recovery:  both binaries resume a session ending in an unanswered
#                      tool call, cut the same dangling message, and end with
#                      byte-identical logs;
#   command behaviour: --sandbox and --extensions agree past HOME/WORK normalization.
#
# Exit 0 when every gate holds. Exit 1 only with --corrupt, when the prior
# binary refuses the corrupted fixture as it must (the falsification). Exit 2
# for any other failure.
#
# The previous binary comes from the local tree: --prior-binary names one, or
# the drill builds the tag in a scratch worktree of this repository and removes
# it on the way out. The drill fetches nothing. It reads no real session or
# credential file — both homes are scratch directories with the update check
# off, the key variables unset, and no model selected, so no turn is ever taken
# and no provider is ever called.
#
#     scripts/sh/rollback-drill.sh
#     scripts/sh/rollback-drill.sh --prior-binary /path/to/crucible
#     scripts/sh/rollback-drill.sh --corrupt pending
set -euo pipefail

cd "$(dirname "$0")/../.."

readonly PRIOR_TAG=v0.42.0
readonly CONV_ID=1788000000000-d81101
readonly PEND_ID=1788000000001-d81102
readonly CONV_USER='what does the rollback drill replay'
readonly CONV_AGENT='the conversation the candidate recorded'
readonly PEND_USER='a command left running'
readonly PEND_AGENT='reaching for the tool'

candidate=
prior=
corrupt=
while (($#)); do
    case "$1" in
    --candidate-binary)
        shift
        candidate=${1:?'rollback-drill: --candidate-binary needs a path'}
        ;;
    --prior-binary)
        shift
        prior=${1:?'rollback-drill: --prior-binary needs a path'}
        ;;
    --corrupt)
        shift
        corrupt=${1:?'rollback-drill: --corrupt needs conversation or pending'}
        if [[ $corrupt != conversation && $corrupt != pending ]]; then
            echo "rollback-drill: --corrupt takes conversation or pending, got $corrupt" >&2
            exit 2
        fi
        ;;
    -h | --help)
        sed -n '2,27p' "$0"
        exit 0
        ;;
    *)
        echo "rollback-drill: unknown option $1" >&2
        exit 2
        ;;
    esac
    shift
done

failed=0
fail() {
    printf '    FAIL %s\n' "$1" >&2
    failed=1
}

for tool in cargo git mktemp; do
    command -v "$tool" >/dev/null || {
        echo "rollback-drill: $tool is not installed" >&2
        exit 2
    }
done

# A SHA-256 digest that reads the same on Linux, macOS and Git Bash.
digest() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1"
    elif command -v shasum >/dev/null; then
        shasum -a 256 "$1"
    else
        echo "rollback-drill: no SHA-256 tool installed" >&2
        exit 2
    fi | awk '{ print $1 }'
}

binary_at() {
    if [[ -x $1/crucible ]]; then
        printf '%s' "$1/crucible"
    elif [[ -x $1/crucible.exe ]]; then
        printf '%s' "$1/crucible.exe"
    else
        return 1
    fi
}

stage=$(mktemp -d)
worktree=
cleanup() {
    if [[ -n $worktree ]]; then
        git worktree remove --force "$worktree" >/dev/null 2>&1 || true
    fi
    rm -rf -- "$stage"
}
trap cleanup EXIT

if [[ -z $candidate ]]; then
    if ! candidate=$(binary_at "$PWD/target/debug"); then
        echo '==> candidate build'
        cargo build --locked --bin crucible
        candidate=$(binary_at "$PWD/target/debug") || {
            echo 'rollback-drill: the candidate build left no binary' >&2
            exit 2
        }
    fi
fi
[[ -x $candidate ]] || {
    echo "rollback-drill: no executable at $candidate" >&2
    exit 2
}

if [[ -z $prior ]]; then
    if ! git rev-parse --verify "$PRIOR_TAG^{commit}" >/dev/null 2>&1; then
        echo "rollback-drill: the local tag $PRIOR_TAG is missing; fetch it first — the drill fetches nothing" >&2
        exit 2
    fi
    echo '==> prior build'
    worktree=$stage/prior-src
    git worktree add --detach "$worktree" "$PRIOR_TAG" >/dev/null
    (cd "$worktree" && cargo build --locked --bin crucible)
    prior=$(binary_at "$worktree/target/debug") || {
        echo 'rollback-drill: the prior build left no binary' >&2
        exit 2
    }
fi
[[ -x $prior ]] || {
    echo "rollback-drill: no executable at $prior" >&2
    exit 2
}

work=$stage/work
chome=$stage/candidate-home
phome=$stage/prior-home
mkdir -p "$work" "$chome/sessions" "$phome/sessions"

plant() {
    local home=$1
    printf '{ "updates": { "check": "never" } }\n' >"$home/config.json"
    cat >"$home/sessions/$CONV_ID.jsonl" <<EOF
{"format":13,"session":"$CONV_ID","workspace":"$work"}
{"user":"$CONV_USER"}
{"agent":"$CONV_AGENT","calls":[],"stop":"yielded"}
EOF
    cat >"$home/sessions/$PEND_ID.jsonl" <<EOF
{"format":13,"session":"$PEND_ID","workspace":"$work"}
{"user":"$PEND_USER"}
{"agent":"$PEND_AGENT","calls":[{"args":"{}","id":"call-1","name":"read"}],"stop":"tools"}
EOF
}
plant "$chome"
printf '{ "updates": { "check": "never" } }\n' >"$phome/config.json"
cp "$chome/sessions/$CONV_ID.jsonl" "$phome/sessions/$CONV_ID.jsonl"
cp "$chome/sessions/$PEND_ID.jsonl" "$phome/sessions/$PEND_ID.jsonl"

if [[ -n $corrupt ]]; then
    echo "==> corrupt the prior copy"
    case "$corrupt" in
    conversation) victim=$phome/sessions/$CONV_ID.jsonl ;;
    pending) victim=$phome/sessions/$PEND_ID.jsonl ;;
    esac
    # Break the middle line while lines follow it: damage the prior binary
    # must refuse rather than replay past.
    {
        head -n 1 "$victim"
        printf '{"user":\n'
        tail -n +3 "$victim"
    } >"$victim.damaged"
    mv "$victim.damaged" "$victim"
    printf '    %s\n' "$victim"
    printf '    %s\n' "$(digest "$victim")"
fi

printf 'candidate %s\n' "$("$candidate" --version)"
printf 'prior     %s\n' "$("$prior" --version)"

# One headless run from the fixture workspace: redirected empty input,
# scratch home, no keys, no model. Prints the exit code; the caller reads
# stdout and stderr off files. The working directory matters — a resumed
# session is bound to the directory it was recorded in.
headless() {
    local binary=$1 home=$2
    shift 2
    local status=0
    (cd "$work" && env -u ANTHROPIC_API_KEY -u GEMINI_API_KEY -u MOONSHOT_API_KEY -u OPENAI_API_KEY \
        "CRUCIBLE_CODE_HOME=$home" "$binary" "$@" </dev/null >"$stage/out" 2>"$stage/err") || status=$?
    printf '%s' "$status"
}

has() {
    grep -Fq "$2" "$1" || fail "$3"
}

count_logs() {
    local home=$1 count=0
    for log in "$home"/sessions/*.jsonl; do
        [[ -e $log ]] || break
        count=$((count + 1))
    done
    printf '%s' "$count"
}

echo '==> candidate reads and recovers the fixtures'
status=$(headless "$candidate" "$chome")
[[ $status == 0 ]] || fail "the candidate fresh run exited $status"
has "$stage/out" '/login' 'the candidate fresh run never said what to do about a missing key'
has "$stage/out" '/model' 'the candidate fresh run never named /model'
if grep -q $'\e' "$stage/out"; then
    fail 'the candidate fresh run wrote an escape sequence to redirected output'
fi
[[ $(count_logs "$chome") == 3 ]] || fail 'the candidate fresh run left no session of its own behind'
cp "$stage/out" "$stage/candidate-fresh.out"

status=$(headless "$candidate" "$chome" --resume "$CONV_ID")
[[ $status == 0 ]] || fail "the candidate resume of the conversation exited $status"
has "$stage/out" "$CONV_AGENT" 'the candidate resume did not replay the recorded answer'

status=$(headless "$candidate" "$chome" --resume "$PEND_ID")
[[ $status == 0 ]] || fail "the candidate resume of the pending session exited $status"
has "$stage/out" "$PEND_USER" 'the candidate resume did not recover the interrupted prompt'
if grep -Fq "$PEND_AGENT" "$stage/out"; then
    fail 'the candidate resume replayed the unanswered call as an answer'
fi

echo '==> prior replays the copy'
status=$(headless "$prior" "$phome")
[[ $status == 0 ]] || fail "the prior fresh run exited $status"
has "$stage/out" '/login' 'the prior fresh run never said what to do about a missing key'
[[ $(count_logs "$phome") == 3 ]] || fail 'the prior fresh run left no session of its own behind'

status=$(headless "$prior" "$phome" --resume "$CONV_ID")
conv_status=$status
cp "$stage/out" "$stage/prior-conv.out"
cp "$stage/err" "$stage/prior-conv.err"
[[ $conv_status == 0 ]] || fail "the prior resume of the conversation exited $conv_status"
has "$stage/out" "$CONV_AGENT" 'the prior resume did not replay the recorded answer'

echo '==> pending-action recovery agrees'
status=$(headless "$prior" "$phome" --resume "$PEND_ID")
pend_status=$status
cp "$stage/out" "$stage/prior-pend.out"
cp "$stage/err" "$stage/prior-pend.err"
[[ $pend_status == 0 ]] || fail "the prior resume of the pending session exited $pend_status"
has "$stage/out" "$PEND_USER" 'the prior resume did not recover the interrupted prompt'
if grep -Fq "$PEND_AGENT" "$stage/out"; then
    fail 'the prior resume replayed the unanswered call as an answer'
fi

if [[ -n $corrupt ]]; then
    case "$corrupt" in
    conversation)
        bad_status=$conv_status
        bad_err=$stage/prior-conv.err
        ;;
    pending)
        bad_status=$pend_status
        bad_err=$stage/prior-pend.err
        ;;
    esac
    if [[ $bad_status != 0 ]] && grep -Fq 'could not read the session log' "$bad_err"; then
        echo 'FALSIFICATION HELD: the prior binary refused the corrupted fixture'
        grep -F 'could not read the session log' "$bad_err" | head -n 1
        exit 1
    fi
    echo 'rollback-drill: --corrupt did not make the prior binary refuse; the falsification failed' >&2
    exit 2
fi

echo '==> replay and recovery are byte-identical'
[[ $(digest "$chome/sessions/$CONV_ID.jsonl") == "$(digest "$phome/sessions/$CONV_ID.jsonl")" ]] ||
    fail 'the conversation log differs between candidate and prior'
[[ $(digest "$chome/sessions/$PEND_ID.jsonl") == "$(digest "$phome/sessions/$PEND_ID.jsonl")" ]] ||
    fail 'the recovered pending log differs between candidate and prior'
if ! grep -Fq "$PEND_AGENT" "$chome/sessions/$PEND_ID.jsonl"; then
    printf '    the dangling call was cut: %s\n' "$(digest "$chome/sessions/$PEND_ID.jsonl")"
else
    fail 'the unanswered call is still in the recovered log'
fi

echo '==> command behaviour agrees'
for command in --sandbox --extensions; do
    status=$(headless "$candidate" "$chome" "$command")
    [[ $status == 0 ]] || fail "candidate $command exited $status"
    # The listing names the home it was read from; the homes differ by
    # construction, so compare past the path, not through it.
    sed -e "s|$chome|HOME|g" -e "s|$work|WORK|g" "$stage/out" >"$stage/candidate-$command.out"
    status=$(headless "$prior" "$phome" "$command")
    [[ $status == 0 ]] || fail "prior $command exited $status"
    sed -e "s|$phome|HOME|g" -e "s|$work|WORK|g" "$stage/out" >"$stage/prior-$command.out"
    [[ $(digest "$stage/candidate-$command.out") == $(digest "$stage/prior-$command.out") ]] ||
        fail "$command differs between candidate and prior"
    printf '    %s %s\n' "$command" "$(digest "$stage/prior-$command.out")"
done

if ((failed)); then
    echo 'rollback drill gates failed'
    exit 2
fi

echo
echo 'all rollback drill gates passed'
