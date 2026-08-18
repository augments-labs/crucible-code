# What a change says about itself

No `paths:`, deliberately. A commit message is not a file, a pull request
description is not a file, and a rule that waits for one to be opened would
never load for the work it governs. These load every session because the
moment they apply is the moment something is being written, which no glob can
predict.

## Verbosity is prohibited

Not discouraged. The limits are numbers so that "is this too long" is answerable
rather than argued:

| | Ceiling |
| --- | --- |
| Commit message | a subject line and **at most one paragraph** saying why |
| Changelog entry | a bold lead and **at most three sentences** |
| Pull request | what `.github/PULL_REQUEST_TEMPLATE.md` asks, a short paragraph per section |
| Release note | the changelog entry; it is generated from it |

The diff already says *what*. The message says why, once.

What none of them carries: the alternatives weighed, the threat model, a list of
what was considered and rejected, a table of measurements, a narration of how the
work went. Every one of those has a reader who went looking for it, and the place
they went looking is the code comment, the docs page or the design — not a commit
nobody finishes.

If something genuinely needs the long version, the long version is a file in the
repository and the message is one line pointing at it.

## Nothing published names another repository

Not the commit message, not the pull request, not the changelog, not a comment.
Reading another project to understand how something works is allowed and is
covered by the rule beside this one; *citing* it in what this repository
publishes is not. What is learned arrives here as this project's own reasoning,
in this project's own words, and stands or falls on that.

## A pull request has one reason to change

What decides whether a change is one pull request or several is whether it takes
one summary to say what it does: a change that needs two is two, however short
each of them turns out to be, and a module that only compiles whole is one
however long.

The line ceiling that used to answer this instead is temporarily off while the
project is this young: CI still counts a diff and prints the number, and sends
nothing back for it. Off rather than gone; `CONTRIBUTING.md` has the rest.
