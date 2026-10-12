# Installed coding agents

An entry under [`externalAgents`](../configuration/configuration.md#externalagents)
names a coding agent installed on this machine. crucible starts it as a child
process under the conversation's sandbox, speaks to it over its own wire, and
never reads or copies its credentials: the agent signs in and sends for itself.
This build reads the block and refuses to start a provider whose access names
an agent, with a sentence saying so; the sections below say how each agent is
run once one is started.

## Antigravity

crucible speaks to Antigravity's agent server over the Agent Client Protocol.

```json
{
  "externalAgents": {
    "agy": {
      "driver": "antigravity",
      "executable": "/opt/antigravity/agy_acp_server.par",
      "profileDir": "/home/me/.crucible-antigravity"
    }
  }
}
```

**Which release.** Only release 1.3.0 of the agent server, which calls itself
`antigravity-acp`, is run, on macOS, Linux and Windows for `x86_64` and
`aarch64`. Name its executable, `agy_acp_server.par`, or `agy_acp_server.exe`
on Windows, with an absolute path. A release that describes itself otherwise
when it starts, or no longer offers loading and resuming a session, signing out
or its usual sign-in, is sent nothing more and refused with a sentence naming
what differs.

**Its profile.** `profileDir` is required: it is the directory the agent keeps
its sign-in, files and unpacked runtime under, and its usual home is the one
the Antigravity IDE signs in under. On macOS and Linux crucible makes it
private to you, readable by your user alone, each time the agent starts. The
agent is started with an environment of its own: `PATH`, your home, user name,
locale and time zone, and on Windows the variables that locate the system, and
nothing else from your shell, so a Google credential, project or key your
shell holds for another program never reaches it. Its temporary files go in
the profile too. Its sign-in token is kept as a file in the profile rather than
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

**Network.** Its sandbox lets the agent reach `accounts.google.com`,
`oauth2.googleapis.com`, `cloudcode-pa.googleapis.com` and
`daily-cloudcode-pa.googleapis.com`, and no other Google host.
