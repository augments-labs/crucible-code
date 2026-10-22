# Installed coding agents

An entry under [`externalAgents`](../configuration/configuration.md#externalagents)
names a coding agent installed on this machine. crucible starts it as a child
process under the conversation's sandbox, speaks to it over its own wire, and
never reads or copies its credentials: the agent signs in and sends for itself.
A provider whose access names an agent has its conversations run by that agent
in place of its own models: no key for the provider is looked for, and the
agent is started by the conversation's first turn rather than when crucible
starts. An agent reaches only the provider it signs in to: Claude Code reaches
`anthropic` and Antigravity `google`, and an `access` that names an agent under
any other provider is refused at start. A disconnected agent, or one whose
program cannot be run, is refused before anything is recorded. The sections
below say how each agent is run.

The agent's program is looked at when it is selected, and again before each
start, without reading it. On Unix crucible compares its length,
modification time, device, inode and change time, so a rewrite, a replacement,
a `chmod`, a `chown` or a new hard link to it each count as a change. On
Windows it compares length and modification time alone, which a program
allowed to write the file can restore. A program that is no longer the one
selected is refused with a sentence naming it, and is selected anew when
crucible is started again.

`/login` in a conversation running on an agent offers all three choices. For
the provider the agent reaches, a subscription or key is stored, the
provider's `access` is rewritten from `externalAgent` to `subscription` or
`apiKey` and the provider is made the `provider` asked, with the default model
and rung a login takes where your configuration names no model for it; if that
cannot be written, the file is left as it was and the conversation stays on
the agent. The provider the agent reaches includes the one a connect during
this run moved the conversation off. Once written, and for a subscription or
key for a provider no agent reaches, the conversation moves off the agent onto
that provider at once, asking the model a login takes as
[Which model](providers.md#which-model) says, in a new session in the same
permission mode, with nothing carried over; its line says so, and the session
it left stays listed and can be resumed. Where that provider cannot be set
up, its line says why and the conversation stays on the agent; where the new
session cannot be started, or another agent reaches the provider, the
credential is only stored, and a rewritten `access` is read from the next
launch. *Use it* on a row already
holding its credential does the same with the one stored. `/logout` for the
provider the agent reaches removes the stored credential and says the agent
still reaches it. Logging out there of the provider the conversation was asking
before it moved onto the agent also takes the removed credential from what
`/resume` goes back to: a session that provider's model ran resumes on a
credential still left for it, else on the next launch that reaches a provider
directly.

`/login` always offers a third choice, *Use an installed coding agent*, where
this release ships an agent, whatever provider is being asked or none. It asks
which agent, says where the agent's program cannot be found, and asks once
before the agent is first started, saying that starting it runs its profile's
hooks, MCP servers and plugins. Where the agent reads signed in, nothing more
is asked: the agent's own provider is reached through it, made the `provider`
asked, and the conversation moves onto it. Where the agent's sign-in could not be
read, *Could not verify sign-in* gives one reason, the agent's own where it
said one: how it exited and the first line it printed, cut to 160 characters,
with its control characters taken out. *Check again* reads it again. Escape
changes nothing and goes back; apart from that one yes, nothing is written
until the agent reads signed in.

`/logout` lists each agent named under `externalAgents` after the stored
credentials, as an external agent instance, and `/logout <name>` takes one
directly. Taking one offers *Connect*, which reaches the agent's own provider
through it and moves the conversation onto it,
*Disconnect from Crucible* (where it is enabled), *Sign out of the agent* and
*Remove integration settings*; each then asks its own question, naming the
agent, its profile and the work it has running, and Escape there changes
nothing. Disconnecting, or signing out, writes the agent's `enabled: false`
and takes away the `access` of every provider reached through it, in one
update, so the next session asks that provider directly.

Removing stops the agent's run in the conversation first and then takes its
entry out of the user file; where the file cannot be written and the
conversation was on that agent, the line says the agent is stopped for this run
only and its settings could not be removed, and the entry stays, so the next
run starts the agent again. Where the conversation was on another route,
nothing was stopped and the line says only that the removal failed.

Where the file cannot be written for a disconnect, the line says the agent is
stopped for this run only and the disconnect could not be saved, if the
conversation was on that agent; the entry stays enabled, so the next run starts
the agent again. Where the conversation was on another route, nothing was
stopped and the line says only that the disconnect failed. A sign-out writes
every instance on the agent's profile, so it says the same stopped-for-this-run
line where the conversation was on any of them, and otherwise only that the
sign-out failed; the agent's own program is not run either way.

Connecting an agent moves the conversation onto it at once, whatever provider
it was asking, in a new session bound to it with nothing carried over, and the line it
ends on names the agent and says the conversation starts fresh there; the
session it left stays listed and can be resumed. In `/resume`, a session an
agent ran names the agent on its line. Where the agent cannot be started, the
conversation stays where it was and the next session starts on the agent.
Connecting the agent a conversation is already on leaves it there, and where it
was disconnected, its next turn starts the agent again.

While a conversation runs on an agent, the row under the box says, where the
provider and model would be, which agent runs its turns and who is paid for
them, and the model it is started on, such as `execution: work, model:
claude-opus-5-5 · billing: external`: the agent's own account pays, and
crucible does not know under which plan. `/model` offers every model of the
provider the agent reaches, from the same list crucible asks that provider
from, each with its rungs, and `/effort` the rungs of the model in force; what
is taken starts the agent's next run, and is written into its entry. A model
typed in is taken only where it is named with letters, digits and
`. _ - : @ / [ ]`, at most 128 of them and never starting with `-`, since the
agent's program is handed it as an argument; any other is refused. Beside
them `/model` offers the other providers' models as it does off an agent; one
taken, where a credential for its provider is in hand and no agent reaches
that provider, moves the conversation off the agent onto that provider and
model at once, in a new session, as a login for it would, and is written down
as the `provider`, model and rung asked; a rung that model does not serve is
refused and nothing moves. An agent that chooses its own model has
`/model` alone and `/effort` say so instead; a model named on the `/model` line
of a provider with a credential in hand moves the conversation off the agent
onto it, and one of the agent's own provider is refused.

A model taken of a provider whose `access` names an installed agent, while the
conversation runs on no agent that reaches that provider, moves the
conversation onto that agent at once, in a new session, the agent to be
started on that model on its first turn, with the rung asked, else the one in
force where the model serves it, else the model's default; both are written
into its entry. Its line says so, and the session it left stays listed and can
be resumed. Where that agent chooses its own model or is disconnected, or the
model is not named as above or does not serve the rung asked, its line says
why and nothing moves.

A conversation moved onto an agent by `/resume` of a session it ran, or by a
model taken as above, starts the agent with its next message. Where the agent's
route has no yes, that message asks about it first, as
[content use](content-use.md#an-installed-agent) says, and nothing is started
or sent before the yes.

The line above the box reads how much of the model's window the agent's
context holds, such as `41k/1.0M · 95% window left`: the window is the one
crucible records for the model the agent is started on, which nobody chooses
and `contextWindow` does not change, and what is held is what the agent last
said its own request carried, its answer included. A session just started
holds nothing; one resumed reads nothing until the agent's first answer. When
the agent makes room in its own context, the transcript says so with what it
held before and after, such as `the agent made room · 150k → 30k carried`, and
the reading drops to what was left. `/context` gives the same figure in its
`held by the agent` row.

## Claude Code

crucible speaks to Claude Code over its own stream protocol: the program is
started once for a run, printing and reading one JSON frame per line, and each
turn is one message on the same input, ended by the frame that answers it.

```json
{
  "externalAgents": {
    "work": {
      "driver": "claude",
      "executable": "/usr/local/bin/claude",
      "configDir": "/home/me/.claude-work"
    }
  }
}
```

**Which release.** Releases 2.1.277 and later before 2.2.0 are run. Its
version is read from `claude --version` once each time crucible connects to
it, and a release outside that range, or a program that does not say it is
Claude Code, is sent nothing more and refused with a sentence saying which. A
turn whose agent reports a release outside that range when it starts is
stopped too.

**Its profile.** `configDir` is optional: it is the directory the agent keeps
its sign-in, settings and sessions under, and without it the agent uses its
usual one under your home: the sign-in, hooks, MCP servers and plugins of the
Claude Code you run yourself. The agent is started with an environment of its
own: `PATH`, your home, user name, locale and time zone, and on Windows the
variables that locate the system, and nothing else from your shell, so an
Anthropic key or a cloud account your shell holds for another program never
reaches it and the account it uses is the one its profile is signed in to.
Since none of that environment says anything about an account, a path under
your home the agent names, in what it says or in what it asks to run, is shown
as that path. It is also told to send nothing it does not need to answer and
to leave its own installation alone.

**Its model.** `model` is optional: it names the model the agent is started
on, such as `claude-opus-5-5`, and it is passed to the program as its
`--model`. Without it the agent starts on Anthropic's default model in
crucible's list. Letters, digits and `. _ - : @ / [ ]` are accepted, up to 128
of them, and never a leading `-`. `effort` is optional too: `low`, `medium`,
`high`, `xhigh` or `max`, passed as its `--effort`, and without it the model's
default rung. `/model` and `/effort` write both.

**Signing in.** Whether it is signed in is read from its own
`claude auth status` in its profile. Signing in happens in the agent's own
terminal: crucible shows the command that signs the profile in, such as
`CLAUDE_CONFIG_DIR='/home/me/.claude-work' claude auth login`, for you to run,
then checks again. Signing out runs its own `claude auth logout`; without a
`configDir`, that signs out the Claude Code you run yourself too.

**Permissions.** The conversation's mode chooses the permission mode it is
started in: ask runs it in `default`, allow edits in `acceptEdits`, which edits
files without asking and asks before anything else, and full access in
`bypassPermissions`, which asks about nothing. Its other permission modes are
never used. A change of mode takes effect at the next turn, which starts the
agent again and continues its session; the turn under way keeps the mode it
began in. crucible's own rules are not consulted for the agent's actions, so
in full access a `deny` rule does not stop it; its sandbox still holds it in.
In ask and allow edits, every action its own rules leave to a prompt comes to
you as a question, which
names the agent that would act and how far what it runs is held in, such as
`execution: work · confinement: enforced`; a no refuses that action alone, and
the agent's turn goes on without it. Allowing for
the rest of the session keeps, for that session alone and never in its settings
files, only the agent's rules for the command you were asked about. A rule
without `:*` is kept only when it is that command itself. A rule ending in
`:*` is kept only when that command is the words before the `:*`, or begins
with them followed by a space, such as `git` or every `git` command for
`git:*`, but not `gitx status`. A rule is not kept for any tool other than
`Bash`. Apart from a final `:*`, a rule is not kept when the words it
names have a wildcard (`*`, `?`, `[` or `{`), no letter or digit, a space at
the start or end, or
any other whitespace, such as a tab (U+0009) or a no-break space (U+00A0): a
rule with a no-break space between `ls` and `-la` is dropped. A rule whose
words hold a byte-order mark (U+FEFF) is not kept either.
Shell syntax (``; & | < > ( ) $ ` " ' \ =``) is checked only in the
words before `:*`, so a rule for the commands that begin with words holding any
of it is not kept; the command itself is kept even when it holds shell syntax,
if it passes the checks above. Where the agent offers no such rule, or says its
rule would allow more than you were asked, the allow is for that call alone.
Once a turn is stopped, every question it asked is answered no, whatever was
chosen.

**Project settings.** The agent runs the hooks and helper commands and connects
the servers a project's own Claude Code settings name before it is asked
anything, as it does when you start it yourself in that directory, and those
settings can allow actions it would otherwise ask about. A turn is stopped,
saying why, when the agent reports that it sends requests with a key from its
settings, a project's or a helper command rather than the account its profile
is signed in to, or that it runs in a permission mode that asks less than the
one it was started in. A token or another provider a project's settings set in
the agent's environment is not reported, so read a project's Claude Code
settings before you start the agent in it.

**Usage.** Each turn reports its tokens, its input the sum of its uncached,
cache-read and cache-written parts, and its cost in US dollars as the agent
counts it. A run that resumes a session the agent kept reports no cost for its
first turn, since what the session cost before it is not known; after a turn
that reports its tokens and no cost, later turns of the run report no cost
either. The session's cost is the sum of what its turns reported: a turn that
reports none leaves it unpriced, and a stopped turn, which reports nothing,
leaves what was summed before it as the least the session cost, or the session
unpriced where nothing was.

**Network.** Its sandbox adds to what the conversation lets it reach
`api.anthropic.com`, `claude.ai`, `claude.com` and `platform.claude.com`, by
name, and no other host.

## Antigravity

crucible speaks to Antigravity's agent server over the Agent Client Protocol.

```json
{
  "externalAgents": {
    "agy": {
      "driver": "antigravity",
      "executable": "/home/me/.crucible/programs/antigravity/1.3.0/agy_acp_server.par",
      "profileDir": "/home/me/.crucible-antigravity"
    }
  }
}
```

**Which release.** Only release 1.3.0 of the agent server, which calls itself
`antigravity-acp`, is run, on macOS, Linux and Windows for `x86_64` and
`aarch64`. [`crucible agents install
antigravity`](../reference/cli.md#agents-install-driver) downloads it for your
platform, checks it is that release and puts it under `programs/antigravity/1.3.0`
in crucible's home. Where no entry names Antigravity, `/login` sets one up run
as that release, unless an `antigravity` command is on your `PATH`, and where
the release is not there yet it says to run that command and choose *Check
again*. An entry you write yourself names its executable,
`agy_acp_server.par`, or `agy_acp_server.exe` on Windows, with an absolute
path. When a later crucible runs another release, run the command again and
change the entry's `executable` to the path it prints. A release that describes itself otherwise
when it starts, or no longer offers loading and resuming a session, signing out
or its usual sign-in, is sent nothing more and refused with a sentence naming
what differs.

**Its model.** The agent chooses its own, and a `model` or an `effort` in its
entry is refused, since nothing would read it.

**Its profile.** `profileDir` is required: it is the directory the agent keeps
its sign-in, files and unpacked runtime under, and its usual home is the one
the Antigravity IDE signs in under. On macOS and Linux crucible makes it
private to you, readable by your user alone, each time the agent starts. The
agent is started with an environment of its own: `PATH`, your home, user name,
locale and time zone, and on Windows the variables that locate the system, and
nothing else from your shell, so a Google credential, project or key your
shell holds for another program never reaches it, and a path under your home
the agent names is shown as that path. Its temporary files go in the profile
too. Its sign-in token is kept as a file in the profile rather than
in the shared keyring, so a sign-in the Antigravity IDE or another Google
product keeps is never read as the agent's.

**Signing in.** Whether it is signed in is read from the agent's own answer
when a session is opened in its profile. To sign in, the agent prints a link to
Google's sign-in page and waits for the browser to come back to it; crucible
starts it with a browser that opens nothing, checks that the link is that page
and nothing longer than a link, and shows it as text for you to open. A link it
prints during a turn means the profile is signed out, and the turn fails
saying to sign in. Signing out runs the agent's own logout, then asks again
whether a session can be opened, and says the sign-out is unverified unless
the agent then answers that it is signed out.

**Its session.** Each time the agent is started again, after a failed turn, a
change of mode or `/resume`, it is asked to resume the session it last named.
Where it answers that it has no session by that name, that message fails with
what the agent said, and the next one starts it on a new session, which holds
nothing of what was said before; any other refusal leaves the session it named
to be asked for again.

**Permissions.** The agent asks about every action itself. In ask mode each
question comes to you; in allow edits crucible answers yes, for that action
alone, to a change, deletion or move of a file, and asks you the rest; in full
access it answers yes to every question, for that action alone. Where the agent
offers no yes for that action alone, the question comes to you whatever the
mode. crucible's own rules are not consulted for the agent's actions, and once
a turn is stopped nothing more is answered yes.

**Network.** Its sandbox adds to what the conversation lets it reach
`accounts.google.com`, `oauth2.googleapis.com`, `cloudcode-pa.googleapis.com`
and `daily-cloudcode-pa.googleapis.com`, by name, and no other Google host.
