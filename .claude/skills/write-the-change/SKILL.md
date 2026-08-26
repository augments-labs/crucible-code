---
name: write-the-change
description: >-
  Write the changelog entry, documentation, commit message or pull request for a
  Crucible change. Use when preparing a change for review or publication.
---

# Write the change

The diff says what moved. Published prose says why a reader should care and what
they must do differently.

## Keep each artifact to its job

- Commit: a subject and at most one short paragraph explaining why.
- Changelog: a bold lead and at most three sentences, written for someone
  deciding whether to upgrade.
- Pull request: answer the template with one short paragraph per section and
  name the test that failed before the change.
- Release note: the changelog entry; do not write a second version.

Long reasoning belongs beside the code or in a focused design document where a
future maintainer will look for it, not in a commit narrative.

## Document what exists

Update user documentation, the first-run README surface, contributor setup and
the changelog in the same change when their subject moved. Describe behaviour
that ships now, not a future roadmap. Read the module documentation above changed
code and update it if the implementation made a sentence false.

Do not put internal planning identifiers or harness paths into shipped comments,
docs, schemas or manifests. The repository gate catches common forms, but it
cannot judge whether the prose is true.

## Keep one reason per pull request

A pull request should take one summary to explain. If it needs two independent
summaries, split it where both pieces build and are useful on their own. A module
whose implementation and proof cannot compile apart remains one change; line
count alone does not decide this.
