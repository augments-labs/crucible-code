# Modes

A session runs in one mode, set by `permissions.mode` in
[configuration](../configuration/configuration.md): `ask`, `allowEdits` or
`fullAccess`. Nothing set means `ask`. That is where a session starts rather
than what it stays at — <kbd>Shift-Tab</kbd> steps it while you type.

A mode decides exactly one thing: what happens to a call no
[rule](rules.md) mentions. It is never a way around the engine — every call
takes the same route to running whatever the mode, a `deny` or `ask` rule
holds in every one, and a read runs in every one.

| A call that would… | `ask` | `allowEdits` | `fullAccess` |
| --- | --- | --- | --- |
| read | run | run | run |
| change a file | ask | run | run |
| run a program proved to change nothing outside the workspace | ask | run | run |
| run any other program | ask | ask | run |

`allowEdits` is for the stretch of work where being interrupted per edit costs
more than the edits do: the workspace changes silently, and anything that could
reach past it still asks. `fullAccess` asks about nothing, which makes a `deny`
rule the only thing that can say no there — write those first.

## What `allowEdits` counts as an edit

Creating a directory is the same change to the same tree whether `write` made
it or a shell did, so `allowEdits` runs a command it can prove reaches no
further than an edit would. Proving that takes all of this at once:

- the line is one simple command — no `;`, `&&`, `||` or pipe;
- the program is one of `mkdir`, `rmdir`, `touch`, `rm`, `cp` or `mv`, spelled
  as itself rather than as a path to it;
- every flag is one of that program's that carries no value of its own, so
  `mkdir -p src/net` qualifies and `mkdir -m 755 src/net` does not;
- every remaining word resolves to a path inside the workspace, after symbolic
  links are followed, including one being created;
- and at least one such path is named.

Anything else asks, including a great many commands that are perfectly
harmless. A glob, a `~`, a quoted word: the shell rewrites those before the
program sees them, so the path crucible checked would not be the path that
changed. `mkdir -p src/net/http` where `src/net` does not exist yet asks for
the same reason — there is nothing there to resolve against.

This is not a list of safe commands. It is the list of commands whose reach can
be established: `rm -rf src` is on it, and `allowEdits` runs it without asking,
which is the authority `write` already had said out loud. A `deny` rule still
holds over every one of them.

## The mode is always on screen

The row under the prompt box says which one is in force — `ask mode on`,
`allow edits on`, `full access mode on` — every time, not once at the top.
Hours in, when the opening lines have scrolled away, which mode a session is in
must not depend on what you remember starting. The box itself is drawn in that
mode's colour, so a session that is not asking looks unlike one that is before
the row is read at all.

## Stepping it while you type

<kbd>Shift-Tab</kbd> steps to the next mode and wraps round: `ask`, then
`allowEdits`, then `fullAccess`, then `ask` again. It is reachable only while a
prompt is being typed, which is what keeps it from changing anything mid-turn:
no call is ever decided under a mode other than the one that was on screen when
the turn started.

Stepping into `fullAccess` is agreed to before it takes effect. The row says
what the mode means and waits: <kbd>Enter</kbd> confirms it, <kbd>Esc</kbd>
leaves the session in the mode it was already in. The other two steps take
effect on the press, because a mode that still asks about something can be
answered by stepping again.

A step changes one thing and no others. The rules you wrote hold exactly as
they did, and anything already allowed for the session stays allowed.

## When nobody can answer

When input has ended — a prompt piped in, a closed terminal — a question has
nobody to answer it, and an unanswerable question is a refusal. There is no
deny-by-default mode to select because this is not a choice; it is what asking
means with nobody there. A non-interactive run that must proceed says so
explicitly, with `allow` rules or with `fullAccess`.

## `--continue` resumes the transcript, not the mode

The mode is read from configuration at every start. Continuing a session picks
up its transcript; it does not pick up the mode the session last ran in — a
step made with <kbd>Shift-Tab</kbd> included — nor a session-long allow, which
lives only as long as the process it was made in.

An answer of `always` is not one of those. It writes an `allow`
[rule](rules.md) into `.crucible/config.local.json`, and every later start reads
that file, so the permission is in force until you delete the line —
see [What `always` writes](permissions.md#what-always-writes).
