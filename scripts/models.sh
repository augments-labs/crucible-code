#!/usr/bin/env bash
# Rewrites the model table from the public database of model limits.
#
# The one thing in this repository that reaches the network on purpose outside a
# turn, which is why the request is here in the open rather than inside the
# program that reads it. Run it when a model is added to the catalogue in
# src/cli.rs, or when a vendor changes a limit. Then read the diff: this run is
# the only thing that holds the table to what the vendor serves, and no rerun
# reproduces it, because nothing here keeps a copy of what was read.
set -euo pipefail

readonly DATABASE='https://models.dev/api.json'

cd "$(dirname "$0")/.."

command -v curl >/dev/null || {
    echo 'models: curl is required to read the database' >&2
    exit 2
}

# The program names the file it writes in its own source. Nothing here repeats
# that path: the request and the reading are two jobs, and this script has the
# one that reaches the network.
curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
    --max-time 60 "$DATABASE" |
    cargo run --quiet --bin generate-models

cargo fmt --all
echo 'models: wrote the table and the slice beside it — read the diff before committing them'
