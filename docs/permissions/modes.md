# Modes

A session runs in one mode, set by `permissions.mode` in
[configuration](../configuration/configuration.md): `ask`, `allowEdits` or
`fullAccess`. Nothing set means `ask`.

A mode decides exactly one thing: what happens to a call no
[rule](rules.md) mentions. It is never a way around the engine — every call
takes the same route to running whatever the mode, a `deny` or `ask` rule
holds in every one, and a read runs in every one.

| A call that would… | `ask` | `allowEdits` | `fullAccess` |
| --- | --- | --- | --- |
| read | run | run | run |
| change a file | ask | run | run |
| run a program | ask | ask | run |

`allowEdits` is for the stretch of work where being interrupted per edit costs
more than the edits do: files change silently, commands still ask. `fullAccess`
asks about nothing, which makes a `deny` rule the only thing that can say no
there — write those first.

## The mode is always on screen

The prompt line spells it the way configuration does — `allowEdits › ` —
every time, not once at the top. Hours in, when the opening lines have
scrolled away, which mode a session is in must not depend on what you remember
starting.

## When nobody can answer

When input has ended — a prompt piped in, a closed terminal — a question has
nobody to answer it, and an unanswerable question is a refusal. There is no
deny-by-default mode to select because this is not a choice; it is what asking
means with nobody there. A non-interactive run that must proceed says so
explicitly, with `allow` rules or with `fullAccess`.

## `--continue` resumes the transcript, not the mode

The mode is read from configuration at every start. Continuing a session picks
up its transcript; it does not pick up the mode the session last ran in, nor
anything allowed with `always` — those live only as long as the process they
were made in.
