# Security Policy

## Supported versions

While the project is on `0.0.x`, only the most recent release is supported.
Fixes ship forward as a new patch release; older tags are not patched.

## Reporting a vulnerability

**Do not open a public issue.**

Use GitHub's private reporting — the **Security** tab of this repository,
*Report a vulnerability*. If that is unavailable to you, email
<pnjoyim@augmentslabs.com>.

Useful to include: what an attacker gains, the affected version or commit, and
the smallest reproduction you have. A rough report sent early beats a polished
one sent late.

You can expect an acknowledgement within 72 hours and an assessment within seven
days. If a report is valid you will be credited in the release notes unless you
ask not to be.

Please give a reasonable window to ship a fix before disclosing publicly.

## What is in scope

crucible runs shell commands and edits files on the machine it is installed on —
that is its job, not a vulnerability. What matters is whether it does so
*without the user's consent*, or leaks what it was trusted with:

- A tool call that mutates a file or spawns a process without an approved
  permission decision, or a way to forge or bypass one.
- Path traversal that reaches outside the workspace the session was opened on.
- An API key or credential appearing in a log, an error message, a session file,
  a crash report or terminal output.
- Model output — or file contents fed to the model — that can escalate into
  command execution the user never approved.
- A crafted response from a provider that corrupts or executes through the
  parser.

## What is out of scope

- The user approving a destructive command themselves. Consent is the boundary;
  crucible does not second-guess an approved decision.
- Findings against a dependency with no path to exploit through crucible —
  report those upstream.
- Missing hardening with no demonstrated impact, and scanner output submitted
  without a working reproduction.
