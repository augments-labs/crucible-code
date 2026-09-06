# Reaching the web

Two tools, and both are off unless the session has something to answer them.

- **`web_search`** takes a query and answers with titles, addresses and short
  extracts.
- **`web_fetch`** takes one address and answers with that page as text.

They are the only tools whose effect is not on your machine. Everything else
crucible does reads a file, changes a file or runs a program, and all of that
can be undone by you afterwards. A query that has left cannot be recalled, and
that is why these two are asked about in a mode where a change to a file is not.

## What answers them

crucible does not run a search engine and does not ask you to sign up for one.
It asks the vendor whose credential you already set, in a request of its own —
separate from the turn, carrying nothing but the query or the address.

What you get depends on what your vendor serves:

| Provider | `web_search` | `web_fetch` |
| --- | --- | --- |
| Anthropic | yes | yes |
| Google — Gemini API key | yes | yes |
| OpenAI — API key or ChatGPT plan | yes | yes |
| Moonshot — Kimi Code | yes | yes |
| Moonshot — open platform | — | — |

A tool with nothing to answer it does not appear at all, rather than appearing
and failing every call.

One difference worth knowing. OpenAI has no standalone fetch — opening a page is
an action inside its search tool — so `web_fetch` there asks that tool to open
the one address, confined to its host. What comes back is the model's rendering
of the page rather than the page itself. Anthropic and Kimi Code hand over the
document; OpenAI hands over an account of it, which is fine for reading and
poor for quoting exactly.

Google uses native `google_search` for search and `url_context` for fetch,
through the same Gemini model, API key and checked Interactions endpoint as
the session. Fetch enables only URL context, requires successful retrieval and
a citation to the exact requested URL, and returns model-extracted text, not
raw HTML. Neither operation uses a Google subscription login or remote
interaction history. Incomplete, cancelled or malformed responses yield no
partial result.

A Google Search answer without usable source citations is reported as a source
error, not as evidence that the query found no results.

Google Search currently projects citation titles, URLs and cited text into the
ordinary search-tool result. That text follows normal session storage,
resumption, compaction and provider-switch behavior. Crucible does not render
Google's supplied Search Suggestions HTML or implement grounding-specific
retention. These are unresolved limitations against Google's
[grounding usage terms](https://ai.google.dev/gemini-api/terms#grounding-with-google-search),
not a claim of reviewed contractual compliance.

Moonshot's two services belong to the Kimi Code platform, which is where
crucible sends this provider unless you have set `providers.moonshot.baseUrl`
yourself. A key issued against the open platform is refused by them, so a
session pointed there gets neither tool.

Because it is a request crucible makes rather than one the model makes for
itself, there is a call for the permission engine to hold a verdict about — which
is what lets the rest of this page exist.

## What it costs

A search runs against your own credential and is billed to it. Anthropic and
OpenAI both charge **$10 per 1 000 searches**, plus the tokens of the request
that runs one, because on those two the search is run by a model. Kimi Code's
services are plain endpoints and are covered by the plan the credential is for.
On any subscription, both tools are part of what you already pay for.

Gemini native Search is metered per executed search query, in addition to model
tokens; one tool request can execute several queries. URL context uses model
tokens. Consult [Google's current pricing](https://ai.google.dev/gemini-api/docs/pricing)
for model, quota and grounding charges. `store: false` disables optional remote
interaction storage; it does not promise zero provider retention, including for
grounding requests.

`web_fetch` carries no charge of its own on Anthropic. You pay for the page as
input tokens, the same as any other text a tool returns.

Nothing here spends anything on its own: in `ask` mode, which is the default,
every call is put to you first.

## Asking, and answering once

| Mode | `web_search`, `web_fetch` |
| --- | --- |
| `ask` | asked |
| `allowEdits` | asked |
| `fullAccess` | allowed |

`allowEdits` still asks, and that is deliberate. What that mode relaxes is
writing files in a directory you already opened crucible in. Sending a query
somewhere else is not that.

Answering *don't ask again* writes down a rule naming the **host**, never the
page:

```
allow web_fetch(docs.rs)
```

The next request to that host runs without asking; a request to any other host
is a new question. A rule about the page would be a rule that never matched
twice, because the next address carries a different path.

You can write them yourself, in the same file as every other rule:

```json
{
  "permissions": {
    "allow": ["web_search(*)", "web_fetch(docs.rs)", "web_fetch(*.rust-lang.org)"],
    "deny": ["web_fetch(pastebin.com)"]
  }
}
```

`deny` beats `ask` beats `allow`, the same as anywhere else.

### An address crucible cannot read

`https://docs.rs@evil.example/` names the host `evil.example`. Anything before
an `@` is user information, and reading that address as `docs.rs` is how a rule
you wrote about a documentation site ends up authorising somewhere else.

crucible refuses to guess. An address it cannot read into a host plainly —
anything with user information, anything carrying whitespace, anything with no
host, anything that is not `http` or `https` — matches no rule except a blanket.
In `ask` and `allowEdits` that means you are asked about it whatever else you
have allowed, and `web_fetch` refuses to send it in any case.

One thing to know if you run in `fullAccess`: a narrow `deny` rule cannot catch
an address like that either, because it matches no narrow rule of any kind. In
that mode there is nothing left to ask, so the call is allowed and then refused
by the tool. If you rely on `deny` rules, `ask` or `allowEdits` is where they
do their work.

### A redirect somewhere else is a new question

Allowing a host allows that host. If a page redirects to a different one,
`web_fetch` does not hand back what it found there — it says where it was sent
and stops. The verdict you gave was about the address that was asked for, and a
site you trust can send a request anywhere; the page comes back only once you
have allowed the host it actually came from. A redirect inside one host is still
that host and is answered normally.

Nothing here can reach your own machine. crucible does not fetch the page
itself — the vendor does, from its own network — so `localhost`, a private
address and anything else behind your firewall are all unreachable through this
tool whatever rule you write.

## What comes back is not trusted

A page and a search result are written by somebody who is not you and is not
crucible. They arrive in the same transcript as your words and the model's, and
a page can perfectly well contain a paragraph addressed to a coding agent.

crucible does not act on any of it. The model is told where every line came
from, and the tools bound what one call can return so a long page cannot crowd
out the conversation. That is the whole of the protection, and it is worth
knowing its shape: **the defence is that the model is told, not that the page
was cleaned.** Nothing scrubs a fetched page of instructions, because nothing
can tell an instruction from a quoted example of one.

So the practical advice is the ordinary advice: a page you fetch is a report
from a source you chose, and `deny` rules and `ask` mode are how you keep it
that way.
