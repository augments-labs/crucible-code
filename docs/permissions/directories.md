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
screen changes the answer rather than slipping past it. What that leaves is a
path with no symbolic link anywhere in it, and on Unix the file is then reached
by walking it rather than by naming it: crucible opens the directory the path
was proved under, then each directory below in turn against the one before it,
refusing any step that has become a link or has stopped being a directory, and
asks for the file itself against the directory holding it. Every step is one
system call inside a directory the step before it already reached, so there is
no gap left between deciding a name is safe and using it. A new file is created
at the end of the same walk, with the flag that makes the operating system
refuse a symbolic link at the last component. `edit` reads and rewrites through
a validated regular-file handle, then prepares the replacement privately under
the proven parent instead of truncating that handle.

`write` does not truncate an existing file. It prepares a private file beside
the destination, preserves the existing mode, flushes the complete contents,
and renames the private file over the destination as one namespace operation.
Unix then flushes the directory. A failure before the rename leaves the
previous file whole; a failure of the final directory flush is reported after
the replacement is already visible. Crucible also checks the destination's
file identity immediately before commit and refuses a concurrent change. That
check and rename are separate system calls, not a compare-and-swap primitive.
`edit` uses the same commit path after reading at most 1 MiB through the opened
file. Its result has the same 1 MiB ceiling, and cancellation is checked between
fixed-size reads and again before the replacement is prepared.

A link you meant is untouched by any of that. A checkout reached through one
works, because the working directory is resolved when crucible starts and the
link is never on the way down; so does a project that links to its own files,
because resolving followed the link and settled containment about where it led.
What the walk refuses is a link that was not there when the path was checked.

Windows opens a file by name, then validates the final path of the resulting
handle before content is read. `write` prepares its file privately and commits
by rename relative to a held, validated parent, so an ancestor changed to a
directory reparse point cannot redirect the commit. Safe relative creation of
a missing directory is unavailable through the Windows boundary used here, so
`write` on Windows requires its parent directory to exist and fails closed
instead of using a full-path fallback. Its handle-relative rename has no
write-through form: the file is flushed before and after rename, but Windows
does not make the same directory-durability promise as Unix.

What that bounds is crucible, on either platform. It is not a boundary on the
machine, and two things get past it on Unix as well. A directory crucible is
walking through can be *moved* out of the working directory and the file below
it goes with it — the file opened is still the file that was checked, and
whoever moved it could already read it. And a second hard name for a file
elsewhere is not a link at all, so nothing distinguishes it from the original
and no check made on names reaches it.

None of those takes a privilege beyond writing into that directory, which is the
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
