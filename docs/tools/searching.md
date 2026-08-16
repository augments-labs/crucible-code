# Searching the tree

Two tools walk the workspace: `grep` searches what is inside files, `glob` finds
files by the shape of their path. Neither asks before it runs.

## What both of them skip

One place decides which files exist, so the two can never disagree — an agent
told a file is not there by one tool and shown it by the other has no way to
tell which answer to believe. Both skip hidden files, anything a `.gitignore` or
`.ignore` excludes, and anything the global git excludes list does. Neither
follows a symbolic link out of the tree.

`.gitignore` is read even where there is no `.git` yet, which is not what `rg`
does by default. A file listed there is one you have already called noise, and a
project that has not been committed yet is still a project: reporting its build
output back to the model spends the turn on something nobody can act on.

A file the walk reaches is named back relative to the workspace root, so the
model can hand it straight to `read` and you can write a rule about it.

## `grep`

| Argument | What it is |
| --- | --- |
| `pattern` | The regular expression to search for, or the exact text if `fixed` is true. Required. |
| `path` | A file or directory to search under. Defaults to the whole workspace. |
| `glob` | Only search files whose path matches this, for example `**/*.rs`. |
| `ignore_case` | Match without regard to case. Defaults to false. |
| `fixed` | Read `pattern` as the exact text to find rather than as an expression. Defaults to false. |
| `mode` | `content` for the matching lines, `files` for their names. Defaults to `content`. |
| `context` | How many lines either side of each match. Defaults to 0, never more than 20. |
| `limit` | How many results. Defaults to 200, never more than 1000. |

The walk and the search are ripgrep's own crates, which is a speed decision
before it is a convenience one: the only way to search a real tree quickly is to
skip what `rg` skips and read what it reads.

`fixed` reads `pattern` as the exact text to find. It is what you want for
anything copied out of a file: `[dependencies]` is a character class to an
expression and matches every line holding one of those letters, and
`unwrap_or(` is not an expression at all. Escaping such a pattern by hand costs
a turn whichever way it goes wrong — a refused call, or an answer about
something else with nothing in it saying so.

`content` is the default answer and gives a line per match, `path:line:text`:

```
src/client.rs:42:    pub fn timeout(&self) -> Duration {
src/server.rs:118:        let timeout = settings.timeout.unwrap_or(DEFAULT);
```

A matching line longer than 400 characters is cut there — a match inside a
minified bundle is worth reporting and the bundle is not worth sending.

`context` asks for the lines around each match, the way `grep -C` does. They
carry dashes where a match carries colons, and the line number is on both, so a
gap between groups is visible in the numbers:

```
src/client.rs-41-    /// How long to wait for the server.
src/client.rs:42:    pub fn timeout(&self) -> Duration {
src/client.rs-43-        self.timeout
```

Two matches close enough to share lines share them: a line is reported once,
whichever match it belongs to. `limit` counts matches and never the lines around
them, so a search asking for three either side comes back with as many matches as
it would without — and a match the limit cut takes its own context with it,
rather than leaving lines standing beside nothing. `files` is a list of names,
which has nowhere to put a line, so it ignores the argument.

`files` answers with the name of every file holding a match, once each:

```
src/client.rs
src/server.rs
```

That is the answer to "where does this live", and it is also the faster
question: the search stops reading a file at its first match rather than
carrying on to the end of it. `limit` counts files in that mode and matching
lines in the other, so the same number means two different sizes of answer.

A search that ran out of room says which bound it hit, and they take different
remedies:

```
[showing first 200 matches: narrow the pattern or raise limit]
[stopped at 74 matches: the answer was full at 30000 bytes, narrow the pattern]
```

A file with a line too long to hold is searched as far as that line and then
named, so the model knows where to go and look itself:

```
[stopped partway through vendor/bundle.min.js: a match below that point is not here]
```

Nothing matching is a failure rather than an empty answer —
`nothing matched TODO` — because a model handed an empty result reads it as a
successful search of nothing.

## `glob`

| Argument | What it is |
| --- | --- |
| `pattern` | The glob to match, for example `**/*.rs` or `src/**/mod.rs`. Required. |
| `path` | A directory to search under. Defaults to the whole workspace. |
| `sort` | `path` for alphabetical, `modified` for most recently changed first. Defaults to `path`. |
| `limit` | How many paths. Defaults to 200, never more than 1000. |

The answer is one path per line, and a walk that matched more says so:
`[41 more: narrow the pattern or raise limit]`.

`sort` decides the order and therefore which paths a `limit` keeps, which is the
part worth knowing. The listing is bounded while the tree is still being walked,
not afterwards — so `sort: "modified"` with a limit of 20 returns the twenty
newest files in the tree, where sorting a capped alphabetical answer afterwards
would return the twenty lowest paths rearranged. `modified` is what finds what a
project has been working on; `path` is what you want the rest of the time, and
it reads no modification times at all, so the second order's cost is paid only
by the calls that ask for it.

A file whose modification time cannot be read — one that vanished mid-walk, one
on a filesystem that keeps no times — sorts last rather than being dropped.
Putting it at the top of an answer about recent work would be the wrong claim;
leaving it out would be a file the model is told does not exist.

## Stopping one

<kbd>Ctrl-C</kbd> during a search or a walk answers with what it had rather than
failing. Half a tree searched is half a tree searched, and the lines it found
are what the turn was spent on — but an answer that stopped early looks exactly
like one that finished, so the difference is written into it:

```
[stopped before the walk finished: a match in a file it did not reach is not here]
[stopped before the walk finished: these are the newest paths it reached, not the newest there are]
```
