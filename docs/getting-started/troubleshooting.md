# Troubleshooting

What to do when crucible did something you did not expect, looked up by what
you saw. Most entries quote the line the way crucible writes it, with the
parts that vary in angle brackets, and the rest name what you see instead. Each
says what it means and points at the page that explains it in depth.

A line that starts with `crucible: ` was written to standard error by a run
that then stopped, with a non-zero exit code. A line that starts with `!` sits
under a turn in the transcript, and the session carries on. A failed turn is
written on its own, in the trouble colour, and begins with the provider's name,
as in `anthropic: HTTP 401: ...`.

## Starting up

### `crucible: crucible has nowhere to keep its files: set HOME, or set CRUCIBLE_CODE_HOME to the absolute path of the directory you want it to use`

Neither `HOME` nor `CRUCIBLE_CODE_HOME` holds an absolute path, so there is
nowhere to look for the configuration file or to write a session. Export one
of them; a relative `CRUCIBLE_CODE_HOME` is ignored rather than resolved
against the directory you started in. See
[`CRUCIBLE_CODE_HOME`](../configuration/configuration.md#crucible_code_home)
and [where sessions are kept](../sessions/sessions.md#where-they-are-kept).

### `crucible: <file> is not valid JSON at line <n>, column <m>: <problem>`

One of the three configuration files does not parse, and crucible stops
before drawing anything. The same shape names a key it does not have,
`<file>: <key> is not a setting crucible has at line <n>, column <m>`,
followed by the keys accepted there, and a value it does not take,
`<file>: <key> does not accept <value> at line <n>, column <m>`. Open the
file at that line; `crucible config check` reads the three files the way a
start would and stops, printing `configuration valid` or
`configuration invalid` with a line per file saying `valid`, `invalid` or
`absent`. See [when something is
wrong](../configuration/configuration.md#when-something-is-wrong) and
[checking without
starting](../configuration/configuration.md#checking-without-starting).

### `crucible: <file>: <path> wants <kind> at line <n>, column <m>`

The file parses, and the value at `<path>` is the wrong kind of thing for
that key. `<kind>` says which kind: `a string`, `true or false`, `a whole
number that is not negative`, `a positive whole number within the documented
ceiling`, `one of a fixed set of strings`, `a bounded set of nonempty
strings`, `a whole number, or one written as a string`, `a list`, `an object` or `an
object of the extension's own settings`. One mistake that produces it is
quoting a value that is not text: `{"sandbox":{"enabled":"true"}}` gets
`sandbox.enabled wants true or false`, because `"true"` with quotes is a
string. A file that is not an object at the top gets `the document wants an
object`. The position is left off where the key's name occurs more than once
in the file. Write the value the way `<kind>` says; `crucible config check`
reads the files the same way. See [when something is
wrong](../configuration/configuration.md#when-something-is-wrong).

### `crucible: --model needs a provider before the slash, as in --model openai/gpt-5.6-terra`

`--model` was given a slash, which asks for the shape `provider/model`, and
nothing stood before it. Name the provider: `--model
anthropic/claude-fable-5-1`. A provider with nothing after the slash, `--model
openai/`, asks for that provider and leaves the model to
`providers.openai.model` in configuration. See [which
model](../providers/providers.md#which-model).

### `crucible: no provider called <name>; this build has <names>`

`--model`, or `provider` in a configuration file, named a provider this build
does not include, and the sentence lists the ones it does. Pick one of those.
A file written by a later crucible gets this sentence rather than a silent fall
back to whichever key is exported. See [which
provider](../providers/providers.md#which-provider).

### `crucible: no provider called qwen; this build has anthropic, google, moonshot, openai`

This is 0.43.3 or earlier started with no `--model` over a configuration file
in which 0.44 or later wrote `provider` as one of the providers 0.43.3 does not
have: `deepseek`, `meta`, `mimo`, `minimax`, `qwen`, `xai` or `zai`. `/model`
writes it when one of them is chosen, and so does a `/login` that sets the
session up. Before rolling back to
0.43.3, set `provider` in the configuration file in your home directory to
`anthropic`, `google`, `moonshot` or `openai`, or delete it. Keys and plan keys
stored for the new providers stay in `auth.json` under names 0.43.3 leaves
alone, and are used again by a later crucible.

## Keys and models

### `Warning: No models available. Use /login or set an API key environment variable. Then use /model to select a model.`

Said under the welcome, and again at a prompt, when no provider holds a
credential: none of `ANTHROPIC_API_KEY`, `DASHSCOPE_API_KEY`,
`DEEPSEEK_API_KEY`, `GEMINI_API_KEY`, `META_API_KEY`, `MIMO_API_KEY`,
`MINIMAX_API_KEY`, `MOONSHOT_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY` or
`ZAI_API_KEY` is exported with a value, and `/login` has stored nothing.
An exported variable that is empty holds no key. Everything but a turn works in
this state, so export a key or run `/login`, then `/model`.

Two neighbours say which half is missing. `Warning: No provider selected. Use
/model to select a provider and model.` means more than one provider has a
credential and nothing chose between them: `provider` in configuration or
`--model` does. A `provider` in configuration whose key cannot be found is left
unused, so this warning, or the one above, can appear with `provider` set.
`Warning: No model selected. Use /model to select the model to ask.` means a
provider is chosen and no model is named, by `--model` or by
`providers.<name>.model`.

Down a pipe there is nobody to type `/model`, so a run with no terminal ends
instead, with the same sentence and `No turn was taken.` after it. Give the
key and the model before running redirected. See [sign in or give it a
key](getting-started.md#sign-in-or-give-it-a-key), [which
provider](../providers/providers.md#which-provider) and [which
model](../providers/providers.md#which-model).

### `crucible: <VARIABLE> is not set`

`--model` named a provider, as in `--model anthropic/claude-fable-5-1`, and the
variable it reads its key from is unset or holds only whitespace, with no key
stored by `/login` to fall back on. A `provider` in configuration with no key
does not stop startup: the session opens with one of the warnings above, and
down a pipe the first prompt then ends the run. The line carries the variable's
name and never a value. Export it, or store a key with `/login`. Only the
chosen provider's variable is read, and `providers.<name>.apiKeyEnv` points a
provider at a different variable, in which case the usual one is not read at
all. See [keys](../providers/providers.md#keys) and [a key written down instead
of exported](../providers/providers.md#a-key-written-down-instead-of-exported).

### `! no credential is available for <provider>; use /login or set its API key variable`

`/model` named a provider, typed as `provider/model` or taken off the panel,
that is not the one answering and holds no credential: its variable is unset
or blank, and `/login` has stored nothing for it. Nothing changes; the
session keeps the provider and model it had. Its neighbours are said at other
moments. `Warning: No models available` is a warning at the welcome or a
prompt, about every provider at once; `crucible: <VARIABLE> is not set` stops
a start whose `--model` named the provider, and names the variable. This one
answers a command in a running session and names the provider. The same
sentence after `/login` means the credential just stored does not count: an
account login is not read for a provider with `providers.<name>.baseUrl`
set. Export the key, or store one with `/login`, then `/model` again. See
[keys](../providers/providers.md#keys) and [account login
today](../providers/providers.md#account-login-today).

### `crucible: providers.<name>.baseUrl: <address> is not an address crucible will send a key to: it must be https, or http on localhost, 127.0.0.1 or [::1]`

The address in `providers.<name>.baseUrl` is plain `http` on a host other than
the loopback ones, so the key would travel unencrypted. Two neighbours refuse
an address that `names no host to send to` and one where `the provider address
contains user information or a fragment`. A third,
`providers.<name>.baseUrl cannot be used with a subscription login; export an
API key to use that address`, says that an account signed in with `/login` is
fixed to the vendor's own address and a `baseUrl` needs an API key instead. Fix
the address, or take it out. See [keys](../providers/providers.md#keys) and
[account login today](../providers/providers.md#account-login-today).

### `crucible: <name>: <what the vendor's terms say> Nothing was sent; answer it once in a terminal.`

The route this run would send on, named as the question titles it, is one
whose vendor says it may use what is sent to train or improve its models,
nobody has said yes to it, and there is no
terminal to ask on: input or output is redirected. Nothing was sent. Start
crucible once in a terminal and send anything on that route: choose **Use it
anyway** and the answer is kept, so later runs, redirected or not, are not
asked. See [content use](../providers/content-use.md).

### `! this route needs an answer first; make the window taller and send again`

The panel asking about a route whose vendor uses what is sent did not fit the
window, so nothing was sent and your message is still in the prompt box. Make
the window taller and send it again. The same line ending `choose again` comes
from `/login` or `/model`, where nothing was chosen.

### `<provider>: nothing was sent: <route> waits for an answer`

A request was about to leave on a route whose vendor uses what is sent, before
that route had its yes. It was held, and nothing was sent. Send a message on
the route in a terminal to be asked. For a route named `model:<provider>/<model>`,
a web search names the model the session is asking now: to keep searches off
that model, choose another with `/model`. Or see [content
use](../providers/content-use.md).

### `<provider>: nothing was sent: the provider in force gives this session no web search`

The provider the session asks now has no web search for it (or no web fetch,
for the line ending that way): it serves none, or the credential it is set up
with cannot be used for one, such as a Kimi open platform key. `/model`,
`/login` and `/logout` can each leave the session there; signing out leaves no
provider at all, and the line then names `web` where a provider would be. The
call was refused before you were asked about it, and nothing was sent. Which
web tools a session offers is settled when it starts. Switch to a provider that
serves the tool, or see [reaching the web](../tools/web.md).

### `crucible: <home>/config.json: contentUse is not a setting crucible has at line <n>, column <m>`

This is 0.43.3 or earlier reading a configuration file a later crucible wrote a
yes into. Delete the `contentUse` block from the configuration file in your
home directory, then start the older crucible again. A later crucible asks the
question again the next time you send on such a route.

### `crucible: <home>/config.json: providers.openai.fast is not a setting crucible has at line <n>, column <m>`

This is 0.43.3 or earlier reading a configuration file a later crucible wrote a
speed into; the provider named may be another. Delete `"fast": true` from each
provider in the configuration file in your home directory, then start the older
crucible again. A later crucible asks at standard speed until `/fast` is chosen
again.

### `anthropic: HTTP 401: check the Anthropic API key and its model access`

The provider refused the request with that status. For Anthropic's
`claude-fable-5-1`, `claude-opus-5-5` and `claude-sonnet-5-5`, OpenAI's
`gpt-6-astra` and every Google model the message is a sentence of crucible's
own, chosen by status; for any other model, and for every other provider, it is
the service's own words, read for at most 8 KiB and
ending in ` [cut: the reply was longer than crucible reads]` or
` [cut: crucible stopped reading here]` where it was cut.

| Status | Sentence | Means |
| --- | --- | --- |
| 401, 403 | `check the <vendor> API key and its model access` (OpenAI: `check the OpenAI credential and model access`) | The key is wrong, or has no access to that model. |
| 404 | `check the <vendor> model name and endpoint` | The model name is not one the vendor serves, or `baseUrl` points somewhere else. |
| 408, 429, 5xx | `<vendor> is temporarily unable to serve this request` | The service is busy or broke; crucible asked again, twice at most, before saying so. |
| Any other | `check the <vendor> model and request settings; private response details omitted` (Anthropic: `check the Anthropic model, request settings and workspace retention; private response details omitted`) | The request was refused for something in it, such as a setting the model does not take. |

A 401 from MoonshotAI in its own words, with a key you know is good, is
usually a key from the other console: a Kimi Code key is accepted at
`https://api.kimi.com/coding/v1`, an Open Platform key only at
`https://api.moonshot.ai/v1`, which is set with `providers.moonshot.baseUrl` as
`https://api.moonshot.ai/v1/chat/completions`, the whole address requests are
posted to.
See [when a response goes
away](../providers/providers.md#when-a-response-goes-away) and [MoonshotAI
issues a key against one console or the
other](../providers/providers.md#moonshotai-issues-a-key-against-one-console-or-the-other).

The same holds for the other vendors that bind a key to a site. A Qwen or
MiniMax key is refused at the other site's address, and a Qwen plan's key
anywhere but its plan's address; a key from `DASHSCOPE_API_KEY` or
`MINIMAX_API_KEY` goes to the international site, and one from `ZAI_API_KEY` to
z.ai, so a bigmodel.cn key there may be refused too. Give a mainland China key
on its own row in `/login`, or set `baseUrl` to the whole address its site's
requests go to, ending `/chat/completions`. See [rows and
sites](../providers/providers.md#rows-and-sites).

### `anthropic: overloaded_error: Anthropic could not finish this request; private details omitted`

The request was accepted and the failure arrived inside the answer:
`overloaded_error`, `api_error`, `timeout_error` or `rate_limit_error`. The
sentence is crucible's own for `claude-fable-5-1`, `claude-opus-5-5` and
`claude-sonnet-5-5`; any other Anthropic model shows the service's words after
the kind, as in `anthropic: overloaded_error: Overloaded`. It is about the
moment, not the request, so crucible asked again twice, a quarter and then half a second later, with `retrying` in the row above
the box, before reporting it. Ask again; nothing about the prompt needs to
change. See [when a response goes
away](../providers/providers.md#when-a-response-goes-away).

### `anthropic: unexpected response: Anthropic reported a message failure; private details omitted`

The other failure `claude-fable-5-1`, `claude-opus-5-5` and `claude-sonnet-5-5`
report from inside an answer: Anthropic put an error in the stream whose kind
is none of the four above. crucible keeps neither the kind nor the words,
because a response from one of these can carry the model's private history, and it does not ask again: a kind outside those four
is read as being about the request rather than the moment, so the same
request would get the same answer. Any other Anthropic model shows every
failure inside an answer as `anthropic: <kind>: <words>`, and is asked again.
It is about the request as sent, so change something in it, the prompt, the
effort or the model, before deciding it is the service. See [when a response
goes away](../providers/providers.md#when-a-response-goes-away).

### `! the session no longer fits this model's window; try /compact`

The provider refused the request as larger than the model's window, so asking
again unchanged cannot work. `/compact` replaces the middle of the transcript
with a written summary under fixed headings and keeps the most recent turns
word for word, which is what crucible does on its own when it sees the window
filling. See [when the window
fills](../sessions/sessions.md#when-the-window-fills).

### `! unfinished: the answer reached the token ceiling`

The answer ended for a reason other than being finished, and one of these lines
says which: the token ceiling, `the provider's filter cut the answer short`,
`the provider paused this turn; ask it to go on`, or `the provider stopped for
a reason this build does not know`. A narrower question fixes the first,
asking for less does not help the second, the same prompt again carries on from
the third, and the last is a stop crucible could not name, so asking again is
what there is to try. `! stopped` is a turn you ended with <kbd>Esc</kbd>. See
[when an answer stops early](getting-started.md#when-an-answer-stops-early).

### `■ Usage limit reached · weekly window · resets Mon 09:00`

The plan behind the ChatGPT sign-in, a Kimi Code sign-in or key, or a MiniMax
Token Plan key, is
used up for that window, and the turn ended there. The window is one of the plan's own or one it keeps for the model
you are using; another model's spent window does not stop this one. The line
under it says which way: `The turn stopped before sending` where what crucible
last read put a window at 100% with its reset still ahead, so nothing went out;
`The vendor refused the request` where the vendor said so itself. Either way
nothing in the transcript was lost and the refusal is not asked again, since
asking before the reset reaches the same answer. Send a prompt once the window
starts again; `/usage` shows the window at 100% and when it resets, in your
local time. `resets soon` is a reset the clock has reached, and
`resets: not reported` is a refusal that named none, so send later. Prompts you
queued behind that turn are not sent on to the spent plan: they stay queued over
the box, where <kbd>Ctrl+Q</kbd> opens them to edit or delete, and follow the
next prompt you send. The mark is `#` where the glyphs are ASCII.

## The network and proxies

### `anthropic: TLS setup failed`

The certificate the host, or an `https://` proxy, presented was not trusted.
crucible trusts the Mozilla root certificates built into it and no others: not
the operating system's store, not `SSL_CERT_FILE`, and there is no setting for
one. On a network that inspects TLS by re-signing it, as some corporate proxies
do, every request fails this way, `/login` says account login could not reach
the authorization service, and the release check finds nothing. A proxy that
passes the tunnel through untouched works. See
[certificates](../providers/network.md#certificates).

### `anthropic: host was not found`

The provider's hostname did not resolve, or, behind a proxy, the proxy's did.
A direct lookup has five seconds; one that takes longer is reported as
`hostname resolution stalled; restart crucible before trying another provider
request`, and every later lookup for a turn fails at once until crucible is
restarted, because the operating system's lookup cannot be cancelled. Check
the hostname in `baseUrl` or in the proxy variable, and start crucible again
after a stall. See [how long it
waits](../providers/network.md#how-long-it-waits).

### `anthropic: connection failed`

No connection could be made to the host or to the proxy, the proxy refused the
tunnel (a wrong or missing password, or a provider hostname it could not
resolve), or the proxy is one crucible refuses: `socks4a://` and `socks5h://`
addresses fail every request that would go through them. The proxy is read from
the first of `ALL_PROXY`, `all_proxy`, `HTTPS_PROXY`, `https_proxy`,
`HTTP_PROXY` and `http_proxy` that holds an address, in the environment
crucible was started in, so set it in your shell, not in the `env` block.
`NO_PROXY` names the hosts that go direct, and nothing goes direct unless it is
listed, `localhost` included. See [through a
proxy](../providers/network.md#through-a-proxy) and [hosts that skip the
proxy](../providers/network.md#hosts-that-skip-the-proxy).

### `anthropic: request timed out`

One of the waits ran out: 15 seconds to make the connection, including the
lookup, the proxy's tunnel and TLS; a minute to send the request; a minute for
the answer to start. A failure about the moment is asked again, twice at most,
before it is reported. See [how long it
waits](../providers/network.md#how-long-it-waits) and the table under [when a
request fails](../providers/network.md#when-a-request-fails).

## Sessions

### `crucible: no earlier session for <directory>`

`--continue` picks up the most recent session started in the current
directory, and nothing has been recorded there. Run it from the directory the
session was started in, or start a new session without the flag. See
[continuing](../sessions/sessions.md#continuing).

### `crucible: <log> is open in another crucible`

The session `--continue` asked for is still being written by another crucible,
and continuing it would cut the log back underneath that one. Nothing was read
or changed. Finish or close the other run first, or start a session of your
own: two crucibles in one directory are two sessions. The claim is a `.lock`
file beside the log, released however the process ends, so a crash leaves
nothing stuck. See [one at a time](../sessions/sessions.md#one-at-a-time).

### `crucible: no session <id> in this workspace`

`--resume` named an id that no session in this directory was recorded under,
or one that is not an id at all. The id is the one the parting message printed
and `/resume` takes; it is refused by name rather than matched to the nearest
one. Check it, and check that you are in the directory it was started in. See
[picking one by name](../sessions/sessions.md#picking-one-by-name).

### `crucible: <log> was written by a different version of crucible`

The log was written by a newer build, or under a format whose lines no longer
mean what this build would read them as, and it is refused rather than
continued half-understood. The file is left whole, and is still yours to read;
start a new session here. See [stability](../sessions/sessions.md#stability).

### `! this session has stopped being recorded: <os error>`

A write to the session log failed, most often for a full disk, and the turn
went on without it. It is said once. What reached the disk before it is still
there, and a later write that succeeds still lands, so freeing the space is
enough; nothing has to be restarted. See [when recording
stops](../sessions/sessions.md#when-recording-stops).

## Permissions and the sandbox

### `<tool> was not allowed`

A permission question was answered no, and the turn ended there: under the
call, the result row reads `the user did not allow this`, and this line
stands on its own, the way a failed turn does. In a run with no terminal at
one end, which reads whole lines, the question is written into the transcript
as `? <tool> wants to run: <command>` (or `wants to change: <path>`, `wants
to read: <path>`, `wants to reach <host>: <what>`) with `[y]es  [s]ession
[n]o` under it, and the next line of input is its answer: `y` or `yes` allows
it once, `s` or `session` for the rest of the session, and anything else is
no. When input has ended, a prompt piped in or a closed terminal, there is no
next line, and a question nobody can answer settles as a denial. There is no
deny-by-default mode to select; that is what asking means with nobody there.
A run that must proceed on its own says so with `allow`
[rules](../permissions/rules.md) or with `fullAccess`. A `deny` rule is
different: the model is told `permission policy does not allow this; asking
again will not change it` and the turn goes on. See [when nobody can
answer](../permissions/modes.md#when-nobody-can-answer).

### `sandbox unchanged: sandbox backend unavailable: <reason>`

`/sandbox enable` checks that this machine can enforce confinement before it
writes anything, and it could not. On Linux the reason names what the check
found among the `bwrap` executables on `PATH`. `no suitable system Bubblewrap
was found outside writable roots; bundled backend unavailable` means none was
found, or every one sat under the current directory or a directory the
sandbox lets a command write. `system Bubblewrap does not expose the required
confinement options; descriptor binds and temporary overlays need Bubblewrap
0.11.0 or newer` is decided by the options `bwrap --help` lists, not by the
version; Ubuntu 24.04 ships 0.9.0, which has the binds but not the overlays.
For either, install Bubblewrap 0.11.0 or newer from the system's packages,
with `crucible-sandbox-broker` beside `crucible`. The two reasons below have
entries of their own, and others name a step of the same check, such as
`system Bubblewrap probe timed out`. On macOS the built-in `sandbox-exec` does
the confining and only the broker is needed; on Windows run `.\crucible.exe
sandbox setup` once from an Administrator PowerShell. `crucible --sandbox`
prints the backend and what a command would run under without running one.
See [turning it on](../security/sandboxing.md#turning-it-on) and [platform
support](../security/sandboxing.md#platform-support).

### `sandbox unchanged: sandbox backend unavailable: system Bubblewrap or its parent path is not root-owned and non-writable`

The `bwrap` found is not trusted: it is not a plain file owned by root, or a
directory above it is not, or one of them can be written by its group or by
others, or the file is larger than 64 MiB. Ownership is checked before the
version, so a Bubblewrap built in your home directory is refused however new
it is. Install the distribution's package, which puts a root-owned `bwrap`
under a root-owned path. A neighbour, `system Bubblewrap returned an invalid
version`, means the trusted one answered `bwrap --version` with something
other than `bubblewrap <x>.<y>.<z>`. See [how Linux and macOS confine a
command](../security/sandboxing.md#how-linux-and-macos-confine-a-command).

### `sandbox unchanged: sandbox backend unavailable: system Bubblewrap cannot create the required namespaces on this host`

Bubblewrap was found, trusted and has the options, and a trial run failed:
crucible asks it to run `/bin/true` inside the user, PID, IPC, network and
UTS namespaces confinement needs, and it exited with an error within three
seconds. That is the host rather than the version: a kernel or a policy that
withholds unprivileged user namespaces answers this way. crucible discards
what Bubblewrap printed, so to read its own words run the same trial in a
shell:

```sh
bwrap --die-with-parent --new-session --unshare-user --unshare-pid --unshare-ipc --unshare-net --unshare-uts --disable-userns --ro-bind / / --dev /dev --proc /proc --cap-drop ALL --clearenv -- /bin/true
```

What it says is what the host has to be given, and that is a change to the
machine's settings rather than to crucible's. Until then confinement stays
off here. See [platform support](../security/sandboxing.md#platform-support).

### `bash: could not prepare operating-system confinement: sandbox backend unavailable: <reason>`

Confinement is on and this machine cannot enforce it, so the command was
refused rather than run unconfined. The reason is one of those above. Give the
platform what it needs, or turn confinement off with `/sandbox disable` or
`{"sandbox":{"enabled":false}}` in `~/.crucible/config.json`; a project whose
configuration requires it refuses that with `sandbox unchanged: project
configuration requires confinement`. There is no enforcing backend on FreeBSD.
See [turning it on](../security/sandboxing.md#turning-it-on) and [unconfined
execution](../security/sandboxing.md#unconfined-execution).

## The terminal display

### Everything is drawn in one colour

`output.color` is `auto` unless you set it, and `auto` writes colour only when
the output is a terminal and `NO_COLOR` is unset or empty. Unset `NO_COLOR`, or
set `{"output":{"color":"always"}}` in configuration, which writes colour on a
terminal even with `NO_COLOR` set; `never` turns it off even on a terminal. A
file or pipe never gets colour, whatever `output.color` says. Even with
`always`, `TERM` decides how much colour there is: `TERM=dumb`, or no `TERM` at
all, means none, unless `COLORTERM` is `truecolor` or `24bit`. See
[`output`](../configuration/configuration.md#output).

### There is no prompt box, and the mode is written in front of each line

Input or output is redirected, as in `crucible < prompts.txt`. That is the
redirected run rather than a fault: lines are read whole, one prompt each, and
the mode is written in front of them because there is no row under a box to
show it. With no colour to read them into, the emphasis markers the model wrote
are left in the text, which is what makes `crucible < prompts.txt > answers.md`
a file of markdown. See [run it](getting-started.md#run-it).

### `crucible: what you typed is longer than 1 MiB; no prompt was accepted`

A line of redirected input was longer than 1 MiB, which is as much as one
prompt is held to, and the run stopped without taking it. Split it, or name
the file in the prompt instead of pasting its contents: a path to a picture or
a PDF goes with the prompt, and any other path is a word the `read` tool opens
when the model asks for it. See [naming a file in the
prompt](getting-started.md#naming-a-file-in-the-prompt).
