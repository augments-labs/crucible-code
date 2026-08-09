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

## Reach is not permission

An extra directory changes what is refused, not what is asked. A write there
is still a write: under `ask` it prompts like any other, and a `deny` rule
reaches it like any other path. Only an absolute pattern can name one —
`deny edit(/home/you/src/shared-lib/**)` — because a file there has no
spelling below the working directory, and `src/**` would honestly mean nothing
there.
