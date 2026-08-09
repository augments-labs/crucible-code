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
  "output": { "toolDetail": "full" }
}
```

## The files

Three, read in this order. Nearer to the work wins.

| File | Holds | Checked in? |
| --- | --- | --- |
| `~/.crucible/config.json` | what you want everywhere | no, it is yours |
| `.crucible/config.json` | what this project needs | yes — everyone who clones gets it |
| `.crucible/config.local.json` | what this project needs *for you* | no, [gitignore it](#the-file-that-travels) |

The two project files are looked for in the directory you started crucible in,
which is what makes a project's settings a property of the checkout rather than
of the shell that launched it.

The command line is a fourth layer and is nearer than all three: `--model
openai/gpt-5.2` wins over anything a file says.

A file that is not there is not an error. A file that *is* there and will not
open is, and says so — silently skipping it would turn a permissions mistake
into settings that mysteriously stopped applying.

## What you can set

### `providers`

Keyed by provider name — `anthropic`, `openai`.

| Key | Means |
| --- | --- |
| `model` | The model to ask when `--model` does not name one. |
| `apiKeyEnv` | The name of the environment variable holding that provider's key. |

`apiKeyEnv` takes a **name**, never a key. A key is read from the environment at
startup and has no path into a document, a session file or a log line. Pointing
crucible at another variable also points it away from the usual one: with
`"apiKeyEnv": "WORK_ANTHROPIC_KEY"`, `ANTHROPIC_API_KEY` is not read at all.

```json
{ "providers": { "openai": { "model": "gpt-5.2" } } }
```

That model is reached by naming the provider and no model — `crucible --model
openai/`. A bare `crucible` uses `anthropic`, so it takes
`providers.anthropic.model`, and falls back to `claude-sonnet-5` if nothing set
one. See [Providers and models](providers.md).

### `output`

| Key | Answers | Means |
| --- | --- | --- |
| `color` | `auto`, `always`, `never` | Whether to dim the prompt. `auto` follows the terminal and `NO_COLOR`; the other two override both. |
| `toolDetail` | `compact`, `full` | How much of a tool call and its result one line shows. |

### `env`

Environment variables for the commands crucible runs — the bash tool's children,
and nothing else. crucible cannot put a variable in its own environment: writing
to one is `unsafe` in a process with threads, and crucible forbids unsafe code.

```json
{ "env": { "RUST_LOG": "warn", "PAGER": "cat" } }
```

Values are strings, because that is what an environment holds. A setting that
reads as a number is written `"12"`.

## The file that travels

`.crucible/config.json` is checked in, so anything in it reaches everyone who
clones the repository. crucible therefore refuses an arbitrary `env` variable in
that one file:

```
crucible: /home/you/api/.crucible/config.json: env cannot set TOKEN at line 3,
column 5 — this file is checked in, so a value written here travels to everyone
who clones this repository. Only crucible's own settings, which start with
CRUCIBLE_CODE_, are read from a checked-in file. Put this one in
.crucible/config.local.json, which git ignores, or in the configuration file in
your home directory
```

`.crucible/config.local.json` is the answer, and belongs in your `.gitignore`:

```gitignore
.crucible/config.local.json
```

The exception is crucible's own names, which begin with `CRUCIBLE_CODE_`. One of
those is not arbitrary — it is a knob crucible declares and whose meaning
crucible fixes — so a project may set one for everybody who clones it, and that
is still not a way to ship somebody's key.

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

## How layers combine

A **scalar** takes the nearest layer that set it. An **object** is merged key by
key, so a project naming one provider leaves your other one alone.

```json5
// ~/.crucible/config.json
{ "providers": { "anthropic": { "model": "claude-opus-5" },
                 "openai":    { "model": "gpt-5.2" } },
  "output": { "toolDetail": "full" } }

// .crucible/config.json
{ "providers": { "openai": { "model": "gpt-5.2-mini" } } }
```

In that project: `openai` asks for `gpt-5.2-mini`, `anthropic` still asks for
`claude-opus-5`, and `toolDetail` is still `full`. Nothing in the document is a
list yet, so there is no third rule to learn.

## Your editor

The `$schema` line is what makes an editor complete these files, check them as
you type, and show what each key means. It is optional and crucible ignores it.

The schema is generated from the same declaration the parser walks, so an editor
that accepts a document and a crucible that refuses it would have to disagree
with itself. [`schema/crucible-code-schema.json`](../schema/crucible-code-schema.json)
in this repository is the copy a build gate keeps honest.

The schema is not fixed. Keys may be added, renamed or removed in any 0.0.x
release, and the URL above serves one copy — the newest release, not the version
you are running. An editor marking something red is worth a second look; the
program is what decides.

## When something is wrong

crucible stops before drawing anything and says which file, which key, where it
is, and what was accepted instead:

```
crucible: /home/you/api/.crucible/config.json: output.colour is not a setting
crucible has at line 3, column 5 — accepted here: color, toolDetail

crucible: /home/you/api/.crucible/config.json: output.color does not accept
beige at line 3, column 5 — accepted here: auto, always, never

crucible: /home/you/api/.crucible/config.json: output.color wants one of a
fixed set of strings at line 3, column 5

crucible: /home/you/.crucible/config.json is not valid JSON at line 2,
column 14: key must be a string
```

Where a key appears more than once in the file, the position is left off rather
than pointing at one of them, which would send you to a line that is correct.

An error may name an environment variable. It never quotes the value beside it.

---

> **0.0.x is unstable.** Any key here may be renamed or removed in any 0.0.x
> release with no deprecation period. Nothing above is a compatibility promise
> until 0.1.0.
