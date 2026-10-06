### Internal

- **A change writes its changelog entry to a file of its own under
  `changelog.d/`.** No two pull requests edit `CHANGELOG.md` any more; the
  release commit builds the version section from the entries with
  `scripts/python/changelog-entries.py assemble`, and the repository gate
  holds each entry to its shape and `## [Unreleased]` to staying empty.
