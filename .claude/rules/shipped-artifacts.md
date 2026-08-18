---
paths:
  - "docs/**"
  - "README.md"
  - "CHANGELOG.md"
  - "CONTRIBUTING.md"
  - "crates/**/*.rs"
  - "src/**/*.rs"
---

# What ships, and what a change owes it

## No process memory in shipped artifacts

Comments explain the code. No requirement IDs, no design-doc citations, no
references to planning directories. Traceability lives in commit messages and
test names.

`docs/` is shipped — it is published as a website — so this binds every page: one
documents what exists today and never what a later release will add.
`scripts/check.sh` greps the shipped tree for those shapes, because the files
that legitimately hold them sit one directory away.

## A change owes its documentation in the same commit

`docs/` for anything a user meets, `README.md` for the first minute of it,
`CONTRIBUTING.md` and `docs/building/` for what a contributor has to install, the
changelog for anything that ships.

The module doc comment is on that list and is the one most often left behind:
this project states its invariants in the prose at the top of a file, so one the
code has outgrown is a false statement of the invariant sitting where the next
reader goes to learn it. Read the prose above what you changed, and above
whatever now behaves differently because you changed it.
