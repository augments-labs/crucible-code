---
paths:
  - "Cargo.toml"
  - "crates/**/Cargo.toml"
  - "deny.toml"
---

# Dependencies are `=`-pinned and justified

A new one needs a comment in `Cargo.toml` saying why it is needed;
`scripts/check.sh` fails without both.

Pinning is also what hides an advisory published afterwards, so `deny.toml` is
scanned on a clock instead — that check cannot live in a script whose whole
promise is the same answer for the same tree.
