# Changelog entries

A pull request writes its changelog entry here, in a file of its own, instead
of editing [`CHANGELOG.md`](../CHANGELOG.md): two pull requests open at the
same time then never edit the same lines, and only a release commit writes the
changelog.

An entry is `changelog.d/<name>.md`, where `<name>` is lower-case letters,
digits and hyphens and starts with a letter or a digit, such as the branch's
last part. Its first line is exactly one of these headings, its second line is
empty, and from the third line on come one or more list items, the first
opening with a bold lead:

```markdown
### Fixed

- **A bold lead saying what changed.** At most three sentences, for someone
  deciding whether to upgrade.
```

The headings are `### Added`, `### Changed`, `### Fixed`, `### Removed`,
`### Security`, `### Documentation` and `### Internal`. An entry holds no other
heading and is at most 4096 bytes; this file is the only one here that is not
an entry. `python3 scripts/python/changelog-entries.py check`, which the
repository gate runs, holds every entry to that shape and `## [Unreleased]` to
staying empty.

At release, `python3 scripts/python/changelog-entries.py assemble --version X
--date YYYY-MM-DD` writes the version section from these files, headings in
the order above and entries under one heading in the order of their names, and
deletes them. [`RELEASING.md`](../RELEASING.md) says what is written by hand
after that.
