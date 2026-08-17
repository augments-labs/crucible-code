# Releasing

Cutting a release is mechanical on purpose. Everything that can fail should fail
in CI, before the tag exists.

## Versioning

`0.x` while the shape is still moving. Configuration files, session files and
the command-line surface may change in any `0.x` release with no deprecation
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
   VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
   name=crucible-$VERSION-linux-x86_64
   install -Dm755 target/release/crucible "$name/crucible"
   install -Dm644 README.md LICENSE -t "$name/"
   install -Dm755 scripts/install.sh scripts/uninstall.sh -t "$name/"
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
`scripts/check.sh`, `scripts/bench.sh` and `400 changed lines` are green on it.
That is a repository ruleset with no bypass, so it holds for the person cutting
the release too — the tired push at the end of a long day is the one it exists
to catch. A required check is the only kind there is: one that merely runs is a
red mark beside a merge button that still works, which is what the size check
was until it was added to that list. It is still on it now that it reports
rather than blocks, so that turning the ceiling back on is one line in the
workflow and nothing here.

```bash
# 1. Bump the single version, and update the changelog in the same commit.
git switch -c release/v0.0.1
$EDITOR Cargo.toml CHANGELOG.md
cargo build                     # refresh Cargo.lock with the new version

# The version is drawn in the session box, so the whole-screen snapshots hold
# it and three of them fail until they are told the new one.
INSTA_UPDATE=always cargo test -p crucible-code --test whole_screen
git diff --stat tests/whole_screen/snapshots

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

Read that snapshot diff rather than accepting it blind. One changed line per
file, the one with the version in it, is what a version bump costs; anything
else on it is a rendering change that arrived with the release and belongs in
the changelog rather than in a snapshot nobody looked at.

`cargo deny` is not a required check, and must not become one. It runs only when
the dependency set changes, so on a release that touches no dependency it never
reports — and a required check that never reports leaves the pull request
pending for ever rather than failing it.

Pushing the tag is the trigger. The release workflow builds every artifact,
checksums and attests them, and opens the GitHub Release with the changelog
section as its body.

## Artifacts

| Platform | Target | Artifact |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | `crucible-<version>-linux-x86_64.tar.gz` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `crucible-<version>-linux-aarch64.tar.gz` |
| macOS Apple silicon | `aarch64-apple-darwin` | `crucible-<version>-macos-aarch64.tar.gz` |
| macOS Intel | `x86_64-apple-darwin` | `crucible-<version>-macos-x86_64.tar.gz` |
| Windows x86-64 | `x86_64-pc-windows-msvc` | `crucible-<version>-windows-x86_64.tar.gz`, `.exe` |
| Windows ARM64 | `aarch64-pc-windows-msvc` | `crucible-<version>-windows-aarch64.tar.gz`, `.exe` |
| FreeBSD x86-64 | `x86_64-unknown-freebsd` | `crucible-<version>-freebsd-x86_64.tar.gz` |

`install.sh` and `uninstall.sh` are standalone release assets and are also in
every archive. The Bash installer is for Unix targets; Windows uses the bare
executable and the manual checksum path documented in Getting started.

An artifact is named for the version it holds rather than for the tag it was cut
from, so there is no `v` in it — the `v` belongs to git. The triple is what the
build is; the name is what somebody has to recognise in a list, and the two are
not the same job.

Each Windows target ships the bare `.exe` beside its archive, because the usual
way to get one on Windows is to download it and run it.

One `SHA256SUMS` covers the lot, rather than a file beside each artifact: a
single line to publish and a single file to check against.

Every target builds on a runner of its own architecture, FreeBSD in a virtual
machine and the other six natively — nothing here is cross-compiled. Builds are
release profile — fat LTO, one codegen unit, symbols stripped — the same
settings every published measurement was taken under, so the numbers in the
changelog describe the binary people actually download.

The Linux containers and FreeBSD guest begin without Rust. They download a
versioned `rustup-init` for their native target, compare it with the SHA-256
fixed in the workflow, and only then execute it. `rust-toolchain.toml` still
chooses the compiler; pinning the bootstrapper prevents a release tag from
executing whatever the moving `sh.rustup.rs` endpoint serves that day.

## After the tag

1. Run the same gate against what was actually published, which is the one
   artifact no earlier step was allowed to trust:

   ```bash
   scripts/smoke.sh v0.0.1
   ```

   Given a tag rather than a file it downloads the release, checks the tarball
   against the published `SHA256SUMS`, and runs everything from step 5 on that
   copy — so a build that was fine locally and an upload that went wrong are told
   apart rather than averaged. It is the Linux x86-64 artifact it looks for: the
   sandbox it runs in is a Linux one, and the other six are proved by their own
   native build and by CI rather than from here.

   The tag is what the version it reads back is measured against, so this runs
   the same way months later from a `main` that has moved on — which is the
   point of a gate about a published artifact.
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
   [SchemaStore/schemastore](https://github.com/SchemaStore/schemastore),
   raised from the fork
   [NjoyimPeguy/schemastore](https://github.com/NjoyimPeguy/schemastore) — it is
   already there, and a second one is a second place for a release to be half
   done. Branch off *their* `master` rather than the fork's, which is however
   stale it was left, and replace
   `src/schemas/json/crucible-code-schema.json` with the file this tag ships.
   Run their formatter and their gate over it before committing:

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
   is unstable for the whole 0.x line — an editor is a hint, and the program is
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
