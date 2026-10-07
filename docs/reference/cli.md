# Command line

`crucible --help` prints the flags and the subcommands `sandbox`, `config`,
`doctor`, `auth`, `mcp`, `extensions`, `sessions` and `help`, under a longer
introduction; `crucible -h` prints the same under the one-line introduction. Each subcommand has its own help, such as `crucible
config --help` and `crucible sandbox setup --help`, and `crucible help` and
`crucible help <command>` print the same pages. `crucible --version` (or `-V`)
prints `crucible` and the version number on one line, and stops. All of these
are answered by the parser, before a file is read or anything is started.

Run with nothing after it, `crucible` opens a session in the directory you are
standing in, as [Run it](../getting-started/first-session.md#run-it)
describes. The flags change what that session is. `--extensions`, `--sandbox`
and the subcommands do one thing and stop, and none of them can be
combined with a session flag. crucible takes no prompt on the command line: a
bare word, as in `crucible "fix the bug"`, is refused as `unrecognized
subcommand 'fix the bug'` and the run ends 2. The installer also links `cru`
to the same executable, so everything here holds for `cru`
([Install it](../getting-started/first-session.md#install-it)).

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

the one `/resume` lists inside a session
([Picking one by name](../sessions/sessions.md#picking-one-by-name)), and the
one `crucible sessions list` prints beside each session. An id
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
of `ANTHROPIC_API_KEY`, `DASHSCOPE_API_KEY`, `DEEPSEEK_API_KEY`,
`GEMINI_API_KEY`, `META_API_KEY`, `MIMO_API_KEY`, `MINIMAX_API_KEY`,
`MOONSHOT_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY` and `ZAI_API_KEY` (a
variable exported empty holds none, so it does not compete), or one stored by
`/login`. A key in `DASHSCOPE_API_KEY`, `MINIMAX_API_KEY` or `ZAI_API_KEY` is
sent to the vendor's international site; a key of its mainland China site is
given through `/login`, or reaches it with that provider's `baseUrl`. Where more than one is usable, qualify the
name or set `provider` in the configuration file in your home directory, the
only file `provider`, `baseUrl` and `apiKeyEnv` are read from; otherwise
crucible starts with no provider chosen and says so
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
crucible: no provider called gemini; this build has anthropic, deepseek, google, meta, mimo, minimax, moonshot, openai, qwen, xai, zai
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

### `mcp list`

Lists every MCP server written down under `mcp.servers` in your home
configuration file, a line each, and stops. No server is started, no program
is looked up and no variable is read, so the list says what each server would
be started with and never whether it would start; that is known only once a
run names it with `--with-mcp`, which is also the only way one is started. The
first line counts the servers, as `no MCP servers written down in`, `1 MCP
server written down in` or `<n> MCP servers written down in` followed by the
file. Each line after it gives the server's name, its command, how many
arguments it is given, how many variables it sets and takes from your
environment, and `required` where it is. Only the home file is read, since a
checkout cannot write a server down.

```
1 MCP server written down in /home/you/.crucible/config.json
none is started unless a run names it with --with-mcp, and none was started to write this, so whether each would start is not known

  docs  docs-mcp, 3 arguments, 1 variable set, 1 variable taken from your environment
```

### `mcp get <NAME>`

Says how the server written down as `<NAME>` would be started, and stops: its
command, arguments, directory, variables, how long crucible waits for it,
how often it is restarted and whether a run that names it fails without it.
Nothing is started for this either. A secret is left out wherever crucible can
tell a record holds one: a variable set under `env` is shown by its name with
`<redacted>` for its value, a variable under `envFrom` by its name and the one
it is taken from, and an argument is shown with `<redacted>` wherever a key
could be. That is everything after a flag, a header or a name before `=` or
`:` that names a key, to the end of the argument, and the next argument too
when what was hidden ends on a word such as `Bearer`. Every such name in an
argument counts, including one inside a value (`--env=DB_PASSWORD=…`), one in
pairs run together with `;`, `&` or `,` as a connection string writes them,
and a key in a JSON object. It is also a URL's user, query and fragment
wherever in the argument the URL starts, the password in `user:password@host`,
and a word shaped like a token. A value after a flag whose name says nothing
about it, such as `-p`, is shown. More is hidden than is secret, on purpose.

```
  command    docs-mcp
  arguments  --token
             <redacted>
             --url=https://<redacted>@mcp.example.test/sse?<redacted>
  env        DOCS_TOKEN=<redacted>
  envFrom    DOCS_KEY from EXAMPLE_DOCS_KEY
```

A name nothing is written down under is refused with the names there are, and
the run ends 1:

```
crucible: no mcp server called dosc; this configuration has docs, notes
```

In both lists, every string from the file is cut to 512 bytes, ending `… (cut)`
where there was more, and a control character, line break or Unicode format
character in it is written as its escape. The refusal names every server
whole, with a control character or Unicode format character written as its
escape but a line break kept, as every refusal on standard error keeps one.
`mcp` on its own, without `list` or `get`, is a usage error.

### `extensions list`, `--extensions`

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
[Extensions](../configuration/configuration.md#extensions). Every name, path
and reason in the listing that came from a directory or a manifest is written
with what a terminal would act on as its escape. `--extensions` prints the
same listing and ends the same way, and `extensions` on its own is a usage
error.

### `sessions list [--json]`

Lists the sessions recorded for the directory crucible was started in, newest
first, and stops. It reads the session index and the first line of each
session's log, which says when the session started, how many messages it held
when it was last indexed, the branch it was started on and the title it was
given; nothing anybody wrote in a session is read, and no session is opened,
resumed, locked or written to, so a list can be taken while another crucible
has one open.

```bash
crucible sessions list
crucible sessions list --json
```

The first line counts the sessions, as `no sessions recorded for`, `1 session
recorded for` or `<n> sessions recorded for` followed by the directory. Each
session is one line: its id, when it started, how many messages it holds, the
branch where there was one, and its title or `untitled`. Pass the id to
[`--resume`](#-r---resume-session_id) to pick that one up.

```
2 sessions recorded for /home/you/api
  019854c2-9a1e-73f1-b0d6-2f1c4e7a58d1  3h ago  12 messages  on main  fix the parser
  01985321-04bb-7c2a-9e15-a1d03b7c6e42  yesterday  4 messages  untitled

`crucible --resume ID` carries one on
```

At most 128 sessions are listed. The list says, after the sessions, when it is
not the whole of what was recorded: how many older sessions were left out, how
many logs could not be read, that the index already holds as many sessions as
it keeps so older ones may be missing, or that no index has been written yet,
which the next session started or continued here writes. A title or branch
is written with what a terminal would act on as its escape, and one longer
than 16 KiB is cut there, ends `… (cut)` and leaves the list incomplete.

`--json` prints one JSON document on one line to standard output instead,
with `format_version` 1, `kind` `sessions`, `status` and a `truncated` flag.
`status` is `complete` when the list is the whole of what was recorded,
`incomplete` when it is not, and `failed` when no list could be made. Beside
it are `sessions`, each with its `id`, `started` (milliseconds since the
epoch), `messages`, and `branch` and `title` where there are ones, each as
`text` with its own `truncated` flag; `omitted`, the older sessions left out;
`unreadable`, the logs that could not be read; `index_full`; and
`unindexed`. When the list cannot be made, for example because the index is
not one crucible wrote, the reason is one line beginning `crucible: ` on
standard error and the run ends 1; with `--json` a document with `status`
`failed` and a `problem` that names the step that stopped, and no file and
nothing the index held, is written to standard output as well. `sessions` on
its own, without `list`, is a usage error.

### `--sandbox`

The same report as `crucible sandbox inspect`, as text, and the same exit.

### `sandbox inspect [--json]`

Prints the confinement a command in the current directory would run under,
and stops: which backend would enforce it, what that backend can and cannot
hold, the reach and ceilings a command would get, and why it would be refused.
Nothing is started to produce it, not the backend, not its helper, not a
command, and no file is written. Every path in it but the workspace root is a
digest, so it can be pasted into an issue whole.

```bash
crucible sandbox inspect
crucible sandbox inspect --json
```

The first line is `sandbox enabled in <root>` or `sandbox disabled in <root>`,
and `mode` says whether project configuration requires confinement. The
backend is the first one a confined command's search would reach that passes
the same trust checks, without starting it; where crucible read the backend's
file it prints the file's `sha256` as `build`. A backend's version that only starting it could
tell is printed `unverified`, with the reason, and whatever else only starting
something could check is listed under `not checked, since checking would start
something:`. The rest is explained under
[Reading the report](../security/sandboxing.md#reading-the-report).

Each feature is `enforced`, `observed` or `unsupported`, and each ceiling is
printed beside the claim it rests on. A backend that was found but will not
take this workspace's policy prints its matrix, what was asked for, and then
`no command could be run here` with the reason under it. No backend found
prints `no sandbox backend was found` with its reason. Both are the answer,
and the run ends 0.

`--json` prints one JSON document on one line to standard output instead,
with `format_version` 1, `kind` `sandbox-inspection`, `status`, and a
`truncated` flag. `status` is `ready` when a backend was found and would take
the policy, `refused` when it would not and `refusal` says why, `unavailable`
when none was found, and `failed` when no report could be made. Beside it are
`enabled`, `mode` (`optional` or `required`), the `requested` and `effective`
plans, `backend` with its `name`, `provenance`, `version` (`stated` with a
`name`, or `unverified` with `why`), optional `build` and `capabilities`, and
`unchecked`, `refusal` and `confined`. `confined` is never true for the
`compatibility` backend, which runs a command as an ordinary subprocess. Each
plan carries its `policy` and `commands` digests, the `cwd` digest, its
`roots` (with `omitted` for any past the list's limit), `hidden`, `network`,
`ceilings` (each with `amount`, `nanos`, `unit` and `claim`), `staged`,
`persistent` and `snapshots`.

When the report cannot be made, for example because there is no home
directory to read configuration from, the reason is one line beginning
`crucible: ` on standard error and the run ends 1; with `--json` a document
with `status` `failed` and the `problem` is written to standard output as
well. The `problem` names the step that stopped, such as reading
configuration, and no file; the line on standard error names the file, where
there is one.

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
`failures`, `schema` and a `truncated` flag. In either report, a control
character, line break, line or paragraph separator or Unicode format character
a file chose is written as its escape rather than sent to the terminal; the
text report keeps the zero-width joiner and non-joiner, which shape a word.
`config` on its own, without `check`, is a usage error.

The check reads each value's shape and the file it may come from. It does not
ask whether a provider name is one this build serves or whether a `baseUrl` is
an address crucible will send a key to: an `http` address that is not loopback
passes here, and the next start refuses it.

### `doctor [--json]`

Checks whether this machine is ready to run a conversation, and stops. It is
offline: nothing is sent to a provider, no account login is renewed, no
backend, extension or server is started, and no file is written, not even to
tighten one it reports as open. A stored credential is never used, renewed
or shown: the store is read only for the names its credentials are held
under, and no reason names a path or a value from the environment, so the
report can be pasted into an issue whole.

```bash
crucible doctor
crucible doctor --json
```

The report opens with `crucible doctor: healthy`, `warnings` or `failed`,
then one line per check: its status (`ok`, `warning`, `failed` or
`unavailable`), its id and the reason, with what to do about it on the line
under any check that is not `ok`. A reason or remedy longer than 16 KiB is cut
there and ends in `[cut]`, and a report with one says so on the line after
the first. The checks are always these, in this order, and an id never
changes:

| Id | What it looks at |
| --- | --- |
| `home` | Whether crucible's home directory was found, from `HOME` or `CRUCIBLE_CODE_HOME`. |
| `workspace` | Whether the directory crucible was started in exists and can be worked in. |
| `config` | Whether the user, project and project-local configuration files parse and resolve, file by file, as `config check` reads them. |
| `private-state` | Whether others can read or write the home directory, the sessions directory, the user configuration file or the credential store. Not checked on Windows. |
| `credential-store` | Whether the stored credentials can be used, and how many there are. |
| `credentials` | Which provider would take its credential from where: an environment variable, the store or an account login, settled as a start settles it. |
| `provider` | Which provider and model a start would open, and whether one could be asked anything. Whether the vendor accepts the credential is not known, since nothing is sent. |
| `sandbox-backend` | Whether a backend that confines commands was found here, observed the way `sandbox inspect` observes it, without starting it. |
| `sandbox-policy` | Whether the policy this directory's configuration asks for would confine a command, as `sandbox inspect` settles it. |
| `extension-discovery` | Whether the extensions directory could be read, and how many extensions are installed. |
| `extension-trust` | How many extensions may run, are not turned on, are turned on without a digest that matches, or need another crucible. |
| `mcp` | How many MCP servers are declared, and how many required. None is started, so whether each starts is not checked. |

A check that rests on one which failed is `unavailable` and says what it
rests on: with no configuration that reads, `credentials`, `provider`,
`sandbox-policy` and `mcp` are, while the home, the store, the backend and
the extensions are still looked at. `extension-trust` is also `unavailable`
when the user configuration file does not read, since what was decided about
each extension is kept there; the extensions are still discovered.
`crucible --extensions`, `config check` and `sandbox inspect` say more about
the checks that point to them.

It exits 0 when every check is `ok` or `unavailable`, 1 when any is a
`warning` and none `failed`, and 2 when any `failed`. A problem it finds is
part of the report, not a failure of the run, so standard error stays empty.
`--json` prints one JSON document on one line to standard output instead,
with `format_version` 1, `kind` `doctor`, `status` (`healthy`, `warnings` or
`failed`), a `truncated` flag and `checks`, each with its `id`, `status`,
`reason` and, for anything but `ok`, a `remedy`.

## Credentials from the command line

`crucible auth` says, stores and removes the credentials a launch signs
providers in with, outside any session. Each command takes a provider, such as
`anthropic`, or the name a `/login` row stores its credential under, such as
`moonshot@kimi.ai` ([Providers](../providers/providers.md) lists the rows).
A word that names neither is refused with the names this build serves, and the
run ends 1. A key is never an argument: a word an `auth` command does not take
is refused without being repeated, in case it was one, and the run ends 2.

### `auth status [PROVIDER] [--json]`

Says which credential a launch would sign each provider in with, or the one
named, by the rule a start uses: a deliberately signed-in account, then the
provider's variable, then a stored API key. It is offline: nothing is sent, no
account login is renewed, the store's lock is not taken and nothing is
written. The store is read for the names and lapse times its credentials are
held under and never for a value, and a variable is only asked whether it is
set.

```bash
crucible auth status
crucible auth status openai --json
```

Each provider is `configured` (a launch would sign it in), `absent` (a launch
would find nothing), `expired` (its stored account login's access has lapsed;
a launch renews it where the account still allows, which only the vendor can
say) or `unverified` (the store or the user configuration file could not be
read, or an account login holds no lapse time, so it could not be settled).
Whether a vendor accepts any credential is never checked, and the report says
so.

It exits 0 when every provider was settled, and 1 when one could not be, when
the provider named has nothing to sign in with, or when a name was too long to
show whole and the report is incomplete. `--json` prints one JSON
document on one line to standard output instead of the report, with
`format_version` 1, `kind` `auth-status`, `status` (`complete`, `incomplete`
or `failed`), `acceptance` `unchecked`, a `problem` sentence or null, a
`truncated` flag, and `providers`, each with its `provider`, `state`,
`source` (`environment`, `stored-key`, `account` or null), the `variable` it
reads a key from with `variable_set` and `variable_configured` (whether
`apiKeyEnv` named it), `base_url_configured`, what is `stored` for it by
`name`, `kind` and `expires_at` (seconds since the Unix epoch, or null), and a
`reason`. In either report, a control character, line break or Unicode format
character in a name the configuration or the store gave is written as its
escape rather than sent to the terminal; in the document that is a JSON `\u`
escape, which reads back as the same character. A run that settled nothing, such as one naming a provider nobody
serves, still writes a `failed` document, and says why on standard error.

### `auth login PROVIDER [--api-key-stdin]`

Stores a key, or signs in to an account, through the route `/login` takes,
and says under which name it was stored.

```bash
printf '%s\n' "$KEY" | crucible auth login anthropic --api-key-stdin
crucible auth login openai
```

`--api-key-stdin` reads one key from a pipe: at most 16 KiB, with the
whitespace around it set aside, and a longer one is refused rather than cut.
Standard input that is a terminal is refused, since it would show the key as
it is typed. The key is checked against its row the way the key box checks it
and written under the row's stored name, replacing the provider's other stored
credential as `/login` does. Nothing is sent to check it. Where the provider's
variable is set, the line after says a launch uses the variable first, and
where a vendor may use what it is sent, that crucible asks before the first
request.

Without the flag, in a terminal, a key row asks for the key at a prompt that
does not show it, and an account row signs in the way `/login` does: OpenAI's
by browser or by device code, Kimi's by device code, with the page to visit and
any code written to standard error and the browser opened where it can be.
Where the provider's `baseUrl` is set, the line after says a launch does not
use the account while it is. A name that is both, such as `openai`, asks
which. A sign-in whose vendor may use what it is sent shows the vendor's words
and asks first, and nothing reaches the vendor before a yes. With no terminal to ask on, the run says what to run
instead and ends 1 rather than waiting. The prompt that hides a key needs
standard output on the terminal as well, so a key row, or a name that is both,
with its output redirected is refused the same way before anything is asked;
an account sign-in needs standard input and standard error alone.

A login that changes which credential the provider is stored with takes
`providers.<name>.fast` out of the user configuration file, as `/login` does
([When it goes back to standard](../providers/fast.md#when-it-goes-back-to-standard)).
It exits 0 once the credential is stored and 1 when nothing was: a key that
does not fit, a sign-in that failed or was declined, a store that could not be
written, or a user configuration file that could not be read, since what goes
with a credential is settled from it. A login never signs another provider in.

### `auth logout PROVIDER`

Takes every credential crucible stored for the provider out of its store, in
one write under the lock a renewal holds, and leaves every other provider and
every name this build has no row for as they were. A row's name signs out the
whole provider: `auth logout moonshot@kimi.ai` takes out every credential
stored for `moonshot`, a key under the bare `moonshot` row among them, not that
row's alone. The yes given to a vendor's terms for that sign-in goes with it,
and where a credential went, so does `providers.<name>.fast` in the user
configuration file, as for `/logout`.

```bash
crucible auth logout anthropic
```

A variable that still holds a key is the shell's, not crucible's, and is never
touched: the run names it and says to unset it there. It exits 0 once the
provider's credentials are out of the store, or when it held none, and 1 when
the store could not be changed, such as while another crucible is writing it,
or the user configuration file could not be read; in either case nothing was.

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
[Install it](../getting-started/first-session.md#install-it). On Linux and
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

`sandbox` on its own, without `inspect`, `setup` or `uninstall`, is a usage
error.

## Running without a terminal

When input or output is redirected, `crucible < prompts.txt`, there is no box
to type in: each line of input is one prompt, taken in order, and the run ends
at the end of the input. A blank line is skipped, and a line whose first word
is a slash followed by letters, such as `/effort high`, is taken as a command,
as it would be in the box, rather than sent. What the output looks like is under
[Run it](../getting-started/first-session.md#run-it), and a permission
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
`Warning: No model selected. Use /model to select the model to ask.` instead.
When credentials are set up but no provider was chosen, it is
`Warning: No provider selected. Use /model to select a provider and model.`
Either way `No turn was taken.` follows it. With output still on a terminal,
the warning is drawn there instead and the run goes on to the next line.

## Exit status

| Status | Meaning |
| --- | --- |
| 0 | The run ended as asked: a session that ended, or a report that was written. `config check` on a configuration that holds, `sandbox inspect` or `--sandbox` whatever the backend answered, an `mcp list`, `mcp get`, `extensions list` or `--extensions` that was written, a `sessions list` that was written whether `complete` or `incomplete`, a `doctor` with nothing to warn about, an `auth status` that settled every provider, an `auth login` that stored a credential, and an `auth logout` that took the provider's credentials out or found none to take end here. |
| 1 | crucible could not run, or could not carry on. One line beginning `crucible: ` on standard error says why. `config check` on a configuration that does not hold, a `sandbox inspect` or `sessions list` that could not be made, and an `mcp get` naming a server nothing is written down under, end here. A `doctor` that found warnings and no failure ends here too, with its report on standard output and nothing on standard error. So does an `auth status` that could not settle a provider, found nothing for the one named, or held a name too long to show whole, with its report on standard output, and an `auth` command given a provider nobody serves. |
| 2 | A `doctor` that found a failure, with its report on standard output and nothing on standard error. Otherwise, the command line itself was refused by the parser: a flag it does not know, a value it cannot take, a subcommand missing its action, or flags that exclude each other. It says which, with the usage or the subcommand's help, on standard error; on an `auth` command line it says so without repeating the word it refused. `--help` and `--version` are the parser's too, and end 0. |
| 128 + signal | On Linux, macOS and FreeBSD, the process was told to stop from outside: a termination (`SIGTERM`, status 143) at any time, or a hang-up (`SIGHUP`, status 129) when it had a terminal to lose. A running turn is ended and written down first, then the process ends by that signal, the way a shell expects. What the next `--continue` finds is under [Continuing](../sessions/sessions.md#continuing). On Windows neither is caught. |

Flags that exclude each other are refused with a line saying one `cannot be
used with` the other: `--continue` with `--resume`, `--extensions` or
`--sandbox` with any session flag or with each other, and any flag with a
subcommand.
