# Security

- [Operating-system confinement](sandboxing.md) — the `bash` process boundary,
  backend capability matrix, compatibility modes, inspection and limits.
- [Permissions](../permissions/index.md) — which operations may be attempted.

Permission and confinement answer different questions: permission decides
whether Crucible may start an operation, while confinement limits what the
approved process and every descendant can reach.
