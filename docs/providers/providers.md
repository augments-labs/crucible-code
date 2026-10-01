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
   remembers a vendor choice, and it is read only from the configuration file
   in your home directory: a file a repository brings cannot choose who is sent
   your key.
3. Exactly one provider having a usable credential: a stored account login, a
   stored API key, or a key in one of the variables in [Keys](#keys). That is
   the absence of a choice to make, and it lets a first run work with one
   credential and nothing configured.

A credential says a provider **can be reached**, and never which to ask. Which
variable your shell happens to carry is a fact about that shell, and a turn sent
to the wrong vendor is billed there and leaves your prompt behind, so no
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
says what to ask OpenAI *for*. It is not a way of saying to ask OpenAI, and
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
prompt box, which reads `provider · model · effort`, or `provider · model`
where no rung is in force, the vendor before the model; what is typed after
`--model` or `/model` keeps the slash, `provider/model`. That row stands for the whole session and is said
again whenever one of the three changes, so it keeps up when `/model` hands the
session to another vendor mid-way. The welcome card deliberately carries no provider, model or
effort because it is the first thing in the transcript and is scrolled away from
rather than kept up to date.

## Which model

There is **no model built in**, and none of these rungs is a guess:

1. `--model`, where it names one.
2. `providers.<name>.model` in your
   [configuration](../configuration/configuration.md), for the provider being
   asked. A provider and a bare slash (`--model openai/`) is how you reach
   this rung with the flag present.
3. Nothing. crucible starts anyway and says so under the welcome, naming
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
whichever keys the shell is carrying. A name that is empty, longer than 256
bytes or holds a control character is refused with a line instead of being
tried. `/model` on its own stands a shelf over
the whole shell: a search line across the top, every provider this build serves
in one pane beside the models in the other, and the rungs the marked model takes
on a strip underneath, under the name of the one being asked now. Typing narrows
both panes at once, against a model's name or a provider's: somebody who types
`openai` wants everything that vendor serves, somebody who types `sonnet` wants
the one model, and neither should have to say which kind of name they just
typed. Tab crosses between the panes, the up and down arrows walk whichever one
the mark is in, the left and right arrows walk the rungs, Enter takes the model
and the rung under it together, and Escape leaves everything as it was. A mouse
lights the row it is over, across the whole width of that pane; passing over a
row chooses nothing, and clicking one puts the mark on it. Clicking a model the
mark is already on takes it, so a double click picks one outright. A provider
is never taken that way, because it narrows the models beside it rather than
being an answer itself. The rung stays on the arrow keys, which is what
keeps taking the model and saying how hard it should think one visit. Down a
pipe, where nobody can walk a shelf, it writes the models out as the line that
asks for each.

Each provider lists the models its credential in use serves: a ChatGPT
sign-in leaves out `gpt-5.5`, and a Qwen plan key lists its plan's models.
With one provider marked, the models pane is headed by the provider and that
credential, as in `openai · ChatGPT sign-in`, and a quiet row under the models,
which the mark never takes, counts what an API key would add and names
`/login`. A row's note says `trains` on a model whose vendor may train on what
is sent to it (see [Content use](content-use.md)),
else `no rung` on one that serves none, else `fast` on one with a fast form.

Taking a row off the models pane moves the session to whoever serves it first:
a model belongs to the vendor that serves it, and the two change together. The
rung goes with them, because a rung is asked of a model: choosing one and then
being sent somewhere else to say how hard it should think is the same question
put twice. A model whose vendor serves no rung is taken with the rung left
exactly as it was, and its row says so. A model the shelf does not carry is
still named. What is offered is a shortcut past the vendor's documentation, and
the vendor remains the authority on what it serves.

A model belongs to the provider serving it. crucible never writes a name under
one provider and sends it to another. The pairing is settled once, by
[Which provider](#which-provider), and the model rungs above are all read for
that same provider.

Naming a provider this build does not have is a startup failure that says which
ones it has:

```
crucible: no provider called gemini; this build has anthropic, deepseek, google, meta, mimo, minimax, moonshot, openai, qwen, xai, zai
```

## What each provider serves

This build serves eleven providers. The name in the first column is what
`--model`, `provider` and `providers.<name>` take, and the models are what
`/model` offers for it, each with the rungs it takes:

| Provider | Vendor | Models and their rungs | Variable |
| --- | --- | --- | --- |
| `anthropic` | Anthropic | `claude-fable-5-1`, `claude-fable-5`, `claude-opus-5-5`, `claude-opus-5`, `claude-sonnet-5-5`, `claude-sonnet-5`: all five; `claude-haiku-4-5`: none | `ANTHROPIC_API_KEY` |
| `deepseek` | DeepSeek | `deepseek-flash`, `deepseek-v4-pro`: low, high, max | `DEEPSEEK_API_KEY` |
| `google` | Google | `gemini-3.8-flash`, `gemini-3.7-flash`, `gemini-3.6-flash`, `gemini-3.1-pro-preview`: low, medium, high | `GEMINI_API_KEY` |
| `meta` | Meta | `muse-spark-1.3`, `muse-spark-1.3-contributor`, `muse-spark-1.2`, `muse-spark-1.2-contributor`: low, medium, high, xhigh | `META_API_KEY` |
| `mimo` | MiMo | `mimo-v2.6-pro`, `mimo-v2.6-flash`: none | `MIMO_API_KEY` |
| `minimax` | MiniMax | `MiniMax-M3`, `MiniMax-M2.7`: none | `MINIMAX_API_KEY` |
| `moonshot` | MoonshotAI | `k3`, `k3-256k`, `kimi-for-coding`, `kimi-for-coding-highspeed`: low, high, max | `MOONSHOT_API_KEY` |
| `openai` | OpenAI | `gpt-6-astra`, `gpt-6.1-sol`, `gpt-6-sol`, `gpt-6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`: all five; `gpt-5.5`: low, medium, high, xhigh | `OPENAI_API_KEY` |
| `qwen` | Qwen | `qwen3.8-max`, `qwen3.8-flash`: low, medium, xhigh; `qwen3.7-plus`, `qwen3.6-plus`: none | `DASHSCOPE_API_KEY` |
| `xai` | xAI | `grok-4.7`, `grok-4.6`: low, medium, high, xhigh | `XAI_API_KEY` |
| `zai` | Z.ai | `glm-5.3`, `glm-5.3-flash`: low, high, max; `glm-5.2`: high, max | `ZAI_API_KEY` |

The two Meta models whose names end in `-contributor` are cheaper because Meta
may train on what is sent to them; crucible asks before the first one is
chosen or sent to, and the standard two are not asked about. See [content
use](content-use.md).

Meta and xAI are spoken to over the Responses protocol, the other five vendors
new in this release over Chat Completions. crucible sends those seven text
alone in this release: a picture, a PDF, audio or video stays out of the
request, whatever the model reads. A refusal from any of them reaches you in
the vendor's own words, except Z.ai's refusal of a prompt too long for the
model, which makes room and asks again as a
[full window](../sessions/sessions.md#when-the-window-fills) does. Meta's and
MiniMax's refusals of a request too long for the model carry no code, so
crucible reads each by its shape and its exact words, and they make room and
ask again too. The others end the turn on that refusal unless it carries a
code crucible reads as a full window. MiniMax's `1039` "Token limit exceeded"
cannot be told apart from a rate limit, so it still ends the turn.

### Rows and sites

Each vendor's credentials are given on rows of `/login`. A row in the
*subscription* list takes the plan's own key, typed into the same box as an
API key; nothing is signed in to. With no `baseUrl` set, a row's requests go
to its address followed by `/chat/completions`, or by `/responses` for Meta and
xAI.

| Row | List | Its keys | Address | Models the vendor serves there |
| --- | --- | --- | --- | --- |
| MiniMax Token Plan · minimax.io | subscription | start `sk-cp-` | `https://api.minimax.io/v1` | both |
| MiniMax Token Plan · minimaxi.com | subscription | start `sk-cp-` | `https://api.minimax.cn/v1` | both |
| Qwen Coding Plan · alibabacloud.com | subscription | start `sk-sp-` | `https://coding-intl.dashscope.aliyuncs.com/v1` | `qwen3.7-plus`, `qwen3.6-plus` |
| Qwen Coding Plan · aliyun.com | subscription | start `sk-sp-` | `https://coding.dashscope.aliyuncs.com/v1` | `qwen3.7-plus`, `qwen3.6-plus` |
| Qwen Token Plan · alibabacloud.com | subscription | start `sk-sp-` | `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` | `qwen3.8-max`, `qwen3.8-flash`, `qwen3.7-plus` |
| Qwen Token Plan · aliyun.com | subscription | start `sk-sp-` | `https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1` | `qwen3.8-max`, `qwen3.8-flash`, `qwen3.7-plus` |
| DeepSeek | API key | | `https://api.deepseek.com` | both |
| Meta | API key | | `https://api.meta.ai/v1` | all four |
| MiMo | API key | never start `tp-` or `ttp-` | `https://api.xiaomimimo.com/v1` | both |
| MiniMax · minimax.io | API key | start `sk-api-` | `https://api.minimax.io/v1` | both |
| MiniMax · minimaxi.com | API key | start `sk-api-` | `https://api.minimax.cn/v1` | both |
| Qwen · alibabacloud.com | API key | | `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` | all four |
| Qwen · aliyun.com | API key | | `https://dashscope.aliyuncs.com/compatible-mode/v1` | all four |
| xAI | API key | | `https://api.x.ai/v1` | both |
| Z.ai · z.ai | API key | | `https://api.z.ai/api/paas/v4` | all three |
| Z.ai · bigmodel.cn | API key | | `https://open.bigmodel.cn/api/paas/v4` | all three |

A key in a provider's variable belongs to one row of it: for MiniMax and Qwen
the international site's API key row, for Z.ai the z.ai row. A key of the
mainland China site, or a plan's key, is given through `/login`, or reaches
that site with `providers.<name>.baseUrl` set to the whole address its
requests go to: the row's address followed by `/chat/completions`, since a
`baseUrl` is posted to as written. `baseUrl` is read only from the
configuration file in your home directory. Qwen and MiniMax
bind a key to its site and refuse it at the other; Z.ai does not say what its
other site makes of one.

A Qwen Coding Plan or Token Plan key starts with `sk-sp-` either way, and
nothing in it says which plan, or whether a Token Plan key is the Personal or
the Team edition. The plans are for interactive use in a coding tool;
Alibaba Cloud's pages say a plan's key used in a script or a backend may be
suspended. MiniMax sells M Plan in place of Token Plan to new buyers, with one
key prefix for both, and its M Plan pages list only `MiniMax-M3.1-Flash-Preview`
as a text model, which this build does not offer. Whether an M Plan key is
served `MiniMax-M3` and `MiniMax-M2.7` is not settled by any page; the vendor
answers.

#### Qwen's shared addresses

Alibaba Cloud put the shared `dashscope.aliyuncs.com` domain into maintenance
on 30 September 2026: what it serves keeps working, and nothing new is added
to it. Its notice does not say whether `dashscope-intl.aliyuncs.com` is
included, or whether a model released later reaches either. The two Qwen API
key rows still send there. Alibaba Cloud asks for the address of your own Model
Studio workspace wherever possible, which a key row reaches with `baseUrl`, the
address its requests are posted to:

```json
{ "providers": { "qwen": { "baseUrl": "https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/chat/completions" } } }
```

with your workspace's id for `{WorkspaceId}`, and `cn-beijing` in place of
`ap-southeast-1` for a mainland China workspace. A request sent to a `baseUrl`
asks for no fast form and claims no reviewed cache: see [Fast](fast.md) and
[Prompt caching](prompt-caching.md).

### What each new vendor has, and what no source settled

Where a vendor's pages did not settle a fact, the vendor still ships, with the
choice that asks least of it, and this says which fact it was.

- **Meta.** Web search through `web_search`, and no fetch: Meta serves none.
  Provider-managed caching. No fast form: Meta's `service_tier` field has no
  stated price, speed or behaviour. `max` is not offered on
  `muse-spark-1.3`, because Meta's pages disagree on whether it takes one.
  What a failed `web_search` call carries is not documented.
- **xAI.** Web search through `web_search`, and no fetch: xAI's search item
  documents no action that opens one page. Provider-managed caching. No fast
  form: xAI serves a priority tier, but no source shows how it refuses one.
- **DeepSeek.** No web tool on Chat Completions. Provider-managed caching,
  with no published smallest prefix. No fast form: DeepSeek serves none.
  `deepseek-flash` reads pictures; crucible sends it text alone.
- **Z.ai.** No web tool: its Chat Completions search is not a tool the model
  calls, and its pages disagree on which models take it. Provider-managed
  caching, with no stated lifetime. No fast form: no source shows how
  `glm-5.3-flashx` is refused. Z.ai's and Zhipu's API terms forbid using the
  service through unauthorised third-party software, without saying whether a
  client sending your own key is that; accepting those terms is yours.
- **Qwen.** No web tool: its pages disagree on whether these four models take
  search on Chat Completions. Provider-managed caching; caching at the plan
  addresses is not documented. No fast form: no source shows how
  `qwen3.8-max-prime` is refused or reported. Whether the plan addresses
  honour a rung is not documented; crucible offers the 3.8 models' rungs
  there as on a key.
- **MiMo.** No web tool: MiMo's search must be switched on in its console,
  and no source shows how a request is refused where it is not. Provider-managed
  caching. No fast form: no source shows how `mimo-v2.6-pro-ultraspeed` is
  refused or reported. Xiaomi's agreement forbids tools it has not authorised,
  without saying what that covers; accepting those terms is yours.
- **MiniMax.** No web tool on Chat Completions. Provider-managed caching from
  512 input tokens. No fast form: no source shows how its priority tier or
  `MiniMax-M2.7-highspeed` is refused, or how a Chat Completions answer says
  which served it. MiniMax's reference and guide name different fields for the
  reasoning it sends back.

## How much context is used

A model's native maximum is not necessarily the session's context window.
Without an explicit configuration, the window is 200,000 tokens for Anthropic,
Google and the seven vendors new in this release, 272,000 for OpenAI, and
262,144 for Moonshot. A model with a smaller native limit
stays smaller. The compaction reserve is separate and is subtracted when deciding
whether another exchange fits.

This is local context management, not a request parameter. Anthropic's current
Opus, Sonnet and Fable models and Kimi K3 may accept 1M natively, while the
OpenAI models offered here expose a larger maximum; Crucible still compacts at
the figures above. To opt in deliberately, set the named model under
`providers.<name>.contextWindow`, or set `defaultContextWindow` for that provider.

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

The bottom rung is not a default in disguise. Which rungs a model serves, and
whether it takes one at all, is decided by its vendor and differs between
models of the same vendor, so a rung crucible chose on your behalf would reach
models that refuse the field outright. Naming one for a model that does not take
it is refused by the vendor rather than dropped here, the same bargain a model
name is already on.

A word that is not a rung is refused before anything is drawn, and crucible
exits with status 2:

```
error: invalid value 'maximum' for '--effort <RUNG>': no effort called maximum; crucible takes low, medium, high, xhigh, max
```

Where a rung was chosen, the row under the prompt box says so after the model.
Where none was, nothing is drawn in its place: the rung in force is then the
vendor's, and crucible is never told which it picked.

Mid-session, `/effort` stands a ladder over the rungs the model in force serves,
and `/effort <rung>` takes one outright. The ladder is a track with the rungs
written under it, `Faster` at one end and `Smarter` at the other: the left and
right arrows move the mark, Enter takes what is under it and Escape leaves it.
Its title names the model, because a rung is one word in one request and what
it buys is that model's to say, and with no model chosen yet `/effort` says so
instead of standing one. The same rungs are on the strip beneath the shelf
`/model` stands, so settling both takes one visit, and `/effort` changes the
rung without touching the model. Either way the rung applies from the next turn
on and is written to `~/.crucible/config.json` beside the model, so the next run
here asks for the same. There is no way back to asking for nothing from inside
a session: a rung you can see on the screen cannot be un-seen by being handed a
default this program is never told the name of. Remove the key from the file
for that.

Where a vendor serves a model faster for a higher price, `/fast` asks for it,
after saying what it costs. [Fast](fast.md) lists which models have a fast form,
the vendor's price for each, and when the speed goes back to standard.

### Asking crucible what it is

Both answers are told to the model before every turn, so asking a session which
model it is and how hard it is thinking gets what is actually on the request:

```
› what model are you?

crucible, asking claude-opus-5 at max effort.
```

Neither is something a model can find out for itself. Its own name it would
answer from training, which is whatever was true when it was trained, and is
wrong the moment `/model` changes it. The rung is a field on a request it
never sees. Both are read off the session again before each turn rather than
written down once, so the answer keeps up with `/model` and `/effort` instead of
describing the session the first turn was taken in.

Where no rung was named, what is said is that the vendor's own default applies.
crucible is never told which rung that is, and neither is the model.

### The ladder holds what the model serves

Which rungs a model takes is written down beside its name in the shelf `/model`
stands, so the ladder is the model's rather than crucible's: `moonshot/k3` gets
three rungs, `openai/gpt-5.5` gets four, and a model whose vendor serves none is
told so instead of being offered a ladder that cannot be answered. A rung that
is missing is missing rather than drawn and greyed: a row the arrows have to
step over is a row worth not drawing.

That list is read off each vendor's documentation and goes stale between
releases, so nothing is narrowed except the offer. `--effort` and
`/effort <rung>` go to the vendor whatever this build has written down, and a
model it has never heard of (one released since, or one typed rather than
picked) is offered all five. What a stale entry costs is a missing row in a
panel, never a refusal from the program that is not the one serving the model.

Google is the encoder-level exception: its Interactions requests support
`low`, `medium` and `high`, and reject `xhigh` or `max` before sending HTTP.
A typed model switch to Google refuses an incompatible inherited effort without
changing the current model. Pick a supported rung in `/model`, or change the
current effort first. Nothing is silently mapped to a different rung.

### New reasoning models

| Provider | Exact model name | Offered effort |
| --- | --- | --- |
| Google | `gemini-3.8-flash` | low, medium, high |
| Google | `gemini-3.7-flash` | low, medium, high |
| Google | `gemini-3.6-flash` | low, medium, high |
| Google | `gemini-3.1-pro-preview` | low, medium, high |
| Anthropic | `claude-fable-5-1` | low, medium, high, xhigh, max |
| Anthropic | `claude-opus-5-5` | low, medium, high, xhigh, max |
| Anthropic | `claude-sonnet-5-5` | low, medium, high, xhigh, max |
| OpenAI | `gpt-6-astra` | low, medium, high, xhigh, max |
| OpenAI | `gpt-6.1-sol` | low, medium, high, xhigh, max |
| OpenAI | `gpt-6-sol` | low, medium, high, xhigh, max |
| OpenAI | `gpt-6-luna` | low, medium, high, xhigh, max |

No configured effort means no effort field. Opus 5.5 and Sonnet 5.5 are
asked, and their thinking kept and replayed, as Fable 5.1's is. Fable 5.1 still
uses adaptive thinking, including the documented preserved-thinking and per-message effort
controls. Preserved thinking requires a compatible Anthropic workspace data
retention setting; consult [Anthropic's preserved-thinking guide](https://platform.claude.com/docs/en/build-with-claude/preserved-thinking).
Crucible does not change workspace retention on your behalf.

Astra preserves ordered Responses reasoning, messages and function calls. Its
documented effort updates are kept at their original user-message boundaries;
when an unset effort or unsupported boundary cannot be represented by that
mechanism, Crucible uses ordinary request-level effort instead of inventing a
default. See [OpenAI reasoning](https://developers.openai.com/api/docs/guides/reasoning).

## Keys

A key is read at startup and goes no further than the header it signs a request
with. Which variable is read follows from the provider:

| Provider | Variable | Sent as |
| --- | --- | --- |
| `anthropic` | `ANTHROPIC_API_KEY` | `x-api-key` |
| `deepseek` | `DEEPSEEK_API_KEY` | `authorization: Bearer …` |
| `google` | `GEMINI_API_KEY` | `x-goog-api-key` |
| `meta` | `META_API_KEY` | `authorization: Bearer …` |
| `mimo` | `MIMO_API_KEY` | `authorization: Bearer …` |
| `minimax` | `MINIMAX_API_KEY` | `authorization: Bearer …` |
| `moonshot` | `MOONSHOT_API_KEY` | `authorization: Bearer …` |
| `openai` | `OPENAI_API_KEY` | `authorization: Bearer …` |
| `qwen` | `DASHSCOPE_API_KEY` | `authorization: Bearer …` |
| `xai` | `XAI_API_KEY` | `authorization: Bearer …` |
| `zai` | `ZAI_API_KEY` | `authorization: Bearer …` |

Meta's own reference names `MODEL_API_KEY`, a name another vendor's tool could
read too; crucible reads `META_API_KEY`, as Meta's own command-line client
does. A key kept under another name is reached with `apiKeyEnv`.

Only the chosen provider's variable is read. Running `crucible --model
openai/gpt-5.6-terra` needs `OPENAI_API_KEY` set and does not care whether
`ANTHROPIC_API_KEY` is.

A configuration file can point a provider at a different variable, which is what
a second key for the same vendor needs:

```json
{ "providers": { "anthropic": { "apiKeyEnv": "WORK_ANTHROPIC_KEY" } } }
```

That is a variable **name**, and pointing crucible at one points it away from
the other: `ANTHROPIC_API_KEY` is then not read at all. `apiKeyEnv`, `baseUrl`
and `fast` are read only from the configuration file in your home directory; a
project file that sets one is refused with `cannot be set here`.

A custom `baseUrl` must use HTTPS unless it is the exact loopback host
`localhost`, `127.0.0.1` or `[::1]`. User information and fragments are refused,
and diagnostics show the recipient but redact the path and query because those
parts often contain tenant identifiers or tokens. Authenticated model requests
never follow redirects; the provider receives the 3xx refusal instead.

A refused response body is read for at most ten seconds and 8 KiB. That deadline
is elapsed time for the whole body, including bytes a slow peer continues to
trickle between waits, and it is per attempt: a status crucible retries can cost
it more than once in a turn, and Esc ends the wait wherever it has got to. Where
a refusal reaches you in the service's own words, a reply crucible read past the
bound, and finished reading in time, ends in
` [cut: the reply was longer than crucible reads]`. One whose reading ran out of
time or broke off after the bound had filled ends in
` [cut: crucible stopped reading here]`, because whether that one had more to
come usually cannot be told. Either way the end of what was kept is dropped
wherever that end begins a key, since a cut can land in the middle of one.
Google, Fable 5.1, Opus 5.5, Sonnet 5.5 and Astra answer every refusal with a
sentence of crucible's own instead.

The exact key crucible sent is removed from a log line, an error message, a
session file and anything crucible prints. That is the value crucible knows it
sent, so a service that echoes a key back changed, or leaves part of one
somewhere other than the end of a cut reply, is not something this can match. If
you see a key, that is a bug worth [reporting privately](../../SECURITY.md).

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
from a labelled box that takes a paste and draws a dot per character rather
than the key. `/login` on its own asks how usage is paid for: *Your account with
subscription* lists the accounts, and *Provide your own API key* lists the
providers, each row naming the variable the provider reads from, and opens the
same box. A row that holds the credential its provider is served by says
`signed in`.

The command reports that the key was stored, not that it was verified. Provider
authentication is established by the next request; a rejected key stays stored
until `/logout <provider>` removes it.

The session is then set up with that provider from the next turn on, without
restarting, unless another provider is already answering, in which case the
session keeps the provider and model it has and the line points at `/model`,
where switching is chosen rather than implied. Authentication selects neither
[model](#which-model) nor [effort](#how-hard-to-think); where neither was
already chosen, `/model` is the next explicit step.

Where the login is what set the session up, `provider` is written down for it
too, because a credential says a vendor can be reached and never which to ask.
Logging in is somebody saying which, and the next run here should not have to
be asked again. That still selects neither a model nor an effort.

Where no variable above is set and that file names nobody, the warning under the
welcome names this command, and the prompt is there underneath it as usual.

`/logout <provider>` removes that provider's stored account or API key, and
`/logout` on its own offers every stored credential crucible can remove.
Editing the file by hand works too: crucible only reads what is there, and a
name under `keys` that this build does not serve is left alone rather than
offered for removal.

The names under `keys` and `subscriptions` say which `/login` row a credential
was given on. A row every release has is written under the provider's own name,
the one `--model openai/…` takes; a row an earlier release does not know is
written under the provider, `@` and its site, as `moonshot@kimi.ai` is. So a
roll back to an earlier release finds its credentials where it left them and
leaves the rest alone. `version` says which crucible wrote the file, so one
from a later version is left alone rather than guessed at.

A provider holds **one** credential, whichever row it was given on. Saving a key
or completing a sign-in removes that provider's other stored credential in the
same write, and the screen before it says which. Nothing is replaced until the
new credential is stored, so a sign-in that does not complete leaves the file as
it was. Other providers' credentials are unaffected.

Where the file holds two credentials for one provider, which only an earlier
release writing after a roll back can cause, the one under the provider's own
name is used. The next start removes the other in one write and says which it
was; where that write cannot be made, the start goes on with the same credential
and says so, and tries again the next time.

For API-key authentication, **the variable wins over a stored API key**. It is
the key chosen for this process (a second account, a work key, one rotated an
hour ago), while the stored key is the standing answer underneath it, so
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
active. Unset it in the launching shell if that one is meant to go too.

A file crucible cannot read is a sentence under the welcome (`! auth.json could
not be read: …`) and not the end of the run: nobody is logged in for that run,
which leaves the environment, and ending it would take away the session the file
gets fixed from.

## Authentication is a separate axis

A provider is a wire protocol: how a request is shaped and how a response is
read. How you prove who you are is a different question, and crucible keeps them
apart: a provider is handed an already-resolved credential and never learns what
kind it was.

API keys are one credential implementation. ChatGPT browser and device login
and Kimi Code device login are renewable credential implementations behind the
same trait; the OpenAI and Moonshot wire modules receive an applied header and
never learn whether it came from an account or a key.

### Account login today

`/login` offers ChatGPT and Kimi Code accounts. The other rows of its
subscription list, MiniMax's plan and Qwen's Coding Plan and Token Plan, take a
plan's key and sign in to nothing; see [rows and sites](#rows-and-sites).
ChatGPT uses browser PKCE or device authorization and is fixed to the ChatGPT
subscription Responses endpoint.
Kimi Code uses RFC 8628 device authorization, with one row for each of Kimi's
two sites, and an account belongs to one of them:

| Row | Token exchange | Authorization page | Requests |
| --- | --- | --- | --- |
| Kimi Code · kimi.com, mainland China accounts | `auth.kimi.com` | `www.kimi.com` | `https://api.kimi.com/coding/v1` |
| Kimi Code · kimi.ai, accounts outside mainland China | `auth.kimi.ai` | `www.kimi.ai` | `https://api.kimi.ai/coding/v1` |

The kimi.ai hosts are those Kimi's own client names for its global region
([`packages/oauth/src/region.ts`](https://github.com/MoonshotAI/kimi-code/blob/main/packages/oauth/src/region.ts),
read 29 September 2026), which also shares one client id across both sites.
crucible accepts only each row's fixed HTTPS origins, and sends a credential,
its renewals and the web tools it signs to the hosts of the row it was given on
and to no other. Both refresh in the protected store, each request within 30
seconds; a renewal runs once for everything waiting on that account, and Escape
stops the turn without waiting for it. A configured `baseUrl` is never allowed
to receive either token.

Anthropic subscription OAuth is deliberately absent: Claude subscription tokens
are not a third-party authentication contract. Anthropic is reached with a
Console API key instead. The *Provide your own API key* route also stores API
keys for DeepSeek, Google, Meta, MiMo, MiniMax, MoonshotAI, OpenAI, Qwen, xAI
and Z.ai. Meta's Muse Code subscription, the Z.ai Coding Plan, MiMo's Token
Plan and xAI's SuperGrok have no row: Meta keeps the Muse Code credential for
Muse Code alone, the Z.ai Coding Plan names the tools it may be used in and
crucible is not one, and the other two are not offered in this release. Google accepts a Gemini Developer API
key only; Gemini product subscriptions are not an authentication route here.

Google requests use `POST https://generativelanguage.googleapis.com/v1beta/interactions?alt=sse`
with `stream:true`, `store:false`, and local input history, never
`previous_interaction_id`. A custom `providers.google.baseUrl` is the complete
Interactions endpoint, not a base path to which Crucible appends a route.
See [Google's Interactions documentation](https://ai.google.dev/gemini-api/docs/interactions-overview).

Moonshot authorization and model requests identify the host truthfully as
crucible with a stable protected device id.

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
function tools on the older endpoint. Crucible needs tool calls to work with
reasoning enabled. The cost is that most other vendors serving an
"OpenAI-compatible" API implement the older endpoint and not this one, so
`openai` means OpenAI here rather than anything that speaks its shape. `moonshot` is that older endpoint, read by a provider of
its own, and so are `deepseek`, `zai`, `qwen`, `mimo` and `minimax`, each with
its vendor's own ways of asking for reasoning and of reporting usage. `meta` and
`xai` speak Responses, with what each vendor does differently kept beside it.

Two consequences you can see:

- Responses are not stored for later retrieval through a response ID: requests
  use `store:false`. This does not promise zero provider-side retention for
  safety, billing or other purposes under the vendor's terms.
- One number bounds the reasoning and the visible answer together, so a long
  think leaves less room for the answer. Crucible sends the model's own output
  limit, held at 16,000 tokens, or 8,192 for a model it has no limits for. A
  ChatGPT sign-in sends no ceiling, because that service refuses the field;
  its own ceiling applies instead.

## When a response goes away

A connection can close between the request and the first word of the answer. The
usual reason is time: a turn that runs tools holds its connection open while they
work, and a socket the provider closed in the meantime returns nothing at all. A
failure about the moment rather than the request reads the same way from here:
HTTP 429 or 408, or a 5xx from the service or from a gateway in front of it.

crucible asks again, twice at most, pausing a quarter of a second before the
first and half a second before the second. The row above the box says `retrying`
while it does, and <kbd>Esc</kbd> ends the wait. An attempt that failed leaves
nothing in the transcript, and nothing is asked again once a word of the answer
has arrived. Those words are on screen already, and a second answer would be
written underneath the half of the first one you have read. A failure that
outlasts both retries is reported as itself.

A refusal about the request rather than the moment is reported the first time: a
key without access, a model name nobody serves, a response that did not parse.
Asking again would spend your time to reach the same sentence.

## MoonshotAI issues a key against one console or the other

Here a working key can still be refused, and the refusal does not say why.
A Qwen or MiniMax key given to the other site is refused the same way; see
[rows and sites](#rows-and-sites).

MoonshotAI sells two products with separate consoles, and a key from one is not
accepted by the other:

| Where the key came from | Address it is accepted at |
| --- | --- |
| Kimi Code Console, kimi.com | `https://api.kimi.com/coding/v1` |
| Kimi Code Console, kimi.ai | `https://api.kimi.ai/coding/v1` |
| Open Platform | `https://api.moonshot.ai/v1` |

Nothing in the key itself says which, so crucible cannot read it and decide. It
asks the coding console, that being the plan sold for what crucible does. A key
from the open platform says so in
[configuration](../configuration/configuration.md):

```json
{ "providers": { "moonshot": { "baseUrl": "https://api.moonshot.ai/v1/chat/completions" } } }
```

`baseUrl` is the address requests are posted to, written whole: the API's
base address with the path of the endpoint after it.

The two consoles also spell their models differently. What `/model` offers is
the coding console's spelling, that being the one crucible asks: `k3`,
`k3-256k`, `kimi-for-coding` and `kimi-for-coding-highspeed`. The open platform
serves `kimi-k3`, `kimi-k2.7-code` and `kimi-k2.7-code-highspeed`, and does not
serve a 256k K3 at all, so a key from there is a `baseUrl` and a typed name.

Requests to this provider identify crucible by name in the `user-agent` header.
MoonshotAI's terms require a client to say truthfully what it is, and treat a
tampered identifier as a violation.
