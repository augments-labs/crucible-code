# Modes

A session runs in one mode, set by `permissions.mode` in
[configuration](../configuration/configuration.md): `ask`, `allowEdits` or
`fullAccess`. Nothing set means `ask`. That is where a session starts rather
than what it stays at — <kbd>Shift-Tab</kbd> steps it while you type, and
`/mode` names one outright.

A mode decides exactly one thing: what happens to a call no
[rule](rules.md) mentions. It is never a way around the engine — every call
takes the same route to running whatever the mode, a `deny` or `ask` rule
holds in every one, and a read runs in every one.

| A call that would… | `ask` | `allowEdits` | `fullAccess` |
| --- | --- | --- | --- |
| read | run | run | run |
| change a file | ask | run | run |
| run a program | ask | ask | run |

`allowEdits` means what its name says: `write` and `edit` change files without
asking, and anything that starts a process asks. It is for the stretch of work
where being interrupted per edit costs more than the edits do. `fullAccess`
asks about nothing, which makes a `deny` rule the only thing that can say no
there — write those first.

## Why `allowEdits` still asks before a command

`bash` runs a shell, and a shell reaches whatever you can. crucible reads the
line closely enough to say what will run, which is what lets a [rule](rules.md)
be written about it — but reading is not containment. Whatever a word in the
line was found to point at, the shell looks it up again by name when the command
runs, and a symbolic link put at that name in between sends the change somewhere
else, with nobody asked.

The file tools have no such gap. They keep hold of the directory the path was
proved under and never look the name up a second time, which is what
[containment](directories.md#what-containment-is-measured-against) is measured
by. `sh` cannot be made to work that way, so the mode that runs a command
without a question is `fullAccess`, and there is no other.

That leaves `allowEdits` as one sentence, which is the whole of what it is for:
the tools that change files change them, and anything that starts a process is
put to you. Standing permission for a command you run all day is an
[allow rule](rules.md) or an answer of `always` — written down where you can
read it back and take it away again.

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

Every step takes effect on the press, `fullAccess` included. The row under the
box says which mode that landed in and the box changes colour with it, and the
same key steps out again — a mode reached by one key is left by two more.

A step changes one thing and no others. The rules you wrote hold exactly as
they did, and anything already allowed for the session stays allowed.

## Naming the one you want

`/mode allowEdits` puts the session in that mode outright, spelled the way
[configuration](../configuration/configuration.md) spells it. It is the same
change under the same conditions — between turns, nothing else about the
session moving.

`/mode` on its own says which one is in force and lists the three to choose
from. A word that names none of them is refused, and the session stays where it
was.

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
