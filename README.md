<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
  <img alt="crucible" src="assets/logo-light.svg" width="50%">
</picture>

**The harness where agents are forged.**

A fast, lightweight terminal coding agent written in Rust.

</div>

---

Crucible reads and searches a workspace, edits files, runs commands, and keeps a
streaming coding session on a terminal screen you can scroll, select, and click.
It asks before sensitive work by default and records sessions so they can be
continued later.

## Highlights

- **Provider-independent sessions.** Anthropic, Moonshot and OpenAI are wire
  adapters; API keys and supported account logins are separate credentials.
- **Permissioned tools.** Reads inside the workspace are available by default;
  file changes, commands and reads outside it are decided by rules and the
  active permission mode. OS sandboxing is opt-in with `sandbox.enabled: true`;
  see [sandbox setup and platform support](docs/security/sandboxing.md).
- **A responsive terminal UI.** Prompts remain editable while a turn runs, tool
  output streams in place, and redirected output stays plain text.
- **Bounded resource use.** Tool output, retained screen records, configuration
  documents and replay indexes have explicit ceilings. Performance budgets are
  executable release gates rather than README claims.
- **Resumable work.** Sessions are append-only, private to the current user, and
  scoped to the workspace where they began.

## Install

Linux, macOS and FreeBSD can use the release installer:

```bash
curl --proto '=https' --tlsv1.2 -fsSLO \
  https://github.com/augments-labs/crucible-code/releases/latest/download/install.sh
bash install.sh
```

Windows executables and manual archives for all supported targets are on the
[releases page](https://github.com/augments-labs/crucible-code/releases). Every
release includes `SHA256SUMS`.

For platform details, manual verification, uninstalling and source builds, see
[Getting started](docs/getting-started/index.md).

## First session

Start Crucible in the directory it should work on:

```bash
export ANTHROPIC_API_KEY=...
cd ~/code/my-project
crucible
```

You can instead start without an environment key and use `/login`. Authentication
does not silently choose a model; `/model` selects the provider, model and
supported reasoning effort explicitly.

Useful commands:

```text
/model       choose a provider, model and effort
/login       add an account or API-key credential
/mode        inspect or change the permission mode
/resume      continue an earlier session in this workspace
/help        show every command
```

## Documentation

- [Getting started](docs/getting-started/index.md)
- [Tools](docs/tools/index.md)
- [Providers and models](docs/providers/index.md)
- [Configuration](docs/configuration/index.md)
- [Permissions](docs/permissions/index.md)
- [Sessions](docs/sessions/index.md)
- [Building from source](docs/building/index.md)

The full documentation index is [`docs/index.md`](docs/index.md).

## Contributing

Development workflow and local checks are in
[`CONTRIBUTING.md`](CONTRIBUTING.md). Coding-agent guidance begins in
[`CLAUDE.md`](CLAUDE.md), also exposed as `AGENTS.md`.

Security issues must be reported through [`SECURITY.md`](SECURITY.md), not a
public issue. Participation is covered by the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

MIT — see [`LICENSE`](LICENSE).
