# Command line

`crucible --help` prints the flags and the subcommands `sandbox`, `config` and
`help`, under a longer introduction; `crucible -h` prints the same under the
one-line introduction. Each subcommand has its own help, such as `crucible
config --help` and `crucible sandbox setup --help`, and `crucible help` and
`crucible help <command>` print the same pages. `crucible --version` (or `-V`)
prints `crucible` and the version number on one line, and stops. All of these
are answered by the parser, before a file is read or anything is started.

Run with nothing after it, `crucible` opens a session in the directory you are
standing in, as [Run it](../getting-started/getting-started.md#run-it)
describes. The flags change what that session is. `--extensions`, `--sandbox`
and the two subcommands do one thing and stop, and none of them can be
combined with a session flag. crucible takes no prompt on the command line: a
bare word, as in `crucible "fix the bug"`, is refused as `unrecognized
subcommand 'fix the bug'` and the run ends 2. The installer also links `cru`
to the same executable, so everything here holds for `cru`
([Install it](../getting-started/getting-started.md#install-it)).

Flags, session files and configuration are unstable for the whole 0.x line.

## Picking up a session

Left off, crucible starts a new session. These two say which earlier one to
pick up instead, and they cannot be given together.

### `-c`, `--continue`

Carries on the most recent session started in this directory: the transcript
is replayed to the model and new turns are appended to the same file.
[Continuing](../sessions/sessions.md#continuing) says what comes back with it,
and [Modes](../permissions/modes.md#--continue-resumes-the-transcript-not-the-mode)
says why the mode does not. Where nothing was ever recorded for this directory,
crucible says so and stops rather than starting a new session:

```
crucible: no earlier session for /home/you/api
```

A session another crucible still has open is refused rather than shared
([One at a time](../sessions/sessions.md#one-at-a-time)).

### `-r`, `--resume <SESSION_ID>`

Picks up the exact session the id names, wherever it falls in this directory's
history. The id is the one a quitting session prints on its way out:

```
Resume this session with:
  crucible --resume 019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1
```

and the one `/resume` lists inside a session
([Picking one by name](../sessions/sessions.md#picking-one-by-name)). An id
nothing here was recorded under, or a word that is not an id at all, is refused
by name rather than matched to the nearest thing:

```
crucible: no session 019854c2 in this workspace
```

## Choosing the model

There is no model built in, and no provider is chosen for you. Both flags sit
above your configuration: the command line is nearer than any of the three
files, so what it names wins
([The files](../configuration/configuration.md#the-files)).

### `-m`, `--model <MODEL>`

The model to ask, as a bare name or as `provider/model`. Only the first slash
divides the halves, so a model name with slashes of its own stays whole.

```bash
crucible --model claude-sonnet-5      # whichever provider holds a credential
crucible --model openai/gpt-5.6-terra # openai, asking for that model
crucible --model openai/              # openai, asking for its configured model
```

Left off, the model is `providers.<name>.model` for the provider being asked,
and where nothing says, crucible starts and asks rather than picking one;
`/model` writes your answer down
([Which model](../providers/providers.md#which-model)).

A bare name goes to whichever provider holds a usable credential: a key in one
of `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `MOONSHOT_API_KEY` and
`OPENAI_API_KEY` (a variable exported empty holds none, so it does not
compete), or one stored by `/login`. Where more than one is usable, qualify the
name or set `provider` in your configuration; otherwise crucible starts with
no provider chosen and says so
([Which provider](../providers/providers.md#which-provider)). The key is read
from that provider's variable, or from whichever one its `apiKeyEnv` names
([Keys](../providers/providers.md#keys)).

A name with nothing before the slash is refused once your configuration has
been read, before a session starts or anything is drawn:

```
crucible: --model needs a provider before the slash, as in --model openai/gpt-5.6-terra
```

A provider this build does not have is refused at the same point, with the
ones it has:

```
crucible: no provider called gemini; this build has anthropic, google, moonshot, openai
```

### `-e`, `--effort <RUNG>`

How hard to think on every turn of the session: `low`, `medium`, `high`,
`xhigh` or `max`, in whatever case you type it (`HIGH` is `high`). Left off, it
is `providers.<name>.effort` for the provider being asked, and where nothing
says either, the vendor's own default for the model. Not every model takes a
rung, and one named for a model that does not is refused by its vendor rather
than dropped ([How hard to think](../providers/providers.md#how-hard-to-think)).

```bash
crucible --effort max
```

A word that is not a rung is a usage error: the parser refuses it before
anything is read, carrying crucible's sentence `no effort called maximum;
crucible takes low, medium, high, xhigh, max`, and exits 2.

## Hosting an MCP server

### `--with-mcp <NAME>`

Hosts the MCP server written down under `mcp.servers` as `<NAME>` for this
run. Repeat it for each server you want; nothing is hosted unless it is named
here, however many are written down. What a hosted server offers is called as
`mcp:<server>/<tool>`, and the server runs confined the way a command does
([MCP servers](../configuration/configuration.md#mcp-servers)).

```bash
crucible --with-mcp docs --with-mcp tickets
```

A name nothing wrote down stops the run rather than being left out:

```
crucible: no mcp server called docs; this configuration has tickets
```

When nothing at all is written down, the line is:

```
crucible: no mcp server called docs; this configuration has none under mcp.servers
```

## Asking without starting

Each of these answers and stops. No session is started, no credential is
opened, and no extension, server or command is run. `--extensions` and
`--sandbox` take no other flag, and neither takes the other.

### `--extensions`

Lists what is installed in `~/.crucible/extensions` (or `extensions` under the
directory `CRUCIBLE_CODE_HOME` names, when it is set), with what each manifest
asks to be allowed to do and the digest crucible took over its bytes, and
stops. Nothing installed is run to produce the list, which is the point of
being able to read it. The first line counts what was found, as `no extensions
in`, `1 extension in` or `<n> extensions in` followed by the directory, or
says `<directory> was not read` when the directory holds more than 64
subdirectories, which is more than crucible reads. One description follows per
extension, and when any of them is not yet allowed the listing says once,
after the descriptions, what to do about it:

```
nothing runs until its enabled key is true and its digest key holds the digest
printed above, both in your home configuration file
```

Last, a directory under it that could not be used is listed under `1 directory
could not be read:` (or `<n> directories`), with the reason beside it: it or
its manifest could not be opened, the manifest was not one, it declares an
identifier another directory already declares, or the extensions directory
itself was over the limit. An extensions directory that itself could not be
opened counts as `no extensions in` and is listed here too. What each
description holds, how to allow one, and the limit are under
[Extensions](../configuration/configuration.md#extensions).

### `--sandbox`

Prints the confinement a command in the current directory would run under,
and stops: which backend enforces it, what that backend can and cannot hold,
the reach and ceilings a command would get, and anything given up along the
way. No command is run to produce it, and every path in it but the workspace
root is a digest, so it can be pasted into an issue whole. The first line is
`sandbox enabled in <root>` or `sandbox disabled in <root>`; the rest is
explained under [Reading the report](../security/sandboxing.md#reading-the-report).

A backend that answers but will not take this workspace's policy prints its
matrix and then `no command could be run here` with the reason under it, and
no backend answering prints `no sandbox backend answered` with its reason.
Both are the answer, and the run ends 0. The report is written even when
shutting the backend down afterwards fails; the run then ends 1 with that
failure on standard error.

### `config check [--json]`

Reads the three configuration files the way a startup would, resolves them the
way it would, and stops. The report opens with `configuration valid` or
`configuration invalid`, names each file as `user config`, `project config` or
`project-local config` with `absent`, `valid` or `invalid` after it, lists
each failure (at most four) and ends with `schema:` and the schema's id
([Checking without starting](../configuration/configuration.md#checking-without-starting)).

```bash
crucible config check
crucible config check --json
```

It exits 0 when everything holds and 1 otherwise, saying the first failure
again on standard error. `--json` prints one JSON document to standard output
instead, with `format_version` 1, `kind` `config-check`, `status`, `files`,
`failures`, `schema` and a `truncated` flag. `config` on its own, without
`check`, is a usage error.

## Windows sandbox maintenance

Native confinement on Windows needs a local account and firewall policy that
only an administrator can create, so it is provisioned once, by hand. Both
commands run from an Administrator PowerShell; they do not elevate themselves,
and an ordinary crucible run stays unelevated. What they create and remove is
described under
[Windows setup maintenance](../security/sandboxing.md#windows-setup-maintenance).
Both run inside `crucible.exe` itself; the `crucible-sandbox-broker.exe` that
a confined command later starts through is not needed for them, and where it
comes from is under
[Install it](../getting-started/getting-started.md#install-it). On Linux and
macOS the broker is installed beside `crucible` and there is nothing to run
([Turning it on](../security/sandboxing.md#turning-it-on)).

### `sandbox setup [--owner <ACCOUNT>]`

Provisions or repairs the native Windows sandbox. `--owner` names the user
account to provision; left off, it is the owner of the elevated process, so
name it when PowerShell was elevated as another administrator.

```powershell
.\crucible.exe sandbox setup --owner 'MACHINE\person'
```

### `sandbox uninstall [--owner <ACCOUNT>]`

Removes the native Windows account, network policy and setup record. `--owner`
is the account whose setup is removed, and defaults the same way; give it the
value setup was given.

```powershell
.\crucible.exe sandbox uninstall
```

Either command prints what it did on standard output and exits 0. A failure is
one line on standard error beginning `crucible: Windows sandbox maintenance
failed:` with the reason, and exits 1. On any other operating system that is
the answer too:

```
crucible: Windows sandbox maintenance failed: the native Windows sandbox is available only on Windows
```

`sandbox` on its own, without `setup` or `uninstall`, is a usage error.

## Running without a terminal

When input or output is redirected, `crucible < prompts.txt`, there is no box
to type in: each line of input is one prompt, taken in order, and the run ends
at the end of the input. A blank line is skipped, and a line whose first word
is a slash followed by letters, such as `/effort high`, is taken as a command,
as it would be in the box, rather than sent. What the output looks like is under
[Run it](../getting-started/getting-started.md#run-it), and a permission
question with nobody to answer it is a refusal
([When nobody can answer](../permissions/modes.md#when-nobody-can-answer)).
Three things end such a run early, each with one line on standard error and
exit status 1. Input that cannot be read, or a line that is not UTF-8:

```
crucible: could not read what you typed: <reason>
```

A line longer than 1 MiB:

```
crucible: what you typed is longer than 1 MiB; no prompt was accepted
```

and, when output is redirected as well, a prompt arriving where there is no
model to ask, since nobody is there to type `/model`. Rather than reading every
remaining line and answering none of them, the run says so and fails:

```
crucible: Warning: No models available. Use /login or set an API key environment variable. Then use /model to select a model. No turn was taken.
```

When a provider is chosen and only the model is missing, the sentence is
`Warning: No model selected. Use /model to select the model to ask.` instead,
with the same `No turn was taken.` after it. With output still on a terminal,
the warning is drawn there instead and the run goes on to the next line.

## Exit status

| Status | Meaning |
| --- | --- |
| 0 | The run ended as asked: a session that ended, or a report that was written. `config check` on a configuration that holds, and `--sandbox` whatever the backend answered, both end here. |
| 1 | crucible could not run, or could not carry on. One line beginning `crucible: ` on standard error says why. `config check` on a configuration that does not hold ends here. |
| 2 | The command line itself was refused by the parser: a flag it does not know, a value it cannot take, a subcommand missing its action, or flags that exclude each other. It says which, with the usage or the subcommand's help, on standard error. `--help` and `--version` are the parser's too, and end 0. |
| 128 + signal | On Linux, macOS and FreeBSD, the process was told to stop from outside: a termination (`SIGTERM`, status 143) at any time, or a hang-up (`SIGHUP`, status 129) when it had a terminal to lose. A running turn is ended and written down first, then the process ends by that signal, the way a shell expects. What the next `--continue` finds is under [Continuing](../sessions/sessions.md#continuing). On Windows neither is caught. |

Flags that exclude each other are refused with a line saying one `cannot be
used with` the other: `--continue` with `--resume`, `--extensions` or
`--sandbox` with any session flag or with each other, and any flag with a
subcommand.
