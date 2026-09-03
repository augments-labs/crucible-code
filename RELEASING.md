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

   This reading is the authoritative one. The release workflow runs the same
   probes on a shared runner, but those numbers are a trend and publication
   does not wait for them — a budget is decided here, before the tag exists.
4. **The changelog is real.** Move everything under `Unreleased` into a new
   version section with today's date, and add the comparison link. Written for
   someone deciding whether to upgrade, not generated from commit subjects.
5. **The install path works from scratch** — advisory for now: the release
   workflow still runs its smoke job but no longer waits for it before
   publishing, and this local run is
   likewise worth doing when the packaging changed rather than on every tag.
   Every gate above builds from this tree with this machine's toolchain, so
   none of them can see what a shipped binary needs from the machine it lands
   on. Package the artifact the way the release workflow does and put it
   somewhere that holds nothing else:

   ```bash
   VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
   name=crucible-$VERSION-linux-x86_64
   install -Dm755 target/release/crucible "$name/crucible"
   install -Dm755 target/release/crucible-sandbox-broker "$name/crucible-sandbox-broker"
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
`CI required` is green. That aggregate covers Rust, repository, dependency and
performance workflows, so the ruleset needs one stable status as new language
workflows become peers. The ruleset has no bypass and therefore applies to the
release change too.

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

The opening screen draws the version, and the whole-screen pictures mask it out
rather than hold it — one `#` per character, so the row keeps its width. A bump
therefore costs no snapshot change, and the one exception is a version spelled
in more characters than the last, which moves what that screen is laid out
around and is worth looking at. If the gate fails there on a release that
changed only a version and the version's length did not move, something new has
started drawing one: read that diff rather than accepting it blind, because
anything on it is a rendering change that arrived with the release and belongs
in the changelog rather than in a snapshot nobody looked at.

The scheduled `audit` status is not required: it runs only when dependencies
change or on its clock, so many pull requests never receive it. Deterministic
license and source policy is part of `CI required` through the always-called
dependency workflow.

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

Six of those seven block the release. FreeBSD does not: it is built in a
virtual machine on a Linux runner, on infrastructure this project cannot pin or
repair, and it has held six finished platforms for half an hour at a time
waiting for that machine. So it is best-effort, and the publish job counts the
other six before uploading anything — a release short of any of them fails, and
a release short of this one carries a warning saying so. Read the warnings on a
release run before announcing it.

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
4. If a **published** release is broken, do not delete or move the tag. Fix
   forward with a patch release: a tag that changes meaning breaks every
   checksum anyone recorded against it. The `release tags` ruleset refuses the
   move, so that is the remedy whether or not anybody remembered this
   paragraph.

   Where that rule comes from decides how far it reaches, so it is worth
   writing down rather than inheriting. A Go module tag is recorded in a public
   checksum database the first time anybody fetches it, and a tag that moves
   afterwards makes every later build of that version fail outright instead of
   quietly serving different code. crates.io and npm arrive at the same place
   from the other direction, by refusing to publish over a version that already
   exists. Container tags are the counter-example, and this repository depends
   on their being one: `stream9` is meant to move, which is why the release
   workflow pins a digest underneath it and why repointing that pin is an
   ordinary commit rather than an incident. So the rule was never about tags. It
   is about whether something outside this repository has already recorded what
   a version here meant.

   Crucible publishes to none of those registries. It is a binary distribution,
   and the only thing that pins a version is the `SHA256SUMS` line the `publish`
   job uploads with the artifacts. Everything the rule protects hangs off that
   line, and until `publish` runs there is no line.

   A tag that published nothing is therefore the other case, and it is not the
   same one. When a job fails before `publish`, there is no release, no
   artifact, and so no checksum for a move to invalidate. Spending a version
   number on infrastructure that was never the code's fault only leaves the next
   reader comparing two versions that carry identical code. Confirm it shipped
   nothing — `gh release view v<version>` answering `release not found` is the
   check — then repair what broke on `main`, relax the `release tags` ruleset,
   move the annotated tag onto the commit carrying the repair, and put the
   ruleset back before anything else. Restoring it is part of the procedure,
   not a follow-up.

   The friction is deliberate. The published release is the usual case; this
   one is the exception, and it has to be shown to apply before it is used.

## If a release must be pulled

Yank is not available for a binary distribution, so:

1. Mark the GitHub Release as a pre-release so it stops being "latest".
2. Add a `### Removed` note to the changelog saying what was wrong and which
   version supersedes it.
3. Ship the replacement the same day if the defect risks data or credentials.
