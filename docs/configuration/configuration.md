# Configuration

crucible reads JSON. Every file is optional — most machines have none of them
and crucible runs the same way.

```json
{
  "$schema": "https://www.schemastore.org/crucible-code-schema.json",
  "providers": {
    "anthropic": { "model": "claude-opus-5" },
    "openai": { "apiKeyEnv": "WORK_OPENAI_KEY" }
  },
  "permissions": {
    "allow": ["bash(cargo test)"],
    "deny": ["read(.env)"]
  },
  "output": { "toolDetail": "full" }
}
```

## The files

Three, read in this order. Nearer to the work wins.

| File | Holds | Checked in? |
| --- | --- | --- |
| `~/.crucible/config.json` | what you want everywhere | no, it is yours |
| `.crucible/config.json` | what this project needs | yes — everyone who clones gets it |
| `.crucible/config.local.json` | non-authority overrides for this checkout | by convention only — [gitignore it](#the-workspace-files) |

The two project files are looked for in the directory you started crucible in,
which is what makes a project's settings a property of the checkout rather than
of the shell that launched it.

The command line is a fourth layer and is nearer than all three: `--model
openai/gpt-5.6-terra` wins over anything a file says.

A file that is not there is not an error. A file that *is* there and will not
open is, and says so — silently skipping it would turn a permissions mistake
into settings that mysteriously stopped applying.

## What you can set

### `provider`

Which provider to ask, by the name `--model` qualifies a model with:

```json
{ "provider": "anthropic" }
```

This is the only setting that chooses a vendor, and everything under
`providers` below is about a provider already being asked rather than a way of
picking one. `/model` and `/login` write it, so a machine that holds a key for
more than one vendor answers the question once.

It is one of the keys [workspace files](#the-workspace-files) may not set:
whoever it names receives the prompt and bills for it, and that is not a
repository's choice to make for everyone who clones it.

### `providers`

Keyed by provider name — `anthropic`, `moonshot`, `openai`.

| Key | Means |
| --- | --- |
| `model` | The model to ask when `--model` does not name one. |
| `effort` | How hard to think before answering, when `--effort` does not say. |
| `apiKeyEnv` | The name of the environment variable holding that provider's key. |
| `baseUrl` | Where to send that provider's requests instead of the vendor's. |

`effort` is one of `low`, `medium`, `high`, `xhigh` or `max`, and it is set per
provider because which rungs exist is the vendor's business — a rung chosen for
the one serving it says nothing about the one that would refuse it. Left out,
crucible asks for no rung at all and the vendor's own default for that model
applies. See [Providers and models](../providers/providers.md).

`apiKeyEnv` takes a **name**, never a key. The credential wiring reads its value
at startup and does not copy it into a document, diagnostic or session message.
Pointing crucible at another variable also points it away from the usual one:
with `"apiKeyEnv": "WORK_ANTHROPIC_KEY"`, `ANTHROPIC_API_KEY` is not read at
all. Choosing an arbitrary inherited secret is authority, so `apiKeyEnv` is
read only from the configuration file in your home directory.

`baseUrl` is for a gateway or a proxy speaking the same protocol. It must be
`https`, or `http` on `localhost` — the key travels in a header on every
request, so the address decides who receives it, and plain `http` to anywhere
else is that key on somebody's network in the clear. For the same reason it is
one of the keys [workspace files](#the-workspace-files) may not set.

```json
{ "providers": { "openai": { "model": "gpt-5.6-terra" } } }
```

There is no model built in, so this is where a bare `crucible` gets one. It is
read for the provider whose key this machine holds, and `/model <name>` writes
it here for you. With nothing set here and nothing on the command line, crucible
starts and asks rather than picking a model on your behalf. See
[Providers and models](../providers/providers.md).

### `permissions`

What runs without asking, what is refused outright, and what happens to
everything else. See [Permissions](../permissions/permissions.md) for the model;
this is the key reference.

| Key | Answers | Means |
| --- | --- | --- |
| `mode` | `ask`, `allowEdits`, `fullAccess` | What happens to a call no rule mentions. |
| `allow` | a list of rules | Runs without asking. |
| `ask` | a list of rules | Put to you, whatever the mode says. |
| `deny` | a list of rules | Refused, in every mode. |
| `extraDirectories` | a list of absolute paths | Directories outside the working directory that tools may reach. |

A rule is a tool name and what it may act on: `read(src/**)`,
`bash(cargo test)`. A tool name on its own — or `bash(*)` — is everything that
tool could do.

```json
{
  "permissions": {
    "mode": "allowEdits",
    "allow": ["read(src/**)", "bash(cargo test)"],
    "deny": ["read(.env)", "edit(.git/**)"]
  }
}
```

The kind decides which rule wins, never how specific its pattern is. `deny`
beats `ask` beats `allow`, so a `deny` holds even under `fullAccess` and cannot
be qualified by an `allow` written next to it. The price is that "deny every
`git` except `git status`" cannot be said; the return is that a `deny` list is
readable on its own as the list of things that cannot happen.

`extraDirectories` entries are absolute, because a path in a configuration file
is not relative to anything the file knows. They belong in
`~/.crucible/config.json`: either workspace filename can be committed, so
neither may widen the directories a checkout can reach. A path such as
`/home/someone/src/lib` is specific to one machine, which is another reason not
to put it in project configuration.

### `output`

| Key | Answers | Means |
| --- | --- | --- |
| `color` | `auto`, `always`, `never` | Whether to write colour. `auto` follows the terminal and `NO_COLOR`; the other two override both. |
| `glyphs` | `unicode`, `ascii` | Which characters crucible draws with. `ascii` if box drawing shows as hollow squares. |
| `mouse` | `off`, `click` | Who the mouse belongs to for the length of a session. |
| `toolDetail` | `compact`, `full` | How much of a tool call and its result one line shows. |

`glyphs` is asked rather than detected. A hollow square where a border should be
is a font missing that character, and nothing about that reaches crucible — the
bytes arrived, the encoding was right, and the gap is in a font this program
cannot see. So it is a setting, and `ascii` is the answer for a terminal whose
font has no box drawing rather than a fallback crucible guesses its way into.

`mouse` is one trade with two ends rather than a preference. Left `off`, the
terminal keeps the mouse: the wheel scrolls its scrollback, dragging selects,
the middle button pastes. Set to `click`, crucible asks the terminal to forward
buttons for the whole session, so a click in the box places the cursor between
turns — and the wheel is a button too, so it stops scrolling until crucible
exits, a turn included, where a click places nothing. crucible draws inline,
which means the transcript above the box belongs to the terminal, so it cannot
scroll that for you in exchange.

### `updates`

| Key | Means |
| --- | --- |
| `check` | `auto` to find out when a newer release exists, `never` to leave the network alone. |

```json
{ "updates": { "check": "never" } }
```

`auto` is the default and is the only thing crucible reaches the network for
besides a turn. At most once a day, on a thread of its own, it asks GitHub which
release is newest and writes the answer to `~/.crucible/release`; nothing waits
for it, so the answer is drawn under the welcome the *next* time you start. No
part of your session, your directory or your configuration is sent — the request
is a plain GET for the repository's latest release, carrying a user agent that
names crucible and its version.

`never` stops the asking. crucible then never contacts GitHub, and never says
anything about releases.

### `env`

Environment variables for the commands crucible runs — the bash tool's children,
and nothing else. crucible cannot put a variable in its own environment: writing
to one is `unsafe` in a process with threads, and crucible forbids unsafe code.

```json
{ "env": { "RUST_LOG": "warn", "PAGER": "cat" } }
```

Values are strings, because that is what an environment holds. A setting that
reads as a number is written `"12"`.

A command is **not** started with the environment crucible was started in. It
gets a short list of what a program needs in order to run at all, and whatever
`env` adds on top:

- On Unix: `PATH`, `HOME`, `TERM`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`.
- On Windows: `PATH`, `PATHEXT`, `COMSPEC`, `SystemRoot`, `SystemDrive`,
  `windir`, `TEMP`, `TMP`, `TERM`, `HOME`, `USERPROFILE`, `HOMEDRIVE`,
  `HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `ProgramFiles`, `ProgramFiles(x86)`,
  `ProgramData`.

Everything else stops here, and your provider key is why. `env` and `printenv`
are ordinary things for a model to run, and what a command prints comes back as
tool output — onto your screen, into the next request, and into the session log.
The list says what to keep rather than what to drop, because `apiKeyEnv` takes a
name: a key can be called anything, so a list of the names keys usually have
would cover exactly the names somebody thought of.

A name written in `env` beats the inherited one, so `"PATH"` there replaces what
crucible was started with rather than adding to it.

A command that needs anything else — a `CARGO_TARGET_DIR`, a token a deploy
script reads — is told about it here, which is you handing it over on purpose.

## The workspace files

`.crucible/config.json` is checked in, while `.crucible/config.local.json` is
ignored only by convention. A repository can commit either filename, so
crucible refuses an arbitrary `env` variable in both:

```
crucible: /home/you/api/.crucible/config.json: env cannot set TOKEN at line 3,
column 5 — crucible cannot tell a file you wrote from one that arrived with the
checkout, so no file under the working directory sets a variable for commands.
Only crucible's own settings, which start with CRUCIBLE_CODE_, are read from
one. Put this in the configuration file in your home directory, or set it in
the shell you start crucible in
```

Keep `.crucible/config.local.json` in your `.gitignore`; the convention keeps
personal preferences out of commits even though it cannot make that file a
trusted source of authority:

```gitignore
.crucible/config.local.json
```

The exception is crucible's own names, which begin with `CRUCIBLE_CODE_`. One of
those is not arbitrary — it is a knob crucible declares and whose meaning
crucible fixes — so a project may set one for everybody who clones it, and that
is still not a way to ship somebody's key.

The same refusal covers every key that could loosen what crucible does unasked:
`permissions.mode`, `permissions.allow`, `permissions.extraDirectories`,
`providers.<name>.apiKeyEnv`, `providers.<name>.baseUrl`, and `provider`. The
last three are not permissions, and they are here for the same reason — they
choose which credential is read or who receives it, and nothing on that path
stops to ask. Each is read only from your home file and refused in both files
under the workspace.

The refusal is structural rather than a warning, and there is no "trusted
project" setting that switches it off. The guarantee holds only because there is
no such path.

## `CRUCIBLE_CODE_HOME`

Moves crucible's whole directory — the configuration file and the session logs
both. It is taken as the home itself, not as somewhere to put a `.crucible`
inside, and only when it is an absolute path.

Because it is read to *find* the configuration file, it is the one setting of
crucible's own that a configuration file cannot carry. Writing it in one is
refused rather than accepted and ignored:

```
crucible: /home/you/.crucible/config.json: env cannot set CRUCIBLE_CODE_HOME at
line 3, column 5 — crucible reads it before it opens any configuration file,
because it is what says where the files are. Set it in your shell instead
```

## `CRUCIBLE_CODE_CLEAR_SCREEN`

Empties the terminal — the screen and the scrollback above it — before crucible
draws its first row. Off unless you ask for it, because crucible draws inline:
what is already on the screen is your own work, and the terminal's scrollback is
yours to keep.

```json
{ "env": { "CRUCIBLE_CODE_CLEAR_SCREEN": "true" } }
```

Written in `env` like any other variable, so it layers like one: a project can
set it for everybody who clones the repository, your home directory can set it
for every project, and the environment you start crucible in beats both.

```console
$ CRUCIBLE_CODE_CLEAR_SCREEN=0 crucible
```

`1` and `true` mean yes, `0` and `false` mean no, in any capitalisation.
Anything else is refused rather than read as `false`:

```
crucible: .crucible/config.json: env CRUCIBLE_CODE_CLEAR_SCREEN at line 3,
column 5 is not set to an answer crucible takes — accepted here: 1, true, 0,
false
```

A run whose output is redirected is never cleared. A pipe has no screen, so the
sequence would be escape bytes at the top of your file.

## How layers combine

A **scalar** takes the nearest layer that set it. An **object** is merged key by
key, so a project naming one provider leaves your other one alone. A **list** is
concatenated: every layer's entries are kept and none of them replaces another.

Say `~/.crucible/config.json` holds this:

```json
{ "providers": { "anthropic": { "model": "claude-opus-5" },
                 "openai":    { "model": "gpt-5.6-terra" } },
  "output": { "toolDetail": "full" },
  "permissions": { "deny": ["read(.env)"] } }
```

and the project's `.crucible/config.json` holds this:

```json
{ "providers": { "openai": { "model": "gpt-5.6-sol" } },
  "permissions": { "allow": ["bash(cargo test)"] } }
```

In that project: `openai` asks for `gpt-5.6-sol`, `anthropic` still asks for
`claude-opus-5`, `toolDetail` is still `full`, and both permission rules are in
force.

Concatenation is the only rule a list could have here. If a nearer layer
replaced a farther one, a `.crucible/config.json` that mentions `deny` at all
would silently drop every `deny` you wrote at home — and a checked-out
repository would be deciding what your own machine protects. Keeping both is
safe precisely because `deny` wins wherever it came from.

The cost is that a list cannot be shortened by a nearer layer, only added to.
Removing an entry means editing the file that holds it.

## Comments

JSON has no comment syntax, which is the one real cost of the format. `$comment`
is the standard's own answer to it, and crucible takes it anywhere in a
document — at the top, beside a rule list, inside a provider block — and does
nothing with it:

```json
{ "permissions": {
    "$comment": "read(.env) is denied because the deploy keys are in it",
    "deny": ["read(.env)"] } }
```

A `//` line is not a comment here. crucible parses JSON, so a file carrying one
is refused before anything is drawn.

## Your editor

The `$schema` line is what makes an editor complete these files, check them as
you type, and show what each key means. It is optional and crucible ignores it.
Those two are the keys the standard reserves, and the only ones beginning with
`$` that mean anything here.

The schema is generated from the same declaration the parser walks, so an editor
that accepts a document and a crucible that refuses it would have to disagree
with itself. [`schema/crucible-code-schema.json`](../../schema/crucible-code-schema.json)
in this repository is the copy a build gate keeps honest.

The schema is not fixed. Keys may be added, renamed or removed in any 0.x
release, and the URL above serves one copy — the newest release, not the version
you are running. An editor marking something red is worth a second look; the
program is what decides.

## When something is wrong

crucible stops before drawing anything and says which file, which key, where it
is, and what was accepted instead:

```
crucible: /home/you/api/.crucible/config.json: output.colour is not a setting
crucible has at line 3, column 5 — accepted here: color, glyphs, mouse,
toolDetail

crucible: /home/you/api/.crucible/config.json: output.color does not accept
beige at line 3, column 5 — accepted here: auto, always, never

crucible: /home/you/api/.crucible/config.json: output.color wants one of a
fixed set of strings at line 3, column 5

crucible: /home/you/.crucible/config.json is not valid JSON at line 2,
column 14: key must be a string

crucible: /home/you/api/.crucible/config.json: permissions.allow[1] at line 3,
column 5 — read(src is not a rule; a rule names a tool and what it may act on,
like read(src/**)

crucible: /home/you/api/.crucible/config.local.json:
permissions.extraDirectories[0] must be an absolute path at line 3, column 5 —
../shared is relative, and a configuration file cannot know what it would be
relative to
```

An entry in a list is named by the index it sits at and located at the key
holding the list, because an entry has no key of its own to search the file for
— and the other `"read(src/**)"` further down would be a perfectly correct line
to be sent to.

Where a key appears more than once in the file, the position is left off rather
than pointing at one of them, which would send you to a line that is correct.

An error may name an environment variable. It never quotes the value beside it.
