# Providers and models

`--model` takes a model name, optionally qualified by the provider serving it.

```bash
crucible --model claude-sonnet-5      # the provider whose key this machine holds
crucible --model openai/gpt-5.6-terra # openai
crucible --model openai/              # openai, asking for the model it is configured with
crucible                              # both halves left to the machine and your configuration
```

Only the **first** slash divides the two halves, so a model name that contains
slashes of its own stays intact: `openai/meta/llama-4` asks the `openai`
provider for `meta/llama-4`.

## Which provider

**No provider is written into the build**, and none of these rungs is a guess:

1. `--model provider/model`, where it names one outright.
2. `provider` in your [configuration](../configuration/configuration.md), when
   that provider still has a usable credential. This is the only setting that
   remembers a vendor choice.
3. Exactly one provider having a usable credential: a stored account login, a
   stored API key, or a key in one of the variables in [Keys](#keys). That is
   the absence of a choice to make, and it lets a first run work with one
   credential and nothing configured.

A credential says a provider **can be reached**, and never which to ask. Which
variable your shell happens to carry is a fact about that shell, and a turn sent
to the wrong vendor is billed there and leaves your prompt behind — so no
credential outranks another, and no order between vendors is written down
anywhere in crucible.

A variable exported empty holds no key, so it does not compete: a shell carrying
`ANTHROPIC_API_KEY=` alongside a real `OPENAI_API_KEY` asks OpenAI. A provider
pointed at another variable by `apiKeyEnv` is looked for under that name.

Several authenticated providers and nothing choosing between them is a question
rather than a coin toss. crucible starts without selecting one and says:

```
Warning: No provider selected. Use /model to select a provider and model.
```

A model written under a provider does not answer it. `providers.openai.model`
says what to ask OpenAI *for* — it is not a way of saying to ask OpenAI, and
reading it as one is how a machine holding two keys used to end up at whichever
vendor a model had been chosen for weeks earlier.

A name this build has nothing for is refused the same way there as on the flag,
so a file written by a later crucible is a sentence rather than a silent fall
back to whichever key is exported.

A remembered provider, model and effort become dormant when their credential
is removed or its environment variable is unset. crucible still opens with no
active provider, model or effort so `/login` and `/model` remain available; it
does not silently fall through to a different provider whose credential happens
to be present.

Whichever rung settled it, the answer is at the right of the row under the
prompt box, which names the vendor before the model in the `provider/model`
shape `--model` takes back. That row stands for the whole session and is said
again whenever one of the three changes, so it keeps up when `/model` hands the
session to another vendor mid-way. The welcome card deliberately carries no provider, model or
effort because it is the first thing in the transcript and is scrolled away from
rather than kept up to date.

## Which model

There is **no model built in**, and none of these rungs is a guess:

1. `--model`, where it names one.
2. `providers.<name>.model` in your
   [configuration](../configuration/configuration.md), for the provider being
   asked. A provider and a bare slash — `--model openai/` — is how you reach
   this rung with the flag present.
3. Nothing. crucible starts anyway and says so under the welcome — naming
   whichever half of setting it up is still missing:

```
Warning: No model selected. Use /model to select the model to ask.
```

```
Warning: No models available. Use /login or set an API key environment
variable. Then use /model to select a model.
```

Everything except taking a turn works in that state, which is what leaves
somewhere to type the answer. Down a pipe there is nobody to type it, so a
prompt arriving there cannot be answered and the run ends non-zero rather than
reading every remaining line and answering none of them.

`/model <name>` asks for that model from the next
turn on and writes it to `~/.crucible/config.json` under the provider this run
is set up for, so the next run starts with it. It writes `provider` beside it, so
the next run asks the same vendor rather than settling that question again from
whichever keys the shell is carrying. `/model` on its own stands a shelf over
the whole shell: a search line across the top, every provider this build serves
in one pane beside the models in the other, and the rungs the marked model takes
on a strip underneath, under the name of the one being asked now. Typing narrows
both panes at once, against a model's name or a provider's — somebody who types
`openai` wants everything that vendor serves, somebody who types `sonnet` wants
the one model, and neither should have to say which kind of name they just
typed. Tab crosses between the panes, the up and down arrows walk whichever one
the mark is in, the left and right arrows walk the rungs, Enter takes the model
and the rung under it together, and Escape leaves everything as it was. A mouse
lights the row it is over, across the whole width of that pane; passing over a
row chooses nothing, and clicking one puts the mark on it. Clicking a model the
mark is already on takes it, so a double click picks one outright — and a
provider is never taken that way, because it narrows the models beside it rather
than being an answer itself. The rung stays on the arrow keys, which is what
keeps taking the model and saying how hard it should think one visit. Down a
pipe, where nobody can walk a shelf, it writes the models out as the line that
asks for each.

Taking a row off the models pane moves the session to whoever serves it first —
a model belongs to the vendor that serves it, and the two change together. The
rung goes with them, because a rung is asked of a model: choosing one and then
being sent somewhere else to say how hard it should think is the same question
put twice. A model whose vendor serves no rung is taken with the rung left
exactly as it was, and its row says so. A model the shelf does not carry is
still named — what is offered is a shortcut past the vendor's documentation, and
the vendor remains the authority on what it serves.

A model belongs to the provider serving it. crucible never writes a name under
one provider and sends it to another — the pairing is settled once, by
[Which provider](#which-provider), and the model rungs above are all read for
that same provider.

Naming a provider this build does not have is a startup failure that says which
ones it has:

```
crucible: no provider called gemini; this build has anthropic, moonshot, openai
```

## How hard to think

Models that reason before answering take a rung saying how much of that to do.
crucible's rungs are `low`, `medium`, `high`, `xhigh` and `max`, and they mean
the same thing whichever provider a session is on:

1. `--effort`, where it names one.
2. `providers.<name>.effort` in your
   [configuration](../configuration/configuration.md), for the provider being
   asked.
3. Nothing. crucible asks for no rung, and the vendor's own default for that
   model is what applies.

```bash
crucible --effort max
```

The bottom rung is not a default in disguise. Which rungs a model serves — and
whether it takes one at all — is decided by its vendor and differs between
models of the same vendor, so a rung crucible chose on your behalf would reach
models that refuse the field outright. Naming one for a model that does not take
it is refused by the vendor rather than dropped here, the same bargain a model
name is already on.

A word that is not a rung is refused before anything is drawn:

```
crucible: no effort called maximum; crucible takes low, medium, high, xhigh, max
```

Where a rung was chosen, the row under the prompt box says so after the model.
Where none was, nothing is drawn in its place: the rung in force is then the
vendor's, and crucible is never told which it picked.

Mid-session, `/effort` stands a ladder over the rungs the model in force serves
and `/effort <rung>` takes one outright. The ladder is a track with the rungs
written under it,
`Faster` at one end and `Smarter` at the other; the left and right arrows move
the mark, Enter takes what is under it and Escape leaves it. The same rungs are
on the strip beneath the shelf `/model` stands, so a session settling both at
once settles them in one visit, and `/effort` is the way to change the rung
without touching the model. It is stood over a
model by name and asks which model is being asked first, since a rung is one
word in one request and what it buys is that model's to say. Either way it applies from the next turn on and is written to
`~/.crucible/config.json` beside the model, so the next run here asks for the
same. There is no way back to asking for nothing from inside a session — a rung
you can see on the screen cannot be un-seen by being handed a default this
program is never told the name of. Remove the key from the file for that.

### Asking crucible what it is

Both answers are told to the model before every turn, so asking a session which
model it is and how hard it is thinking gets what is actually on the request:

```
› what model are you?

crucible, asking claude-opus-5 at max effort.
```

Neither is something a model can find out for itself. Its own name it would
answer from training — which is whatever was true when it was trained, and is
wrong the moment `/model` changes it — and the rung is a field on a request it
never sees. Both are read off the session again before each turn rather than
written down once, so the answer keeps up with `/model` and `/effort` instead of
describing the session the first turn was taken in.

Where no rung was named, what is said is that the vendor's own default applies —
crucible is never told which rung that is, and neither is the model.

### The ladder holds what the model serves

Which rungs a model takes is written down beside its name in the shelf `/model`
stands, so the ladder is the model's rather than crucible's: `moonshot/k3` gets
three rungs, `openai/gpt-5.5` gets four, and a model whose vendor serves none is
told so instead of being offered a ladder that cannot be answered. A rung that
is missing is missing rather than drawn and greyed — a row the arrows have to
step over is a row worth not drawing.

That list is read off each vendor's documentation and goes stale between
releases, so nothing is narrowed except the offer. `--effort` and
`/effort <rung>` go to the vendor whatever this build has written down, and a
model it has never heard of — one released since, or one typed rather than
picked — is offered all five. What a stale entry costs is a missing row in a
panel, never a refusal from the program that is not the one serving the model.

## Keys

A key is read at startup and goes no further than the header it signs a request
with. Which variable is read follows from the provider:

| Provider | Variable | Sent as |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | `x-api-key` |
| `moonshot` | `MOONSHOT_API_KEY` | `authorization: Bearer …` |
| `openai` | `OPENAI_API_KEY` | `authorization: Bearer …` |

Only the chosen provider's variable is read. Running `crucible --model
openai/gpt-5.6-terra` needs `OPENAI_API_KEY` set and does not care whether
`ANTHROPIC_API_KEY` is.

A configuration file can point a provider at a different variable, which is what
a second key for the same vendor needs:

```json
{ "providers": { "anthropic": { "apiKeyEnv": "WORK_ANTHROPIC_KEY" } } }
```

That is a variable **name**, and pointing crucible at one points it away from
the other: `ANTHROPIC_API_KEY` is then not read at all.

A custom `baseUrl` must use HTTPS unless it is the exact loopback host
`localhost`, `127.0.0.1` or `[::1]`. User information and fragments are refused,
and diagnostics show the recipient but redact the path and query because those
parts often contain tenant identifiers or tokens. Authenticated model requests
never follow redirects; the provider receives the 3xx refusal instead.

A refused response body is read for at most ten seconds and 8 KiB. That deadline
is elapsed time for the whole body, including bytes a slow peer continues to
trickle between waits.

A key never appears in a log line, an error message, a session file, or
anything crucible prints. If you see one, that is a bug worth
[reporting privately](../../SECURITY.md).

### A key written down instead of exported

The other place crucible looks is `~/.crucible/auth.json`. The auth directory,
store, partial write and lock are created owner-only on Unix and with a
protected user access-control list on Windows; existing permissions are
tightened before a credential is read. A key kept there is set up once and
needs nothing from your shell afterwards:

```json
{
  "version": 2,
  "keys": { "openai": "sk-…" },
  "subscriptions": {},
  "identities": {}
}
```

`/login <provider>` inside a session is the direct API-key route. It writes
from a box that draws a dot per character rather than the key. `/login` on its
own offers the account plans and a Console account; the console route then
opens the provider list and the same box.

The command reports that the key was stored, not that it was verified. Provider
authentication is established by the next request; a rejected key stays stored
until `/logout <provider>` removes it.

The session is then set up with that provider from the next turn on, without
restarting — unless another provider is already answering, in which case the
session keeps the provider and model it has and the line points at `/model`,
where switching is chosen rather than implied. Authentication selects neither
[model](#which-model) nor [effort](#how-hard-to-think); where neither was
already chosen, `/model` is the next explicit step.

Where the login is what set the session up, `provider` is written down for it
too, because a credential says a vendor can be reached and never which to ask —
logging in is somebody saying which, and the next run here should not have to
be asked again. That still selects neither a model nor an effort.

Where no variable above is set and that file names nobody, the warning under the
welcome names this command, and the prompt is there underneath it as usual.

`/logout <provider>` removes that provider's stored account or API key, and
`/logout` on its own offers every stored credential crucible can remove.
Editing the file by hand works too — crucible only reads what is there, and a
name under `keys` that this build does not serve is left alone rather than
offered for removal.

The names under `keys` are provider names, the same ones `--model openai/…`
takes. `version` says which crucible wrote the file, so one from a later version
is left alone rather than guessed at.

For API-key authentication, **the variable wins over a stored API key**. It is
the key chosen for this process — a second account, a work key, one rotated an
hour ago — while the stored key is the standing answer underneath it, so
`OPENAI_API_KEY=` turns off the *variable* and leaves the file's key doing its
job. A deliberately authorized subscription account wins instead, at that
provider's fixed account endpoint, so an inherited key cannot silently switch
plan usage to API billing. A custom `baseUrl` is an API-key audience and
therefore uses the configured environment or stored key rather than an account
token; with nothing else to sign with, the run is refused rather than sending
a plan's token to a gateway.

`/logout` reaches the protected store only. A child process cannot unset a
variable in its parent shell, so after removing a stored credential crucible
resolves the provider again and names any environment variable that remains
active — unset it in the launching shell if that one is meant to go too.

A file crucible cannot read is a sentence under the welcome — `! auth.json could
not be read: …` — and not the end of the run: nobody is logged in for that run,
which leaves the environment, and ending it would take away the session the file
gets fixed from.

## Authentication is a separate axis

A provider is a wire protocol — how a request is shaped and how a response is
read. How you prove who you are is a different question, and crucible keeps them
apart: a provider is handed an already-resolved credential and never learns what
kind it was.

API keys are one credential implementation. ChatGPT browser and device login
and Kimi Code device login are renewable credential implementations behind the
same trait; the OpenAI and Moonshot wire modules receive an applied header and
never learn whether it came from an account or a key.

### Account login today

`/login` offers ChatGPT and Kimi Code account plans. ChatGPT uses browser PKCE
or device authorization and is fixed to the ChatGPT Codex Responses endpoint.
Kimi Code uses RFC 8628 device authorization and is fixed to its managed coding
endpoint. Its token exchange stays on `auth.kimi.com`, while the browser opens
the authorization page on `www.kimi.com`; crucible accepts only those fixed
HTTPS origins. Both refresh in the protected store. A configured `baseUrl` is
never allowed to receive either token.

Anthropic subscription OAuth is deliberately absent: Claude subscription tokens
are not a third-party authentication contract. Anthropic is reached with a
Console API key instead. The generic Console account route also stores API keys
for OpenAI and MoonshotAI.

Moonshot authorization and model requests identify the host truthfully as
crucible with a stable protected device id. They do not reuse another harness's
product identity.

## What differs between them

Nothing you have to think about. The protocols disagree about where the system
prompt goes, whether a transcript is a list of messages or a flat list of items,
whether a tool call belongs to the message that made it, whether a tool is
declared nested or flat, how a failed result is marked, and how a stream ends.
All of that is handled inside the provider; the same session behaves the same way
on any of them.

One difference is worth knowing about because it decides which OpenAI models
work at all. crucible talks to OpenAI over `/v1/responses` rather than
`/v1/chat/completions`, because a model that reasons before answering refuses
function tools on the older endpoint — and a harness whose whole purpose is
calling tools cannot answer that by telling the model not to think. The cost is
that other vendors serving an "OpenAI-compatible" API implement the older
endpoint and not this one, so `openai` means OpenAI here rather than anything
that speaks its shape. `moonshot` is that older endpoint, read by a provider of
its own.

Two consequences you can see:

- Nothing is retained by the vendor. Requests are sent with `store` off, so a
  response is not kept for later retrieval.
- No token ceiling is sent. On that endpoint one number bounds the reasoning and
  the visible answer together, so a figure chosen for an answer is one the model
  can spend entirely on thinking; the model's own ceiling applies instead.

## When a response goes away

A connection can close between the request and the first word of the answer. The
usual reason is time: a turn that runs tools holds its connection open while they
work, and a socket the provider closed in the meantime returns nothing at all. A
service saying it is busy reads the same way from here — HTTP 429, or a 5xx from
the service or from a gateway in front of it.

crucible asks again, twice at most, pausing a quarter of a second before the
first and half a second before the second. The row above the box says `retrying`
while it does, and <kbd>Esc</kbd> ends the wait. An attempt that failed leaves
nothing in the transcript, and nothing is asked again once a word of the answer
has arrived — those words are on screen already, and a second answer would be
written underneath the half of the first one you have read. A failure that
outlives both goes is reported as itself.

A refusal about the request rather than the moment is reported the first time: a
key without access, a model name nobody serves, a response that did not parse.
Asking again would spend your time to reach the same sentence.

## MoonshotAI issues a key against one console or the other

This is the one provider where a working key can still be refused, and the
refusal does not say why.

MoonshotAI sells two products with separate consoles, and a key from one is not
accepted by the other:

| Where the key came from | Address it is accepted at |
| --- | --- |
| Kimi Code Console | `https://api.kimi.com/coding/v1` |
| Open Platform | `https://api.moonshot.ai/v1` |

Nothing in the key itself says which, so crucible cannot read it and decide. It
asks the coding console, that being the plan sold for what crucible does. A key
from the open platform says so in
[configuration](../configuration/configuration.md):

```json
{ "providers": { "moonshot": { "baseUrl": "https://api.moonshot.ai/v1" } } }
```

The two consoles also spell their models differently. What `/model` offers is
the coding console's spelling, that being the one crucible asks: `k3`,
`k3-256k`, `kimi-for-coding` and `kimi-for-coding-highspeed`. The open platform
serves `kimi-k3`, `kimi-k2.7-code` and `kimi-k2.7-code-highspeed`, and does not
serve a 256k K3 at all — so a key from there is a `baseUrl` and a typed name.

Requests to this provider identify crucible by name in the `user-agent` header.
MoonshotAI's terms require a client to say truthfully what it is, and treat a
tampered identifier as a violation.
