# Operating-system confinement

Every `bash` command crosses a host-owned sandbox service before any command
process starts. Tool arguments describe the command, not the backend. The host
resolves one immutable filesystem, network, environment, command and resource
plan; probes the backend; refuses missing hard capabilities; materializes inert
inputs; and only then launches the process.

OS confinement is disabled by default. Enable it with
`{"sandbox":{"enabled":true}}`; `enabled: false` in your home configuration
selects unconfined execution. Approval, minimal environment, command deadlines,
output bounds and audit remain active in either mode. Existing explicit
`sandbox.mode` settings retain their meaning, but cannot appear beside
`enabled` in the same document. Project settings can require confinement and
cannot disable it. See [configuration](../configuration/configuration.md#sandbox).

When enabled, the production Linux backend uses a
canonical, root-owned, non-writable system Bubblewrap executable reached only
through root-owned, non-writable parent directories. Every bounded `PATH`
candidate is tried until one passes its exact command-surface, numeric version,
namespace and SHA-256 identity probes. This release records that the bundled
choice is unavailable; it does not silently substitute a plain subprocess,
Landlock, a container, or a worktree. If no system candidate satisfies the
effective policy, preparation fails before materialization or spawn.

Bubblewrap 0.11.0 or newer is needed: the view is built from descriptor binds
and temporary overlays, which older releases do not offer. The probe checks the
options themselves rather than the version, because distributions backport some
of them. Ubuntu 24.04's 0.9.0 has the descriptor binds but not the overlays, so
there `required` confinement reports an unavailable backend until a newer
Bubblewrap is installed. Should a launch still be refused by the system
Bubblewrap, its own message is quoted in the error.

Inside the namespace, PID 1 is Crucible's own `crucible-sandbox-broker`
executable, shipped in every Linux release archive and installed beside the
Crucible binary, where it is found and pinned by open descriptor before the
namespace starts. It is accepted only when it and every directory
above it belong to root or to the user running Crucible and are writable by
neither group nor others; a copy under `/tmp`, in another user's directory or
below a group-writable directory is ignored, and `chmod g-w` on the offending
directory is the remedy.

The Linux view starts from an empty temporary root. It exposes only the minimal
read-only runtime needed to execute the selected absolute program, the exact
workspace/reached roots at their granted access, protected repository and
Crucible metadata carve-outs (including `.git`, `.agents`, `.codex`, and
`.crucible`), a minimal `/proc` and `/dev`, and a transactionally staged
manifest. Bounded unreadable patterns use a deliberately small `*`/single-`**`
grammar and one deterministic, no-symlink, no-mount-crossing tree scan. It
creates isolated user, PID, IPC, UTS and network namespaces, drops capabilities,
sets no-new-privileges through Bubblewrap, disables nested user namespaces,
clears the environment, and closes every undeclared file descriptor. Closed
networking has no usable host, loopback, Unix-socket, DNS, metadata-service or
inherited-socket route. Killing the namespace owner also kills descendants that
deliberately leave the original process group or session.

The exact endpoint allowlist is deliberately unsupported in this release. It
requires a policy-bound proxy or equivalent mechanism with redirect, DNS,
metadata, forwarding and outbound-byte enforcement. A requested exact rule is
rejected instead of being translated into broad egress.

## Platform support

| Platform | Enforcing backend in this version | When enabled |
| --- | --- | --- |
| Linux | Verified system Bubblewrap | Runs only when the backend satisfies the requested policy |
| macOS | Not implemented | Refuses command execution |
| Windows | Not implemented | Refuses command execution |

A normal unconfined command succeeding on macOS or Windows is not evidence of
sandbox support. Their native enforcing backends are required follow-up work;
this table describes the current implementation. An unavailable backend never
turns `enabled: true` into permission to run outside confinement.

SDK callers constructing `SandboxPolicy::standard` or `Bash::new` still receive
a required policy. The application applies its resolved configuration explicitly;
changing the application's default does not weaken independently constructed
SDK policies or inherited child authority.

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
| `cpu_limit` | enforced | unsupported |
| `memory_limit` | enforced | unsupported |
| `disk_limit` | unsupported | unsupported |
| `process_limit` | enforced on Linux 5.14 or newer | unsupported |
| `open_file_limit` | enforced | unsupported |
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

A capability being enforced says what a backend *can* apply, not what the
default policy asks for. Under `required`, the standard policy states
`cpu_limit` at one hour per process and `open_file_limit` at 4096; `bash` adds
`command_time_limit`, `output_limit` and `concurrency_limit` for the one command
it is running. `memory_limit` is enforced but not asked for: the knob is the
address space a process may map rather than the memory it uses, and runtimes
that reserve enormously and touch little would be refused by any ceiling low
enough to catch a real runaway. `disk_limit` is unsupported above, and a policy
may not ask for a ceiling its backend cannot apply — which is also why lowering
the mode takes the two confining ceilings off with it, rather than carrying
numbers the compatibility backend would have to refuse.

`process_limit` is the one ceiling a policy does not have to ask for. The broker
is PID 1 of the namespace, and it caps the processes beneath it at 1024 whether
or not anything states a number, the way it zeroes the core-dump ceiling: a
workload that forks in a loop is otherwise bounded by nothing the sandbox owns,
and the processor and descriptor ceilings do not help, because each new process
gets its own. A policy may state fewer and the broker takes the lower of the
two; it may not state more.

Stating one is what the table's row is about, and it needs a kernel that counts
processes per user namespace — Linux 5.14 and newer. Below that the kernel
counts them for the real user across the whole machine, so a stated ceiling
would bound the host's other work rather than the sandbox's, and `required`
refuses the policy instead of applying a number that means something else. The
broker's own 1024 still applies there, and under the older counting it can bind
before the sandbox has reached 1024 of its own — but the only thing it ever
stops is the sandbox forking. Nothing outside the namespace is ended by it.

The currently declared session surface is prepare, materialize, start, inspect,
read bounded output, observe usage/violations, stop and dispose. PTY, direct
file operations, persistence, snapshots and resume are absent rather than
stubs that run outside policy.

## Conformance

The table above is a statement, and nothing in a type system can check it. A
row that claims more than the backend keeps would put a command behind a fence
that is not there; a row that claims less would refuse work for no reason. So
the claims are asked about rather than trusted, by a suite Crucible publishes
rather than keeps to itself:

```rust
use crucible_tools::conformance::{Conformance, SandboxClaim};

let audited = Conformance::audit(&backend, workspace_root)?;
assert!(audited.faults().next().is_none());
assert!(audited.holds(SandboxClaim::Isolation));
println!("{}", audited.report());
```

For every feature a policy can name, the suite writes the smallest policy that
requires exactly that feature and offers it to the backend, in the mode that
reaches the backend that was probed. It then reads the claim and the answer as
one statement. A claimed feature must be accepted; a disclaimed one must be
refused *by that name*, before any session exists. Two answers are faults and
nothing else is: **overclaimed**, where a backend that says it cannot do
something takes the policy anyway, and **withheld**, where a backend refuses
what it says it can do. A backend that fails for its own reasons — no
executable, a kernel that will not give it namespaces — leaves the claim
**unreached**, because a host without a sandbox must not read as a backend that
lies.

Where there is no backend to probe at all, `audit` returns the probe's error
instead of a table. macOS and Windows have no enforcing backend, and a Linux
host may have no usable Bubblewrap, so a suite that reported those as faults
would be accusing an absent backend of overclaiming. Whether that error is a
skip or a failure belongs to the caller: Crucible's own jobs treat it as a skip
on a developer machine and a failure on the one job that installs a backend and
therefore requires it.

Some features cannot be asked for in a policy at all. A terminal, direct file
operations and resuming somebody else's session are asked of a session, and a
bare policy offered in their name would be accepted by everyone. Those are
reported **stated** or **absent** — read, not tested. Saying a claim was
exercised when nothing could exercise it is the same failure the suite exists
to catch.

The answers are grouped into the families a backend is chosen by: isolation,
materialization, network, resources, terminal, persistence, accounting and
cost. A backend that fences a filesystem perfectly and cannot bound one byte of
egress is not partly conformant — it holds one family and not another, and a
policy is matched against the family it needs. `holds` says a family is exact,
not that it is supported: a backend that disclaims every network feature and
refuses every network policy holds that family, and the claims are what tell a
caller it can do nothing there.

This is published because the backends that must pass it are not all in this
repository. A container, a Kubernetes executor, a hosted session or another
operating system's adapter can depend on `crucible-tools`, run the suite over a
directory it owns and get the same verdicts from the same table, before it is
wired to anything. Nothing is materialized and no command is started: each
session is prepared to see whether it can be, and dropped.

## Compatibility modes

Only home/user configuration may choose `degraded` or `off`; project and
descendant policy may preserve or strengthen that choice but never weaken it.
`degraded` tries the enforcing Linux backend first and uses the compatibility
backend only when enforcement is unavailable. `off` selects compatibility
directly. When compatibility is selected, inspection and audit records say
`confined: false`, name the degradation, and retain the exact compatibility
capability snapshot.

There is no enforcing backend on macOS, Windows or FreeBSD yet. Commands run
through compatibility by default and are reported as unconfined. Enabling the
sandbox on those systems refuses a command before it starts; it does not fall
back silently. A user can explicitly choose `sandbox.enabled: false` or the
legacy `sandbox.mode: degraded` to allow compatibility execution.

Compatibility still clears and explicitly rebuilds the command environment,
checks requested and transformed command guardrails, enforces command deadlines,
captured-output and concurrency ceilings, supervises its owned process scope, records
bounded usage and emits lifecycle audit facts. It does not restrict filesystem
or network reach and must not be described as a sandbox. Its process-isolation
capability is explicitly unsupported; unlike the enforcing PID-namespace
backend, it cannot promise containment of a hostile process that deliberately
escapes the owned process group.

## Environment and credentials

The command environment is an explicit, bounded map rather than a copy of the
host environment. Linux supplies only a private `HOME` and `TMPDIR` plus the
literal variables selected by the host. SSH/GPG agent sockets, inherited
descriptors, provider keys, cloud configuration and arbitrary host variables
do not cross the boundary automatically. Values reach the command through the
backend's cleared process environment, never through its argument list, which
every local user can read under `/proc` while a command runs.

A secret projection carries a bounded opaque credential handle and user/account
provenance alongside the host-resolved value. Handles and values are redacted
from debugging, inspection, audit, JSONL and diagnostics. Credential variables
share the ordinary environment count, name, uniqueness, NUL and aggregate-byte
bounds; a credential cannot silently replace a literal variable with the same
name.

## Lifecycle and inspection

Policy resolution, capability negotiation, materialization, both guardrail
decisions, command start/finish, violations, usage and cleanup are bounded typed
facts carrying the original run ancestry, tool call and sandbox ID. They are
written to the framework journal before their live events. Detached commands
retain the same fixed attribution; facts produced after the starting tool call
returns are drained at the next runner boundary.

Inspection retains backend ID/version/provenance, capability claims, separate
hashed requested and effective policies and redacted plans, manifest,
working-directory and root identities, root access/provenance, network shape,
requested limits, unreadable-pattern counts, command-policy digest, degradation
and cleanup state. Parent filesystem carve-outs, command filters, network
authority, resource ceilings, session grants and unreadable patterns cannot be
dropped or relabelled by a descendant. It does not retain command arguments,
environment values, endpoint names, raw approval proofs, credential handles or
values, proxy material or raw out-of-scope paths.

The local supervisor attempts to stop the owned process scope and reap its leader on normal
exit, deadline, output violation, cancellation, refusal, launch failure, panic,
explicit stop and ordinary host shutdown. On enforcing Linux a deadline or
output ceiling first tells the broker to end the workload, so the workload's
own wait status is still reported, and kills the launcher once the broker has
exited or after a short budget. Repeated stops retry unfinished cleanup within
bounded waits. Successful cleanup is idempotent; a failed attempt does not become
successful just because it is called again. Staging data and the command's
admission slot remain held until process cleanup is confirmed. If the process
owner is dropped while cleanup is still uncertain, staging data is retained and
the slot remains consumed for the lifetime of that sandbox service. A recorded
supervisor failure stays visible even if later cleanup releases those resources.

An uncatchable host/process kill cannot run user-space destructors. On enforcing
Linux, loss of the broker status channel and Bubblewrap's parent-death boundary
still terminate the PID-namespace workload. The next preparation replays the
checksummed lifecycle WAL, rolls back an unambiguous interrupted transaction,
and quarantines ambiguous publication instead of inventing cleanup or success.

## Reading the report

`crucible --sandbox` prints that inspection report for the directory you are
standing in and stops. No command is run to produce it: a session is negotiated,
asked what it settled on, and dropped. Nothing is materialized and nothing is
spawned.

```
sandbox required in /home/you/project
  backend   linux-bubblewrap bubblewrap 0.11.1, system
  build     sha256:0abea81db798ebf6b4742ac0664802d97521547a353c2a0dbdc21d76cbbfd2c0

what this backend can hold:
  filesystem            enforced
  network_deny          enforced
  network_allowlist     unsupported
  ...
  audit                 enforced
  usage                 observed

what a command would run under:
  mode      required
  cwd       sha256:1e3ffb52af5c198fe9cc1d10392d3a4160db588bedefa3a7e740dbc2fc84d9af
  reach     4 places, named by digest
    read_write  workspace           sha256:d6407f95fd7d98352f3068e2dbdf96c0bef1fc9147c28a70ac2c2dfb2ecdd169
    protected   protected_metadata  sha256:66c7ff1304b464fc8b146ef2eeb56306dcb7476982d7913c9418f51a9d6943d4
  hidden    0 patterns
  network   closed
  ceilings
    cpu 60m                 enforced
    4096 open files         enforced
  staged    nothing
  outlives  no
  snapshots no
  confined  yes
  gave up   nothing
  cleanup   pending; nothing was run and nothing was staged
  policy    sha256:51d8eb2751c012e44f287582830e063f5d960ef3cc896960e3555f84c90520d5
  manifest  sha256:72c169df3655f6857b163163b2618631ec65982557eea70b57ea1cb983ae0890
```

Every ceiling is printed beside the claim it rests on, because a number the
backend only observes is recorded after the fact rather than imposed, and the
two read identically until they are on one line. The capability matrix lists
every feature, including the ones this policy never asks for; it is what makes
an `enforced` elsewhere in the report mean something.

The workspace root is the only path printed. Roots, the working directory and
the policies are named by the digests the record keeps, so a report can be
pasted into an issue without pasting your tree with it. Two reports over the
same directory produce the same digests, which is what makes them comparable
between machines.

Where a backend answers but will not take this workspace's policy, the matrix is
printed and the refusal follows it — the unsupported feature above is usually
the whole explanation. Where nothing answers at all, the report says that rather
than printing a matrix of claims belonging to nobody. Neither is an error: both
are the answer, and the run ends successfully.

## Writable roots and publication

On enforcing Linux a writable root is not handed to the command directly. The
command writes into a private projection of that root, and the host publishes
the changed paths back only after the command has ended. Until then nothing the
command wrote is visible outside the sandbox, and a reader of the workspace sees
the root exactly as it was when the command started.

That projection is an overlay, and Bubblewrap 0.11 takes no mount options for
one, so it is the single mount inside the sandbox that would honour a setuid
bit or a device node. Neither is a way out. The namespace maps one unprivileged
identity and nothing above it, so a file the command marks setuid can only ever
name the identity it already runs as, and a `mknod` for a host disc is refused
because the command holds no capability to make a device with. Every other
mount, including the read-only runtime and the whole of `/dev` apart from the
device nodes themselves, is mounted `nosuid` and `nodev`. The tests assert the
behaviour rather than the flag, because the flag is the one thing here that
cannot be set.

Publication is decided by how the command ended:

- An ordinary exit, zero or nonzero, publishes the changed paths. A failing
  build still leaves the files it wrote, as it would have without confinement.
- Termination by a signal, a deadline, <kbd>Esc</kbd>, an output ceiling or a
  refusal discards the projection. Nothing partial reaches the workspace.
- A root that changed underneath the command, by anything outside the sandbox,
  is not published. The delta is discarded rather than merged, and the result
  says so.

Publication itself is transactional. The changed paths are staged in this
user's private sandbox state directory under `/var/tmp`, which no other user can
read or enter, journaled in a checksummed write-ahead log, applied, and
verified. A projected file larger than 8 GiB is refused rather than digested, so
a sparse file cannot make publication read through the whole of it. A
failure between those steps rolls the root back to its pinned baseline. Where a
rollback cannot itself be proved, the staged content is retained as quarantine
evidence and the cleanup outcome reports it, rather than deleting what cannot be
accounted for. Writable transactions are serialized under a host-owned registry
lock, so two commands never publish into the same root at once, and the next
preparation recovers any transaction an earlier process abandoned.

A detached command follows the same rules when it ends later. Its start result
is accepted only after it is durably stored, and its terminal publication is
journaled under the same call identity, so a host restart between the two
neither loses the command nor publishes it twice.

## Design references

The implementation review was pinned to OpenAI Codex
`dde85b435b16994f956bce08e5fb796ed94c27fd` and Philharmonica ADK
`df69de3411e78b61faf7bb4a4d641b02f53d0bc8`. Their mechanisms informed the
backend and capability seams; the public contracts, policy vocabulary and
journal behavior remain Crucible-owned.
