# Security

- [Operating-system confinement](sandboxing.md): the `bash` process boundary,
  backend capability matrix, disabled confinement, inspection and limits.
- [Permissions](../permissions/index.md): which operations may be attempted.
- [Installed coding agents](../providers/agents.md): an agent runs as a child
  process under the conversation's sandbox, signs in for itself, and asks you
  before each action its own rules leave to a prompt, though its profile's
  hooks run, and a project's own settings can allow actions, without asking.

Permission and confinement answer different questions: permission decides
whether Crucible may start an operation, while confinement limits what the
approved process and every descendant can reach.
