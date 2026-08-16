# Tools

Six tools, advertised in the order a model tends to reach for them:

| Tool | What it does | Asks first |
| --- | --- | --- |
| `read` | Reads a file | no |
| `grep` | Searches file contents | no |
| `glob` | Finds files by pattern | no |
| `edit` | Replaces text in a file | yes |
| `write` | Creates or overwrites a file | yes |
| `bash` | Runs a command | yes |

The list is fixed. There is nothing to install and nothing to switch off; what
you configure is which calls get through, and that is
[Permissions](../permissions/index.md).

## Everything but `bash` stays in the working directory

The directory you start crucible in is the workspace root. `read`, `grep`,
`glob`, `edit` and `write` resolve every path against it and refuse one that
leads outside — including by symbolic link, and including a link planted
between the moment the path was checked and the moment the file was opened. A
refused path comes back as a failure the model can correct by sending a
different one. [Reaching outside the working
directory](../permissions/directories.md) is how that boundary is widened, per
session and deliberately.

`bash` is the standing exception and the only one. A shell reaches whatever you
reach, so what bounds it is the question you are asked, which names the command
rather than a directory.

Two files are outside every tool's reach in every mode: `config.json` and
`config.local.json` inside any directory named `.crucible`. [The files no tool
may write](../permissions/permissions.md#the-files-no-tool-may-write) says why
that one cannot be a rule.

## Every answer is bounded, and says when it was cut

What a tool returns goes into the next request to the model whole, so an
unbounded answer is an unbounded bill and a context window spent on a log file
somebody `cat`'d by accident. Every tool here stops at 30000 bytes.

A cut answer says so, in the answer:

```
[more follows: call read again with offset 501]
[showing first 200 matches: narrow the pattern or raise limit]
[stopped at 74 matches: the answer was full at 30000 bytes, narrow the pattern]
[41200 bytes of output cut from the middle]
```

The notes are addressed to the model rather than to you, and they are the
difference between a model that asks for the next page and one that reads a
prefix as the whole file. The two halves of the bound are not the same promise:
`limit` says how many results to look for, and the bytes say how much text comes
back. Where they disagree the bytes win — which is why a note about a full
answer says to narrow the pattern rather than to raise the limit.

## A failed tool is not a failed turn

A tool that cannot do what was asked says so in a sentence, and that sentence
goes back to the model as the call's result. The turn carries on and the model
decides what to do about it: read the file it was told to read first, widen the
text it could not find, correct a path. In the transcript that call is marked
`✗`.

A turn ends on something narrower: a tool that is unusable rather than
unsuccessful. Arguments that are not the shape the tool takes are one. Your `no`
at a [question](../permissions/permissions.md#the-question) is the other — and a
`deny` rule's no is deliberately not, because a rule is standing policy and a
retry hits the same wall without asking you again.

## The arguments arrive from the model

A model writes a tool call after reading files in your project, so the arguments
are as trustworthy as those files are. They are checked accordingly. A `limit`
past a tool's ceiling is clamped to it, and a `timeout` past `bash`'s is refused
outright — the first is a request for more than an answer holds, the second is a
request to let something run for a day. A word outside the set an
argument accepts — `mode`, `sort` — is refused rather than quietly read as the
default, because a call that asked for one thing and silently got another is an
answer nobody can act on. A path is resolved before it is used, and the
resolution is what the workspace check sees.
