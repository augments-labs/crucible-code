#!/usr/bin/env bash
# Rewrites the model table from the public database of model limits.
#
# The one thing in this repository that reaches the network on purpose outside a
# turn, which is why the request is here in the open rather than inside the
# program that reads it. Run it when a model is added to the catalogue in
# src/cli.rs, or when a vendor changes a limit. Then read the diff: a test holds
# the table to the slice recorded beside it, and nothing holds either to what
# the vendor actually serves except somebody reading this run.
set -euo pipefail

readonly DATABASE='https://models.dev/api.json'

cd "$(dirname "$0")/.."

command -v curl >/dev/null || {
    echo 'models: curl is required to read the database' >&2
    exit 2
}

# The program writes both files itself and names them in its own source, which
# is what keeps the table and the slice it was read from one refresh rather than
# two. Nothing here repeats those paths: a script that knew one of them could be
# changed to write only that one.
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
    --max-time 60 "$DATABASE" |
    cargo run --quiet --bin generate-models

cargo fmt --all
echo 'models: wrote the table and the slice beside it — read the diff before committing them'
