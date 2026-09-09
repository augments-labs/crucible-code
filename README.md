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

- **Provider-independent sessions.** Anthropic, Google, Moonshot and OpenAI are wire
  adapters. Each provider uses either an API key or a supported account login;
  switching login methods replaces that provider’s stored credential.
- **Permissioned tools.** Reads inside the workspace are available by default;
  file changes, commands and reads outside it are decided by rules and the
  active permission mode. OS sandboxing is opt-in with `sandbox.enabled: true`;
  configure filesystem, network and command limits or inspect them with `/sandbox`.
  See [sandbox setup and platform support](docs/security/sandboxing.md).
- **A responsive terminal UI.** Prompts remain editable while a turn runs, tool
  output streams in place, and compact tool summaries open into full details.
  Redirected output stays plain text.
- **Bounded resource use.** Tool output, retained screen records, configuration
  documents and replay indexes have explicit ceilings. Performance budgets are
  executable release gates rather than README claims.
- **Resumable work.** Sessions are append-only, private to the current user, and
  scoped to the workspace where they began. Resume restores the conversation,
  compaction markers and recorded diff previews.

## Performance, memory and resource use

Crucible ships as a native executable. Its performance suite measures the actual
terminal process, including input responsiveness and memory after a long session.
A local release-profile run on September 8, 2026 produced:

| What is checked | Measured | Enforced budget |
| --- | ---: | ---: |
| First terminal frame, p95 | 1.9 ms | ≤ 20 ms |
| First input rendered, p95 | 2.1 ms | ≤ 60 ms |
| `--help` and `--version` exit, p95 | 0.7 ms | ≤ 12 ms |
| Session picker with a deep history preview, p95 | 2.1 ms | ≤ 20 ms |
| Peak RSS across the memory stress fixtures | 25.7 MiB | ≤ 35 MiB |

Measured on Linux x86-64, Intel Core i9-10900K, with Rust 1.97.1 and a local
fixture provider. The 20-turn conversation alone peaked at 12.7 MiB RSS; the
larger figure includes retained-history and image pressure. These are local
workload measurements, not a promise for every machine or session. Rendering
probes also enforce sustained throughput and pacing budgets.

Tool output and retained terminal records have explicit limits. Resuming streams
visible history from disk a message batch at a time; it does not build another
full transcript in memory. Idle and active-turn tests check CPU use and input
responsiveness, while compaction reduces the context sent on later model requests.

Run `scripts/sh/bench.sh > budgets.json` from a source checkout to measure your own
machine. The [performance probes](scripts/sh/bench.sh) define each workload and
threshold; provider latency and child-process memory are separate costs.

## Pick up where you left off

Use `/resume` to browse this workspace’s sessions, `crucible --continue` to open
the latest one, or `crucible --resume <id>` to select one directly. Scroll through
original prompts, answers and tool results, including the points where compaction
happened. Newly recorded edits keep their bounded diff previews; older sessions
retain only the change details they originally recorded.

Large sessions can offer a choice between summarizing model context and carrying
it whole. That choice affects the next request, while the visible conversation
remains available. See [sessions and compaction](docs/sessions/sessions.md).

## Control the workspace and the work

Choose a permission mode with `/mode`, inspect optional OS isolation with
`/sandbox`, and configure filesystem, network and command limits. Crucible can
search local files and the web, edit code, run tests, and use configured MCP tools
and skills.

Press **Ctrl+B** to leave a running command in the background. Completion is
reported automatically, so independent work can continue. Long tool results open
with **Ctrl+O** or a click. See [tools](docs/tools/index.md) and
[permissions](docs/permissions/index.md) for the controls behind these actions.

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

Google Gemini uses `GEMINI_API_KEY` or `/login google`; Google and Anthropic
accept API keys only, not product subscription logins.

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
[`AGENTS.md`](AGENTS.md), also exposed through the `CLAUDE.md` symlink.

Security issues must be reported through [`SECURITY.md`](SECURITY.md), not a
public issue. Participation is covered by the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

MIT — see [`LICENSE`](LICENSE).
