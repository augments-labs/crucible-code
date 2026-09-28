# Network

What crucible connects to, and how it gets there: the proxy it takes from your
environment, the certificates it trusts and how long it waits.

## What crucible connects to

crucible makes requests of its own for three things, and nothing else:

- **The provider's endpoint**, for every request a turn makes and for
  [`web_search` and `web_fetch`](../tools/web.md), which are asked of the same
  vendor. That is its built-in address, or the
  [`baseUrl`](../configuration/configuration.md#providers) you set. The vendor
  opens the page a fetch names; crucible never does.
- **The account hosts**, when you sign in to a ChatGPT or Kimi Code plan with
  `/login` and when its token is renewed ([account
  login](providers.md#account-login-today)).
- **`api.github.com`**, to learn whether there is a newer release. It is asked
  once crucible has started, at most once a day, and is given ten seconds for
  the whole exchange; what it says is shown on the next run. A check that
  fails keeps the last answer and still counts as that day's check.
  `{ "updates": { "check": "never" } }` turns it off
  ([`updates`](../configuration/configuration.md#updates)).

MCP servers and extensions are programs crucible starts and talks to over their
standard input and output. crucible opens no connection for them, and any they
open are their own. A command's connections are its own too, except that with
confinement on, crucible's per-command proxy makes them on its behalf (see
[commands connect on their own](#commands-connect-on-their-own)).

## Through a proxy

crucible takes its proxy from the first of these variables that holds an
address it can read:

`ALL_PROXY`, `all_proxy`, `HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`,
`http_proxy`

That one proxy carries every request, whatever the address's scheme, so a
proxy named in `HTTP_PROXY` is used for `https` addresses as well. An address
with no scheme is read as `http://`. A value that is not an address with a
host, or whose scheme is not `http`, `https` or one of the SOCKS schemes below,
is passed over for the next variable. A SOCKS address is not passed over: it is
the one taken, and the variables after it are not read, so `ALL_PROXY` naming
a SOCKS proxy hides an `HTTPS_PROXY` you also set.

These are read from the environment crucible was started in, so set them in
your shell. One set in the [`env`](../configuration/configuration.md#env) block
reaches the commands crucible runs but not crucible's own requests. Proxy
settings made in the operating system's own network settings are not read.

What happens next depends on the scheme:

| Proxy | What crucible does |
| --- | --- |
| `http://` | Asks it for a tunnel with `CONNECT` and speaks TLS to the provider through it, unless the provider's address is `http`. The request to the proxy is plain text. |
| `https://` | The same, over TLS to the proxy, checked against the same certificates as any other host. |
| `socks://`, `socks4://`, `socks5://` | Ignores it and connects straight to the host. |
| `socks4a://`, `socks5h://` | Refuses to connect: every request fails unless its host skips the proxy. |

A user name and password in the address, as in
`http://name:secret@proxy.example:3128`, are sent to the proxy as `Basic`
credentials and to nowhere else. They are sent exactly as written: a `%40` in
the password stays those three characters and is not read as `@`. An address
that is no longer valid once the name and password are taken out of it is
refused, like the SOCKS kinds in the last row.

### Hosts that skip the proxy

`NO_PROXY` lists hosts that are connected to directly; `no_proxy` is read only
when `NO_PROXY` is not set at all, so `NO_PROXY=` with nothing after it hides
`no_proxy`. Entries are separated by commas, and spaces are kept as part of an
entry, so write the list without them. Each entry is compared with the host,
ignoring case and the host's port:

| Entry | Matches |
| --- | --- |
| `*` | every host |
| `.example.com` | hosts ending in `.example.com`, not `example.com` itself |
| `*example.com` | hosts ending in `example.com`, `badexample.com` included |
| `10.` | hosts beginning with `10.` |
| `internal*` | hosts beginning with `internal` |
| `example.com` | `example.com` alone |

There is no address-range matching: `10.0.0.0/8` matches nothing, and neither
does an entry that carries a port. Nothing is skipped unless the list names it,
`localhost` and `127.0.0.1` included, so a provider on your own machine is
reached through the proxy until you add it.

## Certificates

crucible trusts the Mozilla root certificates built into it, over TLS 1.2 or
1.3, and no others. It does not read the operating system's certificate store,
a file named by `SSL_CERT_FILE` or any certificate you add, and there is no
setting for one. On a network that inspects TLS by re-signing it with its own
authority, as some corporate proxies do, every request fails. A turn or a web
tool reports `TLS setup failed`, `/login` says account login could not reach
the authorization service, and the release check finds nothing. A proxy that
passes the tunnel through untouched works.
crucible presents no client certificate.

## Redirects

A redirect is never followed. The `3xx` comes back as the provider's answer and
the turn reports it, because a redirect chooses a new recipient for the key the
request carries.

## How long it waits

Making a connection has 15 seconds, for the lookup, the proxy's tunnel and TLS
together. Sending the request has a minute, and the start of the answer a minute
more. crucible sets up at most four connections at a time for turns and web
requests; a request that needs a fifth waits until one of those is made or
fails. A request from a turn or a web tool is given three minutes in all, from
being sent to the start of the answer, whatever it spent waiting; the three
minutes end when the answer starts, not when it finishes. A web tool's answer
then has two minutes to arrive in full. Each request made while you sign in,
or while an account's token is renewed, has 30 seconds from start to finish.

Connecting directly, a provider's hostname is given five seconds to resolve.
The operating system's lookup cannot be cancelled, so once one has taken
longer, every later lookup for a turn or a web tool fails at once, and a
request that needs a new connection fails with it until crucible is restarted.
Behind a proxy crucible looks up only the proxy's hostname, within the 15
seconds for the connection, and the proxy looks up the provider's.

## When a request fails

A turn names the provider and what went wrong, as in
`anthropic: TLS setup failed`:

| Message | Means |
| --- | --- |
| `host was not found` | The proxy's hostname did not resolve, or, with no proxy, the provider's. |
| `hostname resolution stalled; restart crucible before trying another provider request` | A lookup took longer than five seconds, or an earlier one did. |
| `connection failed` | No connection could be made to the host or the proxy, the proxy refused the tunnel (for a wrong or missing password, or a provider hostname it could not resolve, say), or the proxy is one crucible refuses. |
| `TLS setup failed` | The host's or the `https://` proxy's certificate was not trusted, or TLS with it failed. |
| `request timed out` | One of the waits above ran out. |
| `HTTP protocol failed` | The answer's head passed 64 KiB or 128 fields, or the request could not be built. |
| `HTTP request failed` | The exchange broke some other way, such as a connection closed before the answer. |
| `request URL was invalid` | The address is not `http` or `https` with a host. |

A turn [asks again twice](providers.md#when-a-response-goes-away) before it
reports any of these.

## Commands connect on their own

A command the `bash` tool runs is not a crucible request, and nothing above
applies to it. Its environment is built for it rather than copied from yours
([environment and
credentials](../security/sandboxing.md#environment-and-credentials)). With
confinement on under Linux or macOS and network domains allowed, its proxy
variables name crucible's own per-command proxy, which connects straight to
each allowed host rather than through the proxy your environment names
([`sandbox`](../configuration/configuration.md#sandbox)).
