#!/usr/bin/env -S python3 -I
"""The agent catalog's intake: registry `binary`, `uvx` and `npx` entries become review data.

The intake runs in the catalog repository's CI, between a job that downloads
and starts vendor programs with no token, and a job that signs what a
maintainer merged. It holds no token and no signing key, signs nothing and
merges nothing, and it starts no program, `crucible` included: what
`crucible agents unpack-check` and `crucible agents probe` printed on each
target's runner is read from the files the untrusted job left. Everything it
reads is hostile, so it runs in isolated mode (`-I`), reads every file within a
ceiling before parsing it, refuses JSON with a repeated key or a non-finite
number or text UTF-8 cannot carry, and shows nothing of it in the human diff
but whole, inside a code span it cannot close, with every character that could
hide or reorder it written as its code point. Whatever one target or entry
breaks drops it alone, with its reason, and the run goes on with the others;
one run reads at most `RUN_BYTES` of observed data within `RUN_SECONDS`, and
unpacks at most `REPACK_RUN_DECOMPRESSED` from every `.tar.bz2` it repacks
and every build archive it checks, and what is left once any is spent waits for the next run.

`review` reads:

- `--registry`: the ACP registry index, at most 1 MiB and 512 agents;
- `--choices`: the reviewer's own choices, by registry id: the agent `key`
  crucible names it by, chosen once and never renamed, and the `reach` it may
  have once running;
- `--runners`: the runner matrix, by platform: whether its probe runs
  `confined` in a sandbox with the network closed, and the `reason` where not;
- `--observed`: what the untrusted job left, one directory per registry id and
  platform, `<id>/<platform>/`, holding `artifact` (the download), `fetched.json`
  (`{"chain": [...]}`, every address it was fetched through, the registry's
  first), `unpack-check.json` and `probe.json`; neither directory may be a
  link. Where the download is a `.tar.bz2`, `repacked` beside it is the job's
  repack of it, by `repack` below, and `unpack-check.json` lists that repack.
  A FreeBSD runtime's directory holds the pinned `.pkg` as `artifact`, its
  `fetched.json`, whose chain begins at the pin, the bundle `bundle` below
  made of it as `repacked`, `unpack-check.json` listing that bundle, and the
  bundle's SBOM as `sbom.json`. A `uvx` agent's directory instead holds `resolved.json` (pip's
  `--report` for the package on that platform's pinned interpreter, of which
  only `version`, and each package's `download_info.url`, `metadata.name` and
  `metadata.version`, are read), `interpreter/` and `wheels/<n>/` (`n` the
  package's place in the report), each holding the `artifact`, `fetched.json`
  and `unpack-check.json` of one download, and `probe.json`; no directory on
  the way may be a link. An `npx` agent's directory holds `resolve/`
  (`package-lock.json`, npm's lock for the package), `tarballs/<n>/` (`n` the
  package's place in the lock's `packages`, the root counted as 0), each the
  `artifact` and `fetched.json` of one tarball, and per platform `tree/`
  (`lock.json`, `npm.json` and `unpack-check.json`, below) and `probe.json`.
  Node's own downloads are under `node/`: `SHASUMS256.txt.asc` and its
  `fetched.json`, and `<platform>/` holding the `artifact` and `fetched.json`
  of each build, or on `freebsd-x86_64` the FreeBSD runtime, so an agent whose registry id is `node`, or is another's,
  once case is folded, is dropped;
- `--interpreters`, where given: the reviewer's interpreter pins, by
  platform, each `{"url", "sha256", "python"}`: the `https` address of a
  python-build-standalone `install_only` build, its SHA-256, and the `3.M` it
  runs; on `freebsd-x86_64` the pin is instead a FreeBSD package,
  `{"url", "sha256", "abi", "python"}`: the `https` address on
  `pkg.freebsd.org` of a `.pkg` under `/<abi>/`, plain, its SHA-256, its ABI,
  `FreeBSD:<major>:amd64`, and the `3.M` it runs, bundled as below, where
  only a wheel tagged `any` fits. A `uvx` target on a platform with no pin is
  dropped;
- `--node`, where given: the reviewer's Node pin, `{"version": "X.Y.Z"}`,
  an even major line from 22, and 22.15.0 or later on line 22, with, where
  given, `"freebsd-x86_64": {"url", "sha256", "abi", "version"}`, a FreeBSD
  package as the interpreter pin's is, whose `version` is in the pin's major
  line and 22.15.0 or later. With no pin, every `npx` target is dropped, and
  with no FreeBSD package, every one on `freebsd-x86_64`. Giving it needs
  `gpg` on `PATH`;
- `--node-keys`, where given: the directory of the keys a Node release may be
  signed under, each `<fingerprint>.asc` and a `fingerprints` list of the
  primary fingerprints accepted; the keys vendored beside this script where
  not given;
- `--dispositions`, where given: the reviewer's decision on each install
  script `--ignore-scripts` skipped, by registry id and then
  `"<location> <event>"`, one of `inert`, `replaced` or `lost`; `built` and
  `local` are what review classes a script the reviewer leaves undecided, from
  what the job left of it, below, and given here refuse the run;
- `--deny`: `deny.toml`, whose licence list decides what may be mirrored;
- `--sboms`, where given: the input of each artifact marked for mirroring,
  its lockfile or SBOM, as `<artifact sha256>.json`, which the licence gate
  below reads; with it, `--texts`, the licence texts the gate ships;
- `--merged`, where given: the merged data, against which a version already
  reviewed is held. A release whose content differs from what was reviewed
  under the same version is refused, naming what differs, since a changed
  release is taken in only under a new version. An `npx` agent's targets'
  locks are an exception: the tree npm resolves for one version can
  change, so the entry keeps the merged `lock_revision` while every target's
  lock is the merged one, and takes the next where any differs; any other
  difference is refused as for every agent. An artifact's `licence_gate`
  record is not compared either: this review's inputs decide it, and the
  record made now stands.

It writes the review data to `--out`, and each file-list asset (a per-file list
longer than 256 entries, kept as an asset named by its SHA-256) and each npm
lock under `--assets`. A target is dropped when its download is missing, larger than
1 GiB or not the bytes the registry pinned, when it was fetched through
anything but `https` to a named host, when its `cmd`, `args` or `env` is not
what a catalog may hold, `env` naming a loader variable among them, or when its
per-file list breaks a rule, a link that escapes its tree among them. An entry
is dropped when no target is left, when its reach names a literal address or
every host, or when any target left lacks complete probe output: a probe that
ran without a sandbox is accepted only on a runner the matrix lists as
unconfined, and is marked for the reviewer; a sign-in method whose `tty` is
not `true`, `false` or `unknown`, and any conformance result but `not run`, is
refused; and probes on different targets must agree on `agentInfo` and the
sign-in methods, the refusal naming the field that differs. Each drop is
recorded with its reason. An artifact whose declared licence is on `deny.toml`'s list is marked
with that licence for mirroring, and carries the licence gate's record over
its whole input as its `licence_gate`; one whose input `--sboms` does not
hold is recorded as failing, `no lockfile or SBOM was given`, so it keeps its
vendor address and is not mirrored. A passing record's NOTICE and licence
texts are written under `--assets` by their SHA-256; the run holds them
within 64 MiB together until then, each distinct file counted once, and an
artifact whose NOTICE would pass that fails the gate. An output built or
repacked from an artifact is gated the same way, and where it passes it is
the artifact's `replaced_by`, the asset installed in its place, by the hash
and size its record binds.

A `.tar.bz2` download, which the client does not unpack, is repacked by
`review` itself from the bytes it hashed, as `repack` writes it, and dropped
unless that repack is the job's `repacked` byte for byte, so the listing is of
the bytes that ship; one not marked for mirroring is dropped, since it cannot
be repacked. The artifact keeps the download's address, hash and size, its
`format` is `tar.gz`, its listing is the repack's, and the licence gate runs
over the repack, from the input named by the download's SHA-256; a failing
record drops the target, which has no route to install but its repack. A
FreeBSD runtime is taken the same way from its pinned `.pkg`, with the
bundle's SBOM as the gate's input, and every file of the bundle kept; a
FreeBSD runtime missing or refused drops only the targets on
`freebsd-x86_64`. `review` cannot rebuild a FreeBSD bundle as it rebuilds a
repack: the bundle is the untrusted job's output, held only by the pinned
`.pkg`, the client's listing of it, the licence gate, and the FreeBSD probe.
A passing repack or bundle is written under `--assets` by its SHA-256, the
run holding at most 1 GiB of them together, and one past that is dropped.

`repack` writes the tar.gz `review` makes of a `.tar.bz2` (`--input`) to
`--out`, whole or not at all: a GNU tar, which the client's tar reader reads,
its long names in GNU long name headers, in one gzip stream at level 9 whose
header carries mtime 0 and no file name. Each member keeps its order, name,
kind, permission bits and content; its owner and group are 0, their names
empty, and its mtime 0, so two repacks of one download are the same bytes
where the same zlib writes them. The download must be one bzip2 stream with
nothing after it, of at most 1 GiB, unpacking to at most 4 GiB, and every
member is held to what the client's unpacker accepts: a regular file, a
directory or a symbolic link with a relative target, no hard link, device,
FIFO, sparse member or global extended header, at most 65,536 members, every
name one the client writes, and none written twice once case is folded.

`bundle` checks and packs the runtime the FreeBSD step staged (`--staged`):
the program, alone in `bin/`, the libraries it bundles in `lib/`, CPython's
standard library under `lib/python3.M/`, and each package's licence files
under `licenses/<package>/`. `--ldd` is what `ldd` printed for the program
and every staged library once their run path is `$ORIGIN/../lib`, read
within 1 MiB and parsed once, a line that does not parse refusing the run;
every library it names must resolve into the staged `lib/` or the base
system's `/lib/` or `/usr/lib/`, and one `not found`, under `/usr/local/` or
anywhere else cannot be bundled and refuses the run. A file in `lib/` no
`ldd` line needs, and a staged file no listed package installs, refuse it
too: a listed path installs the staged file it names under `/usr/local/`
alone, so `/usr/local/lib/libfoo.so.1` installs `lib/libfoo.so.1`, and a path
anywhere else installs nothing. A refusal shows what `ldd`, the package list
or the staged names gave it escaped, as the human diff shows hostile text. `--packages` is the package list the step's `pkg query` gave, within
16 MiB: `{"packages": [{"name", "version", "licenses", "logic", "files"}]}`,
`licenses` the ports licence names, `logic` one of `single`, `and` or `or`,
and `files` the absolute paths it installs. `--abi` is the package ABI,
`FreeBSD:<major>:amd64`. It writes the staged directory as `repack` writes a
tar.gz to `--out`, and to `--sbom` a CycloneDX JSON SBOM of the packages,
which `licence-gate` reads, each package's licences an expression through
the ports names `APACHE20`, `ART20`, `BSD2CLAUSE`, `BSD3CLAUSE`,
`BSD4CLAUSE`, `BSL`, `ISCL`, `MIT`, `MPL20`, `OPENSSL`, `PSFL` and `ZLIB`,
and any other name `NOASSERTION`, which the gate refuses; both whole, or
neither. It runs neither `ldd` nor `pkg`, and reaches no network.

A `uvx` agent's package is pinned as `name==version`, and each platform the
runner matrix names is a target of its own: the pinned interpreter, started
with `-I`, and every package pip's report installs on it, each a wheel from
`https://files.pythonhosted.org/packages/`, at a plain address (no `%`,
query, fragment, parameter, backslash, or empty, `.` or `..` segment) that
every hop of its fetch keeps on that host, whose file name agrees with the
report and whose tags that interpreter on that platform accepts (Linux wheels
for glibc 2.34 or older, macOS ones for 11.0 on Apple silicon and 10.12 on
Intel). A target is dropped when the interpreter is not the bytes its pin
names, when a wheel holds a part of `.data` but `purelib` or `platlib`, which
the client does not install, when the agent's own wheel names no console
script by the package's name as `module:attr`, when an interpreter file lies
outside `python/` or a wheel's outside `site/`, when two of its artifacts
list one path, when a wheel lists one member twice (a leading `./` removed
and case folded, as the client compares names), when a wheel does not end as
a wheel ends (its end record last, with no comment, and any zip64 record just
before its locator, which points at it), or when that record claims more
members or a larger central directory than the stated ceilings. A package
pip found only as a source distribution installs as the wheel the job built
from it, which the job leaves under its download's `wheels/<n>/built/`:
`name.json` (`{"file"}`, the wheel's file name), `artifact`, the wheel, read
within 1 GiB, and `unpack-check.json` listing it; the source distribution's
own `artifact` and `fetched.json` are hashed and checked as a wheel's are.
The built wheel is held to every check a downloaded one is, and to the
licence gate below over the SBOM named by the wheel's SHA-256, no SBOM
failing it; the artifact keeps the source distribution's address, hash and
size, takes the wheel's format and listing, and names the gated wheel as
`replaced_by`, kept as an asset as a repack is. A failing gate drops the
target with its reason. A source distribution the job built no wheel from
drops its target with a `build` list naming each one, clipped as a reason is,
so it waits for a build at review. An interpreter and its wheels are never marked for
mirroring, and are never mirrored but on `freebsd-x86_64`, where the
interpreter installs as its bundle, shipped under its licence gate as below.

An `npx` agent's package is pinned as `name@version`: a lowercase npm name,
scoped or not, and one exact version, so a range, a tag or an `npm:` alias
drops it. It runs on Node, which is checked before any agent: the signed
`SHASUMS256.txt.asc` for the pin, fetched from
`https://nodejs.org/dist/v<version>/`, is verified with `gpg` in a home of
its own and taken only under one good signature over a SHA-2 digest, made
before its key and that key's primary expired, by a key whose primary
fingerprint is listed, and only the
`<sha256>  <file>` lines it signed are read, none twice. Each platform's
build must be the bytes that list gives it, fetched through `nodejs.org`
alone; its `bin/node` (`node.exe` on Windows) and `LICENSE` are all a client
keeps of it, and Node is never mirrored but on `freebsd-x86_64`: Node
publishes no FreeBSD build, so there Node is the pinned FreeBSD package,
bundled as below and installed as that bundle, shipped under its licence gate. The tree is npm's lock, in lockfile version 3, of
which only each package's location, name, version, `resolved`, `integrity`,
`inBundle`, `hasInstallScript`, `dev`, `optional`, `os`, `cpu` and `libc`
are read; a `dev` package is never installed. Every location is
`node_modules/<name>`, nested only under `node_modules/`, unique once case is
folded, and no longer than 767 bytes. Every tarball is fetched from
`https://registry.npmjs.org/` at a plain address of printable ASCII with
no space, and every hop of its fetch
stays on that host; its SHA-512 and size are computed here, a SHA-512
`integrity` must agree with it, a SHA-1 one is held only where it is the only
one, and a package with neither is refused. A tarball is read as the client
reads it, each header as it is written. A global one holding comment records
alone, which use up its body, is accepted wherever the client accepts one,
between members included, unless it follows another or comes between a
member's own headers and the member. A tarball must hold one gzip stream
and nothing after it, at most 65,536 members, an ordinary file, a
directory or a link alone, every name one the client writes, inside its
package and the ones it bundles and not straight under a `node_modules`
directory, none written twice once case is folded nor one directory spelled
in two cases, and at most 131,072 names, directories included, holding at
most 8 MiB; and a lock's tarballs write at most 65,536 files and 4 GiB
together. Each package's own package.json,
a bundled one's included, must give the name and version the lock gives it. A
bundled package has no tarball of its own and is counted in the one that
holds it. A target's lock, the asset a client unpacks its tree from, lists
every package installed on it, in the lock's order, an optional one whose
`os`, `cpu` or `libc` refuses the platform left out (Linux runners are
glibc, and a `libc` refuses any other system), decided from the lock as npm
decides before npm runs, each with the licence its package.json states, where
it states one as text, and the programs it declares by `bin` or
`directories.bin`; it is named by its SHA-256, so targets with the same tree
share one. A licence or programs the client would refuse drop the target: a
licence that is not printable ASCII of at most 256 bytes, a program whose name
is not one file name or is Node's own in any case, whose script lies in another package or is not a file
the tree holds, two whose names differ only in case, one two packages at the
top of the tree declare or two below it and none at it, and more than 64 in
one package or 1,024 in the tree. The target is dropped unless the job's
`tree/lock.json` is that lock byte for byte and the tree `npm ci` built
(`tree/npm.json`) and the one `unpack-check` built from the lock
(`tree/unpack-check.json`) agree on every file under an installed location,
npm's own `.bin` links and `.package-lock.json` aside, but for an execute bit
on a file a package names as a `bin`, a `.gitignore` npm renamed from an
`.npmignore` of the same bytes, and any execute bit on Windows, where nothing
is marked runnable. The agent starts as `npx` would start it: Node with the
one `bin` the agent's own package names, or the one named for the package.
Each `preinstall`, `install` and `postinstall` script skipped, an `install`
implied by a `binding.gyp` among them, is listed on the target with the
reviewer's disposition, where npm's `hasInstallScript` must agree with the
scripts the package's own `package.json` gives. One the reviewer did not
decide is classed from what the job left of it in its target's
`scripts/<the SHA-256 of "<location> <event>">/`, a name that cannot climb
out of the target nor collide, checked as every directory on the way is; one
the job left nothing of is listed as `undecided` in the review data, marked
in the diff, and refused by `render`. The job leaves there `run-1.json` and
`run-2.json`, each the `tree` listing of the package's directory after one
run of the script, at two roots and under two `HOME`s, read within the
ceiling of a tree listing and refused past the client's ceilings for a tree
as incomplete, and `hosts.json`, `{"hosts": [...]}`, the at most 64 hosts the
recording proxy saw over both runs, read within 1 MiB. A script is a build
by rule when the command its package's `package.json` gives for the event,
or the `node-gyp rebuild` a `binding.gyp` implies, holds a character that
could hide what it runs (a control, bidi, zero-width or other such one) or
one a shell or Windows expands (`$`, a backquote, `*`, `?`, `[`, `]`, `~`,
`{`, `}` or `%`), names a compiler, a native-addon builder, a build tool or a
package manager (`cc`, `gcc`, `g++`, `c++`, `clang`, `clang++`, `cl`,
`node-gyp*`, `node-pre-gyp`, `cmake-js`, `prebuild*`, `make`, `gmake`,
`nmake`, `cmake`, `ninja`, `cargo`, `rustc`, `go`, `npm`, `npx`, `yarn`,
`pnpm`, `pnpx`, `bun` or `bunx`) as any word of it split as a shell splits it, by its last path
segment, case folded, with an `@version`, one `.exe`, `.cmd`, `.bat`, `.js`,
`.cjs` or `.mjs` and one `-<digits>` version, dotted or not, taken off,
or ending, as a cross compiler's target triplet does, in `-gcc`, `-g++`,
`-cc`, `-c++`, `-clang`, `-clang++` or `-ld`, so `npx node-gyp`, `gcc-12`,
`x86_64-linux-gnu-gcc` and `npm run build` are each one, or cannot be
split; it is a build on every `windows-*` and
`freebsd-*` platform, whatever it runs; and it is a build by difference when
its two runs left listings that differ, or a run left none, a broken one, or
one past the ceilings. Otherwise it is `local`: the two listings are the same
bytes, and the entry lists the script with its `command`, the `hosts` it
reached, each a host's name, and `result`, `{sha256, size}`, the listing,
kept as an asset within the ceiling of one and, with every NOTICE and
licence text, within the 64 MiB a run holds of them, past which the target
is dropped, so the client shows all three
before it runs it. A build installs as what the job built: `built.tar.gz`
beside the runs, the package's directory after the build as `pack` writes
it, read within 1 GiB, which review writes again through the same writer
and refuses unless it is the same bytes, holds files and directories alone,
and holds the files `built.json` lists, as `tree` lists them; the licence
gate below then runs over the SBOM named by the archive's SHA-256, no SBOM
failing it, and on a pass the archive is kept as an asset, counted with the
repacks, and the script is listed `built`, its record and output on the
script in the review data, where the diff shows them, and left out by
`render`. A build the job left no `built.tar.gz` of, or whose gate fails,
drops the target with the reason.

`pack` writes the package directory `--root` after a build to `--out` as
`repack` writes a tar.gz, whole or not at all: its names in sorted order,
directory by directory, each directory mode 0755 and each file 0755 where any
execute bit is set and 0644 otherwise, so two builds of the same files are the
same bytes; a link or any other kind of name refuses it, as does an archive
past 1 GiB. The catalog repository's untrusted job runs each build and
source distribution, under the recording proxy, and leaves what is above;
review never runs either.

The job before the intake runs, for each `npx` agent, `npm install
--package-lock-only --ignore-scripts --omit=dev
--registry=https://registry.npmjs.org/ --before=<seven days ago>` to write
`resolve/package-lock.json`, and fetches each tarball it lists; then, on each
platform's runner, `lock` to write `tree/lock.json`, `npm ci --ignore-scripts
--omit=dev` with the pinned Node, `tree` over what it installed to write
`tree/npm.json`, and `crucible agents unpack-check` on the lock to write
`tree/unpack-check.json`.

`lock` writes the lock `review` computes for one platform (`--platform`)
from an agent's observed directory (`--resolved`) and its package
(`--package`, `name@version`), so the trees the job compares are of the lock
the catalog will list. `tree` lists the regular files under `--root`, as
`{"files": [{path, sha256, size, exec}]}` sorted by path, `exec` set where
any execute bit is; a link is neither followed nor listed, and the listing
stops at the client's ceilings for a tree. `recheck` runs daily: from the
merged data and the directory of lock assets it lists each tarball the
merged npm locks name, once, by its SHA-512 and address; given what the
registry answered for each (`--results`, the status by SHA-512), it proposes
a revocation, by key, version and lock revision, of every lock holding a
tarball answered 404 or 410, which a fresh install could no longer fetch,
and none for any other answer, which may pass. The proposals are written as
a bare list in the shape of the merged data's `revoked`, `{key, version,
lock_revision, reason}`, so a reviewer who agrees appends them as they are.

`licence-gate` decides whether an asset may be published by the catalog,
as a mirror, a repack or a build: only when every component of the build's
whole input (`--input`) is used under licences `deny.toml`'s list (`--deny`)
holds. The input is a CycloneDX JSON SBOM (`bomFormat` `CycloneDX`), each of
whose `components`, and its subject `metadata.component` where that gives
a licence, gives its `name`, `version` and `licenses`, each a
`license.id` or an `expression`, all of which apply, or the intake's own npm
lock, each of whose packages gives the licence its package.json states. It
is read within 32 MiB and 65,536 components, and anything else is refused, a
component that nests others among them. A component fails the gate with no
licence, with `NOASSERTION` or `NONE`, a `LicenseRef-` or `DocumentRef-`
reference, an id with a `+`, an expression over 256 bytes or one that does
not parse as `AND`, `OR`, `WITH` and parentheses, or an id the list does not
hold; an `OR` takes the first alternative, in written order, that the list
holds, and an `X WITH Y` passes only where the list spells it whole. A
component also fails where its name, version or copyright lines hold a
character that could hide or reorder what the NOTICE shows, as `diff` shows
such characters (line breaks and tabs are kept in copyright lines). One
component failing fails the whole input, and the refusal names the first.
Where it passes, it writes into `--out`, a directory that must not exist yet,
`NOTICE` (every component as `name version licence`, sorted, each followed
by the copyright lines its SBOM carries as a licence's `text.content`,
within 16 KiB each and 1 MiB together, and then each licence's text once)
and `LICENSES/<id>.txt`, each text taken from `--texts/<id>.txt`, a licence
with none there failing the gate; the same input gives the same bytes. Both
ship beside the asset as catalog release assets. It prints its record:
`{"passed": true, "asset", "notice", "licences", "components"}`, the first
two each a `{sha256, size}`, `licences` a `{id, sha256, size}` per text and
`components` their count, so the record names the exact asset gated (`--asset`)
and what ships beside it; or `{"passed": false, "reason"}`, and the run is
refused. A write that fails removes `--out` again, so a rerun is not
refused for what it left.

`diff` writes the human diff of review data against the merged data before it,
as Markdown, showing only what changed, compared on whole values. A version
the merged data already holds is passed over only when it is the same in every
field; otherwise it is shown, marked as a same-version change, or, where only
an artifact's `licence_gate` record differs, as a record that changed. Each
artifact's record is shown where it is new or changed, absent, failing or
passing, so the reviewer sees an artifact become one that may be
redistributed; a passing record is shown with every file it binds, the NOTICE
and each licence text by hash and size, so two records that differ never read
the same.

`render` writes the catalog a client reads, from the merged data (`--merged`,
review data a reviewer completed and merged, with the `revoked` list a
reviewer adds to it, `{key, version, reason}` and `lock_revision` where only
one revision of a version is withdrawn) and, where there is one, the catalog
published before it (`--previous`). The serial is the previous one raised by
one (1 where none), `--signed-at` is the signing time in whole seconds,
`expires` is thirty days after it, and `revoked` holds every revocation either
source ever made, once each, since a revocation outlives the data that made
it. Entries pass through but for what only a reviewer reads (each target's
`probe` record, an artifact's `mirror_licence` mark and `licence_gate`
record); a long file list is
already an asset the entry names by hash and size. The bytes are sorted keys,
no spaces, UTF-8 and no floats, so the same data and time give the same bytes.
Nothing is written, and the run is refused, for a catalog over `INDEX_BYTES`
or holding more than `REVOKED` revocations; for a previous catalog that is not
in this schema, has a serial below 1, was signed after `--signed-at`, or lists
no `revoked` (an absent list is never read as an empty one, so a revocation
cannot be dropped unseen); for a number outside 0 to 2**53-1; and for an entry
missing a field the client's reader requires or with a key that is not an
agent key. Signing is not done here: the publishing job
signs the bytes with minisign.

`publish-check` runs in the publishing job before anything is uploaded. It
reads the merged data (`--merged`) and the directory of assets the job would
upload (`--assets`), each a regular file named by its lower-case hexadecimal
SHA-256 with no extension, and refuses the run, uploading nothing, when any
asset is not one of these: a file whose name is not a hash, whose bytes do not
hash to its name, whose hash the merged data does not hold, or whose size is
not the one the data records. A file-list or lock asset is the catalog's own
data and needs no more. An artifact copy, a mirror included, and an artifact's
`replaced_by` are redistributions, and each passes only where its entry's
`licence_gate` record passed in the shape the gate writes, that record's
`asset` is those very bytes, and, for a copy, the artifact carries its
`mirror_licence` mark; the same bytes under two entries pass only where both
do. Each redistributed asset uploaded ships with the NOTICE and every
licence text its record gives, or the run is refused. A NOTICE or licence
text passes by the hash and size a passing record gives it. A hash the data
names both ways answers to both rules. An empty directory passes, since
nothing is then uploaded. An asset is opened without following a link, but
the check cannot hold a directory still: the publishing job checks and
uploads from a directory the untrusted job cannot write once the check has
begun, or the check proves nothing about what is uploaded.
"""

from __future__ import annotations

import argparse
import base64
import bz2
import configparser
import gzip
import hashlib
import io
import json
import os
import pathlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import unicodedata
import urllib.parse
import zipfile
import zlib

# The ceilings of what is read, each before it is parsed.
REGISTRY_BYTES = 1 << 20
AGENTS = 512
ARTIFACT_BYTES = 1 << 30
UNPACK_CHECK_BYTES = 16 << 20
PROBE_BYTES = 1 << 20
SMALL_BYTES = 1 << 20
DATA_BYTES = 64 << 20
# What one run of `review` may read in all, and how long it may take; what is
# left once either is spent is dropped, to be reviewed at the next run.
RUN_BYTES = 64 << 30
RUN_SECONDS = 3 * 60 * 60
# What one value may hold: arguments, variables, addresses, methods.
ITEMS = 64
TEXT_BYTES = 4096
CAPABILITY_BYTES = 4096
# The catalog's own limits.
LISTED_FILES = 256
ASSET_BYTES = 4 << 20
LISTED_ENTRIES = 1 << 20
LINK_STEPS = 8
VERSION_BYTES = 64
FORMAT = 1
# What a rendered catalog may be, as the client that reads it allows: the
# bytes of the index, and the revocations it carries; and its life.
INDEX_BYTES = 1 << 20
REVOKED = 4096
SCHEMA = 1
LIFETIME_SECONDS = 30 * 24 * 60 * 60
# The largest whole number a catalog's serial or time may be, one every JSON
# reader holds exactly.
WHOLE = (1 << 53) - 1
# How much of one hostile text a reason quotes, its head and its tail, which
# is more than any host is long; and how many changed files the diff names.
REASON_CHARS = 300
SHOWN_FILES = 50
# The most differing fields a reason names.
NAMED_FIELDS = 8
# A `uvx` target's own ceilings: the packages pip's report may install, the
# report's bytes, which carry each package's metadata, and the bytes of the
# agent's own `entry_points.txt`.
WHEELS = 256
REPORT_BYTES = 16 << 20
ENTRY_POINTS_BYTES = 64 << 10
# The bytes a wheel's central directory may take, read from its own record
# before `zipfile` holds it: 46 bytes, the smallest member header, for each of
# `LISTED_ENTRIES` members, so no directory within it holds more headers than
# the entry ceiling allows.
CENTRAL_DIRECTORY_BYTES = 46 * LISTED_ENTRIES
# The one host a wheel is fetched from, at every hop of the way.
PYPI_HOST = "files.pythonhosted.org"
# An npm target's own ceilings: npm's lock and the packages it lists; one
# tarball, the bytes it unpacks to and the bytes of a package.json in it; the
# two listings of the tree that are compared; Node's signed checksums, and
# one vendored key; and the reviewer's dispositions.
LOCK_BYTES = 32 << 20
LOCK_PACKAGES = 8192
TARBALL_BYTES = 256 << 20
TARBALL_DECOMPRESSED = 1 << 30
PACKAGE_JSON_BYTES = 1 << 20
TREE_LISTING_BYTES = 64 << 20
TREE_ENTRIES = LISTED_ENTRIES
SHASUMS_BYTES = 1 << 20
KEY_BYTES = 64 << 10
DISPOSITIONS_BYTES = SMALL_BYTES
# The files and bytes one lock's tree may write, and how long a location and
# a version may be, as the client that unpacks it allows.
TREE_FILES = 1 << 16
TREE_BYTES = 4 << 30
NPM_PATH_BYTES = 767
# The most members one tarball may give, and the most bytes and components a
# name written from it may have, in any one component and in all.
TARBALL_MEMBERS = 1 << 16
# A tar block, and the most bytes a GNU long name or link, a name an extended
# header gives, and an extended header itself may hold, as the client reads
# them.
TAR_BLOCK = 512
LONG_NAME_BYTES = 4097
EXTENDED_BYTES = 64 << 10
# The most names one tarball's files and the directories above them make,
# each held once, as the client allows a tree; and the most bytes those
# names, each component held once, may hold, 64 a name on average.
TREE_NAMES = 1 << 17
TARBALL_NAME_BYTES = 64 * TREE_NAMES
# The longest licence a package's lock entry may give, and the most programs
# one package and one tree may declare, as the client reads them.
LICENSE_BYTES = 256
# The licence gate's ceilings: its input, the components it lists (no fewer
# than the files one npm tree may write), each licence text it ships, the
# copyright lines a component carries, alone and together, and the NOTICE.
GATE_INPUT_BYTES = 32 << 20
GATE_COMPONENTS = 1 << 16
LICENCE_TEXT_BYTES = 1 << 20
COMPONENT_TEXT_BYTES = 16 << 10
COMPONENT_TEXTS_BYTES = 1 << 20
NOTICE_BYTES = 16 << 20
# What one `review` holds of every NOTICE and licence text it ships.
GATE_HELD_BYTES = 64 << 20
# A repack's ceilings: the bytes it writes, as many as a download may be; the
# bytes a `.tar.bz2` may unpack to, the client's own ceiling on a release, and
# every `.tar.bz2` one `review` unpacks together, past which the rest wait for
# the next run; the one compression level it writes at; and what one `review`
# holds of every repack and FreeBSD bundle it ships.
REPACK_BYTES = 1 << 30
REPACK_DECOMPRESSED = 4 << 30
REPACK_RUN_DECOMPRESSED = 16 << 30
REPACK_LEVEL = 9
REPACK_HELD_BYTES = 1 << 30
# A FreeBSD bundle's inputs: what `ldd` printed, and the package list.
LDD_BYTES = 1 << 20
PACKAGES_BYTES = 16 << 20
# The FreeBSD a runtime is bundled for, by its package ABI, and the one host
# its package is fetched from.
FREEBSD_ABI = re.compile(r"FreeBSD:[1-9][0-9]{0,2}:amd64")
PKG_HOST = "pkg.freebsd.org"
# Where a FreeBSD package installs what a bundle stages under bin/ and lib/.
PKG_PREFIX = "/usr/local/"
# The SPDX licence each FreeBSD ports licence name stands for. A name not
# here is `NOASSERTION`, which the licence gate refuses.
PORTS_LICENCES = {
    "APACHE20": "Apache-2.0",
    "ART20": "Artistic-2.0",
    "BSD2CLAUSE": "BSD-2-Clause",
    "BSD3CLAUSE": "BSD-3-Clause",
    "BSD4CLAUSE": "BSD-4-Clause",
    "BSL": "BSL-1.0",
    "ISCL": "ISC",
    "MIT": "MIT",
    "MPL20": "MPL-2.0",
    "OPENSSL": "OpenSSL",
    "PSFL": "PSF-2.0",
    "ZLIB": "Zlib",
}
PACKAGE_PROGRAMS = 64
TREE_PROGRAMS = 1024
NAME_COMPONENT_BYTES = 255
NAME_COMPONENTS = 64
# The devices Windows takes a name for whatever follows it after a dot,
# besides `com0` to `com9` and `lpt0` to `lpt9`, and the characters it
# refuses in a name besides the backslash and the colon.
WINDOWS_DEVICES = {"con", "prn", "aux", "nul", "conin$", "conout$"}
WINDOWS_REFUSES = '<>"|?*'
SEMVER_BYTES = 256
# How long one gpg call may take.
GPG_SECONDS = 60
# The oldest Node a pin may name: an even, long-term-support, major line, and
# on line 22 the first release with `module.registerHooks`, which the client
# starts an agent's loader with.
NODE_FLOOR = (22, 15, 0)
NODE_VERSION = re.compile(r"(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,3})")
NODE_DIST = "https://nodejs.org/dist/"
# Node's build for each platform, by the name its release gives it, and the
# archive it comes as. Node publishes no FreeBSD build, so on FreeBSD it runs
# on the FreeBSD package the reviewer pins, bundled.
NODE_BUILDS = {
    "linux-x86_64": ("linux-x64", "tar.gz"),
    "linux-aarch64": ("linux-arm64", "tar.gz"),
    "darwin-x86_64": ("darwin-x64", "tar.gz"),
    "darwin-aarch64": ("darwin-arm64", "tar.gz"),
    "windows-x86_64": ("win-x64", "zip"),
    "windows-aarch64": ("win-arm64", "zip"),
}
# What npm calls each platform's system and machine in a package's `os` and `cpu`.
NPM_OS = {"linux": "linux", "darwin": "darwin", "windows": "win32", "freebsd": "freebsd"}
NPM_CPU = {"x86_64": "x64", "aarch64": "arm64"}
# The C library npm finds on a Linux runner.
NPM_LIBC = "glibc"
# Where every package an npm target installs is taken from, at every hop.
NPM_REGISTRY = "https://registry.npmjs.org/"
NPM_HOST = "registry.npmjs.org"
# An `npx` package pinned as `name@version`: a lowercase npm name, scoped or
# not, and one exact semantic version, so no range, tag or `npm:` alias passes.
NPX = re.compile(
    r"((?:@[a-z0-9][a-z0-9._~-]{0,212}/)?[a-z0-9][a-z0-9._~-]{0,213})"
    r"@((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)"
)
# The printable characters a component of an npm location may not hold, as
# the client refuses them in a name it unpacks.
NPM_REFUSED = '\\:<>"|?*'
# The install-time scripts `--ignore-scripts` skips, in the order npm runs them.
INSTALL_EVENTS = ("preinstall", "install", "postinstall")
# What a reviewer may decide a skipped script was: nothing the agent needs,
# done instead by crucible, or lost with nothing in its place. `built` and
# `local` are review's to class, from what the job left, never a reviewer's.
DISPOSITIONS = ("inert", "replaced", "lost")
# What the intake classes a skipped script the reviewer did not decide: one
# the job built, whose output installs in its place, or one the client runs
# on the user's machine, the job having seen it leave the same files twice.
CLASSED = ("built", "local")
# The words that make a script a build by rule, whatever its runs leave: a
# compiler, a native-addon builder, a build tool, or a package manager, which
# can reach a build through another script, so no subcommand of one is taken
# as safe. Each word of the command as a shell splits it is tested by its
# last path segment, case folded, with an `@version`, then one `.exe`,
# `.cmd`, `.bat`, `.js`, `.cjs` or `.mjs`, then one `-<digits>` version,
# dotted or not, taken off, so `npx node-gyp@10`, `gcc-12`, `make.bat`
# and `node .../node-gyp.js` are caught as surely as `node-gyp`; a name
# opening with one of `BUILD_PREFIXES`, or ending in one of
# `BUILD_SUFFIXES` as a cross compiler's target triplet does, is one too. A command that names one
# for another reason is built too: erring toward a build is the safe
# direction, since a build is held to the licence gate and a local script
# runs on the user's machine.
BUILD_TOOLS = frozenset({
    "cc", "gcc", "g++", "c++", "clang", "clang++", "cl", "node-pre-gyp", "cmake-js", "make", "gmake",
    "nmake", "cmake", "ninja", "cargo", "rustc", "go", "npm", "npx", "yarn", "pnpm", "pnpx", "bun", "bunx",
})
BUILD_PREFIXES = ("node-gyp", "prebuild")
BUILD_SUFFIXES = ("-gcc", "-g++", "-cc", "-c++", "-clang", "-clang++", "-ld")
# What a shell or Windows expands into words the classifier cannot see, so a
# command holding any of them is a build, quoted or not: a variable,
# substitution, glob, home or brace expansion, or a `%variable%`.
EXPANDS = frozenset("$`*?[]~{}%")
# What splits a command into words, as a shell splits it: blanks, its
# operators, a backquote and the `(` of `$(`, and `=`, so an assignment's
# value is a word of its own.
COMMAND_BREAKS = frozenset(" \t;&|()<>`={}!")
# The platforms where every skipped script is built and none runs locally.
BUILT_ON = ("windows-", "freebsd-")
# How many rules a client unpacks a lock's tree by: the first.
UNPACK_RULES = 1

PLATFORM = re.compile(r"(linux|darwin|windows|freebsd)-(x86_64|aarch64)")
REGISTRY_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
AGENT_KEY = re.compile(r"[a-z0-9-]{1,32}")
LABEL = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?")
SHA256 = re.compile(r"[0-9a-f]{64}")
# A `uvx` package pinned as `name==version`: a PyPI project name, and a
# version of the characters a version is spelt with, so no extra, range,
# marker or address passes.
PACKAGE = re.compile(r"([A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?)==(.*)", re.DOTALL)
PINNED = re.compile(r"[A-Za-z0-9][A-Za-z0-9.!+_-]*")
# An interpreter pin's version, `3.M`.
PYTHON = re.compile(r"3\.(0|[1-9][0-9]{0,2})")
# Where every package a `uvx` target installs is taken from.
PYPI = "https://files.pythonhosted.org/packages/"
# A wheel's file name: `{name}-{version}(-{build})?-{python}-{abi}-{platform}.whl`.
WHEEL_NAME = re.compile(
    r"(?P<name>[A-Za-z0-9](?:[A-Za-z0-9._]*[A-Za-z0-9])?)-(?P<version>[A-Za-z0-9.!+_]+)"
    r"(?:-(?P<build>[0-9][A-Za-z0-9_.]*))?-(?P<python>[A-Za-z0-9_.]+)-(?P<abi>[A-Za-z0-9_.]+)"
    r"-(?P<platform>[A-Za-z0-9_.]+)\.whl"
)
# The platform tags a wheel may carry on each platform an interpreter is
# pinned for, beside `any`. Each ceiling is the oldest system crucible's own
# release runs on: glibc 2.34 on Linux, the floor the release workflow builds
# against and checks; macOS 11.0 on Apple silicon, the first macOS to run on
# it; and macOS 10.12 on Intel, the oldest Rust's x86_64-apple-darwin target
# supports. musllinux, linux_* and every tag not named here are refused.
PLATFORM_TAGS: dict[str, dict] = {
    "linux-x86_64": {
        "glibc": 34,
        "machine": "x86_64",
        "legacy": {"manylinux1_x86_64", "manylinux2010_x86_64", "manylinux2014_x86_64"},
    },
    "linux-aarch64": {"glibc": 34, "machine": "aarch64", "legacy": {"manylinux2014_aarch64"}},
    "darwin-aarch64": {"macos": (11, 0), "machines": {"arm64", "universal2"}},
    "darwin-x86_64": {"macos": (10, 12), "machines": {"x86_64", "intel", "universal2"}},
    "windows-x86_64": {"exact": {"win_amd64"}},
    "windows-aarch64": {"exact": {"win_arm64"}},
    # A FreeBSD bundle's CPython runs a wheel built for no platform alone.
    "freebsd-x86_64": {"exact": set()},
}


class Refused(Exception):
    """An input the whole run cannot go on from."""


class Dropped(Exception):
    """A target or an entry that is left out, and why."""


class Spent(Dropped):
    """The run's budget is spent, so what is left waits for the next run."""


class Build(Dropped):
    """A `uvx` target whose report installs a source distribution, so it waits
    for that to be built at review; `build` names each one."""

    def __init__(self, build: list[dict]) -> None:
        self.build = build
        names = named([f"{item['package']} {item['version']}" for item in build])
        super().__init__(
            f"{names} {'comes' if len(build) == 1 else 'come'} only as a source distribution here, "
            "so this target waits for a build at review"
        )


class Budget:
    """What one run has read, and since when."""

    def __init__(self) -> None:
        self.read = 0
        self.unpacked = 0
        self.started = time.monotonic()

    def spend(self, count: int) -> None:
        self.read += count
        if self.read > RUN_BYTES or time.monotonic() - self.started > RUN_SECONDS:
            raise Spent(
                f"the run's budget of {RUN_BYTES} bytes and {RUN_SECONDS} seconds is spent; "
                "it is reviewed at the next run"
            )

    def unpack(self, count: int) -> None:
        self.unpacked += count
        if self.unpacked > REPACK_RUN_DECOMPRESSED:
            raise Spent(
                f"the run's budget of {REPACK_RUN_DECOMPRESSED} bytes unpacked from every .tar.bz2 and build archive is spent; "
                "it is reviewed at the next run"
            )


# The budget of the `review` running, which every read of observed data spends.
BUDGET: Budget | None = None


def spend(count: int) -> None:
    if BUDGET is not None:
        BUDGET.spend(count)


def unpack(count: int) -> None:
    """Spend `count` bytes a repack unpacked against the run's budget."""
    if BUDGET is not None:
        BUDGET.unpack(count)


def clipped(text: object) -> str:
    """Hostile `text` as a reason quotes it: whole within `REASON_CHARS`, and
    otherwise its head and its tail, so the end that names a host is kept."""
    text = str(text)
    if len(text) <= REASON_CHARS:
        return text
    half = REASON_CHARS // 2
    return text[:half] + "..." + text[-half:]


def differences(new: object, old: object, at: str = "") -> list[str]:
    """Where `new` and `old` differ, as field paths; a list of targets is
    compared by platform, and a number never equals a flag."""
    if type(new) is not type(old):
        return [at or "everything"]
    if isinstance(new, dict):
        found = []
        for name in sorted(set(new) | set(old)):
            place = f"{at}.{name}" if at else name
            if name not in new or name not in old:
                found.append(place)
            else:
                found += differences(new[name], old[name], place)
        return found
    if isinstance(new, list):
        if all(isinstance(item, dict) and "platform" in item for item in new + old):
            by_new = {str(item["platform"]): item for item in new}
            by_old = {str(item["platform"]): item for item in old}
            found = []
            for platform in sorted(set(by_new) | set(by_old)):
                place = f"{at}[{platform}]"
                if platform not in by_new or platform not in by_old:
                    found.append(place)
                else:
                    found += differences(by_new[platform], by_old[platform], place)
            return found
        found = []
        for index in range(max(len(new), len(old))):
            place = f"{at}[{index}]"
            if index >= len(new) or index >= len(old):
                found.append(place)
            else:
                found += differences(new[index], old[index], place)
        return found
    return [] if new == old else [at or "everything"]


def named(paths: list[str]) -> str:
    shown_paths = [clipped(path) for path in paths[:NAMED_FIELDS]]
    more = f" and {len(paths) - NAMED_FIELDS} more" if len(paths) > NAMED_FIELDS else ""
    return ", ".join(shown_paths) + more


def unrepeated(pairs: list[tuple[str, object]]) -> dict:
    held: dict = {}
    for name, value in pairs:
        if name in held:
            raise ValueError(f"the key {clipped(name)} is given twice")
        held[name] = value
    return held


def non_finite(name: str) -> object:
    raise ValueError(f"{name} is not a number JSON holds")


def parsed(text: bytes, what: str) -> object:
    try:
        return json.loads(text, object_pairs_hook=unrepeated, parse_constant=non_finite)
    except (ValueError, RecursionError) as refused:
        raise Refused(f"{what} is not JSON as read here: {refused}") from None


def read(path: pathlib.Path, ceiling: int, what: str) -> bytes:
    """The regular file at `path`, never a link, refused past `ceiling`."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Refused(f"{what} is missing") from None
    if not stat.S_ISREG(held.st_mode):
        raise Refused(f"{what} is not a regular file")
    with open(path, "rb") as file:
        text = file.read(ceiling + 1)
    spend(len(text))
    if len(text) > ceiling:
        raise Refused(f"{what} is larger than {ceiling} bytes")
    return text


def same_bytes(path: pathlib.Path, expected: bytes, what: str) -> None:
    """Drop unless the regular file at `path`, never a link, is `expected`:
    refused on its size before any of it is read, and otherwise compared a
    chunk at a time, so it is never held beside `expected` whole."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Dropped(f"{what} is missing") from None
    if not stat.S_ISREG(held.st_mode):
        raise Dropped(f"{what} is not a regular file")
    try:
        # Opened without following a link, so a path swapped for one after the
        # check above reads nothing.
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError:
        raise Dropped(f"{what} is not a regular file") from None
    differs = Dropped(f"{what} is not the bytes this review's repack of the download is")
    view = memoryview(expected)
    at = 0
    with os.fdopen(descriptor, "rb") as file:
        held = os.fstat(file.fileno())
        if not stat.S_ISREG(held.st_mode):
            raise Dropped(f"{what} is not a regular file")
        if held.st_size != len(expected):
            raise differs
        while chunk := file.read(1 << 20):
            spend(len(chunk))
            if view[at:at + len(chunk)] != chunk:
                raise differs
            at += len(chunk)
    if at != len(expected):
        raise differs


def read_json(path: pathlib.Path, ceiling: int, what: str) -> object:
    return parsed(read(path, ceiling, what), what)


def obj(value: object, what: str) -> dict:
    if not isinstance(value, dict):
        raise Dropped(f"{what} is not an object")
    return value


def encoded(value: str, what: str) -> bytes:
    """`value` in UTF-8, refused where it holds what UTF-8 cannot, such as a
    lone surrogate JSON can spell."""
    try:
        return value.encode()
    except UnicodeEncodeError:
        raise Dropped(f"{what} holds text that is not Unicode a catalog can carry") from None


def text(value: object, what: str) -> str:
    """Non-empty text of no control character, within its ceiling."""
    if not isinstance(value, str) or not value:
        raise Dropped(f"{what} is not text")
    if len(encoded(value, what)) > TEXT_BYTES or any(ord(c) < 0x20 or ord(c) == 0x7F for c in value):
        raise Dropped(f"{what} is not text a catalog holds")
    return value


def argument(value: object, what: str) -> str:
    """An argument or a variable's value, which may be empty."""
    if not isinstance(value, str) or "\0" in value or len(encoded(value, what)) > TEXT_BYTES:
        raise Dropped(f"{what} is not an argument a catalog holds")
    return value


def arguments(value: object, what: str) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > ITEMS:
        raise Dropped(f"{what} is not a list of at most {ITEMS} arguments")
    return [argument(item, f"{what}[{index}]") for index, item in enumerate(value)]


def number(value: object, what: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise Dropped(f"{what} is not a whole number")
    return value


def digest(value: object, what: str) -> str:
    if not isinstance(value, str) or not SHA256.fullmatch(value):
        raise Dropped(f"{what} is not a SHA-256 in lowercase hexadecimal")
    return value


def inside(path: str) -> bool:
    """Whether `path` names something under the directory it is relative to."""
    return (
        bool(path)
        and not any(c in path for c in "\\:\0")
        and all(part not in ("", ".", "..") for part in path.split("/"))
    )


def relative(value: object, what: str) -> str:
    """A path relative to the version's directory; one leading `./` is dropped."""
    path = text(value, what)
    path = path[2:] if path.startswith("./") else path
    if not inside(path):
        raise Dropped(f"{what} {clipped(path)} is not a relative path that stays inside its directory")
    return path


def version(value: object, what: str) -> str:
    held = text(value, what)
    if len(held.encode()) > VERSION_BYTES or held in (".", "..") or any(c in held for c in "/\\:"):
        raise Dropped(f"{what} {clipped(held)} is not a version that can name one directory")
    return held


def loader(name: str) -> bool:
    """Whether a loader reads `name` to choose what code runs, in any case."""
    name = name.upper()
    return name.startswith(("LD_", "DYLD_")) or name in ("NODE_OPTIONS", "NODE_PATH", "PYTHONPATH", "PATH")


def variables(value: object, what: str) -> dict[str, str]:
    if value is None:
        return {}
    if not isinstance(value, dict) or len(value) > ITEMS:
        raise Dropped(f"{what} is not an object of at most {ITEMS} variables")
    held = {}
    for name, setting in value.items():
        if not name or "=" in name or "\0" in name or len(encoded(name, what)) > TEXT_BYTES:
            raise Dropped(f"{what} names a variable no environment can hold")
        if loader(name):
            raise Dropped(f"{what} sets {clipped(name)}, a variable a loader reads to choose what code runs")
        held[name] = argument(setting, f"{what}.{clipped(name)}")
    return held


def host(name: str) -> bool:
    """Whether `name` is a host's name, of two labels or more, and no address."""
    labels = name.split(".")
    return (
        len(name) <= 253
        and len(labels) >= 2
        and all(LABEL.fullmatch(label) for label in labels)
        and not labels[-1].isdigit()
    )


def reach(value: object, what: str) -> list[str]:
    """Hosts by name, each may open with `*.` over a name of two labels or more."""
    if value is None:
        return []
    if not isinstance(value, list) or len(value) > ITEMS:
        raise Dropped(f"{what} is not a list of at most {ITEMS} hosts")
    held = []
    for item in value:
        pattern = text(item, what)
        named = pattern[2:] if pattern.startswith("*.") else pattern
        if not host(named):
            raise Dropped(f"{what} names {clipped(pattern)}, which is not a host by name or a pattern under one")
        held.append(pattern)
    return held


def address(value: object, what: str) -> tuple[str, str]:
    """An `https` address to a named host, with no user, password or port."""
    url = text(value, what)
    try:
        parts = urllib.parse.urlsplit(url)
    except ValueError:
        raise Dropped(f"{what} {clipped(url)} is not an https address on a host by name") from None
    name = parts.netloc
    if parts.scheme != "https" or not host(name):
        raise Dropped(f"{what} {clipped(url)} is not an https address on a host by name")
    return url, name


def format_of(url: str) -> str:
    path = urllib.parse.urlsplit(url).path.lower()
    if path.endswith((".tar.gz", ".tgz")):
        return "tar.gz"
    if path.endswith(".zip"):
        return "zip"
    if path.endswith(".tar.bz2"):
        return "tar.bz2"
    if path.endswith((".tar.xz", ".tar.zst", ".tar", ".gz", ".bz2", ".xz", ".7z", ".rar")):
        raise Dropped(f"{clipped(url)} is an archive of a kind this intake does not take in")
    return "bare"


def opened(path: pathlib.Path):
    """The download at `path`, opened for reading, refused where it is not a
    regular file."""
    try:
        held = os.lstat(path)
    except OSError:
        raise Dropped("could not be hashed: nothing was downloaded") from None
    if not stat.S_ISREG(held.st_mode):
        raise Dropped("could not be hashed: the download is not a regular file")
    try:
        # Opened without following a link, so a path swapped for one after the
        # check above reads nothing.
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError:
        raise Dropped("could not be hashed: the download is not a regular file") from None
    file = os.fdopen(descriptor, "rb")
    if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
        file.close()
        raise Dropped("could not be hashed: the download is not a regular file")
    return file


def digested(file) -> tuple[str, int]:
    """The SHA-256 and size of the open download `file`, read within its ceiling."""
    sha = hashlib.sha256()
    size = 0
    while chunk := file.read(1 << 20):
        spend(len(chunk))
        size += len(chunk)
        if size > ARTIFACT_BYTES:
            raise Dropped(f"could not be hashed: the download is larger than {ARTIFACT_BYTES} bytes")
        sha.update(chunk)
    return sha.hexdigest(), size


def hashed(path: pathlib.Path) -> tuple[str, int]:
    """The SHA-256 and size of the download at `path`, read within its ceiling."""
    with opened(path) as file:
        return digested(file)


def file_list(value: object) -> tuple[list[dict], int, int]:
    """What `unpack-check` printed: its files, `unpacked` and `decompressed`."""
    printed = obj(value, "the unpack-check output")
    files = printed.get("files")
    if not isinstance(files, list) or len(files) > LISTED_ENTRIES:
        raise Dropped("the unpack-check output lists no files")
    unpacked = number(printed.get("unpacked"), "unpack-check's unpacked")
    decompressed = number(printed.get("decompressed"), "unpack-check's decompressed")
    held: list[dict] = []
    seen: dict[str, dict] = {}
    folded: set[str] = set()
    total = 0
    for index, item in enumerate(files):
        item = obj(item, f"unpack-check file {index}")
        path = text(item.get("path"), f"unpack-check file {index}'s path")
        if not inside(path):
            raise Dropped(f"the unpack-check output lists {clipped(path)}, which is not inside its tree")
        if path.casefold() in folded:
            raise Dropped(f"the unpack-check output lists {clipped(path)} twice")
        folded.add(path.casefold())
        kind = item.get("kind")
        if kind == "file":
            size = number(item.get("size"), f"{clipped(path)}'s size")
            exec_flag = item.get("exec")
            if not isinstance(exec_flag, bool):
                raise Dropped(f"{clipped(path)}'s exec flag is not true or false")
            listed = {
                "exec": exec_flag,
                "kind": "file",
                "path": path,
                "sha256": digest(item.get("sha256"), f"{clipped(path)}'s sha256"),
                "size": size,
            }
            total += size
        elif kind == "dir":
            listed = {"kind": "dir", "path": path}
        elif kind == "link":
            target = text(item.get("target"), f"{clipped(path)}'s link target")
            if target.startswith("/") or any(c in target for c in "\\:"):
                raise Dropped(f"{clipped(path)} links to {clipped(target)}, which is not a relative link")
            listed = {"kind": "link", "path": path, "target": target}
        else:
            raise Dropped(f"{clipped(path)} is not a file, a directory or a link")
        held.append(listed)
        seen[path] = listed
    if total != unpacked:
        raise Dropped(f"the unpack-check output's files sum to {total} bytes, not the {unpacked} it says")
    for listed in held:
        if listed["kind"] == "link":
            resolved(listed["path"], seen)
    return held, unpacked, decompressed


def resolved(link: str, seen: dict[str, dict]) -> str:
    """The listed file `link` reaches, through directories alone and at most
    `LINK_STEPS` links, refused where it escapes its tree or reaches nothing."""
    current = link
    for _ in range(LINK_STEPS):
        target = seen[current]["target"]
        parts: list[str] = current.split("/")[:-1]
        for part in target.split("/"):
            if part in ("", "."):
                continue
            if part == "..":
                if not parts:
                    raise Dropped(f"{clipped(link)} links to {clipped(target)}, which escapes its tree")
                parts.pop()
            else:
                parts.append(part)
        for depth in range(1, len(parts)):
            above = seen.get("/".join(parts[:depth]))
            if above is not None and above["kind"] != "dir":
                raise Dropped(f"{clipped(link)} links through {clipped('/'.join(parts[:depth]))}, which is not a directory")
        current = "/".join(parts)
        reached = seen.get(current)
        if reached is None:
            raise Dropped(f"{clipped(link)} links to {clipped(target)}, which is not in its tree")
        if reached["kind"] == "file":
            return current
        if reached["kind"] == "dir":
            raise Dropped(f"{clipped(link)} links to a directory, not a file")
    raise Dropped(f"{clipped(link)} takes more than {LINK_STEPS} links to reach a file")


def place_of(observed: pathlib.Path, *parts: str) -> pathlib.Path:
    """Where the untrusted job left what it observed of one target, or of one
    download of it, refused where a directory on the way there is a link,
    which could reach anywhere."""
    place = observed
    for part in parts:
        place = place / part
        try:
            held = os.lstat(place)
        except OSError:
            return observed.joinpath(*parts)
        if not stat.S_ISDIR(held.st_mode):
            raise Dropped(f"{clipped(part)} is not a directory of the observed tree")
    return place


def chained(place: pathlib.Path, url: str, begins: str) -> list[str]:
    """The hosts of the chain `fetched.json` under `place` gives, which must
    begin at `url`, `begins` saying whose address that is."""
    try:
        fetched = obj(read_json(place / "fetched.json", SMALL_BYTES, "fetched.json"), "fetched.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    chain = fetched.get("chain")
    if not isinstance(chain, list) or not chain or len(chain) > ITEMS:
        raise Dropped("fetched.json gives no chain of addresses")
    if chain[0] != url:
        raise Dropped(f"fetched.json does not begin at {begins}")
    hosts: list[str] = []
    for hop in chain:
        _, name = address(hop, "the redirect to")
        if name not in hosts:
            hosts.append(name)
    return hosts


def unpacked_at(place: pathlib.Path) -> tuple[list[dict], int, int]:
    """What `unpack-check` printed of the download under `place`."""
    try:
        printed = read_json(place / "unpack-check.json", UNPACK_CHECK_BYTES, "unpack-check.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    return file_list(printed)


def runnable(files: list[dict], cmd: str, platform: str) -> None:
    """Refuse `cmd` where `files` does not list it as a file it may run."""
    program = next((file for file in files if file["path"] == cmd), None)
    if program is None or program["kind"] != "file":
        raise Dropped(f"cmd {clipped(cmd)} is not a file the archive unpacks to")
    if not program["exec"] and not platform.startswith("windows-"):
        raise Dropped(f"cmd {clipped(cmd)} is not runnable")


def artifact_of(
    url: str,
    format: str,
    sha256: str,
    size: int,
    listing: tuple[list[dict], int, int],
    assets: dict[str, bytes],
) -> dict:
    """One artifact as the catalog pins it, its file list kept as a file-list
    asset where it is longer than a catalog lists inline."""
    files, unpacked, decompressed = listing
    artifact: dict = {
        "decompressed": decompressed,
        "format": format,
        "sha256": sha256,
        "size": size,
        "unpacked": unpacked,
        "url": url,
    }
    if len(files) > LISTED_FILES:
        held = compact(files)
        if len(held) > ASSET_BYTES:
            raise Dropped(f"its file list is larger than a catalog asset may be, {ASSET_BYTES} bytes")
        named = hashlib.sha256(held).hexdigest()
        assets[named] = held
        artifact["files"] = {"sha256": named, "size": len(held)}
    else:
        artifact["files"] = files
    return artifact


def compact(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    spec: object,
    runners: dict,
    mirrored: str | None,
    gate: Gate,
    assets: dict[str, bytes],
) -> dict:
    """One registry target as the catalog will list it, before its probe; an
    artifact marked for mirroring carries the licence gate's record. A
    `.tar.bz2`, which the client does not unpack, is repacked here from the
    download, and installs as that repack where the job's is the same bytes
    and the gate passes over it."""
    if not PLATFORM.fullmatch(platform):
        raise Dropped("is not a platform the catalog names")
    spec = obj(spec, "the target")
    url, _ = address(spec.get("archive"), "the archive")
    cmd = relative(spec.get("cmd"), "cmd")
    args = arguments(spec.get("args"), "args")
    env = variables(spec.get("env"), "env")
    pinned = spec.get("sha256")
    if pinned is not None:
        pinned = digest(pinned.lower() if isinstance(pinned, str) else pinned, "the registry's sha256")
    format = format_of(url)
    if format == "tar.bz2" and mirrored is None:
        raise Dropped("is a .tar.bz2 under a licence not on deny.toml's list, so it cannot be repacked")
    if platform not in runners:
        raise Dropped("no runner in the matrix probes this platform")
    place = place_of(observed, agent, platform)
    made = b""
    with opened(place / "artifact") as download:
        sha256, size = digested(download)
        if pinned is not None and pinned != sha256:
            raise Dropped("the download is not the bytes the registry pinned")
        if format == "tar.bz2":
            # Repacked from the very bytes just hashed, never from the job's.
            download.seek(0)
            made = repacked(download)
    hosts = chained(place, url, "the registry's address")
    if format == "tar.bz2":
        same_bytes(place / "repacked", made, "the job's repack")
    listing = unpacked_at(place)
    runnable(listing[0], cmd, platform)
    if format == "tar.bz2":
        artifact = artifact_of(url, "tar.gz", sha256, size, listing, assets)
        artifact["mirror_licence"] = mirrored
        source = None if gate.sboms is None else gate.sboms / f"{sha256}.json"
        gate.replacing(artifact, source, made, assets)
    else:
        artifact = artifact_of(url, format, sha256, size, listing, assets)
    if mirrored is not None and "licence_gate" not in artifact:
        artifact["mirror_licence"] = mirrored
        artifact["licence_gate"] = gate.of(sha256, place / "artifact", assets)
    return {
        "args": args,
        "artifacts": [artifact],
        "cmd": cmd,
        "env": env,
        "hosts": hosts,
        "platform": platform,
    }


def freebsd_pin(pin: object, where: str, fields: set[str]) -> dict:
    """A FreeBSD runtime's pin, holding `fields` and no other: the `https`
    address on `PKG_HOST` of a `.pkg` under its ABI's directory, plain, its
    SHA-256 and the ABI, `FreeBSD:<major>:amd64`."""
    if not isinstance(pin, dict):
        raise Refused(f"{where} is not an object")
    extra = sorted(set(pin) - fields)
    if extra:
        raise Refused(f"{where} holds {named(extra)}, which no pin holds")
    abi = pin.get("abi")
    if not isinstance(abi, str) or not FREEBSD_ABI.fullmatch(abi):
        raise Refused(f"{where}: its abi {clipped(abi)} is not a FreeBSD package ABI on amd64, FreeBSD:<major>:amd64")
    url = pin.get("url")
    try:
        _, name = address(url, where)
        path = plain(url)
    except Dropped as failed:
        raise Refused(str(failed)) from None
    if name != PKG_HOST or not path.startswith(f"/{abi}/") or not path.endswith(".pkg"):
        raise Refused(f"{where} names {clipped(url)}, which is not an https address of a .pkg under /{abi}/ on {PKG_HOST}")
    sha256 = pin.get("sha256")
    if not isinstance(sha256, str) or not SHA256.fullmatch(sha256):
        raise Refused(f"{where}: its sha256 is not a SHA-256 in lowercase hexadecimal")
    return {"abi": abi, "sha256": sha256, "url": url}


def interpreters_of(path: pathlib.Path | None) -> dict[str, dict]:
    """The reviewer's interpreter pins, by platform: each a python-build-standalone
    `install_only` build's address, its SHA-256 and the `3.M` it runs, and on
    `freebsd-x86_64` a FreeBSD package's, as `freebsd_pin` holds it, with its
    ABI and the `3.M` it runs. With no pins, no platform has an interpreter."""
    if path is None:
        return {}
    value = read_json(path, SMALL_BYTES, "the interpreter pins")
    if not isinstance(value, dict):
        raise Refused("the interpreter pins are not an object by platform")
    held = {}
    for platform, pin in value.items():
        if platform not in PLATFORM_TAGS:
            raise Refused(
                f"the interpreter pins name {clipped(platform)}, which is not a platform "
                "a wheel's tags are read for"
            )
        where = f"the interpreter pin for {platform}"
        if platform == "freebsd-x86_64":
            held[platform] = freebsd_pin(pin, where, {"url", "sha256", "python", "abi"})
            python = pin.get("python")
            if not isinstance(python, str) or not PYTHON.fullmatch(python):
                raise Refused(f"{where}: its python {clipped(python)} is not a version 3.M")
            held[platform].update({"minor": int(python[2:]), "python": python})
            continue
        if not isinstance(pin, dict):
            raise Refused(f"{where} is not an object")
        extra = sorted(set(pin) - {"url", "sha256", "python"})
        if extra:
            raise Refused(f"{where} holds {named(extra)}, which no pin holds")
        url = pin.get("url")
        try:
            address(url, where)
        except Dropped:
            url = None
        if url is None or not urllib.parse.urlsplit(url).path.endswith("-install_only.tar.gz"):
            raise Refused(
                f"{where} names {clipped(pin.get('url'))}, which is not an https address of a "
                "python-build-standalone install_only build"
            )
        sha256 = pin.get("sha256")
        if not isinstance(sha256, str) or not SHA256.fullmatch(sha256):
            raise Refused(f"{where}: its sha256 is not a SHA-256 in lowercase hexadecimal")
        python = pin.get("python")
        if not isinstance(python, str) or not PYTHON.fullmatch(python):
            raise Refused(f"{where}: its python {clipped(python)} is not a version 3.M")
        held[platform] = {"minor": int(python[2:]), "python": python, "sha256": sha256, "url": url}
    return held


def normal(name: str) -> str:
    """A package's name as an index compares it."""
    return re.sub(r"[-_.]+", "-", name).lower()


def python_fits(python: str, abi: str, minor: int) -> bool:
    """Whether the interpreter tag `python` with the ABI tag `abi` runs on a
    CPython of 3.`minor` built without debug or free threading."""
    cpython = re.fullmatch(r"cp3(0|[1-9][0-9]*)", python)
    if cpython is not None:
        built = int(cpython[1])
        if built == minor:
            return abi in (f"cp3{minor}", "abi3", "none")
        return 2 <= built < minor and abi == "abi3"
    generic = re.fullmatch(r"py3(0|[1-9][0-9]*)?", python)
    return generic is not None and abi == "none" and (generic[1] is None or int(generic[1]) <= minor)


def platform_fits(tag: str, platform: str) -> bool:
    """Whether the platform tag `tag` runs on `platform`, as `PLATFORM_TAGS` holds it."""
    if tag == "any":
        return True
    accepted = PLATFORM_TAGS.get(platform)
    if accepted is None:
        return False
    if "glibc" in accepted:
        held = re.fullmatch(r"manylinux_2_(0|[1-9][0-9]*)_([a-z0-9_]+)", tag)
        if held is not None:
            return held[2] == accepted["machine"] and int(held[1]) <= accepted["glibc"]
        return tag in accepted["legacy"]
    if "macos" in accepted:
        held = re.fullmatch(r"macosx_(0|[1-9][0-9]*)_(0|[1-9][0-9]*)_([a-z0-9_]+)", tag)
        return (
            held is not None
            and held[3] in accepted["machines"]
            and (int(held[1]), int(held[2])) <= accepted["macos"]
        )
    return tag in accepted["exact"]


def fits(tags: str, platform: str, minor: int) -> bool:
    """Whether a wheel tagged `tags` (`{python}-{abi}-{platform}`, each part
    expanded on its dots) runs on an interpreter of 3.`minor` on `platform`."""
    parts = tags.split("-")
    if len(parts) != 3:
        return False
    pythons, abis, platforms = (part.split(".") for part in parts)
    return any(python_fits(python, abi, minor) for python in pythons for abi in abis) and any(
        platform_fits(tag, platform) for tag in platforms
    )


def dotted(value: str) -> bool:
    """Whether `value` is a dotted Python name."""
    return all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", part, re.ASCII) for part in value.split("."))


class Spending:
    """An open download that spends the run's budget on every byte read from
    it, so a wheel's members are read within the budget."""

    def __init__(self, file) -> None:
        self.file = file

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(count)
        spend(len(chunk))
        return chunk

    def seek(self, offset: int, whence: int = 0) -> int:
        return self.file.seek(offset, whence)

    def tell(self) -> int:
        return self.file.tell()

    def seekable(self) -> bool:
        return True


def data_part(name: str) -> None:
    """Refuse the wheel member `name` where the client would not install it:
    of a `.data` directory (its top directory's last dotted part being `data`
    in any case) only `purelib` and `platlib` are installed."""
    member = name[2:] if name.startswith("./") else name
    directory = member.endswith("/")
    member = member[:-1] if directory else member
    top, slash, rest = member.partition("/")
    _, dot, suffix = top.rpartition(".")
    if not dot or not suffix.isascii() or suffix.lower() != "data":
        return
    if not slash:
        if directory:
            return
        refused = top
    else:
        part, slash, path = rest.partition("/")
        if part in ("purelib", "platlib") and (path or directory):
            return
        refused = part
    raise Dropped(
        f"holds {clipped(member)} under {clipped(refused)}, which the client refuses: "
        "of a wheel's .data directory only purelib and platlib are installed"
    )


def entry_point_of(archive: zipfile.ZipFile, names: list[str], package: str) -> str:
    """The `module:attr` the agent's own wheel names as its console script `package`."""
    tops = {name.split("/", 1)[0] for name in names if name.split("/", 1)[0].endswith(".dist-info")}
    if len(tops) != 1:
        raise Dropped("holds no single .dist-info directory")
    try:
        info = archive.getinfo(f"{tops.pop()}/entry_points.txt")
    except KeyError:
        raise Dropped("has no entry_points.txt") from None
    with archive.open(info) as file:
        content = file.read(ENTRY_POINTS_BYTES + 1)
    if len(content) > ENTRY_POINTS_BYTES:
        raise Dropped(f"its entry_points.txt is larger than {ENTRY_POINTS_BYTES} bytes")
    # A default section no header can name, as a header holds no line break,
    # so a `[DEFAULT]` section is a section and lends console_scripts nothing.
    parser = configparser.ConfigParser(interpolation=None, strict=True, delimiters=("=",), default_section="\n")
    parser.optionxform = str  # a script's name is read as written
    try:
        parser.read_string(content.decode())
    except (configparser.Error, UnicodeDecodeError):
        raise Dropped("its entry_points.txt cannot be read: it is not UTF-8, or repeats a section or a name") from None
    if not parser.has_option("console_scripts", package):
        raise Dropped(f"names no console script {clipped(package)}")
    value = parser.get("console_scripts", package)
    module, colon, attr = value.partition(":")
    if not colon or not dotted(module) or not dotted(attr):
        raise Dropped(f"its console script {clipped(package)} is {clipped(value)}, not module:attr")
    return value


def central_directory(file) -> None:
    """Refuse the open wheel `file` where its central directory, as the record
    that ends it states, holds more than `LISTED_ENTRIES` members or more than
    `CENTRAL_DIRECTORY_BYTES`, read before `zipfile` holds that directory.

    A wheel is held to the layout a wheel has, so that the record read here is
    the one `zipfile` obeys: the end record is the file's last 22 bytes, with no
    comment after it; and where the 20 bytes before it are a zip64 locator, or
    any of its fields stands at its all-ones value and so defers to zip64, the
    zip64 record is the 56 bytes just before that locator, where `zipfile`
    reads it, and the locator must point at that very place."""
    end_bytes, locator_bytes, record_bytes = 22, 20, 56
    size = file.seek(0, os.SEEK_END)
    if size < end_bytes:
        raise Dropped("is not a wheel this intake can read")
    file.seek(size - end_bytes)
    end = file.read(end_bytes)
    if end[:4] != b"PK\x05\x06" or end[20:22] != b"\x00\x00":
        raise Dropped(
            "does not end in the record that closes a central directory, with no zip comment after it, "
            "as a wheel ends"
        )
    on_disk, entries, directory, offset = struct.unpack("<HHII", end[8:20])
    deferred = 0xFFFF in (on_disk, entries) or 0xFFFFFFFF in (directory, offset)
    locator_at = size - end_bytes - locator_bytes
    locator = b""
    if locator_at >= 0:
        file.seek(locator_at)
        locator = file.read(locator_bytes)
    if locator[:4] == b"PK\x06\x07" or deferred:
        record_at = locator_at - record_bytes
        if locator[:4] != b"PK\x06\x07" or record_at < 0:
            raise Dropped("its central directory defers to a zip64 record it does not hold")
        (stated,) = struct.unpack("<Q", locator[8:16])
        file.seek(record_at)
        record = file.read(record_bytes)
        if stated != record_at or record[:4] != b"PK\x06\x06":
            raise Dropped(
                "its central directory's zip64 locator does not point at the zip64 record just before it, "
                "the one zipfile reads"
            )
        entries, directory = struct.unpack("<QQ", record[32:48])
    if entries > LISTED_ENTRIES:
        raise Dropped(f"its central directory lists more than {LISTED_ENTRIES} members")
    if directory > CENTRAL_DIRECTORY_BYTES:
        raise Dropped(f"its central directory is larger than {CENTRAL_DIRECTORY_BYTES} bytes")
    file.seek(0)


def wheel_read(file, package: str | None) -> str | None:
    """Refuse the open wheel `file` where a member is one the client would not
    install, or two members share a name, and give its console script
    `package` where one is asked for."""
    try:
        spending = Spending(file)
        central_directory(spending)
        with zipfile.ZipFile(spending) as archive:
            infos = archive.infolist()
            if len(infos) > LISTED_ENTRIES:
                raise Dropped(f"holds more than {LISTED_ENTRIES} members")
            names = [info.filename for info in infos]
            # Names compared as the client compares them: a leading `./`
            # removed, and case folded.
            seen: set[str] = set()
            for name in names:
                compared = name.removeprefix("./").casefold()
                if compared in seen:
                    raise Dropped(f"lists {clipped(name)} twice, as the client compares names")
                seen.add(compared)
                data_part(name)
            return entry_point_of(archive, names, package) if package is not None else None
    except Dropped:
        raise
    except Exception:  # whatever zipfile cannot read is a wheel this intake cannot read
        raise Dropped("is not a wheel this intake can read") from None


def under(files: list[dict], top: str) -> None:
    """Refuse a listing with a path outside the directory `top`."""
    for file in files:
        path = file["path"]
        if path != top and not path.startswith(top + "/"):
            raise Dropped(f"lists {clipped(path)}, which is not under {top}/")


def staged(prefix: str, work):
    """What `work` gives, a drop it raises naming `prefix` first."""
    try:
        return work()
    except Spent:
        raise
    except Dropped as failed:
        raise Dropped(f"{prefix}: {failed}") from None


def report_of(place: pathlib.Path) -> list[tuple[str, str, str]]:
    """The address, name and version of each package pip's report under
    `place` installs, in its order; nothing else of the report is read."""
    try:
        report = obj(read_json(place / "resolved.json", REPORT_BYTES, "resolved.json"), "pip's report")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if report.get("version") != "1":
        raise Dropped(f"pip's report is in version {clipped(report.get('version'))}, not 1, the one read here")
    install = report.get("install")
    if not isinstance(install, list) or not install or len(install) > WHEELS:
        raise Dropped(f"pip's report installs nothing, or more than {WHEELS} packages")
    items = []
    for index, item in enumerate(install):
        item = obj(item, f"the report's package {index}")
        url = text(obj(item.get("download_info"), f"the report's package {index}'s download").get("url"),
                   f"the report's package {index}'s address")
        metadata = obj(item.get("metadata"), f"the report's package {index}'s metadata")
        items.append(
            (
                url,
                text(metadata.get("name"), f"the report's package {index}'s name"),
                text(metadata.get("version"), f"the report's package {index}'s version"),
            )
        )
    return items


def plain(url: str) -> str:
    """The path of `url`, refused where it is not a plain address: a %, query,
    fragment, parameter or backslash would let it be read as other than it is
    written, and so would an empty, `.` or `..` segment."""
    odd = next((mark for mark in "%?#;\\" if mark in url), None)
    if odd is not None:
        raise Dropped(
            f"{clipped(url)} is not a plain address: it holds a {odd}, and a %, query, fragment, "
            "parameter or backslash would let it be read as other than it is written"
        )
    path = urllib.parse.urlsplit(url).path
    if any(segment in ("", ".", "..") for segment in path[1:].split("/")):
        raise Dropped(f"{clipped(url)} holds an empty, . or .. segment in its path")
    return path


def runtime_bundle(
    place: pathlib.Path, pin: dict, cmd: str, platform: str, gate: Gate, assets: dict[str, bytes]
) -> tuple[dict, list[str], list[dict]]:
    """A FreeBSD runtime as the job left it under `place`: the package `pin`
    names as `artifact`, fetched from its address, and the bundle made of it
    as `repacked`, listed by `unpack-check.json`, with its SBOM as
    `sbom.json`. The artifact is the package's address, hash and size,
    installed as the bundle where the licence gate passes over it. Returns
    the artifact, the hosts it came through and its listing."""
    sha256, size = hashed(place / "artifact")
    if sha256 != pin["sha256"]:
        raise Dropped("the download is not the bytes the pin names")
    hosts = chained(place, pin["url"], "the pinned address")
    try:
        bundle = read(place / "repacked", REPACK_BYTES, "the bundle")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    listing = unpacked_at(place)
    for listed in listing[0]:
        if listed["path"].split("/")[0] not in ("bin", "lib", "licenses"):
            raise Dropped(f"lists {clipped(listed['path'])}, which is not under bin/, lib/ or licenses/")
    runnable(listing[0], cmd, platform)
    artifact = artifact_of(pin["url"], "tar.gz", sha256, size, listing, assets)
    gate.replacing(artifact, place / "sbom.json", bundle, assets)
    return artifact, hosts, listing[0]


def wheel_fits(file: str, name: str, item_version: str, platform: str, pin: dict) -> None:
    """Refuse the wheel named `file` where its name does not agree with the
    report's `name` and `item_version`, or its tags are none the platform's
    pinned interpreter accepts."""
    held = WHEEL_NAME.fullmatch(file)
    if held is None:
        raise Dropped(f"{shown(clipped(file))} is not a wheel's file name")
    if normal(held["name"]) != normal(name) or held["version"] != item_version:
        raise Dropped(
            f"wheel {shown(clipped(file))} does not agree with the report, which says it is "
            f"{shown(clipped(name))} {shown(clipped(item_version))}"
        )
    if not fits(f"{held['python']}-{held['abi']}-{held['platform']}", platform, pin["minor"]):
        raise Dropped(
            f"wheel {shown(clipped(file))} is built for none of the tags an interpreter of "
            f"Python {pin['python']} on {platform} accepts"
        )


def sdist_built(
    inner: pathlib.Path,
    url: str,
    sha256: str,
    size: int,
    name: str,
    item_version: str,
    package: str | None,
    platform: str,
    pin: dict,
    gate: Gate,
    assets: dict[str, bytes],
) -> tuple[dict, list[dict], str | None]:
    """The source distribution downloaded under `inner`, hashed as `sha256`
    and `size`, as it installs: the wheel the job built from it, under
    `built/`, held to every check a downloaded wheel is and to the licence
    gate, from the SBOM named by the wheel's SHA-256. The artifact keeps the
    source distribution's address, hash and size, and the wheel replaces it."""
    built = place_of(inner, "built")
    try:
        file = obj(read_json(built / "name.json", SMALL_BYTES, "built/name.json"), "built/name.json").get("file")
        wheel = read(built / "artifact", REPACK_BYTES, "built/artifact")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    file = text(file, "the built wheel's file name")
    wheel_fits(file, name, item_version, platform, pin)
    named_script = wheel_read(io.BytesIO(wheel), package)
    listing = unpacked_at(built)
    under(listing[0], "site")
    artifact = artifact_of(url, "wheel", sha256, size, listing, assets)
    built_sha = hashlib.sha256(wheel).hexdigest()
    gate.replacing(artifact, None if gate.sboms is None else gate.sboms / f"{built_sha}.json", wheel, assets)
    return artifact, listing[0], named_script


def uvx_target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    package: str,
    released: str,
    interpreters: dict[str, dict],
    gate: Gate,
    assets: dict[str, bytes],
) -> dict:
    """One platform of a `uvx` agent as the catalog will list it, before its
    probe: the interpreter pinned for it and every wheel pip's report installs
    on it, each the bytes that were downloaded and checked. On FreeBSD the
    interpreter is the pinned package, bundled, as `runtime_bundle` takes it."""
    pin = interpreters.get(platform)
    if pin is None:
        raise Dropped("no interpreter is pinned for this platform")
    place = place_of(observed, agent, platform)
    wheels = []
    build = []
    own = None
    for index, (url, name, item_version) in enumerate(report_of(place)):
        if not url.startswith(PYPI):
            raise Dropped(f"{clipped(url)} is not on {PYPI}, the one place a wheel is taken from")
        file = plain(url).rsplit("/", 1)[-1]
        mine = normal(name) == normal(package) and item_version == released
        own = own or mine
        if not file.endswith(".whl"):
            # A source distribution the job built a wheel from is taken as
            # that wheel; one it did not waits for a build. At most `WHEELS`
            # rows, as the report lists no more, each field clipped as a
            # reason quotes it.
            if os.path.lexists(place_of(observed, agent, platform, "wheels", str(index), "built")):
                wheels.append((index, url, file, mine, name, item_version))
            else:
                build.append({"package": clipped(name), "url": clipped(url), "version": clipped(item_version)})
            continue
        wheel_fits(file, name, item_version, platform, pin)
        wheels.append((index, url, file, mine, None, None))
    if not own:
        raise Dropped(f"the report installs no {clipped(package)} {clipped(released)}")
    if build:
        raise Build(build)
    windows = platform.startswith("windows-")
    freebsd = platform == "freebsd-x86_64"
    cmd = "python/python.exe" if windows else f"python/bin/python{pin['python']}"
    if freebsd:
        cmd = f"bin/python{pin['python']}"

    def python() -> tuple[dict, list[str], list[dict]]:
        inner = place_of(observed, agent, platform, "interpreter")
        if freebsd:
            return runtime_bundle(inner, pin, cmd, platform, gate, assets)
        sha256, size = hashed(inner / "artifact")
        if sha256 != pin["sha256"]:
            raise Dropped("the download is not the bytes the interpreter pin names")
        hosts = chained(inner, pin["url"], "the pinned address")
        listing = unpacked_at(inner)
        under(listing[0], "python")
        runnable(listing[0], cmd, platform)
        return artifact_of(pin["url"], "tar.gz", sha256, size, listing, assets), hosts, listing[0]

    artifact, hosts, files = staged("the interpreter", python)
    artifacts = [artifact]
    listings = [files]
    entry_point = None
    for index, url, file, mine, name, item_version in wheels:

        def one() -> tuple[dict, list[str], list[dict], str | None]:
            inner = place_of(observed, agent, platform, "wheels", str(index))
            with opened(inner / "artifact") as download:
                sha256, size = digested(download)
                download.seek(0)
                # A source distribution's own download is hashed alone; the
                # wheel built from it is what is read as a wheel.
                named_script = None if name is not None else wheel_read(download, package if mine else None)
            reached = chained(inner, url, "the report's address")
            elsewhere = next((name for name in reached if name != PYPI_HOST), None)
            if elsewhere is not None:
                raise Dropped(f"fetched.json reaches {clipped(elsewhere)}, but a wheel is fetched from {PYPI_HOST} alone")
            if name is not None:
                artifact, files, named_script = sdist_built(
                    inner, url, sha256, size, name, item_version, package if mine else None, platform, pin, gate, assets
                )
                return artifact, reached, files, named_script
            listing = unpacked_at(inner)
            under(listing[0], "site")
            return artifact_of(url, "wheel", sha256, size, listing, assets), reached, listing[0], named_script

        kind = "wheel" if name is None else "source distribution"
        artifact, reached, files, named_script = staged(f"{kind} {shown(clipped(file))}", one)
        artifacts.append(artifact)
        listings.append(files)
        hosts += [name for name in reached if name not in hosts]
        entry_point = entry_point or named_script
    folded: set[str] = set()
    for files in listings:
        for listed in files:
            if listed["path"].casefold() in folded:
                raise Dropped(f"{clipped(listed['path'])} is listed by two of its artifacts, which the door would refuse")
        folded.update(listed["path"].casefold() for listed in files)
    return {
        "artifacts": artifacts,
        "cmd": cmd,
        "entry_point": entry_point,
        "hosts": hosts,
        "interpreter_args": ["-I"],
        "platform": platform,
    }


def method(value: object, index: int) -> dict:
    """One sign-in method the probe recorded, with its route if it is a
    terminal one: piped unless the agent said it needs a real terminal."""
    value = obj(value, f"sign-in method {index}")
    kind = value.get("kind")
    if kind not in ("agent", "env_var", "terminal", "other"):
        raise Dropped(f"sign-in method {index} is of no kind the probe records")
    held = {
        "id": text(value.get("id"), f"sign-in method {index}'s id"),
        "kind": kind,
        "name": text(value.get("name"), f"sign-in method {index}'s name"),
    }
    if kind == "terminal":
        tty = value.get("tty")
        if not (isinstance(tty, bool) or (isinstance(tty, str) and tty == "unknown")):
            raise Dropped(
                f"sign-in method {index}'s tty is neither true, false nor unknown, "
                "so whether it needs a terminal is not known"
            )
        held.update(
            {
                "args": arguments(value.get("args"), f"sign-in method {index}'s args"),
                "command": relative(value.get("command"), f"sign-in method {index}'s command"),
                "env": variables(value.get("env"), f"sign-in method {index}'s env"),
                "route": "terminal" if tty is True else "piped",
                "tty": tty,
            }
        )
    return held


def node_of(path: pathlib.Path | None) -> tuple[str | None, dict | None]:
    """The reviewer's Node pin, `{"version": "X.Y.Z"}`, held to `NODE_FLOOR`,
    and its FreeBSD package where it pins one under `freebsd-x86_64`, as
    `freebsd_pin` holds it, with the package's `version`, in the pin's major
    line and at or above `NODE_FLOOR`. With no pin, no npm target has a Node
    to run on, and with no FreeBSD package, none on FreeBSD."""
    if path is None:
        return None, None
    value = read_json(path, SMALL_BYTES, "the Node pin")
    if not isinstance(value, dict):
        raise Refused("the Node pin is not an object")
    extra = sorted(set(value) - {"version", "freebsd-x86_64"})
    if extra:
        raise Refused(f"the Node pin holds {named(extra)}, which no pin holds")
    pinned = value.get("version")
    held = NODE_VERSION.fullmatch(pinned) if isinstance(pinned, str) else None
    if held is None:
        raise Refused(f"the Node pin's version {clipped(pinned)} is not a version X.Y.Z")
    parts = tuple(int(part) for part in held.groups())
    if parts[0] % 2 or parts < NODE_FLOOR:
        raise Refused(
            f"Node {pinned} is not a long-term-support release with module.registerHooks: "
            "the pin is an even major line from 22, and 22.15.0 or later on line 22"
        )
    if "freebsd-x86_64" not in value:
        return pinned, None
    where = "the Node pin for freebsd-x86_64"
    freebsd = freebsd_pin(value["freebsd-x86_64"], where, {"url", "sha256", "abi", "version"})
    packaged = value["freebsd-x86_64"].get("version")
    held = NODE_VERSION.fullmatch(packaged) if isinstance(packaged, str) else None
    if held is None:
        raise Refused(f"{where}: its version {clipped(packaged)} is not a version X.Y.Z")
    packaged_parts = tuple(int(part) for part in held.groups())
    if packaged_parts[0] != parts[0]:
        raise Refused(f"{where}: Node {packaged} is not in the major line of the pinned Node {pinned}")
    if packaged_parts < NODE_FLOOR:
        raise Refused(f"{where}: Node {packaged} is older than {'.'.join(map(str, NODE_FLOOR))}, the oldest a pin may name")
    freebsd["version"] = packaged
    return pinned, freebsd


def release_keys(path: pathlib.Path) -> tuple[set[str], list[bytes]]:
    """The primary fingerprints a Node release may be signed under, from
    `fingerprints`, and every armoured key beside it."""
    try:
        listing = read(path / "fingerprints", SMALL_BYTES, "the Node release keys' fingerprints").decode()
    except UnicodeDecodeError:
        raise Refused("the Node release keys' fingerprints are not UTF-8") from None
    listed: set[str] = set()
    for line in listing.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if not re.fullmatch(r"[0-9A-F]{40}", line):
            raise Refused(f"the Node release keys' fingerprints list {clipped(line)}, which is not a fingerprint")
        listed.add(line)
    try:
        names = sorted(name for name in os.listdir(path) if name.endswith(".asc"))
    except OSError:
        raise Refused("the Node release keys are not a directory that can be read") from None
    if not listed or not names or len(names) > ITEMS:
        raise Refused(f"the Node release keys list no fingerprint, or hold no key or more than {ITEMS}")
    return listed, [read(path / name, KEY_BYTES, f"the Node release key {clipped(name)}") for name in names]


def gpg_run(command: list[str]) -> bytes:
    """What `command` printed, or the drop that says why it could not run."""
    try:
        ran = subprocess.run(command, capture_output=True, timeout=GPG_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        raise Dropped(f"gpg took longer than {GPG_SECONDS} seconds") from None
    except OSError as failed:
        raise Dropped(f"gpg could not be run: {clipped(failed.strerror)}") from None
    if ran.returncode != 0 and "--verify" not in command:
        raise Dropped(f"gpg refused the vendored Node release keys: {clipped(ran.stderr.decode(errors='replace'))}")
    return ran.stdout


def node_sums(gpg: str, keys: tuple[set[str], list[bytes]], signed: bytes) -> dict[str, str]:
    """The SHA-256 of each file `signed`, a Node release's
    `SHASUMS256.txt.asc`, lists, once its signature is held to be one of the
    vendored release keys': one signature, good, by a key whose primary
    fingerprint is listed, over a SHA-2 digest, made before any expiry of the
    key that made it and of its primary.
    Only the body gpg verified is read, in a keyring of its own."""
    listed, armoured = keys
    with tempfile.TemporaryDirectory(prefix="node-keys-") as held:
        home = pathlib.Path(held)
        command = [
            gpg, "--homedir", str(home), "--batch", "--no-tty", "--no-autostart",
            "--no-default-keyring", "--keyring", str(home / "keys.gpg"),
        ]
        files = []
        for index, key in enumerate(armoured):
            files.append(home / f"key-{index}.asc")
            files[-1].write_bytes(key)
        gpg_run(command + ["--import", *map(str, files)])
        (home / "SHASUMS256.txt.asc").write_bytes(signed)
        status = gpg_run(command + [
            "--trust-model", "always", "--no-auto-key-retrieve", "--status-fd", "1",
            "--output", str(home / "body"), "--verify", str(home / "SHASUMS256.txt.asc"),
        ]).decode(errors="replace")
        keyring = gpg_run(command + ["--with-colons", "--fixed-list-mode", "--list-keys"]).decode(errors="replace")
        lines = [line.split() for line in status.splitlines() if line.startswith("[GNUPG:] ")]
        said = [line[1] for line in lines if len(line) > 1]
        refused = sorted(set(said) & {"BADSIG", "ERRSIG", "EXPSIG", "REVKEYSIG", "NO_PUBKEY", "NODATA"})
        if refused or said.count("NEWSIG") != 1 or not {"GOODSIG", "EXPKEYSIG"} & set(said):
            raise Dropped(
                "SHASUMS256.txt.asc does not carry one good signature by a vendored Node release key"
                + (f": gpg said {named(refused)}" if refused else "")
            )
        valid = next((line for line in lines if line[1] == "VALIDSIG" and len(line) >= 12), None)
        if valid is None or valid[11] not in listed:
            raise Dropped("SHASUMS256.txt.asc is signed by a key whose fingerprint the vendored list does not hold")
        # OpenPGP's SHA-256, SHA-384, SHA-512 and SHA-224; a collision in
        # anything weaker could put other sums under the same signature.
        if valid[9] not in ("8", "9", "10", "11"):
            raise Dropped(f"SHASUMS256.txt.asc is signed over digest {clipped(valid[9])}, not one of the SHA-2 family")
        # The expiry of every key and subkey, by fingerprint, as the colon
        # listing gives it: the expiry field of a `pub` or `sub` line, then
        # its `fpr` line.
        expiries: dict[str, str] = {}
        expiry = ""
        for line in keyring.splitlines():
            fields = line.split(":")
            if fields[0] in ("pub", "sub") and len(fields) > 6:
                expiry = fields[6]
            elif fields[0] == "fpr" and len(fields) > 9:
                expiries[fields[9]] = expiry
        # Before the expiry of the key that made it and of its primary, which
        # gpg's listing may not carry down to a subkey.
        signed_at = valid[4]
        for ends in {expiries.get(valid[2], ""), expiries.get(valid[11], "")}:
            if not signed_at.isdigit() or (ends and (not ends.isdigit() or int(signed_at) >= int(ends))):
                raise Dropped("SHASUMS256.txt.asc was signed after the key that signed it expired")
        body = read(home / "body", SHASUMS_BYTES, "SHASUMS256.txt")
    sums: dict[str, str] = {}
    for line in body.decode(errors="replace").splitlines():
        held_line = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if held_line is None:
            raise Dropped(f"SHASUMS256.txt holds {clipped(line)}, which is not a SHA-256 and a file name")
        if held_line[2] in sums:
            raise Dropped(f"SHASUMS256.txt lists {clipped(held_line[2])} twice")
        sums[held_line[2]] = held_line[1]
    return sums


class Node:
    """The Node release a run pins, its signed checksums, or why there are
    none, and each platform's download of it as an artifact, made once and
    shared by every npm agent."""

    def __init__(
        self,
        pinned: str | None,
        sums: dict[str, str] | None,
        failure: str | None,
        freebsd: dict | None = None,
        gate: Gate | None = None,
    ) -> None:
        self.pinned = pinned
        self.sums = sums
        self.failure = failure
        self.freebsd = freebsd
        self.gate = gate
        self.made: dict[str, tuple] = {}

    def artifact(self, observed: pathlib.Path, platform: str, assets: dict[str, bytes]) -> tuple[dict, list[str], str]:
        """Node's artifact for `platform`, the hosts it came through, and the
        binary a target runs."""
        if self.pinned is None:
            raise Dropped("no Node is pinned, so an npm agent has nothing to run on")
        if self.failure is not None:
            raise Dropped(f"Node {self.pinned} is not verified: {self.failure}")
        if platform not in self.made:
            try:
                self.made[platform] = (self.build(observed, platform), None)
            except Spent:
                raise
            except Dropped as failed:
                self.made[platform] = (None, str(failed))
        made, failure = self.made[platform]
        if failure is not None:
            raise Dropped(failure)
        artifact, hosts, cmd, kept = made
        assets.update(kept)
        return copy_of(artifact), list(hosts), cmd

    def build(self, observed: pathlib.Path, platform: str) -> tuple[dict, list[str], str, dict[str, bytes]]:
        build = NODE_BUILDS.get(platform)
        if platform == "freebsd-x86_64":
            if self.freebsd is None or self.gate is None:
                raise Dropped(f"Node publishes no build for {platform}, and no FreeBSD package is pinned")
            pin, gate = self.freebsd, self.gate

            def bundled() -> tuple[dict, list[str], str, dict[str, bytes]]:
                held: dict[str, bytes] = {}
                artifact, hosts, _ = runtime_bundle(place_of(observed, "node", platform), pin, "bin/node", platform, gate, held)
                return artifact, hosts, "bin/node", held

            return staged(f"Node {pin['version']} for FreeBSD", bundled)
        if build is None:
            raise Dropped(f"Node publishes no build for {platform}")
        name, format = build
        top = f"node-v{self.pinned}-{name}"
        file = f"{top}.{format}"
        url = f"{NODE_DIST}v{self.pinned}/{file}"
        cmd = f"{top}/node.exe" if platform.startswith("windows-") else f"{top}/bin/node"
        kept = [cmd, f"{top}/LICENSE"]
        place = place_of(observed, "node", platform)

        def made() -> tuple[dict, list[str], str, dict[str, bytes]]:
            sha256, size = hashed(place / "artifact")
            assert self.sums is not None
            if self.sums.get(file) != sha256:
                raise Dropped(f"the download is not the bytes SHASUMS256.txt lists for {file}")
            hosts = chained(place, url, "Node's address")
            listing = unpacked_at(place)
            for listed in listing[0]:
                if listed["kind"] == "link" or (listed["kind"] == "file" and listed["path"] not in kept):
                    raise Dropped(f"unpack-check lists {clipped(listed['path'])}, which is not kept")
            runnable(listing[0], cmd, platform)
            held: dict[str, bytes] = {}
            artifact = artifact_of(url, format, sha256, size, listing, held)
            artifact["kept"] = kept
            return artifact, hosts, cmd, held

        return staged(f"Node {self.pinned}", made)


def copy_of(value: object) -> object:
    """A deep copy of JSON data, so no two entries share one value."""
    return json.loads(json.dumps(value))


def node_release(
    observed: pathlib.Path,
    pinned: str | None,
    gpg: str | None,
    keys: tuple[set[str], list[bytes]] | None,
    freebsd: dict | None = None,
    gate: Gate | None = None,
) -> Node:
    """The run's Node: its checksums once the signed list the job left is
    verified and was fetched from Node's own address, or why not."""
    if pinned is None or gpg is None or keys is None:
        return Node(None, None, None)
    try:
        place = place_of(observed, "node")
        chained(place, f"{NODE_DIST}v{pinned}/SHASUMS256.txt.asc", "Node's address for SHASUMS256.txt.asc")
        try:
            signed = read(place / "SHASUMS256.txt.asc", SHASUMS_BYTES, "SHASUMS256.txt.asc")
        except Refused as refused:
            raise Dropped(str(refused)) from None
        return Node(pinned, node_sums(gpg, keys, signed), None, freebsd, gate)
    except Spent:
        raise
    except Dropped as failed:
        return Node(pinned, None, str(failed), freebsd, gate)


def npx_package(value: object) -> tuple[str, str]:
    """An `npx` package as `name@version`: a lowercase npm name, scoped or
    not, at one exact version, so no range, tag or `npm:` alias passes."""
    held = NPX.fullmatch(value) if isinstance(value, str) else None
    if held is None:
        raise Dropped(f"the npx package {clipped(value)} is not pinned as a lowercase name@ an exact version")
    return held[1], held[2]


def npm_part(part: str) -> bool:
    """Whether `part` is one component a location may hold."""
    return (
        bool(part)
        and part not in (".", "..")
        and not part.endswith((".", " "))
        and all("!" <= c <= "~" and c not in NPM_REFUSED for c in part)
    )


def npm_name(name: str) -> bool:
    """Whether `name` is a package's name as a location holds it: one
    component, or a scope beginning `@` and one component under it."""
    parts = name.split("/")
    if len(parts) == 2 and parts[0].startswith("@") and len(parts[0]) > 1:
        return npm_part(parts[0]) and npm_part(parts[1]) and parts[1] != "node_modules"
    return len(parts) == 1 and npm_part(name) and not name.startswith("@") and name != "node_modules"


def npm_location(value: object) -> str:
    """Where npm installs a package: `node_modules/` and a name, then any
    number of `/node_modules/` and another, within `NPM_PATH_BYTES`."""
    if not isinstance(value, str) or len(value.encode(errors="replace")) > NPM_PATH_BYTES:
        raise Dropped(f"the lock installs a package at {clipped(value)}, which is not where npm installs one")
    rest = value
    while True:
        head, slash, rest = rest.partition("/")
        if head != "node_modules" or not slash:
            break
        first, slash, rest = rest.partition("/")
        name = first
        if first.startswith("@"):
            second, slash, rest = rest.partition("/")
            name = f"{first}/{second}"
        if not npm_name(name):
            break
        if not slash:
            return value
    raise Dropped(f"the lock installs a package at {clipped(value)}, which is not where npm installs one")


def fitting(values: object, current: str, what: str) -> bool:
    """Whether npm's `os` or `cpu` list `values` admits `current`: a `!` names
    one refused, and any other entry the only ones admitted."""
    if values is None:
        return True
    if not isinstance(values, list) or len(values) > ITEMS or not all(isinstance(item, str) for item in values):
        raise Dropped(f"the lock's {what} is not a list of names")
    if f"!{current}" in values:
        return False
    admitted = [item for item in values if not item.startswith("!")]
    return not admitted or current in admitted


class Bounded:
    """A stream read no further than `ceiling` bytes, refused past it."""

    def __init__(self, file, ceiling: int, what: str) -> None:
        self.file = file
        self.left = ceiling
        self.what = what

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(self.left + 1 if count < 0 else min(count, self.left + 1))
        self.left -= len(chunk)
        if self.left < 0:
            raise Dropped(f"{self.what} unpacks to more than its ceiling of bytes")
        return chunk


class Unpacking:
    """A stream whose every byte read is spent with `unpack`."""

    def __init__(self, file) -> None:
        self.file = file

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(count)
        unpack(len(chunk))
        return chunk


class OneGzip:
    """The one gzip stream `file` holds, decompressed as it is read; whatever
    follows that stream is left for `ended` to refuse."""

    def __init__(self, file) -> None:
        self.file = file
        self.inflate = zlib.decompressobj(wbits=31)
        self.held = b""

    def read(self, count: int = -1) -> bytes:
        count = 1 << 20 if count < 0 else count
        while len(self.held) < count and not self.inflate.eof:
            chunk = self.inflate.unconsumed_tail or self.file.read(1 << 16)
            if not chunk:
                raise Dropped("the tarball's gzip stream ends before it is whole")
            self.held += self.inflate.decompress(chunk, 1 << 20)
        chunk, self.held = self.held[:count], self.held[count:]
        return chunk

    def ended(self) -> bool:
        """Whether the stream is whole and nothing follows it."""
        return self.inflate.eof and not self.inflate.unused_data and not self.file.read(1)


def npm_written(name: str) -> str:
    """`name` as the client writes a file under it, one leading `./` dropped,
    refused where the client would refuse it: printable ASCII alone,
    relative, with no backslash, colon or character Windows refuses, at most
    255 bytes a component and 64 components, and no empty, `.` or `..`
    component, none that ends in a dot or a space, and none that names a
    device Windows reserves."""
    name = name[2:] if name.startswith("./") else name
    shown_name = clipped(name)
    parts = name.split("/")
    if len(parts) > NAME_COMPONENTS or any(len(part) > NAME_COMPONENT_BYTES for part in parts):
        raise Dropped(f"the tarball writes {shown(shown_name)}, deeper than {NAME_COMPONENTS} components "
                      f"or with one longer than {NAME_COMPONENT_BYTES} bytes")
    if any(not " " <= character <= "~" for character in name):
        raise Dropped(f"the tarball writes {shown(shown_name)}, which is not printable ASCII")
    if name.startswith("/") or any(mark in name for mark in "\\:" + WINDOWS_REFUSES):
        raise Dropped(f"the tarball writes {shown(shown_name)}, which is absolute or holds a character "
                      "Windows refuses")
    for part in parts:
        stem = part.split(".")[0].rstrip(" ").lower()
        if part in ("", ".", ".."):
            raise Dropped(f"the tarball writes {shown(shown_name)}, with an empty, . or .. component")
        if part.endswith((".", " ")):
            raise Dropped(f"the tarball writes {shown(shown_name)}, with a component ending in a dot or a space")
        if stem in WINDOWS_DEVICES or (len(stem) == 4 and stem[:3] in ("com", "lpt") and stem[3].isdigit()):
            raise Dropped(f"the tarball writes {shown(shown_name)}, which names a device Windows reserves")
    return name


def npm_after_name(text: str) -> str | None:
    """What follows the package's name `text` begins with, as the client
    finds one: one component, or a scope of `@` and more and one component
    under it; none where it begins with no name."""
    first, _, rest = text.partition("/")
    if not first or first == "node_modules":
        return None
    if not first.startswith("@"):
        return text[len(first):]
    second = rest.split("/")[0]
    if len(first) == 1 or not second:
        return None
    return text[len(first) + 1 + len(second):]


def npm_writes(locations: set[str], held: set[str], location: str, path: str) -> bool:
    """Whether the tarball of the package at `location` may write `path`, as
    the client decides: the deepest of the lock's `locations` above it,
    reached from `location` by `/node_modules/` and a package's name, is one
    `held` in that tarball, and what is below it does not begin with a
    `node_modules` component, whatever its case."""
    folded = path.lower()
    end = deepest = len(location)
    while folded.startswith("/node_modules/", end):
        rest = npm_after_name(folded[end + len("/node_modules/"):])
        if rest is None or not rest.startswith("/"):
            break
        end = len(folded) - len(rest)
        if path[:end] in locations:
            deepest = end
    return path[:deepest] in held and folded[deepest + 1:].split("/")[0] != "node_modules"


def tar_number(field: bytes, binary: bool = False) -> int:
    """The number a tar header's `field` gives, as the client's tar reader
    reads it: where `binary` allows it and the high bit of its first byte is
    set, its last eight bytes in base 256; otherwise octal, up to its first
    NUL, within ASCII whitespace and after at most one `+`."""
    if binary and field[0] & 0x80:
        return int.from_bytes(field[-8:], "big")
    text = field.split(b"\0", 1)[0].strip(b" \t\n\v\f\r")
    digits = text[1:] if text.startswith(b"+") else text
    if not digits or digits.strip(b"01234567") or int(digits, 8) >> 64:
        raise Dropped("the tarball holds a header with a number that is not one")
    return int(digits, 8)


def tar_name(header: bytes) -> bytes:
    """The name a tar `header` gives, as the client's tar reader reads it:
    its name field up to its first NUL, after the prefix field and a `/`
    where the header is ustar and the prefix is not empty or the name field
    holds a backslash."""
    name = header[:100].split(b"\0", 1)[0]
    if header[257:263] == b"ustar\0" and header[263:265] == b"00" and (header[345] or b"\\" in header[:100]):
        prefix = header[345:500].split(b"\0", 1)[0]
        name = prefix + b"/" + name if prefix else name
    return name


def tar_extended(data: bytes) -> list[tuple[bytes, bytes]]:
    """The records of an extended header's `data`, as the client's tar
    reader reads them: a line each up to the first empty one, each `length
    key=value` with `length` counting the line and its newline."""
    records = []
    for line in data.split(b"\n"):
        if not line:
            break
        length, space, record = line.partition(b" ")
        key, equals, value = record.partition(b"=")
        digits = length[1:] if length.startswith(b"+") else length
        if not space or not equals or not digits.isdigit() or int(digits) != len(line) + 1:
            raise Dropped("the tarball holds an extended header the client cannot read")
        records.append((key, value))
    return records


def tar_read(stream, count: int) -> bytes:
    """The next `count` bytes of `stream`, refused where it ends first."""
    held = bytearray()
    while len(held) < count:
        chunk = stream.read(min(count - len(held), 1 << 20))
        if not chunk:
            raise Dropped("the tarball ends inside a header or a member")
        held += chunk
    return bytes(held)


def tar_skip(stream, count: int) -> None:
    """Reads past the next `count` bytes of `stream`, refused where it ends
    first."""
    while count:
        chunk = stream.read(min(count, 1 << 20))
        if not chunk:
            raise Dropped("the tarball ends inside a member")
        count -= len(chunk)


def tarball_of(place: pathlib.Path, wanted: set[str], location: str, locations: set[str]) -> dict:
    """The tarball under `place`, of the package at `location`, as npm
    unpacks it and the client reads it: its SHA-512, SHA-1 and size; the
    files and bytes it writes, its first component stripped and a member
    with nothing after it skipped, directories and links writing nothing,
    and the names it writes, each component held once;
    and the `package.json` and whether a `binding.gyp` lies at each package
    root in `wanted` ("" its own, or a bundled package's below it).

    Its headers are read as the client reads them, each as it is written: a
    GNU long name or link, or an extended header, describes the one member
    after it, and its own name is its header's. One global extended header
    holding only `comment` records, which use up its body, is skipped, as it
    names no member; any other is refused: npm applies one to every member
    after it and the client does not, so the two would write such a member
    under different names.

    It is refused where the client would refuse it: more than one gzip
    stream, a header whose checksum or numbers are wrong, one member
    described twice by the same kind of header or a description with no
    member after it, an extended header the client cannot read or whose
    `size` is not its member's, a member of a kind other than a file, a
    directory or a link, more than `TARBALL_MEMBERS` members, a name the
    client does not write or that is longer than `NPM_PATH_BYTES` under
    `location`, a file written outside its package and the ones it bundles,
    of the lock's `locations`, or straight under a `node_modules` directory,
    a name written twice, or as a file and a directory, once case is folded,
    a directory spelled in two cases, and names that with the directories
    above them are more than `TREE_NAMES`, or hold more than
    `TARBALL_NAME_BYTES`. A sparse member, by its header or by its extended
    one, is refused, though the client writes the second as it is stored."""
    held_locations = {location} | {f"{location}/{root}" for root in wanted if root}
    with opened(place / "artifact") as file:
        sha512 = hashlib.sha512()
        sha1 = hashlib.sha1()
        size = 0
        while chunk := file.read(1 << 20):
            spend(len(chunk))
            size += len(chunk)
            if size > TARBALL_BYTES:
                raise Dropped(f"the tarball is larger than {TARBALL_BYTES} bytes")
            sha512.update(chunk)
            sha1.update(chunk)
        file.seek(0)
        files = unpacked = members = 0
        manifests: dict[str, bytes] = {}
        gyps: set[str] = set()
        # The names written and the directories above them, once folded, as
        # a tree of components each held once, so what is kept grows with
        # the names and not with their depth: each node's child by its
        # component folded, each node's component as spelled, and whether
        # each node is a file.
        children: dict[tuple[int, str], int] = {}
        spelled_at: list[str] = [""]
        files_at: list[bool] = [False]
        parents: list[int] = [0]
        held_bytes = 0
        # What the headers read so far say of the member after them, by type.
        ahead: dict[bytes, bytes] = {}
        global_read = False
        try:
            stream = OneGzip(Spending(file))
            unzipped = Bounded(stream, TARBALL_DECOMPRESSED, "the tarball")
            # One header at a time, and nothing kept of a member once read,
            # so a tarball of many members is not held whole.
            while header := unzipped.read(TAR_BLOCK):
                header += tar_read(unzipped, TAR_BLOCK - len(header))
                if not any(header):
                    break
                if tar_number(header[148:156]) != sum(header[:148]) + sum(header[156:]) + 8 * 32:
                    raise Dropped("the tarball holds a header whose checksum is wrong")
                kind = header[156:157]
                size_of = tar_number(header[124:136], binary=True)
                padding = -size_of % TAR_BLOCK
                if kind in (b"L", b"K", b"x"):
                    ceiling = EXTENDED_BYTES if kind == b"x" else LONG_NAME_BYTES
                    if size_of > ceiling:
                        raise Dropped(f"the tarball holds a long name or extended header of more than {ceiling} bytes")
                    if kind in ahead:
                        raise Dropped("the tarball describes one member twice")
                    ahead[kind] = tar_read(unzipped, size_of)
                    tar_skip(unzipped, padding)
                    continue
                if kind == b"g":
                    # A global header holding only a comment names no member,
                    # so npm, which applies one to every member after it, and
                    # the client, which does not, write the same names. Its
                    # comment is read for its keys alone and never shown.
                    if ahead or global_read:
                        raise Dropped("the tarball holds a global extended header after another, or between "
                                      "a member's own headers and the member")
                    if size_of > EXTENDED_BYTES:
                        raise Dropped(f"the tarball holds a global extended header of more than "
                                      f"{EXTENDED_BYTES} bytes")
                    held_global = tar_read(unzipped, size_of)
                    tar_skip(unzipped, padding)
                    # Its records must use up its body, each line ending in a
                    # newline and none of them blank: the reader stops at a
                    # blank line, as the client's does, and npm's reads on.
                    lines = held_global.split(b"\n")
                    if lines[-1] or not all(lines[:-1]) or any(
                            key != b"comment" for key, _ in tar_extended(held_global)):
                        raise Dropped("the tarball holds a global extended header that says more than a "
                                      "comment, which npm applies to the members after it and the client does not")
                    global_read = True
                    continue
                members += 1
                if members > TARBALL_MEMBERS:
                    raise Dropped(f"the tarball holds more than {TARBALL_MEMBERS} members")
                raw = tar_name(header)
                for key, value in tar_extended(ahead.pop(b"x", b"")):
                    if key in (b"path", b"linkpath") and len(value) > LONG_NAME_BYTES:
                        raise Dropped(f"the tarball holds a name longer than {LONG_NAME_BYTES} bytes")
                    if key == b"path":
                        raw = value
                    digits = value[1:] if value.startswith(b"+") else value
                    if key == b"size" and not (digits.isdigit() and int(digits) == size_of):
                        raise Dropped("the tarball's extended header gives a member another size than its header")
                    if key.startswith(b"GNU.sparse."):
                        raise Dropped("the tarball holds a sparse member, of a kind npm's rules refuse")
                long = ahead.pop(b"L", None)
                if long is not None:
                    raw = long[:-1] if long.endswith(b"\0") else long
                ahead.pop(b"K", None)
                # Every byte stands for itself, so a name outside printable
                # ASCII is refused below as it is written.
                name = raw.decode("latin-1")
                if kind in (b"5", b"2", b"1"):
                    tar_skip(unzipped, size_of + padding)
                    continue
                if kind not in (b"\0", b"0"):
                    raise Dropped(f"the tarball holds {shown(clipped(name))}, of a kind npm's rules refuse")
                _, slash, rest = name.partition("/")
                if not slash or not rest:
                    tar_skip(unzipped, size_of + padding)
                    continue
                rest = npm_written(rest)
                path = f"{location}/{rest}"
                if len(path) > NPM_PATH_BYTES:
                    raise Dropped(f"the tarball writes {shown(clipped(path))}, longer than {NPM_PATH_BYTES} bytes")
                npm_written(path)
                if not npm_writes(locations, held_locations, location, path):
                    raise Dropped(f"the tarball writes {shown(clipped(path))}, outside its package and the ones "
                                  "it bundles, or straight under a node_modules directory")
                tar_number(header[100:108])
                node = 0
                parts = path.split("/")
                for depth, part in enumerate(parts, 1):
                    child = children.get((node, part.casefold()))
                    if child is None:
                        # The first node is the tree's root, which is no name.
                        if len(files_at) > TREE_NAMES:
                            raise Dropped(f"the tarball writes more than {TREE_NAMES} names, each "
                                          "directory above a file counted")
                        held_bytes += len(part)
                        if held_bytes > TARBALL_NAME_BYTES:
                            raise Dropped(f"the tarball's names hold more than {TARBALL_NAME_BYTES} bytes")
                        child = children[(node, part.casefold())] = len(files_at)
                        spelled_at.append(part)
                        files_at.append(depth == len(parts))
                        parents.append(node)
                    elif depth == len(parts) or files_at[child] or spelled_at[child] != part:
                        raise Dropped(f"the tarball writes {shown(clipped(path))} twice, or as a file and "
                                      "a directory, once case is folded, or under a directory spelled "
                                      "in another case")
                    node = child
                files += 1
                unpacked += size_of
                root, _, leaf = rest.rpartition("/")
                if root in wanted and leaf == "binding.gyp":
                    gyps.add(root)
                if root in wanted and leaf == "package.json":
                    if size_of > PACKAGE_JSON_BYTES:
                        raise Dropped(f"{clipped(rest)} is larger than {PACKAGE_JSON_BYTES} bytes")
                    manifests[root] = tar_read(unzipped, size_of)
                    tar_skip(unzipped, padding)
                else:
                    tar_skip(unzipped, size_of + padding)
            if ahead:
                raise Dropped("the tarball describes a member it does not hold")
            while unzipped.read(1 << 20):
                pass
            if not stream.ended():
                raise Dropped("the tarball goes on past its one gzip stream")
        except Dropped:
            raise
        except Exception:  # whatever gzip cannot read is a tarball npm would not unpack
            raise Dropped("is not a gzip-compressed tar this intake can read") from None
    return {
        "files": files,
        "gyps": gyps,
        "manifests": manifests,
        "names": {"children": children, "files_at": files_at, "parents": parents, "spelled_at": spelled_at},
        "sha1": sha1.digest(),
        "sha512": sha512.digest(),
        "size": size,
        "unpacked": unpacked,
    }


def tarball_node(tarball: dict, path: str) -> int | None:
    """The name `tarball` writes at `path`, spelled as it is written, or
    None where it writes none there."""
    names = tarball["names"]
    node = 0
    for part in path.split("/"):
        node = names["children"].get((node, part.casefold()))
        if node is None or names["spelled_at"][node] != part:
            return None
    return node


def tarball_holds(tarball: dict, path: str) -> bool:
    """Whether `tarball` writes a file at `path`."""
    node = tarball_node(tarball, path)
    return node is not None and tarball["names"]["files_at"][node]


def tarball_files_under(tarball: dict, directory: str) -> set[str]:
    """Every file `tarball` writes below `directory`, at any depth."""
    names = tarball["names"]
    top = tarball_node(tarball, directory)
    if top is None or names["files_at"][top]:
        return set()
    below: dict[int, list[int]] = {}
    for node in range(1, len(names["parents"])):
        below.setdefault(names["parents"][node], []).append(node)
    found: set[str] = set()
    waiting = [(top, directory)]
    while waiting:
        node, path = waiting.pop()
        for child in below.get(node, []):
            held = f"{path}/{names['spelled_at'][child]}"
            if names["files_at"][child]:
                found.add(held)
            else:
                waiting.append((child, held))
    return found


def integrity_agrees(value: object, tarball: dict) -> None:
    """Refuse a tarball the lock's `integrity` does not describe: every
    SHA-512 it gives must agree, and a SHA-1 is held only where no SHA-512 is
    given; a lock that gives neither pins nothing npm checks, so it is
    refused."""
    if value is None:
        raise Dropped("the lock gives no SHA-512 or SHA-1 integrity for its tarball")
    given: dict[str, list[bytes]] = {}
    for token in text(value, "the lock's integrity").split():
        algorithm, dash, encoded_digest = token.partition("-")
        encoded_digest = encoded_digest.split("?", 1)[0]
        try:
            given.setdefault(algorithm, []).append(base64.b64decode(encoded_digest, validate=True))
        except ValueError:
            raise Dropped(f"the lock's integrity {clipped(token)} is not base64") from None
    held = given.get("sha512") or given.get("sha1")
    if held is None:
        raise Dropped("the lock gives no SHA-512 or SHA-1 integrity for its tarball")
    expected = tarball["sha512"] if "sha512" in given else tarball["sha1"]
    if any(item != expected for item in held):
        raise Dropped("the tarball is not the bytes the lock's integrity describes")


def install_events(manifest: dict, gyp: bool) -> list[str]:
    """The install-time scripts npm would run for a package, which
    `--ignore-scripts` skips: its own, and the `install` npm implies for a
    `binding.gyp` at its root when it gives neither `install` nor `preinstall`."""
    scripts = manifest.get("scripts")
    scripts = scripts if isinstance(scripts, dict) else {}
    events = [event for event in INSTALL_EVENTS if isinstance(scripts.get(event), str)]
    if gyp and "install" not in events and "preinstall" not in events:
        events.append("install")
    return [event for event in INSTALL_EVENTS if event in events]


def npm_resolved(observed: pathlib.Path, agent: str, package: str, released: str) -> dict:
    """The tree npm's lock under `<agent>/resolve/` describes, with each
    tarball the job left for it checked and counted: the packages it would
    install on some platform, in the lock's order, and the agent's own
    package.json."""
    place = place_of(observed, agent, "resolve")
    try:
        lock = obj(read_json(place / "package-lock.json", LOCK_BYTES, "package-lock.json"), "package-lock.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if lock.get("lockfileVersion") != 3 or isinstance(lock.get("lockfileVersion"), bool):
        raise Dropped("package-lock.json is not in lockfile version 3, the one read here")
    listed = obj(lock.get("packages"), "package-lock.json's packages")
    if len(listed) > LOCK_PACKAGES + 1:
        raise Dropped(f"package-lock.json lists more than {LOCK_PACKAGES} packages")
    order = list(listed.items())
    if not order or order[0][0] != "":
        raise Dropped("package-lock.json's packages do not begin with the project's root")
    packages: list[dict] = []
    folded: set[str] = set()
    for index, (location, fields) in enumerate(order[1:], start=1):
        fields = obj(fields, f"the lock's package {index}")
        if fields.get("dev") is True:
            continue  # never installed, so nothing of it is read
        location = npm_location(location)
        if location.casefold() in folded:
            raise Dropped(f"the lock installs two packages at {clipped(location)}, as a folding file system names them")
        folded.add(location.casefold())
        name = fields.get("name", location.rpartition("node_modules/")[2])
        if not isinstance(name, str) or not npm_name(name):
            raise Dropped(f"the lock names the package at {clipped(location)} {clipped(name)}, which is not a package's name")
        held_version = fields.get("version")
        if (
            not isinstance(held_version, str)
            or not held_version
            or len(held_version) > SEMVER_BYTES
            or not all("!" <= c <= "~" for c in held_version)
        ):
            raise Dropped(f"the lock's version for {clipped(location)} is not a version")
        packages.append({
            "bundled": fields.get("inBundle") is True,
            "cpu": fields.get("cpu"),
            "index": index,
            "libc": fields.get("libc"),
            "location": location,
            "name": name,
            "optional": fields.get("optional") is True,
            "os": fields.get("os"),
            "scripted": fields.get("hasInstallScript") is True,
            "version": held_version,
            "fields": fields,
        })
    top = f"node_modules/{package}"
    own = next((item for item in packages if item["location"] == top), None)
    if own is None or own["bundled"] or own["name"] != package or own["version"] != released:
        raise Dropped(f"the lock installs no {clipped(package)} {clipped(released)} at {clipped(top)}")
    unpacked = {item["location"] for item in packages if not item["bundled"]}
    for item in packages:
        if item["bundled"]:
            above = item["location"]
            holder = None
            while "/node_modules/" in above:
                above = above.rpartition("/node_modules/")[0]
                if above in unpacked:
                    holder = above
                    break
            if holder is None:
                raise Dropped(f"{clipped(item['location'])} is bundled in no package the lock installs above it")
            item["holder"] = holder
    by_location = {item["location"]: item for item in packages}
    locations = set(by_location)
    for item in packages:
        if item["bundled"]:
            continue
        wanted = {""} | {
            other["location"][len(item["location"]) + 1:]
            for other in packages
            if other.get("holder") == item["location"]
        }

        def fetched(item: dict = item, wanted: set[str] = wanted) -> dict:
            url = text(item["fields"].get("resolved"), "its resolved address")
            # The client fetches the address as it is written, so it holds
            # nothing that would have to be encoded first.
            if any(not "!" <= character <= "~" for character in url):
                raise Dropped(
                    f"{shown(clipped(url))} holds a space or a character outside printable ASCII, "
                    "so it is not fetched as it is written"
                )
            if not url.startswith(NPM_REGISTRY):
                raise Dropped(f"{clipped(url)} is not on {NPM_REGISTRY}, the one place a package is taken from")
            plain(url)
            inner = place_of(observed, agent, "tarballs", str(item["index"]))
            tarball = tarball_of(inner, wanted, item["location"], locations)
            integrity_agrees(item["fields"].get("integrity"), tarball)
            reached = chained(inner, url, "the lock's address")
            elsewhere = next((name for name in reached if name != NPM_HOST), None)
            if elsewhere is not None:
                raise Dropped(f"fetched.json reaches {clipped(elsewhere)}, but a tarball is fetched from {NPM_HOST} alone")
            return {"tarball": tarball, "url": url}

        item.update(staged(f"the tarball of {clipped(item['location'])}", fetched))
    for item in packages:
        holder = by_location[item.get("holder", item["location"])]
        root = item["location"][len(holder["location"]) + 1:] if item["bundled"] else ""
        manifest_bytes = holder["tarball"]["manifests"].get(root)
        if manifest_bytes is None:
            raise Dropped(f"the tarball for {clipped(item['location'])} holds no package.json for it")
        try:
            manifest = obj(parsed(manifest_bytes, "its package.json"), "its package.json")
        except Refused as refused:
            raise Dropped(f"{clipped(item['location'])}: {refused}") from None
        if manifest.get("name") != item["name"] or manifest.get("version") != item["version"]:
            raise Dropped(
                f"{clipped(item['location'])}: its package.json names another package or version "
                "than the lock gives it"
            )
        item["manifest"] = manifest
        item["events"] = install_events(manifest, root in holder["tarball"]["gyps"])
        if bool(item["events"]) != item["scripted"]:
            raise Dropped(
                f"the lock says {clipped(item['location'])} "
                + ("has an install script, and its package.json gives none" if item["scripted"]
                   else "has no install script, and its package.json gives "
                   + ", ".join(item["events"]))
            )
    return {"own": own, "packages": packages}


def npm_installed(resolution: dict, platform: str) -> list[dict]:
    """The packages `npm ci --omit=dev` installs on `platform`, in the lock's
    order, decided from the lock as npm decides: an optional one whose `os`,
    `cpu` or `libc` refuses the platform is left out with everything under
    it, and any other such one refuses the target. A `libc` is held only on
    Linux, whose runners are glibc; anywhere else, any `libc` refuses."""
    system, machine = platform.split("-")
    current_os, current_cpu = NPM_OS[system], NPM_CPU[machine]
    linux = system == "linux"
    left_out: list[str] = []
    installed = []
    for item in resolution["packages"]:
        if any(item["location"].startswith(above + "/") for above in left_out):
            continue
        fits_libc = item["libc"] is None or (linux and fitting(item["libc"], NPM_LIBC, "libc"))
        if fitting(item["os"], current_os, "os") and fitting(item["cpu"], current_cpu, "cpu") and fits_libc:
            installed.append(item)
        elif item["optional"]:
            left_out.append(item["location"])
        else:
            raise Dropped(f"{clipped(item['location'])} is not optional, and its os, cpu or libc refuses {platform}")
    if any(item.get("holder") is not None and item["holder"] in left_out for item in installed):
        raise Dropped("a bundled package's holder is not installed here")
    return installed


def npm_licence(item: dict) -> str | None:
    """The licence the package.json of `item` states, exactly as it states
    it, where it states one as text; a legacy object or list of licences is
    none. Refused where the client would refuse it: more than
    `LICENSE_BYTES` long, or anything but printable ASCII."""
    licence = item["manifest"].get("license")
    if not isinstance(licence, str) or not licence:
        return None
    if len(licence) > LICENSE_BYTES or any(not " " <= character <= "~" for character in licence):
        raise Dropped(f"{clipped(item['location'])} gives a licence that is not printable ASCII of at most "
                      f"{LICENSE_BYTES} bytes, which the client refuses")
    return licence


def npm_programs(item: dict, tarball: dict) -> dict[str, str]:
    """The programs the package.json of `item` declares, by name, each with
    its script relative to the package, as `manifest_bins` reads them from
    the files `tarball`, its own or its holder's, writes. A script that is not
    a relative path inside the package is left out, as npm links nothing for
    it. Refused where the client would refuse the lock: a name that is not
    one file name or is Node's own, a script it does not write as given, one in another
    package's location or not a file in the tree, and more than
    `PACKAGE_PROGRAMS` of them."""
    location = item["location"]
    under = location + "/"
    declared = manifest_bins(item["manifest"], item["name"], location, set()) or {}
    directory = item["manifest"].get("directories")
    if item["manifest"].get("bin") is None and isinstance(directory, dict) and isinstance(directory.get("bin"), str):
        try:
            listed = f"{location}/{relative(directory['bin'], 'its directories.bin')}"
        except Dropped:
            listed = None
        if listed is not None:
            files = tarball_files_under(tarball, listed)
            declared = manifest_bins(item["manifest"], item["name"], location, files) or {}
    if len(declared) > PACKAGE_PROGRAMS:
        raise Dropped(f"{clipped(location)} declares more than {PACKAGE_PROGRAMS} programs, which the client refuses")
    programs: dict[str, str] = {}
    for name, script in declared.items():
        try:
            relative(script, "its bin")
        except Dropped:
            continue  # npm links no bin it cannot place
        written = None
        if name and "/" not in name:
            try:
                written = npm_written(name)
            except Dropped:
                pass
        if written != name:
            raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, whose name is not one "
                          "file name the client writes")
        if name.lower() == "node":
            raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, the pinned Node's own "
                          "name, which the client refuses")
        try:
            script = npm_written(script)
        except Dropped:
            raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, whose script the "
                          "client does not write as it is given") from None
        if script.split("/")[0].casefold() == "node_modules":
            raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, whose script lies in "
                          "another package's location, which the client refuses")
        held = under + script
        if len(held) > NPM_PATH_BYTES or not tarball_holds(tarball, held):
            raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, whose script "
                          f"{shown(clipped(held))} is not a file in the tree")
        programs[name] = script
    return programs


def npm_linked(programs: list[tuple[str, dict[str, str]]]) -> None:
    """Refuse the programs of a tree, each package's location with what it
    declares in the lock's order, where the client would refuse to link them
    into `.bin`: more than `TREE_PROGRAMS` in all, two names that differ only
    in case, one name two packages at the top of the tree declare, or one two
    packages below the top declare and none at the top settles."""
    claims: dict[str, dict] = {}
    count = 0
    for location, declared in programs:
        after = npm_after_name(location.removeprefix("node_modules/")) if location.startswith("node_modules/") else None
        top = after == ""
        for name in declared:
            count += 1
            if count > TREE_PROGRAMS:
                raise Dropped(f"the tree declares more than {TREE_PROGRAMS} programs, which the client refuses")
            claim = {"name": name, "location": location, "top": top, "contested": False}
            held = claims.setdefault(name.casefold(), claim)
            if held is claim:
                continue
            if held["name"] != name:
                raise Dropped(f"{clipped(location)} declares the program {shown(clipped(name))}, and "
                              f"{clipped(held['location'])} one whose name differs only in case")
            if held["top"] and top:
                raise Dropped(f"{clipped(location)} and {clipped(held['location'])} both declare the program "
                              f"{shown(clipped(name))} at the top of the tree")
            if top:
                claims[name.casefold()] = claim
            elif not held["top"]:
                held["contested"] = True
    for claim in claims.values():
        if claim["contested"]:
            raise Dropped(f"packages below the top of the tree declare the program {shown(clipped(claim['name']))}, "
                          "and none at the top settles which runs")


def npm_lock(installed: list[dict]) -> bytes:
    """The lock asset a client unpacks the tree from: every package installed,
    in the lock's order, a bundled one marked as such and every other with the
    tarball the intake checked, each with the licence and the programs its
    package.json gives, compact with sorted keys; refused where its tarballs
    write more files or bytes together, or hold more bytes, than the client
    unpacks for one tree, or where the client would refuse a licence or the
    programs."""
    held_tarballs = [item["tarball"] for item in installed if not item["bundled"]]
    for field, ceiling, said in (
        ("files", TREE_FILES, "write more than {} files"),
        ("unpacked", TREE_BYTES, "write more than {} bytes"),
        ("size", TREE_BYTES, "hold more than {} bytes"),
    ):
        if sum(tarball[field] for tarball in held_tarballs) > ceiling:
            raise Dropped(f"its tarballs {said.format(ceiling)} together, more than a client unpacks for one tree")
    by_location = {item["location"]: item for item in installed}
    packages = []
    programs = []
    for item in installed:
        package: dict = {"location": item["location"], "name": item["name"], "version": item["version"]}
        licence = npm_licence(item)
        if licence is not None:
            package["license"] = licence
        declared = npm_programs(item, by_location[item.get("holder", item["location"])]["tarball"])
        if declared:
            package["bin"] = declared
            programs.append((item["location"], declared))
        if item["bundled"]:
            package["in_bundle"] = True
        else:
            tarball = item["tarball"]
            package.update(
                files=tarball["files"],
                sha512=tarball["sha512"].hex(),
                size=tarball["size"],
                unpacked=tarball["unpacked"],
                url=item["url"],
            )
        packages.append(package)
    npm_linked(programs)
    held = compact({"packages": packages, "unpack_rules": UNPACK_RULES})
    if len(held) > ASSET_BYTES:
        raise Dropped(f"its lock is larger than a catalog asset may be, {ASSET_BYTES} bytes")
    return held


def tree_listing(place: pathlib.Path, name: str, built: bool) -> dict[str, dict]:
    """A tree's files by path: npm's, as `tree` lists `npm ci`'s output, or
    crucible's, as `unpack-check` prints the tree a lock unpacks to."""
    try:
        printed = read_json(place / name, TREE_LISTING_BYTES, f"tree/{name}")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    return listed_tree(printed, f"tree/{name}", built)


def listed_tree(printed: object, name: str, built: bool) -> dict[str, dict]:
    """The files by path of the tree listing `printed`, `name` saying which."""
    printed = obj(printed, name)
    files = printed.get("files")
    if not isinstance(files, list) or len(files) > TREE_ENTRIES:
        raise Dropped(f"{name} lists no files, or more than {TREE_ENTRIES}")
    held: dict[str, dict] = {}
    # Two paths a folding file system takes for one are one file listed twice.
    folded: set[str] = set()
    for index, item in enumerate(files):
        item = obj(item, f"{name} file {index}")
        path = text(item.get("path"), f"{name} file {index}'s path")
        if not inside(path) or path.casefold() in folded:
            raise Dropped(f"{name} lists {shown(clipped(path))}, which is not inside its tree or is listed twice")
        if built and item.get("kind") != "file":
            raise Dropped(f"{name} lists {shown(clipped(path))} as other than a file")
        runs = item.get("exec")
        if not isinstance(runs, bool):
            raise Dropped(f"{name}: {shown(clipped(path))}'s exec flag is not true or false")
        held[path] = {
            "exec": runs,
            "sha256": digest(item.get("sha256"), f"{name}: {shown(clipped(path))}'s sha256"),
            "size": number(item.get("size"), f"{name}: {shown(clipped(path))}'s size"),
        }
        folded.add(path.casefold())
    return held


def npm_own(path: str, locations: set[str]) -> bool:
    """Whether npm wrote `path` for itself rather than from a tarball: the
    project's `package.json` and `package-lock.json`, and under the root's or
    a package's `node_modules`, its `.bin` links and `.package-lock.json`."""
    if path in ("package.json", "package-lock.json"):
        return True
    parts = path.split("/")
    for at, part in enumerate(parts[:-1]):
        tail = parts[at + 1:]
        if part == "node_modules" and (tail[0] == ".bin" and len(tail) > 1 or tail == [".package-lock.json"]):
            above = "/".join(parts[:at])
            return above == "" or above in locations
    return False


def owner_of(path: str, locations: set[str]) -> str | None:
    """The deepest installed location `path` lies under."""
    parts = path.split("/")
    for depth in range(len(parts) - 1, 0, -1):
        above = "/".join(parts[:depth])
        if above in locations:
            return above
    return None


def trees_agree(place: pathlib.Path, installed: list[dict], platform: str) -> set[str]:
    """Refuse a target whose tree as npm built it differs from crucible's,
    compared by path, SHA-256 and exec flag under every installed location,
    and give the files crucible's holds. npm makes runnable a file the `bin`
    of the package.json of the package holding it names, and never takes a
    mode away, and writes a `.gitignore` as `.npmignore`; neither is a
    difference. On Windows no file is runnable, so the exec flag is not
    compared."""
    locations = {item["location"] for item in installed}
    npm = tree_listing(place, "npm.json", built=False)
    built = tree_listing(place, "unpack-check.json", built=True)
    every = set(npm) | set(built)
    bins: set[str] = set()
    for item in installed:
        for path in (manifest_bins(item["manifest"], item["name"], item["location"], every) or {}).values():
            try:
                held = f"{item['location']}/{relative(path, 'its bin')}"
            except Dropped:
                continue  # npm links no bin it cannot place, so it marks nothing
            if owner_of(held, locations) == item["location"]:
                bins.add(held)
    ours: dict[str, dict] = {}
    for path, item in npm.items():
        if npm_own(path, locations):
            continue
        if owner_of(path, locations) is None:
            raise Dropped(f"npm installed {clipped(path)}, which lies under no package the lock installs here")
        ours[path] = item
    for path in built:
        directory, slash, leaf = path.rpartition("/")
        renamed = f"{directory}{slash}.npmignore"
        if leaf == ".gitignore" and path not in ours and renamed in ours and renamed not in built:
            ours[path] = ours.pop(renamed)
    windows = platform.startswith("windows-")
    for path in sorted(set(ours) | set(built)):
        if path not in ours:
            raise Dropped(f"npm's tree lacks {clipped(path)}, which crucible's holds")
        if path not in built:
            raise Dropped(f"crucible's tree lacks {clipped(path)}, which npm's holds")
        if ours[path]["sha256"] != built[path]["sha256"] or ours[path]["size"] != built[path]["size"]:
            raise Dropped(f"{clipped(path)} differs between npm's tree and crucible's")
        made_runnable = ours[path]["exec"] and not built[path]["exec"] and path in bins
        if not windows and ours[path]["exec"] != built[path]["exec"] and not made_runnable:
            raise Dropped(f"{clipped(path)} is runnable in one of npm's tree and crucible's and not the other")
    return set(built)


def manifest_bins(manifest: dict, name: str, location: str, files: set[str]) -> dict[str, str] | None:
    """The bins a package's package.json gives, by name, as paths in the
    package: a `bin` given as a path is named as the package is without its
    scope, and with no `bin`, each file of `files` under `directories.bin` is
    one, named as the file is; None where it gives none it can be read for."""
    unscoped = name.rpartition("/")[2]
    given = manifest.get("bin")
    directories = manifest.get("directories")
    if isinstance(given, str):
        return {unscoped: given}
    if isinstance(given, dict) and all(isinstance(path, str) for path in given.values()):
        return dict(given)
    if given is None and isinstance(directories, dict) and isinstance(directories.get("bin"), str):
        try:
            under = f"{location}/{relative(directories['bin'], 'its directories.bin')}/"
        except Dropped:
            return None
        return {path.rpartition("/")[2]: path[len(location) + 1:] for path in sorted(files) if path.startswith(under)}
    return None


def chosen_bin(own: dict, files: set[str]) -> str:
    """The bin `npx` would run for the agent's own package: the only one its
    package.json names, else the one named as the package is without its
    scope."""
    location = own["location"]
    unscoped = own["name"].rpartition("/")[2]
    bins = manifest_bins(own["manifest"], own["name"], location, files)
    if bins is None:
        raise Dropped("its package.json names no bin")
    if len(bins) == 1:
        [path] = bins.values()
    elif unscoped in bins:
        path = bins[unscoped]
    else:
        raise Dropped(f"its package.json names {len(bins)} bins and none {clipped(unscoped)}, so npx would run none")
    held = f"{location}/{relative(path, 'its bin')}"
    if held not in files:
        raise Dropped(f"its bin {clipped(held)} is not a file in the tree")
    return held


def npm_target(
    observed: pathlib.Path,
    agent: str,
    platform: str,
    resolution: dict,
    node: Node,
    decided: dict[str, str],
    gate: Gate,
    assets: dict[str, bytes],
) -> dict:
    """One platform of an npm agent as the catalog will list it, before its
    probe: Node for it, and the lock of what `npm ci` installs on it, once the
    tree npm built there is the tree crucible builds from that lock, with
    each skipped script the reviewer did not decide classed by `classed`."""
    artifact, hosts, cmd = node.artifact(observed, platform, assets)
    installed = npm_installed(resolution, platform)
    lock = npm_lock(installed)
    place = place_of(observed, agent, platform, "tree")
    try:
        made = read(place / "lock.json", ASSET_BYTES, "tree/lock.json")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if made != lock:
        raise Dropped("tree/lock.json is not the lock the intake computes for this platform, so the trees compared are of another")
    files = trees_agree(place, installed, platform)
    bin = chosen_bin(resolution["own"], files)
    named_lock = hashlib.sha256(lock).hexdigest()
    assets[named_lock] = lock
    scripts = []
    for item in installed:
        for event in item["events"]:
            script = {"disposition": decided.get(f"{item['location']} {event}"), "event": event, "package": item["location"]}
            if script["disposition"] is None:
                script = classed(observed, agent, platform, item, script, gate, assets)
            scripts.append(script)
    return {
        "artifacts": [artifact],
        "bin": bin,
        "cmd": cmd,
        "hosts": hosts + ([NPM_HOST] if NPM_HOST not in hosts else []),
        "lock": {"sha256": named_lock, "size": len(lock), "unpack_rules": UNPACK_RULES},
        "node_args": [],
        "platform": platform,
        "scripts": scripts,
    }


def command_words(command: str) -> list[str] | None:
    """The words of `command` as a shell splits it on `COMMAND_BREAKS`, each
    twice, with its backslashes taken as escapes and as written, quotes
    removed; none where it cannot be split, a quote left open or a
    backslash ending it. A quote does not hold a word together here, as a
    quoted `sh -c 'make'` still runs `make`: more words are only more ways
    to be a build."""
    words: list[str] = []
    word = raw = ""
    quote = None
    at = 0
    while at < len(command):
        character = command[at]
        if character == "\\" and quote != "'":
            if at + 1 == len(command):
                return None
            word += command[at + 1]
            raw += character + command[at + 1]
            at += 2
            continue
        if quote is not None and character == quote:
            quote = None
        elif quote is None and character in "'\"":
            quote = character
        elif character in COMMAND_BREAKS:
            words += [word, raw]
            word = raw = ""
        else:
            word += character
            raw += character
        at += 1
    if quote is not None:
        return None
    return [held for held in words + [word, raw] if held]


def builds(command: str) -> str | None:
    """Why `command` is a build by rule, or none where it holds nothing
    `hidden` flags or a shell expands and names no build tool: each word
    named as `BUILD_TOOLS` says. A hidden character makes the script a
    build rather than dropping its target, so the target's other scripts
    are still classed, and this one is held to a build the gate passes."""
    if any(hidden(character) for character in command):
        return "its command holds a character that could hide what it runs"
    expanded = sorted(EXPANDS.intersection(command))
    if expanded:
        return f"its command holds {shown(expanded[0])}, which a shell expands"
    words = command_words(command)
    if words is None:
        return "its command cannot be split as a shell splits it"
    for word in words:
        name = re.split(r"[/\\]", word)[-1].lower().partition("@")[0]
        name = re.sub(r"-[0-9]+(\.[0-9]+)*$", "", re.sub(r"\.(exe|cmd|bat|js|cjs|mjs)$", "", name))
        if name in BUILD_TOOLS or name.startswith(BUILD_PREFIXES) or name.endswith(BUILD_SUFFIXES):
            return f"its command runs {shown(clipped(word))}"
    return None


def run_listing(place: pathlib.Path, name: str) -> tuple[bytes | None, str]:
    """The bytes of the listing `name` one run of a script left under
    `place`, or none, and why, where the run left none, a broken one, or one
    past the client's ceilings for a tree, so it is incomplete."""
    try:
        listing = read(place / name, TREE_LISTING_BYTES, name)
        files = listed_tree(parsed(listing, name), name, built=False)
    except (Refused, Dropped) as failed:
        return None, str(failed)
    if len(files) > TREE_FILES or sum(file["size"] for file in files.values()) > TREE_BYTES:
        return None, f"{name} lists more than {TREE_FILES} files or {TREE_BYTES} bytes, so it is incomplete"
    return listing, ""


def script_hosts(place: pathlib.Path) -> list[str]:
    """The hosts `hosts.json` under `place` says a script's runs reached,
    sorted, each once; refused where one is not a host's name."""
    try:
        given = obj(read_json(place / "hosts.json", SMALL_BYTES, "hosts.json"), "hosts.json").get("hosts")
    except Refused as refused:
        raise Dropped(str(refused)) from None
    if not isinstance(given, list) or len(given) > ITEMS:
        raise Dropped(f"hosts.json gives no list of at most {ITEMS} hosts")
    for index, name in enumerate(given):
        if not host(text(name, f"hosts.json host {index}")):
            raise Dropped(f"hosts.json names {shown(clipped(name))}, which is not a host's name")
    return sorted(set(given))


def classed(
    observed: pathlib.Path, agent: str, platform: str, item: dict, script: dict, gate: Gate, assets: dict[str, bytes]
) -> dict:
    """The skipped script `script` of the installed package `item`, which
    the reviewer did not decide, classed from what the job left of it under
    `scripts/<the SHA-256 of "<location> <event>">/`: as it is, undecided,
    where the job left nothing; otherwise `local` where its two runs left the
    same listing of its package, and `built` where the job built it and the
    licence gate passes over that build. It is a build by rule, by its
    platform, or where its runs differ, are missing or are incomplete; a
    build the job left no output of, or the gate refuses, drops the target."""
    location, event = script["package"], script["event"]
    place = place_of(observed, agent, platform, "scripts", hashlib.sha256(f"{location} {event}".encode()).hexdigest())
    if not os.path.lexists(place):
        return script
    named = f"the {event} script of {shown(clipped(location))}"
    given = item["manifest"].get("scripts")
    command = given.get(event) if isinstance(given, dict) else None
    # The `install` npm implies for a `binding.gyp` runs this.
    command = text(command if isinstance(command, str) else "node-gyp rebuild", f"{named}'s command")
    why = builds(command)
    if why is None and platform.startswith(BUILT_ON):
        why = f"every script on {platform} is built"
    first = second = None
    if why is None:
        first, missing = run_listing(place, "run-1.json")
        second, other = run_listing(place, "run-2.json")
        why = missing or other or (None if first == second else "its two runs left listings of its package that differ")
    if why is None and first is not None and len(first) > ASSET_BYTES:
        why = f"the listing its runs left is larger than a catalog asset may be, {ASSET_BYTES} bytes"
    if why is None and first is not None:
        hosts = staged(named, lambda: script_hosts(place))
        if not gate.kept({"result": first}, assets):
            raise Dropped(
                f"the listing {named}'s runs left would bring what this review holds past {GATE_HELD_BYTES} bytes"
            )
        result = hashlib.sha256(first).hexdigest()
        return {
            **script,
            "command": command,
            "disposition": "local",
            "hosts": hosts,
            "result": {"sha256": result, "size": len(first)},
        }
    if not os.path.lexists(place / "built.tar.gz"):
        raise Dropped(f"{named} is a build ({why}), and the job left no build of it")
    built = {**script, "disposition": "built"}
    staged(f"the build of {named}", lambda: built_output(place, built, gate, assets))
    return built


class Hashing:
    """A stream whose bytes read are hashed and counted."""

    def __init__(self, file) -> None:
        self.file = file
        self.sha = hashlib.sha256()
        self.size = 0

    def read(self, count: int = -1) -> bytes:
        chunk = self.file.read(count)
        self.sha.update(chunk)
        self.size += len(chunk)
        return chunk


def built_output(place: pathlib.Path, script: dict, gate: Gate, assets: dict[str, bytes]) -> None:
    """Gate the build the job left of `script` under `place`, `built.tar.gz`,
    read within `REPACK_BYTES`, as `pack` writes one: refused unless `Packing`
    writes its members again to the same bytes, they are files and
    directories alone, and its files are the ones `built.json` lists. The
    gate's input is the SBOM named by its SHA-256."""
    try:
        made = read(place / "built.tar.gz", REPACK_BYTES, "built.tar.gz")
        listed = listed_tree(read_json(place / "built.json", TREE_LISTING_BYTES, "built.json"), "built.json", built=False)
    except Refused as refused:
        raise Dropped(str(refused)) from None
    unzipped = Bounded(Unpacking(OneGzip(io.BytesIO(made))), REPACK_DECOMPRESSED, "built.tar.gz")
    packing = Packing()
    files: dict[str, dict] = {}
    try:
        with tarfile.open(fileobj=unzipped, mode="r|", tarinfo=Member, encoding="utf-8", errors="surrogateescape") as tar:
            for member in tar:
                if member.type in (tarfile.REGTYPE, tarfile.AREGTYPE):
                    content = Hashing(tar.extractfile(member))
                    packing.add(member.name, "file", member.mode, content=content, size=member.size)
                    files[member.name] = {
                        "exec": bool(member.mode & 0o111), "sha256": content.sha.hexdigest(), "size": content.size,
                    }
                elif member.type == tarfile.DIRTYPE:
                    packing.add(member.name, "dir", member.mode)
                else:
                    raise Dropped(f"built.tar.gz holds {shown(clipped(member.name))}, which is neither a file nor a directory")
        while unzipped.read(1 << 20):
            pass
        again = packing.made()
    except Dropped:
        raise
    except Exception:  # whatever gzip or tar cannot read is no build `pack` wrote
        raise Dropped("built.tar.gz is not a gzip-compressed tar this intake can read") from None
    if again != made:
        raise Dropped("built.tar.gz is not written as `pack` writes a build")
    if files != listed:
        raise Dropped("built.tar.gz does not hold the files built.json lists")
    sha256 = hashlib.sha256(made).hexdigest()
    gate.replacing(script, None if gate.sboms is None else gate.sboms / f"{sha256}.json", made, assets)


def dispositions_of(path: pathlib.Path | None) -> dict[str, dict[str, str]]:
    """The reviewer's dispositions for the scripts `--ignore-scripts` skips, by
    registry id and then `<location> <event>`."""
    if path is None:
        return {}
    value = read_json(path, DISPOSITIONS_BYTES, "the dispositions")
    if not isinstance(value, dict) or not all(isinstance(item, dict) for item in value.values()):
        raise Refused("the dispositions are not an object by registry id of objects by script")
    for agent_id, scripts in value.items():
        for script, disposition in scripts.items():
            where = f"the disposition of {clipped(agent_id)} {clipped(script)}"
            if disposition in ("built", "local"):
                raise Refused(f"{where} is {disposition}, which belongs to the build route, not to this review")
            if disposition not in DISPOSITIONS:
                raise Refused(f"{where} is {clipped(disposition)}, not one of {', '.join(DISPOSITIONS)}")
    return value


def lock_revision(made: dict, held: dict | None) -> int:
    """The lock revision of an npm entry: 1 for a version not reviewed
    before; the merged one where every target's lock is the one merged; and
    one more than the merged one where any lock changed."""
    if held is None:
        return 1
    before = held.get("lock_revision")
    before = before if isinstance(before, int) and not isinstance(before, bool) and before > 0 else 1
    locks = {listed["platform"]: listed["lock"] for listed in made["targets"]}
    merged_locks = {
        str(listed.get("platform")): listed.get("lock")
        for listed in held.get("targets", [])
        if isinstance(listed, dict)
    }
    return before if locks == merged_locks else before + 1


def unlocked(entry: dict) -> dict:
    """An npm entry without its lock revision and each target's lock, the
    one part a version reviewed before may change in."""
    held = {field: value for field, value in entry.items() if field != "lock_revision"}
    targets = held.get("targets")
    if isinstance(targets, list):
        held["targets"] = [
            {field: value for field, value in listed.items() if field != "lock"} if isinstance(listed, dict) else listed
            for listed in targets
        ]
    return held


def ungated(entry: object) -> object:
    """`entry` without each artifact's and script's licence-gate record, which this
    review's inputs decide rather than the release, so a version reviewed
    before is not called changed for it."""
    if not isinstance(entry, dict) or not isinstance(entry.get("targets"), list):
        return entry
    targets = []
    for listed in entry["targets"]:
        for part in ("artifacts", "scripts"):
            if isinstance(listed, dict) and isinstance(listed.get(part), list):
                listed = {
                    **listed,
                    part: [
                        {field: value for field, value in held.items() if field != "licence_gate"}
                        if isinstance(held, dict)
                        else held
                        for held in listed[part]
                    ],
                }
        targets.append(listed)
    return {**entry, "targets": targets}


def probe(value: object, platform: str, runner: dict) -> tuple[dict, list[dict], dict]:
    """The `agentInfo`, sign-in methods and record of one target's probe."""
    value = obj(value, f"the probe output for {platform}")
    if value.get("incomplete") is True:
        raise Dropped(f"the probe output for {platform} is incomplete")
    sandbox = text(value.get("sandbox"), f"the probe's sandbox for {platform}")
    if sandbox != "closed":
        if not sandbox.startswith("unavailable"):
            raise Dropped(f"the probe for {platform} does not say whether its sandbox was closed")
        if runner["confined"]:
            raise Dropped(
                f"{platform} was probed without a sandbox on a runner the matrix lists as confined"
            )
    info = obj(value.get("agentInfo"), f"the probe's agentInfo for {platform}")
    admits = {
        "name": text(info.get("name"), f"the agentInfo name for {platform}"),
        "version": text(info.get("version"), f"the agentInfo version for {platform}"),
    }
    methods = value.get("authMethods")
    if not isinstance(methods, list) or len(methods) > ITEMS:
        raise Dropped(f"the probe for {platform} gives no list of sign-in methods")
    capabilities = value.get("capabilities")
    if len(compact(capabilities)) > CAPABILITY_BYTES:
        raise Dropped(f"the probe's capabilities for {platform} are larger than {CAPABILITY_BYTES} bytes")
    conformance = text(value.get("conformance"), f"the probe's conformance for {platform}")
    if conformance != "not run":
        raise Dropped(
            f"the probe for {platform} reports conformance {clipped(conformance)}, and only "
            "not run is taken in, since no conformance case runs at intake"
        )
    record = {
        "capabilities": capabilities,
        "conformance": conformance,
        "confined": runner["confined"],
        "sandbox": sandbox,
    }
    if "reason" in runner:
        record["reason"] = runner["reason"]
    return admits, [method(item, index) for index, item in enumerate(methods)], record


def entry(
    observed: pathlib.Path,
    agent: dict,
    agent_id: str,
    choice: dict,
    runners: dict,
    allowed: set[str],
    gate: Gate,
    interpreters: dict[str, dict],
    node: Node,
    decided: dict[str, str],
    dropped: list[dict],
) -> tuple[dict, dict[str, bytes]]:
    distribution = agent.get("distribution")
    distribution = distribution if isinstance(distribution, dict) else {}
    # A binary wins, then `uvx`: `npx` is read only for an agent with neither.
    binary = distribution.get("binary")
    uvx = distribution.get("uvx") if binary is None else None
    npx = distribution.get("npx") if binary is None and uvx is None else None
    if isinstance(binary, dict):
        kind = "binary"
        if len(binary) > ITEMS:
            raise Dropped(f"lists more than {ITEMS} binary targets")
    elif isinstance(uvx, dict):
        kind = "uvx"
        package = uvx.get("package")
        pinned = PACKAGE.fullmatch(package) if isinstance(package, str) else None
        if pinned is None or not PINNED.fullmatch(pinned[2]):
            raise Dropped(f"the uvx package {clipped(package)} is not pinned as name==version")
        name, pinned_version = pinned[1], version(pinned[2], "the uvx package's version")
        args = arguments(uvx.get("args"), "args")
        env = variables(uvx.get("env"), "env")
    elif isinstance(npx, dict):
        kind = "npm"
        name, pinned_version = npx_package(npx.get("package"))
        args = arguments(npx.get("args"), "args")
        env = variables(npx.get("env"), "env")
    else:
        raise Dropped("is distributed neither as a binary nor through uvx or npx, the kinds this intake takes in")
    released = version(agent.get("version"), "the version")
    licence = text(agent.get("license"), "the licence")
    reached = reach(choice.get("reach"), "the chosen reach")
    mirrored = licence if licence in allowed else None
    assets: dict[str, bytes] = {}
    targets = []
    # npm's lock is read once for every target, where Node can run any.
    resolution = (
        npm_resolved(observed, agent_id, name, pinned_version)
        if kind == "npm" and node.pinned is not None and node.failure is None
        else None
    )
    for platform, spec in sorted(binary.items()) if kind == "binary" else [(platform, None) for platform in sorted(runners)]:
        try:
            if kind == "binary":
                targets.append(target(observed, agent_id, platform, spec, runners, mirrored, gate, assets))
            elif kind == "npm":
                made = npm_target(observed, agent_id, platform, resolution, node, decided, gate, assets)
                targets.append({"args": args, "env": env, **made})
            else:
                made = uvx_target(observed, agent_id, platform, name, pinned_version, interpreters, gate, assets)
                targets.append({"args": args, "env": env, **made})
        except Exception as failed:  # whatever one target breaks, the others go on
            row = {"platform": clipped(platform), "reason": reason(failed), "registry_id": agent_id}
            if isinstance(failed, Build):
                row["build"] = failed.build
            dropped.append(row)
    if not targets:
        raise Dropped("has no target left to offer")
    probed = []
    for listed in targets:
        platform = listed["platform"]
        try:
            printed = read_json(place_of(observed, agent_id, platform) / "probe.json", PROBE_BYTES, "probe.json")
        except Refused as refused:
            raise Dropped(f"has no probe output for {platform}: {refused}") from None
        admits, methods, record = probe(printed, platform, runners[platform])
        listed["probe"] = record
        probed.append((platform, admits, methods))
    first, admits, methods = probed[0]
    for platform, other_admits, other_methods in probed[1:]:
        differing = differences(
            {"admits": other_admits, "auth_methods": other_methods},
            {"admits": admits, "auth_methods": methods},
        )
        if differing:
            raise Dropped(f"its probes on {first} and {platform} disagree about {named(differing)}")
    return (
        {
            "admits": admits,
            "auth_methods": methods,
            "key": choice["key"],
            "kind": kind,
            "license": licence,
            "reach": reached,
            "registry_id": agent_id,
            "targets": targets,
            "version": released,
        },
        assets,
    )


def reason(failed: Exception) -> str:
    """Why `failed` drops what it was raised over."""
    if isinstance(failed, (Dropped, Refused)):
        return str(failed)
    return f"could not be read here: {type(failed).__name__}: {clipped(failed)}"


def merged_of(path: pathlib.Path | None) -> dict[tuple[str, str], dict]:
    """The entries the merged data holds, by key and version."""
    if path is None:
        return {}
    data = read_json(path, DATA_BYTES, "the merged data")
    entries = data.get("entries") if isinstance(data, dict) else None
    if not isinstance(entries, list) or not all(isinstance(entry, dict) for entry in entries):
        raise Refused("the merged data is not review data as the intake writes it")
    return {(str(entry.get("key")), str(entry.get("version"))): entry for entry in entries}


def choices_of(value: object) -> dict[str, dict]:
    if not isinstance(value, dict):
        raise Refused("the choices are not an object by registry id")
    keys: set[str] = set()
    for agent_id, choice in value.items():
        if not isinstance(choice, dict) or not isinstance(choice.get("key"), str):
            raise Refused(f"the choice for {clipped(agent_id)} names no key")
        key = choice["key"]
        if not AGENT_KEY.fullmatch(key):
            raise Refused(f"{clipped(key)} is not an agent key: 1 to 32 lowercase letters, digits and hyphens")
        if key in keys:
            raise Refused(f"the key {key} is chosen for two agents")
        keys.add(key)
    return value


def runners_of(value: object) -> dict[str, dict]:
    if not isinstance(value, dict):
        raise Refused("the runner matrix is not an object by platform")
    held = {}
    for platform, runner in value.items():
        if not PLATFORM.fullmatch(platform) or not isinstance(runner, dict):
            raise Refused(f"the runner matrix names {clipped(platform)}, which is not a platform")
        confined = runner.get("confined")
        if not isinstance(confined, bool):
            raise Refused(f"the runner for {platform} does not say whether it is confined")
        held[platform] = {"confined": confined}
        if not confined:
            reason = runner.get("reason")
            if not isinstance(reason, str) or not reason:
                raise Refused(f"the runner for {platform} is unconfined and gives no reason")
            held[platform]["reason"] = reason
    return held


def allowed_licences(path: pathlib.Path) -> set[str]:
    try:
        listed = tomllib.loads(read(path, SMALL_BYTES, "deny.toml").decode())["licenses"]["allow"]
    except (tomllib.TOMLDecodeError, UnicodeDecodeError, KeyError, TypeError):
        raise Refused("deny.toml gives no licence list") from None
    if not isinstance(listed, list) or not all(isinstance(item, str) for item in listed):
        raise Refused("deny.toml's licence list is not a list of names")
    return set(listed)


class Ungated(Exception):
    """Why the licence gate refuses an input."""


def spdx_tokens(expression: str) -> list[str]:
    return re.findall(r"[()]|[^\s()]+", expression)


def spdx_chosen(expression: str, allowed: set[str]) -> list[str]:
    """The licences `expression` is used under, each as `allowed` spells it,
    in written order: an `AND` both sides, an `OR` the first side that passes.
    Refused where it does not parse, is longer than `LICENSE_BYTES`, or no
    side of it passes; the reason is the first that failed."""
    if len(encoded(expression, "the licence")) > LICENSE_BYTES:
        raise Ungated(f"gives a licence longer than {LICENSE_BYTES} bytes")
    tokens = spdx_tokens(expression)
    operators = {"AND", "OR", "WITH", "(", ")"}
    at = 0

    def unparsed() -> Ungated:
        return Ungated(f"gives the licence {shown(clipped(expression))}, which does not parse as an SPDX expression")

    def take() -> str:
        nonlocal at
        if at >= len(tokens):
            raise unparsed()
        at += 1
        return tokens[at - 1]

    def single(token: str) -> tuple[list[str] | None, str]:
        if token in ("NOASSERTION", "NONE"):
            return None, f"gives the licence {token}, which asserts none"
        if token.startswith(("LicenseRef-", "DocumentRef-")):
            return None, f"gives the licence {shown(clipped(token))}, a reference to text no list can hold"
        if token.endswith("+"):
            return None, f"gives the licence {shown(clipped(token))}, that or any later version, which no list can hold"
        if token not in allowed:
            return None, f"gives the licence {shown(clipped(token))}, which is not on deny.toml's list"
        return [token], ""

    def atom() -> tuple[list[str] | None, str]:
        token = take()
        if token == "(":
            held = either()
            if take() != ")":
                raise unparsed()
            return held
        if token in operators:
            raise unparsed()
        if at < len(tokens) and tokens[at] == "WITH":
            take()
            exception = take()
            if exception in operators:
                raise unparsed()
            whole = f"{token} WITH {exception}"
            if whole not in allowed:
                return None, f"gives the licence {shown(clipped(whole))}, which is not on deny.toml's list as one"
            return [whole], ""
        return single(token)

    def both() -> tuple[list[str] | None, str]:
        chosen, why = atom()
        while at < len(tokens) and tokens[at] == "AND":
            take()
            right, right_why = atom()
            if chosen is not None:
                chosen, why = (None, right_why) if right is None else (chosen + right, "")
        return chosen, why

    def either() -> tuple[list[str] | None, str]:
        chosen, why = both()
        while at < len(tokens) and tokens[at] == "OR":
            take()
            right, right_why = both()
            if chosen is None and right is not None:
                chosen, why = right, ""
        return chosen, why

    if not tokens:
        raise unparsed()
    chosen, why = either()
    if at != len(tokens):
        raise unparsed()
    if chosen is None:
        raise Ungated(why)
    return chosen


def component_text(value: object, what: str) -> str:
    """Copyright lines a component carries, as text a NOTICE holds."""
    held = obj(value, what)
    if "encoding" in held:
        raise Ungated(f"carries {what} in an encoding, which the gate does not decode")
    content = held.get("content")
    if not isinstance(content, str):
        raise Ungated(f"carries {what} with no content")
    if len(encoded(content, what)) > COMPONENT_TEXT_BYTES:
        raise Ungated(f"carries {what} longer than {COMPONENT_TEXT_BYTES} bytes")
    unshown(content.replace("\n", "").replace("\t", ""), what)
    return content


def unshown(value: str, what: str) -> None:
    """Refuse `value`, bound for the NOTICE, where any character in it could
    hide or reorder what a reader of the NOTICE sees, as `shown` would write
    it out."""
    found = next((c for c in value if hidden(c)), None)
    if found is not None:
        raise Ungated(f"gives {what} holding {shown(found)}, which could hide or reorder text in the NOTICE")


def sbom_component(item: dict) -> tuple[list[str], list[str]]:
    """A CycloneDX component's licences, each an expression, and its
    copyright lines; several licences are all of them, as with `AND`."""
    if item.get("components"):
        raise Ungated("nests components, which the gate does not read past")
    licences = item.get("licenses", [])
    if not isinstance(licences, list) or len(licences) > ITEMS:
        raise Ungated(f"gives no list of at most {ITEMS} licences")
    expressions, texts = [], []
    for listed in licences:
        listed = obj(listed, "a licence")
        if "expression" in listed and "license" not in listed:
            expression = listed["expression"]
        elif "license" in listed and "expression" not in listed:
            licence = obj(listed["license"], "a licence")
            expression = licence.get("id")
            if not isinstance(expression, str):
                raise Ungated("names a licence by no SPDX id")
            if licence.get("text") is not None:
                texts.append(component_text(licence["text"], "a licence text"))
        else:
            raise Ungated("gives a licence that is neither an id nor an expression")
        if not isinstance(expression, str):
            raise Ungated("gives a licence expression that is not text")
        expressions.append(expression)
    return expressions, texts


def lock_component(item: dict) -> tuple[list[str], list[str]]:
    """A package of the intake's npm lock: the licence its package.json states."""
    licence = item.get("license")
    if licence is None:
        return [], []
    if not isinstance(licence, str):
        raise Ungated("gives a licence that is not text")
    return [licence], []


def gate_components(held: bytes) -> list[tuple[str, str, list[str], str]]:
    """Every component of the input, in its order: its name, version,
    licences and copyright lines; refused where it is neither format."""
    value = parsed(held, "the licence gate's input")
    if isinstance(value, dict) and value.get("bomFormat") == "CycloneDX":
        listed, read_one = value.get("components"), sbom_component
    elif isinstance(value, dict) and isinstance(value.get("packages"), list) and "unpack_rules" in value:
        listed, read_one = value["packages"], lock_component
    else:
        raise Ungated("the input is neither a CycloneDX SBOM nor an npm lock the intake wrote")
    if not isinstance(listed, list) or not listed:
        raise Ungated("the input lists no component")
    if read_one is sbom_component and "metadata" in value:
        # The SBOM's own subject is part of the input where it gives a
        # licence; one in no shape the gate reads is refused, not passed over.
        metadata = value["metadata"]
        if not isinstance(metadata, dict):
            raise Ungated("the input's metadata is not an object")
        subject = metadata.get("component")
        if subject is not None:
            if not isinstance(subject, dict):
                raise Ungated("the input's metadata.component is not an object")
            licences = subject.get("licenses", [])
            if not isinstance(licences, list):
                raise Ungated("the input's metadata.component gives licenses that are not a list")
            if licences:
                listed = [subject, *listed]
    if len(listed) > GATE_COMPONENTS:
        raise Ungated(f"the input lists more than {GATE_COMPONENTS} components")
    components = []
    carried = 0
    for index, item in enumerate(listed):
        item = obj(item, f"component {index}")
        name = text(item.get("name"), f"component {index}'s name")
        who = shown(clipped(name))
        try:
            unshown(name, "a name")
            version_of = text(item.get("version"), "its version")
            who = f"{who} {shown(clipped(version_of))}"
            unshown(version_of, "a version")
            expressions, texts = read_one(item)
            if not expressions:
                raise Ungated("gives no licence")
            carried += sum(len(item.encode()) for item in texts)
            if carried > COMPONENT_TEXTS_BYTES:
                raise Ungated(f"brings the copyright lines components carry together past {COMPONENT_TEXTS_BYTES} bytes")
        except (Ungated, Dropped) as failed:
            raise Ungated(f"the component {who} {failed}") from None
        components.append((name, version_of, expressions, "\n".join(texts)))
    return components


def licence_gate(
    source: pathlib.Path, allowed: set[str], texts: pathlib.Path, asset: pathlib.Path | bytes
) -> tuple[dict, dict[str, bytes]]:
    """The gate over a build's whole input `source`: its record, and where it
    passes the files that ship beside `asset`, a file or the bytes of one, `NOTICE` and
    `LICENSES/<id>.txt`. Every component must be used under licences
    `allowed` lists, each with its text under `texts`."""
    try:
        components = gate_components(read(source, GATE_INPUT_BYTES, "the licence gate's input"))
        lines = set()
        used: set[str] = set()
        for name, version_of, expressions, carried in components:
            chosen: list[str] = []
            for expression in expressions:
                try:
                    picked = spdx_chosen(expression, allowed)
                except Ungated as failed:
                    raise Ungated(f"the component {shown(clipped(name))} {shown(clipped(version_of))} {failed}") from None
                chosen += [licence for licence in picked if licence not in chosen]
            used.update(chosen)
            lines.add((name, version_of, " AND ".join(chosen), carried))
        held_texts = {}
        for licence in sorted(used):
            if not inside(f"{licence}.txt"):
                raise Ungated(f"{licence} cannot name a licence text file")
            try:
                held_texts[licence] = read(texts / f"{licence}.txt", LICENCE_TEXT_BYTES, f"the licence text of {licence}")
                held_texts[licence].decode()
            except UnicodeDecodeError:
                raise Ungated(f"the licence text of {licence} is not UTF-8") from None
        parts = [
            "This asset is built from the components below, each with its version and the\n"
            "licence it is used under. The text of each licence follows them.\n\n"
        ]
        for name, version_of, licence, carried in sorted(lines):
            parts.append(f"{name} {version_of} {licence}\n")
            parts += [f"    {line}\n" for line in carried.splitlines()]
        notice = "".join(parts).encode()
        for licence, content in held_texts.items():
            notice += f"\n==== {licence} ====\n\n".encode() + content + (b"" if content.endswith(b"\n") else b"\n")
            if len(notice) > NOTICE_BYTES:
                raise Ungated(f"the NOTICE would be larger than {NOTICE_BYTES} bytes")
        if isinstance(asset, bytes):
            sha256, size = hashlib.sha256(asset).hexdigest(), len(asset)
        else:
            sha256, size = hashed(asset)
    except Spent:
        raise
    except (Ungated, Refused, Dropped) as failed:
        return {"passed": False, "reason": clipped(failed)}, {}
    files = {"NOTICE": notice, **{f"LICENSES/{licence}.txt": content for licence, content in held_texts.items()}}
    record = {
        "asset": {"sha256": sha256, "size": size},
        "components": len(components),
        "licences": [
            {"id": licence, "sha256": hashlib.sha256(content).hexdigest(), "size": len(content)}
            for licence, content in held_texts.items()
        ],
        "notice": {"sha256": hashlib.sha256(notice).hexdigest(), "size": len(notice)},
        "passed": True,
    }
    return record, files


# The record of a mirror-marked artifact `review` was given no input for.
UNGATED = {"passed": False, "reason": "no lockfile or SBOM was given"}


class Gate:
    """The licence gate as `review` runs it, over each mirror-marked
    artifact's input in `sboms`, named `<artifact sha256>.json`. What it
    keeps of every NOTICE and licence text until the run writes them is held
    within `GATE_HELD_BYTES` for the run, with every listing a local
    script's runs agreed on, and what it keeps of every repack and bundle
    within `REPACK_HELD_BYTES`, each distinct file counted once."""

    def __init__(self, allowed: set[str], sboms: pathlib.Path | None, texts: pathlib.Path | None) -> None:
        if sboms is not None and texts is None:
            raise Refused("--sboms is given with no --texts, so no licence text could ship beside a mirror")
        self.allowed, self.sboms, self.texts = allowed, sboms, texts
        self.held: set[str] = set()
        self.held_bytes = 0
        self.repacked: set[str] = set()
        self.repacked_bytes = 0

    def of(self, sha256: str, asset: pathlib.Path, assets: dict[str, bytes]) -> dict:
        """The record for the artifact `asset`, whose hash is `sha256`; the
        NOTICE and licence texts of a passing one are kept as assets."""
        source = None if self.sboms is None else self.sboms / f"{sha256}.json"
        if source is None or self.texts is None or not os.path.lexists(source):
            return dict(UNGATED)
        record, files = licence_gate(source, self.allowed, self.texts, asset)
        if not self.kept(files, assets):
            return {
                "passed": False,
                "reason": f"its NOTICE would bring what this review holds past {GATE_HELD_BYTES} bytes",
            }
        return record

    def kept(self, files: dict[str, bytes], assets: dict[str, bytes]) -> bool:
        """Whether `files` are kept as assets by their hashes, which they
        are only within `GATE_HELD_BYTES` for the run."""
        named = self.beside(files)
        if named is None:
            return False
        self.held_bytes += sum(len(content) for sha, content in named.items() if sha not in self.held)
        self.held.update(named)
        assets.update(named)
        return True

    def beside(self, files: dict[str, bytes]) -> dict[str, bytes] | None:
        """The NOTICE and licence texts `files` by their hashes, or none
        where keeping them would bring what this review holds past
        `GATE_HELD_BYTES`."""
        named = {hashlib.sha256(content).hexdigest(): content for content in files.values()}
        more = sum(len(content) for sha, content in named.items() if sha not in self.held)
        return None if self.held_bytes + more > GATE_HELD_BYTES else named

    def replacing(self, artifact: dict, source: pathlib.Path | None, output: bytes, assets: dict[str, bytes]) -> None:
        """Gate `output`, a repack, bundle or build that installs in `artifact`'s
        place, over its whole input `source`, and record it with `replaced`;
        where it passes, `output`, its NOTICE and its licence texts are kept
        as assets. A failing record drops the artifact, which has no route
        to install but `output`."""
        if source is None or not os.path.lexists(source):
            raise Dropped(UNGATED["reason"])
        if self.texts is None:
            raise Dropped("no licence texts were given, so nothing could ship beside it")
        sha256 = hashlib.sha256(output).hexdigest()
        more = 0 if sha256 in self.repacked else len(output)
        if self.repacked_bytes + more > REPACK_HELD_BYTES:
            raise Dropped(f"its repack would bring what this review holds of repacks past {REPACK_HELD_BYTES} bytes")
        record, files = licence_gate(source, self.allowed, self.texts, output)
        if record["passed"] is not True:
            raise Dropped(record["reason"])
        named = self.beside(files)
        if named is None:
            raise Dropped(f"its NOTICE would bring what this review holds past {GATE_HELD_BYTES} bytes")
        self.held_bytes += sum(len(content) for sha, content in named.items() if sha not in self.held)
        self.held.update(named)
        self.repacked.add(sha256)
        self.repacked_bytes += more
        replaced(artifact, record)
        assets.update(named)
        assets[sha256] = output


def replaced(artifact: dict, record: dict) -> None:
    """Record the gate's `record` over an output built or repacked from
    `artifact`; where it passed, that output is what installs in the
    artifact's place, named by the hash and size the record binds."""
    artifact["licence_gate"] = record
    artifact.pop("replaced_by", None)
    if record.get("passed") is True:
        artifact["replaced_by"] = {"sha256": record["asset"]["sha256"], "size": record["asset"]["size"]}


def licence_gate_command(arguments: argparse.Namespace) -> int:
    allowed = allowed_licences(arguments.deny)
    if os.path.lexists(arguments.out):
        raise Refused("--out already exists; the gate writes a directory of its own")
    record, files = licence_gate(arguments.input, allowed, arguments.texts, arguments.asset)
    if record["passed"]:
        try:
            (arguments.out / "LICENSES").mkdir(parents=True)
            for name, content in sorted(files.items()):
                (arguments.out / name).write_bytes(content)
        except OSError as failed:
            # `--out` did not exist before, so all of it is this run's, and a
            # rerun must not be refused for what a failed write left.
            shutil.rmtree(arguments.out, ignore_errors=True)
            raise Refused(f"the NOTICE could not be written to {clipped(arguments.out)}: {clipped(failed.strerror)}") from None
    print(json.dumps(record, sort_keys=True))
    if not record["passed"]:
        raise Refused(f"the licence gate refuses the input: {record['reason']}")
    return 0


class OneBz2:
    """The one bzip2 stream `file` holds, decompressed as it is read, no more
    than `ARTIFACT_BYTES` of it read; whatever follows that stream is left for
    `ended` to refuse."""

    def __init__(self, file) -> None:
        self.file = file
        self.decompressor = bz2.BZ2Decompressor()
        self.taken = 0

    def read(self, count: int = -1) -> bytes:
        count = 1 << 20 if count < 0 else count
        held = b""
        while len(held) < count and not self.decompressor.eof:
            chunk = b""
            if self.decompressor.needs_input:
                chunk = self.file.read(1 << 16)
                if not chunk:
                    raise Dropped("the archive ends before its bzip2 stream does")
                spend(len(chunk))
                self.taken += len(chunk)
                if self.taken > ARTIFACT_BYTES:
                    raise Dropped(f"the archive is larger than {ARTIFACT_BYTES} bytes")
            try:
                held += self.decompressor.decompress(chunk, count - len(held))
            except (OSError, EOFError):
                raise Dropped("is not a bzip2-compressed tar this intake can read") from None
        return held

    def ended(self) -> bool:
        """Whether the stream is whole and nothing follows it."""
        return self.decompressor.eof and not self.decompressor.unused_data and not self.file.read(1)


class Member(tarfile.TarInfo):
    """A member of a `.tar.bz2`, its headers held, before `tarfile` reads
    them, to what the client's tar reader takes: a GNU long name or link of
    at most `LONG_NAME_BYTES`, an extended header of at most
    `EXTENDED_BYTES` whose `size` is its member's, no header describing one
    member twice, and no global or Solaris extended header and no sparse
    member, which the client would read otherwise than `tarfile` does."""

    def _proc_member(self, archive):
        described = archive.__dict__.setdefault("described", set())
        if self.type in (tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK, tarfile.XHDTYPE):
            ceiling = EXTENDED_BYTES if self.type == tarfile.XHDTYPE else LONG_NAME_BYTES
            if self.size > ceiling:
                kind = "an extended header" if self.type == tarfile.XHDTYPE else "a long name or link"
                raise Dropped(f"the archive holds {kind} of more than {ceiling} bytes")
            if self.type in described:
                raise Dropped("the archive describes one member twice")
            described.add(self.type)
        else:
            described.clear()
            if self.type == tarfile.XGLTYPE:
                raise Dropped("the archive holds a global extended header, which the client does not apply")
            if self.type == tarfile.SOLARIS_XHDTYPE:
                raise Dropped("the archive holds a Solaris extended header, which the client does not read")
            if self.type == tarfile.GNUTYPE_SPARSE:
                raise Dropped("the archive holds a sparse member, which the client refuses")
        return super()._proc_member(archive)

    def _proc_gnusparse_00(self, next, raw_headers):
        raise Dropped("the archive holds a sparse member, which the client refuses")

    def _proc_gnusparse_01(self, next, pax_headers):
        raise Dropped("the archive holds a sparse member, which the client refuses")

    def _proc_gnusparse_10(self, next, pax_headers, tarfile):
        raise Dropped("the archive holds a sparse member, which the client refuses")

    def _apply_pax_info(self, pax_headers, encoding, errors) -> None:
        if any(key.startswith("GNU.sparse.") for key in pax_headers):
            raise Dropped("the archive holds a sparse member, which the client refuses")
        given = pax_headers.get("size")
        if given is not None and not (given.isdigit() and int(given) == self.size):
            raise Dropped("the archive's extended header gives a member another size than its header")
        super()._apply_pax_info(pax_headers, encoding, errors)


class Sink(io.BytesIO):
    """What a repack writes, refused past `REPACK_BYTES`."""

    def write(self, data) -> int:
        if self.tell() + len(data) > REPACK_BYTES:
            raise Dropped(f"the repack would be larger than {REPACK_BYTES} bytes")
        return super().write(data)


class Packing:
    """A deterministic tar.gz, as a repack and a FreeBSD bundle are written:
    a GNU tar, which the client's tar reader reads, its long names in GNU
    long name headers, in one gzip stream at `REPACK_LEVEL` whose header
    names no file and carries mtime 0. Each member keeps its name, kind,
    mode's permission bits and content; its owner and group are 0, their
    names empty, and its mtime 0. Each is held to the client's rules, so a
    repack never turns a refusal into a pass: at most `TARBALL_MEMBERS`, a
    name and a link's target of at most `LONG_NAME_BYTES` - 1 bytes, a name
    the client writes, a link's target relative, and no name written twice,
    or as a file and a directory, once case is folded."""

    def __init__(self) -> None:
        self.sink = Sink()
        self.gzip = gzip.GzipFile(filename="", mode="wb", fileobj=self.sink, compresslevel=REPACK_LEVEL, mtime=0)
        self.tar = tarfile.open(fileobj=self.gzip, mode="w", format=tarfile.GNU_FORMAT, encoding="utf-8")
        self.members = 0
        # The names written and the directories above them, once folded, as
        # a tree of components: each node's child by its component folded,
        # and each node's component as spelled and its kind, `None` for a
        # directory only a name below it implies.
        self.children: dict[tuple[int, str], int] = {}
        self.spelled: list[str] = [""]
        self.kinds: list[str | None] = ["dir"]
        self.name_bytes = 0

    def add(self, name: str, kind: str, mode: int, *, content=None, size: int = 0, link: str = "") -> None:
        self.members += 1
        if self.members > TARBALL_MEMBERS:
            raise Dropped(f"the archive holds more than {TARBALL_MEMBERS} members")
        for value in (name, link):
            if len(value.encode(errors="surrogateescape")) > LONG_NAME_BYTES - 1:
                raise Dropped(f"the archive holds a name or link longer than {LONG_NAME_BYTES - 1} bytes")
        name = npm_written(name)
        if kind == "link" and (not link or link.startswith("/") or any(mark in link for mark in "\\:\0")):
            raise Dropped(f"{shown(clipped(name))} links to {shown(clipped(link))}, which is not a relative link")
        node = 0
        parts = name.split("/")
        for depth, part in enumerate(parts, 1):
            last = depth == len(parts)
            child = self.children.get((node, part.casefold()))
            if child is None:
                if len(self.kinds) > TREE_NAMES:
                    raise Dropped(f"the archive writes more than {TREE_NAMES} names, each directory above a file counted")
                self.name_bytes += len(part)
                if self.name_bytes > TARBALL_NAME_BYTES:
                    raise Dropped(f"the archive's names hold more than {TARBALL_NAME_BYTES} bytes")
                child = self.children[(node, part.casefold())] = len(self.kinds)
                self.spelled.append(part)
                self.kinds.append(kind if last else None)
            elif (
                self.spelled[child] != part
                or self.kinds[child] not in (None, "dir")
                or (last and (self.kinds[child] is not None or kind != "dir"))
            ):
                raise Dropped(f"the archive writes {shown(clipped(name))} twice, or as a file and a directory, "
                              "once case is folded, or under a directory spelled in another case")
            elif last:
                self.kinds[child] = kind
            node = child
        info = tarfile.TarInfo(name)
        info.type = {"file": tarfile.REGTYPE, "dir": tarfile.DIRTYPE, "link": tarfile.SYMTYPE}[kind]
        info.mode = mode & 0o777
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        info.mtime = 0
        info.linkname = link
        info.size = size if kind == "file" else 0
        self.tar.addfile(info, content if kind == "file" else None)

    def made(self) -> bytes:
        self.tar.close()
        self.gzip.close()
        return self.sink.getvalue()


def repacked(file) -> bytes:
    """The `.tar.bz2` `file` repacked as `Packing` writes a tar.gz, its
    members in their order; refused where it is not one bzip2 stream with
    nothing after it, its compressed bytes are more than `ARTIFACT_BYTES`, it
    unpacks to more than `REPACK_DECOMPRESSED`, or a member is anything but a
    file, a directory or a symbolic link: a hard link, a device, a FIFO or a
    sparse member is refused, as the client refuses it. In `review`, what it
    unpacks is spent against the run's `REPACK_RUN_DECOMPRESSED`."""
    stream = OneBz2(file)
    unzipped = Bounded(Unpacking(stream), REPACK_DECOMPRESSED, "the archive")
    packing = Packing()
    try:
        with tarfile.open(fileobj=unzipped, mode="r|", tarinfo=Member, encoding="utf-8", errors="surrogateescape") as tar:
            for member in tar:
                if member.type in (tarfile.REGTYPE, tarfile.AREGTYPE):
                    packing.add(member.name, "file", member.mode, content=tar.extractfile(member), size=member.size)
                elif member.type == tarfile.DIRTYPE:
                    packing.add(member.name, "dir", member.mode)
                elif member.type == tarfile.SYMTYPE:
                    packing.add(member.name, "link", member.mode, link=member.linkname)
                else:
                    kind = {
                        tarfile.LNKTYPE: "a hard link",
                        tarfile.CHRTYPE: "a device",
                        tarfile.BLKTYPE: "a device",
                        tarfile.FIFOTYPE: "a FIFO",
                    }.get(member.type, "of a kind the client refuses")
                    raise Dropped(f"the archive holds {shown(clipped(member.name))}, {kind}, which the client refuses")
        while unzipped.read(1 << 20):
            pass
        if not stream.ended():
            raise Dropped("the archive goes on past its one bzip2 stream")
    except Dropped:
        raise
    except Exception:  # whatever bzip2 or tar cannot read is an archive the client would not unpack
        raise Dropped("is not a bzip2-compressed tar this intake can read") from None
    return packing.made()


def written_whole(path: pathlib.Path, content: bytes) -> None:
    """Write `content` to `path` whole or not at all: to a file beside it,
    renamed over it once written."""
    held = None
    try:
        descriptor, name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
        held = pathlib.Path(name)
        with os.fdopen(descriptor, "wb") as file:
            file.write(content)
        held.chmod(0o644)
        os.replace(held, path)
        held = None
    except OSError as failed:
        raise Refused(f"{clipped(path)} could not be written: {clipped(failed.strerror)}") from None
    finally:
        if held is not None:
            held.unlink(missing_ok=True)


def repack_command(arguments: argparse.Namespace) -> int:
    try:
        descriptor = os.open(arguments.input, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as failed:
        raise Refused(f"{clipped(arguments.input)} cannot be read: {clipped(failed.strerror)}") from None
    try:
        with os.fdopen(descriptor, "rb") as file:
            if not stat.S_ISREG(os.fstat(file.fileno()).st_mode):
                raise Dropped("it is not a regular file")
            made = repacked(file)
    except Dropped as failed:
        raise Refused(f"{clipped(arguments.input)} cannot be repacked: {failed}") from None
    written_whole(arguments.out, made)
    return 0


def staged_tree(staged: pathlib.Path) -> dict[str, tuple[str, int]]:
    """Every name under the staged directory `staged`, relative to it and in
    order, with its kind and mode, no link followed: refused where one is
    anything but a file, a directory or a symbolic link, or there are more
    than `TARBALL_MEMBERS`."""
    found: dict[str, tuple[str, int]] = {}

    def walk(directory: pathlib.Path, prefix: str) -> None:
        try:
            names = sorted(os.listdir(directory))
        except OSError:
            raise Refused(f"the staged directory {shown(clipped(prefix or '.'))} cannot be read") from None
        for name in names:
            path = f"{prefix}{name}"
            held = os.lstat(directory / name)
            if len(found) >= TARBALL_MEMBERS:
                raise Refused(f"the staged directory holds more than {TARBALL_MEMBERS} names")
            if stat.S_ISREG(held.st_mode):
                found[path] = ("file", held.st_mode)
            elif stat.S_ISLNK(held.st_mode):
                found[path] = ("link", held.st_mode)
            elif stat.S_ISDIR(held.st_mode):
                found[path] = ("dir", held.st_mode)
                walk(directory / name, f"{path}/")
            else:
                raise Refused(f"the staged {shown(clipped(path))} is a fifo, a device or a socket, not a file, "
                              "a directory or a link")

    walk(staged, "")
    return found


def pack_command(arguments: argparse.Namespace) -> int:
    """A built package directory written as `Packing` writes a tar.gz, the
    one form of a build review accepts: its names in the order `staged_tree`
    walks them, each directory mode 0755 and each file 0755 where any execute
    bit is set, else 0644, so two builds of the same files are the same bytes.
    A link, or a name of any other kind, is refused: a build installs files."""
    root = arguments.root
    try:
        if not stat.S_ISDIR(os.lstat(root).st_mode):
            raise Refused("the built package's root is not a directory")
    except OSError:
        raise Refused("the built package's root is missing") from None
    packing = Packing()
    try:
        for path, (kind, mode) in staged_tree(root).items():
            if kind == "dir":
                packing.add(path, "dir", 0o755)
                continue
            if kind != "file":
                raise Refused(f"the built package holds {shown(clipped(path))}, a link, and a build installs files")
            with opened(root / path) as file:
                size = os.fstat(file.fileno()).st_size
                packing.add(path, "file", 0o755 if mode & 0o111 else 0o644, content=file, size=size)
        made = packing.made()
    except Dropped as dropped:
        raise Refused(f"the built package cannot be packed: {dropped}") from None
    if len(made) > REPACK_BYTES:
        raise Refused(f"the built package packs to more than {REPACK_BYTES} bytes, which review will not read")
    written_whole(arguments.out, made)
    return 0


# One line `ldd` prints under a file it was given: a library it resolves, one
# it does not find, or the virtual object the kernel maps.
LDD_FOUND = re.compile(r"\t([^\s=]+) => (/[^\0]*) \((0x[0-9a-f]+)\)")
LDD_MISSING = re.compile(r"\t([^\s=]+) => not found \(0\)")
LDD_VDSO = re.compile(r"\t\[vdso\] \((0x[0-9a-f]+)\)")


def ldd_needs(text_read: bytes, root: str, tree: dict[str, tuple[str, int]]) -> tuple[set[str], set[str]]:
    """What `ldd` printed, `text_read`, parsed once: each staged file it was
    given, by a line `<staged path>:` with `root` the staged directory's real
    path, and every library each one resolves, into the staged `lib/` or the
    base system's `/lib/` or `/usr/lib/`. Returns the staged files it printed
    for and the staged names its libraries resolve to. A line that does not
    parse, a file given twice or not staged, and a library resolved anywhere
    else or not found are refused."""
    try:
        lines = text_read.decode().split("\n")
    except UnicodeDecodeError:
        raise Refused("what ldd printed is not UTF-8") from None
    if lines[-1] == "":
        lines.pop()
    sections: set[str] = set()
    needed: set[str] = set()
    current = None
    for number_of, line in enumerate(lines, 1):
        if not line.startswith("\t"):
            if not line.endswith(":") or not line.startswith(root + "/"):
                if line.endswith(":") and line.startswith("/"):
                    raise Refused(f"ldd printed for {shown(clipped(line[:-1]))}, which is not a staged file")
                raise Refused(f"what ldd printed does not parse at line {number_of}: {shown(clipped(line))}")
            current = line[len(root) + 1:-1]
            if tree.get(current, ("",))[0] != "file":
                raise Refused(f"ldd printed for {shown(clipped(current))}, which is not a staged file")
            if current in sections:
                raise Refused(f"ldd printed for {shown(clipped(current))} twice")
            sections.add(current)
            continue
        if current is None:
            raise Refused(f"what ldd printed does not parse at line {number_of}: {shown(clipped(line))}")
        if LDD_VDSO.fullmatch(line):
            continue
        missing = LDD_MISSING.fullmatch(line)
        if missing is not None:
            raise Refused(
                f"{shown(clipped(missing[1]))} cannot be bundled: ldd finds it nowhere for {shown(clipped(current))}"
            )
        found = LDD_FOUND.fullmatch(line)
        if found is None:
            raise Refused(f"what ldd printed does not parse at line {number_of}: {shown(clipped(line))}")
        resolved_to = os.path.normpath(found[2])
        if resolved_to.startswith(root + "/lib/"):
            rel = resolved_to[len(root) + 1:]
            if "/" not in rel[len("lib/"):] and tree.get(rel, ("",))[0] in ("file", "link"):
                needed.add(rel)
                continue
        elif resolved_to.startswith(("/lib/", "/usr/lib/")) and not resolved_to.startswith(root + "/"):
            continue
        raise Refused(f"{shown(clipped(found[1]))} cannot be bundled: ldd resolves it to {shown(clipped(found[2]))}, "
                      "which is neither a library staged in the bundle's lib/ nor one of the base system's "
                      "/lib/ or /usr/lib/")
    return sections, needed


def bundle_packages(value: object) -> list[dict]:
    """The packages the FreeBSD step fetched, as `pkg query` gave them:
    `{"packages": [{"name", "version", "licenses", "logic", "files"}]}`."""
    if not isinstance(value, dict) or set(value) != {"packages"} or not isinstance(value["packages"], list):
        raise Refused('the package list is not {"packages": [...]}')
    if not value["packages"] or len(value["packages"]) > ITEMS:
        raise Refused(f"the package list holds no package, or more than {ITEMS}")
    held = []
    names: set[str] = set()
    files_listed = 0
    for index, item in enumerate(value["packages"]):
        where = f"package {index}"
        if not isinstance(item, dict):
            raise Refused(f"{where} is not an object")
        extra = sorted(set(item) - {"name", "version", "licenses", "logic", "files"})
        if extra:
            raise Refused(f"{where} holds {named(extra)}, which no package holds")
        name = item.get("name")
        if not isinstance(name, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,127}", name):
            raise Refused(f"{where}'s name is not a package's name")
        where = f"package {name}"
        if name in names:
            raise Refused(f"the package list lists {where} twice")
        names.add(name)
        given = item.get("version")
        if not isinstance(given, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._,+-]{0,63}", given):
            raise Refused(f"{where}'s version is not a package's version")
        licences = item.get("licenses")
        if not isinstance(licences, list) or len(licences) > ITEMS or not all(
            isinstance(licence, str) and re.fullmatch(r"[A-Za-z0-9._+-]{1,64}", licence) for licence in licences
        ):
            raise Refused(f"{where}'s licenses are not a list of at most {ITEMS} licence names")
        logic = item.get("logic")
        if logic not in ("single", "and", "or"):
            raise Refused(f"{where}'s licence logic is not single, and or or")
        if logic == "single" and len(licences) > 1:
            raise Refused(f"{where} gives {len(licences)} licences under single logic, which takes one")
        files = item.get("files")
        if not isinstance(files, list):
            raise Refused(f"{where}'s files are not a list")
        files_listed += len(files)
        if files_listed > LISTED_ENTRIES:
            raise Refused(f"the package list lists more than {LISTED_ENTRIES} files")
        for path in files:
            if not isinstance(path, str) or not path.startswith("/") or "\0" in path or len(path) > TEXT_BYTES:
                raise Refused(f"{where} lists {shown(clipped(path))}, which is not an absolute path")
        held.append({"files": files, "licences": licences, "logic": logic, "name": name, "version": given})
    return held


def ports_expression(licences: list[str], logic: str) -> str:
    """The SPDX expression of a package's ports licences under `logic`, each
    through `PORTS_LICENCES`; one the table does not hold, and none at all,
    is `NOASSERTION`, which the licence gate refuses."""
    if not licences:
        return "NOASSERTION"
    joined = " OR " if logic == "or" else " AND "
    return joined.join(PORTS_LICENCES.get(licence, "NOASSERTION") for licence in licences)


def bundle_command(arguments: argparse.Namespace) -> int:
    """The FreeBSD step's staged runtime checked and written as a tar.gz and
    an SBOM, both whole or neither; see the module documentation."""
    abi = arguments.abi
    if not FREEBSD_ABI.fullmatch(abi):
        raise Refused(f"the ABI {shown(clipped(abi))} is not a FreeBSD ABI on amd64, FreeBSD:<major>:amd64")
    staged = arguments.staged
    root = os.path.realpath(staged)
    tree = staged_tree(staged)
    for path, (kind, _) in tree.items():
        top, _, below = path.partition("/")
        if top not in ("bin", "lib", "licenses") or (not below and kind != "dir"):
            raise Refused(f"the staged directory holds {shown(clipped(path))}, which is not under bin/, lib/ or licenses/")
    programs = [path for path in tree if path.startswith("bin/")]
    if len(programs) != 1 or tree[programs[0]][0] != "file":
        raise Refused(f"bin/ holds {len(programs)} names, and a staged runtime is one program, a file")
    packages = bundle_packages(read_json(arguments.packages, PACKAGES_BYTES, "the package list"))
    names = {package["name"] for package in packages}
    # What each listed file installs in the bundle: its path under the one
    # prefix packages install to, so `/usr/local/lib/libfoo.so.1` installs
    # `lib/libfoo.so.1` and a path anywhere else installs nothing.
    installed = {
        path.removeprefix(PKG_PREFIX)
        for package in packages
        for path in package["files"]
        if path.startswith(PKG_PREFIX)
    }
    python = None
    for path, (kind, _) in tree.items():
        parts = path.split("/")
        if parts[0] == "licenses":
            if len(parts) >= 2 and parts[1] not in names:
                raise Refused(f"{shown(clipped('/'.join(parts[:2])))} names no listed package")
            if len(parts) == 2 and kind != "dir":
                raise Refused(f"{shown(clipped(path))} is not a package's directory of licence files")
        elif parts[0] == "lib" and len(parts) > 2 or (parts[0] == "lib" and kind == "dir" and len(parts) == 2):
            if not PYTHON.fullmatch(parts[1].removeprefix("python")) or not parts[1].startswith("python"):
                raise Refused(f"{shown(clipped(path))} is neither a library in lib/ nor under CPython's lib/python3.M/")
            if python not in (None, parts[1]):
                raise Refused("lib/ holds the standard library of two CPythons")
            python = parts[1]
    printed = read(arguments.ldd, LDD_BYTES, "what ldd printed")
    sections, needed = ldd_needs(printed, root, tree)
    # A staged library a link names is needed where the link is.
    for path in sorted(needed):
        current = path
        for _ in range(LINK_STEPS):
            if tree.get(current, ("",))[0] != "link":
                break
            target = os.path.normpath(os.path.join(os.path.dirname(current), os.readlink(staged / current)))
            if target.startswith("lib/") and target in tree:
                needed.add(target)
            current = target
    for path, (kind, _) in tree.items():
        parts = path.split("/")
        if parts[0] == "lib" and len(parts) == 2 and kind != "dir" and path not in needed:
            raise Refused(f"no ldd line needs {shown(clipped(path))}, so it is not bundled")
    loaded = [programs[0]] + [
        path for path, (kind, _) in tree.items()
        if kind == "file" and path.startswith("lib/") and (path.count("/") == 1 or path.endswith(".so"))
    ]
    for path in loaded:
        if path not in sections:
            raise Refused(f"ldd printed nothing for {shown(clipped(path))}, so what it loads is not known")
    for path, (kind, _) in tree.items():
        if path.startswith(("bin/", "lib/")) and kind != "dir" and path not in installed:
            raise Refused(f"no listed package installs {shown(clipped(path))}")
    try:
        packing = Packing()
        for path, (kind, mode) in tree.items():
            if kind == "file":
                with opened(staged / path) as file:
                    size = os.fstat(file.fileno()).st_size
                    packing.add(path, "file", mode, content=file, size=size)
            elif kind == "link":
                packing.add(path, "link", mode, link=os.readlink(staged / path))
            else:
                packing.add(path, "dir", mode)
        made = packing.made()
    except Dropped as failed:
        raise Refused(f"the staged runtime cannot be bundled: {failed}") from None
    except OSError as failed:
        raise Refused(f"the staged runtime cannot be read: {clipped(failed.strerror)}") from None
    sbom = {
        "bomFormat": "CycloneDX",
        "components": [
            {
                "licenses": [{"expression": ports_expression(package["licences"], package["logic"])}],
                "name": package["name"],
                "type": "library",
                "version": package["version"],
            }
            for package in sorted(packages, key=lambda package: package["name"])
        ],
        "metadata": {"properties": [{"name": "freebsd:abi", "value": abi}]},
        "specVersion": "1.6",
        "version": 1,
    }
    written_whole(arguments.out, made)
    try:
        written_whole(arguments.sbom, json.dumps(sbom, indent=2, sort_keys=True).encode() + b"\n")
    except Refused:
        arguments.out.unlink(missing_ok=True)
        raise
    return 0


def review(arguments: argparse.Namespace) -> int:
    global BUDGET
    BUDGET = None
    text_read = read(arguments.registry, REGISTRY_BYTES, "the registry")
    registry = parsed(text_read, "the registry")
    agents = registry.get("agents") if isinstance(registry, dict) else None
    if not isinstance(agents, list) or len(agents) > AGENTS:
        raise Refused(f"the registry lists no agents, or more than {AGENTS}")
    choices = choices_of(read_json(arguments.choices, SMALL_BYTES, "the choices"))
    runners = runners_of(read_json(arguments.runners, SMALL_BYTES, "the runner matrix"))
    allowed = allowed_licences(arguments.deny)
    gate = Gate(allowed, arguments.sboms, arguments.texts)
    merged = merged_of(arguments.merged)
    interpreters = interpreters_of(arguments.interpreters)
    pinned_node, freebsd_node = node_of(arguments.node)
    gpg = keys = None
    if pinned_node is not None:
        gpg = shutil.which("gpg")
        if gpg is None:
            raise Refused("gpg is not on PATH, so Node's signed checksums cannot be verified")
        keys = release_keys(arguments.node_keys or pathlib.Path(__file__).resolve().parent / "node-release-keys")
    dispositions = dispositions_of(arguments.dispositions)
    BUDGET = Budget()
    node = node_release(arguments.observed, pinned_node, gpg, keys, freebsd_node, gate)
    entries = []
    dropped: list[dict] = []
    assets: dict[str, bytes] = {}
    listed: set[str] = set()
    # Each top-level name of the observed tree, as a file system that folds
    # case names it: Node's own directory, and one directory per registry id.
    folded: dict[str, int] = {"node": 1}
    for agent in {agent.get("id") for agent in agents if isinstance(agent, dict) and isinstance(agent.get("id"), str)}:
        folded[agent.casefold()] = folded.get(agent.casefold(), 0) + 1
    for index, agent in enumerate(agents):
        agent_id = agent.get("id") if isinstance(agent, dict) else None
        if not isinstance(agent_id, str) or not REGISTRY_ID.fullmatch(agent_id):
            shown_id = agent_id if isinstance(agent_id, str) else f"agents[{index}]"
            dropped.append({"reason": "its registry id cannot name a directory", "registry_id": clipped(shown_id)})
            continue
        if agent_id in listed:
            dropped.append({"reason": "the registry lists it twice", "registry_id": agent_id})
            continue
        listed.add(agent_id)
        if folded[agent_id.casefold()] > 1:
            # The job leaves Node's own downloads under `node/` beside each
            # agent's directory, and a folding file system takes two names
            # that differ in case alone for one.
            dropped.append({
                "reason": "its registry id names the same observed directory as node, where Node's downloads "
                "are left, or another agent's, once case is folded",
                "registry_id": agent_id,
            })
            continue
        choice = choices.get(agent_id)
        if choice is None:
            dropped.append({"reason": "has no key chosen by a reviewer yet", "registry_id": agent_id})
            continue
        try:
            made, kept = entry(
                arguments.observed, agent, agent_id, choice, runners, allowed, gate, interpreters, node,
                dispositions.get(agent_id, {}), dropped,
            )
        except Exception as failed:  # whatever one entry breaks, the others go on
            dropped.append({"reason": reason(failed), "registry_id": agent_id})
            continue
        held = merged.get((made["key"], made["version"]))
        new, old = made, held
        if made["kind"] == "npm":
            # A version reviewed before may differ from the merged one in its
            # locks alone, which give its lock revision; any other difference
            # meets the refusal below.
            made["lock_revision"] = lock_revision(made, held)
            new, old = unlocked(made), None if held is None else unlocked(held)
        # The record made now stands; `diff` shows where it changed.
        changed = differences(ungated(new), ungated(old)) if old is not None else []
        if changed:
            dropped.append(
                {
                    "reason": f"version {clipped(made['version'])} was already reviewed with other "
                    f"content, and {named(changed)} differ; a changed release is taken in only "
                    "under a new version",
                    "registry_id": agent_id,
                }
            )
            continue
        entries.append(made)
        assets.update(kept)
    BUDGET = None
    # Each script a kept npm entry skipped that no reviewer has decided yet.
    undecided = sorted(
        {
            (made["registry_id"], script["package"], script["event"])
            for made in entries
            for listed_target in made["targets"]
            for script in listed_target.get("scripts", [])
            if script["disposition"] is None
        }
    )
    data = {
        "dropped": sorted(dropped, key=lambda item: (item["registry_id"], item.get("platform", ""))),
        "entries": sorted(entries, key=lambda item: item["key"]),
        "format": FORMAT,
        "registry": {"sha256": hashlib.sha256(text_read).hexdigest(), "size": len(text_read)},
        "runners": runners,
        "undecided": [
            {"event": event, "package": package, "registry_id": agent_id} for agent_id, package, event in undecided
        ],
    }
    held_data = (json.dumps(data, indent=2, sort_keys=True) + "\n").encode()
    if len(held_data) > DATA_BYTES:
        raise Refused(f"the review data would be larger than {DATA_BYTES} bytes, which diff reads")
    arguments.assets.mkdir(parents=True, exist_ok=True)
    for name, listing in sorted(assets.items()):
        (arguments.assets / name).write_bytes(listing)
    written = arguments.out.with_name(arguments.out.name + ".partial")
    written.write_bytes(held_data)
    written.replace(arguments.out)
    return 0


def whole(value: object, what: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= WHOLE:
        raise Refused(f"{what} is not a whole number from 0 to {WHOLE}")
    return value


def revocations(value: object, what: str) -> list[dict]:
    """The revocations `value` lists, each checked as the catalog's reader will."""
    if not isinstance(value, list) or len(value) > REVOKED:
        raise Refused(f"{what} does not list its revocations, or lists more than {REVOKED}")
    held = []
    for item in value:
        item = obj(item, "a revocation")
        key = item.get("key")
        if not isinstance(key, str) or not AGENT_KEY.fullmatch(key):
            raise Refused(f"a revocation in {what} names {clipped(key)}, which is not an agent key")
        try:
            made = {
                "key": key,
                "reason": text(item.get("reason"), "a revocation's reason"),
                "version": version(item.get("version"), "a revocation's version"),
            }
        except Dropped as dropped:
            raise Refused(f"a revocation in {what}: {dropped}") from None
        if item.get("lock_revision") is not None:
            made["lock_revision"] = whole(item["lock_revision"], "a revocation's lock revision")
        held.append(made)
    return held


def whole_numbers(value: object, what: str) -> None:
    """Refuse a float anywhere in `value`: a catalog's numbers are whole."""
    if isinstance(value, float) or (isinstance(value, int) and not 0 <= value <= WHOLE):
        raise Refused(f"{what} holds a number that is not whole, or is outside 0 to {WHOLE}")
    for inner in value.values() if isinstance(value, dict) else value if isinstance(value, list) else ():
        whole_numbers(inner, what)


def present(holder: dict, name: str, kind: type, where: str) -> object:
    held = holder.get(name)
    if not isinstance(held, kind) or isinstance(held, bool):
        raise Refused(f"{where}: {name} is missing or is not a {kind.__name__}")
    return held


def checked(entry: dict) -> None:
    """Refuse an entry the client's reader would refuse for a field a renderer
    can see is missing or malformed, so no catalog is written that every client
    would turn away. The reader still decides the rest."""
    key = entry.get("key")
    if not isinstance(key, str) or not AGENT_KEY.fullmatch(key):
        raise Refused(f"an entry's key {clipped(key)} is not an agent key: 1 to 32 lowercase letters, digits and hyphens")
    where = f"entry {key}"
    try:
        version(entry.get("version"), "the version")
    except Dropped as dropped:
        raise Refused(f"{where}: {dropped}") from None
    for name in ("kind", "registry_id", "license", "review"):
        if not present(entry, name, str, where):
            raise Refused(f"{where}: {name} is empty")
    admits = present(entry, "admits", dict, where)
    for name in ("name", "version"):
        present(admits, name, str, f"{where}: admits")
    for name in ("reach", "sign_in_hosts", "state_paths", "auth_methods"):
        present(entry, name, list, where)
    targets = present(entry, "targets", list, where)
    if not targets:
        raise Refused(f"{where}: targets is empty, so nothing could be offered")
    for target in targets:
        if not isinstance(target, dict):
            raise Refused(f"{where}: a target is not an object")
        for name in ("platform", "cmd"):
            present(target, name, str, f"{where}: a target")
        present(target, "hosts", list, f"{where}: a target")
        if entry["kind"] == "uvx":
            started = present(target, "interpreter_args", list, f"{where}: a target")
            if not all(isinstance(item, str) for item in started):
                raise Refused(f"{where}: a target's interpreter_args is not a list of text")
            module, colon, attr = present(target, "entry_point", str, f"{where}: a target").partition(":")
            if not colon or not module or not attr:
                raise Refused(f"{where}: a target's entry_point is not module:attr")
        if entry["kind"] == "npm":
            npm_checked(entry, target, where)
        artifacts = present(target, "artifacts", list, f"{where}: a target")
        if not artifacts:
            raise Refused(f"{where}: a target's artifacts is empty, so nothing would be pinned")
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                raise Refused(f"{where}: an artifact is not an object")
            for name in ("url", "sha256", "format"):
                present(artifact, name, str, f"{where}: an artifact")
            for name in ("size", "unpacked", "decompressed"):
                present(artifact, name, int, f"{where}: an artifact")


def npm_checked(entry: dict, target: dict, where: str) -> None:
    """Refuse an npm target the client could not start, or one carrying a
    script no reviewer decided."""
    present(entry, "lock_revision", int, where)
    started = present(target, "node_args", list, f"{where}: a target")
    if not all(isinstance(item, str) for item in started):
        raise Refused(f"{where}: a target's node_args is not a list of text")
    try:
        relative(present(target, "bin", str, f"{where}: a target"), "bin")
    except Dropped as dropped:
        raise Refused(f"{where}: a target's {dropped}") from None
    lock = present(target, "lock", dict, f"{where}: a target")
    if lock.get("unpack_rules") != UNPACK_RULES or isinstance(lock.get("unpack_rules"), bool):
        raise Refused(f"{where}: a target's lock is not unpacked by unpack_rules {UNPACK_RULES}, the rules a client has")
    present(lock, "sha256", str, f"{where}: a target's lock")
    present(lock, "size", int, f"{where}: a target's lock")
    for script in present(target, "scripts", list, f"{where}: a target"):
        if not isinstance(script, dict) or script.get("disposition") not in DISPOSITIONS + CLASSED:
            shown_script = script.get("disposition") if isinstance(script, dict) else script
            raise Refused(
                f"{where}: a script's disposition is {clipped(shown_script)}, and a script is "
                f"published only once a reviewer decides it is one of {', '.join(DISPOSITIONS)}, "
                f"or review classes it {' or '.join(CLASSED)}"
            )
        for name in ("package", "event"):
            present(script, name, str, f"{where}: a script")
        if script["disposition"] == "local":
            # What the client shows before it runs a script on the user's machine.
            try:
                command = text(present(script, "command", str, f"{where}: a local script"), "a local script's command")
                if any(hidden(character) for character in command):
                    raise Dropped("a local script's command holds a character that could hide what it runs")
                hosts = present(script, "hosts", list, f"{where}: a local script")
                if len(hosts) > ITEMS or not all(isinstance(name, str) and host(name) for name in hosts):
                    raise Dropped(f"a local script's hosts are not a list of at most {ITEMS} hosts")
                result = present(script, "result", dict, f"{where}: a local script")
                digest(result.get("sha256"), "a local script's result sha256")
                if number(result.get("size"), "a local script's result size") > ASSET_BYTES:
                    raise Dropped(f"a local script's result is larger than a catalog asset may be, {ASSET_BYTES} bytes")
            except Dropped as dropped:
                raise Refused(f"{where}: {dropped}") from None


def rendered(entries: list[dict]) -> list[dict]:
    """The entries as the catalog lists them: what only a reviewer reads is left out."""
    listed = []
    for entry in entries:
        copy = dict(entry)
        copy["targets"] = [
            {name: value for name, value in target.items() if name != "probe"}
            for target in copy.get("targets", [])
        ]
        for target in copy["targets"]:
            target["artifacts"] = [
                {name: value for name, value in artifact.items() if name not in ("mirror_licence", "licence_gate")}
                for artifact in target.get("artifacts", [])
            ]
            if "scripts" in target:
                # A built script's record and output are the publish job's;
                # the catalog lists the script as built.
                target["scripts"] = [
                    {name: value for name, value in script.items() if name not in ("licence_gate", "replaced_by")}
                    for script in target["scripts"]
                ]
        listed.append(copy)
    return listed


def render(arguments: argparse.Namespace) -> int:
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    entries = merged.get("entries") if isinstance(merged, dict) else None
    if not isinstance(entries, list) or not all(isinstance(item, dict) for item in entries):
        raise Refused("the merged data is not review data as the intake writes it")
    try:
        entries = rendered(entries)
    except (AttributeError, TypeError):
        raise Refused("the merged data is not review data as the intake writes it") from None
    whole_numbers(entries, "the merged data")
    for item in entries:
        checked(item)
    names = [(entry.get("key"), entry.get("version")) for entry in entries]
    if len(set(map(str, names))) != len(names):
        raise Refused("the merged data holds one version of an agent twice")
    revoked = revocations(merged.get("revoked", []), "the merged data")
    serial = 0
    signed_at = whole(arguments.signed_at, "--signed-at")
    if arguments.previous is not None:
        before = obj(read_json(arguments.previous, INDEX_BYTES, "the previous catalog"), "the previous catalog")
        if before.get("schema") != SCHEMA or isinstance(before.get("schema"), bool):
            raise Refused(f"the previous catalog is not in schema {SCHEMA}, the one this renders")
        serial = whole(before.get("serial"), "the previous catalog's serial")
        if serial < 1:
            raise Refused("the previous catalog's serial is below 1, the lowest a catalog has")
        if signed_at < whole(before.get("signed_at"), "the previous catalog's signed_at"):
            raise Refused("--signed-at is earlier than the previous catalog's signed_at")
        if "revoked" not in before:
            raise Refused("the previous catalog lists no revocations, so earlier ones could be dropped unseen")
        revoked = revocations(before["revoked"], "the previous catalog") + revoked
    seen: set[tuple] = set()
    accumulated = []
    for item in revoked:
        identity = (item["key"], item["version"], item.get("lock_revision"))
        if identity not in seen:
            seen.add(identity)
            accumulated.append(item)
    if len(accumulated) > REVOKED:
        raise Refused(f"the catalog would carry {len(accumulated)} revocations, more than the {REVOKED} a client keeps")
    catalog = {
        "entries": entries,
        "expires": whole(signed_at + LIFETIME_SECONDS, "the expiry"),
        "revoked": sorted(accumulated, key=lambda item: (item["key"], item["version"], item.get("lock_revision", -1))),
        "schema": SCHEMA,
        "serial": whole(serial + 1, "the serial"),
        "signed_at": signed_at,
    }
    held = json.dumps(catalog, sort_keys=True, separators=(",", ":"), ensure_ascii=False, allow_nan=False)
    try:
        held_bytes = held.encode()
    except UnicodeEncodeError:
        raise Refused("the catalog holds text UTF-8 cannot carry") from None
    if len(held_bytes) > INDEX_BYTES:
        raise Refused(f"the catalog would be {len(held_bytes)} bytes, more than the {INDEX_BYTES} a client reads")
    written = arguments.out.with_name(arguments.out.name + ".partial")
    try:
        written.write_bytes(held_bytes)
        written.replace(arguments.out)
    except OSError as failed:
        written.unlink(missing_ok=True)
        raise Refused(f"the catalog could not be written to {clipped(arguments.out)}: {clipped(failed.strerror)}") from None
    return 0


def passing(record: object) -> dict | None:
    """`record` where it is a passing licence-gate record in the shape the
    gate writes, binding an asset, a NOTICE and each licence text; else none."""
    if not isinstance(record, dict) or record.get("passed") is not True:
        return None
    try:
        for bound in [record.get("asset"), record.get("notice"), *record.get("licences")]:
            bound = obj(bound, "a bound file")
            digest(bound.get("sha256"), "its sha256")
            number(bound.get("size"), "its size")
        for licence in record["licences"]:
            text(licence.get("id"), "a licence id")
        number(record.get("components"), "its components")
    except (Dropped, TypeError):
        return None
    return record


def published(merged: dict) -> tuple[dict[str, int], dict[str, int | None], dict[str, dict[str, str]]]:
    """What the merged data lets the catalog release hold, by hash: each
    file-list and lock asset, and each NOTICE and licence text a passing
    licence-gate record gives, and each local script's listing, with its
    size; each artifact copy or replacement, a built script's output among
    them, with the size its record binds, or none where it may not be
    redistributed (no passing record for those very bytes on every entry that
    holds them, or, for a copy, no mirror mark); and, for each of those, the
    NOTICE and licence texts that must ship beside it, each by its hash and
    what it is."""
    listings: dict[str, int] = {}
    redistributed: dict[str, int | None] = {}
    beside: dict[str, dict[str, str]] = {}

    def allow(sha: str, size: int | None, record: dict | None) -> None:
        # The same bytes under two entries pass only where both do, and ship
        # beside what each record names.
        redistributed[sha] = size if redistributed.get(sha, size) == size else None
        if record is not None:
            held = beside.setdefault(sha, {})
            held[record["notice"]["sha256"]] = "the NOTICE"
            held.update({licence["sha256"]: "a licence text" for licence in record["licences"]})

    try:
        for entry in merged["entries"]:
            for target in entry.get("targets", []):
                for artifact in target.get("artifacts", []):
                    record = passing(artifact.get("licence_gate"))
                    gated = None if record is None else record["asset"]
                    sha = digest(artifact.get("sha256"), "an artifact's sha256")
                    copied = (
                        gated is not None
                        and gated["sha256"] == sha
                        and isinstance(artifact.get("mirror_licence"), str)
                    )
                    allow(sha, gated["size"] if copied else None, record if copied else None)
                    replacement = artifact.get("replaced_by")
                    if replacement is not None:
                        replacement = obj(replacement, "an artifact's replaced_by")
                        replaced_sha = digest(replacement.get("sha256"), "a replacement's sha256")
                        replaced_size = number(replacement.get("size"), "a replacement's size")
                        held = gated is not None and gated["sha256"] == replaced_sha and gated["size"] == replaced_size
                        allow(replaced_sha, replaced_size if held else None, record if held else None)
                    if record is not None:
                        for shipped in [record["notice"], *record["licences"]]:
                            listings[shipped["sha256"]] = shipped["size"]
                    files = artifact.get("files")
                    if isinstance(files, dict):
                        listings[digest(files.get("sha256"), "a file list's sha256")] = number(
                            files.get("size"), "a file list's size"
                        )
                lock = target.get("lock")
                if lock is not None:
                    # An npm target's lock is an asset as a file list is.
                    listings[digest(lock.get("sha256"), "a lock's sha256")] = number(lock.get("size"), "a lock's size")
                for script in target.get("scripts", []):
                    result = script.get("result")
                    if result is not None:
                        # A local script's listing is the catalog's own data.
                        listings[digest(result.get("sha256"), "a result's sha256")] = number(
                            result.get("size"), "a result's size"
                        )
                    record = passing(script.get("licence_gate"))
                    replacement = script.get("replaced_by")
                    if replacement is not None:
                        # A built script's output is a redistribution, as a repack is.
                        replacement = obj(replacement, "a script's replaced_by")
                        replaced_sha = digest(replacement.get("sha256"), "a replacement's sha256")
                        replaced_size = number(replacement.get("size"), "a replacement's size")
                        gated = None if record is None else record["asset"]
                        held = gated is not None and gated["sha256"] == replaced_sha and gated["size"] == replaced_size
                        allow(replaced_sha, replaced_size if held else None, record if held else None)
                    if record is not None:
                        for shipped in [record["notice"], *record["licences"]]:
                            listings[shipped["sha256"]] = shipped["size"]
    except (KeyError, TypeError, AttributeError, Dropped):
        raise Refused("the merged data is not review data as the intake writes it") from None
    return listings, redistributed, beside


def publish_check(arguments: argparse.Namespace) -> int:
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    if not isinstance(merged, dict):
        raise Refused("the merged data is not review data as the intake writes it")
    listings, redistributed, beside = published(merged)
    try:
        held = os.lstat(arguments.assets)
    except OSError:
        raise Refused("the assets directory is missing") from None
    if not stat.S_ISDIR(held.st_mode):
        raise Refused("the assets are not a directory")
    for place in sorted(arguments.assets.iterdir()):
        name = clipped(place.name)
        if not SHA256.fullmatch(place.name):
            raise Refused(f"asset {name} is not named by a lower-case SHA-256 with no extension")
        try:
            actual, size = hashed(place)
        except Dropped as failed:
            raise Refused(f"asset {name} {failed}") from None
        if actual != place.name:
            raise Refused(f"asset {name} is not the bytes its name claims")
        if place.name not in listings and place.name not in redistributed:
            raise Refused(f"asset {name} is not in the merged data")
        # Independent: a hash the data names both ways answers to both.
        if place.name in listings and size != listings[place.name]:
            raise Refused(f"asset {name} is not the size the merged data records for it")
        if place.name in redistributed:
            if redistributed[place.name] is None:
                raise Refused(
                    f"asset {name} copies or replaces an artifact with no passing licence-gate record for "
                    "these bytes on every entry holding them, or a copy whose artifact lacks a mirror mark"
                )
            if size != redistributed[place.name]:
                raise Refused(f"asset {name} is not the size its licence-gate record binds")
    uploaded = {place.name for place in arguments.assets.iterdir()}
    for sha in sorted(uploaded & set(redistributed)):
        missing = sorted(what for held, what in beside.get(sha, {}).items() if held not in uploaded)
        if missing:
            raise Refused(
                f"asset {sha} would ship without {missing[-1]} its licence-gate record gives beside it"
            )
    return 0


# Characters that draw nothing, or move or break the line, beyond what the
# Unicode categories below already name: variation selectors, the combining
# grapheme joiner, and letters that draw as blanks.
INVISIBLE = frozenset(
    [0x034F, 0x115F, 0x1160, 0x2800, 0x3164, 0xFFA0]
    + list(range(0xFE00, 0xFE10))
    + list(range(0xE0100, 0xE01F0))
)


def hidden(character: str) -> bool:
    """Whether `character` could hide or reorder what the diff shows: any
    control, format (bidi and zero-width ones among them), surrogate, private,
    unassigned, line or paragraph separator, or space other than the ASCII one."""
    category = unicodedata.category(character)
    return (
        category in ("Cc", "Cf", "Cs", "Co", "Cn", "Zl", "Zp")
        or (category == "Zs" and character != " ")
        or ord(character) in INVISIBLE
    )


def shown(value: object) -> str:
    """Hostile `value`, whole, as a code span it cannot close, on one line.

    No mention, link or markup in it is read as one, and every character that
    could hide or reorder it is written as its code point, as is a backslash,
    so the escapes cannot be imitated."""
    held = "".join(
        "\\\\"
        if c == "\\"
        else (f"\\u{ord(c):04x}" if ord(c) <= 0xFFFF else f"\\U{ord(c):08x}")
        if hidden(c)
        else c
        for c in str(value)
    )
    longest = max((len(run) for run in re.findall(r"`+", held)), default=0)
    fence = "`" * (longest + 1)
    return f"{fence} {held} {fence}"


def words(values: object, empty: str = "none") -> str:
    if not isinstance(values, list) or not values:
        return empty if values in (None, [], {}) else shown(values)
    return " ".join(shown(value) for value in values)


def setting(env: object) -> str:
    if not isinstance(env, dict):
        return shown(env)
    return words([f"{name}={value}" for name, value in env.items()])


def files_of(files: object) -> str:
    if isinstance(files, dict):
        return f"a file-list asset {shown(files.get('sha256'))} of {shown(files.get('size'))} bytes"
    return f"{len(files)} listed" if isinstance(files, list) else "none"


def capabilities_of(value: object) -> str:
    return shown(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


# What `compared` is given when there is nothing before to compare with.
NEW = object()


def compared(line: str, new: object, old: object, render) -> list[str]:
    """`line` showing `new` whole, and what it was where it changed; nothing
    where it did not. The values are compared, never what is shown of them."""
    if old is NEW:
        return [f"- {line}: {render(new)}"]
    if differences(new, old) == []:
        return []
    return [f"- {line}: {render(new)} (was {render(old)})"]


def changed_files(new: object, old: object) -> list[str]:
    if not isinstance(new, list) or not isinstance(old, list):
        return []
    before = {str(file.get("path")): file for file in old if isinstance(file, dict)}
    after = {str(file.get("path")): file for file in new if isinstance(file, dict)}
    lines = [f"  - added {shown(path)}" for path in after if path not in before]
    lines += [f"  - removed {shown(path)}" for path in before if path not in after]
    lines += [f"  - changed {shown(path)}" for path in after if path in before and after[path] != before[path]]
    if len(lines) > SHOWN_FILES:
        lines = lines[:SHOWN_FILES] + [f"  - and {len(lines) - SHOWN_FILES} more"]
    return lines


def probe_line(platform: object, record: dict) -> str:
    conformance = shown(record.get("conformance"))
    if record.get("sandbox") == "closed":
        return f"- {shown(platform)} probe: sandbox closed, conformance {conformance}"
    return (
        f"- {shown(platform)} probe: **probed without a sandbox**; the runner matrix lists it "
        f"unconfined: {shown(record.get('reason'))}. The probe said {shown(record.get('sandbox'))}, "
        f"conformance {conformance}; decide whether to admit this target."
    )


def method_line(method: dict) -> str:
    line = f"- sign-in {shown(method.get('id'))}, {shown(method.get('kind'))}: {shown(method.get('name'))}"
    if method.get("kind") == "terminal":
        how = "piped" if method.get("route") == "piped" else "on a real terminal, unconfined"
        if method.get("tty") == "unknown":
            how += " (the agent did not say; piped is the default)"
        line += (
            f"; runs {shown(method.get('command'))} {words(method.get('args', []), '')} "
            f"with {setting(method.get('env', {}))}, {how}"
        )
    return line


ARTIFACT_FIELDS = ("url", "format", "size", "sha256", "unpacked", "decompressed", "mirror_licence")


# The fields a passing licence-gate record holds.
GATE_RECORD_FIELDS = ("asset", "components", "licences", "notice", "passed")


def gate_of(record: object) -> str:
    """A licence-gate record as the reviewer reads it: whether the artifact
    may now be redistributed, and every file the record binds, so two records
    that differ never read the same."""
    if record is None:
        return "none"
    held = passing(record)
    if held is not None and (
        any(set(held[bound]) - {"sha256", "size"} for bound in ("asset", "notice"))
        or any(set(licence) - {"id", "sha256", "size"} for licence in held["licences"])
    ):
        held = None
    if held is not None:
        licences = "; ".join(
            f"{shown(clipped(licence['id']))} {shown(licence['sha256'])} of {shown(licence['size'])} bytes"
            for licence in held["licences"]
        )
        extra = {name: value for name, value in held.items() if name not in GATE_RECORD_FIELDS}
        return (
            f"**passed**, so it may be redistributed: asset {shown(held['asset']['sha256'])} of "
            f"{shown(held['asset']['size'])} bytes, {shown(held['components'])} components, "
            f"NOTICE {shown(held['notice']['sha256'])} of {shown(held['notice']['size'])} bytes, "
            f"licences {licences or 'none'}"
            + (f", and {shown(json.dumps(extra, sort_keys=True))}" if extra else "")
        )
    if isinstance(record, dict) and record.get("passed") is False and set(record) <= {"passed", "reason"}:
        return f"failed: {shown(record.get('reason'))}"
    return f"not a record the gate writes: {shown(json.dumps(record, sort_keys=True))}"


def replaced_of(output: object) -> str:
    """What installs in an artifact's place, as the reviewer reads it."""
    if output is None:
        return "nothing, so the artifact installs as it is"
    if isinstance(output, dict) and set(output) == {"sha256", "size"}:
        return f"{shown(output['sha256'])} of {shown(output['size'])} bytes"
    return f"not an output the gate binds: {shown(json.dumps(output, sort_keys=True))}"


def artifact_lines(artifact: dict, old: dict | None) -> list[str]:
    if old is None:
        lines = [
            f"- artifact {shown(artifact.get('url'))}: {shown(artifact.get('format'))}, "
            f"{shown(artifact.get('size'))} bytes, sha256 {shown(artifact.get('sha256'))}, "
            f"unpacks {shown(artifact.get('unpacked'))} bytes, files {files_of(artifact.get('files'))}"
            + (f", mirror licence {shown(artifact['mirror_licence'])}" if "mirror_licence" in artifact else "")
        ]
        if "licence_gate" in artifact:
            lines.append(f"- artifact licence gate: {gate_of(artifact['licence_gate'])}")
        if "replaced_by" in artifact:
            lines.append(f"- artifact replaced by {replaced_of(artifact['replaced_by'])}")
        return lines
    lines = []
    for field in ARTIFACT_FIELDS:
        lines += compared(f"artifact {field}", artifact.get(field), old.get(field), shown)
    lines += compared("artifact licence gate", artifact.get("licence_gate"), old.get("licence_gate"), gate_of)
    lines += compared("artifact replaced by", artifact.get("replaced_by"), old.get("replaced_by"), replaced_of)
    lines += compared("artifact files", artifact.get("files"), old.get("files"), files_of)
    return lines + changed_files(artifact.get("files"), old.get("files"))


def lock_of(lock: object) -> str:
    if not isinstance(lock, dict):
        return shown(lock)
    return (
        f"a lock asset {shown(lock.get('sha256'))} of {shown(lock.get('size'))} bytes, "
        f"unpack_rules {shown(lock.get('unpack_rules'))}"
    )


def scripts_of(scripts: object) -> list[str]:
    """Each script `--ignore-scripts` skipped, with its disposition, or marked
    for the reviewer where it has none; a local one with the command it runs,
    the hosts it reached and the listing it left, and a built one with its
    licence-gate record and output."""
    if not isinstance(scripts, list):
        return [f"- scripts: {shown(scripts)}"]
    lines = []
    for script in scripts:
        script = script if isinstance(script, dict) else {}
        disposition = script.get("disposition")
        decided = "**undecided**" if disposition is None else shown(disposition)
        if disposition == "local":
            # What the client runs on the user's machine, and what it reaches and leaves.
            decided += (
                f", runs {shown(script.get('command'))}, reaching {shown(json.dumps(script.get('hosts')))}, "
                f"leaving {replaced_of(script.get('result'))}"
            )
        elif disposition == "built":
            decided += (
                f", licence gate {gate_of(script.get('licence_gate'))}, "
                f"replaced by {replaced_of(script.get('replaced_by'))}"
            )
        lines.append(f"- script {shown(script.get('package'))} {shown(script.get('event'))}: {decided}")
    return lines


def entry_lines(entry: dict, previous: dict | None, same: bool) -> list[str]:
    npm = entry.get("kind") == "npm"
    revised = same and npm and previous is not None and entry.get("lock_revision") != previous.get("lock_revision")
    if revised:
        was = " (**new lock revision** of a version already reviewed: its locks differ as below)"
    elif same and differences(ungated(entry), ungated(previous)) == []:
        was = " (same version; its licence-gate record changed as below)"
    elif same:
        was = " (**same version, content changed**: a release already reviewed differs as below)"
    elif previous:
        was = f" (was {shown(previous.get('version'))})"
    else:
        was = " (new agent)"
    lines = [f"## {shown(entry.get('key'))} {shown(entry.get('version'))}{was}", ""]
    before = previous or {}

    def against(field: str) -> object:
        return before.get(field) if previous is not None else NEW

    mirrored = any(
        "mirror_licence" in artifact
        for listed in entry.get("targets", [])
        for artifact in listed.get("artifacts", [])
    )
    lines += compared("registry id", entry.get("registry_id"), against("registry_id"), shown)
    lines += compared("licence", entry.get("license"), against("license"), shown)
    uvx = entry.get("kind") == "uvx"
    # A FreeBSD runtime is the one artifact of a uvx or npm entry that ships:
    # it installs as its bundle, which its licence gate binds.
    bundled = sorted({
        str(listed.get("platform"))
        for listed in entry.get("targets", [])
        for artifact in listed.get("artifacts", [])
        if isinstance(artifact, dict) and "replaced_by" in artifact
    })

    def shipped(runtime: str) -> str:
        if not bundled:
            return ""
        return f", except on {words(bundled)}, where {runtime} installs as its bundle, shipped under its licence gate"

    if uvx:
        lines.append("- not mirrored: an interpreter and its wheels are never mirrored" + shipped("the interpreter"))
    elif npm:
        lines.append("- not mirrored: Node and the npm tree are never mirrored" + shipped("Node"))
        revision = entry.get("lock_revision")
        lines.append(
            f"- lock revision {shown(revision)}"
            + (f" (**new lock revision**, was {shown(previous.get('lock_revision'))})" if revised else "")
        )
    else:
        lines.append("- " + ("marked for mirroring" if mirrored else "not mirrored: its licence is not on deny.toml's list"))
    lines += compared(
        "answers initialize as",
        entry.get("admits"),
        against("admits"),
        lambda admits: f"{shown(admits.get('name'))} {shown(admits.get('version'))}" if isinstance(admits, dict) else shown(admits),
    )
    lines += compared("reach", entry.get("reach", []), against("reach"), words)
    if previous is None or differences(entry.get("auth_methods"), before.get("auth_methods")):
        lines += [method_line(method) for method in entry.get("auth_methods", [])]
        if previous is not None:
            lines.append("- the sign-in methods above changed")
    old_targets = {listed.get("platform"): listed for listed in before.get("targets", [])}
    for listed in entry.get("targets", []):
        platform = listed.get("platform")
        old = old_targets.get(platform)
        if old is not None and differences(listed, old) == []:
            continue
        fresh = previous is not None and old is None
        lines += ["", f"### {shown(platform)}" + (" (new target)" if fresh else ""), ""]
        prior = old if old is not None else {}

        def was_of(field: str) -> object:
            return prior.get(field) if old is not None else NEW

        record = listed.get("probe", {})
        if old is None or differences(record, prior.get("probe")):
            lines.append(probe_line(platform, record))
        lines += compared("capabilities", record.get("capabilities"), prior.get("probe", {}).get("capabilities") if old is not None else NEW, capabilities_of)
        lines += compared("cmd", listed.get("cmd"), was_of("cmd"), shown)
        if uvx:
            lines += compared("interpreter args", listed.get("interpreter_args", []), was_of("interpreter_args"), words)
            lines += compared("entry point", listed.get("entry_point"), was_of("entry_point"), shown)
        if npm:
            lines += compared("node args", listed.get("node_args", []), was_of("node_args"), words)
            lines += compared("bin", listed.get("bin"), was_of("bin"), shown)
            lines += compared("lock", listed.get("lock"), was_of("lock"), lock_of)
            # A script is shown whole every time, so an undecided one is never passed over.
            lines += scripts_of(listed.get("scripts", []))
        lines += compared("args", listed.get("args", []), was_of("args"), words)
        lines += compared("env", listed.get("env", {}), was_of("env"), setting)
        lines += compared("hosts", listed.get("hosts", []), was_of("hosts"), words)
        old_artifacts = prior.get("artifacts", [])
        for index, artifact in enumerate(listed.get("artifacts", [])):
            old_artifact = old_artifacts[index] if index < len(old_artifacts) else None
            lines += artifact_lines(artifact, old_artifact)
    lines.append("")
    return lines


def diff(arguments: argparse.Namespace) -> int:
    new = read_json(arguments.review, DATA_BYTES, "the review data")
    old = read_json(arguments.old, DATA_BYTES, "the merged data") if arguments.old else {"entries": []}
    try:
        merged = [entry for entry in old["entries"] if isinstance(entry, dict)]
        held = {(entry.get("key"), entry.get("version")): entry for entry in merged}
        lines = ["# Agent catalog review", ""]
        registry = new.get("registry", {})
        lines += [
            f"Registry read: sha256 {shown(registry.get('sha256'))}, {shown(registry.get('size'))} bytes.",
            "",
        ]
        for entry in new["entries"]:
            reviewed = held.get((entry.get("key"), entry.get("version")))
            if reviewed is not None:
                if differences(entry, reviewed) == []:
                    continue
                lines += entry_lines(entry, reviewed, same=True)
                continue
            previous = next(
                (old_entry for old_entry in reversed(merged) if old_entry.get("key") == entry.get("key")),
                None,
            )
            lines += entry_lines(entry, previous, same=False)
        lines += ["## Dropped", ""]
        for item in new["dropped"]:
            where = f" on {shown(item['platform'])}" if item.get("platform") else ""
            lines.append(f"- {shown(item.get('registry_id'))}{where}: {shown(item.get('reason'))}")
    except (KeyError, TypeError, AttributeError):
        raise Refused("the review data or the merged data is not review data as the intake writes it") from None
    sys.stdout.buffer.write(("\n".join(lines) + "\n").encode())
    return 0


def lock(arguments: argparse.Namespace) -> int:
    """Write the lock `review` computes for one platform, so the job builds
    and compares the tree a client would unpack."""
    try:
        name, released = npx_package(arguments.package)
        system, _, machine = arguments.platform.partition("-")
        if system not in NPM_OS or machine not in NPM_CPU:
            raise Dropped(f"{clipped(arguments.platform)} is not a platform the catalog lists")
        resolved = arguments.resolved
        resolution = npm_resolved(resolved.parent, resolved.name, name, released)
        held = npm_lock(npm_installed(resolution, arguments.platform))
    except Dropped as dropped:
        raise Refused(str(dropped)) from None
    arguments.out.write_bytes(held)
    return 0


def tree(arguments: argparse.Namespace) -> int:
    """List the regular files under `--root`, as npm's side of the tree
    comparison: by path, each file's SHA-256, size and whether any execute bit
    is set. A link is never followed and never listed; a directory is walked."""
    root = arguments.root
    try:
        held = os.lstat(root)
    except OSError:
        raise Refused("the tree's root is missing") from None
    if not stat.S_ISDIR(held.st_mode):
        raise Refused("the tree's root is not a directory")
    files = []
    total = 0
    waiting = [(root, "")]
    while waiting:
        place, prefix = waiting.pop()
        with os.scandir(place) as listing:
            for found in listing:
                path = prefix + found.name
                if len(files) + len(waiting) > TREE_ENTRIES:
                    raise Refused(f"the tree holds more than {TREE_ENTRIES} entries")
                if found.is_dir(follow_symlinks=False):
                    waiting.append((pathlib.Path(found.path), path + "/"))
                    continue
                if not found.is_file(follow_symlinks=False):
                    continue
                descriptor = os.open(found.path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                with os.fdopen(descriptor, "rb") as file:
                    mode = os.fstat(file.fileno()).st_mode
                    if not stat.S_ISREG(mode):
                        continue
                    sha = hashlib.sha256()
                    size = 0
                    while chunk := file.read(1 << 20):
                        size += len(chunk)
                        total += len(chunk)
                        if total > TREE_BYTES:
                            raise Refused(f"the tree holds more than {TREE_BYTES} bytes")
                        sha.update(chunk)
                files.append({"exec": bool(mode & 0o111), "path": path, "sha256": sha.hexdigest(), "size": size})
    written = compact({"files": sorted(files, key=lambda item: item["path"])})
    if len(written) > TREE_LISTING_BYTES:
        raise Refused(f"the tree's listing is larger than {TREE_LISTING_BYTES} bytes")
    arguments.out.write_bytes(written)
    return 0


def recheck(arguments: argparse.Namespace) -> int:
    """List each tarball the merged npm locks name, once, for the daily job
    to ask the registry for; or, given what it answered (`--results`, the
    status by SHA-512), propose a revocation of each lock revision holding a
    tarball the registry answered 404 or 410 for, which a fresh install could
    no longer fetch. Any other answer proposes nothing, since it may pass.
    The proposals are written as entries of the merged data's `revoked`."""
    merged = read_json(arguments.merged, DATA_BYTES, "the merged data")
    entries = merged.get("entries") if isinstance(merged, dict) else None
    if not isinstance(entries, list):
        raise Refused("the merged data is not review data as the intake writes it")
    tarballs: dict[str, str] = {}
    holders: dict[str, set[tuple[str, str, int]]] = {}
    for listed in entries:
        if not isinstance(listed, dict) or listed.get("kind") != "npm":
            continue
        held = (str(listed.get("key")), str(listed.get("version")), whole(listed.get("lock_revision"), "a lock revision"))
        for listed_target in listed.get("targets", []):
            named = digest(listed_target.get("lock", {}).get("sha256"), "a lock's sha256")
            raw = read(arguments.assets / named, ASSET_BYTES, f"lock asset {named}")
            if hashlib.sha256(raw).hexdigest() != named:
                raise Refused(f"lock asset {named} is not the bytes its name claims")
            packages = parsed(raw, f"lock asset {named}")
            packages = packages.get("packages") if isinstance(packages, dict) else None
            if not isinstance(packages, list):
                raise Refused(f"lock asset {named} lists no packages")
            for package in packages:
                if not isinstance(package, dict) or package.get("in_bundle") is True:
                    continue
                sha512 = package.get("sha512")
                url = package.get("url")
                if not isinstance(sha512, str) or not re.fullmatch(r"[0-9a-f]{128}", sha512) or not isinstance(url, str):
                    raise Refused(f"lock asset {named} lists a tarball with no SHA-512 or address")
                tarballs.setdefault(sha512, url)
                holders.setdefault(sha512, set()).add(held)
    if arguments.results is None:
        listing = [{"sha512": sha512, "url": url} for sha512, url in sorted(tarballs.items(), key=lambda item: item[1])]
        arguments.out.write_bytes(compact({"tarballs": listing}))
        return 0
    results = read_json(arguments.results, SMALL_BYTES, "the results")
    if not isinstance(results, dict):
        raise Refused("the results are not an object of statuses by SHA-512")
    # Every tarball the registry no longer serves, by the lock revision
    # that holds it, so each revision is proposed once however many are gone.
    lost: dict[tuple[str, str, int], list[tuple[str, int]]] = {}
    for sha512, status in results.items():
        if sha512 not in tarballs or isinstance(status, bool) or not isinstance(status, int):
            raise Refused(f"the results give {clipped(sha512)}, which is not a listed tarball with a whole status")
        if status in (404, 410):
            for holder in holders[sha512]:
                lost.setdefault(holder, []).append((tarballs[sha512], status))
    revoked = []
    for (key, released, revision), gone in sorted(lost.items()):
        url, status = min(gone)
        more = f", and {len(gone) - 1} more of its tarballs are gone" if len(gone) > 1 else ""
        # The address is the lock's, which no reviewer wrote, so it is
        # quoted as the diff quotes hostile text.
        reason = f"its tarball {shown(clipped(url))} is answered {status} by the registry{more}"
        revoked.append({"key": key, "lock_revision": revision, "reason": reason, "version": released})
    arguments.out.write_bytes(compact(revoked))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    reviewing = commands.add_parser("review", help="turn registry binary, uvx and npx entries into review data")
    for name in ("registry", "choices", "runners", "observed", "deny", "assets", "out"):
        reviewing.add_argument(f"--{name}", type=pathlib.Path, required=True)
    reviewing.add_argument(
        "--merged", type=pathlib.Path, help="the merged data, against which a version already reviewed is held"
    )
    reviewing.add_argument(
        "--interpreters", type=pathlib.Path, help="the interpreter pinned for each platform a uvx agent runs on"
    )
    reviewing.add_argument("--node", type=pathlib.Path, help="the Node release npm agents run on")
    reviewing.add_argument("--node-keys", type=pathlib.Path, help="the keys a Node release may be signed under")
    reviewing.add_argument("--dispositions", type=pathlib.Path, help="the reviewer's decision on each skipped script")
    reviewing.add_argument(
        "--sboms", type=pathlib.Path, help="each mirror-marked artifact's lockfile or SBOM, as <sha256>.json"
    )
    reviewing.add_argument("--texts", type=pathlib.Path, help="the licence texts, one <id>.txt per listed licence")
    gating = commands.add_parser("licence-gate", help="gate an asset on the licences of its whole input")
    gating.add_argument("--input", type=pathlib.Path, required=True, help="the build's lockfile or SBOM")
    gating.add_argument("--deny", type=pathlib.Path, required=True, help="deny.toml, whose licence list decides")
    gating.add_argument("--texts", type=pathlib.Path, required=True, help="the licence texts, one <id>.txt each")
    gating.add_argument("--asset", type=pathlib.Path, required=True, help="the file that would be published")
    gating.add_argument("--out", type=pathlib.Path, required=True, help="a new directory for NOTICE and LICENSES/")
    differing = commands.add_parser("diff", help="the human diff of review data against merged data")
    differing.add_argument("--old", type=pathlib.Path, help="the merged data before this review")
    differing.add_argument("review", type=pathlib.Path)
    rendering = commands.add_parser("render", help="render merged data to the catalog the publishing job signs")
    rendering.add_argument("--merged", type=pathlib.Path, required=True)
    rendering.add_argument("--previous", type=pathlib.Path, help="the catalog published before this one")
    rendering.add_argument("--signed-at", type=int, required=True, help="the signing time, in whole Unix seconds")
    rendering.add_argument("--out", type=pathlib.Path, required=True)
    checking = commands.add_parser("publish-check", help="refuse an asset the merged data does not allow uploaded")
    checking.add_argument("--merged", type=pathlib.Path, required=True)
    checking.add_argument("--assets", type=pathlib.Path, required=True, help="the directory of assets to upload")
    locking = commands.add_parser("lock", help="write the lock an npm agent's tree is unpacked from on one platform")
    locking.add_argument("--resolved", type=pathlib.Path, required=True, help="the agent's observed directory")
    locking.add_argument("--package", required=True, help="the agent's package, as name@version")
    locking.add_argument("--platform", required=True)
    locking.add_argument("--out", type=pathlib.Path, required=True)
    listing = commands.add_parser("tree", help="list the files of the tree npm installed")
    listing.add_argument("--root", type=pathlib.Path, required=True)
    listing.add_argument("--out", type=pathlib.Path, required=True)
    rechecking = commands.add_parser("recheck", help="list merged npm tarballs, or propose revocations for gone ones")
    rechecking.add_argument("--merged", type=pathlib.Path, required=True)
    rechecking.add_argument("--assets", type=pathlib.Path, required=True, help="the directory holding the lock assets")
    rechecking.add_argument("--results", type=pathlib.Path, help="the registry's status for each tarball, by SHA-512")
    rechecking.add_argument("--out", type=pathlib.Path, required=True)
    repacking = commands.add_parser("repack", help="repack a .tar.bz2 as the tar.gz review makes of it")
    repacking.add_argument("--input", type=pathlib.Path, required=True, help="the .tar.bz2 download")
    repacking.add_argument("--out", type=pathlib.Path, required=True, help="where the tar.gz is written")
    bundling = commands.add_parser("bundle", help="check and pack a staged FreeBSD runtime with its SBOM")
    bundling.add_argument("--staged", type=pathlib.Path, required=True, help="the staged bin/, lib/ and licenses/")
    bundling.add_argument("--ldd", type=pathlib.Path, required=True, help="what ldd printed for the staged files")
    bundling.add_argument("--packages", type=pathlib.Path, required=True, help="the package list pkg query gave")
    bundling.add_argument("--abi", required=True, help="the FreeBSD package ABI, as FreeBSD:<major>:amd64")
    bundling.add_argument("--out", type=pathlib.Path, required=True, help="where the tar.gz is written")
    bundling.add_argument("--sbom", type=pathlib.Path, required=True, help="where the CycloneDX SBOM is written")
    packing = commands.add_parser("pack", help="pack a built package directory as review reads a build")
    packing.add_argument("--root", type=pathlib.Path, required=True, help="the package directory after the build")
    packing.add_argument("--out", type=pathlib.Path, required=True, help="where the tar.gz is written")
    arguments = parser.parse_args()
    commands_run = {
        "review": review, "diff": diff, "render": render, "publish-check": publish_check,
        "lock": lock, "tree": tree, "recheck": recheck, "licence-gate": licence_gate_command,
        "repack": repack_command, "bundle": bundle_command, "pack": pack_command,
    }
    try:
        return commands_run[arguments.command](arguments)
    except Refused as refused:
        print(f"agent-catalog-intake: {refused}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
