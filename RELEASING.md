# Releasing

Cutting a release is mechanical on purpose. Everything that can fail should fail
in CI, before the tag exists.

## Versioning

`0.0.x` while the shape is still moving. Configuration files, session files and
the command-line surface may change in any `0.0.x` release with no deprecation
period, and the changelog says so at the top.

The version lives in exactly one place — `[workspace.package] version` in the
root `Cargo.toml`. Every crate inherits it with `version.workspace = true`, so
the workspace ships as one unit and there is no per-crate version to drift.

## Before you tag

1. **`main` is green.** CI passed on the commit you intend to tag.
2. **Gates pass locally.**

   ```bash
   scripts/check.sh
   ```

3. **The budgets hold.** On a quiet machine — not a shared CI runner, whose
   numbers are a trend and nothing more:

   ```bash
   scripts/bench.sh > budgets.json
   ```

   Each probe carries its own limit and exits non-zero when it is over, so the
   script decides rather than you reading a table; a non-zero exit blocks the
   release. A slower build is not a release, it is a bug that happens to
   compile. Keep the JSON with the tag — the next release compares against it.
4. **The changelog is real.** Move everything under `Unreleased` into a new
   version section with today's date, and add the comparison link. Written for
   someone deciding whether to upgrade, not generated from commit subjects.
5. **The install path works from scratch.** Every gate above builds from this
   tree with this machine's toolchain, so none of them can see what a shipped
   binary needs from the machine it lands on. Package the artifact the way the
   release workflow does and put it somewhere that holds nothing else:

   ```bash
   name=crucible-v$VERSION-x86_64-unknown-linux-gnu
   install -Dm755 target/release/crucible "$name/crucible"
   install -Dm644 README.md LICENSE -t "$name/"
   tar czf "$name.tar.gz" "$name"

   scripts/smoke.sh "$name.tar.gz"
   ```

   The sandbox carries the binary, the loader and the libraries the binary
   itself names — no shell, no toolchain, no certificate bundle, no source tree,
   and a home directory that did not exist a moment ago. It reports the glibc
   floor, which is the number that decides which distributions this release
   leaves behind; when it moves, `docs/getting-started/getting-started.md` says so.

   The run stops short of a completed turn unless `CRUCIBLE_SMOKE_KEY` is set,
   because a turn costs tokens. Set it for the release you actually cut:

   ```bash
   CRUCIBLE_SMOKE_KEY=$ANTHROPIC_API_KEY scripts/smoke.sh "$name.tar.gz"
   ```

## Cutting it

Nothing reaches `main` except through a pull request, and none merges until
`scripts/check.sh` and `scripts/bench.sh` are green on it. That is a repository
ruleset with no bypass, so it holds for the person cutting the release too — the
tired push at the end of a long day is the one it exists to catch.

```bash
# 1. Bump the single version, and update the changelog in the same commit.
git switch -c release/v0.0.1
$EDITOR Cargo.toml CHANGELOG.md
cargo build                     # refresh Cargo.lock with the new version
scripts/check.sh

git commit -am "chore(release): 0.0.1"
git push -u origin release/v0.0.1

# 2. Open it, let CI answer, merge it.
gh pr create --base main --title "release: 0.0.1"
gh pr checks --watch
gh pr merge --merge --delete-branch

# 3. Tag the commit CI just proved green.
git switch main && git pull
git tag -a v0.0.1 -m "crucible 0.0.1"
git push origin v0.0.1
```

`cargo deny` is not a required check, and must not become one. It runs only when
the dependency set changes, so on a release that touches no dependency it never
reports — and a required check that never reports leaves the pull request
pending for ever rather than failing it.

Pushing the tag is the trigger. The release workflow builds the binary, attaches
it with its checksum, and opens the GitHub Release with the changelog section as
its body.

## Artifacts

| Target | Artifact |
| --- | --- |
| `x86_64-unknown-linux-gnu` | `crucible-v<version>-x86_64-unknown-linux-gnu.tar.gz` |

Each archive ships beside a `.sha256` file. Builds are release profile — fat
LTO, one codegen unit, symbols stripped — the same settings every published
measurement was taken under, so the numbers in the changelog describe the binary
people actually download.

Other targets are added when someone runs them; a platform nobody tests is a
promise nobody keeps.

## After the tag

1. Run the same gate against what was actually published, which is the one
   artifact no earlier step was allowed to trust:

   ```bash
   scripts/smoke.sh v0.0.1
   ```

   Given a tag rather than a file it downloads the release, checks the tarball
   against the published `.sha256`, and runs everything from step 5 on that copy
   — so a build that was fine locally and an upload that went wrong are told
   apart rather than averaged.
2. **If the schema changed, republish it.**

   ```bash
   git diff v0.0.2..v0.0.3 -- schema/
   ```

   `schema/crucible-code-schema.json` is generated from `shape.rs` and a gate
   keeps the checked-in copy honest, so it is always right *here*. SchemaStore
   serves a separate copy, and nothing in this repository can make that one
   follow: a release that adds a key leaves every editor pointed at the
   published URL marking it red, and a release that removes one leaves them
   completing a key crucible now refuses. Open a pull request against
   [SchemaStore/schemastore](https://github.com/SchemaStore/schemastore)
   replacing `src/schemas/json/crucible-code-schema.json` with the file this tag
   ships, and run their formatter and their gate over it before committing:

   ```bash
   npm clean-install
   ./node_modules/.bin/prettier --config .prettierrc.cjs --write \
     src/schemas/json/crucible-code-schema.json
   node ./cli.js check --schema-name=crucible-code-schema.json
   ```

   Their formatter sorts `$schema` above `$id` and puts a short `enum` on one
   line, so the copy they serve is never byte for byte ours and a plain `cp`
   fails their CI. Key order and whitespace are the only things it may change;
   a diff touching anything else means the file was hand-edited, which is what
   the generator exists to prevent. Their positive and negative tests under
   `src/test/` and `src/negative_test/` are the other half of the pull request,
   and a key that changed shape needs them changed with it.

   Only one copy is served, so it describes the newest release and not the one
   somebody is running. That is why the schema's own description says the format
   is unstable for the whole 0.0.x line — an editor is a hint, and the program is
   the authority.
3. Open a fresh `Unreleased` section in the changelog.
4. If the release is broken, do not delete or move the tag. Fix forward with a
   patch release. A tag that changes meaning breaks every checksum anyone
   recorded against it — which is why a `v*` tag can no longer be deleted or
   moved at all. The ruleset refuses it, so the remedy is the patch release
   whether or not anybody remembered this paragraph.

## If a release must be pulled

Yank is not available for a binary distribution, so:

1. Mark the GitHub Release as a pre-release so it stops being "latest".
2. Add a `### Removed` note to the changelog saying what was wrong and which
   version supersedes it.
3. Ship the replacement the same day if the defect risks data or credentials.
