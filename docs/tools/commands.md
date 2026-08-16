# Running a command

`bash` is the one tool that is not confined to the working directory, because a
shell reaches whatever you reach. What bounds it is the question you are asked,
which names the command rather than a directory.

| Argument | What it is |
| --- | --- |
| `command` | The command line, as a shell would read it. Required. |
| `timeout` | Seconds to allow before stopping it. Defaults to 120, and anything over 600 is refused. |

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

Output longer than 30000 bytes keeps its two ends and says how much went:

```
[41200 bytes of output cut from the middle]
```

The ends are what carry the meaning — what the command started doing, and how it
ended. The bound is applied while the command is still running rather than on
the way out, so `yes` or `cat /dev/urandom` costs a fixed amount of memory rather
than filling it with bytes that were always going to be thrown away.

## When it stops

`timeout` seconds, or <kbd>Ctrl-C</kbd>, or the command finishing. The shell and
its descendants are one scope — a process group on Unix, a kill-on-close job on
Windows — and that scope is ended on every one of those paths, then given a
fifth of a second to let go of its pipes.

That is resource cleanup rather than a sandbox. A background process does not
get to keep an output reader or a turn alive, and a Unix program that
deliberately creates a new session can still leave the group. Nothing here
confines what an allowed command can reach.

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
