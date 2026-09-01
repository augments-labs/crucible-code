# Running a command

`bash` is the one tool that is not confined to the working directory, because a
shell reaches whatever you reach. What bounds it is the question you are asked,
which names the command rather than a directory.

| Argument | What it is |
| --- | --- |
| `command` | The command line, as a shell would read it. Required. |
| `timeout` | Seconds to allow before stopping it. Defaults to 120, and anything over 600 is refused. |
| `description` | One line saying what the call is for, shown to you on the [question](../permissions/permissions.md#the-question) and again on the row that reports a backgrounded command ending. Optional, and nothing the command runs reads it. |
| `explanation` | The long form of the same thing: a list of strings, one per paragraph, shown on the question when you press `ctrl+e`. Optional, and the tool never reads it. |

The command runs through `sh -c` in the workspace root, so the model gets pipes
and redirection without crucible growing a shell of its own. On Windows that is
whichever `sh.exe` is on the `PATH`; [Git for
Windows](https://git-scm.com/download/win) carries one and crucible finds that
one where it is normally installed even when it is not on the `PATH`. A machine
with no POSIX shell says so and runs nothing.

## What a command is started with

Not crucible's own environment. That one holds your provider credential, and
`env` is a command a model runs for ordinary reasons — so a child is given a
built set of variables rather than an inherited copy.

The list is what a command needs to run at all rather than what is probably
harmless. On Linux, macOS and FreeBSD it is `PATH`, `HOME`, `LC_ALL`,
`LC_CTYPE`, `LANG`, `TERM` and `TMPDIR`. Windows needs a longer one, because the
home directory is spelled several ways there and a program that opens a socket
fails without `SystemRoot`.

Anything else a command needs is named in
[`env`](../configuration/configuration.md#env), which is you saying so. A
command that prints its environment prints no key either way.

## While it is running

A command that takes two minutes used to say nothing for two minutes. Its last
five lines now stand under the call, with a row under them counting every line
and every byte it has printed:

```
● Bash(cargo build --release)
    Compiling crucible-provider v0.5.0
    Compiling crucible-session v0.5.0
    Compiling crucible-tools v0.5.0
    Compiling crucible-auth v0.5.0
    Compiling crucible v0.5.0
    41 lines · 3.1 kB

✳ running (43s · ↓ 1.2k · esc to interrupt)
```

The row under the sample also names the one key that acts on the command itself:
<kbd>Ctrl</kbd>+<kbd>B</kbd> answers the call now and leaves the command running,
so the turn goes on without waiting for it. It is drawn from the moment the call is
out, before the counts, because a command that has printed nothing for half a
minute is the one you most want to put down.

Five rows is a sample and the count is what says so. <kbd>Ctrl</kbd>+<kbd>O</kbd>
stands the whole of what has arrived so far — the same key that shows a finished
result the transcript had to cut down to a row — and <kbd>Esc</kbd> closes it
again while the command carries on.

The sample is the first thing a short window gives up: the call line, the row
saying the turn is running, and the box all keep their rooms before it does,
because what it shows is one keypress away either way. It is drawn and taken
back rather than written down, so what the transcript keeps is the result below
and not a second copy of the build log.

A command writing over one line rather than adding lines — a progress bar — stays
one row. A command printing faster than the screen can be read has rows skipped:
the count row is what tells you so, and what the model is sent is unaffected.

## Leaving one running

Two ways, and they answer different situations. The model sends `background` when
it means to start something with no end of its own — a dev server, a file watcher,
a tunnel. You press <kbd>Ctrl</kbd>+<kbd>B</kbd> when a command you did not expect
to be long turns out to be. Either way the call is answered, the turn goes on, and
the process keeps running.

A call that asked for it is answered as soon as the command has had a moment to
fail on the spot — two hundred milliseconds, which is long enough for
`npm: command not found` to reach the model now rather than in a panel it cannot
open. A command already over by then was never a background command and comes back
as an ordinary result. The answer names the number it is running as:

```
VITE v5.4.2  ready in 412 ms
➜  Local:   http://localhost:5173/

[left running as #1; completion is reported automatically; do not poll or wait]
```

A command you pressed the key on says who let go of it, because the model asked
for that one to be waited for and is getting it back early:

```
[left running as #2; the developer pressed ctrl+b to leave it running rather
than keep waiting; carry on with what does not depend on it; completion is
reported automatically; do not poll or wait]
```

Without that it reads as its own call coming back, and a model that wanted the
command waited for will reasonably ask for it again.

`timeout` and `background` together are refused rather than one of them ignored: a
command left running has no deadline, so a call that sent both asked for two
different things.

**At most four run at once.** A fifth call is refused, naming the four in the way,
and the command it started is ended rather than left where nobody can see it.

**The question says so.** Where a call asks to be left running, the panel where you
allow it says the command will still be running after the turn ends — allowing it
is allowing that, and a panel that said only what the command was would be asking
about the wrong thing.

## Finding them again

The row under the box counts them, in the accent, because it is the one thing on
that row you can act on:

```
  ask mode on (shift+tab to cycle) · 2 commands
```

<kbd>Ctrl</kbd>+<kbd>B</kbd> at the prompt lists them where the box was — the same
key that put one down, which is how every other key here works. Each row says how
long it has been running, how many lines it has printed and how much:

```
──────────────────────────────────────────────────────────────────────────────

Still running

› 1. Bash(npm run dev)                       4m 12s · 84 lines · 6.4 kB
  2. Bash(cargo watch -x test)               1m 03s · 512 lines · 48 kB

esc to close · enter shows it · x stops it
```

<kbd>Enter</kbd> stands what one has printed, in the view a finished result is
stood in. <kbd>x</kbd> ends it, with no confirmation: the command was started by a
call you allowed, and stopping it is why the list is reachable.

## When one ends on its own

It says so, because the count going quietly down would leave you — and the model —
believing a server is up:

```
✗ starting the dev server ended on its own · exit status 1 · 96 lines
```

The line is written the moment it happens, even between turns. It names the call
the way the call described itself, in whatever words the model chose, and falls
back to `Bash(npm run dev)` where it described itself as nothing — by the time a
command ends, the turn that started it has usually scrolled away, so this is the
one chance to say which of the four it was in words you were shown at the time.
The command gives up columns before the ending does: how it ended and how much
it printed is the part nobody can go back and ask for.

The model is told the moment there is somewhere to put it. A turn that is
running takes the ending between one step and the next, so a plan built around a
server that has already fallen over gets interrupted rather than finished. Where
nothing is running, the next turn carries it — and where there is no next turn
— the model left a build running and yielded, and the box is sitting there
waiting — the ending starts one. A model waiting on your machine should not also
be waiting on your keyboard. Whatever you had half-written in the box is still
there afterwards.

Whichever route gets there first is the only one that says it. An ending is told
exactly once however long the turn it landed in was.

Nothing starts a turn where you have already begun one: a prompt you typed or
queued takes the ending with it instead, so it is said once either way.

## What ends them

`/clear` does not. A running dev server is a fact about your machine rather than
about the conversation, and unlike a forgotten transcript a killed server cannot be
resumed. <kbd>Esc</kbd> does not either: it stops the turn, and a command you
deliberately let go of is not part of the turn that started it.

What does: <kbd>x</kbd> in the list, and crucible exiting — every process group
goes with it, however the process leaves, including a panic. The one case it cannot
cover is a signal that kills crucible outright, which runs no cleanup at all; the
commands survive that, and your own shell is what ends them then.

## What comes back

Standard output and standard error, joined. A command that succeeded is just its
output:

```
   Compiling api v0.3.1
    Finished `dev` profile in 4.21s
```

A command that failed carries its exit status under it, and that is a failed
result rather than a failed turn — the model reads `[exit status 1]` and decides
what to do about it. A command that produced nothing at all answers
`(no output)`, because an empty answer reads as a tool that did not run.

Four notes cover the ways a command ends other than by finishing:

```
[exit status 1]
[the command was killed]
[stopped: the command ran too long]
[output was still arriving: something the command left running holds it open]
```

The last one is worth recognising. It means the command exited but something it
started is still holding the pipe open, so what you are reading is a prefix. It
is said whatever the exit status was: a command can succeed and still have more
to print, and unsaid, the model reads the prefix as the whole.

Long process output keeps its two ends and says both how much arrived and how
much of the middle was omitted while the pipes were being read:

```
[process output was 41200 bytes; 11456 bytes omitted from the middle during capture]
```

The ends are what carry the meaning — what the command started doing, and how it
ended. The bound is applied while the command is still running rather than on
the way out, so `yes` or `cat /dev/urandom` costs a fixed amount of memory rather
than filling it with bytes that were always going to be thrown away. The final
tool result is capped at 30,000 encoded JSON-string bytes, including escaping;
if that removes more of an escape-heavy result, a second note gives its original
encoded size and the encoded bytes omitted.

## When it stops

`timeout` seconds, or <kbd>Esc</kbd>, or the command finishing. The shell and
its descendants are one backend-owned scope, and that scope is ended and reaped
on every one of those paths. On Linux, the default `required` mode also places
that scope behind the verified Bubblewrap boundary described in
[Operating-system confinement](../security/sandboxing.md). Permission still
decides whether the command may start; the sandbox separately limits what the
approved command can reach.

A background process does not keep an output reader or a turn alive. It remains
inside the same sandbox/process-tree scope until it exits or is stopped, and its
late usage and cleanup facts keep the call attribution that started it.

## Why it is always asked about

A `SpawnsProcess` call is asked about in every [mode](../permissions/modes.md)
but `fullAccess`, and nothing the tool can say about the line changes that.

The reason is the gap between reading a command line and running it. crucible
reads the line far enough to say what will run, which is what a rule can
honestly be written about — a line whose shape it cannot read that far is
refused, and refusing means being asked. But what it cannot say is where a word
in that line will land: the shell looks the name up again when the command runs,
so a symbolic link put there in between sends the write somewhere else and
nobody was asked. The file tools have no such gap, because they keep hold of the
directory they proved. `sh` cannot be made to work that way.

So a reading of a command line is worth a rule you wrote and a question you
answer, and it is not worth crucible deciding on its own. [What an allow rule
really grants](../permissions/allowing.md) is the rest of that.
