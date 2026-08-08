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
5. **The install path works from scratch.** In a clean container or a fresh
   user, install the built artifact and run one real session end to end.

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
| `x86_64-unknown-linux-gnu` | `crucible-v0.0.1-x86_64-unknown-linux-gnu.tar.gz` |

Each archive ships beside a `.sha256` file. Builds are release profile — fat
LTO, one codegen unit, symbols stripped — the same settings every published
measurement was taken under, so the numbers in the changelog describe the binary
people actually download.

Other targets are added when someone runs them; a platform nobody tests is a
promise nobody keeps.

## After the tag

1. Download the published artifact — not your local build — verify the checksum,
   and run `crucible --version`.
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
