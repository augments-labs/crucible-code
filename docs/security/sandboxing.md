# Operating-system confinement

Every `bash` command crosses a host-owned sandbox service before any command
process starts. Tool arguments describe the command, not the backend. The host
resolves one immutable filesystem, network, environment, command and resource
plan; probes the backend; refuses missing hard capabilities; materializes inert
inputs; and only then launches the process.

`sandbox.mode` defaults to `required`. The production Linux backend uses a
canonical, root-owned, non-writable system Bubblewrap executable whose command
surface, namespace support, version and SHA-256 identity are verified before
use. This release does not bundle Bubblewrap and does not silently substitute a
plain subprocess, Landlock, a container, or a worktree. If the system backend
cannot satisfy the effective policy, preparation fails before materialization
or spawn.

The Linux view starts from an empty temporary root. It exposes only the minimal
read-only runtime needed to execute the selected absolute program, the exact
workspace/reached roots at their granted access, protected repository and
Crucible metadata carve-outs, a minimal `/proc` and `/dev`, and a transactionally
staged manifest. It creates isolated user, PID, IPC, UTS and network namespaces,
drops capabilities, sets no-new-privileges through Bubblewrap, disables nested
user namespaces, clears the environment, and closes every undeclared file
descriptor. Closed networking has no usable host, loopback, Unix-socket, DNS,
metadata-service or inherited-socket route.

The exact endpoint allowlist is deliberately unsupported in this release. It
requires a policy-bound proxy or equivalent mechanism with redirect, DNS,
metadata, forwarding and outbound-byte enforcement. A requested exact rule is
rejected instead of being translated into broad egress.

## Capability matrix

`enforced` is a hard boundary, `observed` is bounded measurement only, and
`unsupported` is rejected whenever the effective policy explicitly requires
that feature. `degraded` and `off` relax only the documented baseline kernel
isolation; they cannot turn an explicit limit, manifest, exact network rule,
persistence request or snapshot request into best-effort behavior.

| Capability | Linux Bubblewrap | Compatibility |
| --- | --- | --- |
| `filesystem` | enforced | unsupported |
| `network_deny` | enforced | unsupported |
| `network_allowlist` | unsupported | unsupported |
| `descriptor_isolation` | enforced | unsupported |
| `process_isolation` | enforced | unsupported |
| `kernel_surface` | enforced | unsupported |
| `privilege_isolation` | enforced | unsupported |
| `materialization` | enforced | unsupported |
| `cpu_limit` | unsupported | unsupported |
| `memory_limit` | unsupported | unsupported |
| `disk_limit` | unsupported | unsupported |
| `process_limit` | unsupported | unsupported |
| `open_file_limit` | unsupported | unsupported |
| `command_time_limit` | enforced | enforced |
| `session_time_limit` | unsupported | unsupported |
| `outbound_byte_limit` | unsupported | unsupported |
| `output_limit` | enforced | enforced |
| `concurrency_limit` | enforced | enforced |
| `cost_limit` | unsupported | unsupported |
| `pty` | unsupported | unsupported |
| `file_operations` | unsupported | unsupported |
| `persistence` | unsupported | unsupported |
| `snapshot` | unsupported | unsupported |
| `resume` | unsupported | unsupported |
| `audit` | enforced | enforced |
| `usage` | observed | observed |

The currently declared session surface is prepare, materialize, start, inspect,
read bounded output, observe usage/violations, stop and dispose. PTY, direct
file operations, persistence, snapshots and resume are absent rather than
stubs that run outside policy.

## Compatibility modes

Only home/user configuration may choose `degraded` or `off`; project and
descendant policy may preserve or strengthen that choice but never weaken it.
`degraded` tries the enforcing Linux backend first and uses the compatibility
backend only when enforcement is unavailable. `off` selects compatibility
directly. Both inspection and audit records say `confined: false`, name the
degradation, and retain the exact compatibility capability snapshot.

Compatibility still clears and explicitly rebuilds the command environment,
checks requested and transformed command guardrails, enforces command deadlines,
captured-output and concurrency ceilings, supervises the process tree, records
bounded usage and emits lifecycle audit facts. It does not restrict filesystem
or network reach and must not be described as a sandbox.

## Lifecycle and inspection

Policy resolution, capability negotiation, materialization, both guardrail
decisions, command start/finish, violations, usage and cleanup are bounded typed
facts carrying the original run ancestry, tool call and sandbox ID. They are
written to the framework journal before their live events. Detached commands
retain the same fixed attribution; facts produced after the starting tool call
returns are drained at the next runner boundary.

Inspection retains backend ID/version/provenance, capability claims, hashed
policy/manifest/working-directory/root identities, root access/provenance,
network shape, requested limits, command-policy digest, degradation and cleanup
state. It does not retain command arguments, environment values, endpoint names,
raw approval proofs, credentials, proxy material or raw out-of-scope paths.

The local supervisor stops and reaps the complete owned process scope on normal
exit, deadline, output violation, cancellation, refusal, launch failure, panic,
explicit stop and ordinary host shutdown. Cleanup is idempotent. An
uncatchable host/process kill cannot run user-space destructors, so recovery
must treat a missing final cleanup fact as an interrupted lifecycle rather than
inventing success.
