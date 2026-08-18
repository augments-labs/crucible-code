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

Today that means **Anthropic**. Set `ANTHROPIC_API_KEY`, or log in, and both
tools appear. With any other provider they do not appear at all: a tool that is
registered and fails every call teaches the model to keep trying it.

Because it is a request crucible makes rather than one the model makes for
itself, there is a call for the permission engine to hold a verdict about — which
is what lets the rest of this page exist.

## What it costs

A search runs against your own credential and is billed to it. Anthropic charges
**$10 per 1 000 searches**, plus the tokens of the request that runs one. On a
subscription, both tools are part of the plan and cost nothing at the margin.

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
    "deny": ["web_fetch(169.254.169.254)"]
  }
}
```

`deny` beats `ask` beats `allow`, the same as anywhere else.

### An address crucible cannot read

`https://docs.rs@evil.example/` names the host `evil.example`. Anything before
an `@` is user information, and reading that address as `docs.rs` is how a rule
you wrote about a documentation site ends up authorising somewhere else.

crucible refuses to guess. An address it cannot read into a host plainly —
anything with user information, anything with no host, anything that is not
`http` or `https` — matches no rule except a blanket, so you are asked about it
whatever you have allowed. `web_fetch` then refuses to send it at all.

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
