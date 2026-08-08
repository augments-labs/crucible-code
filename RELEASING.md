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
   leaves behind; when it moves, `docs/getting-started.md` says so.

   The run stops short of a completed turn unless `CRUCIBLE_SMOKE_KEY` is set,
   because a turn costs tokens. Set it for the release you actually cut:

   ```bash
   CRUCIBLE_SMOKE_KEY=$ANTHROPIC_API_KEY scripts/smoke.sh "$name.tar.gz"
   ```

## Cutting it

```bash
# 1. Bump the single version, and update the changelog in the same commit.
$EDITOR Cargo.toml CHANGELOG.md
cargo build                     # refresh Cargo.lock with the new version
scripts/check.sh

git commit -am "chore(release): 0.0.1"
git push origin main

# 2. Tag the commit CI just proved green.
git tag -a v0.0.1 -m "crucible 0.0.1"
git push origin v0.0.1
```

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
2. Open a fresh `Unreleased` section in the changelog.
3. If the release is broken, do not delete or move the tag. Fix forward with a
   patch release. A tag that changes meaning breaks every checksum anyone
   recorded against it.

## If a release must be pulled

Yank is not available for a binary distribution, so:

1. Mark the GitHub Release as a pre-release so it stops being "latest".
2. Add a `### Removed` note to the changelog saying what was wrong and which
   version supersedes it.
3. Ship the replacement the same day if the defect risks data or credentials.
