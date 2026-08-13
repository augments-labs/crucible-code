# Directories

The file tools reach the directory crucible was started in, and nothing else.
A path outside it — measured after symbolic links are resolved — is refused by
the tool itself, before any question could be asked. `bash` is the standing
exception: a shell reaches whatever you can, which is why what bounds it is
[the question and the rules](rules.md) rather than a boundary on paths.

`permissions.extraDirectories` widens that reach:

```json
{
  "permissions": {
    "extraDirectories": ["/home/you/src/shared-lib"]
  }
}
```

Entries are absolute paths, resolved once at startup the way the working
directory already is. A relative entry is refused with an error naming the
file and the position, because a path in a configuration file is not relative
to anything the file knows. An absolute path also names one machine, which is
why the entry belongs in `.crucible/config.local.json`, the layer that is not
checked in: `/home/you/src/shared-lib` means nothing to anyone else who
clones. Like the rule lists, the lists from every layer concatenate.

The working directory stays the anchor: a relative path in a tool call still
means what it means from there, and `bash` still runs there. An extra
directory is reached by its own name.

## What containment is measured against

A path is resolved once to say what a call is about, and resolved again inside
the tool that acts on it — so a symbolic link planted while a question was on
screen changes the answer rather than slipping past it. The file is then opened
in a way that carries that second check into the call itself. A new file is
created with the flag that makes the operating system refuse a symbolic link at
the last component, so it cannot be created through one. A file that is already
there is opened only after the last component has been confirmed not to be a
link, and the opened file is then asked whether it is the one that answer was
about. `edit` reads and rewrites through a single open file rather than naming
it twice.

What that bounds is crucible. It is not a boundary on the machine, and two
things get past it if something else is writing into your working directory at
the same time. A directory *above* the file — rather than the file itself — can
be replaced with a link between the check and the call, in a window as long as
two system calls; what would close that is resolving a path one step at a time
against a directory already held open, which crucible does not do. And a second
hard name for a file elsewhere is not a link at all, so nothing distinguishes it
from the original and no check made on names reaches it.

Neither takes a privilege beyond writing into that directory, which is the
point: containment answers for what crucible resolves, and a working directory
another local program is rearranging underneath it is outside what a check on
paths can promise.

## Reach is not permission

An extra directory changes what is refused, not what is asked. A write there
is still a write: under `ask` it prompts like any other, and a `deny` rule
reaches it like any other path. Only an absolute pattern can name one —
`deny edit(/home/you/src/shared-lib/**)` — because a file there has no
spelling below the working directory, and `src/**` would honestly mean nothing
there.
