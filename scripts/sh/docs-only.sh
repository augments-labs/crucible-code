#!/usr/bin/env bash
# Prints `true` when every path on standard input, one per NUL, is
# documentation, and `false` otherwise.
#
# Documentation is a Markdown file anywhere, or anything under `docs/`.
# CHANGELOG.md is not: the release notes are compiled into the binary. No path
# at all is not documentation either, since nothing then says it is. No test
# that runs only on macOS or Windows reads a document, so a pull request this
# answers `true` for loses no test by leaving those platforms to its merge.
#
#     git diff --name-only --no-renames -z HEAD^1 HEAD | scripts/sh/docs-only.sh
set -euo pipefail

seen=0
while IFS= read -r -d '' path; do
    [[ -n "$path" ]] || continue
    seen=1
    case "$path" in
    CHANGELOG.md)
        echo false
        exit 0
        ;;
    docs/* | *.md) ;;
    *)
        echo false
        exit 0
        ;;
    esac
done

if ((seen)); then
    echo true
else
    echo false
fi
