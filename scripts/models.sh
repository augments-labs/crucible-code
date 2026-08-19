#!/usr/bin/env bash
# Rewrites the model table from the public database of model limits.
#
# The one thing in this repository that reaches the network on purpose outside a
# turn, which is why the request is here in the open rather than inside the
# program that reads it. Run it when a model is added to the catalogue in
# src/cli.rs, or when a vendor changes a limit. Then read the diff: it is the
# only review this data ever gets.
set -euo pipefail

readonly DATABASE='https://models.dev/api.json'
readonly TABLE='src/cli/models.rs'

cd "$(dirname "$0")/.."

command -v curl >/dev/null || {
    echo 'models: curl is required to read the database' >&2
    exit 2
}

curl --proto '=https' --tlsv1.2 --fail --location --silent --show-error \
    --max-time 60 "$DATABASE" |
    cargo run --quiet --bin generate-models >"$TABLE.new"

mv "$TABLE.new" "$TABLE"
cargo fmt -- "$TABLE" 2>/dev/null || cargo fmt --all
echo "models: wrote $TABLE — read the diff before committing it"
