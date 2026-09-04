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

When `/model`, `/effort` or `/login` changes the user file, crucible prepares an
owner-only sibling and replaces the complete document atomically. A failed
write before that commit leaves the previous file whole. An owner-only lock
spans the bounded reread through the commit, so simultaneous crucible processes
cannot silently lose one another's settings.

A file that is not there is not an error. A file that *is* there and will not
open is, and says so — silently skipping it would turn a permissions mistake
into settings that mysteriously stopped applying. Each file is limited to 1
MiB before JSON parsing, so a checkout cannot choose an unbounded startup
allocation.

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
| `contextWindow` | The session's context-window size in tokens, keyed by model name. |
| `defaultContextWindow` | The same, for any model of this provider not named above. |

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

`contextWindow` is keyed by model because a session changes which model it asks
without changing which vendor it writes to, and a figure left behind would
describe the model you had just left:

```json
{ "providers": { "openai": { "contextWindow": { "gpt-5.6-sol": 272000 } } } }
```

Without either setting, the session's context window is 200,000 tokens for
Anthropic, 272,000 for OpenAI, and 262,144 for Moonshot. A known model with a
smaller native limit keeps the smaller figure, and an unknown model of a known
provider gets that provider's default. Native 1M support therefore does not make
1M the session default.

Use `contextWindow` to opt a named model into a larger window, or
`defaultContextWindow` for every otherwise-unnamed model of one provider. Neither
is sent anywhere. The configured/default value is the window; the existing
compaction reserve is applied separately, so automatic compaction starts when
`carried + reserve >= window`. Setting a window too large may let a request reach
the provider's real limit and be refused; setting one too small compacts earlier.

### `systemPrompt`

What the model is asked under, before you have typed anything.

| Key | Answers | Means |
| --- | --- | --- |
| `tone` | `concise`, `explanatory`, `learning` | How much of the reasoning comes back with the answer. |
| `append` | a paragraph | Said after crucible's own instructions, every turn. |
| `custom` | a whole prompt | Asked under in place of crucible's own instructions. |

```json
{
  "systemPrompt": {
    "tone": "explanatory",
    "append": "This repository is deployed on Fridays; never push to main."
  }
}
```

All three tones ask for the same work done to the same standard; what changes
is how much of the reasoning arrives with it, which is a fact about who is
reading rather than about what was asked. `concise` is the default and gives
the result and what it cost to reach it. `explanatory` adds why that answer and
not the one next to it. `learning` hands part of the change back: past twenty
lines or so it leaves a single `TODO(human)` where a decision belongs and asks
you to make it.

`append` and `custom` stay two keys rather than one key with a mode. Adding a
paragraph should not mean restating the prompt you wanted to keep, and putting
your own prompt in should not silently concatenate it with the one you were
replacing. Neither reaches the workspace root, the tool list or the model's own
name: those are what the session found out rather than something crucible has an
opinion about.

`custom` is one of the keys [workspace files](#the-workspace-files) may not set.
What it replaces includes the lines about reading a file before changing it and
saying where the work actually stands, and a repository is not allowed to take
those away from whoever cloned it. `append` can only add, so a checkout may set
it.

### `compaction`

What happens when the model's window fills up. See
[Sessions](../sessions/sessions.md#when-the-window-fills).

| Key | Means |
| --- | --- |
| `when` | `full` to make room when there is none left, or `never`. |
| `reserve` | Tokens kept free for the next answer and the tools it calls. |
| `keep` | How many tokens of recent turns are kept word for word after the rest becomes a recap. |
| `recap` | Maximum output tokens for the structured recap; concise recaps stop earlier. |
| `askOnResume` | How large a session must be, in tokens, before picking it up asks about it. |
| `spendCeiling` | The most tokens one turn may produce before crucible stops it. |

```json
{ "compaction": { "when": "full", "keep": 40000 } }
```

Left alone, crucible makes room when it has to and stops a turn for nothing
else. `never` does not disable `/compact` — that is you asking rather than
crucible deciding — it means a turn that runs out of room fails instead of
recovering.

`keep` is in tokens rather than turns because a turn can be enormous: the kept
tail has to fit the window beside the recap, and only a figure in the window's
own unit can promise that. The turn you are in is always kept whole, whatever
it has cost so far; the budget bounds the turns before it. Left unset, crucible
keeps the most recent 20,000 tokens.

`recap` is a ceiling rather than a requested length. Left unset, a structured
recap may produce up to 10,240 tokens, further limited by the model's output
ceiling and the room safely available in its window. A recap that reaches its
token ceiling or omits a required section replaces nothing.

`reserve` is worked out from the model if you do not set it: enough for one
answer of the length crucible asks for, plus the tool results a pass carries
back. **Raising what an answer may be raises the reserve**, because a request
and its answer have to fit the window together — so a larger answer ceiling
means compacting sooner, not later. The reserve is never more than half the
window, so a small model still has half of itself to work in.

`askOnResume` is a number of tokens, and `0` means never ask — which is what
the *stop asking* answer writes down. See
[Sessions](../sessions/sessions.md#picking-up-a-large-one).

`spendCeiling` is off unless you set it. It bounds what a runaway turn actually
consumes rather than counting the calls it makes, because a turn that is long
because there is work in it is not a turn to stop.

### `promptCaching`

Provider-side reuse of an identical logical prompt prefix. It is enabled by
default using each shipped provider's own verified native mechanism; it never
reuses a model answer or skips a provider request.

```json
{
  "promptCaching": {
    "mode": "prefer",
    "isolationScope": "session",
    "requestedRetention": { "class": "ephemeral", "maxSeconds": 1800 },
    "persistentResources": { "mode": "forbid" }
  }
}
```

| Key | Means |
| --- | --- |
| `mode` | `observeOnly`, `prefer`, `require`, or `prohibit`. The default is `prefer`. |
| `allowedMechanisms` | Optional intersection of `providerManagedUsageOnly`, `automaticPrefix`, `explicitBreakpoints`, and `persistentContent`. |
| `isolationScope` | Broadest identity scope allowed to share a prefix: `run`, `session`, `workspace`, or `user`. The default is `session`. |
| `requestedRetention` | Optional provider-neutral `class` and hard `maxSeconds` ceiling. |
| `persistentResources.mode` | Separately managed remote resources are `forbid`, `reuse`, `create`, or `require`. The default is `forbid`. |
| `namespace` | A bounded user-owned identity label; it is never copied directly into a provider cache key. |

`observeOnly` adds no Crucible cache controls, although a provider may still
cache automatically and report that usage. `require` fails before sending if no
reviewed eligible mechanism can be selected. `prohibit` also fails before
sending unless the provider exposes a documented opt-out.

`providerDefault` requests no retention override. `ephemeral` and `extended`
require a positive bounded `maxSeconds`; the figure is a ceiling rather than an
exact TTL promise. Extended retention, resource creation, broad isolation and a
namespace must come from your home configuration. Workspace layers can only
narrow the inherited policy. Persistent resources are never created by the
default, and their private metadata contains no prompt, response or credential.
See [Prompt caching](../providers/prompt-caching.md) for exact provider behavior,
inspection, privacy and source provenance.

### `sandbox`

Operating-system confinement for `bash` and explicitly selected extension and
MCP processes is **disabled by default**. Enable it with:

```json
{ "sandbox": { "enabled": true } }
```

Omitting the block, writing an empty block, or setting `enabled: false` in your
home configuration leaves commands unconfined by the operating system.
Permissions, sensitive-call approval, environment filtering, deadlines, output
bounds and lifecycle accounting still apply.

`enabled: true` resolves to `required`: every requested hard boundary must be
enforced before a command starts. It never silently falls back to an ordinary
subprocess. The currently implemented enforcing backend is Linux Bubblewrap
(0.11.0 or newer with the required features). macOS and Windows enforcing
backends are not yet implemented; enabling confinement there refuses command
execution. See the [current platform status](../security/sandboxing.md#platform-support).

Explicit `sandbox.mode` remains supported: `required` has the same meaning as
`enabled: true`; `off` has the same meaning as `enabled: false`; `degraded`
prefers enforcement and permits a reported compatibility fallback only when
the backend is unavailable. Write **either `enabled` or `mode` in one document**;
using both is an error even when their values agree.

Only your home configuration may disable confinement or select `degraded`.
Either project file may set `enabled: true` or `mode: "required"`, which
strengthens the user choice regardless of which spelling either file uses.
Project `enabled: false`, `off`, and `degraded` are refused. A project, tool,
extension, skill, agent, or descendant cannot weaken confinement chosen above it.

Under `required`, a command also runs under resource ceilings the confining
backend applies: an hour of processor time per process, and 4096 open files at
once. They are not a budget you are meant to work within — a long build and a
program that opens a great many files both pass — but the point past which a
command has stopped being a command. They are not configurable, and a command
may narrow them but not drop them.

The compatibility modes retain command guardrails, deadlines, output bounds,
usage and audit records, but their inspection report says `confined: false`.
They apply no resource ceiling, because the ceilings above are the confining
backend's to apply: choosing `degraded` or `off` takes them off with it.
Permission approval and a worktree are not a sandbox in any mode.

`crucible --sandbox` prints what a command in the directory you are standing in
would actually run under — which backend enforces it, what that backend can and
cannot hold, the reach and ceilings a command would get, and anything given up
along the way — and stops without running one. See
[Operating-system confinement](../security/sandboxing.md) for the exact backend
capability matrix, lifecycle, inspection and failure behavior, and for how to
read that report.

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

### `input`

| Key | Answers | Means |
| --- | --- | --- |
| `send` | `enter`, `altEnter` | Which press sends a prompt, and which one opens a line under it. |

```json
{ "input": { "send": "altEnter" } }
```

Leave it alone and Return sends, while Shift+Return, Alt+Return and Ctrl+J each
open a line under the one you are typing — as does a backslash on the end of the
line you are on, which asks the terminal for nothing at all. That is what almost
every terminal makes possible and it is what the prompt does out of the box.

Set it to `altEnter` and the two swap: Return opens a line and a modified Return
sends. That is the answer for a terminal that keeps Shift+Return for itself and
never forwards it — you press Return for as many lines as you want, then
Alt+Return to send. Ctrl+J sends too, because a terminal has always spelled it
the same way as the other modified Returns.

Control and Return is not on the list and cannot be. A terminal that has not
agreed to the newer keyboard protocol sends exactly the same bytes for it as for
Return alone, so nothing here could tell them apart and choosing it would leave
you with no way to send at all.

### `output`

| Key | Answers | Means |
| --- | --- | --- |
| `color` | `auto`, `always`, `never` | Whether to write colour. `auto` follows the terminal and `NO_COLOR`; the other two override both. |
| `glyphs` | `unicode`, `ascii` | Which characters crucible draws with. `ascii` if box drawing shows as hollow squares. |
| `theme` | `auto`, `dark`, `light`, `colourblind-dark`, `colourblind-light`, `ansi` | Which colours crucible draws with. |
| `syntaxTheme` | a theme name | Which theme fenced code is drawn in. |
| `toolDetail` | `compact`, `full` | How much of a tool call and its result one line shows. |

`theme` is a table of what each colour on screen means, tuned to one background.
`auto` asks the terminal what its background is and picks the dark or the light
table from the answer, which is the setting to leave alone unless you have a
reason: it is the only one that keeps being right when you change your terminal.
The two `colourblind` tables move the diff off the red-green axis — a line put
in goes blue and a line taken out goes amber — and `ansi` spends nothing but the
sixteen colours your terminal already has, so your own terminal theme decides
every hue.

`/theme` picks one at the prompt and writes it here. It draws a diff and a
prompt row under the list in whatever your mark is standing on, because a theme
is a list of colours and nobody can picture one from its name.

One thing is not in any table: the row your own prompt is left on takes a
background blended off your terminal's, a fixed step lighter on a dark one and
darker on a light one, so it cannot fight a terminal theme crucible has not
seen. Most terminals will not say what their background is — the question is not
widely implemented — and there the step is taken off the background the table in
force is drawn for instead, which is the same assumption every other colour on
screen is already making.

`syntaxTheme` is a separate answer because it is a separate question. The theme
above decides the interface — borders, marks, the mode in force, the ground a
diff takes. This decides what a fenced block of code in an answer looks like,
and the two are chosen together on `/theme`, one axis each.

The names are the ones you already have an opinion about: Monokai Extended,
GitHub, Dracula, Nord, gruvbox, Solarized, one-half and the rest. `/theme` lists
every one of them.

A block is read only where its fence named a language crucible knows — ```` ```rust ````
rather than a bare ```` ``` ````. One that named nothing, or named something it does
not know, is drawn exactly as it was before any of this existed: quiet and
whole. TypeScript is read as the JavaScript it extends, so a type annotation is
drawn as ordinary words.

`glyphs` is asked rather than detected. A hollow square where a border should be
is a font missing that character, and nothing about that reaches crucible — the
bytes arrived, the encoding was right, and the gap is in a font this program
cannot see. So it is a setting, and `ascii` is the answer for a terminal whose
font has no box drawing rather than a fallback crucible guesses its way into.

It is one answer for the whole interface rather than one for the box. Every mark
crucible draws comes out of the same set as the border:

| Drawn | `unicode` | `ascii` |
| --- | --- | --- |
| The mark a line is typed after | `›` | `>` |
| One character of a key being pasted | `•` | `*` |
| The mark a tool call opens with | `●` | `*` |
| The corner its result hangs under | `└` | `+` |
| A call that failed | `✗` | `x` |
| A line that was cut | `…` | `...` |
| The keys that walk the effort ladder | `←` `→` | `<` `>` |
| Between two things on one row | `·` | `-` |
| Between a thing and what is said about it | `—` | `--` |

The name at the top of a session goes the same way: `unicode` draws it from half
blocks and `ascii` writes it as letters.

The mouse is not among these keys. crucible holds it for the whole session: the
wheel scrolls the transcript, a click puts the cursor where you point or opens a
result the transcript cut short, resting the pointer on one of those results
lights the one you are on, and a drag selects what it covers and puts it on your
clipboard when you let go.

Hold **Shift** while you drag and the selection is your terminal's own again —
every terminal keeps Shift as the way past a program holding the pointer, which
is the answer for a reader who wanted their emulator's selection rather than
this one.

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
`systemPrompt.custom`, `providers.<name>.apiKeyEnv`, `providers.<name>.baseUrl`,
and `provider`. The last four are not permissions, and they are here for the
same reason — they replace the instructions that say to ask, or choose which
credential is read and who receives it, and nothing on those paths stops to ask.
Each is read only from your home file and refused in both files under the
workspace.

The refusal is structural rather than a warning, and there is no "trusted
project" setting that switches it off. The guarantee holds only because there is
no such path.

## Extensions

`~/.crucible/extensions` is where extensions are installed, one directory each,
with a `manifest.json` saying what that extension is and what it would like to
be allowed to do:

```json
{
  "id": "acme.reviewer",
  "version": "1.4.0",
  "protocol": "1.0",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.34.0",
  "capabilities": ["registerTools", "readRunContext"],
  "contributions": ["tools"]
}
```

`crucible --extensions` lists what is there and stops:

```
1 extension in /home/you/.crucible/extensions

acme.reviewer 1.4.0
  from      /home/you/.crucible/extensions/reviewer/manifest.json
  protocol  1.0, needs crucible 0.34.0
  asks for  registerTools, readRunContext
  gives     tools
  hosted    yes
  may run   no; nobody has said this extension may run
  config    nothing
  digest    sha256:810cb273aa0d388bf206a0685138577efc74b078759f6921d166539580d61e16

nothing runs until its enabled key is true and its digest key holds the digest
printed above, both in your home configuration file
```

Nothing installed is run to produce that list, which is the point of being able
to read it: the entrypoint is a string in a file crucible has not opened, and
the digest is taken over the manifest's own bytes rather than read out of it, so
two listings a week apart tell you whether the file changed.

`hosted` is whether this crucible could run the extension at all, which is a
different question from whether you have allowed it and is not something
allowing it would change. It says no for two reasons. One is a protocol whose
first number is not the one this build speaks — two programs that disagree
about the shape of what crosses the wire, with no older crucible or newer one
that would help:

```
  protocol  2.0, needs crucible 0.34.0
  hosted    no; this crucible speaks protocol 1.0
```

The other is an extension written for a crucible later than the one you are
running, which an upgrade fixes:

```
  protocol  1.0, needs crucible 0.40.0
  hosted    no; this crucible is 0.34.0
```

An extension asking for a higher second number is not refused: the two settle
on the smaller vocabulary they both know, and the listing says which that is,
because the part of the extension written against the rest of it will find it
missing.

```
  protocol  1.4, needs crucible 0.34.0
  hosted    yes, speaking 1.0
```

**Crucible does not yet run extensions.** This release reads the manifests,
shows you them, and records which ones you have decided to allow; there is no
host that starts one yet.

### Allowing one

Installed is not permitted. An extension stays off until you say otherwise, and
you say it under its own identifier — the `id` its manifest states, which is
also what the listing prints. Two keys, not one:

```json
{
  "extensions": {
    "acme.reviewer": {
      "enabled": true,
      "digest": "sha256:810cb273aa0d388bf206a0685138577efc74b078759f6921d166539580d61e16"
    }
  }
}
```

`enabled` is your answer; `digest` is which program you answered about, copied
from the listing. Neither permits anything alone, and the listing says which
half is missing:

```
  may run   no; no digest says which program was agreed to
```

An extension keeps its identifier when it updates itself, and it keeps it if
something else on your machine writes over it. The digest is what does not
survive either, so a decision recorded against it stops applying at the moment
the program you agreed to stopped being the one that is there:

```
  may run   no; the manifest has changed since it was agreed to at sha256:810cb2…
```

That is not an accusation, and an update you were expecting is the ordinary
cause. Read the listing again, decide again, and paste the new digest.

Those keys are read from `~/.crucible/config.json` and from nowhere else. Writing
it in `.crucible/config.json` or `.crucible/config.local.json` is refused rather
than accepted and ignored:

```
crucible: .crucible/config.json: extensions.acme.reviewer.enabled cannot be set
here at line 2, column 5 — this file is inside the workspace and can arrive with
a checkout, and this key only ever widens what crucible does without asking. A
workspace file may tighten its own rules — permissions.ask and permissions.deny
— and may not loosen anybody's. Put this one in the configuration file in your
home directory
```

Both project files can be committed, so a repository carrying that key would be
a checkout deciding that code on your machine may run. Whoever is running
crucible is the only one who can answer that, in the one file only they write.

### Configuring one

An extension's own settings go in a `config` block beside `enabled`, under names
its documentation gives rather than any crucible knows:

```json
{
  "extensions": {
    "acme.reviewer": {
      "enabled": true,
      "digest": "sha256:810cb273aa0d388bf206a0685138577efc74b078759f6921d166539580d61e16",
      "config": { "style": "terse", "rules": ["no-unwrap"], "depth": 3 }
    }
  }
}
```

Nothing inside is checked, because there is nothing here to check it against:
crucible has never read that extension's documentation, and refusing a key it
does not recognise would mean deleting a line the extension told you to write.
Any JSON goes in — strings, numbers, lists, blocks inside blocks — and the only
thing crucible insists on is that the block is a block:

```
crucible: /home/you/.crucible/config.json: extensions.acme.reviewer.config wants
an object of the extension's own settings at line 5, column 7
```

The listing names what you wrote and never what you set it to, because crucible
cannot tell which of those names holds a key you pasted:

```
  may run   yes
  config    depth, rules, style
```

`config` is read only from your home file, like `enabled` and `digest` and for the same
reason one step removed: crucible cannot read these names, so it cannot tell a
harmless one from somewhere to send the checkout. A key whose danger it has no
way to weigh is not one a committed file may write on your behalf.

A directory that cannot be read does not hide the ones that can. Each is listed
under its own heading with the reason:

```
1 directory could not be read:
  /home/you/.crucible/extensions/broken/manifest.json: line 2 column 0: EOF while parsing a value
```

Crucible looks at 64 directories. Past that the listing says the answer is
short rather than presenting a truncated one as complete. Two directories
claiming one `id` are not both kept — the first in sorted order keeps the
identifier and the second is listed as refused, because the identifier is what
everything else would key on.

## MCP servers

An MCP server is somebody else's program that contributes tools. `mcp.servers`
is where you write down which ones exist on this machine, keyed by the name you
want their tools qualified by:

```json
{
  "mcp": {
    "servers": {
      "docs": {
        "command": "npx",
        "args": ["-y", "@example/docs-mcp"],
        "envFrom": { "DOCS_TOKEN": "MY_DOCS_TOKEN" }
      }
    }
  }
}
```

The name you choose is the name you will read later: `docs` makes the server's
`search` tool `mcp:docs/search`. So a name may not hold `:` or `/`, which are
the two characters that qualification is spelled with — a server called `a/b`
would produce tool names nobody could read back to a server.

Writing a record starts nothing. It is a statement that a server exists and how
it would be launched; what launches one is a selection made per run, and the
selection is `--with-mcp`:

```bash
crucible --with-mcp docs
```

Repeat the flag for each server you want. A run that names none starts none,
which is every run that does not type it — twenty servers written down and no
flag is twenty processes that do not exist. A name nothing wrote down stops the
run rather than being quietly left out, because a turn missing the tools you
asked for reads as a model that will not do the work.

The servers a turn hosts are started when it begins and stopped when it ends,
and each is asked once what it offers. What comes back is named under the server
it came from — the `docs` server's `search` tool is `mcp:docs/search` — so
nothing a server offers can take over a name crucible already uses.

A server is somebody else's program, so it runs confined the way a command run
through `bash` does, under the same `sandbox.mode`. It starts in the workspace
unless the record names a `directory`, which is then a root it may write in as
well as the place it starts. That path does not have to be inside the workspace
and is not checked against it: naming one widens what the server may reach, and
it is a key only your own configuration file may write. And it is given exactly
the variables `env` and `envFrom` name and nothing else: a server inherits
none of crucible's own environment.

`command` is the only key a record cannot do without, and it is either an
absolute path or a bare name for `PATH` to answer. Anything in between —
`./server`, `bin/server` — is refused, because it would be resolved against
whichever directory crucible happened to be started in:

```
crucible: /home/you/.crucible/config.json: mcp.servers.docs.command is neither
an absolute path nor a bare program name at line 4, column 7 — ./docs-mcp would
be resolved against whichever directory crucible was started in, so the same
record would run a different program from a different place. Write the whole
path, or a bare name for PATH to answer
```

What counts as absolute is the machine's own answer: a leading `/` on Linux and
macOS, a drive or a share on Windows. A bare name is the spelling that means the
same thing on all of them, which is why the schema offers it first.

`env` holds values and is applied verbatim, so nothing secret belongs in it — a
configuration file is a file, and a value written there is a value on disk.
`envFrom` is the key for a secret: it holds *names* on both sides. `"DOCS_TOKEN":
"MY_DOCS_TOKEN"` means the server is given `DOCS_TOKEN` set to whatever crucible
was itself started with in `MY_DOCS_TOKEN`. The token never appears in a
document, a session file or a log line, which is the same bargain `apiKeyEnv`
makes for provider keys.

The rest of the record is the timing and failure behaviour, and every one of
them has an answer already:

| Key | Default | What it decides |
| --- | --- | --- |
| `handshakeSeconds` | `10` | How long to wait for the server to agree a protocol version |
| `requestSeconds` | `60` | How long to wait for one request |
| `shutdownSeconds` | `5` | How long the server is given to stop before it is killed |
| `restarts` | `0` | How many times it may be started again after it ends |
| `required` | `false` | Whether a run that selected it fails when it cannot be prepared, rather than carrying on without its tools |

`requestSeconds` is how long a server that says nothing is given, not how long
an interrupt takes. Pressing escape before a call reaches the server refuses it
there and then. Pressing it during a call ends the wait at the press: the call
comes back cancelled immediately, whatever `requestSeconds` is set to. What the
press cannot do is reach the server — the request has gone, the tool may be
running, and from crucible's side a tool that never started, one that finished,
and one whose answer was lost look the same. So an interrupted server is
finished with for the rest of that turn rather than asked a second question it
would answer with the first one's reply. Set `requestSeconds` to what you are
willing to wait for a server that has stopped answering.

`restarts` is a ceiling on the endings crucible can prove were harmless, not a
retry count. A server whose process had already gone when crucible tried to
write the call left the far end untouched, so it is started again and the same
call sent once — that is what the number is spent on. Every other ending has a
request outstanding, and no number makes repeating it safe: those end the server
for the turn whatever the ceiling says. A server started again has to come back
offering the tool under the same name and the same schema, because the
description the model wrote its arguments against is the one this run published;
a catalogue that moved retires the server instead. The default of `0` is one
start and no more.

Every key in `mcp.servers` is read **only** from `~/.crucible/config.json`. A
committed `.crucible/config.json` naming a server would be choosing whose
program runs — and what it is told, and what it is started with — on behalf of
whoever cloned the checkout, before anything has been typed:

```
crucible: /home/you/api/.crucible/config.json: mcp.servers cannot be set here at
line 3, column 5 — this file is inside the workspace and can arrive with a
checkout, and this key only ever widens what crucible does without asking. A
workspace file may tighten its own rules — permissions.ask and permissions.deny
— and may not loosen anybody's. Put this one in the configuration file in your
home directory
```

Crucible reads 64 servers, 256 arguments and 256 variables per record. Past a
bound the rest is not read, so a document cannot make startup walk further by
being longer.

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

## `CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`

How many rows one notch of the wheel moves the transcript. `6` unless you say
otherwise, which is about three lines of prose per notch.

```json
{ "env": { "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": "12" } }
```

Written in `env` like any other variable, so it layers like one: a project can
set it for everybody who clones the repository, your home directory can set it
for every project, and the environment you start crucible in beats both.

```console
$ CRUCIBLE_CODE_MOUSE_SCROLL_SPEED=3 crucible
```

A whole number from `1` to `30`. Anything else is refused rather than rounded
into range or ignored:

```
crucible: .crucible/config.json: env CRUCIBLE_CODE_MOUSE_SCROLL_SPEED at line 3,
column 5 is not set to an answer crucible takes — accepted here: a whole number
of rows from 1 to 30
```

The floor is `1` because a wheel set to move nothing is a setting that looks
applied and does nothing. The ceiling is `30` because that is a screenful on
most terminals, and past it the wheel stops being a scroll and becomes a jump.

A run whose output is redirected has no wheel to answer, so the setting is read
and never used.

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

`env` is a block you key, so most of what goes in it is a name crucible has
never heard of and any string will do. The variables crucible reads for itself
are the exception: they are named in the schema beside that, with the number
each one falls back to and the range it takes, so an editor completes the value
and marks one out of range as you type.

A key that crucible answers for itself when no layer set it says so too, and an
editor fills that answer in. Every such key is one this page already documents
with the same word, because the two come from one declaration. A key with no
default carries none — a window worked out from the model, an effort the vendor
decides, a reserve derived from the window — since a default invented for the
schema would be a sentence about behaviour that nothing runs.

The schema is not fixed. Keys may be added, renamed or removed in any 0.x
release, and the URL above serves one copy — the newest release, not the version
you are running. An editor marking something red is worth a second look; the
program is what decides.

## When something is wrong

crucible stops before drawing anything and says which file, which key, where it
is, and what was accepted instead:

```
crucible: /home/you/api/.crucible/config.json: output.colour is not a setting
crucible has at line 3, column 5 — accepted here: color, theme, syntaxTheme,
glyphs, toolDetail

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
