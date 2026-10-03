# Configuration

crucible reads JSON. Every file is optional: most machines have none of them
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

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/settings-dark.svg">
  <img alt="Where a setting comes from, farthest layer first: your own ~/.crucible/config.json, then the project's .crucible/config.json and .crucible/config.local.json, then the command line, and the nearest layer wins. The two project files may set ordinary settings and tighten permissions, but never loosen permissions, choose a provider, credential or server, set an environment variable outside crucible's own names, or name a prompt-cache namespace; a scalar takes the nearest layer that set it, an object is merged key by key, and most lists add up." src="../assets/settings-light.svg" width="720">
</picture>

Three, read in this order. Nearer to the work wins.

| File | Holds | Checked in? |
| --- | --- | --- |
| `~/.crucible/config.json` | what you want everywhere | no, it is yours |
| `.crucible/config.json` | what this project needs | yes; everyone who clones gets it |
| `.crucible/config.local.json` | non-authority overrides for this checkout | by convention only; [gitignore it](#the-workspace-files) |

The two project files are looked for in the directory you started crucible in,
which is what makes a project's settings a property of the checkout rather than
of the shell that launched it.

The command line is a fourth layer and is nearer than all three: `--model
openai/gpt-5.6-terra` wins over anything a file says.

When `/model`, `/effort`, `/fast`, `/login`, `/theme`, `/settings`, `/sandbox
enable` or `disable`, answering **Use it anyway**, removing a credential, or telling
`/resume` to stop asking about a large session changes the user file, crucible
prepares an owner-only sibling and replaces the complete document atomically. A failed
write before that commit leaves the previous file whole. An owner-only lock
spans the bounded reread through the commit, so simultaneous crucible processes
cannot silently lose one another's settings.

`/settings` lists, on its Config tab, every setting that is a switch, a short
list of choices or the mouse scroll speed, and changes one in the user file.
The theme, syntax theme, glyphs, tool detail, scroll rail, scroll speed and the
key that sends change at once. Colour, tone, compaction and the four prompt
caching rows say `applies at next start`, and the update check is read at the
next start anyway, so it says nothing. A row a project file or the environment
sets is shown with who set it and cannot be changed there, since the user file
would not win. A project file that says anything about `promptCaching` sets
all four of its rows, because that block is checked as a whole. Providers,
credentials, MCP servers, extensions, the system prompt, permission rules and
the sandbox's policy are not rows; the sandbox and the permission mode are
shown on the Status tab and changed with `/sandbox` and `/mode`.

A file that is not there is not an error. A file that *is* there and will not
open is, and says so. Silently skipping it would turn a permissions mistake
into settings that mysteriously stopped applying. Anything there that is not an
ordinary file, such as a pipe, is refused with the file named rather than
waited on. Each file is limited to 1 MiB before JSON parsing, so a checkout
cannot choose an unbounded startup allocation.

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

Keyed by provider name: `anthropic`, `deepseek`, `google`, `meta`, `mimo`,
`minimax`, `moonshot`, `openai`, `qwen`, `xai`, `zai`.

| Key | Means |
| --- | --- |
| `model` | The model to ask when `--model` does not name one. |
| `effort` | How hard to think before answering, when `--effort` does not say. |
| `fast` | `true` asks the `model` beside it for its vendor's fast form, at its price. |
| `apiKeyEnv` | The name of the environment variable holding that provider's key. |
| `baseUrl` | Where to send that provider's requests instead of the vendor's. |
| `contextWindow` | The session's context-window size in tokens, keyed by model name. |
| `defaultContextWindow` | The same, for any model of this provider not named above. |

`effort` is one of `low`, `medium`, `high`, `xhigh` or `max`, and it is set per
provider because which rungs exist is the vendor's business. A rung chosen for
the one serving it says nothing about the one that would refuse it. Left out,
crucible asks for no rung at all and the vendor's own default for that model
applies. See [Providers and models](../providers/providers.md).

`fast` is written by `/fast` and read only from the configuration file in your
home directory, because it costs more on every request. It reaches a request
only where the file's `model` is the model in force, that model has a fast form
and no `baseUrl` is set; see
[Fast](../providers/fast.md).

`apiKeyEnv` takes a **name**, never a key. The credential wiring reads its value
at startup and does not copy it into a document, diagnostic or session message.
Pointing crucible at another variable also points it away from the usual one:
with `"apiKeyEnv": "WORK_ANTHROPIC_KEY"`, `ANTHROPIC_API_KEY` is not read at
all. Choosing an arbitrary inherited secret is authority, so `apiKeyEnv` is
read only from the configuration file in your home directory.

`baseUrl` is for a gateway or a proxy speaking the same protocol. It must be
`https`, or `http` on `localhost`, `127.0.0.1` or `[::1]`. The key travels in a
header on every request, so the address decides who receives it, and plain
`http` to anywhere else is that key on somebody's network in the clear. For the
same reason it is one of the keys [workspace files](#the-workspace-files) may
not set.

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

Without either setting, a model crucible knows starts at its native context
window: 1,000,000 tokens for Claude Fable, Opus and Sonnet, DeepSeek,
MiniMax-M3, Qwen and Z.ai; 1,048,576 for Gemini, Muse Spark, MiMo and Kimi K3;
500,000 for Grok; 262,144 for Kimi K3-256k and the Kimi coding models; 204,800
for MiniMax-M2.7; and 200,000 for Claude Haiku 4.5. OpenAI's models start at
872,000, the most its ChatGPT sign-in manages against, because the window is
chosen before crucible knows whether a key or a sign-in answers. A model name
crucible has no record of starts at 200,000 tokens, or 272,000 for OpenAI and
262,144 for Moonshot.

A session may grow to its window before it is compacted, and every turn sends
all of it, so a larger window costs more per turn. Gemini, xAI and OpenAI with
a key also charge more per token once a request passes a threshold of their
own. Use `defaultContextWindow` to lower the
window for every model of one provider, or `contextWindow` for a named model, as
in the example above; either can also raise a window past these figures.
Neither is sent anywhere. The configured/default value is the window; the existing
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
opinion about, and they are [sent as facts](../sessions/context.md) of their
own.

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
else. `never` does not disable `/compact` (that is you asking rather than
crucible deciding); it means a turn that runs out of room fails instead of
recovering.

`keep` is in tokens rather than turns because a turn can be enormous: the kept
tail has to fit the window beside the recap, and only a figure in the window's
own unit can promise that. The turn you are in is kept whole, whatever it has
cost so far; the budget bounds the turns before it. Here, a turn begins at the
latest message on your side of the conversation, so a line you queued while it
ran begins a new one, and so does the note crucible adds when a background
command finishes. The exception is automatic recovery with nothing older to
recap. Once this turn has finished a tool call, if the turns before it already
fit the budget and the window is still full after old tool output is cleared,
the whole conversation is recapped, this turn included. A line you queued
while that call ran is left out of the recap and kept word for word, along with
any lines queued after it. Left unset, crucible keeps the most recent 20,000
tokens.

`recap` is a ceiling rather than a requested length. Left unset, a structured
recap may produce up to 10,240 tokens, further limited by the model's output
ceiling and the room safely available in its window. A recap that reaches its
token ceiling or omits a required section replaces nothing.

`reserve` is worked out from the model if you do not set it: enough for one
answer of the length crucible asks for, plus the tool results a pass carries
back. **Raising what an answer may be raises the reserve**, because a request
and its answer have to fit the window together, so a larger answer ceiling
means compacting sooner, not later. The reserve is never more than half the
window, so a small model still has half of itself to work in.

`askOnResume` is a number of tokens, 60,000 when no layer sets it, and `0`
means never ask, which is what the *stop asking* answer writes down. See
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
| `requestedRetention` | Optional provider-neutral `class` and hard `maxSeconds` ceiling. The default class is `providerDefault`. |
| `persistentResources.mode` | Separately managed remote resources are `forbid`, `reuse`, `create`, or `require`. The default is `forbid`. |
| `namespace` | A user-owned identity label of 1 to 64 ASCII letters, digits, `.`, `-` or `_`; it is never copied directly into a provider cache key. |

`observeOnly` adds no Crucible cache controls, although a provider may still
cache automatically and report that usage. `require` fails before sending if no
reviewed eligible mechanism can be selected. `prohibit` also fails before
sending unless the provider exposes a documented opt-out.

`providerDefault` requests no retention override and takes no `maxSeconds`.
`ephemeral` and `extended` require a `maxSeconds` from 1 to 31,536,000 (a
year); the figure is a ceiling rather than an exact TTL promise. Extended retention, resource creation, broad isolation and a
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

With `enabled: true`, every requested hard boundary must be
enforced before a command starts. It never silently falls back to an ordinary
subprocess. Linux uses Bubblewrap 0.11.0 or newer with the required features.
macOS uses the built-in Seatbelt framework through the fixed system
`/usr/bin/sandbox-exec` launcher. Native Windows uses a dedicated account,
path-capability ACLs, WFP network denial, a restricted token, private desktop,
exact inherited handles and a Job Object after one explicit Administrator
setup. See [turning it on](../security/sandboxing.md#turning-it-on) for each platform.

`enabled` controls OS confinement. An unavailable backend or an unsupported
requested boundary prevents the command from starting.

Only your home configuration may disable confinement. Either project file may
set `enabled: true`, which strengthens the user choice regardless of document
order. Project `enabled: false` is refused. A project, tool, extension, skill,
agent, or descendant cannot weaken confinement chosen above it.

Configure the boundary in the same block:

```json
{
  "sandbox": {
    "enabled": true,
    "filesystem": {
      "writable": [],
      "readOnly": [],
      "unreadable": [],
      "protected": []
    },
    "network": {
      "allowedDomains": [],
      "deniedDomains": [],
      "allowLocalBinding": false,
      "allowUnixSockets": []
    },
    "limits": {
      "commandSeconds": 1200,
      "outputBytes": 10485760,
      "concurrentCommands": 4
    }
  }
}
```

Empty filesystem lists keep the standard workspace and system-runtime policy.
Linux/WSL2 and macOS support every setting above. Native Windows supports
`enabled`, writable/read-only/protected paths, all three command limits and
`/sandbox`, with the [documented native ACL limitations](../security/sandboxing.md#windows-setup-maintenance).
Windows refuses nonempty `unreadable` paths, domain policies, local binding and
Unix-socket grants before starting a command. Empty network lists and
`allowLocalBinding: false` retain its closed network policy. Use Crucible inside
WSL2 when those richer policies are needed on a Windows machine; it uses the
Linux backend. An unsupported setting never causes an unconfined retry.

Relative paths are resolved from the workspace root; absolute paths use the
current platform's spelling. Paths do not expand environment variables or `~`,
and parent traversal (`..`) is rejected.

| Filesystem key | Effect |
| --- | --- |
| `writable` | Grants additional read and write access. Only your home configuration may set it. |
| `readOnly` | Grants read access in home configuration, or removes existing write access in a project file. |
| `unreadable` | Denies reading and writing the named path and its descendants. |
| `protected` | Keeps a path readable while preventing writes, including through a nested writable grant. |

A project file can restrict inherited access, but cannot grant a new readable
or writable root. Unreadable and protected paths cannot be reopened by another
grant. Repository control metadata and agent configuration directories stay
protected within additional writable roots too. This includes version-control
metadata and recognized agent settings, rules and skills.
Each path is limited to 4096 bytes and the effective filesystem policy to 128
rules.

Network access is closed unless granted. `allowedDomains` and `deniedDomains`
accept hostnames, IP literals, `*.example.com` or `*`, without a URL scheme,
path or port. A wildcard domain matches subdomains; name the base domain
separately when it is needed. Denies take precedence. Each list has at most 64
entries.

Authorized outbound TCP goes through a per-command HTTP/CONNECT proxy. A host
grant allows its nonzero TCP ports; it does not inspect encrypted tunnel
contents. Tools must use the supplied proxy environment. Direct outbound TCP,
UDP and application DNS do not acquire that grant. The host resolves permitted
names, checks the resulting addresses before connecting, and evaluates each
new proxy request independently. Private, loopback and metadata addresses need
an explicit IP-literal grant; a wildcard or a public hostname resolving to a
private address does not grant access by itself. A permitted connection goes
through the `http://` proxy crucible's own environment names, if any ([commands
connect on their own](../providers/network.md#commands-connect-on-their-own)).

`allowLocalBinding`, `false` by default, allows local listeners; it does not
grant host egress or publish a Linux namespace port onto the host. On macOS a
local listener may also bind a wildcard address, so use it only when a listener
is intended.
`allowUnixSockets` grants connections to exact existing native Unix socket
paths, with no wildcard matching. It does not grant Windows named pipes.
Granting a privileged service socket grants access to that service's protocol.

Only your home configuration can introduce domain grants, local binding or
Unix socket access. Project allowlists and socket lists must be subsets of
inherited grants; an explicit empty list removes those grants. Project denies
accumulate, and a project can set local binding to `false`.

`commandSeconds` limits wall time, including background commands, and defaults
to 1200 seconds. `outputBytes` limits combined stdout and stderr to 10 MiB by
default; reaching it stops the process scope. `concurrentCommands` defaults to
4 and bounds active command slots shared by Bash and sandboxed MCP processes.
The respective maximum values are 86400, 67108864 and 16; zero is invalid.
Projects and individual command requests may lower limits but cannot raise an
inherited ceiling. These guards apply even with OS confinement disabled.

Use `/sandbox` during a conversation to inspect the effective boundary or
switch enablement. The Dependencies tab reports the platform prerequisites;
arrow keys navigate, Enter selects and Esc closes. `/sandbox enable` and
`/sandbox disable` make the same persistent home-configuration change. A
project-required sandbox cannot be disabled. The panel is available between
turns; existing background commands keep their original policy. A failed update
keeps the previous effective setting.

When enabled, Linux applies an hour of processor time per process and a
4096-open-file ceiling. macOS applies the open-file ceiling; Darwin's catchable
CPU signal is not advertised as a hard limit. Windows applies an hour of
aggregate Job processor time and has no handle-count ceiling. These ceilings
are not a budget you are meant to work within. They are not configurable, and
a command may narrow a supported ceiling but not drop it.

With confinement disabled, commands retain guardrails, deadlines, output bounds,
usage and audit records, but `crucible --sandbox` says `confined  no` and the
session records `confined: false`.
The confinement-only resource ceilings do not apply. Permission approval and a
worktree are not a sandbox.

`crucible --sandbox` prints what a command in the directory you are standing in
would actually run under, and stops without running one. The report says which
backend enforces it, what that backend can and cannot hold, the reach and
ceilings a command would get, and anything given up along the way. See
[Operating-system confinement](../security/sandboxing.md) for the exact backend
capability matrix, lifecycle, inspection and failure behavior, and for how to
read that report.

### `permissions`

What runs without asking, what is refused outright, and what happens to
everything else. See [Permissions](../permissions/permissions.md) for the model;
this is the key reference.

| Key | Answers | Means |
| --- | --- | --- |
| `mode` | `ask`, `allowEdits`, `fullAccess` | What happens to a call no rule mentions. The default is `ask`. |
| `allow` | a list of rules | Runs without asking. |
| `ask` | a list of rules | Put to you, whatever the mode says. |
| `deny` | a list of rules | Refused, in every mode. |
| `extraDirectories` | a list of absolute paths | Directories outside the working directory that tools may reach. |

A rule is a tool name and what it may act on: `read(src/**)`,
`bash(cargo test)`. A tool name on its own, or `bash(*)`, is everything that
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
open a line under the one you are typing, as does a backslash on the end of the
line you are on, which asks the terminal for nothing at all. That is what almost
every terminal makes possible and it is what the prompt does out of the box.

Set it to `altEnter` and the two swap: Return opens a line and a modified Return
sends. That is the answer for a terminal that keeps Shift+Return for itself and
never forwards it: you press Return for as many lines as you want, then
Alt+Return to send. Ctrl+J sends too, because a terminal has always spelled it
the same way as the other modified Returns.

Control and Return is not on the list and cannot be. A terminal that has not
agreed to the newer keyboard protocol sends exactly the same bytes for it as for
Return alone, so nothing here could tell them apart and choosing it would leave
you with no way to send at all.

### `output`

| Key | Answers | Means |
| --- | --- | --- |
| `color` | `auto`, `always`, `never` | Whether to write colour; `auto` by default. `auto` follows the terminal and `NO_COLOR`; `always` writes colour on a terminal even when `NO_COLOR` is set, and `never` writes none. Output that is not a terminal gets no colour whatever this says, and the model's markdown is kept as written. A `TERM` of `dumb`, or no `TERM` at all, still gets no colour, even under `always`, unless `COLORTERM` says `truecolor` or `24bit`. |
| `glyphs` | `unicode`, `ascii` | Which characters crucible draws with; `unicode` by default. `ascii` if box drawing shows as hollow squares. |
| `theme` | `auto`, `dark`, `light`, `colourblind-dark`, `colourblind-light`, `ansi` | Which colours crucible draws with; `auto` by default. |
| `syntaxTheme` | a theme name | Which theme fenced code is drawn in; `Monokai Extended` by default. |
| `toolDetail` | `compact`, `full` | The width of compact tool headings and result previews: a readable measure, or the whole window; `compact` by default. Clipped details remain expandable: recent ones from memory, older ones read back from the session log when the view reaches them, where the session has a log. |
| `scrollRail` | `true`, `false` | Whether the transcript has a one-column scroll rail on its right edge; `true` by default. The rail shows which part of the transcript is on screen and a mark at each prompt; a click off the thumb scrolls there, a drag on its thumb scrolls with the pointer, and a click on a mark lands on that prompt. The prompt you are reading under has a larger mark, and a pointer on the rail lights the track and marks and enlarges the mark under it. Text wraps one column narrower while it is drawn, and a window too narrow to spare the column does not draw it. `false` gives the column back. |
| `screen` | `fullscreen`, `native` | Where crucible draws; `fullscreen` by default, and read only at start. `fullscreen` takes a screen of its own, with its own scrollback, scroll rail and selection. `native` draws in your terminal's own buffer: what is finished is written once into the terminal's scrollback, only the part still changing at the foot is drawn again, and scrolling, selection, search and copy are your terminal's. The scroll rail, the mouse scroll speed and crucible's own selection are off there. `/clear` and `/resume` leave the earlier transcript in the terminal's scrollback. On a terminal that does not rewrap its lines when the window narrows, narrowing it can take a few finished lines off the visible screen; the session file still has them. |

`theme` is a table of what each colour on screen means, tuned to one background.
`auto` asks the terminal what its background is and picks the dark or the light
table from the answer, which is the setting to leave alone unless you have a
reason: it is the only one that keeps being right when you change your terminal.
The two `colourblind` tables move the diff off the red-green axis (a line put
in goes blue and a line taken out goes amber), and `ansi` spends nothing but the
sixteen colours your terminal already has, so your own terminal theme decides
every hue.

Every table spends colour the same way. A line has at most one thing in the
accent, the one your eye should land on: the selected row, a key that opens
something, the rule that opens a panel. A frame in the accent, such as the one
around a question, is not counted against the lines inside it: its edges are
the frame, and each line between them still has one accent at most. In what a
model says and in its tool calls, the theme's colour goes on inline code and
links alone, the things you copy or follow; panels, notes, `/help` names,
`/release-notes` versions and the startup banner keep it. Headings, bold, a
table's header, a tool's name and the figures of a count are bold in your own
foreground, a tool call's mark is your own foreground, and the row your prompt
is left on carries no colour beyond its background. Colour that means
something (a line added or taken out, success, trouble, a mode that lets
crucible act without asking) means it in every table, and everything else is
your own foreground or a quieter grey. Nothing is said by colour alone: every
accent is also a mark or a place on the line, and every meaning also has a sign
or a word, so a `colourblind` table, or `color` set to `never`, loses nothing.
With no colour, an answer keeps the markdown markers it was written with.

`/theme` picks one at the prompt and writes it here. It draws a diff and a
prompt row under the list in whatever your mark is standing on, because a theme
is a list of colours and nobody can picture one from its name.

One thing is not in any table: the row your own prompt is left on takes a
background blended off your terminal's, a fixed step lighter on a dark one and
darker on a light one, so it cannot fight a terminal theme crucible has not
seen. Most terminals will not say what their background is (the question is not
widely implemented), and there the step is taken off the background the table in
force is drawn for instead, which is the same assumption every other colour on
screen is already making.

`syntaxTheme` is a separate answer because it is a separate question. The theme
above decides the interface: borders, marks, the mode in force, the ground a
diff takes. This decides what a fenced block of code in an answer looks like,
and the two are chosen together on `/theme`, one axis each.

The names are the ones you already have an opinion about: Monokai Extended,
GitHub, Dracula, Nord, gruvbox, Solarized, one-half and the rest. `/theme` lists
every one of them.

A block is read only where its fence named a language crucible knows: ```` ```rust ````
rather than a bare ```` ``` ````. One that named nothing, or named something it does
not know, is drawn exactly as it was before any of this existed: quiet and
whole. TypeScript is read as the JavaScript it extends, so a type annotation is
drawn as ordinary words.

`glyphs` is asked rather than detected. A hollow square where a border should be
is a font missing that character, and nothing about that reaches crucible. The
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
| The corner its result hangs under | `⎿` | `+` |
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

Hold **Shift** while you drag and the selection is your terminal's own again.
Every terminal keeps Shift as the way past a program holding the pointer, which
is the answer for a reader who wanted their emulator's selection rather than
this one.

With `output.screen` set to `native`, crucible does not hold the mouse at all:
the wheel, a drag and a click are your terminal's, and every key works as it
does on a screen of crucible's own.

### `updates`

| Key | Means |
| --- | --- |
| `check` | `auto` to find out when a newer release exists, `never` to leave the network alone. |

```json
{ "updates": { "check": "never" } }
```

`auto` is the default. A turn, `/login` and `/compact` all reach the network
because you asked; this is the only time crucible decides to on its own. At most
once a day, on a thread of its own, it asks GitHub which release is newest and
writes the answer to `~/.crucible/release`; nothing waits for it, so the answer
is drawn under the welcome the *next* time you start. No part of your session,
your directory or your configuration is sent. The request is a plain GET for
the repository's latest release, carrying a user agent that names crucible and
its version.

`never` stops the asking. crucible then never contacts GitHub, and never says
anything about releases.

### `contentUse`

| Key | Means |
| --- | --- |
| `accepted` | The routes you have said yes to sending on, though their vendor says it may use what is sent to train or improve its models. |

```json
{ "contentUse": { "accepted": ["key:google"] } }
```

crucible writes a route here when you choose **Use it anyway**, and takes it
out when a credential of that route is removed or replaced; you rarely write it
by hand. A name this build has no route for means nothing. Read only from your
home file. See [content use](../providers/content-use.md) for the routes and
what each vendor says.

### `env`

Environment variables for the commands crucible runs (the bash tool's children)
and the place crucible's own settings are written, under names that begin with
`CRUCIBLE_CODE_`. crucible does not put a variable in its own environment,
because writing to it is `unsafe` in a process with threads. It reads its own
names from the block as settings, and one set in the shell you start crucible in
still wins.

```json
{ "env": { "RUST_LOG": "warn", "PAGER": "cat" } }
```

Values are strings, because that is what an environment holds. A setting that
reads as a number is written `"12"`; `CRUCIBLE_CODE_MOUSE_SCROLL_SPEED` also
takes the integer `12`.

A command is **not** started with the environment crucible was started in. It
gets a short list of what a program needs in order to run at all, and whatever
`env` adds on top:

- On Unix: `PATH`, `HOME`, `TERM`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`.
- On Windows: `PATH`, `PATHEXT`, `COMSPEC`, `SystemRoot`, `SystemDrive`,
  `windir`, `TEMP`, `TMP`, `TERM`, `HOME`, `USERPROFILE`, `HOMEDRIVE`,
  `HOMEPATH`, `APPDATA`, `LOCALAPPDATA`, `ProgramFiles`, `ProgramFiles(x86)`,
  `ProgramData`.

Confinement changes a few of these. On Linux the command gets `HOME` set to
`/crucible-home` and `TMPDIR` set to `/tmp`, both inside the sandbox, and never
gets `SSH_AUTH_SOCK` or `GPG_AGENT_INFO`. On macOS `TMPDIR` is a private
directory crucible owns and removes with the command. When native Windows
confinement is enabled, the backend replaces `TEMP` and `TMP` the same way.
Where the network is granted, crucible sets `HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY` and their lowercase forms to its own proxy, and `NO_PROXY` and
`no_proxy` to nothing, after `env`, so these names in `env` do not reach a
confined command.

Everything else stops here, and your provider key is why. `env` and `printenv`
are ordinary things for a model to run, and what a command prints comes back as
tool output: onto your screen, into the next request, and into the session log.
The list says what to keep rather than what to drop, because `apiKeyEnv` takes a
name: a key can be called anything, so a list of the names keys usually have
would cover exactly the names somebody thought of.

A name written in `env` beats the inherited one, so `"PATH"` there replaces what
crucible was started with rather than adding to it.

A command that needs anything else (a `CARGO_TARGET_DIR`, a token a deploy
script reads) is told about it here, which is you handing it over on purpose.

## The workspace files

`.crucible/config.json` is checked in, while `.crucible/config.local.json` is
ignored only by convention. A repository can commit either filename, so
crucible refuses an arbitrary `env` variable in both:

```
crucible: /home/you/api/.crucible/config.json: env cannot set TOKEN at line 3,
column 5 — crucible cannot tell a file you wrote from one that arrived with the
checkout, so no file under the working directory sets a variable for the
commands crucible runs — PATH alone decides which program each of those
commands is. Only crucible's own settings, which start with CRUCIBLE_CODE_, are read from
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
those is not arbitrary: it is a knob crucible declares and whose meaning
crucible fixes. So a project may set one for everybody who clones it, and that
is still not a way to ship somebody's key.

The same refusal covers every key that could loosen what crucible does unasked:
`permissions.mode`, `permissions.allow`, `permissions.extraDirectories`,
`systemPrompt.custom`, `providers.<name>.apiKeyEnv`, `providers.<name>.baseUrl`,
`providers.<name>.fast`, `provider`, `promptCaching.namespace`,
`contentUse.accepted`, and `sandbox.enabled` set to `false`. Everything after
the three `permissions` keys is not a permission, and is here for the same
reason: these replace the instructions that say to ask, choose which credential
is read and who receives it, spend more of your money on every request, decide
which cached prompts may be shared, answer for you whether a vendor that trains
on what is sent may be sent anything, or take away the sandbox, and nothing on
those paths stops to ask. Each is read only from your home file and refused in
both files under the workspace. The `extensions` and `mcp` blocks are refused
there too; their own sections below say so.

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
first number is not the one this build speaks, so the two programs disagree
about the shape of what crosses the wire. Every release so far that reads
extensions speaks protocol 1, so no other version hosts such an extension:

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
you say it under its own identifier, the `id` its manifest states, which is
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
  may run   no; the manifest has changed since it was agreed to at sha256:810cb273aa0d388bf206a0685138577efc74b078759f6921d166539580d61e16
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
Any JSON goes in (strings, numbers, lists, blocks inside blocks), and the only
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

Crucible reads 64 directories. A directory holding more than that is refused
whole and nothing in it is listed, because the 64 a sweep would reach first are
whichever ones the filesystem handed back, so a list built from them could name
different extensions on the next run:

```
/home/you/.crucible/extensions was not read

1 directory could not be read:
  /home/you/.crucible/extensions holds more than 64 installed directories, so none of them were read — move what is not an extension out of it
```

Two directories claiming one `id` are not both kept. The first in sorted order
keeps the identifier and the second is listed as refused, because the identifier
is what everything else would key on.

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
the two characters that qualification is spelled with; a server called `a/b`
would produce tool names nobody could read back to a server.

Writing a record starts nothing. It is a statement that a server exists and how
it would be launched; what launches one is a selection made per run, and the
selection is `--with-mcp`:

```bash
crucible --with-mcp docs
```

Repeat the flag for each server you want. A run that names none starts none,
which is every run that does not type it. Twenty servers written down and no
flag is twenty processes that do not exist. A name nothing wrote down stops the
run rather than being quietly left out, because a turn missing the tools you
asked for reads as a model that will not do the work.

The servers a turn hosts are started when it begins and stopped when it ends,
and each is asked once what it offers. What comes back is named under the server
it came from (the `docs` server's `search` tool is `mcp:docs/search`), so
nothing a server offers can take over a name crucible already uses.

A server is somebody else's program, so it runs confined the way a command run
through `bash` does, under the same `sandbox.enabled` choice. It starts in the workspace
unless the record names a `directory`, which is then a root it may write in as
well as the place it starts. That path does not have to be inside the workspace
and is not checked against it: naming one widens what the server may reach, and
it is a key only your own configuration file may write. And it is given exactly
the variables `env` and `envFrom` name and nothing else: a server inherits
none of crucible's own environment.

`command` is the only key a record cannot do without, and it is either an
absolute path or a bare name for `PATH` to answer. Anything in between, such as
`./server` or `bin/server`, is refused, because it would be resolved against
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

`env` holds values and is applied verbatim, so nothing secret belongs in it: a
configuration file is a file, and a value written there is a value on disk.
`envFrom` is the key for a secret: it holds *names* on both sides. `"DOCS_TOKEN":
"MY_DOCS_TOKEN"` means the server is given `DOCS_TOKEN` set to whatever crucible
was itself started with in `MY_DOCS_TOKEN`. The token never appears in a
document, a session file or a log line, which is the same bargain `apiKeyEnv`
makes for provider keys.

What the server says back does not carry the value either. Crucible shows every
occurrence of it, overlapping ones included, as `*`, one per byte, in what the
server writes on standard error and in the words of every reply it decodes from
standard output and keeps: tool results, error messages, and the names,
descriptions and schemas of the tools it offers. A reply is decoded first, so
an echo the server's JSON escaped is found too, and the protocol around the
words is read as it was sent: no frame is rewritten, and every number crucible
reads, such as a call's `id`, is read as sent whatever the value is. The one
number kept as words, the spelling of an `id` crucible could not have issued,
is hidden like the rest of the words it keeps. A number crucible keeps is shown
as sent too (an error's code, a number in a tool's schema, the `id` of an
answer to a call it was not waiting on), so a value a server repeats as a
number reaches the model there, which is one more reason such a value belongs
in `env`. A server whose tool schema would have two member names of one object
alike once the value is hidden in them is refused, as a server offering two
tools under one name is. Not caught: an echo cut short or split into pieces,
except a value split across two blocks of one result where it has a line break;
a value the server transforms before saying it; and anything it writes into a
file under a writable root. A short or ordinary value still belongs in `env`,
since an `envFrom` value of `1` shows every `1` in the server's own words as
`*`. Each `envFrom` entry also counts its credential handle, 5 to 7 bytes,
toward the 128 KiB limit on the server's environment, as every credential
handed to a sandboxed command does.

The rest of the record is the timing and failure behaviour, and every one of
them has an answer already:

| Key | Default | What it decides |
| --- | --- | --- |
| `handshakeSeconds` | `10` | How long to wait for the server to agree a protocol version, and for each step of starting its sandbox |
| `requestSeconds` | `60` | How long to wait for one request |
| `shutdownSeconds` | `5` | How long the server is given to stop before it is killed |
| `restarts` | `0` | How many times it may be started again after it ends |
| `required` | `false` | Whether a run that selected it fails when it cannot be prepared, rather than carrying on without its tools |

`requestSeconds` is how long a server that says nothing is given, not how long
an interrupt takes. Pressing escape before a call reaches the server refuses it
there and then. Pressing it during a call ends the wait at the press: the call
comes back cancelled immediately, whatever `requestSeconds` is set to. What the
press cannot do is reach the server: the request has gone, the tool may be
running, and from crucible's side a tool that never started, one that finished,
and one whose answer was lost look the same. So an interrupted server is
finished with for the rest of that turn rather than asked a second question it
would answer with the first one's reply. Set `requestSeconds` to what you are
willing to wait for a server that has stopped answering.

`restarts` is a ceiling on the endings crucible can prove were harmless, not a
retry count. A server whose process had already gone when crucible tried to
write the call left the far end untouched, so it is started again and the same
call sent once; that is what the number is spent on. Every other ending has a
request outstanding, and no number makes repeating it safe: those end the server
for the turn whatever the ceiling says. A server started again has to come back
offering the tool under the same name and the same schema, because the
description the model wrote its arguments against is the one this run published;
a catalogue that moved retires the server instead. The default of `0` is one
start and no more. Replacement also requires confirmed cleanup of the old
process scope. If cleanup cannot be confirmed, disposal keeps reporting the
failure and that toolset refuses another preparation; repeating disposal does
not turn an uncertain stop into success. This also applies to missing pipes,
failed handshakes and invalid catalogues during startup. Even a server with
`required: false` ends preparation when its cleanup cannot be confirmed; an
ordinary startup refusal with confirmed cleanup can still be skipped.

Every key in `mcp.servers` is read **only** from `~/.crucible/config.json`. A
committed `.crucible/config.json` naming a server would be choosing whose
program runs (and what it is told, and what it is started with) on behalf of
whoever cloned the checkout, before anything has been typed:

```
crucible: /home/you/api/.crucible/config.json: mcp.servers cannot be set here at
line 3, column 5 — this file is inside the workspace and can arrive with a
checkout, and this key only ever widens what crucible does without asking. A
workspace file may tighten its own rules — permissions.ask and permissions.deny
— and may not loosen anybody's. Put this one in the configuration file in your
home directory
```

Crucible reads 64 servers, and 256 arguments and 256 variables per record. A
block holding more than one of those is refused by name and line when the file
is read, rather than accepted and then shortened. A server started with the
first 256 of the 300 arguments you wrote is running a command you did not
write, and would say nothing about the ones it dropped.

## `CRUCIBLE_CODE_HOME`

Moves crucible's whole directory: the configuration file and the session logs
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

How many rows or list entries one notch of the wheel moves wherever wheel
scrolling is enabled: the transcript, expanded tool results, background-command
output, and the session picker's list and preview. The default is `6`; arrow
keys still move one step at a time.

```json
{ "env": { "CRUCIBLE_CODE_MOUSE_SCROLL_SPEED": 12 } }
```

The value may be a JSON integer, as above, or a string such as `"12"`.

Written in `env` like any other variable, so it layers like one: a project can
set it for everybody who clones the repository, your home directory can set it
for every project, and the environment you start crucible in beats both.

```console
$ CRUCIBLE_CODE_MOUSE_SCROLL_SPEED=3 crucible
```

A whole number from `3` to `30`, written as decimal digits only: no sign, no
leading zero and no surrounding space, so `+6`, `06` and `" 6"` are refused as
well as `1`, `2` and `31`. The shell variable is read the same way as a string in
the file. Anything else is refused rather than rounded into range or ignored:

```
crucible: .crucible/config.json: env CRUCIBLE_CODE_MOUSE_SCROLL_SPEED at line 3,
column 5 is not set to an answer crucible takes — accepted here: a whole number
of rows from 3 to 30
```

The floor is `3`, and a value below it is refused rather than pulled up to it,
because a setting pulled to another number looks applied and does something
other than what was written. The ceiling is `30` because that is a screenful on
most terminals, and past it the wheel stops being a scroll and becomes a jump.

A run whose output is redirected has no wheel to answer, so the setting is read
and never used.

## How layers combine

A **scalar** takes the nearest layer that set it. An **object** is merged key
by key, so a project naming one provider leaves your other one alone. A
**list** is usually concatenated: every layer's entries are kept and none of
them replaces another. Two things narrow instead. A project's sandbox network
allowlist (`allowedDomains`) and socket list (`allowUnixSockets`) replace the
ones it inherits and may only narrow them, as the [`sandbox`](#sandbox) section
describes. A project's `promptCaching.allowedMechanisms` is intersected with
the list above it, so it can only remove mechanisms.

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
  "permissions": { "deny": ["edit(.git/**)"] } }
```

In that project: `openai` asks for `gpt-5.6-sol`, `anthropic` still asks for
`claude-opus-5`, `toolDetail` is still `full`, and both `deny` rules are in
force.

Concatenation is the only rule the permission lists could have. If a nearer
layer replaced a farther one, a `.crucible/config.json` that mentions `deny` at
all would silently drop every `deny` you wrote at home, and a checked-out
repository would be deciding what your own machine protects. Keeping both is
safe precisely because `deny` wins wherever it came from.

The cost is that a nearer layer cannot shorten a concatenated list, only add to
it. Removing an entry means editing the file that holds it.

## Comments

JSON has no comment syntax, which is the one real cost of the format. `$comment`
is the standard's own answer to it, and crucible takes it anywhere in a
document (at the top, beside a rule list, inside a provider block) and does
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
default carries none (a window worked out from the model, an effort the vendor
decides, a reserve derived from the window), since a default invented for the
schema would be a sentence about behaviour that nothing runs.

The schema is not fixed. Keys may be added, renamed or removed in any 0.x
release, and the URL above serves one copy: the newest release, not the version
you are running. An editor marking something red is worth a second look; the
program is what decides.

## When something is wrong

crucible stops before drawing anything and says which file, which key, where it
is, and what was accepted instead:

```
crucible: /home/you/api/.crucible/config.json: output.colour is not a setting
crucible has at line 3, column 5 — accepted here: color, theme, syntaxTheme,
glyphs, toolDetail, scrollRail, screen

crucible: /home/you/api/.crucible/config.json: output.color does not accept
beige at line 3, column 5 — accepted here: auto, always, never

crucible: /home/you/api/.crucible/config.json: output.color wants one of a
fixed set of strings at line 3, column 5

crucible: /home/you/.crucible/config.json is not valid JSON at line 2,
column 14: key must be a string

crucible: /home/you/.crucible/config.json: permissions.allow[1] at line 3,
column 5 — read(src is not a rule; a rule names a tool and what it may act on,
like read(src/**)

crucible: /home/you/.crucible/config.json: permissions.extraDirectories[0]
must be an absolute path at line 3, column 5 —
../shared is relative, and a configuration file cannot know what it would be
relative to
```

The last two are from the home file because neither key may be written in a
project file at all: there, either one is refused as `cannot be set here`,
before its value is read.

An entry in a list is named by the index it sits at and located at the key
holding the list, because an entry has no key of its own to search the file
for, and the other `"read(src/**)"` further down would be a perfectly correct
line to be sent to.

Where a key appears more than once in the file, the position is left off rather
than pointing at one of them, which would send you to a line that is correct.

An error may name an environment variable. It never quotes the value beside it.

### Checking without starting

`crucible config check` reads the three files the way a startup would, resolves
them the way it would, and stops. It opens no credential, starts no session,
and launches or dials nothing, so it is safe to run when one of those is the
suspect:

```
$ crucible config check
configuration invalid
  user config /home/you/.crucible/config.json: valid
  project config /home/you/api/.crucible/config.json: invalid
  project-local config /home/you/api/.crucible/config.local.json: absent
  /home/you/api/.crucible/config.json: output.color does not accept beige at
  line 3, column 5 — accepted here: auto, always, never
  schema: https://www.schemastore.org/crucible-code-schema.json
```

Each file is `valid`, `invalid` or `absent`, and each error is the one a
startup would stop on, including two layers whose rules contradict each other.
It does not look up a provider's name or check a `baseUrl` address; a start
refuses an unknown provider, and an address that is neither `https` nor `http`
on `localhost`, `127.0.0.1` or `[::1]`, so a file that passes here can still
stop one.
It exits 0 when everything holds and 1 otherwise, repeating the first error on
standard error. `--json` prints one JSON document instead, with the same
`status`, `files`, `failures` and `schema`. Neither report carries a secret; a
path, a rule or a rejected value an error quotes appears as it does above.
